//! # session.rs — 会话生命周期管理 Tauri 命令薄封装
//!
//! 仅承担前端 invoke 参数接收与响应转发：
//! - 会话 CRUD、归档/恢复/批量清理、切换与重命名
//! - 工作模式与深度思考档位快照读写
//! - 撤回、上下文压缩、用量查询、Agent Run 状态查询与恢复
//!
//! 核心业务编排（自动命名、中断恢复闸门、退出收尾、持锁撤回、归档切走、摘要压缩）已下沉至
//! `crate::core::session::{lifecycle, naming}`；项目管理与后台/子代理命令已拆出至
//! `command::project` 与 `command::background`。
//!
//! ## Key Exports
//! - `SessionThinkingSnapshot`: 会话深度思考档位权威快照 DTO
//! - 会话管理、撤回、思考档位、用量、Agent Run 与压缩 Tauri 命令
//!
//! ## Dependencies
//! - Internal: `crate::core::session`, `crate::core::session::lifecycle`, `crate::core::runtime`, `crate::core::orchestration::agent_runs`, `crate::infra::config::app_config`
//! - External: `tauri`, `serde`, `ts_rs`

use crate::core::orchestration::agent_runs;
use crate::core::runtime::{SessionManager, SnapshotRegistry};
use crate::core::session;
use crate::core::session::lifecycle::{
    align_active_profile_to_session, compact_session_memory, ensure_session_recovered,
    is_real_user_message, recall_message_locked, switch_away_and_delete_session,
};
use crate::infra::background::CompactingState;
use crate::infra::config::app_config;
use crate::infra::config::config::ConfigState;
use crate::infra::types::error::CommandError;
use crate::infra::types::events::{AgentWorkModeChangedEvent, BgCompactingChangedEvent};
use crate::infra::types::models::{ContextSectionSnapshot, PlanDocument, SessionContextSnapshot};
use tauri::{Emitter, Manager};

#[tauri::command]
pub async fn get_active_session_id() -> Result<Option<String>, CommandError> {
    tokio::task::spawn_blocking(session::get_last_active_session_id)
        .await
        .map_err(|e| CommandError::internal(format!("查询活跃会话 ID 后台任务异常: {}", e)))
}

#[tauri::command]
pub async fn clear_active_session_id() -> Result<(), CommandError> {
    session::repository::clear_last_active_session_id().map_err(CommandError::from)
}

/// 用户手动切换当前会话的工作模式（edit / plan）。
#[tauri::command]
pub async fn set_session_work_mode(
    session_id: String,
    mode: String,
    session_manager: tauri::State<'_, SessionManager>,
    app: tauri::AppHandle,
) -> Result<(), CommandError> {
    if !["edit", "plan"].contains(&mode.as_str()) {
        return Err(CommandError::invalid_input(format!(
            "不支持的工作模式「{}」。支持的模式：edit（编辑）、plan（规划）。",
            mode
        )));
    }

    let ctx = session_manager.get_or_create(&session_id).await;
    let current = ctx.agent_work_mode.lock().await.clone();
    if current == mode {
        return Ok(());
    }
    *ctx.agent_work_mode.lock().await = mode.clone();
    if let Err(e) = session::update_session_work_mode(&session_id, &mode) {
        eprintln!("[JARVIS] 工作模式落库失败（会话 {}）：{}", session_id, e);
    }

    let _ = app.emit(
        "agent-work-mode-changed",
        AgentWorkModeChangedEvent {
            session_id: session_id.clone(),
            from: current,
            to: mode,
            reason: "用户手动切换".to_string(),
        },
    );

    Ok(())
}

/// 读取当前会话的工作模式（edit / plan）。
#[tauri::command]
pub async fn get_session_work_mode(
    session_id: String,
    session_manager: tauri::State<'_, SessionManager>,
) -> Result<String, CommandError> {
    let ctx = session_manager.get_or_create(&session_id).await;
    let mode = ctx.agent_work_mode.lock().await.clone();
    Ok(mode)
}

#[tauri::command]
pub async fn list_sessions() -> Result<Vec<session::SessionMeta>, CommandError> {
    tokio::task::spawn_blocking(session::list_sessions)
        .await
        .map_err(|e| CommandError::internal(format!("查询会话列表后台任务异常: {}", e)))
}

#[tauri::command]
pub async fn list_deleted_sessions() -> Result<Vec<session::SessionMeta>, CommandError> {
    tokio::task::spawn_blocking(session::list_deleted_sessions)
        .await
        .map_err(|e| CommandError::internal(format!("查询归档会话列表后台任务异常: {}", e)))?
        .map_err(CommandError::from)
}

