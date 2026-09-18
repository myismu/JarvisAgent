//! 项目级授权账本（落盘持久化）。
//!
//! 「本次会话都允许」的语义升级为「本项目允许」后，会话允许键在登记进内存
//! （`grant_session_allowance`）的同时落盘到安装目录 `data/permissions/`，
//! 下次会话挂载同一项目时自动加载注入——用户点过的每次允许成为项目的永久资产。
//! 见 `doc/权限拦截-授权持久化方案（审计点+项目级落盘+三键卡片）.md`。
//!
//! 为什么存安装目录而不存项目内（方案 §2.1）：项目根是 agent 的可写区域，
//! 权限文件放在里面等于允许 agent 改写自己的授权（自授权风险）；安装目录在
//! 沙箱边界外，文件工具硬 Deny、命令需审批，账本天然免疫篡改。
//!
//! 键结构与内存 `SessionAllowance` 同构（kind/scope/label），**粒度零改动**，
//! 只把生命周期从"会话"延长为"项目"。
//!
//! 文件名 = 转义后的项目路径（Windows 非法字符换下划线，中文保留，肉眼可辨认），
//! 不引入 hash 依赖；超长截断兜底 MAX_PATH，文件内 `project_path` 存真身。

use std::fs;
use std::path::{Path, PathBuf};
use std::sync::OnceLock;

use serde::{Deserialize, Serialize};

use crate::infra::state::state::{SessionAllowance, SessionContext};

/// 单条落盘授权（比内存结构多来源信息，供管理面板展示）
#[derive(Serialize, Deserialize, Clone)]
struct StoredAllowance {
    kind: String,
    scope: String,
    label: String,
    /// 来源会话（哪次点击登记的）
    source_session: String,
    created_at: String,
}

#[derive(Serialize, Deserialize)]
struct ProjectAllowanceFile {
    /// 原始项目路径（文件名是转义形式，这里存真身供展示与核对）
    project_path: String,
    updated_at: String,
    keys: Vec<StoredAllowance>,
}

/// 账本目录：安装目录 data/permissions/
fn store_dir() -> PathBuf {
    crate::infra::config::data_paths::data_root().join("permissions")
}

/// 项目路径 → 账本文件名。
fn file_name_for(project_root: &Path) -> String {
    let raw = project_root.to_string_lossy();
    let mut escaped: String = raw
        .chars()
        .map(|c| {
            if matches!(c, '<' | '>' | ':' | '"' | '/' | '\\' | '|' | '?' | '*') {
                '_'
            } else {
                c
            }
        })
        .collect();
    if escaped.chars().count() > 150 {
        escaped = escaped.chars().take(150).collect();
    }
    format!("{}.json", escaped)
}

/// 同一账本文件的并发读改写串行化（同项目多会话同时点「本项目允许」）
static STORE_LOCK: OnceLock<tokio::sync::Mutex<()>> = OnceLock::new();
fn store_lock() -> &'static tokio::sync::Mutex<()> {
    STORE_LOCK.get_or_init(|| tokio::sync::Mutex::new(()))
}

fn now_string() -> String {
    chrono::Local::now().to_rfc3339()
}

/// 读账本文件（不存在或损坏 → 空账本；损坏不覆盖，等下一次 persist 时重写）
fn read_file(path: &Path) -> ProjectAllowanceFile {
    if let Ok(text) = fs::read_to_string(path) {
        if let Ok(file) = serde_json::from_str::<ProjectAllowanceFile>(&text) {
            return file;
        }
    }
    ProjectAllowanceFile {
        project_path: String::new(),
        updated_at: now_string(),
        keys: Vec::new(),
    }
}

/// temp + rename 原子替换：写一半崩溃不会留下半截 JSON
fn write_file(path: &Path, file: &ProjectAllowanceFile) {
    if let Some(parent) = path.parent() {
        let _ = fs::create_dir_all(parent);
    }
    let tmp = path.with_extension("json.tmp");
    if let Ok(text) = serde_json::to_string_pretty(file) {
        if fs::write(&tmp, text).is_ok() {
            let _ = fs::rename(&tmp, path);
        }
    }
}

/// 会话挂载项目后调用：把该项目账本里的授权注入内存会话允许列表。
/// 注入走与 `grant_session_allowance` 相同的去重口径（kind+scope 已存在则跳过）。
pub async fn load_into(ctx: &SessionContext, project_root: &Path) {
    let path = store_dir().join(file_name_for(project_root));
    let file = read_file(&path);
    if file.keys.is_empty() {
        return;
    }
    let mut list = ctx.session_allowances.lock().await;
    for stored in &file.keys {
        if !list
            .iter()
            .any(|a| a.kind == stored.kind && a.scope == stored.scope)
        {
            list.push(SessionAllowance {
                kind: stored.kind.clone(),
                scope: stored.scope.clone(),
                label: stored.label.clone(),
            });
        }
    }
}

/// 点「本项目允许」时调用（挂在 `grant_session_allowance` 内，唯一口径覆盖全部
/// 登记路径）。没有挂载项目（ctx.workspace 为 None）时跳过——非沙盒会话没有
/// 可归属的项目。失败只影响"下次记住"，不影响本次放行，错误仅打印不上抛。
pub async fn persist(ctx: &SessionContext, session_id: &str, kind: &str, scope: &str, label: &str) {
    let project_root = match ctx.workspace.lock().await.clone() {
        Some(ws) => ws,
        None => return,
    };
    let _guard = store_lock().lock().await;
    let path = store_dir().join(file_name_for(&project_root));
    let mut file = read_file(&path);
    if file.project_path.is_empty() {
        file.project_path = project_root.to_string_lossy().to_string();
    }
    if file
        .keys
        .iter()
        .any(|a| a.kind == kind && a.scope == scope)
    {
        return; // 已存在，无需重写
    }
    file.keys.push(StoredAllowance {
        kind: kind.to_string(),
        scope: scope.to_string(),
        label: label.to_string(),
        source_session: session_id.to_string(),
        created_at: now_string(),
    });
    file.updated_at = now_string();
    write_file(&path, &file);
    println!(
        "[JARVIS] 授权已记入项目账本: {} | {} / {}",
        file_name_for(&project_root),
        kind,
        scope
    );
}

/// 撤销一条授权时同步删账本（「撤销」按钮连盘上条目一起清）。
pub async fn revoke(ctx: &SessionContext, kind: &str, scope: &str) {
    let project_root = match ctx.workspace.lock().await.clone() {
        Some(ws) => ws,
        None => return,
    };
    let _guard = store_lock().lock().await;
    let path = store_dir().join(file_name_for(&project_root));
    let mut file = read_file(&path);
    let before = file.keys.len();
    file.keys
        .retain(|a| !(a.kind == kind && a.scope == scope));
    if file.keys.len() != before {
        file.updated_at = now_string();
        write_file(&path, &file);
    }
}

/// 清空本项目账本（「清空」按钮）。
pub async fn clear(ctx: &SessionContext) {
    let project_root = match ctx.workspace.lock().await.clone() {
        Some(ws) => ws,
        None => return,
    };
    let _guard = store_lock().lock().await;
    let path = store_dir().join(file_name_for(&project_root));
    let _ = fs::remove_file(path);
}
