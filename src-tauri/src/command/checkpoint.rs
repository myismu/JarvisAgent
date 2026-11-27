//! # checkpoint.rs — 检查点与回滚 Tauri 命令薄封装
//!
//! 仅负责前端 invoke 参数接收与转发；核心回滚目标解析、快照归一化与分支编排均位于
//! `crate::core::rollback::checkpoint_service`。
//!
//! ## Key Exports
//! - 12 个检查点/回滚/分支 Tauri 命令
//! - 重导出 `Checkpoint` / `FileOperation` / `OpType` / `Branch` / `CheckpointTree` / `BranchInfo` / `RollbackRecallResult` / `RollbackPreviewResult`
//!
//! ## Dependencies
//! - Internal: `crate::core::rollback::checkpoint_service`, `crate::core::runtime::{SessionManager, SnapshotRegistry}`
//! - External: `tauri`

use crate::core::rollback::checkpoint_service::{self, RollbackRecallParams};
pub use crate::core::rollback::checkpoint_service::{
    Branch, BranchInfo, Checkpoint, CheckpointTree, FileOperation, OpType, RollbackPreviewResult,
    RollbackRecallResult,
};
use crate::core::runtime::{SessionManager, SnapshotRegistry};
use crate::infra::types::error::CommandError;

#[tauri::command]
pub async fn list_checkpoints(
    session_id: String,
    branch_name: Option<String>,
    registry: tauri::State<'_, SnapshotRegistry>,
) -> Result<Vec<Checkpoint>, CommandError> {
    checkpoint_service::list_checkpoints(&session_id, branch_name.as_deref(), registry.inner())
        .await
        .map_err(CommandError::from)
}

#[tauri::command]
pub async fn get_checkpoint_tree(
    session_id: String,
    registry: tauri::State<'_, SnapshotRegistry>,
) -> Result<CheckpointTree, CommandError> {
    checkpoint_service::get_checkpoint_tree(&session_id, registry.inner())
        .await
        .map_err(CommandError::from)
}

#[tauri::command]
pub async fn rollback_to_checkpoint(
    session_id: String,
    checkpoint_id: String,
    rollback_files: Option<bool>,
    session_manager: tauri::State<'_, SessionManager>,
    registry: tauri::State<'_, SnapshotRegistry>,
    app: tauri::AppHandle,
) -> Result<Vec<String>, CommandError> {
    checkpoint_service::rollback_to_checkpoint(
        &session_id,
        &checkpoint_id,
        rollback_files.unwrap_or(false),
        session_manager.inner(),
        registry.inner(),
        &app,
    )
    .await
    .map_err(CommandError::from)
}

#[tauri::command]
pub async fn preview_rollback_to_checkpoint_with_recall(
    session_id: String,
    checkpoint_id: String,
    message_id: Option<String>,
    user_message_index: Option<usize>,
    registry: tauri::State<'_, SnapshotRegistry>,
) -> Result<RollbackPreviewResult, CommandError> {
    checkpoint_service::preview_rollback_with_recall(
        &session_id,
        &checkpoint_id,
        message_id.as_deref(),
        user_message_index,
        registry.inner(),
    )
    .await
    .map_err(CommandError::from)
}

#[tauri::command]
#[allow(clippy::too_many_arguments)]
pub async fn rollback_to_checkpoint_with_recall(
    session_id: String,
    checkpoint_id: String,
    rollback_files: Option<bool>,
    message_id: Option<String>,
    user_message_index: Option<usize>,
    session_manager: tauri::State<'_, SessionManager>,
    registry: tauri::State<'_, SnapshotRegistry>,
    app: tauri::AppHandle,
) -> Result<RollbackRecallResult, CommandError> {
    checkpoint_service::rollback_with_recall(
        RollbackRecallParams {
            session_id,
            checkpoint_id,
            rollback_files: rollback_files.unwrap_or(false),
            message_id,
            user_message_index,
        },
        session_manager.inner(),
        registry.inner(),
        &app,
    )
    .await
    .map_err(CommandError::from)
}

#[tauri::command]
pub async fn create_branch(
    session_id: String,
    branch_name: String,
    from_checkpoint_id: Option<String>,
    agent_id: Option<String>,
    description: Option<String>,
    registry: tauri::State<'_, SnapshotRegistry>,
) -> Result<Branch, CommandError> {
    checkpoint_service::create_branch(
        &session_id,
        branch_name,
        from_checkpoint_id,
        agent_id,
        description,
        registry.inner(),
    )
    .await
    .map_err(CommandError::from)
}

#[tauri::command]
pub async fn switch_branch(
    session_id: String,
    branch_name: String,
    registry: tauri::State<'_, SnapshotRegistry>,
) -> Result<Branch, CommandError> {
    checkpoint_service::switch_branch(&session_id, &branch_name, registry.inner())
        .await
        .map_err(CommandError::from)
}

#[tauri::command]
pub async fn list_branches(
    session_id: String,
    registry: tauri::State<'_, SnapshotRegistry>,
) -> Result<Vec<Branch>, CommandError> {
    checkpoint_service::list_branches(&session_id, registry.inner())
        .await
        .map_err(CommandError::from)
}

#[tauri::command]
pub async fn delete_branch(
    session_id: String,
    branch_name: String,
    registry: tauri::State<'_, SnapshotRegistry>,
) -> Result<(), CommandError> {
    let _ = (session_id, branch_name, registry);
    // 快照引擎的 Branch 暂无 delete 方法，先返回成功以保持兼容
    Ok(())
}

#[tauri::command]
pub async fn get_active_branch(
    session_id: String,
    registry: tauri::State<'_, SnapshotRegistry>,
) -> Result<Branch, CommandError> {
    checkpoint_service::get_active_branch(&session_id, registry.inner())
        .await
        .map_err(CommandError::from)
}

#[tauri::command]
pub async fn commit_checkpoint(
    session_id: String,
    trigger_message: String,
    _agent_id: Option<String>,
    _workspace_id: Option<String>,
    registry: tauri::State<'_, SnapshotRegistry>,
) -> Result<Checkpoint, CommandError> {
    checkpoint_service::commit_checkpoint(&session_id, trigger_message, registry.inner())
        .await
        .map_err(CommandError::from)
}

#[tauri::command]
pub async fn clear_pending_operations(_session_id: String) -> Result<(), CommandError> {
    // pending_checkpoint 队列已移除，此命令不再执行任何操作
    Ok(())
}