#[tauri::command]
pub async fn restore_session(id: String) -> Result<(), CommandError> {
    session::restore_session(&id).map_err(CommandError::from)
}

#[tauri::command]
pub async fn purge_session(id: String) -> Result<(), CommandError> {
    session::hard_delete_session(&id).map_err(CommandError::from)
}

#[tauri::command]
pub async fn create_session(
    session_manager: tauri::State<'_, SessionManager>,
    project_id: Option<String>,
) -> Result<session::SessionMeta, CommandError> {
    println!(
        "[DEBUG] create_session called with project_id: {:?}",
        project_id
    );

    // 先验后建：项目目录被删/改名时直接拒绝，不创建孤儿空会话
    let working_directory = project_id
        .as_ref()
        .and_then(|pid| session::repository::get_project_path(pid).ok().flatten())
        .map(|ws| session::strip_extended_path_prefix(&ws).to_string());
    if let Some(ws) = &working_directory {
        if !std::path::Path::new(ws).exists() {
            return Err(CommandError::not_found(format!(
                "项目目录已不存在: {}（可能已被删除或移动），无法创建沙箱会话。请重新打开项目后再试。",
                ws
            )));
        }
    }

    let meta = session::create_session(project_id);

    let ctx = session_manager.get_or_create(&meta.id).await;
    *ctx.workspace.lock().await = meta
        .working_directory
        .clone()
        .map(|ws| std::path::PathBuf::from(session::strip_extended_path_prefix(&ws)));

    if let Some(root) = ctx.workspace.lock().await.clone() {
        crate::core::tools::framework::allowance_store::load_into(&ctx, &root).await;
    }

    // 新会话的工作模式与权限档位跟随用户偏好
    let prefs = app_config::get_ui_preferences();
    let initial_mode = if prefs.agent_work_mode == "plan" {
        "plan"
    } else {
        "edit"
    };
    *ctx.agent_work_mode.lock().await = initial_mode.to_string();
    *ctx.approval_mode.lock().await = if prefs.agent_approval_mode == "auto_approve" {
        "auto_approve".to_string()
    } else {
        "request_approval".to_string()
    };

    Ok(meta)
}

#[tauri::command]
pub async fn restore_sessions(
    ids: Vec<String>,
    app: tauri::AppHandle,
) -> Result<session::BatchOpResult, CommandError> {
    let result = session::restore_sessions(&ids)?;
    let _ = app.emit("session-updated", ());
    Ok(result)
}

#[tauri::command]
pub async fn purge_sessions(
    ids: Vec<String>,
    app: tauri::AppHandle,
) -> Result<session::BatchOpResult, CommandError> {
    let result = session::purge_sessions(&ids)?;
    let _ = app.emit("session-updated", ());
    Ok(result)
}

#[tauri::command]
pub async fn count_deleted_sessions() -> Result<usize, CommandError> {
    tokio::task::spawn_blocking(session::count_deleted_sessions)
        .await
        .map_err(|e| CommandError::internal(format!("统计归档会话数量后台任务异常: {}", e)))?
        .map_err(CommandError::from)
}

/// 切换活跃会话，并同步对齐激活模型预设。
#[tauri::command]
pub async fn switch_session(
    id: String,
    session_manager: tauri::State<'_, SessionManager>,
    snapshot_registry: tauri::State<'_, SnapshotRegistry>,
    app: tauri::AppHandle,
) -> Result<session::SessionMeta, CommandError> {
    {
        let registry = snapshot_registry.0.read().await;
        registry.remove(&id).await;
    }

    let _ = session_manager.get_or_create(&id).await;
    let meta = session::get_session_meta(&id)?;
    println!(
        "[DEBUG] switch_session: id={}, working_directory={:?}",
        id, meta.working_directory
    );

    if let Some(state) = app.try_state::<ConfigState>() {
        align_active_profile_to_session(&app, &state, meta.profile_id.as_deref());
    }

    Ok(meta)
}

/// 撤回最后一条用户消息，委托给 `recall_message` 统一处理。
#[tauri::command]
pub async fn recall_last_message(
    session_id: String,
    session_manager: tauri::State<'_, SessionManager>,
    app: tauri::AppHandle,
) -> Result<String, CommandError> {
    let visible = session::list_visible_session_messages(&session_id)?;
    let target = visible
        .iter()
        .rev()
        .find(|message| is_real_user_message(message))
        .ok_or_else(|| CommandError::not_found("没有可撤回的用户消息"))?;
    let message_id = target.message_id.clone();
    recall_message(
        session_id,
        Some(message_id),
        None,
        None,
        session_manager,
        app,
    )
    .await
}

