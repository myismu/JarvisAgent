//! # workspace.rs — 会话工作区与快照记录桥接
//!
//! 为文件工具提供当前会话工作目录查询，以及写入 rollback 后通知前端的统一入口。
//!
//! ## Key Exports
//! - `get_workspace()`: 获取当前会话绑定的工作目录沙箱
//! - `record_patch_to_snapshot()`: 暂存本轮文件补丁
//! - `commit_pending_snapshot()`: 在一轮 Agent 结束时提交普通快照
//! - `commit_checkpoint_snapshot()`: 创建加速回放用检查点快照
//!
//! ## Dependencies
//! - Internal: `crate::infra::state::state::SessionManager`, `crate::core::SnapshotRegistry`, `crate::core::rollback`
//! - External: `tauri`

use tauri::{Emitter, Manager};

use crate::infra::types::models::Message;
use crate::core::rollback::Patch;
use crate::infra::state::state::{PendingSnapshotPatch, SessionManager};
use crate::infra::state::state::SnapshotRegistry;

/// 获取当前会话的工作目录沙箱
pub(super) async fn get_workspace(
    app: &tauri::AppHandle,
    session_id: &str,
) -> Option<std::path::PathBuf> {
    if let Some(manager) = app.try_state::<crate::infra::state::state::SessionManager>() {
        let ctx = manager.get_or_create(session_id).await;
        let ws = ctx.workspace.lock().await.clone();
        return ws;
    }
    None
}

