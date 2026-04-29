//! # checkpoint.rs — 检查点与分支管理 Tauri 命令
//!
//! 提供检查点的列表查询、树形视图、回滚（含文件恢复和消息撤回）、
//! 分支的创建/切换/删除，以及手动提交检查点等命令。
//!
//! ## 关键导出
//! - `list_checkpoints()` / `get_checkpoint_tree()`: 检查点查询
//! - `rollback_to_checkpoint()`: 回滚到指定检查点（可选文件恢复）
//! - `rollback_to_checkpoint_with_recall()`: 回滚并返回被撤回的用户消息
//! - `create_branch()` / `switch_branch()` / `delete_branch()`: 分支管理
//! - `commit_checkpoint()`: 手动提交检查点（含待处理文件操作）
//! - `clear_pending_operations()`: 清空待处理的文件操作队列
//!
//! ## 约束
//! - 回滚会截断消息历史到对应用户消息位置，并清理后续的 agent_steps 和 plan_documents
//! - 回滚后若会话为空，自动切换到其他会话并删除空会话

use crate::core::session::checkpoint;
use crate::core::state::*;

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

/// 获取目标检查点的前一个检查点时间戳，用于清理后续元数据
fn checkpoint_cutoff_before(session_id: &str, checkpoint_id: &str) -> u64 {
    let checkpoints = checkpoint::list_checkpoints(session_id, None);
    let Some(index) = checkpoints.iter().position(|cp| cp.id == checkpoint_id) else {
        return 0;
    };

    // 第一个检查点之前无 cutoff，返回 0 表示清理全部
    if index == 0 {
        0
    } else {
        checkpoints[index - 1].created_at
    }
}

