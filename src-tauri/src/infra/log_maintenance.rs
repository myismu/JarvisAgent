//! # log_maintenance.rs — 日志分片、归档与保留期清理
//!
//! `data/logs/` 下的三类审计日志（`agent_loop` / `tool_calls` / `rollbacks`）都是
//! append-only JSONL，文件名规则统一为 `<日期>_<session_id>.jsonl`。长期使用会遇到两个问题：
//!
//! 1. **单个文件无限增长**：一个长会话（或多 loop 回合）能把单文件撑到几十 MB，
//!    查看器打开、grep、备份都变重；
//! 2. **旧日志永久堆积**：文本日志没有上限，磁盘只增不减。
//!
//! 本模块提供两件事：
//!
//! - [`resolve_part_file`]：**按大小分片**。写入前调用一次，文件超过阈值就切到
//!   `<日期>_<session>.2.jsonl`、`.3.jsonl` …（第一片沿用历史命名，老日志不受影响）。
//! - [`run_maintenance`]：**归档 + 保留期清理**。超过 N 天的原始日志用 gzip 压成
//!   `<目录>/archive/<原名>.gz` 后删除原文件；归档超过 M 天的 `.gz` 再删除。
//!
//! ## 与请求增量日志的约定（重要）
//!
//! `agent_loop` 的 `request_base` + `request_delta` 是**按文件**重建的：base 必须和它的
//! delta 落在同一个分片里。所以切分片（或跨天换文件）时，写端必须重新锚定一次
//! `request_base` + `request_full` —— 该判定在 `debug_logger` 里通过比对
//! `LastRequest.base_written_file` 与当前分片名完成，本模块只负责给出"当前该写哪个文件"。
//!
//! ## 配置（环境变量，留空用默认）
//!
//! | 变量 | 默认 | 含义 |
//! |---|---|---|
//! | `JARVIS_LOG_MAX_MB` | 8 | 单文件分片阈值（MB），`0` = 不分片 |
//! | `JARVIS_LOG_ARCHIVE_AFTER_DAYS` | 3 | 多少天前的原始日志压缩归档，`0` = 不归档 |
//! | `JARVIS_LOG_KEEP_DAYS` | 90 | 归档保留天数，`0` = 永久保留 |
//!
//! ## 约束
//!
//! - 归档/清理按**文件 mtime** 判定，不按文件名里的日期：仍在被追加写入的文件 mtime 是新的，
//!   不会被误压缩；
//! - 只有 `.gz` 校验通过（大小 > 0）才删除原文件，失败只记录错误、不动原文件；
//! - 分片与清理都不改变 append-only 语义：写入仍是"打开-追加-关闭"。

use std::ffi::{OsStr, OsString};
use std::fs::{self, File};
use std::io;
use std::path::Path;
use std::time::{Duration, SystemTime};

/// 默认分片阈值：8 MB
pub const DEFAULT_MAX_PART_BYTES: u64 = 8 * 1024 * 1024;
/// 默认归档年龄：3 天
pub const DEFAULT_ARCHIVE_AFTER_DAYS: u64 = 3;
/// 默认归档保留：90 天
pub const DEFAULT_KEEP_ARCHIVE_DAYS: u64 = 90;
/// 后台维护循环间隔
pub const MAINTENANCE_INTERVAL: Duration = Duration::from_secs(24 * 3600);
/// 分片数量上限（防御：正常情况下不可能到）
const MAX_PARTS: usize = 1000;
/// 受管理的日志子目录
const LOG_SUBDIRS: [&str; 3] = ["agent_loop", "tool_calls", "rollbacks"];
/// 归档目录名
const ARCHIVE_DIR: &str = "archive";

/// 日志保留策略
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct LogPolicy {
    /// 单个分片文件的大小上限（字节）；0 = 不分片
    pub max_part_bytes: u64,
    /// 原始日志多少天后压缩归档；0 = 不归档
    pub archive_after_days: u64,
    /// 归档文件保留多少天；0 = 永久保留
    pub keep_archive_days: u64,
}

impl Default for LogPolicy {
    fn default() -> Self {
        Self {
            max_part_bytes: DEFAULT_MAX_PART_BYTES,
            archive_after_days: DEFAULT_ARCHIVE_AFTER_DAYS,
            keep_archive_days: DEFAULT_KEEP_ARCHIVE_DAYS,
        }
    }
}

