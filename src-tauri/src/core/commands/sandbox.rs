//! # sandbox.rs — 多 Agent 沙箱会话 Tauri 命令
//!
//! 提供 Agent 沙箱的创建、查询、完成、放弃、发布和比较命令。
//! 沙箱允许多个 Agent 在隔离环境中并行工作，互不干扰。
//!
//! ## 关键导出
//! - `sandbox_create()`: 为指定 Agent 创建沙箱环境
//! - `sandbox_complete()` / `sandbox_abandon()`: 结束沙箱（提交或丢弃）
//! - `sandbox_publish()`: 将沙箱变更发布到主分支
//! - `sandbox_compare()`: 比较所有沙箱与主分支的差异

use crate::core::snapshot_engine::{AgentSandbox, SandboxComparison};
use crate::core::state::SnapshotRegistry;

/// 为指定 Agent 创建沙箱环境
#[tauri::command]
pub async fn sandbox_create(
    session_id: String,
    agent_id: String,
    base_snapshot_id: String,
    description: Option<String>,
    registry: tauri::State<'_, SnapshotRegistry>,
) -> Result<AgentSandbox, String> {
    let manager = registry.0.read().await.get_or_create(&session_id).await?;
    manager
        .create_sandbox(agent_id, base_snapshot_id, description)
        .await
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
