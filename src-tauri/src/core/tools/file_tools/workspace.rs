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
            let result = mgr
                .create_snapshot(vec![patch], message, None, None, None)
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
) -> Option<String> {
    if let Some(registry) = app.try_state::<SnapshotRegistry>() {
        let mgr_result = registry.0.read().await.get_or_create(session_id).await;
        if let Ok(mgr) = mgr_result {
            let result = mgr
                .create_checkpoint_snapshot(Some(message), None, None)
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
