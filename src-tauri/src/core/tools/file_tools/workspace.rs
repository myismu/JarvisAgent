//! # workspace.rs — 会话工作区、检查点与快照记录桥接
//!
//! 为文件工具提供当前会话工作目录查询、待提交文件操作记录，以及写入 snapshot_engine 后通知前端的统一入口。
//!
//! ## Key Exports
//! - `get_workspace()`: 获取当前会话绑定的工作目录沙箱
//! - `record_operation()`: 将文件操作追加到会话待提交检查点
//! - `record_patch_to_snapshot()`: 创建文件快照并发出 `snapshot-created` 事件
//!
//! ## Dependencies
//! - Internal: `crate::core::state::SessionManager`, `crate::core::SnapshotRegistry`, `crate::core::snapshot_engine`
//! - External: `tauri`

use tauri::{Emitter, Manager};

use crate::core::session::checkpoint::FileOperation;
use crate::core::snapshot_engine::Patch;
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

pub(super) async fn record_operation(
    app: &tauri::AppHandle,
    session_id: &str,
    operation: FileOperation,
) {
    if let Some(manager) = app.try_state::<crate::core::state::SessionManager>() {
        let ctx = manager.get_or_create(session_id).await;
        let mut ops = ctx.pending_checkpoint.lock().await;
        ops.push(operation);
    }
}

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
