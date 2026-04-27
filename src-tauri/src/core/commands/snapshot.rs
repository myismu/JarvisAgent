use crate::core::state::SnapshotRegistry;
use crate::core::snapshot_engine::{Patch, Snapshot, SnapshotTreeView, SnapshotSummary, Workspace};

#[tauri::command]
pub async fn snapshot_create(
    session_id: String,
    patches: Vec<Patch>,
    message: Option<String>,
    agent_id: Option<String>,
    workspace_id: Option<String>,
    registry: tauri::State<'_, SnapshotRegistry>,
) -> Result<Snapshot, String> {
    let manager = registry.0.read().await.get_or_create(&session_id).await?;
    manager.create_snapshot(patches, message, agent_id, workspace_id, None).await
}

#[tauri::command]
pub async fn snapshot_get_tree_view(
    session_id: String,
    registry: tauri::State<'_, SnapshotRegistry>,
) -> Result<SnapshotTreeView, String> {
    let manager = registry.0.read().await.get_or_create(&session_id).await?;
    Ok(manager.get_tree_view().await)
}

#[tauri::command]
pub async fn snapshot_get_summaries(
    session_id: String,
    snapshot_ids: Vec<String>,
    registry: tauri::State<'_, SnapshotRegistry>,
) -> Result<Vec<SnapshotSummary>, String> {
    let manager = registry.0.read().await.get_or_create(&session_id).await?;
    Ok(manager.get_summaries(&snapshot_ids).await)
}

#[tauri::command]
pub async fn snapshot_get_detail(
    session_id: String,
    snapshot_id: String,
    registry: tauri::State<'_, SnapshotRegistry>,
) -> Result<Option<Snapshot>, String> {
    let manager = registry.0.read().await.get_or_create(&session_id).await?;
    manager.get_snapshot(&snapshot_id).await
}

#[tauri::command]
pub async fn snapshot_create_branch(
    session_id: String,
    branch_name: String,
    from_snapshot_id: Option<String>,
    agent_id: Option<String>,
    description: Option<String>,
    registry: tauri::State<'_, SnapshotRegistry>,
) -> Result<(), String> {
    let manager = registry.0.read().await.get_or_create(&session_id).await?;
    manager.create_branch(branch_name, from_snapshot_id, agent_id, description).await
}

#[tauri::command]
pub async fn snapshot_switch_branch(
    session_id: String,
    branch_name: String,
    registry: tauri::State<'_, SnapshotRegistry>,
) -> Result<(), String> {
    let manager = registry.0.read().await.get_or_create(&session_id).await?;
    manager.switch_branch(&branch_name).await
}

#[tauri::command]
pub async fn snapshot_rollback(
    session_id: String,
    snapshot_id: String,
    target_dir: String,
    registry: tauri::State<'_, SnapshotRegistry>,
) -> Result<Workspace, String> {
    let manager = registry.0.read().await.get_or_create(&session_id).await?;
    let target_path = std::path::PathBuf::from(target_dir);
    manager.rollback_to(&snapshot_id, &target_path).await
}

#[tauri::command]
pub async fn snapshot_list(
    session_id: String,
    branch_name: Option<String>,
    registry: tauri::State<'_, SnapshotRegistry>,
) -> Result<Vec<Snapshot>, String> {
    let manager = registry.0.read().await.get_or_create(&session_id).await?;
    Ok(manager.list_snapshots(branch_name.as_deref()).await)
}

#[tauri::command]
pub async fn snapshot_list_branches(
    session_id: String,
    registry: tauri::State<'_, SnapshotRegistry>,
) -> Result<Vec<crate::core::snapshot_engine::snapshot::Branch>, String> {
    let manager = registry.0.read().await.get_or_create(&session_id).await?;
    Ok(manager.list_branches().await)
}

#[tauri::command]
pub async fn snapshot_get_current(
    session_id: String,
    registry: tauri::State<'_, SnapshotRegistry>,
) -> Result<(String, String), String> {
    let manager = registry.0.read().await.get_or_create(&session_id).await?;
    Ok((manager.get_current_branch().await, manager.get_current_snapshot_id().await))
}
