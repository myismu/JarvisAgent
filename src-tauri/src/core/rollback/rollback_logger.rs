//! # rollback_logger.rs — 回滚操作审计日志模块
//!
//! 记录原子回滚中每个文件的执行状态，用于诊断"部分文件回滚失败"问题。
//! 日志以 JSONL 格式写入 `data/logs/rollbacks/` 目录，按 session 隔离。
//! 配套 HTML 查看器：`data/logs/log-viewer.html`（统一审计中心）
//!
//! ## Key Exports
//! - `RollbackLogger`: 回滚过程的结构化日志记录器
//! - `FileOpRecord`: 单个文件操作的审计记录
//! - `RollbackSummary`: 本次回滚的汇总统计
//!
//! ## Constraints
//! - append-only 写入，不持有文件句柄
//! - args/content 等大字段不写入日志，只记录 path/action/size 等元信息

use std::fs::OpenOptions;
use std::io::Write;
use std::path::PathBuf;
use std::sync::OnceLock;

use serde::Serialize;

// ───────────────────────── 日志记录结构 ─────────────────────────

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum RollbackPhase {
    /// 写入 staging 目录
    Staging,
    /// 从 staging 应用到目标路径
    Apply,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum FileAction {
    Create,
    Update,
    Delete,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum FileOpStatus {
    Ok,
    Error,
    Skip,
}

#[derive(Debug, Clone, Serialize)]
pub struct FileOpRecord {
    pub ts: String,
    pub session_id: String,
    pub rollback_seq: u32,
    pub phase: RollbackPhase,
    pub path: String,
    pub action: FileAction,
    pub status: FileOpStatus,
    /// 文件大小（字节），仅 Create/Update
    #[serde(skip_serializing_if = "Option::is_none")]
    pub size_bytes: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
    /// 操作耗时（毫秒）
    pub duration_ms: u64,
}

#[derive(Debug, Clone, Serialize)]
pub struct RollbackSummary {
    pub ts: String,
    pub session_id: String,
    pub total: u32,
    pub success: u32,
    pub skipped: u32,
    pub failed: u32,
    pub total_duration_ms: u64,
    /// 失败文件路径列表
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub failed_files: Vec<String>,
}

// ───────────────────────── Logger ─────────────────────────

pub struct RollbackLogger {
    log_dir: PathBuf,
    policy: crate::infra::log_maintenance::LogPolicy,
}

impl RollbackLogger {
    pub fn new() -> Self {
        let log_dir = crate::infra::config::data_paths::logs_dir().join("rollbacks");
        let _ = std::fs::create_dir_all(&log_dir);
        Self {
            log_dir,
            policy: crate::infra::log_maintenance::LogPolicy::from_env(),
        }
    }

    /// 记录单个文件操作
    pub fn log_file_op(&self, record: &FileOpRecord) {
        let date = chrono::Local::now().format("%Y-%m-%d").to_string();
        // 按大小分片：超过阈值自动写 <date>_<session>.2.jsonl、.3.jsonl …
        let filename = crate::infra::log_maintenance::resolve_part_file(
            &self.log_dir,
            &date,
            &record.session_id,
            &self.policy,
        );
        let path = self.log_dir.join(filename);

        if let Ok(mut file) = OpenOptions::new().create(true).append(true).open(&path) {
            if let Ok(line) = serde_json::to_string(record) {
                let _ = writeln!(file, "{}", line);
            }
        }
    }

    /// 写入本次回滚的汇总统计
    pub fn flush_summary(&self, summary: &RollbackSummary) {
        let date = chrono::Local::now().format("%Y-%m-%d").to_string();
        let filename = format!("{}_{}_summary.json", date, summary.session_id);
        let path = self.log_dir.join(filename);

        if let Ok(mut file) = OpenOptions::new()
            .create(true)
            .write(true)
            .truncate(true)
            .open(&path)
        {
            if let Ok(json) = serde_json::to_string_pretty(summary) {
                let _ = writeln!(file, "{}", json);
            }
        }
    }
}

impl Default for RollbackLogger {
    fn default() -> Self {
        Self::new()
    }
}

// ───────────────────────── 全局单例 ─────────────────────────

static LOGGER: OnceLock<RollbackLogger> = OnceLock::new();

pub fn rollback_logger() -> &'static RollbackLogger {
    LOGGER.get_or_init(RollbackLogger::new)
}