/// 统一撤回入口。
#[tauri::command]
pub async fn recall_message(
    session_id: String,
    message_id: Option<String>,
    user_message_index: Option<usize>,
    prune_metadata_cutoff: Option<u64>,
    session_manager: tauri::State<'_, SessionManager>,
    app: tauri::AppHandle,
) -> Result<String, CommandError> {
    let ctx = session_manager.get_or_create(&session_id).await;
    let _execution_guard = ctx.execution_gate.lock().await;
    if ctx.has_active_execution() {
        return Err(CommandError::conflict(
            "当前任务尚未结束，请等待任务停止后再撤回。",
        ));
    }
    recall_message_locked(
        &session_id,
        message_id,
        user_message_index,
        prune_metadata_cutoff,
        &ctx,
        &app,
    )
    .await
    .map_err(CommandError::from)
}

#[tauri::command]
pub async fn delete_session(
    id: String,
    session_manager: tauri::State<'_, SessionManager>,
    app: tauri::AppHandle,
) -> Result<(), CommandError> {
    session_manager.remove(&id).await;
    switch_away_and_delete_session(&id, &app)
        .await
        .map_err(CommandError::from)
}

#[tauri::command]
pub async fn rename_session(
    id: String,
    title: String,
) -> Result<session::SessionMeta, CommandError> {
    session::rename_session(&id, &title, false).map_err(CommandError::from)
}

#[tauri::command]
pub async fn update_session_profile(id: String, profile_id: String) -> Result<(), CommandError> {
    session::update_session_profile(&id, &profile_id).map_err(CommandError::from)
}

#[tauri::command]
pub async fn get_session_meta(id: String) -> Result<session::SessionMeta, CommandError> {
    tokio::task::spawn_blocking(move || session::get_session_meta(&id))
        .await
        .map_err(|e| CommandError::internal(format!("查询会话元数据后台任务异常: {}", e)))?
        .map_err(CommandError::from)
}

// ── 深度思考档位（会话级，布尔） ──

async fn current_main_model(config_state: &tauri::State<'_, ConfigState>) -> String {
    let cfg = config_state.0.lock().await.clone();
    cfg.active_config().main_model
}

/// 组装前端的思考档位快照（`resolvedEnabled` 由后端裁决层算出，前端不做二次判断）。
/// 会话深度思考档位的权威快照（用于查询、修改回包与 `session-thinking-mode-changed` 事件）
#[derive(Debug, Clone, serde::Serialize, ts_rs::TS)]
#[serde(rename_all = "camelCase")]
#[ts(rename_all = "camelCase")]
pub struct SessionThinkingSnapshot {
    pub session_id: String,
    pub thinking_enabled: bool,
    pub resolved_enabled: bool,
    pub reason: String,
    #[ts(optional = nullable)]
    pub notice_i18n_key: Option<String>,
}

fn build_thinking_snapshot(
    session_id: &str,
    session_mode: session::thinking::ThinkingMode,
    caps: Option<&crate::infra::llm::registry::ModelCapabilities>,
) -> SessionThinkingSnapshot {
    let decision = session::thinking::decide(None, session_mode, caps);
    SessionThinkingSnapshot {
        session_id: session_id.to_string(),
        thinking_enabled: session_mode.0,
        resolved_enabled: decision.enabled,
        reason: format!("{:?}", decision.reason),
        notice_i18n_key: decision.notice_i18n_key.map(str::to_string),
    }
}

#[tauri::command]
pub async fn get_pending_thinking_enabled(
    config_state: tauri::State<'_, ConfigState>,
) -> Result<SessionThinkingSnapshot, CommandError> {
    let model_id = current_main_model(&config_state).await;
    let caps = crate::infra::llm::registry::query_capabilities(&model_id);
    let thinking_forced = caps.as_ref().map(|c| c.thinking_forced).unwrap_or(false);

    let default_raw = app_config::read_file().ui_preferences.thinking_default;
    let resolved = session::thinking::ThinkingDefault::parse(&default_raw).resolve(thinking_forced);

    Ok(build_thinking_snapshot(
        "",
        session::thinking::ThinkingMode(resolved),
        caps.as_ref(),
    ))
}

#[tauri::command]
pub async fn get_session_thinking(
    id: String,
    session_manager: tauri::State<'_, SessionManager>,
    config_state: tauri::State<'_, ConfigState>,
) -> Result<SessionThinkingSnapshot, CommandError> {
    let ctx = session_manager.get_or_create(&id).await;
    let session_mode = session::thinking::ThinkingMode(*ctx.thinking_mode.lock().await);

    let model_id = current_main_model(&config_state).await;
    let caps = crate::infra::llm::registry::query_capabilities(&model_id);

    Ok(build_thinking_snapshot(&id, session_mode, caps.as_ref()))
}

