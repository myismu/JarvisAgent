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
//! - `get_session_context_snapshot()`: 查询最近一次上下文 token 快照

use crate::infra::llm::api_client;
use crate::infra::types::models::*;
use crate::core::session;
use crate::infra::state::state::*;
use tauri::{Emitter, Manager};

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
    println!(
        "[DEBUG] create_session called with working_directory: {:?}",
        working_directory
    );

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
    snapshot_registry: tauri::State<'_, SnapshotRegistry>,
) -> Result<session::SessionMeta, String> {
    // 切换会话时释放旧会话的快照管理器缓存
    {
        let registry = snapshot_registry.0.read().await;
        registry.remove(&id).await;
    }

    // 前端通知切换到了该 session，预加载到内存
    let _ = session_manager.get_or_create(&id).await;
    let meta = session::get_session_meta(&id)?;
    println!(
        "[DEBUG] switch_session: id={}, working_directory={:?}",
        id, meta.working_directory
    );
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

    // 清理 SessionManager 内存中的 SessionContext
    if let Some(manager) = app.try_state::<SessionManager>() {
        manager.remove(deleted_session_id).await;
    }

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

    let cfg = crate::infra::config::config::load_config();
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
        let last_user_idx = session
            .messages
            .iter()
            .rposition(|m| matches!(m, Message::User { .. }));
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
            let message_count = session.messages.len();
            session.message_ids.truncate(message_count);
        } else {
            return Err("没有可撤回的用户消息".to_string());
        }
        is_empty = session.messages.is_empty();
    }

    if is_empty {
        switch_away_and_delete_empty_session(&session_id, &app).await?;
    } else {
        {
            let memory = ctx.memory.lock().await.clone();
            session::save_session(&session_id, &memory, None);
        }
        // 保存后从 DB 重新加载，确保内存与持久化数据完全一致
        if let Ok(reloaded) = session::load_session(&session_id) {
            let mut session = ctx.memory.lock().await;
            *session = reloaded;
        }
        let _ = app.emit("session-updated", ());
    }

    Ok(recalled_text)
}

#[tauri::command]
pub async fn recall_message(
    session_id: String,
    message_id: Option<String>,
    user_message_index: Option<usize>,
    session_manager: tauri::State<'_, SessionManager>,
    app: tauri::AppHandle,
) -> Result<String, String> {
    let ctx = session_manager.get_or_create(&session_id).await;
    let stored_target = if let Some(message_id) = message_id.as_ref().filter(|id| !id.trim().is_empty()) {
        session::find_session_message_by_id(&session_id, message_id)?
    } else {
        None
    };
    let recalled_text;
    let is_empty;
    {
        let mut session = ctx.memory.lock().await;
        // 优先通过 stored.seq（session_messages 表行号）定位截断点。
        // 压缩后 message_ids 只含摘要 ID，position() 查不到原始消息，必须从 DB 重建。
        let target_msg: Option<Message> = if let Some(stored) = stored_target.as_ref() {
            let visible = session::list_visible_session_messages(&session_id)?;
            let pos = visible.iter().position(|m| m.seq == stored.seq)
                .ok_or_else(|| "撤回消息不存在".to_string())?;
            // 撤回目标消息及之后的所有消息，只保留之前的
            let target_content = visible.get(pos).map(|m| m.content.clone());
            let keep: Vec<_> = visible.into_iter().take(pos).collect();
            session.messages = keep.iter().map(|m| m.content.clone()).collect();
            session.message_ids = keep.iter().map(|m| m.message_id.clone()).collect();
            target_content
        } else if let Some(idx) = user_message_index {
            if idx >= session.messages.len() {
                return Err("撤回消息不存在".to_string());
            }
            let target = session.messages[idx].clone();
            // 保留 idx 及之前（即丢弃 idx 之后的），因为要撤回的是 idx 这条消息，
            // 所以保留到 idx-1
            session.messages.truncate(idx);
            session.message_ids.truncate(idx);
            Some(target)
        } else {
            return Err("撤回消息不存在".to_string());
        };

        let Some(target_msg) = target_msg else {
            return Err("撤回消息不存在".to_string());
        };
        if let Message::User { content } = &target_msg {
            recalled_text = message_text_content(content);
        } else {
            return Err("撤回目标不是用户消息".to_string());
        }
        is_empty = session.messages.is_empty();
    }

    if let Some(stored) = stored_target {
        // 使用 stored.seq 而非 user_message_index，因为 seq 是 DB 中精确的行号
        session::delete_session_messages_from_seq(&session_id, stored.seq)?;
    } else if let Some(user_message_index) = user_message_index {
        session::delete_session_messages_from_seq(&session_id, user_message_index)?;
    }

    if is_empty {
        switch_away_and_delete_empty_session(&session_id, &app).await?;
    } else {
        {
            let memory = ctx.memory.lock().await.clone();
            session::save_session(&session_id, &memory, None);
        }
        // 保存后从 DB 重新加载，确保内存与持久化数据完全一致
        match session::load_session(&session_id) {
            Ok(reloaded) => {
                let mut session = ctx.memory.lock().await;
                *session = reloaded;
            }
            Err(e) => {
                eprintln!("[Recall] 撤回后重新加载会话内存失败: {}", e);
            }
        }
        let _ = app.emit("session-updated", ());
    }

    Ok(recalled_text)
}

