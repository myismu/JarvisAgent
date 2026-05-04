//! # workspace.rs — 会话工作区与快照记录桥接
//!
//! 为文件工具提供当前会话工作目录查询，以及写入 rollback 后通知前端的统一入口。
//!
//! ## Key Exports
//! - `get_workspace()`: 获取当前会话绑定的工作目录沙箱
//! - `record_patch_to_snapshot()`: 创建文件快照并发出 `snapshot-created` 事件
//! - `commit_checkpoint_snapshot()`: 创建一轮 Agent 执行结束的检查点快照
//!
//! ## Dependencies
//! - Internal: `crate::core::state::SessionManager`, `crate::core::SnapshotRegistry`, `crate::core::rollback`
//! - External: `tauri`

use tauri::{Emitter, Manager};

use crate::core::models::Message;
use crate::core::rollback::Patch;
use crate::core::SnapshotRegistry;

/// 获取当前会话的工作目录沙箱
pub(super) async fn get_workspace(
    app: &tauri::AppHandle,
    session_id: &str,
) -> Option<std::path::PathBuf> {
    if let Some(manager) = app.try_state::<crate::core::state::SessionManager>() {
        let ctx = manager.get_or_create(session_id).await;
        let ws = ctx.workspace.lock().await.clone();
        return ws;
    }
    None
}

/// 查找最后一条"真正的用户输入"消息（跳过工具结果等内部消息）
fn latest_user_message_index(messages: &[Message]) -> Option<usize> {
    use crate::core::models::{Content, ContentBlock};

    messages.iter().enumerate().rev().find_map(|(index, message)| {
        if let Message::User { content } = message {
            // 跳过仅包含 ToolResult 的内部消息
            let is_tool_result_only = match content {
                Content::Multiple(blocks) => blocks.iter().all(|b| matches!(b, ContentBlock::ToolResult { .. })),
                Content::Single(text) => {
                    // 跳过后台通知消息
                    let trimmed = text.trim();
                    trimmed.starts_with("<background-results>") || trimmed.starts_with("<background-results")
                }
            };
            if is_tool_result_only {
                return None;
            }
            Some(index)
        } else {
            None
        }
    })
}

async fn active_user_message_index(app: &tauri::AppHandle, session_id: &str) -> Option<usize> {
    if let Some(manager) = app.try_state::<crate::core::state::SessionManager>() {
        let ctx = manager.get_or_create(session_id).await;
        let session = ctx.memory.lock().await;
        return latest_user_message_index(&session.messages);
    }
    crate::core::session::load_session(session_id)
        .ok()
        .and_then(|session| latest_user_message_index(&session.messages))
}

/// 将文件变更记录为快照
pub(super) async fn record_patch_to_snapshot(
    app: &tauri::AppHandle,
    session_id: &str,
    patch: Patch,
    message: Option<String>,
) {
    let sid = session_id;
    if let Some(registry) = app.try_state::<SnapshotRegistry>() {
        let mgr_result = registry.0.read().await.get_or_create(&sid).await;
        if let Ok(mgr) = mgr_result {
            let trigger_user_memory_index = active_user_message_index(app, sid).await;
            let result = mgr
                .create_snapshot(vec![patch], message, None, None, None, trigger_user_memory_index)
                .await;
            if let Ok(snapshot) = result {
                let _ = app.emit(
                    "snapshot-created",
                    serde_json::json!({
                        "sessionId": sid,
                        "snapshotId": snapshot.id
                    }),
                );
            }
        }
    }
}

/// 检查自上次 checkpoint 以来是否有新的文件补丁
pub async fn has_patches_since_last_checkpoint(
    app: &tauri::AppHandle,
    session_id: &str,
) -> bool {
    if let Some(registry) = app.try_state::<SnapshotRegistry>() {
        let mgr_result = registry.0.read().await.get_or_create(session_id).await;
        if let Ok(mgr) = mgr_result {
            return mgr.count_patches_since_last_checkpoint().await > 0;
        }
    }
    false
}

/// 创建一轮 Agent 执行结束的检查点快照（is_checkpoint = true）
pub async fn commit_checkpoint_snapshot(
    app: &tauri::AppHandle,
    session_id: &str,
    message: String,
    trigger_user_memory_index: Option<usize>,
) -> Option<String> {
    if let Some(registry) = app.try_state::<SnapshotRegistry>() {
        let mgr_result = registry.0.read().await.get_or_create(session_id).await;
        if let Ok(mgr) = mgr_result {
            let result = mgr
                .create_checkpoint_snapshot(Some(message), None, None, trigger_user_memory_index)
                .await;
            if let Ok(snapshot) = result {
                let snapshot_id = snapshot.id.clone();
                let patch_count = snapshot
                    .metadata
                    .get("patch_count")
                    .and_then(|v| v.parse::<usize>().ok())
                    .unwrap_or(0);
                let _ = app.emit(
                    "checkpoint-created",
                    serde_json::json!({
                        "sessionId": session_id,
                        "checkpointId": snapshot.id,
                        "hasOperations": patch_count > 0,
                        "message": snapshot.message
                    }),
                );
                return Some(snapshot_id);
            }
        }
    }
    None
}
