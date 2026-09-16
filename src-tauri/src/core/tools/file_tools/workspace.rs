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

/// 将工具入参中的相对路径解析为执行路径。
///
/// 校验层（`permission::is_within_workspace`）对相对路径按 `ws.join(path)` 解析，
/// 执行层必须使用同一基准——否则进程 CWD 与沙箱目录不一致时，会出现
/// "校验放行、执行越界"的沙箱逃逸。绝对路径原样返回；非沙箱会话
/// （`ws = None`）保持按进程 CWD 解析的旧行为。
///
/// 注意：必须在 `ensure_path_permission` 通过之后调用（`..` 遍历已被拦截）。
pub(super) fn resolve_exec_path(raw: &str, ws: Option<&std::path::Path>) -> String {
    let path = std::path::Path::new(raw);
    if path.is_absolute() {
        return raw.to_string();
    }
    match ws {
        Some(ws) => ws.join(path).to_string_lossy().into_owned(),
        None => raw.to_string(),
    }
}

/// 沙箱目录已不存在时的统一提示（附加到"文件/目录不存在"类报错尾部）。
///
/// 会话绑定的项目目录可能事后被删除或移动：此时校验层仍按沙箱 join 放行，
/// 但所有文件操作都会连环失败，需要提示用户检查目录或重建会话。
pub(super) fn sandbox_missing_hint(ws: Option<&std::path::Path>) -> &'static str {
    match ws {
        Some(ws) if !ws.exists() => {
            "（注意：会话绑定的沙箱目录可能已被删除或移动，请检查该目录是否存在，必要时新建会话重新挂载项目）"
        }
        _ => "",
    }
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn resolve_exec_path_joins_relative_to_workspace() {
        let ws = std::path::Path::new("/proj");
        // 分隔符随平台（Windows join 会插入 \），按 Path 语义比较而非字符串
        assert_eq!(
            std::path::Path::new(&resolve_exec_path("src/main.rs", Some(ws))),
            ws.join("src/main.rs")
        );
        // "." 同样走 join（fs 层对 "ws/." 与 "ws" 等价）
        assert_eq!(
            std::path::Path::new(&resolve_exec_path(".", Some(ws))),
            ws.join(".")
        );
    }

    #[test]
    fn resolve_exec_path_keeps_absolute_paths() {
        let ws = std::path::Path::new("E:\\proj");
        // 绝对路径不受沙箱 join 影响（越界由校验层拦截）
        #[cfg(windows)]
        assert_eq!(
            resolve_exec_path("C:\\other\\a.txt", Some(ws)),
            "C:\\other\\a.txt"
        );
        #[cfg(unix)]
        assert_eq!(resolve_exec_path("/other/a.txt", Some(ws)), "/other/a.txt");
    }

    #[test]
    fn resolve_exec_path_no_workspace_keeps_raw() {
        assert_eq!(resolve_exec_path("src/main.rs", None), "src/main.rs");
    }

    #[test]
    fn sandbox_missing_hint_empty_for_existing_dir() {
        let ws = std::env::temp_dir();
        assert_eq!(sandbox_missing_hint(Some(&ws)), "");
        assert_eq!(sandbox_missing_hint(None), "");
    }
}
