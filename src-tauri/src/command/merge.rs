//! # merge.rs — 分支合并 Tauri 命令
//!
//! 提供快照分支的合并预览、执行和冲突查询命令。
//!
//! ## 关键导出
//! - `merge_preview()`: 预览两个分支的合并结果
//! - `merge_execute()`: 执行合并（带冲突解决方案）
//! - `merge_get_conflicts()`: 获取两个分支间的冲突列表

use crate::core::rollback::Snapshot;
use crate::core::orchestration::multi_agent::{Conflict, ConflictResolution, MergeResult};
use crate::infra::state::state::SnapshotRegistry;
use std::collections::HashMap as StdHashMap;

/// 预览两个分支的合并结果
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
    manager
        .execute_merge(&source_branch, &target_branch, resolutions, message)
        .await
}

#[tauri::command]
pub async fn merge_get_conflicts(
    session_id: String,
    source_branch: String,
    target_branch: String,
    registry: tauri::State<'_, SnapshotRegistry>,
) -> Result<Vec<Conflict>, String> {
    let manager = registry.0.read().await.get_or_create(&session_id).await?;
    manager
        .get_merge_conflicts(&source_branch, &target_branch)
        .await
}
