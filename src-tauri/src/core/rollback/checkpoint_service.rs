//! # checkpoint_service.rs — 检查点查询、分支管理与文件回滚撤回领域服务
//!
//! 承载面向检查点/撤回工作流的核心业务编排，与 Tauri `State` 解耦：
//! - 稳定锚点（`message_id` / `seq`）撤回目标解析与快照轮次归一化
//! - 撤回预览（受影响文件与轮次统计）与文件+历史原子回滚
//! - 检查点列表/树视图与分支生命周期操作
//!
//! ## Key Exports
//! - `Checkpoint` / `FileOperation` / `OpType` / `Branch` / `CheckpointTree` / `BranchInfo` / `RollbackRecallResult` / `RollbackPreviewResult`
//! - `RollbackRecallParams`: 带撤回回滚请求参数结构体
//! - `list_checkpoints()` / `get_checkpoint_tree()` / `rollback_to_checkpoint()` / `preview_rollback_with_recall()` / `rollback_with_recall()`
//! - `create_branch()` / `switch_branch()` / `list_branches()` / `get_active_branch()` / `commit_checkpoint()`
//!
//! ## Dependencies
//! - Internal: `crate::core::rollback::{patch_store, SessionSnapshotManager, Snapshot}`, `crate::core::runtime::{SessionManager, SnapshotRegistry}`, `crate::core::session`
//! - External: `serde`, `tauri`, `ts_rs`

use crate::core::rollback::patch_store;
use crate::core::rollback::{PatchSummary, SessionSnapshotManager, Snapshot};
use crate::core::runtime::{SessionManager, SnapshotRegistry};
use crate::core::session::lifecycle::{recall_message_locked, switch_away_and_delete_session};
use crate::core::session::repository::StoredSessionMessage;
use crate::infra::types::models::{Content, ContentBlock, Message, MessageSource, SessionMemory};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use tauri::Emitter;

// ── 跨端兼容 DTO（供前端与 ts_bindings 使用） ──

#[derive(Serialize, Deserialize, Debug, Clone, ts_rs::TS)]
#[serde(rename_all = "camelCase")]
#[ts(rename_all = "camelCase")]
pub struct Checkpoint {
    pub id: String,
    pub session_id: String,
    #[ts(optional = nullable)]
    pub parent_id: Option<String>,
    pub branch_name: String,
    #[ts(optional = nullable)]
    pub agent_id: Option<String>,
    #[ts(optional = nullable)]
    pub workspace_id: Option<String>,
    #[ts(type = "number")]
    pub created_at: u64,
    pub trigger_message: String,
    pub operations: Vec<FileOperation>,
    pub metadata: HashMap<String, String>,
}

#[derive(Serialize, Deserialize, Debug, Clone, ts_rs::TS)]
#[serde(rename_all = "camelCase")]
#[ts(rename_all = "camelCase")]
pub struct FileOperation {
    pub op_type: OpType,
    pub path: String,
    #[ts(optional = nullable)]
    pub old_content_hash: Option<String>,
    #[ts(optional = nullable)]
    pub backup_path: Option<String>,
    #[ts(optional = nullable)]
    pub new_content_hash: Option<String>,
    #[ts(optional = nullable)]
    pub diff_summary: Option<String>,
}

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, ts_rs::TS)]
#[serde(rename_all = "lowercase")]
#[ts(rename_all = "lowercase")]
pub enum OpType {
    Edit,
    Write,
    Create,
    Delete,
    Rename,
}

#[derive(Serialize, Deserialize, Debug, Clone, ts_rs::TS)]
#[serde(rename_all = "camelCase")]
#[ts(rename_all = "camelCase")]
pub struct Branch {
    pub name: String,
    pub session_id: String,
    #[ts(optional = nullable)]
    pub head_checkpoint_id: Option<String>,
    #[ts(type = "number")]
    pub created_at: u64,
    #[ts(optional = nullable)]
    pub agent_id: Option<String>,
    pub description: String,
    pub is_active: bool,
}

