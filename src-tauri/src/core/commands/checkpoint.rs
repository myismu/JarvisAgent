use crate::core::state::*;
use crate::core::checkpoint;

#[tauri::command]
pub async fn list_checkpoints(
    session_id: String,
    branch_name: Option<String>,
) -> Result<Vec<checkpoint::Checkpoint>, String> {
    Ok(checkpoint::list_checkpoints(&session_id, branch_name.as_deref()))
}

#[tauri::command]
pub async fn get_checkpoint_tree(
    session_id: String,
) -> Result<checkpoint::CheckpointTree, String> {
    Ok(checkpoint::get_checkpoint_tree(&session_id))
}

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

    let ctx = session_manager.get_or_create(&session_id).await;
    let is_empty;
    {
        let mut session = ctx.memory.lock().await;
        let trigger_preview = checkpoint.trigger_message.chars().take(100).collect::<String>();

        let mut target_idx = None;
        for (i, msg) in session.messages.iter().enumerate() {
            if let crate::core::models::Message::User { content } = msg {
                let msg_text = match content {
                    crate::core::models::Content::Single(s) => s.clone(),
                    crate::core::models::Content::Multiple(blocks) => {
                        blocks.iter()
                            .filter_map(|b| {
                                if let crate::core::models::ContentBlock::Text { text } = b {
                                    Some(text.as_str())
                                } else {
                                    None
                                }
                            })
                            .collect::<Vec<_>>()
                            .join(" ")
                    }
                };
                let msg_preview = msg_text.chars().take(100).collect::<String>();
                if msg_preview == trigger_preview {
                    target_idx = Some(i);
                    break;
                }
            }
        }

        if let Some(idx) = target_idx {
            session.messages.truncate(idx);
        }
        is_empty = session.messages.is_empty();
    }

    if is_empty {
        switch_away_and_delete_empty_session(&session_id, &app).await?;
    } else {
        let memory = ctx.memory.lock().await.clone();
        crate::core::sessions::save_session(&session_id, &memory, None);
        let _ = app.emit("session-updated", ());
    }

    Ok(restored_files)
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
pub async fn list_branches(
    session_id: String,
) -> Result<Vec<checkpoint::Branch>, String> {
    Ok(checkpoint::list_branches(&session_id))
}

#[tauri::command]
pub async fn delete_branch(
    session_id: String,
    branch_name: String,
) -> Result<(), String> {
    checkpoint::delete_branch(&session_id, &branch_name)
}

#[tauri::command]
pub async fn get_active_branch(
    session_id: String,
) -> Result<checkpoint::Branch, String> {
    Ok(checkpoint::get_active_branch(&session_id))
}

#[tauri::command]
pub async fn commit_checkpoint(
    session_id: String,
    trigger_message: String,
    agent_id: Option<String>,
    workspace_id: Option<String>,
    session_manager: tauri::State<'_, SessionManager>,
) -> Result<checkpoint::Checkpoint, String> {
    let ctx = session_manager.get_or_create(&session_id).await;
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
