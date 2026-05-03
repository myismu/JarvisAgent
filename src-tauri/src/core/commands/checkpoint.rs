//! # checkpoint.rs — 检查点命令兼容层
//!
//! 所有检查点操作已迁移到底层快照引擎。本模块作为前端兼容层，
//! 将旧的 checkpoint 命令映射到 snapshot 系统。
//!
//! ## 迁移说明
//! - `Checkpoint` ≈ `Snapshot` with `is_checkpoint = true`
//! - 文件回滚统一走 `ReplayEngine`
//! - 分支管理统一走 `SnapshotTree`

use crate::core::rollback::Snapshot;
use crate::core::state::{SessionManager, SnapshotRegistry};

// ==================== 兼容类型定义（原 session/checkpoint.rs）====================

use serde::{Deserialize, Serialize};
use std::collections::HashMap;

#[derive(Serialize, Deserialize, Debug, Clone)]
#[serde(rename_all = "camelCase")]
pub struct Checkpoint {
    pub id: String,
    pub session_id: String,
    pub parent_id: Option<String>,
    pub branch_name: String,
    pub agent_id: Option<String>,
    pub workspace_id: Option<String>,
    pub created_at: u64,
    pub trigger_message: String,
    pub operations: Vec<FileOperation>,
    pub metadata: HashMap<String, String>,
}

#[derive(Serialize, Deserialize, Debug, Clone)]
#[serde(rename_all = "camelCase")]
pub struct FileOperation {
    pub op_type: OpType,
    pub path: String,
    pub old_content_hash: Option<String>,
    pub backup_path: Option<String>,
    pub new_content_hash: Option<String>,
    pub diff_summary: Option<String>,
}

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq)]
#[serde(rename_all = "lowercase")]
pub enum OpType {
    Edit,
    Write,
    Create,
    Delete,
    Rename,
}

#[derive(Serialize, Deserialize, Debug, Clone)]
#[serde(rename_all = "camelCase")]
pub struct Branch {
    pub name: String,
    pub session_id: String,
    pub head_checkpoint_id: Option<String>,
    pub created_at: u64,
    pub agent_id: Option<String>,
    pub description: String,
    pub is_active: bool,
}

#[derive(Serialize, Deserialize, Debug, Clone)]
#[serde(rename_all = "camelCase")]
pub struct CheckpointTree {
    pub session_id: String,
    pub branches: Vec<BranchInfo>,
    pub checkpoints: Vec<Checkpoint>,
}

#[derive(Serialize, Deserialize, Debug, Clone)]
#[serde(rename_all = "camelCase")]
pub struct BranchInfo {
    pub name: String,
    pub head_checkpoint_id: Option<String>,
    pub checkpoint_count: usize,
    pub is_active: bool,
}

#[derive(serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RollbackRecallResult {
    pub restored_files: Vec<String>,
    pub recalled_text: String,
}

fn message_text(content: &crate::core::models::Content) -> String {
    use crate::core::models::{Content, ContentBlock};

    match content {
        Content::Single(s) => s.clone(),
        Content::Multiple(blocks) => blocks
            .iter()
            .filter_map(|b| {
                if let ContentBlock::Text { text } = b {
                    Some(text.as_str())
                } else {
                    None
                }
            })
            .collect::<Vec<_>>()
            .join(" "),
    }
}

/// 模糊匹配检查点触发消息与用户消息（精确匹配或互为前缀）
fn checkpoint_matches_user_message(trigger: &str, message: &str) -> bool {
    let trigger = trigger.trim();
    let message = message.trim();
    if trigger.is_empty() || message.is_empty() {
        return false;
    }

    message == trigger || message.starts_with(trigger) || trigger.starts_with(message)
}

fn find_checkpoint_user_message(
    messages: &[crate::core::models::Message],
    trigger_message: &str,
) -> Option<(usize, String)> {
    use crate::core::models::Message;

    messages.iter().enumerate().find_map(|(i, msg)| {
        if let Message::User { content } = msg {
            let text = message_text(content);
            if checkpoint_matches_user_message(trigger_message, &text) {
                return Some((i, text));
            }
        }
        None
    })
}

/// 将快照映射为兼容的检查点类型（operations 由补丁链聚合而来）
fn snapshot_to_checkpoint(
    session_id: &str,
    snapshot: &Snapshot,
    operations: Vec<FileOperation>,
) -> Checkpoint {
    Checkpoint {
        id: snapshot.id.clone(),
        session_id: session_id.to_string(),
        parent_id: snapshot.parent_id.clone(),
        branch_name: snapshot.branch_name.clone(),
        agent_id: snapshot.agent_id.clone(),
        workspace_id: snapshot.workspace_id.clone(),
        created_at: snapshot.created_at,
        trigger_message: snapshot.message.clone().unwrap_or_default(),
        operations,
        metadata: snapshot.metadata.clone(),
    }
}

