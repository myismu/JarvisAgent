use crate::core::state::SnapshotRegistry;
use crate::core::snapshot_engine::{AgentSandbox, SandboxComparison};

#[tauri::command]
pub async fn sandbox_create(
    session_id: String,
    agent_id: String,
    base_snapshot_id: String,
    description: Option<String>,
    registry: tauri::State<'_, SnapshotRegistry>,
) -> Result<AgentSandbox, String> {
    let manager = registry.0.read().await.get_or_create(&session_id).await?;
    manager.create_sandbox(agent_id, base_snapshot_id, description).await
}

#[tauri::command]
pub async fn sandbox_get(
    session_id: String,
    sandbox_id: String,
    registry: tauri::State<'_, SnapshotRegistry>,
) -> Result<Option<AgentSandbox>, String> {
    let manager = registry.0.read().await.get_or_create(&session_id).await?;
    manager.get_sandbox(&sandbox_id).await
}

#[tauri::command]
pub async fn sandbox_list(
    session_id: String,
    registry: tauri::State<'_, SnapshotRegistry>,
) -> Result<Vec<AgentSandbox>, String> {
    let manager = registry.0.read().await.get_or_create(&session_id).await?;
    Ok(manager.list_sandboxes().await)
}

#[tauri::command]
pub async fn sandbox_complete(
    session_id: String,
    sandbox_id: String,
    registry: tauri::State<'_, SnapshotRegistry>,
) -> Result<(), String> {
    let manager = registry.0.read().await.get_or_create(&session_id).await?;
    manager.complete_sandbox(&sandbox_id).await
}

#[tauri::command]
pub async fn sandbox_abandon(
    session_id: String,
    sandbox_id: String,
    registry: tauri::State<'_, SnapshotRegistry>,
) -> Result<(), String> {
    let manager = registry.0.read().await.get_or_create(&session_id).await?;
    manager.abandon_sandbox(&sandbox_id).await
}

#[tauri::command]
pub async fn sandbox_publish(
    session_id: String,
    sandbox_id: String,
    registry: tauri::State<'_, SnapshotRegistry>,
) -> Result<String, String> {
    let manager = registry.0.read().await.get_or_create(&session_id).await?;
    manager.publish_sandbox(&sandbox_id).await
}

#[tauri::command]
pub async fn sandbox_compare(
    session_id: String,
    registry: tauri::State<'_, SnapshotRegistry>,
) -> Result<Vec<SandboxComparison>, String> {
    let manager = registry.0.read().await.get_or_create(&session_id).await?;
    Ok(manager.compare_sandboxes().await)
}