#[tauri::command]
pub async fn set_session_thinking_enabled(
    id: String,
    enabled: bool,
    session_manager: tauri::State<'_, SessionManager>,
    config_state: tauri::State<'_, ConfigState>,
    app: tauri::AppHandle,
) -> Result<SessionThinkingSnapshot, CommandError> {
    session::update_session_thinking_mode(&id, enabled)?;

    let ctx = session_manager.get_or_create(&id).await;
    *ctx.thinking_mode.lock().await = enabled;

    let model_id = current_main_model(&config_state).await;
    let caps = crate::infra::llm::registry::query_capabilities(&model_id);
    let snapshot =
        build_thinking_snapshot(&id, session::thinking::ThinkingMode(enabled), caps.as_ref());

    let _ = app.emit("session-thinking-mode-changed", snapshot.clone());

    Ok(snapshot)
}

#[tauri::command]
pub async fn get_session_context_snapshot(
    session_id: String,
    session_manager: tauri::State<'_, SessionManager>,
) -> Result<Option<SessionContextSnapshot>, CommandError> {
    let sid_for_read = session_id.clone();
    let mut snapshot =
        match tokio::task::spawn_blocking(move || session::get_context_snapshot(&sid_for_read))
            .await
            .map_err(|e| CommandError::internal(format!("查询上下文快照后台任务异常: {}", e)))??
        {
            Some(s) => s,
            None => return Ok(None),
        };
    let ctx = session_manager.get_or_create(&session_id).await;
    // 短锁拷贝消息快照后立即释放，避免持锁执行长文本格式化与 token 估算
    let messages = ctx.memory.lock().await.messages.clone();
    if messages.is_empty() {
        return Ok(Some(snapshot));
    }

    tokio::task::spawn_blocking(move || {
        let messages_text = crate::core::agent::context_format::format_messages_readable(&messages);
        let new_chars = messages_text.chars().count();
        let token_count =
            crate::infra::llm::token_count::count_text(&snapshot.model, &messages_text);
        let messages_json = serde_json::to_string_pretty(&messages).unwrap_or_default();
        let old_msg_chars: usize = snapshot
            .sections
            .iter()
            .find(|s| s.key == "messages")
            .map(|s| s.chars)
            .unwrap_or(0);
        snapshot.total_chars = snapshot
            .total_chars
            .saturating_sub(old_msg_chars)
            .saturating_add(new_chars);
        snapshot.sections.retain(|s| s.key != "messages");
        snapshot.sections.push(ContextSectionSnapshot {
            key: "messages".to_string(),
            label: "Session Messages".to_string(),
            chars: new_chars,
            estimated_tokens: token_count.tokens,
            token_count_method: token_count.method.as_str().to_string(),
            item_count: messages.len(),
            content: messages_text,
            truncated: false,
            raw_content: Some(messages_json),
        });
        snapshot.estimated_tokens = snapshot.sections.iter().map(|s| s.estimated_tokens).sum();
        snapshot.message_count = messages.len();
        Some(snapshot)
    })
    .await
    .map_err(|e| CommandError::internal(format!("组装上下文快照后台任务异常: {}", e)))
}

#[tauri::command]
pub async fn get_session_usage_breakdown(
    session_id: String,
) -> Result<session::UsageBreakdown, CommandError> {
    tokio::task::spawn_blocking(move || session::get_session_usage_breakdown(&session_id))
        .await
        .map_err(|e| CommandError::internal(format!("查询会话用量明细后台任务异常: {}", e)))?
        .map_err(CommandError::from)
}

#[tauri::command]
pub async fn get_session_usage(
    session_id: String,
) -> Result<session::SessionUsageView, CommandError> {
    tokio::task::spawn_blocking(move || {
        let breakdown = session::get_session_usage_breakdown(&session_id)?;
        Ok::<_, String>(session::SessionUsageView::from_breakdown(&breakdown))
    })
    .await
    .map_err(|e| CommandError::internal(format!("查询会话用量视图后台任务异常: {}", e)))?
    .map_err(CommandError::from)
}

#[tauri::command]
pub async fn list_plan_documents(session_id: String) -> Result<Vec<PlanDocument>, CommandError> {
    tokio::task::spawn_blocking(move || session::list_plan_documents(&session_id))
        .await
        .map_err(|e| CommandError::internal(format!("查询计划文档后台任务异常: {}", e)))?
        .map_err(CommandError::from)
}