/// 清理检查点之后的 agent_steps 和 plan_documents（基于时间戳）
fn prune_metadata_after_checkpoint(
    session: &mut crate::core::models::SessionMemory,
    cutoff_secs: u64,
) {
    if cutoff_secs == 0 {
        session.agent_steps.clear();
        session.plan_documents.clear();
        return;
    }

    session
        .plan_documents
        .retain(|plan| plan.created_at <= cutoff_secs);
    session.agent_steps.retain(|step| {
        let timestamp_secs = if step.timestamp > 10_000_000_000 {
            step.timestamp / 1000
        } else {
            step.timestamp
        };
        timestamp_secs <= cutoff_secs
    });
}

#[tauri::command]
pub async fn list_checkpoints(
    session_id: String,
    branch_name: Option<String>,
    registry: tauri::State<'_, SnapshotRegistry>,
) -> Result<Vec<Checkpoint>, String> {
    let manager = registry.0.read().await.get_or_create(&session_id).await?;
    let snapshots = manager.list_snapshots(branch_name.as_deref()).await;

    let checkpoints: Vec<_> = snapshots
        .iter()
        .filter(|s| s.is_checkpoint)
        .map(|s| snapshot_to_checkpoint(&session_id, s, vec![]))
        .collect();

    Ok(checkpoints)
}

#[tauri::command]
pub async fn get_checkpoint_tree(
    session_id: String,
    registry: tauri::State<'_, SnapshotRegistry>,
) -> Result<CheckpointTree, String> {
    let manager = registry.0.read().await.get_or_create(&session_id).await?;
    let tree_view = manager.get_tree_view().await;
    let snapshots = manager.list_snapshots(None).await;

    let branches: Vec<BranchInfo> = tree_view
        .branches
        .iter()
        .map(|b| BranchInfo {
            name: b.name.clone(),
            head_checkpoint_id: Some(b.root.id.clone()).filter(|id| !id.is_empty()),
            checkpoint_count: snapshots.iter().filter(|s| s.branch_name == b.name && s.is_checkpoint).count(),
            is_active: b.is_active,
        })
        .collect();

    let checkpoints: Vec<_> = snapshots
        .iter()
        .filter(|s| s.is_checkpoint)
        .map(|s| snapshot_to_checkpoint(&session_id, s, vec![]))
        .collect();

    Ok(CheckpointTree {
        session_id,
        branches,
        checkpoints,
    })
}

/// 回滚到指定检查点：可选恢复文件、截断消息历史、清理后续元数据
///
/// 支持"向前追溯"：当 checkpoint_id 为空或指向纯聊天轮次（无实快照）时，
/// 自动向前查找最近的 checkpoint 快照来恢复文件状态。
#[tauri::command]
pub async fn rollback_to_checkpoint(
    session_id: String,
    checkpoint_id: String,
    rollback_files: Option<bool>,
    session_manager: tauri::State<'_, SessionManager>,
    registry: tauri::State<'_, SnapshotRegistry>,
    app: tauri::AppHandle,
) -> Result<Vec<String>, String> {
    use crate::core::commands::session::switch_away_and_delete_empty_session;
    use tauri::Emitter;

    let mut restored_files = Vec::new();

    // 决定实际用于文件回滚的快照 ID：
    // 如果 checkpoint_id 非空，直接用；否则向前追溯最近的 checkpoint
    let effective_checkpoint_id = if !checkpoint_id.is_empty() {
        Some(checkpoint_id.clone())
    } else {
        // 纯聊天轮次没有快照，向前追溯最近的 checkpoint
        let mgr = registry.0.read().await.get_or_create(&session_id).await?;
        mgr.find_nearest_checkpoint_before().await
    };

    if rollback_files.unwrap_or(false) {
        if let Some(effective_id) = &effective_checkpoint_id {
            let workspace = crate::core::state::effective_workspace(&app, &session_id).await;
            if let Some(ws) = workspace {
                let manager = registry.0.read().await.get_or_create(&session_id).await?;
                match manager.rollback_to(effective_id, &ws).await {
                    Ok(_) => {
                        restored_files.push("文件已恢复到检查点状态".to_string());
                    }
                    Err(e) => {
                        eprintln!("[Rollback] 快照回滚失败: {}", e);
                        return Err(format!("文件回滚失败: {}", e));
                    }
                }
            }
        }
        // 如果 effective_checkpoint_id 为 None，说明从未有过文件编辑，
        // 无需恢复文件（工作区本身就是初始状态）
    }

    // 获取快照消息用于匹配用户消息
    let snapshot_message = if !checkpoint_id.is_empty() {
        match registry.0.read().await.get_or_create(&session_id).await {
            Ok(mgr) => match mgr.get_snapshot(&checkpoint_id).await {
                Ok(Some(snap)) => snap.message.clone().unwrap_or_default(),
                _ => String::new(),
            },
            _ => String::new(),
        }
    } else {
        // 纯聊天轮次没有快照消息，后续通过 user_message_index 来定位
        String::new()
    };

    let ctx = session_manager.get_or_create(&session_id).await;
    let is_empty;
    {
        let mut session = ctx.memory.lock().await;
        if !snapshot_message.is_empty() {
            if let Some((idx, _)) =
                find_checkpoint_user_message(&session.messages, &snapshot_message)
            {
                session.messages.truncate(idx);
            }
        }
        // 清理该检查点之后的元数据（基于时间戳）
        let cutoff = if !checkpoint_id.is_empty() {
            match registry.0.read().await.get_or_create(&session_id).await {
                Ok(mgr) => match mgr.get_snapshot(&checkpoint_id).await {
                    Ok(Some(snap)) => snap.created_at,
                    _ => 0,
                },
                _ => 0,
            }
        } else {
            // 纯聊天轮次无快照，用当前时间戳之前的都可以保留
            0
        };
        if cutoff > 0 {
            prune_metadata_after_checkpoint(&mut session, cutoff);
        }
        is_empty = session.messages.is_empty();
    }

    if is_empty {
        switch_away_and_delete_empty_session(&session_id, &app).await?;
    } else {
        let memory = ctx.memory.lock().await.clone();
        crate::core::session::save_session(&session_id, &memory, None);
        let _ = app.emit("session-updated", ());
    }

    Ok(restored_files)
}