#[tauri::command]
pub async fn recall_message_from_index(
    session_id: String,
    user_message_index: usize,
    session_manager: tauri::State<'_, SessionManager>,
    app: tauri::AppHandle,
) -> Result<String, String> {
    recall_message(
        session_id,
        None,
        Some(user_message_index),
        session_manager,
        app,
    )
    .await
}

fn message_text_content(content: &Content) -> String {
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

#[tauri::command]
pub async fn delete_session(
    id: String,
    session_manager: tauri::State<'_, SessionManager>,
) -> Result<(), String> {
    session::delete_session(&id)?;
    session_manager.remove(&id).await;
    Ok(())
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
pub async fn get_session_context_snapshot(
    session_id: String,
    session_manager: tauri::State<'_, SessionManager>,
) -> Result<Option<crate::infra::types::models::SessionContextSnapshot>, String> {
    let mut snapshot = match session::get_context_snapshot(&session_id)? {
        Some(s) => s,
        None => return Ok(None),
    };
    // 从当前 session memory 重建 "Session Messages" 段，
    // 确保包含完整的最后一轮 assistant 回复
    let ctx = session_manager.get_or_create(&session_id).await;
    let memory = ctx.memory.lock().await;
    if !memory.messages.is_empty() {
        let messages_text = {
            let mut out = String::new();
            for (i, msg) in memory.messages.iter().enumerate() {
                let idx = i + 1;
                let role = match msg {
                    Message::User { .. } => "User",
                    Message::Assistant { .. } => "Assistant",
                };
                out.push_str(&format!("[{}] (msg {})\n", role, idx));
                let content = match msg {
                    Message::User { content } | Message::Assistant { content } => content,
                };
                match content {
                    Content::Single(text) => {
                        if !text.trim().is_empty() {
                            out.push_str(text.trim());
                            out.push('\n');
                        }
                    }
                    Content::Multiple(blocks) => {
                        for block in blocks {
                            match block {
                                ContentBlock::Text { text } => {
                                    if !text.trim().is_empty() {
                                        out.push_str(text.trim());
                                        out.push('\n');
                                    }
                                }
                                ContentBlock::ToolUse { name, input, .. } => {
                                    let s = serde_json::to_string(input).unwrap_or_default();
                                    let t = if s.len() > 120 { &s[..120] } else { &s };
                                    out.push_str(&format!("  → {}({})\n", name, t));
                                }
                                ContentBlock::ToolResult { tool_use_id, content: tc } => {
                                    let sid = &tool_use_id[tool_use_id.len().saturating_sub(12)..];
                                    let lines: Vec<&str> = tc.lines().collect();
                                    let preview = if lines.len() > 2 {
                                        format!("{}\n  …", lines[..2].join("\n"))
                                    } else { tc.clone() };
                                    out.push_str(&format!("  ← {}: {}\n", sid, preview));
                                }
                                ContentBlock::Thinking { thinking, .. } => {
                                    let p = if thinking.len() > 80 { &thinking[..80] } else { thinking };
                                    out.push_str(&format!("  … {}\n", p));
                                }
                                _ => {}
                            }
                        }
                    }
                }
                out.push('\n');
            }
            out
        };
        let new_chars = messages_text.chars().count();
        let token_count = crate::infra::llm::token_count::count_text(
            &snapshot.model, &messages_text,
        );
        // 更新 messages 段
        let old_msg_chars: usize = snapshot.sections.iter()
            .find(|s| s.key == "messages")
            .map(|s| s.chars)
            .unwrap_or(0);
        snapshot.total_chars = snapshot.total_chars.saturating_sub(old_msg_chars).saturating_add(new_chars);
        snapshot.sections.retain(|s| s.key != "messages");
        snapshot.sections.push(crate::infra::types::models::ContextSectionSnapshot {
            key: "messages".to_string(),
            label: "Session Messages".to_string(),
            chars: new_chars,
            estimated_tokens: token_count.tokens,
            token_count_method: token_count.method.as_str().to_string(),
            item_count: memory.messages.len(),
            content: messages_text,
            truncated: false,
        });
        snapshot.estimated_tokens = snapshot.sections.iter().map(|s| s.estimated_tokens).sum();
        snapshot.message_count = memory.messages.len();
    }
    Ok(Some(snapshot))
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
pub async fn list_plan_documents(
    session_id: String,
) -> Result<Vec<crate::infra::types::models::PlanDocument>, String> {
    session::list_plan_documents(&session_id)
}

#[tauri::command]
pub async fn list_agent_runs(
    session_id: Option<String>,
    app: tauri::AppHandle,
    session_manager: tauri::State<'_, SessionManager>,
) -> Result<Vec<crate::core::orchestration::agent_runs::AgentRun>, String> {
    let contexts: Vec<_> = {
        let sessions = session_manager.0.read().await;
        sessions
            .iter()
            .filter(|(sid, _)| {
                session_id
                    .as_deref()
                    .map_or(true, |target| target == sid.as_str())
            })
            .map(|(_, ctx)| ctx.clone())
            .collect()
    };

    let mut active_run_ids = std::collections::HashSet::new();
    let mut active_session_ids = std::collections::HashSet::new();
    for ctx in contexts {
        let is_active = ctx
            .cancel_token
            .lock()
            .await
            .as_ref()
            .map(|token| !token.is_cancelled())
            .unwrap_or(false);
        if is_active {
            active_session_ids.insert(ctx.id.clone());
            if let Some(run_id) = ctx.active_run_id.lock().await.clone() {
                active_run_ids.insert(run_id);
            }
        }
    }

    for run_id in &active_run_ids {
        let _ = crate::core::orchestration::agent_runs::mark_active_run(&app, run_id);
    }
    crate::core::orchestration::agent_runs::mark_stale_runs_interrupted(
        &app,
        session_id.as_deref(),
        &active_run_ids,
        &active_session_ids,
    );

    Ok(crate::core::orchestration::agent_runs::list_runs(
        session_id.as_deref(),
    ))
}

#[tauri::command]
pub async fn list_agent_run_events(
    session_id: Option<String>,
    run_id: Option<String>,
) -> Result<Vec<crate::core::orchestration::agent_runs::AgentRunEvent>, String> {
    Ok(crate::core::orchestration::agent_runs::list_events(
        session_id.as_deref(),
        run_id.as_deref(),
    ))
}

#[tauri::command]
pub async fn prepare_resume_agent_run(
    run_id: String,
    session_manager: tauri::State<'_, SessionManager>,
    app: tauri::AppHandle,
) -> Result<crate::core::orchestration::agent_runs::ResumeAgentRunPlan, String> {
    let (checkpoint, plan) = crate::core::orchestration::agent_runs::prepare_resume(&run_id)?;
    let ctx = session_manager.get_or_create(&checkpoint.session_id).await;
    let should_mark_recovered = {
        let mut memory = ctx.memory.lock().await;
        memory.messages = checkpoint.messages.clone();
        session::reset_message_ids(&mut memory);
        recover_interrupted_into_memory(&checkpoint.session_id, &mut memory)
    };
    if should_mark_recovered {
        let memory = ctx.memory.lock().await.clone();
        session::save_session(&checkpoint.session_id, &memory, None);
        let _ = crate::core::orchestration::agent_runs::mark_run_recovered(&run_id);
        let _ = app.emit("session-updated", ());
    }
    Ok(plan)
}

pub(crate) fn recover_interrupted_into_memory(
    session_id: &str,
    memory: &mut SessionMemory,
) -> bool {
    session::normalize_message_ids(memory);
    let current_messages = memory.messages.clone();
    let Some((extra_messages, live_content, live_thinking)) =
        crate::core::orchestration::agent_runs::recover_interrupted_messages(
            session_id,
            &current_messages,
        )
    else {
        return false;
    };
    for message in extra_messages {
        session::append_message(memory, message);
    }
    if let Some(message) = recovered_assistant_message(&live_content, &live_thinking) {
        if !assistant_message_exists_at_tail(&memory.messages, &message) {
            session::append_message(memory, message);
        }
    }
    true
}

fn recovered_assistant_message(live_content: &str, live_thinking: &str) -> Option<Message> {
    let mut blocks = Vec::new();
    let thinking = live_thinking.trim();
    let content = live_content.trim();
    if !thinking.is_empty() {
        blocks.push(ContentBlock::Thinking {
            thinking: thinking.to_string(),
            signature: String::new(),
        });
    }
    if !content.is_empty() {
        blocks.push(ContentBlock::Text {
            text: content.to_string(),
        });
    }
    if blocks.is_empty() {
        None
    } else {
        Some(Message::Assistant {
            content: Content::Multiple(blocks),
        })
    }
}

fn assistant_message_exists_at_tail(messages: &[Message], target: &Message) -> bool {
    let Some(last) = messages.last() else {
        return false;
    };
    assistant_message_texts(last) == assistant_message_texts(target)
}

fn assistant_message_texts(message: &Message) -> Option<(String, String)> {
    let Message::Assistant { content } = message else {
        return None;
    };
    let mut thinking_parts = Vec::new();
    let mut text_parts = Vec::new();
    match content {
        Content::Single(text) => text_parts.push(text.trim().to_string()),
        Content::Multiple(blocks) => {
            for block in blocks {
                match block {
                    ContentBlock::Thinking { thinking, .. } => {
                        thinking_parts.push(thinking.trim().to_string())
                    }
                    ContentBlock::Text { text } => text_parts.push(text.trim().to_string()),
                    _ => {}
                }
            }
        }
    }
    Some((thinking_parts.join("\n\n"), text_parts.join("\n\n")))
}

#[tauri::command]
pub async fn recover_interrupted_session_messages(
    session_id: String,
    session_manager: tauri::State<'_, SessionManager>,
    app: tauri::AppHandle,
) -> Result<bool, String> {
    let ctx = session_manager.get_or_create(&session_id).await;
    let recovered = {
        let mut memory = ctx.memory.lock().await;
        recover_interrupted_into_memory(&session_id, &mut memory)
    };
    if recovered {
        let memory = ctx.memory.lock().await.clone();
        session::save_session(&session_id, &memory, None);
        if let Some(interrupted_run) =
            crate::core::orchestration::agent_runs::find_interrupted_run(&session_id)
        {
            let _ =
                crate::core::orchestration::agent_runs::mark_run_recovered(&interrupted_run.run_id);
        }
        let _ = app.emit("session-updated", ());
    }
    Ok(recovered)
}

#[tauri::command]
pub async fn get_background_tasks(
    session_id: Option<String>,
    bg_state: tauri::State<'_, crate::infra::background::BackgroundState>,
) -> Result<Vec<crate::infra::background::BackgroundTask>, String> {
    let bg = bg_state.0.lock().await;
    let tasks: Vec<_> = if let Some(sid) = session_id {
        bg.tasks
            .values()
            .filter(|t| t.session_id.as_deref() == Some(&sid))
            .cloned()
            .collect()
    } else {
        bg.tasks.values().cloned().collect()
    };
    Ok(tasks)
}

#[tauri::command]
pub async fn dismiss_background_task(
    task_id: String,
    app: tauri::AppHandle,
) -> Result<bool, String> {
    Ok(crate::infra::background::BackgroundManager::dismiss_task(&app, &task_id).await)
}

#[tauri::command]
pub async fn kill_background_task(
    task_id: String,
    app: tauri::AppHandle,
) -> Result<bool, String> {
    Ok(crate::infra::background::BackgroundManager::kill_task(&app, &task_id).await)
}

#[tauri::command]
pub async fn clear_session_background_tasks(
    session_id: String,
    app: tauri::AppHandle,
) -> Result<usize, String> {
    Ok(crate::infra::background::BackgroundManager::clear_session_tasks(&app, &session_id).await)
}

#[tauri::command]
pub async fn get_subagent_runs(
    session_id: Option<String>,
    monitor_state: tauri::State<'_, crate::core::orchestration::subagents::SubAgentMonitorState>,
) -> Result<Vec<crate::core::orchestration::subagents::SubAgentRun>, String> {
    let mut monitor = monitor_state.0.lock().await;
    Ok(monitor.list(session_id.as_deref()))
}

#[tauri::command]
pub async fn list_subagents(
    session_id: Option<String>,
    monitor_state: tauri::State<'_, crate::core::orchestration::subagents::SubAgentMonitorState>,
) -> Result<Vec<crate::core::orchestration::subagents::SubAgentRun>, String> {
    let mut monitor = monitor_state.0.lock().await;
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

#[tauri::command]
pub async fn get_session_todos(
    session_id: String,
    session_manager: tauri::State<'_, SessionManager>,
) -> Result<Vec<crate::infra::types::models::TodoItem>, String> {
    let ctx = session_manager.get_or_create(&session_id).await;
    let todos = ctx.todos.lock().await;
    Ok(todos.clone())
}

#[tauri::command]
pub async fn compact_conversation(
    session_id: String,
    session_manager: tauri::State<'_, SessionManager>,
    compacting: tauri::State<'_, crate::infra::background::CompactingState>,
) -> Result<String, String> {
    compacting.set_compacting(&session_id, true);
    let result = compact_inner(&session_id, session_manager).await;
    compacting.set_compacting(&session_id, false);
    result
}

async fn compact_inner(
    session_id: &str,
    session_manager: tauri::State<'_, SessionManager>,
) -> Result<String, String> {
    let ctx = session_manager.get_or_create(session_id).await;
    let mut memory = ctx.memory.lock().await;
    let keep = crate::infra::types::constants::COMPACT_KEEP_RECENT_MESSAGES;
    if memory.messages.len() <= keep {
        return Ok(format!("消息不足（仅有 {} 条，保留阈值 {} 条），无需压缩。", memory.messages.len(), keep));
    }
    let client = reqwest::Client::new();
    let cfg = {
        let config = crate::infra::config::config::load_config();
        config.active_config().clone()
    };
    crate::core::session::memory::compact_messages(
        &mut memory.messages,
        &client,
        &cfg.api_key,
        &cfg.base_url,
        &cfg.utility_model,
        crate::infra::llm::api_format::ApiFormat::OpenAI,
    )
    .await
    .map_err(|e| format!("压缩失败: {}", e))?;

    let ids: Vec<String> = (0..memory.messages.len())
        .map(|i| format!("compact:{}:{}", i, uuid::Uuid::new_v4().simple()))
        .collect();
    memory.message_ids = ids;
    let cloned = memory.clone();
    drop(memory);
    drop(ctx);

    let _ = crate::core::session::save_session(session_id, &cloned, None);
    Ok("上下文已压缩。".to_string())
}

#[tauri::command]
pub fn is_session_compacting(
    session_id: String,
    compacting: tauri::State<'_, crate::infra::background::CompactingState>,
) -> bool {
    compacting.is_compacting(&session_id)
}
