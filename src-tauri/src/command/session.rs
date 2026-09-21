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
use crate::core::orchestration::{agent_run_repository, agent_runs};
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
/// 模式是会话级属性：写会话状态 + **落库**（`sessions.work_mode`，随会话恢复），
/// 再广播 `agent-work-mode-changed`（前端已在监听该事件）。
/// 设置里的"默认工作模式"只决定新会话的初始值，不再被这里的切换牵动。
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
    if let Err(e) = crate::core::session::update_session_work_mode(&session_id, &mode) {
        eprintln!("[JARVIS] 工作模式落库失败（会话 {}）：{}", session_id, e);
    }

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

/// 已归档的会话列表（界面「归档」区入口）。
///
/// 与 `list_sessions` 成对：归档的会话不在这里、也不在那里同时出现。
#[tauri::command]
pub async fn list_deleted_sessions() -> Result<Vec<session::SessionMeta>, String> {
    session::list_deleted_sessions()
}

/// 取消归档，把一个会话放回活跃列表。
///
/// 它此前保留的消息、快照数据、回收站本体立即重新可用（归档从不删这些）。
#[tauri::command]
pub async fn restore_session(id: String) -> Result<(), String> {
    session::restore_session(&id)
}

/// **彻底删除**一个会话（不可恢复）：真删行 + 清快照数据 + 清回收站目录。
///
/// 入口在侧边栏「归档」区里 —— 用户对已归档的会话点「彻底删除」才会走到这里。
/// 与归档 [`delete_session`]（只打标记、可取消）的区别见 `core::session` 的说明。
///
/// 这是 `hard_delete_session` 目前**唯一**的用户入口：另一个调用点（自动清理空会话）
/// 已随归档改造移除 —— 归档不再按"有没有内容"分流。
/// 另有 [`delete_project`] 连带真删名下会话，效果相同但走各自的 SQL，不复用本函数。
#[tauri::command]
pub async fn purge_session(id: String) -> Result<(), String> {
    session::hard_delete_session(&id)
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

    // 先验后建：项目目录被删/改名时直接拒绝，不创建会话记录（避免孤儿空会话）。
    // 存量项目记录可能带 \\?\ 前缀，展示前剥离。
    let working_directory = project_id
        .as_ref()
        .and_then(|pid| session::repository::get_project_path(pid).ok().flatten())
        .map(|ws| session::strip_extended_path_prefix(&ws).to_string());
    if let Some(ws) = &working_directory {
        if !std::path::Path::new(ws).exists() {
            return Err(format!(
                "项目目录已不存在: {}（可能已被删除或移动），无法创建沙箱会话。请重新打开项目后再试。",
                ws
            ));
        }
    }

    let meta = session::create_session(project_id);

    // 初始化上下文（存量记录带 \\?\ 前缀，绑定前剥离，下游报错文案与显示才干净）
    let ctx = session_manager.get_or_create(&meta.id).await;
    *ctx.workspace.lock().await = meta
        .working_directory
        .clone()
        .map(|ws| std::path::PathBuf::from(session::strip_extended_path_prefix(&ws)));

    // 挂载项目后把该项目的授权账本注入内存（与 state.rs 会话恢复路径同口径）
    if let Some(root) = ctx.workspace.lock().await.clone() {
        crate::core::tools::framework::allowance_store::load_into(&ctx, &root).await;
    }

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
    let abs_canonical = normalized.canonicalize()
        .map_err(|e| format!("解析路径失败: {}", e))?
        .to_string_lossy()
        .to_string();
    // 剥离 \\?\ 前缀后再入库：展示美观，且与用户输入的普通路径一致
    let abs = session::strip_extended_path_prefix(&abs_canonical).to_string();

    // 已存在则直接返回（兼容存量记录：老数据以 canonicalize 原样含 \\?\ 存储）
    if let Ok(Some(project)) = crate::core::session::repository::get_project_by_path(&abs) {
        return Ok(project);
    }
    if abs != abs_canonical {
        if let Ok(Some(project)) = crate::core::session::repository::get_project_by_path(&abs_canonical) {
            return Ok(project);
        }
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
    use crate::core::session::repository;
    // 删项目会连带删掉它名下的所有会话（repository 里那条 DELETE FROM sessions），
    // 所以要**先**把会话 id 抓出来，删完就查不到了；再逐个清回滚侧产物
    // （快照表没有 FK 级联，不清就是永久孤儿）。
    let session_ids = repository::list_session_ids_of_project(&id).unwrap_or_default();
    repository::delete_project(&id)?;
    for sid in session_ids {
        crate::core::session::purge_rollback_artifacts(&sid);
    }
    Ok(())
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
/// 删除会话后自动回落（本文件 `switch_away_and_delete_session`）。
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

/// 归档会话后自动切换到下一个可用会话（若无则创建新会话）。
///
/// **这是所有归档路径的收口**（用户点归档 / 回滚后会话变空 / 删项目）。
/// 函数名里的 `delete`、"删除"时代的命名一律保留不动 —— 改名要同时动 tauri 命令名、
/// 前端 invoke 与 DB 列，成本远大于收益；语义以本注释为准。
pub async fn switch_away_and_delete_session(
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

    // 归档：一律只打 `deleted_at` 标记，**不再按有没有内容分流**。
    //
    // 原先空会话走硬删除（没有内容可挽留，留着只会让列表堆噪音）—— 那是"删除"语义下
    // 的合理取舍。改成归档后这条分流必须去掉：归档的语义是"东西还在，只是收起来"，
    // 空会话归档后也得在归档区里找得到。否则用户点了归档，记录却凭空消失，
    // 而且是静默的（不报错、归档区里也翻不到），等同于数据丢失。
    //
    // 查询失败（会话已不存在 / 已在归档列表）→ 什么都不做，只完成"切走"。
    if session::get_session_meta(deleted_session_id).is_ok() {
        session::delete_session(deleted_session_id)?;
    }
    if let Some(manager) = app.try_state::<SessionManager>() {
        manager.remove(deleted_session_id).await;
    }

    // 对齐激活预设，否则后端会继续用**刚被删掉的会话**的预设
    if let Some(state) = app.try_state::<crate::infra::config::config::ConfigState>() {
        align_active_profile_to_session(app, &state, fallback_profile_id.as_deref());
    }

    // 没有 fallback（删掉了最后一个会话）时清掉 last_active_session_id，
    // 否则下次启动会拿一个已删除的 id 去 switch_session，白报一次错再回落。
    if fallback_id.is_none() {
        let _ = crate::core::session::repository::clear_last_active_session_id();
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
            // 收集**所有被撤回消息**的 id：每条 user 消息都可能触发过 agent_run，
            // 逐个清理，防止崩溃恢复机制把已撤回的内容重新补回（复活）
            let removed_ids: Vec<String> = visible[pos..]
                .iter()
                .map(|m| m.message_id.clone())
                .filter(|id| !id.is_empty())
                .collect();
            let keep: Vec<_> = visible.into_iter().take(pos).collect();
            session.messages = keep.iter().map(|m| m.content.clone()).collect();
            session.message_ids = keep.iter().map(|m| m.message_id.clone()).collect();
            for mid in &removed_ids {
                crate::core::orchestration::agent_runs::cleanup_by_message_id(mid);
            }
            target_content
        } else if let Some(idx) = user_message_index {
            if idx >= session.messages.len() {
                return Err("撤回消息不存在".to_string());
            }
            let target = session.messages[idx].clone();
            // 截断前收集 idx 及之后**所有**被撤消息的 id（与 message_id 分支同口径：
            // 每条都可能触发过 agent_run），截断后 idx 就没了
            let removed_ids: Vec<String> = session.message_ids[idx..]
                .iter()
                .filter(|id| !id.is_empty())
                .cloned()
                .collect();
            session.messages.truncate(idx);
            session.message_ids.truncate(idx);
            for mid in &removed_ids {
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
        switch_away_and_delete_session(&session_id, &app).await?;
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
    switch_away_and_delete_session(&id, &app).await
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

// ── 深度思考档位（会话级，布尔） ──

/// 取出**当前激活预设**的主模型，用于查模型能力。
///
/// 与旧版 `resolve_profile_thinking_default` 的区别：**不再解析任何默认档位**。
/// 设置默认值的解析只发生在"会话首次发消息"那一刻（`pipeline::start_run`），
/// 这里只为构造 UI 快照而查能力（能力夹紧仍然需要它）。
async fn current_main_model(
    config_state: &tauri::State<'_, crate::infra::config::config::ConfigState>,
) -> String {
    let cfg = config_state.0.lock().await.clone();
    cfg.active_config().main_model
}

/// 组装前端的思考档位快照（`resolvedEnabled` 由后端裁决层算出，前端不做二次判断）。
///
/// `thinkingEnabled` 与 `resolvedEnabled` 的区别：
/// - `thinkingEnabled`：会话库里存的值（用户意图 / 建会话时固化的值）；
/// - `resolvedEnabled`：**实际生效**的值（已过模型能力夹紧，如 DeepSeek 上恒为 true）。
///
/// 前端渲染开关高亮用后者，但要保留前者以便提示"意愿被模型否决"。
fn build_thinking_snapshot(
    session_id: &str,
    session_mode: session::thinking::ThinkingMode,
    caps: Option<&crate::infra::llm::registry::ModelCapabilities>,
) -> serde_json::Value {
    let decision = session::thinking::decide(None, session_mode, caps);
    serde_json::json!({
        "sessionId": session_id,
        "thinkingEnabled": session_mode.0,
        "resolvedEnabled": decision.enabled,
        "reason": format!("{:?}", decision.reason),
        "noticeI18nKey": decision.notice_i18n_key,
    })
}

/// 计算「**尚无会话时**」深度思考开关该显示成什么（只读，不落库）。
///
/// ## 为什么需要它
///
/// 新建会话在**发送首条消息之前并不存在**（`create_session` 时才建行），
/// 但输入框上的开关此时就要有个确定状态。按「跟随全局」的定义，
/// 这个值等于「设置默认档位 + 当前主模型是否强制思考」的解析结果——
/// 这套解析**只能在后端做**（前端不读设置、不查能力），所以单独开一个命令。
///
/// 返回值只是为了**首屏渲染**：真正入库的值仍在首条消息时由
/// `pipeline::ensure_session_thinking_initialized` 重新解析固化，
/// 所以这里即使短暂过期也污染不到真实请求。
///
/// 与 `get_session_thinking` 的回包同构（`sessionId` 为空串），前端可共用解析逻辑。
#[tauri::command]
pub async fn get_pending_thinking_enabled(
    config_state: tauri::State<'_, crate::infra::config::config::ConfigState>,
) -> Result<serde_json::Value, String> {
    let model_id = current_main_model(&config_state).await;
    let caps = crate::infra::llm::registry::query_capabilities(&model_id);
    let thinking_forced = caps.as_ref().map(|c| c.thinking_forced).unwrap_or(false);

    let default_raw = crate::command::app_config::read_file()
        .ui_preferences
        .thinking_default;
    let resolved = session::thinking::ThinkingDefault::parse(&default_raw).resolve(thinking_forced);

    let decision = session::thinking::decide(
        None,
        session::thinking::ThinkingMode(resolved),
        caps.as_ref(),
    );
    Ok(serde_json::json!({
        "sessionId": "",
        "thinkingEnabled": resolved,
        "resolvedEnabled": decision.enabled,
        "reason": format!("{:?}", decision.reason),
        "noticeI18nKey": decision.notice_i18n_key,
    }))
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
    let session_mode = session::thinking::ThinkingMode(*ctx.thinking_mode.lock().await);

    let model_id = current_main_model(&config_state).await;
    let caps = crate::infra::llm::registry::query_capabilities(&model_id);

    Ok(build_thinking_snapshot(&id, session_mode, caps.as_ref()))
}

/// 设置某会话的深度思考档位（**布尔**）。
///
/// 与工作模式/权限档位完全同构的"单一写入口"：
/// 写 DB → 写 `SessionContext` → 广播事件 → **返回权威快照**。
/// 前端以返回值为准（服务端 last-write-wins），不做乐观本地状态。
///
/// 调用时机：界面拨动后**发送消息**时由前端 flush（与另外两个字段共用一个函数），
/// 因此"拨了不发消息"的表态不会被记住。
#[tauri::command]
pub async fn set_session_thinking_enabled(
    id: String,
    enabled: bool,
    session_manager: tauri::State<'_, SessionManager>,
    config_state: tauri::State<'_, crate::infra::config::config::ConfigState>,
    app: tauri::AppHandle,
) -> Result<serde_json::Value, String> {
    // 1) 落库（布尔，永不写 NULL）
    session::update_session_thinking_mode(&id, enabled)?;

    // 2) 同步内存态，避免本轮决策读到旧值
    let ctx = session_manager.get_or_create(&id).await;
    *ctx.thinking_mode.lock().await = enabled;

    // 3) 组装权威快照
    let model_id = current_main_model(&config_state).await;
    let caps = crate::infra::llm::registry::query_capabilities(&model_id);
    let snapshot = build_thinking_snapshot(
        &id,
        session::thinking::ThinkingMode(enabled),
        caps.as_ref(),
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
                                ContentBlock::Context { text } => {
                                    // 动态上下文块（意图标签 / 能力边界 / 项目结构 / 用户画像）。
                                    // 出网前会被 `materialize_context_blocks_for_wire` 翻译成普通
                                    // Text 一并发出，所以它**真实占用** prompt token。
                                    // 这里必须渲染，否则本命令重建出的 messages 段会系统性偏低，
                                    // 与 `pipeline::build_context_estimate` 的口径对不上。
                                    if !text.trim().is_empty() {
                                        out.push_str("  ← [Context]\n");
                                        out.push_str(text.trim());
                                        out.push('\n');
                                    }
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
) -> Result<Vec<crate::core::orchestration::agent_runs::AgentRunLoopEvent>, String> {
    Ok(crate::core::orchestration::agent_runs::list_loop_events(
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
    // v15：`prepare_resume` 不再返回 checkpoint（已无 checkpoint 体系），
    // 恢复上下文由门卫 `ensure_session_recovered` 从 events 重放。
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

/// 应用退出（ExitRequested）收尾：把内存态会话整仓落库，并把所有仍为
/// Running 的 run 标记为 Interrupted。
///
/// 此前"关闭窗口 = 进程死亡无痕"：事件驱动检查点的落库时机里没有
/// "进程退出"这一项，活跃 run 永久停留 Running，检查点之后的内存进度
/// 全部丢失且不留任何痕迹（崩溃恢复也因状态门槛不满足而无法收口）。
/// 此处在退出路径做最后的同步落库 —— blocking 系列锁在 ExitRequested
/// 已有先例（后台进程树清理），代价是关窗多一次整仓写（大会话数百毫秒量级）。
pub fn finalize_active_runs_on_exit(handle: &tauri::AppHandle) {
    use tauri::Manager;

    if let Some(sm) = handle.try_state::<SessionManager>() {
        // 先收集再落库：锁只覆盖 clone 的瞬间即释放，不与 pipeline 的短锁长持竞争
        let snapshots: Vec<(String, SessionMemory)> = {
            let map = sm.0.blocking_read();
            map.iter()
                .map(|(sid, ctx)| {
                    let memory = ctx.memory.blocking_lock();
                    (sid.clone(), memory.clone())
                })
                .collect()
        };
        for (sid, memory) in &snapshots {
            let _ = session::save_session(sid, memory, None);
        }
    }
    crate::core::orchestration::agent_runs::mark_running_interrupted_on_exit();
}

/// 中断恢复**唯一闸门**（门卫）。
///
/// 全系统所有"把中断/崩溃遗留的消息补回会话"的路径（历史命令、全量/分页消息命令、
/// 恢复命令、发消息的前端前置与后端 pre_loop）都必须经过本函数，禁止各自实现恢复链
/// ——此前 6 个入口各自手抄"恢复 + save + 盖章"且无互斥，10b68285 的 13ms
/// 双写坏数据即两个入口并发的产物。
///
/// 流程（锁 → 干活 → 先落库后盖章）：
///
/// 1. **活跃闸门**：本会话存在进程内活跃 run（`ctx.active_run_id`）→ 直接返回。
///    这是比 STALE 时间戳强得多的"run 还活着"判定——活跃 run 跑得再久也不会被误判遗留。
/// 2. **遗留发现**：`find_interrupted_run`（interrupted / running+STALE / recovering+超时）。
/// 3. **原子抢占（锁）**：`try_claim_run_for_recovery` 以单条条件 UPDATE 置为
///    `recovering`。SQLite 单写者保证并发只有一个赢家，抢不到的直接退出。
/// 4. **重放增量**：从 agent_run_events 重建 + diff_tail，只补缺的部分（纯计算）。
/// 5. **先落库后盖章**：append 后 `save_session`，成功才 `mark_run_recovered`；
///    盖章失败必须打日志——run 停留 recovering，由超时接管机制兜底重试，
///    绝不允许静默吞掉（否则要么永久卡 recovering，要么重复恢复）。
///
/// 返回是否补回了消息（调用方据此刷新界面状态）。
pub(crate) fn ensure_session_recovered(
    session_id: &str,
    memory: &mut SessionMemory,
    active_run_id: Option<&str>,
) -> bool {
    session::normalize_message_ids(memory);

    // 闸门 1：会话内有 run 正在跑 → 不做任何恢复
    if active_run_id.is_some() {
        return false;
    }

    // 闸门 2：发现遗留 run（无可恢复属正常路径，绝大多数加载走这里，不打日志）
    let Some(run) = crate::core::orchestration::agent_runs::find_interrupted_run(session_id) else {
        return false;
    };

    // 闸门 3：原子抢占。stale_cutoff 同时承担两个职责：
    // running 遗留的 STALE 判定 + recovering 半途而废的接管判定
    let now_ms = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as i64;
    let stale_cutoff = now_ms
        .saturating_sub(crate::core::orchestration::agent_runs::RUN_STALE_MS as i64);
    match agent_run_repository::try_claim_run_for_recovery(&run.run_id, stale_cutoff) {
        Ok(true) => {}
        Ok(false) => return false, // 被其它入口/实例抢走：它们正在恢复，直接退出
        Err(e) => {
            eprintln!("[JARVIS][恢复] 抢占 run {} 失败：{}", run.run_id, e);
            return false;
        }
    }

    // 干活：从 events 重放 + diff 增量（纯计算，状态已由抢占闸门保证独占）
    let current_messages = memory.messages.clone();
    match agent_runs::recover_outcome_for_run(&run, &current_messages) {
        agent_runs::RecoveryOutcome::None => {
            // 无内容可补且无悬尾：内容已完整落库。也必须把 run 标掉结束恢复态，
            // 否则每次加载都重复抢占空转（无害但浪费且日志噪音）。
            if let Err(e) = agent_runs::mark_run_recovered(&run.run_id) {
                eprintln!(
                    "[JARVIS][恢复] 收口：标记 run {} 为已恢复失败：{}（停留 recovering，待超时接管）",
                    run.run_id, e
                );
            }
            false
        }
        agent_runs::RecoveryOutcome::Content { messages } => {
            println!(
                "[JARVIS][恢复] 抢占 run {} 成功，从 events 重放出 {} 条消息（session {}）",
                run.run_id,
                messages.len(),
                session_id
            );
            // v15：重放序列本身已是**按 loop 顺序铺开**的完整结构
            //（assistant 响应 → user 工具结果 → 下一轮 …），
            // 半截内容本就住在最后一个 loop 的 resp_blocks 里，由重放一并带出。
            for message in messages {
                session::append_message(memory, message, "chat");
            }
            finish_recovery(session_id, memory, &run.run_id)
        }
        agent_runs::RecoveryOutcome::NeedsClosure => {
            // 崩溃 run 无内容可补但会话尾部悬尾（最后一条是 user 且无人回应）：
            // 补一条 assistant 中断占位，维持消息级 user/assistant 严格交替。
            //
            // source="interrupted" 会进界面渲染；阶段二起小字由**结构化 kind**
            // 驱动（`command/history.rs::interrupt_notice_for`），正文不再放标记
            // 文本——标记在发送给模型前按 kind 拼回。
            println!(
                "[JARVIS][恢复] run {} 无内容可补，补中断占位收口（session {}）",
                run.run_id, session_id
            );
            session::append_message_with_kind(
                memory,
                Message::Assistant {
                    content: Content::Single(String::new()),
                },
                "interrupted",
                Some(crate::infra::types::models::InterruptKind::AppClosed.as_str()),
            );
            finish_recovery(session_id, memory, &run.run_id)
        }
    }
}

/// 恢复收尾（第二刀）：**先落库，后盖章；盖章失败必须喊出来**。
///
/// `save_session` 不返回 Result（历史签名，19 个调用点，保持口径不变）：
/// 其内部的 DB 写失败同样会作用在下方盖章的 update 上——真失败时 run
/// 停留 `recovering`，由超时接管机制在下一次恢复时重试，不会丢恢复机会。
fn finish_recovery(session_id: &str, memory: &SessionMemory, run_id: &str) -> bool {
    session::save_session(session_id, memory, None);
    if let Err(e) = agent_runs::mark_run_recovered(run_id) {
        eprintln!(
            "[JARVIS][恢复] 严重：session {} 落库完成，但标记 run {} 为已恢复失败：{}（run 停留 recovering，待超时接管重试）",
            session_id, run_id, e
        );
    }
    true
}


#[tauri::command]
pub async fn recover_interrupted_session_messages(
    session_id: String,
    session_manager: tauri::State<'_, SessionManager>,
    app: tauri::AppHandle,
) -> Result<bool, String> {
    let ctx = session_manager.get_or_create(&session_id).await;
    let active_run_id = ctx.active_run_id.lock().await.clone();
    let recovered = {
        let mut memory = ctx.memory.lock().await;
        ensure_session_recovered(&session_id, &mut memory, active_run_id.as_deref())
    };
    if recovered {
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
    let keep = crate::infra::types::constants::COMPACT_MIN_MESSAGES;
    if memory.messages.len() <= keep {
        return Ok(format!("消息不足（仅有 {} 条，下限 {} 条），无需压缩。", memory.messages.len(), keep));
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
