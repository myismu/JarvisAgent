//! # session.rs — 会话生命周期管理 Tauri 命令
//!
//! 提供会话的创建、切换、删除、重命名、元数据查询等核心命令，
//! 以及 Agent 步骤、方案文档、Agent Run、子 Agent 等扩展查询命令。
//! 还包含自动命名、撤回最后消息等内部辅助函数。
//!
//! ## 关键导出
//! - `create_session()`: 创建新会话，可指定工作目录沙箱
//! - `set_session_work_mode()`: 用户手动切换会话工作模式（chat/edit/plan）
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
    Ok(session::get_last_active_session_id())
}

#[tauri::command]
pub async fn clear_active_session_id() -> Result<(), String> {
    crate::core::session::repository::clear_last_active_session_id()
}

/// 用户手动切换当前会话的工作模式（edit / plan）。
///
/// 以前模式只写进 UI 偏好（app-config.json），已有会话的 `ctx.agent_work_mode`
/// 永远不会被更新，导致界面显示"规划"而后端仍在"编辑"。这个命令补上那条链路：
/// 写会话状态 → 广播 `agent-work-mode-changed`（前端已在监听该事件）→ 偏好由前端落盘，
/// 供新会话继承。
///
/// 第二步起"只读保护（chat）"已取消：安全由权限档位承担（见 set_session_approval_mode）。
/// 这里只接受 edit / plan。
#[tauri::command]
pub async fn set_session_work_mode(
    session_id: String,
    mode: String,
    session_manager: tauri::State<'_, SessionManager>,
    app: tauri::AppHandle,
) -> Result<(), String> {
    if !["edit", "plan"].contains(&mode.as_str()) {
        return Err(format!(
            "不支持的工作模式「{}」。支持的模式：edit（编辑）、plan（规划）。",
            mode
        ));
    }

    let ctx = session_manager.get_or_create(&session_id).await;
    let current = ctx.agent_work_mode.lock().await.clone();
    if current == mode {
        return Ok(());
    }
    *ctx.agent_work_mode.lock().await = mode.clone();

    let _ = app.emit(
        "agent-work-mode-changed",
        serde_json::json!({
            "sessionId": session_id,
            "from": current,
            "to": mode,
            "reason": "用户手动切换",
        }),
    );

    Ok(())
}

/// 读取当前会话的工作模式（chat = 只读保护 / edit / plan）。
///
/// 前端用它校准模式控件：会话状态才是唯一事实来源，UI 偏好只决定新会话的初始模式。
#[tauri::command]
pub async fn get_session_work_mode(
    session_id: String,
    session_manager: tauri::State<'_, SessionManager>,
) -> Result<String, String> {
    let ctx = session_manager.get_or_create(&session_id).await;
    let mode = ctx.agent_work_mode.lock().await.clone();
    Ok(mode)
}

#[tauri::command]
pub async fn list_sessions() -> Result<Vec<session::SessionMeta>, String> {
    Ok(session::list_sessions())
}

#[tauri::command]
pub async fn create_session(
    session_manager: tauri::State<'_, SessionManager>,
    project_id: Option<String>,
) -> Result<session::SessionMeta, String> {
    println!(
        "[DEBUG] create_session called with project_id: {:?}",
        project_id
    );

    let meta = session::create_session(project_id);

    // 初始化上下文
    let ctx = session_manager.get_or_create(&meta.id).await;
    *ctx.workspace.lock().await = meta.working_directory.clone().map(std::path::PathBuf::from);

    // 新会话的工作模式与权限档位跟随用户偏好
    let prefs = crate::command::app_config::get_ui_preferences()
        .await
        .unwrap_or_default();
    let initial_mode = if prefs.agent_work_mode == "plan" {
        "plan"
    } else {
        "edit"
    };
    *ctx.agent_work_mode.lock().await = initial_mode.to_string();
    // 新会话的权限档位跟随偏好（请求审批 / 帮我批准）
    *ctx.approval_mode.lock().await = if prefs.agent_approval_mode == "auto_approve" {
        "auto_approve".to_string()
    } else {
        "request_approval".to_string()
    };

    Ok(meta)
}

