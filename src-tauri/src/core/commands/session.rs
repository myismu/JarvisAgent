use crate::core::models::*;
use crate::core::sessions;
use crate::core::state::*;
use crate::core::api_client;
use tauri::Emitter;

#[tauri::command]
pub async fn get_active_session_id() -> Result<Option<String>, String> {
    // 前端自行维护 active_session_id。
    // 如果前端需要读取最后一次的 session：
    Ok(sessions::get_last_active_session_id())
}

#[tauri::command]
pub async fn list_sessions() -> Result<Vec<sessions::SessionMeta>, String> {
    Ok(sessions::list_sessions())
}

#[tauri::command]
pub async fn create_session(
    session_manager: tauri::State<'_, SessionManager>,
    working_directory: Option<String>,
) -> Result<sessions::SessionMeta, String> {
    println!("[DEBUG] create_session called with working_directory: {:?}", working_directory);

    let validated_dir = if let Some(ref dir) = working_directory {
        let path = std::path::Path::new(dir);
        if !path.exists() || !path.is_dir() {
            return Err(format!("目录不存在或不是文件夹: {}", dir));
        }
        Some(dir.clone())
    } else {
        None
    };

    let meta = sessions::create_session(validated_dir.clone());
    
    // 初始化上下文
    let ctx = session_manager.get_or_create(&meta.id).await;
    *ctx.workspace.lock().await = validated_dir.map(std::path::PathBuf::from);

    Ok(meta)
}

#[tauri::command]
pub async fn switch_session(
    id: String,
    session_manager: tauri::State<'_, SessionManager>,
) -> Result<sessions::SessionMeta, String> {
    // 前端通知切换到了该 session，预加载到内存
    let _ = session_manager.get_or_create(&id).await;
    let meta = sessions::get_session_meta(&id)?;
    println!("[DEBUG] switch_session: id={}, working_directory={:?}", id, meta.working_directory);
    Ok(meta)
}

pub async fn switch_away_and_delete_empty_session(
    deleted_session_id: &str,
    app: &tauri::AppHandle,
) -> Result<(), String> {
    let fallback = sessions::list_sessions()
        .into_iter()
        .find(|session| session.id != deleted_session_id);

    let next_active_session_id = if let Some(meta) = fallback {
        meta.id
    } else {
        let meta = sessions::create_session(None);
        meta.id
    };

    sessions::delete_session(deleted_session_id)?;

    let _ = app.emit(
        "active-session-changed",
        SessionCleanupResult {
            deleted_session_id: Some(deleted_session_id.to_string()),
            active_session_id: Some(next_active_session_id),
        },
    );
    let _ = app.emit("session-updated", ());

    Ok(())
}

pub async fn auto_name_session(
    app: tauri::AppHandle,
    session_id: String,
    memory: SessionMemory,
) -> Result<(), String> {
    if memory.messages.is_empty() {
        return Ok(());
    }

    let cfg = crate::core::config::load_config();
    let agent_cfg = cfg.active_config();
    let model_id = &agent_cfg.utility_model;
    let api_key = &agent_cfg.api_key;
    let base_url = &agent_cfg.base_url;
    let api_format = agent_cfg.api_format_enum();

    let mut text_to_summarize = String::new();
    for msg in memory.messages.iter().take(4) {
        if let Ok(m) = serde_json::to_string(msg) {
            text_to_summarize.push_str(&m);
            text_to_summarize.push('\n');
        }
    }

    let summary_prompt = format!("请根据以下对话内容，给出一个极简的会话名称（不超过10个字，不要有任何解释，不要包含标点符号和引号）：\n\n{}", text_to_summarize);

    let client = reqwest::Client::new();
    let title = api_client::call_llm_simple(
        &client,
        api_key,
        base_url,
        model_id,
        api_format,
        "你是一个专门用于提取会话名称的助手。只输出名称本身。",
        &summary_prompt,
        50,
    )
    .await
    .unwrap_or_default();

    let title = title
        .trim()
        .trim_matches('"')
        .trim_matches('\'')
        .to_string();
    if !title.is_empty() {
        let _ = sessions::rename_session(&session_id, &title, true);
        let _ = app.emit("session-renamed", ());
    }

    Ok(())
}

#[tauri::command]
pub async fn recall_last_message(
    session_id: String,
    session_manager: tauri::State<'_, SessionManager>,
    app: tauri::AppHandle,
) -> Result<String, String> {
    let ctx = session_manager.get_or_create(&session_id).await;
    let recalled_text;
    let is_empty;
    {
        let mut session = ctx.memory.lock().await;
        let last_user_idx = session.messages.iter().rposition(|m| matches!(m, Message::User { .. }));
        if let Some(idx) = last_user_idx {
            if let Message::User { content } = &session.messages[idx] {
                recalled_text = match content {
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
                };
            } else {
                recalled_text = String::new();
            }
            session.messages.truncate(idx);
        } else {
            return Err("没有可撤回的用户消息".to_string());
        }
        is_empty = session.messages.is_empty();
    }

    if is_empty {
        switch_away_and_delete_empty_session(&session_id, &app).await?;
    } else {
        let memory = ctx.memory.lock().await.clone();
        sessions::save_session(&session_id, &memory, None);
        let _ = app.emit("session-updated", ());
    }

    Ok(recalled_text)
}