/// 回滚并返回被撤回的用户消息文本（供前端重新填入输入框）
///
/// 支持"向前追溯"：当 checkpoint_id 为空或指向纯聊天轮次时，
/// 自动向前查找最近的 checkpoint 快照来恢复文件状态。
#[tauri::command]
pub async fn rollback_to_checkpoint_with_recall(
    session_id: String,
    checkpoint_id: String,
    rollback_files: Option<bool>,
    user_message_index: Option<usize>,
    session_manager: tauri::State<'_, SessionManager>,
    registry: tauri::State<'_, SnapshotRegistry>,
    app: tauri::AppHandle,
) -> Result<RollbackRecallResult, String> {
    use crate::core::commands::session::switch_away_and_delete_empty_session;
    use tauri::Emitter;

    let mut restored_files = Vec::new();

    // 决定实际用于文件回滚的快照 ID
    let effective_checkpoint_id = if !checkpoint_id.is_empty() {
        Some(checkpoint_id.clone())
    } else {
        let mgr = registry.0.read().await.get_or_create(&session_id).await?;
        mgr.find_nearest_checkpoint_before().await
    };

    if rollback_files.unwrap_or(false) {
        if let Some(effective_id) = &effective_checkpoint_id {
            let workspace = crate::core::state::effective_workspace(&app, &session_id).await;
            if let Some(ws) = workspace {
                let manager = registry.0.read().await.get_or_create(&session_id).await?;
                match manager.rollback_to(effective_id, &ws).await {
                    Ok(_) => {
                        restored_files.push("文件已恢复到检查点状态".to_string());
                    }
                    Err(e) => {
                        eprintln!("[Rollback] 快照回滚失败: {}", e);
                        return Err(format!("文件回滚失败: {}", e));
                    }
                }
            }
        }
    }

    let snapshot_message = if !checkpoint_id.is_empty() {
        match registry.0.read().await.get_or_create(&session_id).await {
            Ok(mgr) => match mgr.get_snapshot(&checkpoint_id).await {
                Ok(Some(snap)) => snap.message.clone().unwrap_or_default(),
                _ => String::new(),
            },
            _ => String::new(),
        }
    } else {
        String::new()
    };

    let ctx = session_manager.get_or_create(&session_id).await;
    let recalled_text;
    let is_empty;
    {
        let mut session = ctx.memory.lock().await;
        let (idx, recalled) = if let Some(index) = user_message_index {
            if index >= session.messages.len() {
                return Err("撤回消息不存在".to_string());
            }
            if let crate::core::models::Message::User { content } = &session.messages[index] {
                (index, message_text(content))
            } else {
                return Err("撤回目标不是用户消息".to_string());
            }
        } else if !snapshot_message.is_empty() {
            find_checkpoint_user_message(&session.messages, &snapshot_message)
                .ok_or_else(|| "无法在会话中找到该检查点对应的用户消息".to_string())?
        } else {
            return Err("无法定位撤回目标：既没有 checkpoint_id 也没有 user_message_index".to_string());
        };
        session.messages.truncate(idx);
        let cutoff = if !checkpoint_id.is_empty() {
            match registry.0.read().await.get_or_create(&session_id).await {
                Ok(mgr) => match mgr.get_snapshot(&checkpoint_id).await {
                    Ok(Some(snap)) => snap.created_at,
                    _ => 0,
                },
                _ => 0,
            }
        } else {
            0
        };
        if cutoff > 0 {
            prune_metadata_after_checkpoint(&mut session, cutoff);
        }
        recalled_text = recalled;
        is_empty = session.messages.is_empty();
    }

    if is_empty {
        switch_away_and_delete_empty_session(&session_id, &app).await?;
    } else {
        let memory = ctx.memory.lock().await.clone();
        crate::core::session::save_session(&session_id, &memory, None);
        let _ = app.emit("session-updated", ());
    }

    Ok(RollbackRecallResult {
        restored_files,
        recalled_text,
    })
}