#[tauri::command]
pub async fn open_project(
    path: String,
) -> Result<session::ProjectMeta, String> {
    let normalized = std::path::Path::new(&path);
    if !normalized.exists() || !normalized.is_dir() {
        return Err(format!("目录不存在或不是文件夹: {}", path));
    }
    let abs = normalized.canonicalize()
        .map_err(|e| format!("解析路径失败: {}", e))?
        .to_string_lossy()
        .to_string();

    // 已存在则直接返回
    if let Ok(Some(project)) = crate::core::session::repository::get_project_by_path(&abs) {
        return Ok(project);
    }

    let name = std::path::Path::new(&abs)
        .file_name()
        .map(|n| n.to_string_lossy().to_string())
        .unwrap_or_else(|| abs.clone());

    crate::core::session::repository::create_project(&name, &abs)
}

#[tauri::command]
pub async fn list_projects() -> Result<Vec<session::ProjectMeta>, String> {
    crate::core::session::repository::list_projects()
}

#[tauri::command]
pub async fn delete_project(id: String) -> Result<(), String> {
    crate::core::session::repository::delete_project(&id)
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

/// 把"当前会话"的模型预设同步为 `AppConfig.active_profile_id` 并落库。
///
/// **为什么必须有这个函数**：模型预设是会话级状态（`sessions.profile_id`），
/// 但 pipeline 每次读的是 `AppConfig.active_profile_id`。因此**任何"让某会话成为
/// 当前会话"的后端路径都必须对齐一次**，否则后续请求会继续用上一个会话/全局默认的预设。
///
/// 已覆盖的路径：启动恢复会话、点击切会话（前端 `syncProfileFromSession`）、
/// 删除会话后自动回落（本文件 `switch_away_and_delete_empty_session`）。
fn align_active_profile_to_session(
    app: &tauri::AppHandle,
    config_state: &tauri::State<'_, crate::infra::config::config::ConfigState>,
    profile_id: Option<&str>,
) {
    let Ok(mut current) = config_state.0.try_lock() else {
        // 拿不到配置锁就跳过：宁可这次不对齐，也不能阻塞删除流程
        return;
    };
    let target = profile_id
        .map(|s| s.to_string())
        .unwrap_or_else(|| current.global_profile_id.clone());
    if current.active_profile_id == target {
        return;
    }
    current.active_profile_id = target;
    let snapshot = current.clone();
    drop(current);

    if let Err(e) = crate::infra::config::config::save_config(&snapshot) {
        println!("[配置] 对齐会话预设落库失败: {}", e);
        return;
    }
    println!(
        "[配置] 已对齐激活预设: {} (main_model={})",
        snapshot.active_profile_id,
        snapshot.active_config().main_model
    );
    // 通知前端刷新（输入栏据此重读模型名与能力）
    let _ = app.emit("config-updated", ());
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
    // 删除前先记下 fallback 的预设：删除后这个 meta 就取不到了
    let fallback_profile_id = fallback.as_ref().and_then(|m| m.profile_id.clone());
    let fallback_id = fallback.as_ref().map(|m| m.id.clone());

    // 删空会话
    session::delete_session(deleted_session_id)?;
    if let Some(manager) = app.try_state::<SessionManager>() {
        manager.remove(deleted_session_id).await;
    }

    // 对齐激活预设，否则后端会继续用**刚被删掉的会话**的预设
    if let Some(state) = app.try_state::<crate::infra::config::config::ConfigState>() {
        align_active_profile_to_session(app, &state, fallback_profile_id.as_deref());
    }

    let _ = app.emit(
        "active-session-changed",
        SessionCleanupResult {
            deleted_session_id: Some(deleted_session_id.to_string()),
            active_session_id: fallback_id,
        },
    );
    let _ = app.emit("session-updated", ());

    Ok(())
}

/// 使用 utility 模型自动生成会话名称。
///
/// 提取前几条用户消息 + 助手首条文本，过滤掉系统注入的上下文消息。
/// 对 LLM 返回结果做长度校验，防止 LLM 复述原文。
pub async fn auto_name_session(
    app: tauri::AppHandle,
    session_id: String,
    memory: SessionMemory,
) -> Result<(), String> {
    if memory.messages.is_empty() {
        return Ok(());
    }

    // 提取用于命名的摘要文本：取用户消息 + 最后一条助手消息（反映实际产出），跳过系统注入
    // 不再提前 break —— 遍历全部消息以获取最后一条助手回复，首条助手回复往往是「我来做X」
    // 这种流程性表述，而最后一条更可能包含实际成果。
    let mut user_texts: Vec<String> = Vec::new();
    let mut assistant_text: Option<String> = None;
    for msg in &memory.messages {
        match msg {
            Message::User { content } => {
                // 动态上下文是独立的 Context 块，extract_plain_text 只取 Text 块，
                // 所以这里拿到的就是用户原文，无需再做字符串手术
                let text = extract_plain_text(content);
                if !text.trim().is_empty() {
                    user_texts.push(text);
                }
            }
            Message::Assistant { content } => {
                // 始终覆盖，取最后一条助手消息（更可能描述实际产出而非流程承诺）
                if let Content::Multiple(blocks) = content {
                    for b in blocks {
                        if let crate::infra::types::models::ContentBlock::Text { text } = b {
                            let t = text.trim().to_string();
                            if !t.is_empty() { assistant_text = Some(t); break; }
                        }
                    }
                }
            }
        }
    }

    if user_texts.is_empty() {
        return Ok(());
    }

    // 拼成简洁摘要：用户说的话 + 助手简要
    let mut context = String::new();
    for (i, t) in user_texts.iter().enumerate() {
        let short: String = t.chars().take(300).collect();
        context.push_str(&format!("用户{}: {}\n", i + 1, short));
    }
    if let Some(ref at) = assistant_text {
        let short: String = at.chars().take(120).collect();
        context.push_str(&format!("助手: {}\n", short));
    }

    let summary_prompt = format!(
        "为以上对话生成一个描述性标题，让用户以后能一眼回忆起对话的核心主题和关键成果。\n\
         \n\
         规则：\n\
         - 标题应概括「核心主题 + 关键产出」，而非描述「当前处于哪个流程阶段」\n\
         - 避免使用「审批」「讨论中」「进行中」「审查」「确认」「评估」等流程节点词\n\
         - 长度 3~20 字，不含引号、标点和解释\n\
         - 只输出标题本身，不要输出任何其他内容\n\
         \n\
         正确示例（描述内容与成果）：\n\
         - 任务与知识库管理系统架构设计\n\
         - 支付模块空指针异常修复\n\
         - 用户认证 JWT 方案设计\n\
         - 搜索结果分页改用游标方案\n\
         \n\
         错误示例（描述流程阶段）：\n\
         - 实施方案审批 → 看不出具体项目\n\
         - 代码审查 → 不知道审查什么\n\
         - 需求讨论 → 不知道讨论什么\n\
         \n\
         对话内容：\n{}",
        context
    );

    let cfg = crate::infra::config::config::load_config();
    let agent_cfg = cfg.active_config();

    // 辅助调用统一走带超时的客户端
    let client = api_client::build_utility_client();
    let title = api_client::call_llm_simple(
        &client,
        &agent_cfg.api_key,
        &agent_cfg.base_url,
        &agent_cfg.utility_model,
        agent_cfg.api_format_enum(),
        "你是一个会话标题生成器。为对话生成描述性标题，概括核心主题与关键成果，避免流程节点词。只输出标题文本。",
        &summary_prompt,
        30,
    )
    .await
    .unwrap_or_default();

    let title = title
        .trim()
        .trim_matches('"')
        .trim_matches('\'')
        .trim_matches('「')
        .trim_matches('」')
        .to_string();

    // 校验：标题必须在 2~30 字之间，不是原文复述
    let char_count = title.chars().count();
    if char_count >= 2 && char_count <= 30 {
        // 额外检查：标题不能是某条原始消息的原文复述
        let is_verbatim = user_texts.iter().any(|t| t.contains(&title) && t.len() < title.len() * 3)
            || assistant_text.as_ref().map(|t| t.contains(&title) && t.len() < title.len() * 3).unwrap_or(false);
        if !is_verbatim {
            let _ = session::rename_session(&session_id, &title, true);
            let _ = app.emit("session-renamed", ());
            return Ok(());
        }
    }

    Ok(())
}

/// 从 Content 中提取纯文本
fn extract_plain_text(content: &Content) -> String {
    match content {
        Content::Single(s) => s.clone(),
        Content::Multiple(blocks) => blocks
            .iter()
            .filter_map(|b| {
                if let crate::infra::types::models::ContentBlock::Text { text } = b {
                    Some(text.clone())
                } else {
                    None
                }
            })
            .collect::<Vec<_>>()
            .join(" "),
    }
}

/// 撤回最后一条用户消息，返回撤回的文本内容
#[tauri::command]
/// 撤回最后一条用户消息。委托给 recall_message 统一处理。
pub async fn recall_last_message(
    session_id: String,
    session_manager: tauri::State<'_, SessionManager>,
    app: tauri::AppHandle,
) -> Result<String, String> {
    let ctx = session_manager.get_or_create(&session_id).await;
    let last_user_idx = {
        let session = ctx.memory.lock().await;
        session
            .messages
            .iter()
            .rposition(|m| matches!(m, Message::User { .. }))
            .ok_or_else(|| "没有可撤回的用户消息".to_string())?
    };
    recall_message(
        session_id, None, Some(last_user_idx), None,
        session_manager, app,
    ).await
}

/// 统一撤回入口。recall_last_message / rollback_to_checkpoint_with_recall 均委托至此。
#[tauri::command]
pub async fn recall_message(
    session_id: String,
    message_id: Option<String>,
    user_message_index: Option<usize>,
    prune_metadata_cutoff: Option<u64>,
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
            // 截断前提取 message_id，截断后 idx 就没了
            let mid_for_cleanup = session.message_ids.get(idx).cloned();
            session.messages.truncate(idx);
            session.message_ids.truncate(idx);
            // 清理 agent_run（在截断前已拿到 message_id）
            if let Some(ref mid) = mid_for_cleanup.filter(|s| !s.is_empty()) {
                crate::core::orchestration::agent_runs::cleanup_by_message_id(mid);
            }
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

        // 非 user_message_index 路径：用传入的 message_id 清理
        if let Some(mid) = message_id.as_ref().filter(|s| !s.is_empty()) {
            crate::core::orchestration::agent_runs::cleanup_by_message_id(mid);
        }

        // checkpoint 回滚时的元数据清理
        if let Some(cutoff) = prune_metadata_cutoff {
            if cutoff == 0 {
                session.plan_documents.clear();
            } else {
                session.plan_documents.retain(|d| d.created_at <= cutoff);
            }
        }

        is_empty = session.messages.is_empty();
    }

    if let Some(stored) = stored_target {
        session::delete_session_messages_from_seq(&session_id, stored.seq)?;
    } else if let Some(user_message_index) = user_message_index {
        session::delete_session_messages_from_seq(&session_id, user_message_index)?;
    }

    if is_empty {
        if let Some(token) = ctx.cancel_token.lock().await.as_ref() {
            token.cancel();
        }
        tokio::time::sleep(std::time::Duration::from_secs(1)).await;
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
    app: tauri::AppHandle,
) -> Result<(), String> {
    // 复用统一的删除+回落逻辑：删掉指定会话、选 fallback、对齐激活预设、
    // 广播 active-session-changed（前端据此把 activeSessionId 切到 fallback）。
    // 早期这里只做"删行 + 清内存"，既不回落也不切预设，删掉当前会话后
    // 前端仍指向已删除的会话、后端仍用被删会话的预设。
    session_manager.remove(&id).await;
    switch_away_and_delete_empty_session(&id, &app).await
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

// ── 深度思考档位（会话级） ──

/// 取当前激活预设的「默认思考档位」，并与全局 `agent_audience` 回退合并为确定布尔值。
///
/// 这里是设计文档决策 **D2** 的落点：预设为 `auto` 时回退到
/// `agent_audience == "developer"`，与 `pipeline` 里 `loop_think_default` 的既有语义一致，
/// 保证升级后 developer 用户行为零变化。
async fn resolve_profile_thinking_default(
    config_state: &tauri::State<'_, crate::infra::config::config::ConfigState>,
) -> (bool, String) {
    let audience_default = crate::command::app_config::get_ui_preferences()
        .await
        .map(|prefs| prefs.agent_audience == "developer")
        .unwrap_or(false);

    let cfg = config_state.0.lock().await.clone();
    let active = cfg.active_config();
    let profile_default = session::thinking::ThinkingDefault::parse(&active.thinking_default);

    (profile_default.resolve(audience_default), active.main_model)
}

/// 组装前端的思考档位快照（`resolvedEnabled` 由后端裁决层算出，前端不做二次判断）。
fn build_thinking_snapshot(
    session_id: &str,
    session_mode: session::thinking::ThinkingMode,
    caps: Option<&crate::infra::llm::registry::ModelCapabilities>,
    profile_resolved_default: bool,
) -> serde_json::Value {
    let decision = session::thinking::decide(
        None,
        session_mode,
        profile_resolved_default,
        caps,
    );
    serde_json::json!({
        "sessionId": session_id,
        "thinkingMode": session_mode.as_api(),
        "profileResolvedDefault": profile_resolved_default,
        "resolvedEnabled": decision.enabled,
        "reason": format!("{:?}", decision.reason),
        "noticeI18nKey": decision.notice_i18n_key,
    })
}

/// 读取某会话的思考档位快照。
///
/// 优先从内存 `SessionContext` 取（`switch_session` 已同步），
/// 内存无记录时回落 DB —— 保证冷启动/监控窗口也能拿到权威值。
#[tauri::command]
pub async fn get_session_thinking(
    id: String,
    session_manager: tauri::State<'_, SessionManager>,
    config_state: tauri::State<'_, crate::infra::config::config::ConfigState>,
) -> Result<serde_json::Value, String> {
    let ctx = session_manager.get_or_create(&id).await;
    let raw = ctx.thinking_mode.lock().await.clone();
    let session_mode = session::thinking::ThinkingMode::parse(raw.as_deref().unwrap_or("auto"));

    let (profile_resolved_default, model_id) =
        resolve_profile_thinking_default(&config_state).await;
    let caps = crate::infra::llm::registry::query_capabilities(&model_id);

    Ok(build_thinking_snapshot(
        &id,
        session_mode,
        caps.as_ref(),
        profile_resolved_default,
    ))
}

/// 设置某会话的深度思考档位（`auto` / `always` / `never`）。
///
/// 单一写入口：校验 → 写 DB → 写 `SessionContext` → 广播事件 → **返回权威快照**。
/// 前端以返回值为准（服务端 last-write-wins），不做乐观本地状态。
#[tauri::command]
pub async fn set_session_thinking_mode(
    id: String,
    mode: String,
    session_manager: tauri::State<'_, SessionManager>,
    config_state: tauri::State<'_, crate::infra::config::config::ConfigState>,
    app: tauri::AppHandle,
) -> Result<serde_json::Value, String> {
    let normalized = match mode.trim().to_ascii_lowercase().as_str() {
        "auto" => session::thinking::ThinkingMode::Auto,
        "always" | "on" => session::thinking::ThinkingMode::Always,
        "never" | "off" => session::thinking::ThinkingMode::Never,
        other => {
            return Err(format!(
                "非法的思考档位：{}（只允许 auto / always / never）",
                other
            ))
        }
    };

    // 1) 落库（None = auto = NULL）
    session::update_session_thinking_mode(&id, normalized.as_storage())?;

    // 2) 同步内存态，避免本轮决策读到旧值
    let ctx = session_manager.get_or_create(&id).await;
    *ctx.thinking_mode.lock().await = normalized.as_storage().map(|s| s.to_string());

    // 3) 组装权威快照
    let (profile_resolved_default, model_id) =
        resolve_profile_thinking_default(&config_state).await;
    let caps = crate::infra::llm::registry::query_capabilities(&model_id);
    let snapshot = build_thinking_snapshot(
        &id,
        normalized,
        caps.as_ref(),
        profile_resolved_default,
    );

    // 4) 广播（带 payload，跨窗口各自过滤 sessionId）
    let _ = app.emit("session-thinking-mode-changed", snapshot.clone());

    Ok(snapshot)
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
                                    let t = if s.len() > 120 { let mut e=120; while e>0 && !s.is_char_boundary(e) { e-=1; } &s[..e] } else { &s };
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
                                    let p = if thinking.len() > 80 { let mut e=80; while e>0 && !thinking.is_char_boundary(e) { e-=1; } &thinking[..e] } else { thinking };
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
        let messages_json = serde_json::to_string_pretty(&memory.messages).unwrap_or_default();
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
            raw_content: Some(messages_json),
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
        // 无可恢复的 run 属**正常路径**（绝大多数加载都会走到这里），
        // 不打日志以免每次刷新都刷屏。
        return false;
    };
    println!(
        "[JARVIS] 中断恢复：发现可恢复 run（session {}，额外消息 {} 条，半截正文 {} 字）",
        session_id,
        extra_messages.len(),
        live_content.trim().chars().count()
    );
    for message in extra_messages {
        session::append_message(memory, message, "chat");
    }
    if let Some(message) = recovered_assistant_message(&live_content, &live_thinking) {
        if !assistant_message_exists_at_tail(&memory.messages, &message) {
            session::append_message(memory, message, "chat");
        } else {
            // 去重生效：不再重复写入合并副本。加日志便于日后排查
            // "刷新后又多一条"的复现（此前这里的判定过窄，反复写入）。
            println!(
                "[JARVIS] 中断恢复：半截内容已存在于历史，跳过重复写入（session {}）",
                session_id
            );
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

/// 去掉中断标记后的纯正文，用于恢复去重比较。
///
/// 恢复产出的消息形如 `正文 + 内嵌中断标记`，而库里正常路径下存的是
/// `正文` 与 `标记` **两条独立消息**。若按整段文本比较，二者永远不相等，
/// 去重必然失效 → 每加载一次就多一条合并副本（实测反复出现的问题）。
fn strip_interrupt_marker(text: &str) -> String {
    let mut out = text.to_string();
    while let Some(pos) = out.find("[回复被中断]") {
        // 连同该行开头的引用符号一起裁掉
        let line_start = out[..pos].rfind('\n').map(|i| i + 1).unwrap_or(0);
        let line_end = out[pos..].find('\n').map(|i| pos + i).unwrap_or(out.len());
        out.replace_range(line_start..line_end, "");
    }
    out.trim().to_string()
}

/// 判断"半截助手回复"是否已存在于历史尾部（用于恢复时去重）。
///
/// **实测 bug 的防护（三处窄化）**：
/// 1. 旧实现只与 `messages.last()` 比较 —— 最后一条是中断标记时判定失败；
/// 2. 旧实现比较 `(思考, 正文)` **整对** —— 恢复消息常带思考块，历史里可能只有正文；
/// 3. 旧实现按**整段文本**比较 —— 恢复消息内嵌了中断标记，而库里是正文与标记
///    分开两条，整段比较必然不相等。
///
/// 三者叠加的后果：`recover_interrupted_into_memory()` 每次调用都把同一段半截
/// 内容再写一遍，表现为"刷新一次多一条"，删掉后再刷新又回来。
///
/// 现在改为：尾部窗口（4 条）内扫描，比较前**剥离中断标记**，
/// 且思考与正文**各自判定**——任一已存助手消息含相同正文（或相同思考）即视为已存在。
fn assistant_message_exists_at_tail(messages: &[Message], target: &Message) -> bool {
    let Some((target_thinking, target_text)) = assistant_message_texts(target) else {
        return false;
    };
    let target_text = strip_interrupt_marker(&target_text);
    let target_thinking = strip_interrupt_marker(&target_thinking);

    let recent: Vec<(String, String)> = messages
        .iter()
        .rev()
        .take(4)
        .filter_map(assistant_message_texts)
        .map(|(t, x)| (strip_interrupt_marker(&t), strip_interrupt_marker(&x)))
        .collect();

    if !target_text.is_empty() && recent.iter().any(|(_, x)| x == &target_text) {
        return true;
    }
    !target_thinking.is_empty() && recent.iter().any(|(t, _)| t == &target_thinking)
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
    // 辅助调用统一走带超时的客户端
    let client = api_client::build_utility_client();
    let cfg = {
        let config = crate::infra::config::config::load_config();
        config.active_config().clone()
    };
    crate::core::session::memory::compact_messages(
        &mut *memory,
        &client,
        &cfg.api_key,
        &cfg.base_url,
        &cfg.utility_model,
        crate::infra::llm::api_format::ApiFormat::OpenAI,
    )
    .await
    .map_err(|e| format!("压缩失败: {}", e))?;

    // 从 DB 中删除已清理的 internal/background 消息
    if let Err(e) = crate::core::session::repository::delete_session_messages_by_source(
        session_id,
        &["internal", "background"],
    ) {
        println!("[compact] 清理 internal/background 消息失败: {}", e);
    }

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

#[cfg(test)]
mod recovered_dedup_tests {
    //! **实测 bug 的防护**：中断恢复的去重曾经只比对 `messages.last()`，
    //! 当最后一条是别的东西（如 `source=interrupted` 的中断标记）时判定失败，
    //! 把同一段半截内容重复写入 —— 表现为"每刷新一次就多一条重复消息"。
    use super::assistant_message_exists_at_tail;
    use crate::infra::types::models::*;

    fn assistant(text: &str) -> Message {
        Message::Assistant {
            content: Content::Single(text.to_string()),
        }
    }

    fn assistant_blocks(blocks: Vec<ContentBlock>) -> Message {
        Message::Assistant {
            content: Content::Multiple(blocks),
        }
    }

    fn text_block(text: &str) -> ContentBlock {
        ContentBlock::Text {
            text: text.to_string(),
        }
    }

    /// 回归防护（核心）：目标内容在倒数第二条、最后一条是中断标记时，
    /// 必须判定为"已存在"，否则会重复写入（这正是实测的复现路径）。
    #[test]
    fn detects_match_before_trailing_interrupted_marker() {
        let messages = vec![
            assistant("This reply was cut off mid-sen"),
            assistant("> ⚠️ **[回复被中断]** 上次回复在此处中断，请基于上下文继续完成。"),
        ];
        assert!(
            assistant_message_exists_at_tail(
                &messages,
                &assistant("This reply was cut off mid-sen")
            ),
            "末尾是中断标记时，仍应认出前面的同一段半截内容，避免重复落库"
        );
    }

    /// 最后一条即目标：行为与旧实现一致，不能回归
    #[test]
    fn still_detects_trailing_match() {
        let messages = vec![assistant("同一段内容")];
        assert!(assistant_message_exists_at_tail(
            &messages,
            &assistant("同一段内容")
        ));
    }

    /// 多块消息的正文比较应忽略非 Text 块差异之外的内容
    #[test]
    fn matches_multiple_blocks_by_text() {
        let messages = vec![assistant_blocks(vec![
            ContentBlock::Thinking {
                thinking: "先看看".to_string(),
                signature: String::new(),
            },
            text_block("半截正文"),
        ])];
        assert!(assistant_message_exists_at_tail(
            &messages,
            &assistant("半截正文")
        ));
    }

    /// 不同内容不得误判为已存在，否则真实的半截回复会被漏掉
    #[test]
    fn different_content_is_not_a_match() {
        let messages = vec![assistant("上一轮的完整回复")];
        assert!(!assistant_message_exists_at_tail(
            &messages,
            &assistant("本轮被打断的半截话")
        ));
    }

    /// 超出尾部窗口的历史不应参与判定（避免误吞真正的新内容）
    #[test]
    fn only_scans_recent_tail() {
        let mut messages = vec![assistant("很久以前的同款文本")];
        for _ in 0..5 {
            messages.push(assistant("中间过程的其它回复"));
        }
        assert!(!assistant_message_exists_at_tail(
            &messages,
            &assistant("很久以前的同款文本")
        ));
    }

    /// 用户消息不参与助手消息的去重判定
    #[test]
    fn user_messages_are_ignored() {
        let messages = vec![Message::User {
            content: Content::Single("半截正文".to_string()),
        }];
        assert!(!assistant_message_exists_at_tail(
            &messages,
            &assistant("半截正文")
        ));
    }

    /// **回归防护（核心，实测"删了又回来"）**：库里正常路径是把
    /// 「正文」与「中断标记」存成两条独立消息，而恢复产出的是
    /// 「正文 + 内嵌标记」的合并消息。按整段文本比较必然不相等，
    /// 于是每加载一次就再写一条合并副本。
    ///
    /// 修复后按剥离标记的**纯正文**比较，必须判定为已存在。
    #[test]
    fn merged_recovery_does_not_duplicate_split_stored_pair() {
        let stored = vec![
            assistant("This reply was cut off mid-sen"),
            assistant("> ⚠️ **[回复被中断]** 上次回复在此处中断，请基于上下文继续完成。"),
        ];
        let recovered = assistant(
            "This reply was cut off mid-sen\n\n> ⚠️ **[回复被中断]** 上次回复在此处中断，请基于上下文继续完成。",
        );
        assert!(
            assistant_message_exists_at_tail(&stored, &recovered),
            "合并副本与已存的分体消息是同一段内容，不得重复写入（实测 bug）"
        );
    }

    /// 合并副本自身已存在时也不得再写一次（连续多次加载/刷新的场景）
    #[test]
    fn merged_recovery_is_idempotent() {
        let merged = assistant(
            "半截正文\n\n> ⚠️ **[回复被中断]** 上次回复在此处中断，请基于上下文继续完成。",
        );
        let stored = vec![merged.clone()];
        assert!(assistant_message_exists_at_tail(&stored, &merged));
    }

    /// 剥离标记不得伤及正文本身（防止误判把新内容吞掉）
    #[test]
    fn stripping_marker_keeps_body_intact() {
        assert_eq!(
            super::strip_interrupt_marker(
                "正文第一行\n\n> ⚠️ **[回复被中断]** 上次回复在此处中断，请基于上下文继续完成。"
            ),
            "正文第一行"
        );
        assert_eq!(super::strip_interrupt_marker("纯正文没有标记"), "纯正文没有标记");
    }
}