impl LogPolicy {
    /// 从环境变量读取策略（非法值回退默认）
    pub fn from_env() -> Self {
        let default = Self::default();
        Self {
            max_part_bytes: match env_u64("JARVIS_LOG_MAX_MB") {
                Some(mb) => mb.saturating_mul(1024 * 1024),
                None => default.max_part_bytes,
            },
            archive_after_days: env_u64("JARVIS_LOG_ARCHIVE_AFTER_DAYS")
                .unwrap_or(default.archive_after_days),
            keep_archive_days: env_u64("JARVIS_LOG_KEEP_DAYS").unwrap_or(default.keep_archive_days),
        }
    }
}

fn env_u64(name: &str) -> Option<u64> {
    std::env::var(name).ok()?.trim().parse::<u64>().ok()
}

/// 选择本次写入应落在哪个分片文件（返回文件名，调用方 join 到自己的目录）。
///
/// 命名：第一片 `<日期>_<session>.jsonl`（与历史日志一致），
/// 之后 `<日期>_<session>.2.jsonl`、`.3.jsonl` …
///
/// 判定：从第一片开始找，遇到"不存在"或"未超阈值"的文件就用它；都满了就继续往后。
pub fn resolve_part_file(
    dir: &Path,
    date: &str,
    session_id: &str,
    policy: &LogPolicy,
) -> OsString {
    let base = format!("{date}_{session_id}.jsonl");
    if policy.max_part_bytes == 0 {
        return OsString::from(base);
    }
    let mut current = base;
    for part in 1..=MAX_PARTS {
        let path = dir.join(&current);
        match fs::metadata(&path) {
            Ok(meta) if meta.len() >= policy.max_part_bytes => {
                current = format!("{date}_{session_id}.{}.jsonl", part + 1);
            }
            // 文件不存在（本片还没开始写）或还有空间 → 用它
            _ => return OsString::from(current),
        }
    }
    OsString::from(current)
}

/// 维护结果（用于打日志 / 断言）
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub struct MaintenanceReport {
    /// 本次压缩归档的原始日志数量
    pub archived: usize,
    /// 本次删除的过期归档数量
    pub pruned: usize,
    /// 归档前原始文件的总字节数（估算释放量）
    pub bytes_archived: u64,
    /// 出错次数（权限/占用等），不动原文件
    pub errors: usize,
}

impl MaintenanceReport {
    pub fn is_noop(&self) -> bool {
        self.archived == 0 && self.pruned == 0 && self.errors == 0
    }
}

/// 对应用日志根目录执行一次归档 + 清理
pub fn run_maintenance(policy: &LogPolicy) -> MaintenanceReport {
    run_maintenance_at(
        &crate::infra::config::data_paths::logs_dir(),
        SystemTime::now(),
        policy,
    )
}

/// 对指定日志根目录执行一次归档 + 清理（`now` 可注入，便于测试）
pub fn run_maintenance_at(root: &Path, now: SystemTime, policy: &LogPolicy) -> MaintenanceReport {
    let mut report = MaintenanceReport::default();
    for subdir in LOG_SUBDIRS {
        let dir = root.join(subdir);
        if !dir.is_dir() {
            continue;
        }
        let archive_dir = dir.join(ARCHIVE_DIR);
        if policy.archive_after_days > 0 {
            archive_old_files(&dir, &archive_dir, now, policy, &mut report);
        }
        if policy.keep_archive_days > 0 {
            prune_old_archives(&archive_dir, now, policy, &mut report);
        }
    }
    report
}

fn age_of(now: SystemTime, modified: SystemTime) -> Option<Duration> {
    now.duration_since(modified).ok()
}

fn is_older_than(now: SystemTime, modified: io::Result<SystemTime>, days: u64) -> bool {
    let Ok(modified) = modified else { return false };
    age_of(now, modified)
        .map(|age| age >= Duration::from_secs(days.saturating_mul(24 * 3600)))
        .unwrap_or(false)
}

/// 把 `dir` 下超过保留期的 `.jsonl` / `_summary.json` 压缩到 `dir/archive/*.gz` 并删除原文件
fn archive_old_files(
    dir: &Path,
    archive_dir: &Path,
    now: SystemTime,
    policy: &LogPolicy,
    report: &mut MaintenanceReport,
) {
    let Ok(entries) = fs::read_dir(dir) else {
        report.errors += 1;
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if !path.is_file() || !is_archivable(&path) {
            continue;
        }
        if !is_older_than(now, entry.metadata().and_then(|m| m.modified()), policy.archive_after_days)
        {
            continue;
        }
        let Some(name) = path.file_name() else { continue };
        let dest = archive_dir.join(format!("{}.gz", name.to_string_lossy()));
        let original_bytes = entry.metadata().map(|m| m.len()).unwrap_or(0);

        match gzip_file(&path, &dest) {
            Ok(()) => match fs::remove_file(&path) {
                Ok(()) => {
                    report.archived += 1;
                    report.bytes_archived += original_bytes;
                }
                Err(err) => {
                    // 压缩成功但删不掉：保留 .gz 与原文，下次再来
                    report.errors += 1;
                    eprintln!("[日志维护] 归档后删除失败 {}: {}", path.display(), err);
                }
            },
            Err(err) => {
                report.errors += 1;
                let _ = fs::remove_file(&dest); // 半成品不留
                eprintln!("[日志维护] 归档失败 {}: {}", path.display(), err);
            }
        }
    }
}