/// 查找最后一条"真正的用户输入"消息（跳过工具结果等内部消息）
fn latest_user_message_index(messages: &[Message]) -> Option<usize> {
    use crate::infra::types::models::{Content, ContentBlock};

    messages
        .iter()
        .enumerate()
        .rev()
        .find_map(|(index, message)| {
            if let Message::User { content } = message {
                // 跳过仅包含 ToolResult 的内部消息
                let is_tool_result_only = match content {
                    Content::Multiple(blocks) => blocks
                        .iter()
                        .all(|b| matches!(b, ContentBlock::ToolResult { .. })),
                    Content::Single(text) => {
                        // 跳过后台通知消息
                        let trimmed = text.trim();
                        trimmed.starts_with("<background-results>")
                            || trimmed.starts_with("<background-results")
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
    if let Some(manager) = app.try_state::<crate::infra::state::state::SessionManager>() {
        let ctx = manager.get_or_create(session_id).await;
        let session = ctx.memory.lock().await;
        return latest_user_message_index(&session.messages);
    }
    crate::core::session::load_session(session_id)
        .ok()
        .and_then(|session| latest_user_message_index(&session.messages))
}

async fn active_run_id(app: &tauri::AppHandle, session_id: &str) -> Option<String> {
    let manager = app.try_state::<SessionManager>()?;
    let ctx = manager.get_or_create(session_id).await;
    let run_id = ctx.active_run_id.lock().await.clone();
    run_id
}

/// 工具层调用：只发布 Patch 到内存队列，不持久化。
/// pipeline 会在本轮结束时统一调用 commit_pending_snapshot 持久化。
async fn persist_pending_patch(
    app: &tauri::AppHandle,
    session_id: &str,
    patch: Patch,
    message: Option<String>,
) {
    if let Some(manager) = app.try_state::<SessionManager>() {
        let ctx = manager.get_or_create(session_id).await;
        let trigger_user_memory_index = active_user_message_index(app, session_id).await;
        let trigger_user_message_id = trigger_user_memory_index.and_then(|index| {
            ctx.memory
                .try_lock()
                .ok()
                .and_then(|memory| memory.message_ids.get(index).cloned())
        });
        let run_id = ctx
            .active_run_id
            .lock()
            .await
            .clone()
            .unwrap_or_else(|| "manual".to_string());
        let seq = {
            let guard = ctx.pending_patches.lock().await;
            guard
                .iter()
                .filter(|item| item.run_id == run_id)
                .map(|item| item.seq)
                .max()
                .map_or(0, |seq| seq + 1)
        };

        ctx.pending_patches.lock().await.push(PendingSnapshotPatch {
            run_id,
            seq,
            patch,
            message,
            trigger_user_memory_index,
            trigger_user_message_id,
        });
    }
}

fn records_to_pending(
    records: Vec<crate::infra::db::PendingSnapshotPatchRecord>,
) -> Vec<PendingSnapshotPatch> {
    records
        .into_iter()
        .map(|record| PendingSnapshotPatch {
            run_id: record.run_id,
            seq: record.seq,
            patch: record.patch,
            message: record.message,
            trigger_user_memory_index: record.trigger_user_memory_index,
            trigger_user_message_id: record.trigger_user_message_id,
        })
        .collect()
}

/// 将文件变更暂存为本轮待提交补丁
pub(super) async fn record_patch_to_snapshot(
    app: &tauri::AppHandle,
    session_id: &str,
    patch: Patch,
    message: Option<String>,
) {
    persist_pending_patch(app, session_id, patch, message).await;
}

pub async fn has_pending_patches(app: &tauri::AppHandle, session_id: &str) -> bool {
    if let Some(manager) = app.try_state::<SessionManager>() {
        let ctx = manager.get_or_create(session_id).await;
        if !ctx.pending_patches.lock().await.is_empty() {
            return true;
        }
    }
    crate::infra::db::list_agent_run_patches(session_id, None)
        .map(|records| !records.is_empty())
        .unwrap_or(false)
}

pub async fn commit_pending_snapshot(
    app: &tauri::AppHandle,
    session_id: &str,
    message: String,
    trigger_user_memory_index: Option<usize>,
) -> Option<String> {
    let manager = app.try_state::<SessionManager>()?;
    let ctx = manager.get_or_create(session_id).await;
    let run_id = active_run_id(app, session_id).await;
    let pending = {
        let mut guard = ctx.pending_patches.lock().await;
        if guard.is_empty() {
            let records =
                crate::infra::db::list_agent_run_patches(session_id, run_id.as_deref())
                    .unwrap_or_else(|err| {
                        eprintln!("[Snapshot] 读取 pending patch 失败: {}", err);
                        Vec::new()
                    });
            records_to_pending(records)
        } else if let Some(run_id) = run_id.as_ref() {
            let (current_run, remaining): (Vec<_>, Vec<_>) =
                guard.drain(..).partition(|item| item.run_id == *run_id);
            *guard = remaining;
            current_run
        } else {
            std::mem::take(&mut *guard)
        }
    };

    if pending.is_empty() {
        return None;
    }

    let commit_run_id = run_id
        .or_else(|| pending.first().map(|item| item.run_id.clone()))
        .unwrap_or_else(|| "manual".to_string());
    let mut pending = pending;
    pending.sort_by_key(|item| item.seq);

    let patches = pending
        .iter()
        .map(|item| item.patch.clone())
        .collect::<Vec<_>>();
    let snapshot_message = if !message.trim().is_empty() {
        Some(message)
    } else {
        pending.iter().rev().find_map(|item| item.message.clone())
    };
    let user_index = trigger_user_memory_index.or_else(|| {
        pending
            .iter()
            .find_map(|item| item.trigger_user_memory_index)
    });

    if let Some(registry) = app.try_state::<SnapshotRegistry>() {
        let mgr_result = registry.0.read().await.get_or_create(session_id).await;
        if let Ok(mgr) = mgr_result {
            match mgr
                .create_snapshot(
                    patches,
                    snapshot_message.clone(),
                    None,
                    None,
                    None,
                    user_index,
                )
                .await
            {
                Ok(snapshot) => {
                    let snapshot_id = snapshot.id.clone();
                    if let Err(err) = crate::infra::db::delete_agent_run_patches(
                        session_id,
                        Some(&commit_run_id),
                    ) {
                        eprintln!("[Snapshot] 清理 pending patch 失败: {}", err);
                    }
                    let _ = app.emit(
                        "snapshot-created",
                        serde_json::json!({
                            "sessionId": session_id,
                            "snapshotId": snapshot.id
                        }),
                    );
                    if mgr.should_create_checkpoint().await {
                        let _ = mgr
                            .create_checkpoint_snapshot(snapshot_message, None, None, user_index)
                            .await;
                    }
                    return Some(snapshot_id);
                }
                Err(err) => {
                    eprintln!("[Snapshot] 提交本轮快照失败: {}", err);
                    ctx.pending_patches.lock().await.extend(pending);
                }
            }
        }
    }

    None
}