fn snapshot_branch_to_checkpoint_branch(
    branch: &crate::core::rollback::snapshot::Branch,
) -> Branch {
    Branch {
        name: branch.name.clone(),
        session_id: branch.session_id.clone(),
        head_checkpoint_id: Some(branch.head_snapshot_id.clone()).filter(|id| !id.is_empty()),
        created_at: branch.created_at,
        agent_id: branch.agent_id.clone(),
        description: branch.description.clone(),
        is_active: branch.is_active,
    }
}

#[tauri::command]
pub async fn create_branch(
    session_id: String,
    branch_name: String,
    from_checkpoint_id: Option<String>,
    agent_id: Option<String>,
    description: Option<String>,
    registry: tauri::State<'_, SnapshotRegistry>,
) -> Result<Branch, String> {
    let manager = registry.0.read().await.get_or_create(&session_id).await?;
    manager
        .create_branch(branch_name.clone(), from_checkpoint_id, agent_id, description)
        .await?;
    let branches = manager.list_branches().await;
    branches
        .into_iter()
        .find(|b| b.name == branch_name)
        .map(|b| snapshot_branch_to_checkpoint_branch(&b))
        .ok_or_else(|| format!("创建分支 '{}' 后无法找到", branch_name))
}

#[tauri::command]
pub async fn switch_branch(
    session_id: String,
    branch_name: String,
    registry: tauri::State<'_, SnapshotRegistry>,
) -> Result<Branch, String> {
    let manager = registry.0.read().await.get_or_create(&session_id).await?;
    manager.switch_branch(&branch_name).await?;
    let branches = manager.list_branches().await;
    branches
        .into_iter()
        .find(|b| b.name == branch_name)
        .map(|b| snapshot_branch_to_checkpoint_branch(&b))
        .ok_or_else(|| format!("分支 '{}' 不存在", branch_name))
}

#[tauri::command]
pub async fn list_branches(
    session_id: String,
    registry: tauri::State<'_, SnapshotRegistry>,
) -> Result<Vec<Branch>, String> {
    let manager = registry.0.read().await.get_or_create(&session_id).await?;
    Ok(manager
        .list_branches()
        .await
        .iter()
        .map(snapshot_branch_to_checkpoint_branch)
        .collect())
}

#[tauri::command]
pub async fn delete_branch(
    session_id: String,
    branch_name: String,
    registry: tauri::State<'_, SnapshotRegistry>,
) -> Result<(), String> {
    let _ = (session_id, branch_name, registry);
    // 快照引擎的 Branch 暂无 delete 方法，先返回成功以保持兼容
    Ok(())
}

#[tauri::command]
pub async fn get_active_branch(
    session_id: String,
    registry: tauri::State<'_, SnapshotRegistry>,
) -> Result<Branch, String> {
    let manager = registry.0.read().await.get_or_create(&session_id).await?;
    let current = manager.get_current_branch().await;
    let branches = manager.list_branches().await;
    branches
        .into_iter()
        .find(|b| b.name == current)
        .map(|b| snapshot_branch_to_checkpoint_branch(&b))
        .ok_or_else(|| "无活跃分支".to_string())
}

/// 手动提交检查点（创建显式 checkpoint 快照）
#[tauri::command]
pub async fn commit_checkpoint(
    session_id: String,
    trigger_message: String,
    _agent_id: Option<String>,
    _workspace_id: Option<String>,
    registry: tauri::State<'_, SnapshotRegistry>,
) -> Result<Checkpoint, String> {
    let manager = registry.0.read().await.get_or_create(&session_id).await?;
    let snapshot = manager
        .create_checkpoint_snapshot(Some(trigger_message), None, None)
        .await?;
    Ok(snapshot_to_checkpoint(&session_id, &snapshot, vec![]))
}

#[tauri::command]
pub async fn clear_pending_operations(
    _session_id: String,
) -> Result<(), String> {
    // pending_checkpoint 队列已移除，此命令不再执行任何操作
    Ok(())
}