/// 允许归档的文件类型：JSONL 与汇总 JSON（不含已归档的 .gz）
fn is_archivable(path: &Path) -> bool {
    match path.file_name().and_then(OsStr::to_str) {
        Some(name) => {
            (name.ends_with(".jsonl") || name.ends_with(".json")) && !name.ends_with(".gz")
        }
        None => false,
    }
}

/// 删除归档目录里超过保留期的 `.gz`
fn prune_old_archives(
    archive_dir: &Path,
    now: SystemTime,
    policy: &LogPolicy,
    report: &mut MaintenanceReport,
) {
    let Ok(entries) = fs::read_dir(archive_dir) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        let is_gz = path
            .file_name()
            .and_then(OsStr::to_str)
            .map(|name| name.ends_with(".gz"))
            .unwrap_or(false);
        if !path.is_file() || !is_gz {
            continue;
        }
        if !is_older_than(now, entry.metadata().and_then(|m| m.modified()), policy.keep_archive_days)
        {
            continue;
        }
        match fs::remove_file(&path) {
            Ok(()) => report.pruned += 1,
            Err(err) => {
                report.errors += 1;
                eprintln!("[日志维护] 删除过期归档失败 {}: {}", path.display(), err);
            }
        }
    }
}

/// gzip 压缩单个文件；只有产物非空才算成功
fn gzip_file(src: &Path, dest: &Path) -> io::Result<()> {
    if let Some(parent) = dest.parent() {
        fs::create_dir_all(parent)?;
    }
    let mut input = File::open(src)?;
    let output = File::create(dest)?;
    let mut encoder = flate2::write::GzEncoder::new(output, flate2::Compression::default());
    io::copy(&mut input, &mut encoder)?;
    let output = encoder.finish()?;
    output.sync_all()?;
    if fs::metadata(dest)?.len() == 0 {
        return Err(io::Error::new(
            io::ErrorKind::Other,
            "压缩产物为空，视为失败",
        ));
    }
    Ok(())
}

/// 启动后台维护：立即跑一次，之后每 24h 一次。
///
/// 放在后台线程里，避免阻塞启动；失败只打日志，绝不影响主流程。
pub fn spawn_maintenance_loop() -> std::io::Result<()> {
    let policy = LogPolicy::from_env();
    std::thread::Builder::new()
        .name("jarvis-log-maintenance".to_string())
        .spawn(move || loop {
            let report = run_maintenance(&policy);
            if !report.is_noop() {
                println!(
                    "[日志维护] 归档 {} 个文件（{} 字节）、清理过期归档 {} 个、错误 {} 次；策略: 分片 {}MB / {} 天归档 / 保留 {} 天",
                    report.archived,
                    report.bytes_archived,
                    report.pruned,
                    report.errors,
                    policy.max_part_bytes / (1024 * 1024),
                    policy.archive_after_days,
                    policy.keep_archive_days,
                );
            }
            std::thread::sleep(MAINTENANCE_INTERVAL);
        })
        .map(|_| ())
}

