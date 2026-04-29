//! # session.rs — 会话生命周期管理 Tauri 命令
//!
//! 提供会话的创建、切换、删除、重命名、元数据查询等核心命令，
//! 以及 Agent 步骤、方案文档、Agent Run、子 Agent 等扩展查询命令。
//! 还包含自动命名、撤回最后消息等内部辅助函数。
//!
//! ## 关键导出
//! - `create_session()`: 创建新会话，可指定工作目录沙箱
//! - `switch_session()`: 切换活跃会话
//! - `delete_session()` / `rename_session()`: 会话管理
//! - `recall_last_message()`: 撤回最后一条用户消息
//! - `auto_name_session()`: 使用 LLM 自动生成会话名称（内部函数）
//! - `list_agent_runs()` / `get_subagent_runs()`: Agent 运行记录查询

use crate::core::models::*;
use crate::core::session;
use crate::core::state::*;
use crate::core::llm::api_client;
use tauri::Emitter;

#[tauri::command]
pub async fn get_active_session_id() -> Result<Option<String>, String> {
    // 前端自行维护 active_session_id。
    // 如果前端需要读取最后一次的 session：
    Ok(session::get_last_active_session_id())
}

#[tauri::command]
pub async fn list_sessions() -> Result<Vec<session::SessionMeta>, String> {
    Ok(session::list_sessions())
}

#[tauri::command]
pub async fn create_session(
    session_manager: tauri::State<'_, SessionManager>,
    working_directory: Option<String>,
) -> Result<session::SessionMeta, String> {
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

    let meta = session::create_session(validated_dir.clone());
    
    // 初始化上下文
    let ctx = session_manager.get_or_create(&meta.id).await;
    *ctx.workspace.lock().await = validated_dir.map(std::path::PathBuf::from);

    Ok(meta)
}

#[tauri::command]
pub async fn switch_session(
    id: String,
    session_manager: tauri::State<'_, SessionManager>,
) -> Result<session::SessionMeta, String> {
    // 前端通知切换到了该 session，预加载到内存
    let _ = session_manager.get_or_create(&id).await;
    let meta = session::get_session_meta(&id)?;
    println!("[DEBUG] switch_session: id={}, working_directory={:?}", id, meta.working_directory);
    Ok(meta)
}

/// 删除会话后自动切换到下一个可用会话（若无则创建新会话）
pub async fn switch_away_and_delete_empty_session(
    deleted_session_id: &str,
    app: &tauri::AppHandle,
) -> Result<(), String> {
    // 找到第一个非当前的会话作为 fallback
    let fallback = session::list_sessions()
        .into_iter()
        .find(|session| session.id != deleted_session_id);

    let next_active_session_id = if let Some(meta) = fallback {
        meta.id
    } else {
        let meta = session::create_session(None);
        meta.id
    };

    session::delete_session(deleted_session_id)?;

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

/// 使用 utility 模型自动生成会话名称（取前 4 条消息摘要）
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

    // 取前 4 条消息用于摘要
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

    // 去除 LLM 可能添加的引号
    let title = title
        .trim()
        .trim_matches('"')
        .trim_matches('\'')
        .to_string();
    if !title.is_empty() {
        let _ = session::rename_session(&session_id, &title, true);
        let _ = app.emit("session-renamed", ());
    }

    Ok(())
}

/// 撤回最后一条用户消息，返回撤回的文本内容
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
        // 从后往前找最后一条用户消息
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
        session::save_session(&session_id, &memory, None);
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
    session::delete_session(&id)
}

#[tauri::command]
pub async fn rename_session(id: String, title: String) -> Result<session::SessionMeta, String> {
    session::rename_session(&id, &title, false)
}

#[tauri::command]
pub async fn update_session_profile(id: String, profile_id: String) -> Result<(), String> {
    session::update_session_profile(&id, &profile_id)
}

#[tauri::command]
pub async fn get_session_meta(id: String) -> Result<session::SessionMeta, String> {
    session::get_session_meta(&id)
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

    let mut memory = session::load_session(&session_id).unwrap_or_default();
    memory.agent_steps = steps;
    session::save_session(&session_id, &memory, None);
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
    session::list_plan_documents(&session_id)
}

#[tauri::command]
pub async fn list_agent_runs(
    session_id: Option<String>,
) -> Result<Vec<crate::core::orchestration::agent_runs::AgentRun>, String> {
    Ok(crate::core::orchestration::agent_runs::list_runs(session_id.as_deref()))
}

#[tauri::command]
pub async fn list_agent_run_events(
    session_id: Option<String>,
    run_id: Option<String>,
) -> Result<Vec<crate::core::orchestration::agent_runs::AgentRunEvent>, String> {
    Ok(crate::core::orchestration::agent_runs::list_events(session_id.as_deref(), run_id.as_deref()))
}

#[tauri::command]
pub async fn prepare_resume_agent_run(
    run_id: String,
    session_manager: tauri::State<'_, SessionManager>,
) -> Result<crate::core::orchestration::agent_runs::ResumeAgentRunPlan, String> {
    let (checkpoint, plan) = crate::core::orchestration::agent_runs::prepare_resume(&run_id)?;
    let ctx = session_manager.get_or_create(&checkpoint.session_id).await;
    {
        let mut memory = ctx.memory.lock().await;
        memory.messages = checkpoint.messages;
    }
    Ok(plan)
}

#[tauri::command]
pub async fn get_background_tasks(
    bg_state: tauri::State<'_, crate::core::infra::background::BackgroundState>,
) -> Result<Vec<crate::core::infra::background::BackgroundTask>, String> {
    let bg = bg_state.0.lock().await;
    Ok(bg.tasks.values().cloned().collect())
}

#[tauri::command]
pub async fn get_subagent_runs(
    session_id: Option<String>,
    monitor_state: tauri::State<'_, crate::core::orchestration::subagents::SubAgentMonitorState>,
) -> Result<Vec<crate::core::orchestration::subagents::SubAgentRun>, String> {
    let monitor = monitor_state.0.lock().await;
    Ok(monitor.list(session_id.as_deref()))
}

#[tauri::command]
pub async fn list_subagents(
    session_id: Option<String>,
    monitor_state: tauri::State<'_, crate::core::orchestration::subagents::SubAgentMonitorState>,
) -> Result<Vec<crate::core::orchestration::subagents::SubAgentRun>, String> {
    let monitor = monitor_state.0.lock().await;
    Ok(monitor.list(session_id.as_deref()))
}

#[tauri::command]
pub async fn list_subagent_events(
    session_id: Option<String>,
    run_id: Option<String>,
    monitor_state: tauri::State<'_, crate::core::orchestration::subagents::SubAgentMonitorState>,
) -> Result<Vec<crate::core::orchestration::subagents::SubAgentEvent>, String> {
    let monitor = monitor_state.0.lock().await;
    Ok(monitor.list_events(session_id.as_deref(), run_id.as_deref()))
}

#[tauri::command]
pub async fn cancel_subagent_run(
    run_id: String,
    app: tauri::AppHandle,
) -> Result<crate::core::orchestration::subagents::SubAgentRun, String> {
    crate::core::orchestration::subagents::SubAgentMonitor::cancel_run(&app, &run_id).await
}