/// 清理检查点之后的 agent_steps 和 plan_documents（基于时间戳）
fn prune_metadata_after_checkpoint(session: &mut crate::core::models::SessionMemory, cutoff_secs: u64) {
    if cutoff_secs == 0 {
        // 无 cutoff（回滚到第一个检查点），清理全部
        session.agent_steps.clear();
        session.plan_documents.clear();
        return;
    }

    session.plan_documents.retain(|plan| plan.created_at <= cutoff_secs);
    session.agent_steps.retain(|step| {
        // 兼容毫秒和秒两种时间戳格式
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
) -> Result<Vec<checkpoint::Checkpoint>, String> {
    Ok(checkpoint::list_checkpoints(
        &session_id,
        branch_name.as_deref(),
    ))
}

#[tauri::command]
pub async fn get_checkpoint_tree(session_id: String) -> Result<checkpoint::CheckpointTree, String> {
    Ok(checkpoint::get_checkpoint_tree(&session_id))
}

/// 回滚到指定检查点：可选恢复文件、截断消息历史、清理后续元数据
#[tauri::command]
pub async fn rollback_to_checkpoint(
    session_id: String,
    checkpoint_id: String,
    rollback_files: Option<bool>,
    session_manager: tauri::State<'_, SessionManager>,
    app: tauri::AppHandle,
) -> Result<Vec<String>, String> {
    use crate::core::commands::session::switch_away_and_delete_empty_session;
    use tauri::Emitter;

    let mut restored_files = Vec::new();

    if rollback_files.unwrap_or(false) {
        restored_files = checkpoint::rollback_to_checkpoint(&session_id, &checkpoint_id)?;
    }

    let checkpoint = checkpoint::list_branches(&session_id)
        .iter()
        .find_map(|b| checkpoint::load_checkpoint(&session_id, &b.name, &checkpoint_id))
        .ok_or_else(|| format!("检查点 '{}' 不存在", checkpoint_id))?;
    let metadata_cutoff = checkpoint_cutoff_before(&session_id, &checkpoint_id);

    let ctx = session_manager.get_or_create(&session_id).await;
    let is_empty;
    {
        let mut session = ctx.memory.lock().await;
        if let Some((idx, _)) = find_checkpoint_user_message(&session.messages, &checkpoint.trigger_message) {
            session.messages.truncate(idx);
        }
        prune_metadata_after_checkpoint(&mut session, metadata_cutoff);
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
#[tauri::command]
pub async fn rollback_to_checkpoint_with_recall(
    session_id: String,
    checkpoint_id: String,
    rollback_files: Option<bool>,
    session_manager: tauri::State<'_, SessionManager>,
    app: tauri::AppHandle,
) -> Result<RollbackRecallResult, String> {
    use crate::core::commands::session::switch_away_and_delete_empty_session;
    use tauri::Emitter;

    let mut restored_files = Vec::new();

    if rollback_files.unwrap_or(false) {
        restored_files = checkpoint::rollback_to_checkpoint(&session_id, &checkpoint_id)?;
    }

    let checkpoint = checkpoint::list_branches(&session_id)
        .iter()
        .find_map(|b| checkpoint::load_checkpoint(&session_id, &b.name, &checkpoint_id))
        .ok_or_else(|| format!("检查点 '{}' 不存在", checkpoint_id))?;
    let metadata_cutoff = checkpoint_cutoff_before(&session_id, &checkpoint_id);

    let ctx = session_manager.get_or_create(&session_id).await;
    let recalled_text;
    let is_empty;
    {
        let mut session = ctx.memory.lock().await;
        let (idx, recalled) = find_checkpoint_user_message(&session.messages, &checkpoint.trigger_message)
            .ok_or_else(|| "无法在会话中找到该检查点对应的用户消息".to_string())?;
        session.messages.truncate(idx);
        prune_metadata_after_checkpoint(&mut session, metadata_cutoff);
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

#[tauri::command]
pub async fn create_branch(
    session_id: String,
    branch_name: String,
    from_checkpoint_id: Option<String>,
    agent_id: Option<String>,
    description: Option<String>,
) -> Result<checkpoint::Branch, String> {
    Ok(checkpoint::create_branch(
        &session_id,
        &branch_name,
        from_checkpoint_id.as_deref(),
        agent_id.as_deref(),
        description.as_deref(),
    ))
}

#[tauri::command]
pub async fn switch_branch(
    session_id: String,
    branch_name: String,
) -> Result<checkpoint::Branch, String> {
    checkpoint::switch_branch(&session_id, &branch_name)
}

#[tauri::command]
pub async fn list_branches(session_id: String) -> Result<Vec<checkpoint::Branch>, String> {
    Ok(checkpoint::list_branches(&session_id))
}

#[tauri::command]
pub async fn delete_branch(session_id: String, branch_name: String) -> Result<(), String> {
    checkpoint::delete_branch(&session_id, &branch_name)
}

#[tauri::command]
pub async fn get_active_branch(session_id: String) -> Result<checkpoint::Branch, String> {
    Ok(checkpoint::get_active_branch(&session_id))
}

/// 手动提交检查点，消费待处理的文件操作队列
#[tauri::command]
pub async fn commit_checkpoint(
    session_id: String,
    trigger_message: String,
    agent_id: Option<String>,
    workspace_id: Option<String>,
    session_manager: tauri::State<'_, SessionManager>,
) -> Result<checkpoint::Checkpoint, String> {
    let ctx = session_manager.get_or_create(&session_id).await;
    // 消费并清空待处理的文件操作
    let operations = ctx.pending_checkpoint.lock().await.drain(..).collect();
    let parent_id = checkpoint::get_head_checkpoint_id(&session_id);

    Ok(checkpoint::create_checkpoint(
        &session_id,
        parent_id.as_deref(),
        &trigger_message,
        agent_id.as_deref(),
        workspace_id.as_deref(),
        operations,
    ))
}

#[tauri::command]
pub async fn clear_pending_operations(
    session_id: String,
    session_manager: tauri::State<'_, SessionManager>,
) -> Result<(), String> {
    let ctx = session_manager.get_or_create(&session_id).await;
    ctx.pending_checkpoint.lock().await.clear();
    Ok(())
}