// ───────────────────────── 测试 ─────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::{Read, Write};
    use std::path::PathBuf;

    fn temp_root(tag: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "jarvis-log-maintenance-{}-{}",
            tag,
            uuid::Uuid::new_v4()
        ));
        fs::create_dir_all(&dir).expect("create temp root");
        dir
    }

    fn write_file(path: &Path, content: &str) {
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        let mut file = File::create(path).unwrap();
        file.write_all(content.as_bytes()).unwrap();
    }

    #[test]
    fn resolve_part_file_keeps_first_part_until_threshold() {
        let root = temp_root("part1");
        let policy = LogPolicy {
            max_part_bytes: 16,
            ..LogPolicy::default()
        };

        // 文件不存在 → 用第一片
        assert_eq!(
            resolve_part_file(&root, "2026-09-15", "s1", &policy),
            OsString::from("2026-09-15_s1.jsonl")
        );

        // 未超阈值 → 仍是第一片
        write_file(&root.join("2026-09-15_s1.jsonl"), "0123456789");
        assert_eq!(
            resolve_part_file(&root, "2026-09-15", "s1", &policy),
            OsString::from("2026-09-15_s1.jsonl")
        );

        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn resolve_part_file_rolls_over_when_full() {
        let root = temp_root("part2");
        let policy = LogPolicy {
            max_part_bytes: 8,
            ..LogPolicy::default()
        };
        write_file(&root.join("2026-09-15_s1.jsonl"), "0123456789"); // 10 > 8 → 满
        assert_eq!(
            resolve_part_file(&root, "2026-09-15", "s1", &policy),
            OsString::from("2026-09-15_s1.2.jsonl")
        );
        write_file(&root.join("2026-09-15_s1.2.jsonl"), "0123456789"); // 也满
        assert_eq!(
            resolve_part_file(&root, "2026-09-15", "s1", &policy),
            OsString::from("2026-09-15_s1.3.jsonl")
        );
        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn resolve_part_file_disabled_keeps_single_file() {
        let root = temp_root("part3");
        let policy = LogPolicy {
            max_part_bytes: 0,
            ..LogPolicy::default()
        };
        write_file(&root.join("2026-09-15_s1.jsonl"), &"x".repeat(1024));
        assert_eq!(
            resolve_part_file(&root, "2026-09-15", "s1", &policy),
            OsString::from("2026-09-15_s1.jsonl")
        );
        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn maintenance_archives_old_logs_and_keeps_content() {
        let root = temp_root("archive");
        let dir = root.join("agent_loop");
        let content = "{\"type\":\"request_base\"}\n{\"type\":\"request_delta\"}\n";
        write_file(&dir.join("2026-09-01_s1.jsonl"), content);

        // 把"现在"推到 10 天后 → 该文件看起来已 10 天未写
        let now = SystemTime::now() + Duration::from_secs(10 * 24 * 3600);
        let policy = LogPolicy {
            archive_after_days: 3,
            keep_archive_days: 90,
            ..LogPolicy::default()
        };
        let report = run_maintenance_at(&root, now, &policy);

        assert_eq!(report.archived, 1, "应归档 1 个文件");
        assert_eq!(report.errors, 0);
        assert!(!dir.join("2026-09-01_s1.jsonl").exists(), "原文件应删除");

        let gz = dir.join("archive/2026-09-01_s1.jsonl.gz");
        assert!(gz.exists(), "应生成 .gz 归档");

        // 解压回来验证内容无损
        let mut decoder =
            flate2::read::GzDecoder::new(File::open(&gz).expect("open gz"));
        let mut restored = String::new();
        decoder.read_to_string(&mut restored).expect("gunzip");
        assert_eq!(restored, content);

        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn maintenance_keeps_fresh_logs_and_prunes_expired_archives() {
        let root = temp_root("retention");
        let dir = root.join("tool_calls");
        write_file(&dir.join("2026-09-14_s2.jsonl"), "{}\n");
        write_file(&dir.join("archive/2026-01-01_s0.jsonl.gz"), "old");

        // 1) 刚写的文件不该被归档
        let report = run_maintenance_at(&root, SystemTime::now(), &LogPolicy::default());
        assert_eq!(report.archived, 0);
        assert!(dir.join("2026-09-14_s2.jsonl").exists());

        // 2) 归档超过保留期（90 天）→ 删除
        let far_future = SystemTime::now() + Duration::from_secs(200 * 24 * 3600);
        let report = run_maintenance_at(&root, far_future, &LogPolicy::default());
        assert!(report.pruned >= 1, "过期归档应被清理");
        assert!(!dir.join("archive/2026-01-01_s0.jsonl.gz").exists());

        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn maintenance_can_be_disabled() {
        let root = temp_root("disabled");
        let dir = root.join("rollbacks");
        write_file(&dir.join("2020-01-01_s1.jsonl"), "{}\n");
        let policy = LogPolicy {
            archive_after_days: 0,
            keep_archive_days: 0,
            max_part_bytes: 0,
        };
        let report = run_maintenance_at(
            &root,
            SystemTime::now() + Duration::from_secs(999 * 24 * 3600),
            &policy,
        );
        assert_eq!(report, MaintenanceReport::default());
        assert!(dir.join("2020-01-01_s1.jsonl").exists(), "关闭时不应动文件");
        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn policy_reads_env_with_defaults() {
        // 环境变量在测试进程里可能被并发改动，这里只断言"默认值可用且自洽"
        let policy = LogPolicy::default();
        assert_eq!(policy.max_part_bytes, 8 * 1024 * 1024);
        assert_eq!(policy.archive_after_days, 3);
        assert_eq!(policy.keep_archive_days, 90);
    }
}
