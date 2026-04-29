//! # write.rs — 执行安全的普通文本文件写入
//!
//! 实现 agent 可调用的 `write_file` 工具，在权限校验后创建或覆盖普通文本文件，并同步记录检查点与快照；写入前统一行尾并进行 TOCTOU 防护。
//!
//! ## Key Exports
//! - `write_file()`: 创建或覆盖普通文本文件内容
//!
//! ## Dependencies
//! - Internal: `crate::core::session::checkpoint`, `crate::core::snapshot_engine`, `crate::core::tools::permission`, `super::notebook_guard`
//!
//! ## Constraints
//! - 不得用于 `.ipynb` 或 notebook-shaped JSON，Notebook 必须通过 cell 级工具修改

use crate::core::session::checkpoint::{self, FileOperation, OpType};
use crate::core::snapshot_engine::Patch;
use crate::core::tools::permission::ensure_path_permission;

use super::common::{is_locked_file_error, normalize_line_endings};
use super::diff::compute_diff;
use super::notebook_guard::{
    is_notebook_path, looks_like_notebook_json, notebook_text_edit_rejection,
};
use super::workspace::{get_workspace, record_operation, record_patch_to_snapshot};

/// 写入文件（自动备份原始内容 + 自动创建快照）
pub async fn write_file(
    app: &tauri::AppHandle,
    input: &serde_json::Value,
    session_id: &str,
) -> String {
    let path = input["path"].as_str().unwrap_or("");
    let content = input["content"].as_str().unwrap_or("");
    // 统一行尾为 LF，避免 CRLF/LF 混乱
    let content = normalize_line_endings(content);
    let ws = get_workspace(app, session_id).await;
    if let Err(e) = ensure_path_permission(app, path, "写入", ws.as_deref()).await {
        return e;
    }
    if is_notebook_path(path) || looks_like_notebook_json(&content) {
        return notebook_text_edit_rejection(path);
    }

    let branch = checkpoint::get_active_branch(session_id);
    let branch_name = branch.name;

    let file_exists = std::path::Path::new(path).exists();
    let old_content = if file_exists {
        std::fs::read_to_string(path).ok()
    } else {
        None
    };

    // TOCTOU 防护：记录读取时的 mtime
    let read_mtime = if file_exists {
        std::fs::metadata(path).ok().and_then(|m| m.modified().ok())
    } else {
        None
    };

    let old_content_bytes = old_content.as_ref().map(|s| s.as_bytes().to_vec());
    let old_content_hash = old_content_bytes
        .as_ref()
        .map(|c| checkpoint::content_hash(c));
    let backup_path = if let Some(ref content_bytes) = old_content_bytes {
        checkpoint::backup_file(session_id, &branch_name, path, content_bytes)
    } else {
        None
    };

    let new_content_hash = Some(checkpoint::content_hash(content.as_bytes()));

    let op_type = if file_exists {
        OpType::Write
    } else {
        OpType::Create
    };

    let operation = FileOperation {
        op_type,
        path: path.to_string(),
        old_content_hash,
        backup_path,
        new_content_hash,
        diff_summary: None,
    };

    // TOCTOU 防护：写入前检查文件是否在读取后被外部修改
    if let (Some(orig_mtime), Ok(current_meta)) = (read_mtime, std::fs::metadata(path)) {
        if let Ok(current_mtime) = current_meta.modified() {
            if current_mtime != orig_mtime {
                return format!(
                    "写入中止: 文件 {} 在读取后被外部修改。请重新读取后再写入。",
                    path
                );
            }
        }
    }

    match std::fs::write(path, content.as_str()) {
        Ok(_) => {
            record_operation(app, session_id, operation).await;

            let patch = match &old_content {
                None => Patch::CreateFile {
                    path: path.to_string(),
                    content: content.to_string(),
                },
                Some(old) => Patch::UpdateFile {
                    path: path.to_string(),
                    old_content: old.clone(),
                    new_content: content.to_string(),
                    diff: Some(compute_diff(old, &content)),
                },
            };
            let action = if file_exists { "写入" } else { "创建" };
            let msg = Some(format!("{} {}", action, path));
            record_patch_to_snapshot(app, session_id, patch, msg).await;

            format!("成功{} {}", action, path)
        }
        Err(e) => {
            let err_msg = e.to_string();
            if is_locked_file_error(&err_msg) {
                format!(
                    "写入失败: 文件被其他智能体或程序锁定，请稍后重试。详细错误: {}",
                    e
                )
            } else {
                format!("写入失败: {}", e)
            }
        }
    }
}