#[derive(Serialize, Deserialize, Debug, Clone, ts_rs::TS)]
#[serde(rename_all = "camelCase")]
#[ts(rename_all = "camelCase")]
pub struct CheckpointTree {
    pub session_id: String,
    pub branches: Vec<BranchInfo>,
    pub checkpoints: Vec<Checkpoint>,
}

#[derive(Serialize, Deserialize, Debug, Clone, ts_rs::TS)]
#[serde(rename_all = "camelCase")]
#[ts(rename_all = "camelCase")]
pub struct BranchInfo {
    pub name: String,
    #[ts(optional = nullable)]
    pub head_checkpoint_id: Option<String>,
    pub checkpoint_count: usize,
    pub is_active: bool,
}

#[derive(serde::Serialize, ts_rs::TS)]
#[serde(rename_all = "camelCase")]
#[ts(rename_all = "camelCase")]
pub struct RollbackRecallResult {
    pub restored_files: Vec<String>,
    pub recalled_text: String,
    /// 回滚后**没能恢复**的对象（快照无内容 **且** 回收站副本缺失）。
    ///
    /// 非空 = 工作区与目标快照**不完全一致**，前端必须提示用户。
    #[serde(default)]
    pub restore_warnings: Vec<String>,
}

#[derive(serde::Serialize, ts_rs::TS)]
#[serde(rename_all = "camelCase")]
#[ts(rename_all = "camelCase")]
pub struct RollbackPreviewResult {
    pub target_checkpoint_id: String,
    pub checkpoint_label: String,
    pub files: Vec<PatchSummary>,
    /// 目标是否是"会话开始前"（撤销全部代码改动）。
    pub initial_state: bool,
    /// 被撤回消息是第几轮（从 1 开始）。
    pub round_number: usize,
    /// 从被撤回消息起、到会话结束共有多少轮。
    pub affected_rounds: usize,
}

/// 带撤回回滚请求参数（收口多参数入口）
pub struct RollbackRecallParams {
    pub session_id: String,
    pub checkpoint_id: String,
    pub rollback_files: bool,
    pub message_id: Option<String>,
    pub user_message_index: Option<usize>,
}

/// 文件回退目标。
///
/// `Initial` 只能由"该轮之前确实没有任何文件快照"推导出来；
/// 解析失败必须返回错误，**绝不能**用空字符串兜底成 Initial。
#[derive(Clone)]
enum FilesRollbackTarget {
    /// 退到该快照代表的文件状态（通常是"本轮开始前"的上一轮快照）。
    Snapshot(String),
    /// 回到会话开始前（撤销全部代码改动）。
    Initial,
}

#[derive(Clone)]
struct RollbackTarget {
    clicked_message_id: String,
    clicked_seq: usize,
    clicked_created_at: u64,
    recalled_text: String,
    round_number: usize,
    affected_rounds: usize,
    /// `None` = 本次不涉及文件回退（仅撤回会话）。
    files_target: Option<FilesRollbackTarget>,
}

