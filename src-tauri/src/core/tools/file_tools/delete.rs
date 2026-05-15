//! # delete.rs — 安全的文件删除
//!
//! 实现 agent 可调用的 `delete_file` 工具，在权限校验后删除指定文件，并记录快照。

use std::time::{SystemTime, UNIX_EPOCH};

use crate::core::rollback::Patch;
use crate::core::tools::framework::permission::ensure_path_permission;

use super::common::{read_text_preserve_encoding, resolve_path};
use super::workspace::{get_workspace, record_patch_to_snapshot};

pub async fn delete_file(
    app: &tauri::AppHandle,
    input: &serde_json::Value,
    session_id: &str,
) -> String {
    let path = resolve_path(input);
    let ws = get_workspace(app, session_id).await;
    if let Err(e) = ensure_path_permission(app, path, "删除", ws.as_deref()).await {
        return e;
    }

    if !std::path::Path::new(path).exists() {
        return format!("文件不存在: {}", path);
    }

    // 删除前备份原内容到 snapshot_content，快照只存 hash
    let content_hash = read_text_preserve_encoding(path).ok().map(|d| {
        let hash = Patch::content_hash(&d.content);
        let _ = crate::core::rollback::store::save_content(session_id, &hash, &d.content);
        hash
    });

    let parent = std::path::Path::new(path)
        .parent()
        .unwrap_or_else(|| std::path::Path::new("."));
    let filename = std::path::Path::new(path)
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or("unknown");
    let ts = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs();
    let trash_dir = parent.join(".jarvis_trash");
    if let Err(e) = std::fs::create_dir_all(&trash_dir) {
        return format!("删除失败，无法创建回收目录: {}", e);
    }
    let trash_path = trash_dir.join(format!("{}_{}", ts, filename));

    match std::fs::rename(path, &trash_path) {
        Ok(()) => {
            record_patch_to_snapshot(
                app,
                session_id,
                Patch::DeleteFile {
                    path: path.to_string(),
                    content_hash,
                },
                Some(format!("删除 {} → {}", path, trash_path.display())),
            )
            .await;
            format!("已删除文件: {}（移至 {})", path, trash_path.display())
        }
        Err(e) => format!("删除失败: {}", e),
    }
}
