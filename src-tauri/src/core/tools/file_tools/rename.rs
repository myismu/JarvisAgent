//! # rename.rs — 安全的文件重命名/移动
//!
//! 实现 agent 可调用的 `rename_file` 工具，在权限校验后重命名或移动指定文件。

use crate::core::rollback::Patch;
use crate::core::tools::framework;
use crate::core::tools::framework::permission::ensure_path_permission;

use super::common::resolve_path;
use super::workspace::{get_workspace, record_patch_to_snapshot};

pub async fn rename_file(
    app: &tauri::AppHandle,
    input: &serde_json::Value,
    session_id: &str,
) -> framework::ToolCallResult {
    let path = resolve_path(input);
    let new_path_raw = input["new_path"].as_str().unwrap_or("");
    if new_path_raw.is_empty() {
        return framework::ToolCallResult::error("错误: 缺少 new_path 参数".to_string());
    }
    let new_path_value = serde_json::json!({"path": new_path_raw});
    let new_path = resolve_path(&new_path_value);

    let ws = get_workspace(app, session_id).await;
    if let Err(e) = ensure_path_permission(app, &path, "重命名", ws.as_deref()).await {
        return framework::ToolCallResult::error(e);
    }
    if let Err(e) = ensure_path_permission(app, &new_path, "重命名", ws.as_deref()).await {
        return framework::ToolCallResult::error(e);
    }

    if !std::path::Path::new(&path).exists() {
        return framework::ToolCallResult::error(format!("文件不存在: {}", path));
    }
    if std::path::Path::new(&new_path).exists() {
        return framework::ToolCallResult::error(format!("目标路径已存在: {}", new_path));
    }

    match std::fs::rename(&path, &new_path) {
        Ok(()) => {
            record_patch_to_snapshot(
                app,
                session_id,
                Patch::RenameFile {
                    old_path: path.to_string(),
                    new_path: new_path.to_string(),
                },
                Some(format!("重命名 {} → {}", path, new_path)),
            )
            .await;
            framework::ToolCallResult::ok(format!("已将 {} 重命名为 {}", path, new_path))
        }
        Err(e) => framework::ToolCallResult::error(format!("重命名失败: {}", e)),
    }
}
