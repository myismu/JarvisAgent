use std::collections::HashMap as StdHashMap;
use crate::core::state::SnapshotRegistry;
use crate::core::snapshot_engine::{MergeResult, Conflict, ConflictResolution, Snapshot};

#[tauri::command]
pub async fn merge_preview(
    session_id: String,
    source_branch: String,
    target_branch: String,
    registry: tauri::State<'_, SnapshotRegistry>,
) -> Result<MergeResult, String> {
    let manager = registry.0.read().await.get_or_create(&session_id).await?;
    manager.preview_merge(&source_branch, &target_branch).await
}

#[tauri::command]
pub async fn merge_execute(
    session_id: String,
    source_branch: String,
    target_branch: String,
    resolutions: StdHashMap<String, ConflictResolution>,
    message: Option<String>,
    registry: tauri::State<'_, SnapshotRegistry>,
) -> Result<Snapshot, String> {
    let manager = registry.0.read().await.get_or_create(&session_id).await?;
    manager.execute_merge(&source_branch, &target_branch, resolutions, message).await
}

#[tauri::command]
pub async fn merge_get_conflicts(
    session_id: String,
    source_branch: String,
    target_branch: String,
    registry: tauri::State<'_, SnapshotRegistry>,
) -> Result<Vec<Conflict>, String> {
    let manager = registry.0.read().await.get_or_create(&session_id).await?;
    manager.get_merge_conflicts(&source_branch, &target_branch).await
}