#[tauri::command]
pub async fn list_agent_runs(
    session_id: Option<String>,
    app: tauri::AppHandle,
    session_manager: tauri::State<'_, SessionManager>,
) -> Result<Vec<agent_runs::AgentRun>, CommandError> {
    let contexts = session_manager.list_contexts(session_id.as_deref()).await;

    let mut active_run_ids = std::collections::HashSet::new();
    let mut active_session_ids = std::collections::HashSet::new();
    for ctx in contexts {
        let is_active = ctx
            .current_cancel_token()
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
        let _ = agent_runs::mark_active_run(&app, run_id);
    }
    agent_runs::mark_stale_runs_interrupted(
        &app,
        session_id.as_deref(),
        &active_run_ids,
        &active_session_ids,
    );

    Ok(agent_runs::list_runs(session_id.as_deref()))
}

#[tauri::command]
pub async fn list_agent_run_events(
    session_id: Option<String>,
    run_id: Option<String>,
) -> Result<Vec<agent_runs::AgentRunLoopEvent>, CommandError> {
    tokio::task::spawn_blocking(move || {
        agent_runs::list_loop_events(session_id.as_deref(), run_id.as_deref())
    })
    .await
    .map_err(|e| CommandError::internal(format!("查询 Agent 运行事件后台任务异常: {}", e)))
}

#[tauri::command]
pub async fn prepare_resume_agent_run(
    run_id: String,
    session_manager: tauri::State<'_, SessionManager>,
    app: tauri::AppHandle,
) -> Result<agent_runs::ResumeAgentRunPlan, CommandError> {
    let (_run, plan) = agent_runs::prepare_resume(&run_id)?;
    let ctx = session_manager.get_or_create(&plan.session_id).await;
    let active_run_id = ctx.active_run_id.lock().await.clone();
    let recovered = {
        let mut memory = ctx.memory.lock().await;
        session::reset_message_ids(&mut memory);
        ensure_session_recovered(&plan.session_id, &mut memory, active_run_id.as_deref())
    };
    if recovered {
        let _ = app.emit("session-updated", ());
    }
    Ok(plan)
}

#[tauri::command]
pub async fn recover_interrupted_session_messages(
    session_id: String,
    session_manager: tauri::State<'_, SessionManager>,
    app: tauri::AppHandle,
) -> Result<bool, CommandError> {
    let ctx = session_manager.get_or_create(&session_id).await;
    let active_run_id = ctx.active_run_id.lock().await.clone();
    let recovered = {
        let mut memory = ctx.memory.lock().await;
        ensure_session_recovered(&session_id, &mut memory, active_run_id.as_deref())
    };
    if recovered {
        let _ = app.emit("session-updated", ());
        session::notify_messages_changed(&app, &session_id, "recovery");
    }
    Ok(recovered)
}

/// 压缩命令的生命周期守卫：无论正常返回、报错还是异步任务被丢弃，都清除会话状态并广播结束。
struct CompactingSessionGuard<'a> {
    state: &'a CompactingState,
    session_id: String,
    app: tauri::AppHandle,
}

impl Drop for CompactingSessionGuard<'_> {
    fn drop(&mut self) {
        let _ = self.state.set_compacting(&self.session_id, false);
        let _ = self.app.emit(
            "bg-compacting-changed",
            BgCompactingChangedEvent {
                session_id: self.session_id.clone(),
                compacting: false,
            },
        );
    }
}

#[tauri::command]
pub async fn compact_conversation(
    session_id: String,
    session_manager: tauri::State<'_, SessionManager>,
    compacting: tauri::State<'_, CompactingState>,
    app: tauri::AppHandle,
) -> Result<String, CommandError> {
    if !compacting.set_compacting(&session_id, true) {
        return Err(CommandError::conflict(
            "当前会话已有上下文压缩正在执行，请等待完成。",
        ));
    }
    let _ = app.emit(
        "bg-compacting-changed",
        BgCompactingChangedEvent {
            session_id: session_id.clone(),
            compacting: true,
        },
    );
    let guard = CompactingSessionGuard {
        state: compacting.inner(),
        session_id: session_id.clone(),
        app: app.clone(),
    };
    let result = compact_session_memory(&session_id, session_manager.inner(), &app).await;
    drop(guard);
    result.map_err(CommandError::from)
}

#[tauri::command]
pub fn is_session_compacting(
    session_id: String,
    compacting: tauri::State<'_, CompactingState>,
) -> bool {
    compacting.is_compacting(&session_id)
}