#[tauri::command]
pub async fn delete_session(
    id: String,
) -> Result<(), String> {
    // Frontend is responsible for checking if it's the active one, or it just deletes it.
    // If it deletes the active one, it should call switch_away_and_delete_empty_session or similar.
    sessions::delete_session(&id)
}

#[tauri::command]
pub async fn rename_session(id: String, title: String) -> Result<sessions::SessionMeta, String> {
    sessions::rename_session(&id, &title, false)
}

#[tauri::command]
pub async fn update_session_profile(id: String, profile_id: String) -> Result<(), String> {
    sessions::update_session_profile(&id, &profile_id)
}

#[tauri::command]
pub async fn get_session_meta(id: String) -> Result<sessions::SessionMeta, String> {
    sessions::get_session_meta(&id)
}

#[tauri::command]
pub async fn get_workspace_dir(
    session_id: String,
    session_manager: tauri::State<'_, SessionManager>,
) -> Result<Option<String>, String> {
    let ctx = session_manager.get_or_create(&session_id).await;
    let ws = ctx.workspace.lock().await.clone();
    if let Some(path) = ws {
        Ok(Some(path.to_string_lossy().to_string()))
    } else {
        // 非沙箱会话返回 None，避免前端或 Agent 误以为存在沙箱限制
        Ok(None)
    }
}

#[tauri::command]
pub async fn save_agent_steps(
    steps: Vec<crate::core::models::AgentStep>,
    session_id: String,
    session_manager: tauri::State<'_, SessionManager>,
) -> Result<(), String> {
    let ctx = session_manager.get_or_create(&session_id).await;
    {
        let mut session = ctx.memory.lock().await;
        session.agent_steps = steps.clone();
    }

    let mut memory = sessions::load_session(&session_id).unwrap_or_default();
    memory.agent_steps = steps;
    sessions::save_session(&session_id, &memory, None);
    Ok(())
}

#[tauri::command]
pub async fn get_agent_steps(
    session_id: String,
    session_manager: tauri::State<'_, SessionManager>,
) -> Result<Vec<crate::core::models::AgentStep>, String> {
    let ctx = session_manager.get_or_create(&session_id).await;
    let steps = ctx.memory.lock().await.agent_steps.clone();
    Ok(steps)
}

#[tauri::command]
pub async fn list_plan_documents(
    session_id: String,
) -> Result<Vec<crate::core::models::PlanDocument>, String> {
    sessions::list_plan_documents(&session_id)
}

#[tauri::command]
pub async fn list_agent_runs(
    session_id: Option<String>,
) -> Result<Vec<crate::core::agent_runs::AgentRun>, String> {
    Ok(crate::core::agent_runs::list_runs(session_id.as_deref()))
}

#[tauri::command]
pub async fn list_agent_run_events(
    session_id: Option<String>,
    run_id: Option<String>,
) -> Result<Vec<crate::core::agent_runs::AgentRunEvent>, String> {
    Ok(crate::core::agent_runs::list_events(session_id.as_deref(), run_id.as_deref()))
}

#[tauri::command]
pub async fn prepare_resume_agent_run(
    run_id: String,
    session_manager: tauri::State<'_, SessionManager>,
) -> Result<crate::core::agent_runs::ResumeAgentRunPlan, String> {
    let (checkpoint, plan) = crate::core::agent_runs::prepare_resume(&run_id)?;
    let ctx = session_manager.get_or_create(&checkpoint.session_id).await;
    {
        let mut memory = ctx.memory.lock().await;
        memory.messages = checkpoint.messages;
    }
    Ok(plan)
}

#[tauri::command]
pub async fn get_background_tasks(
    bg_state: tauri::State<'_, crate::core::background::BackgroundState>,
) -> Result<Vec<crate::core::background::BackgroundTask>, String> {
    let bg = bg_state.0.lock().await;
    Ok(bg.tasks.values().cloned().collect())
}

#[tauri::command]
pub async fn get_subagent_runs(
    session_id: Option<String>,
    monitor_state: tauri::State<'_, crate::core::subagents::SubAgentMonitorState>,
) -> Result<Vec<crate::core::subagents::SubAgentRun>, String> {
    let monitor = monitor_state.0.lock().await;
    Ok(monitor.list(session_id.as_deref()))
}

#[tauri::command]
pub async fn list_subagents(
    session_id: Option<String>,
    monitor_state: tauri::State<'_, crate::core::subagents::SubAgentMonitorState>,
) -> Result<Vec<crate::core::subagents::SubAgentRun>, String> {
    let monitor = monitor_state.0.lock().await;
    Ok(monitor.list(session_id.as_deref()))
}

#[tauri::command]
pub async fn list_subagent_events(
    session_id: Option<String>,
    run_id: Option<String>,
    monitor_state: tauri::State<'_, crate::core::subagents::SubAgentMonitorState>,
) -> Result<Vec<crate::core::subagents::SubAgentEvent>, String> {
    let monitor = monitor_state.0.lock().await;
    Ok(monitor.list_events(session_id.as_deref(), run_id.as_deref()))
}

#[tauri::command]
pub async fn cancel_subagent_run(
    run_id: String,
    app: tauri::AppHandle,
) -> Result<crate::core::subagents::SubAgentRun, String> {
    crate::core::subagents::SubAgentMonitor::cancel_run(&app, &run_id).await
}