fn message_text(content: &Content) -> String {
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

fn checkpoint_matches_user_message(trigger: &str, message: &str) -> bool {
    let trigger = trigger.trim();
    let message = message.trim();
    if trigger.is_empty() || message.is_empty() {
        return false;
    }

    message == trigger || message.starts_with(trigger) || trigger.starts_with(message)
}

fn find_checkpoint_user_message(
    messages: &[Message],
    trigger_message: &str,
) -> Option<(usize, String)> {
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

fn log_rollback_abort(session_id: &str, reason: &str) -> String {
    let message = format!("{}，已保留会话历史", reason);
    println!("[Rollback] 会话 {} 撤回中止: {}", session_id, message);
    message
}

/// 这条消息是否能被用户当作"一轮"的起点（用于计算轮次范围）。
///
/// 与历史接口的口径一致：真实用户输入（Chat 来源）且正文里有可渲染内容
/// （Text 或 Image）。工具结果虽然也是 user role，但不算一轮。
fn is_user_round_start(stored: &StoredSessionMessage) -> bool {
    if stored.role != "user" || !stored.source.rendered_in_ui() {
        return false;
    }
    if stored.source == MessageSource::Compact {
        return false;
    }
    match &stored.content {
        Message::User { content } => match content {
            Content::Single(text) => !text.trim().is_empty(),
            Content::Multiple(blocks) => blocks.iter().any(|block| match block {
                ContentBlock::Text { text } => !text.trim().is_empty(),
                ContentBlock::Image { .. } => true,
                _ => false,
            }),
        },
        _ => false,
    }
}

/// 解析"用户点的是哪条消息"。返回 `(消息, 第几轮, 从这条起共几轮)`。
///
/// `message_id` 是唯一首选锚点；`user_message_index` 只在没有 ID 时作为
/// **可见消息表下标**兜底（不再接受前端聚合气泡下标）。
fn resolve_clicked_message(
    session_id: &str,
    message_id: Option<&str>,
    user_message_index: Option<usize>,
) -> Result<(StoredSessionMessage, usize, usize), String> {
    let visible = crate::core::session::list_visible_session_messages(session_id)?;
    let position = if let Some(message_id) = message_id.filter(|id| !id.trim().is_empty()) {
        visible
            .iter()
            .position(|message| message.message_id == message_id)
            .ok_or_else(|| log_rollback_abort(session_id, "撤回消息不存在"))?
    } else if let Some(index) = user_message_index {
        if index >= visible.len() {
            return Err(log_rollback_abort(session_id, "撤回消息不存在"));
        }
        index
    } else {
        return Err(log_rollback_abort(
            session_id,
            "无法定位撤回目标：缺少 message_id",
        ));
    };

    let stored = visible[position].clone();
    if !is_user_round_start(&stored) {
        return Err(log_rollback_abort(session_id, "撤回目标不是用户消息"));
    }

    let mut round_number = 0usize;
    let mut affected_rounds = 0usize;
    for message in &visible {
        if !is_user_round_start(message) {
            continue;
        }
        if message.seq < stored.seq {
            round_number += 1;
        } else {
            affected_rounds += 1;
        }
    }
    round_number += 1;
    affected_rounds = affected_rounds.max(1);

    Ok((stored, round_number, affected_rounds))
}

fn snapshot_trigger_message_id(snapshot: &Snapshot) -> Option<&str> {
    snapshot
        .metadata
        .get("trigger_user_message_id")
        .map(|value| value.trim())
        .filter(|value| !value.is_empty())
}

/// 把"检查点快照"归一成同一轮末尾的普通快照。
///
/// 自动检查点是轮末的加速节点，`parent` 才是同一状态的普通快照；
/// 若直接把检查点的 parent 当撤回目标，会把**本轮改动也一起保留**（少撤一轮）。
async fn normalized_round_snapshot(
    manager: &SessionSnapshotManager,
    snapshot_id: &str,
) -> Result<Snapshot, String> {
    let mut snapshot = manager
        .get_snapshot(snapshot_id)
        .await?
        .ok_or_else(|| format!("目标快照 {} 不存在", snapshot_id))?;

    if snapshot.is_checkpoint {
        if let Some(parent_id) = snapshot.parent_id.clone() {
            if let Some(parent) = manager.get_snapshot(&parent_id).await? {
                let same_round = match (
                    snapshot_trigger_message_id(&snapshot),
                    snapshot_trigger_message_id(&parent),
                ) {
                    (Some(current), Some(parent_trigger)) => current == parent_trigger,
                    _ => match (snapshot.message.as_deref(), parent.message.as_deref()) {
                        (Some(current), Some(parent_message)) => {
                            !current.trim().is_empty() && current == parent_message
                        }
                        _ => false,
                    },
                };
                if same_round {
                    snapshot = parent;
                }
            }
        }
    }

    Ok(snapshot)
}

/// 由"本轮快照"推导文件回退目标：回到它的父节点；父节点为空 = 会话开始前。
async fn files_target_from_round_snapshot(
    manager: &SessionSnapshotManager,
    snapshot: Snapshot,
) -> Result<FilesRollbackTarget, String> {
    match snapshot.parent_id {
        Some(parent_id) => {
            if manager.get_snapshot(&parent_id).await?.is_none() {
                return Err(format!(
                    "目标快照的父节点 {} 不存在，无法确定撤回范围",
                    parent_id
                ));
            }
            Ok(FilesRollbackTarget::Snapshot(parent_id))
        }
        None => Ok(FilesRollbackTarget::Initial),
    }
}

/// 校验传入/关联的快照确实属于"被点消息这一轮或之后"。
fn ensure_snapshot_not_before(
    session_id: &str,
    snapshot: &Snapshot,
    clicked_seq: usize,
    seq_by_message_id: &HashMap<&str, usize>,
) -> Result<(), String> {
    let Some(trigger_id) = snapshot_trigger_message_id(snapshot) else {
        return Ok(());
    };
    let Some(trigger_seq) = seq_by_message_id.get(trigger_id) else {
        return Err(log_rollback_abort(
            session_id,
            "关联快照的触发消息已不在当前会话历史中，未执行撤回",
        ));
    };
    if *trigger_seq < clicked_seq {
        return Err(log_rollback_abort(
            session_id,
            &format!(
                "关联的文件快照属于更早的轮次（seq {} < {}），未执行撤回",
                trigger_seq, clicked_seq
            ),
        ));
    }
    Ok(())
}

/// 解析文件回退目标（按数据库关联 → 前端显式 ID → 快照元数据 → 其后最早文件快照）。
async fn resolve_files_target(
    session_id: &str,
    manager: &SessionSnapshotManager,
    clicked: &StoredSessionMessage,
    explicit_checkpoint_id: Option<&str>,
) -> Result<Option<FilesRollbackTarget>, String> {
    let visible = crate::core::session::list_visible_session_messages(session_id)?;
    let seq_by_message_id: HashMap<&str, usize> = visible
        .iter()
        .map(|message| (message.message_id.as_str(), message.seq))
        .collect();
    let chain_ids = manager.current_chain_ids().await;

    if let Some(link) =
        patch_store::find_checkpoint_user_message_link(session_id, &clicked.message_id)?
    {
        if !link.checkpoint_id.is_empty() {
            let snapshot = normalized_round_snapshot(manager, &link.checkpoint_id)
                .await
                .map_err(|error| {
                    log_rollback_abort(
                        session_id,
                        &format!("本轮文件快照不可用（{}），本次未执行撤回", error),
                    )
                })?;
            if !chain_ids.contains(&snapshot.id) {
                return Err(log_rollback_abort(
                    session_id,
                    "本轮文件快照不在当前分支链路上，未执行撤回",
                ));
            }
            ensure_snapshot_not_before(session_id, &snapshot, clicked.seq, &seq_by_message_id)?;
            return files_target_from_round_snapshot(manager, snapshot)
                .await
                .map(Some);
        }
    }

    if let Some(checkpoint_id) = explicit_checkpoint_id
        .map(str::trim)
        .filter(|value| !value.is_empty())
    {
        let snapshot = normalized_round_snapshot(manager, checkpoint_id).await?;
        if !chain_ids.contains(&snapshot.id) {
            return Err(log_rollback_abort(
                session_id,
                "目标快照不在当前分支链路上，未执行撤回",
            ));
        }
        ensure_snapshot_not_before(session_id, &snapshot, clicked.seq, &seq_by_message_id)?;
        return files_target_from_round_snapshot(manager, snapshot)
            .await
            .map(Some);
    }

    let snapshots = manager.list_snapshots(None).await;

    if let Some(snapshot) = snapshots.iter().find(|snapshot| {
        !snapshot.is_checkpoint
            && chain_ids.contains(&snapshot.id)
            && snapshot_trigger_message_id(snapshot) == Some(clicked.message_id.as_str())
    }) {
        return files_target_from_round_snapshot(manager, snapshot.clone())
            .await
            .map(Some);
    }

    let next_snapshot = snapshots
        .iter()
        .filter(|snapshot| !snapshot.is_checkpoint && chain_ids.contains(&snapshot.id))
        .filter_map(|snapshot| {
            let trigger_id = snapshot_trigger_message_id(snapshot)?;
            let trigger_seq = *seq_by_message_id.get(trigger_id)?;
            if trigger_seq > clicked.seq {
                Some((trigger_seq, snapshot.created_at, snapshot.id.clone()))
            } else {
                None
            }
        })
        .min_by_key(|(seq, created_at, _)| (*seq, *created_at));

    if let Some((_, _, snapshot_id)) = next_snapshot {
        let snapshot = normalized_round_snapshot(manager, &snapshot_id).await?;
        return files_target_from_round_snapshot(manager, snapshot)
            .await
            .map(Some);
    }

    Ok(None)
}

async fn resolve_rollback_target(
    session_id: &str,
    checkpoint_id: &str,
    message_id: Option<&str>,
    user_message_index: Option<usize>,
    resolve_files: bool,
    registry: &SnapshotRegistry,
) -> Result<RollbackTarget, String> {
    let (clicked, round_number, affected_rounds) =
        resolve_clicked_message(session_id, message_id, user_message_index)?;

    let files_target = if resolve_files {
        let manager = registry.0.read().await.get_or_create(session_id).await?;
        let target = resolve_files_target(
            session_id,
            &manager,
            &clicked,
            Some(checkpoint_id).filter(|value| !value.trim().is_empty()),
        )
        .await?;
        if target.is_none() {
            return Err(log_rollback_abort(
                session_id,
                "无法定位本轮或之后的文件快照，本次未执行撤回",
            ));
        }
        target
    } else {
        None
    };

    let recalled_text = match &clicked.content {
        Message::User { content } => message_text(content),
        _ => String::new(),
    };

    Ok(RollbackTarget {
        clicked_message_id: clicked.message_id.clone(),
        clicked_seq: clicked.seq,
        clicked_created_at: clicked.created_at,
        recalled_text,
        round_number,
        affected_rounds,
        files_target,
    })
}

async fn rollback_files_to_snapshot(
    app: &tauri::AppHandle,
    registry: &SnapshotRegistry,
    session_id: &str,
    snapshot_id: &str,
) -> Result<(Vec<String>, Vec<String>), String> {
    let manager = registry.0.read().await.get_or_create(session_id).await?;
    if !snapshot_id.is_empty() && manager.get_snapshot(snapshot_id).await?.is_none() {
        return Err(log_rollback_abort(
            session_id,
            &format!("文件回滚失败：目标快照 {} 不存在", snapshot_id),
        ));
    }

    let target_label = if snapshot_id.is_empty() {
        "初始状态"
    } else {
        snapshot_id
    };

    let target_dir = crate::core::runtime::effective_workspace(app, session_id)
        .await
        .unwrap_or_default();
    let outcome = if snapshot_id.is_empty() {
        manager
            .rollback_to_initial_state(&target_dir)
            .await
            .map_err(|e| log_rollback_abort(session_id, &format!("文件回滚到初始状态失败: {}", e)))?
            .1
    } else {
        manager
            .rollback_to(snapshot_id, &target_dir)
            .await
            .map_err(|e| log_rollback_abort(session_id, &format!("文件回滚失败: {}", e)))?
            .1
    };
    println!("[Rollback] 会话 {} 已恢复到 {}", session_id, target_label);
    if !outcome.restore_failures.is_empty() {
        eprintln!(
            "[Rollback] 会话 {} 有 {} 个对象未能恢复（快照无内容且回收站副本缺失）: {:?}",
            session_id,
            outcome.restore_failures.len(),
            outcome.restore_failures
        );
    }

    Ok((
        vec!["文件已恢复到检查点状态".to_string()],
        outcome.restore_failures,
    ))
}

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

fn prune_metadata_after_checkpoint(session: &mut SessionMemory, cutoff_secs: u64) {
    if cutoff_secs == 0 {
        session.plan_documents.clear();
        return;
    }

    session
        .plan_documents
        .retain(|plan| plan.created_at <= cutoff_secs);
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

fn find_user_message_index_by_display(
    session: &SessionMemory,
    trigger_message: &str,
) -> Option<usize> {
    session
        .messages
        .iter()
        .enumerate()
        .rev()
        .find_map(|(index, message)| {
            if let Message::User { content } = message {
                let text = message_text(content);
                if checkpoint_matches_user_message(trigger_message, &text) {
                    return Some(index);
                }
            }
            None
        })
}

// ── 对外领域服务入口 ──

pub async fn list_checkpoints(
    session_id: &str,
    branch_name: Option<&str>,
    registry: &SnapshotRegistry,
) -> Result<Vec<Checkpoint>, String> {
    let manager = registry.0.read().await.get_or_create(session_id).await?;
    let snapshots = manager.list_snapshots(branch_name).await;

    let checkpoints: Vec<_> = snapshots
        .iter()
        .filter(|s| s.is_checkpoint)
        .map(|s| snapshot_to_checkpoint(session_id, s, vec![]))
        .collect();

    Ok(checkpoints)
}

pub async fn get_checkpoint_tree(
    session_id: &str,
    registry: &SnapshotRegistry,
) -> Result<CheckpointTree, String> {
    let manager = registry.0.read().await.get_or_create(session_id).await?;
    let tree_view = manager.get_tree_view().await;
    let snapshots = manager.list_snapshots(None).await;

    let branches: Vec<BranchInfo> = tree_view
        .branches
        .iter()
        .map(|b| BranchInfo {
            name: b.name.clone(),
            head_checkpoint_id: Some(b.root.id.clone()).filter(|id| !id.is_empty()),
            checkpoint_count: snapshots
                .iter()
                .filter(|s| s.branch_name == b.name && s.is_checkpoint)
                .count(),
            is_active: b.is_active,
        })
        .collect();

    let checkpoints: Vec<_> = snapshots
        .iter()
        .filter(|s| s.is_checkpoint)
        .map(|s| snapshot_to_checkpoint(session_id, s, vec![]))
        .collect();

    Ok(CheckpointTree {
        session_id: session_id.to_string(),
        branches,
        checkpoints,
    })
}

pub async fn rollback_to_checkpoint(
    session_id: &str,
    checkpoint_id: &str,
    rollback_files: bool,
    session_manager: &SessionManager,
    registry: &SnapshotRegistry,
    app: &tauri::AppHandle,
) -> Result<Vec<String>, String> {
    let mut restored_files = Vec::new();
    let mut restore_warnings: Vec<String> = Vec::new();

    let effective_checkpoint_id = if !checkpoint_id.is_empty() {
        Some(checkpoint_id.to_string())
    } else {
        let mgr = registry.0.read().await.get_or_create(session_id).await?;
        mgr.find_nearest_checkpoint_before().await
    };

    if rollback_files {
        if let Some(effective_id) = &effective_checkpoint_id {
            crate::infra::background::BackgroundManager::kill_all_background(app).await;
            let (messages, warnings) =
                rollback_files_to_snapshot(app, registry, session_id, effective_id).await?;
            restored_files = messages;
            restore_warnings = warnings;
        }
    }

    let snapshot_message = if !checkpoint_id.is_empty() {
        match registry.0.read().await.get_or_create(session_id).await {
            Ok(mgr) => match mgr.get_snapshot(checkpoint_id).await {
                Ok(Some(snap)) => snap.message.clone().unwrap_or_default(),
                _ => String::new(),
            },
            _ => String::new(),
        }
    } else {
        String::new()
    };

    let ctx = session_manager.get_or_create(session_id).await;
    let is_empty;
    {
        let mut session = ctx.memory.lock().await;
        if !snapshot_message.is_empty() {
            if let Some((idx, _)) =
                find_checkpoint_user_message(&session.messages, &snapshot_message)
            {
                session.messages.truncate(idx);
                session.message_ids.truncate(idx);
            }
        }
        let cutoff = if !checkpoint_id.is_empty() {
            match registry.0.read().await.get_or_create(session_id).await {
                Ok(mgr) => match mgr.get_snapshot(checkpoint_id).await {
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
        is_empty = session.messages.is_empty();
    }

    if is_empty {
        switch_away_and_delete_session(session_id, app).await?;
    } else {
        let memory = ctx.memory.lock().await.clone();
        crate::core::session::save_session(session_id, &memory, None);
        let _ = app.emit("session-updated", ());
        crate::core::session::notify_messages_changed(app, session_id, "rollback");
    }

    if !restore_warnings.is_empty() {
        restored_files.push(format!(
            "以下对象未能恢复（快照无内容且回收站副本已不在）：{}",
            restore_warnings.join("、")
        ));
    }

    Ok(restored_files)
}

pub async fn preview_rollback_with_recall(
    session_id: &str,
    checkpoint_id: &str,
    message_id: Option<&str>,
    user_message_index: Option<usize>,
    registry: &SnapshotRegistry,
) -> Result<RollbackPreviewResult, String> {
    let target = resolve_rollback_target(
        session_id,
        checkpoint_id,
        message_id,
        user_message_index,
        true,
        registry,
    )
    .await?;

    let manager = registry.0.read().await.get_or_create(session_id).await?;
    let (target_checkpoint_id, files) = match target.files_target.clone() {
        Some(FilesRollbackTarget::Snapshot(snapshot_id)) => {
            let files = manager
                .preview_touched_files_to(&snapshot_id)
                .await
                .map_err(|e| log_rollback_abort(session_id, &e))?;
            (snapshot_id, files)
        }
        Some(FilesRollbackTarget::Initial) => {
            let files = manager
                .preview_touched_files_to("")
                .await
                .map_err(|e| log_rollback_abort(session_id, &e))?;
            (String::new(), files)
        }
        None => {
            return Err(log_rollback_abort(
                session_id,
                "无法定位文件回退目标，本次未执行撤回",
            ))
        }
    };

    Ok(RollbackPreviewResult {
        checkpoint_label: if target_checkpoint_id.is_empty() {
            "会话开始前".to_string()
        } else {
            format!("第 {} 轮之前", target.round_number)
        },
        initial_state: target_checkpoint_id.is_empty(),
        round_number: target.round_number,
        affected_rounds: target.affected_rounds,
        target_checkpoint_id,
        files,
    })
}

pub async fn rollback_with_recall(
    params: RollbackRecallParams,
    session_manager: &SessionManager,
    registry: &SnapshotRegistry,
    app: &tauri::AppHandle,
) -> Result<RollbackRecallResult, String> {
    let mut restored_files = Vec::new();
    let mut restore_warnings: Vec<String> = Vec::new();

    // 先拿执行槽再做解析：解析出的目标必须对应"冻结"的会话状态
    let ctx = session_manager.get_or_create(&params.session_id).await;
    let _execution_guard = ctx.execution_gate.lock().await;
    if ctx.has_active_execution() {
        return Err("当前任务尚未结束，请等待任务停止后再撤回。".to_string());
    }

    let target = resolve_rollback_target(
        &params.session_id,
        &params.checkpoint_id,
        params.message_id.as_deref(),
        params.user_message_index,
        params.rollback_files,
        registry,
    )
    .await?;

    let cutoff = target.clicked_created_at;

    if params.rollback_files {
        let files_target = target.files_target.clone().ok_or_else(|| {
            log_rollback_abort(&params.session_id, "无法定位文件回退目标，本次未执行撤回")
        })?;
        let (target_id, target_label) = match &files_target {
            FilesRollbackTarget::Snapshot(snapshot_id) => {
                (snapshot_id.clone(), snapshot_id.clone())
            }
            FilesRollbackTarget::Initial => (String::new(), "会话开始前".to_string()),
        };
        println!(
            "[Rollback] 会话 {} 准备执行文件回滚: message_id={}, seq={}, round={}, affected={}, target={}",
            params.session_id,
            target.clicked_message_id,
            target.clicked_seq,
            target.round_number,
            target.affected_rounds,
            target_label
        );
        crate::infra::background::BackgroundManager::kill_all_background(app).await;
        let (messages, warnings) =
            rollback_files_to_snapshot(app, registry, &params.session_id, &target_id).await?;
        restored_files = messages;
        restore_warnings = warnings;
    }

    recall_message_locked(
        &params.session_id,
        Some(target.clicked_message_id.clone()).filter(|id| !id.trim().is_empty()),
        None,
        Some(cutoff),
        &ctx,
        app,
    )
    .await?;

    Ok(RollbackRecallResult {
        restored_files,
        recalled_text: target.recalled_text,
        restore_warnings,
    })
}

pub async fn create_branch(
    session_id: &str,
    branch_name: String,
    from_checkpoint_id: Option<String>,
    agent_id: Option<String>,
    description: Option<String>,
    registry: &SnapshotRegistry,
) -> Result<Branch, String> {
    let manager = registry.0.read().await.get_or_create(session_id).await?;
    manager
        .create_branch(
            branch_name.clone(),
            from_checkpoint_id,
            agent_id,
            description,
        )
        .await?;
    let branches = manager.list_branches().await;
    branches
        .into_iter()
        .find(|b| b.name == branch_name)
        .map(|b| snapshot_branch_to_checkpoint_branch(&b))
        .ok_or_else(|| format!("创建分支 '{}' 后无法找到", branch_name))
}

pub async fn switch_branch(
    session_id: &str,
    branch_name: &str,
    registry: &SnapshotRegistry,
) -> Result<Branch, String> {
    let manager = registry.0.read().await.get_or_create(session_id).await?;
    manager.switch_branch(branch_name).await?;
    let branches = manager.list_branches().await;
    branches
        .into_iter()
        .find(|b| b.name == branch_name)
        .map(|b| snapshot_branch_to_checkpoint_branch(&b))
        .ok_or_else(|| format!("分支 '{}' 不存在", branch_name))
}

pub async fn list_branches(
    session_id: &str,
    registry: &SnapshotRegistry,
) -> Result<Vec<Branch>, String> {
    let manager = registry.0.read().await.get_or_create(session_id).await?;
    Ok(manager
        .list_branches()
        .await
        .iter()
        .map(snapshot_branch_to_checkpoint_branch)
        .collect())
}

pub async fn get_active_branch(
    session_id: &str,
    registry: &SnapshotRegistry,
) -> Result<Branch, String> {
    let manager = registry.0.read().await.get_or_create(session_id).await?;
    let current = manager.get_current_branch().await;
    let branches = manager.list_branches().await;
    branches
        .into_iter()
        .find(|b| b.name == current)
        .map(|b| snapshot_branch_to_checkpoint_branch(&b))
        .ok_or_else(|| "无活跃分支".to_string())
}

pub async fn commit_checkpoint(
    session_id: &str,
    trigger_message: String,
    registry: &SnapshotRegistry,
) -> Result<Checkpoint, String> {
    let manager = registry.0.read().await.get_or_create(session_id).await?;
    let trigger_user_message_id = crate::core::session::load_session(session_id)
        .ok()
        .and_then(|session| {
            let index = find_user_message_index_by_display(&session, &trigger_message)?;
            session.message_ids.get(index).cloned()
        });
    let snapshot = manager
        .create_checkpoint_snapshot(
            Some(trigger_message),
            None,
            None,
            trigger_user_message_id.clone(),
        )
        .await?;
    if let Some(message_id) = trigger_user_message_id.as_deref() {
        let _ = patch_store::upsert_checkpoint_user_message_link(
            session_id,
            message_id,
            &snapshot.id,
            true,
            snapshot.created_at,
        );
    }
    Ok(snapshot_to_checkpoint(session_id, &snapshot, vec![]))
}
