//! # lifecycle.rs — 会话生命周期编排、撤回、中断恢复与压缩领域服务
//!
//! 集中承载跨命令与 Agent 管线复用的会话级核心编排：
//! - 中断恢复唯一闸门 `ensure_session_recovered()`（原子抢占 + 先落库后盖章）
//! - 进程退出收尾 `finalize_active_runs_on_exit()`
//! - 会话归档与当前会话切走 `switch_away_and_delete_session()` / `switch_away()`
//! - 激活模型预设与当前会话对齐 `align_active_profile_to_session()`
//! - 已持执行槽的消息撤回 `recall_message_locked()`
//! - 会话上下文压缩 `compact_session_memory()`
//!
//! ## Key Exports
//! - `ensure_session_recovered()`: 崩溃/中断残留恢复的唯一闸门
//! - `finalize_active_runs_on_exit()`: 应用退出时同步落库并收口活跃 run
//! - `align_active_profile_to_session()`: 对齐全局激活预设与目标会话预设
//! - `switch_away_and_delete_session()` / `switch_away()`: 归档与切走当前会话
//! - `is_real_user_message()` / `recall_message_locked()`: 真实用户消息判定与持锁撤回
//! - `compact_session_memory()`: 执行单级 LLM 摘要压缩并同步落库
//!
//! ## Dependencies
//! - Internal: `crate::core::session`, `crate::core::orchestration::{agent_run_repository, agent_runs}`, `crate::core::runtime`
//! - External: `tauri`

use crate::core::orchestration::{agent_run_repository, agent_runs};
use crate::core::runtime::{SessionCleanupResult, SessionContext, SessionManager};
use crate::core::session;
use crate::infra::llm::api_client;
use crate::infra::types::models::{
    Content, ContentBlock, InterruptKind, Message, MessageSource, SessionMemory,
};
use tauri::{Emitter, Manager};

/// 把"当前会话"的模型预设同步为 `AppConfig.active_profile_id` 并落库。
///
/// **为什么必须有这个函数**：模型预设是会话级状态（`sessions.profile_id`），
/// 但 pipeline 每次读的是 `AppConfig.active_profile_id`。因此**任何"让某会话成为
/// 当前会话"的后端路径都必须对齐一次**，否则后续请求会继续用上一个会话/全局默认的预设。
///
/// 已覆盖的路径：启动恢复会话、点击切会话（`command::session::switch_session`）、
/// 归档当前会话后切到新对话（本文件 `switch_away`，重置为全局默认预设）。
pub fn align_active_profile_to_session(
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

/// 归档一个会话，并处理它是不是"当前会话"。
///
/// **这是单条会话归档路径的收口**（用户点归档 / 回滚后会话变空）。项目归档已把名下会话
/// 在同一事务里整体打上标记，不再走本函数，但"切走"那一段完全复用 [`switch_away`]。
pub async fn switch_away_and_delete_session(
    deleted_session_id: &str,
    app: &tauri::AppHandle,
) -> Result<(), String> {
    // 归档一律只打 `deleted_at` 标记，不再按有没有内容分流；
    // 查询失败（会话已不存在 / 已在归档列表）则跳过删除，只完成"切走"。
    if session::get_session_meta(deleted_session_id).is_ok() {
        session::delete_session(deleted_session_id)?;
    }
    if let Some(manager) = app.try_state::<SessionManager>() {
        manager.remove(deleted_session_id).await;
    }

    switch_away(deleted_session_id, app).await
}

/// 归档会话后的"当前会话"处理。
///
/// 口径只看被处理的是不是**当前会话**：
/// - 是当前会话 → 清空会话指针，前端回到新对话欢迎页，并将预设对齐为全局默认；
/// - 不是当前会话 → 当前会话保持不变，只广播事件让前端清理被删会话的缓存。
pub async fn switch_away(deleted_session_id: &str, app: &tauri::AppHandle) -> Result<(), String> {
    let last_active = session::get_last_active_session_id();
    let was_active = last_active.as_deref() == Some(deleted_session_id);
    let next_active_id = if was_active { None } else { last_active };

    if was_active {
        if let Some(state) = app.try_state::<crate::infra::config::config::ConfigState>() {
            align_active_profile_to_session(app, &state, None);
        }
        // 指针不留在已归档会话上，否则下次启动会拿一个已删除的 id 去 switch_session
        let _ = session::repository::clear_last_active_session_id();
    }

    let _ = app.emit(
        "active-session-changed",
        SessionCleanupResult {
            deleted_session_id: Some(deleted_session_id.to_string()),
            active_session_id: next_active_id,
        },
    );
    let _ = app.emit("session-updated", ());

    Ok(())
}

/// 这条存储消息是否是"真实用户输入"（不是工具结果 / 后台通知 / 压缩摘要）。
pub fn is_real_user_message(stored: &session::repository::StoredSessionMessage) -> bool {
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

/// 已持有执行槽时使用的撤回实现。
///
/// 文件回滚与会话截断必须处于同一个执行槽内：否则删完文件后 `recall_message`
/// 才因为"任务仍在运行"失败，会留下工作区已变、历史没变的半截状态。
pub async fn recall_message_locked(
    session_id: &str,
    message_id: Option<String>,
    user_message_index: Option<usize>,
    prune_metadata_cutoff: Option<u64>,
    ctx: &SessionContext,
    app: &tauri::AppHandle,
) -> Result<String, String> {
    let visible = session::list_visible_session_messages(session_id)?;
    let position = if let Some(message_id) = message_id.filter(|id| !id.trim().is_empty()) {
        visible
            .iter()
            .position(|message| message.message_id == message_id)
            .ok_or_else(|| "撤回消息不存在".to_string())?
    } else if let Some(index) = user_message_index {
        if index >= visible.len() {
            return Err("撤回消息不存在".to_string());
        }
        index
    } else {
        return Err("撤回消息不存在".to_string());
    };

    let stored_target = visible[position].clone();
    if !is_real_user_message(&stored_target) {
        return Err("撤回目标不是用户消息".to_string());
    }
    let recalled_text = match &stored_target.content {
        Message::User { content } => message_text_content(content),
        _ => return Err("撤回目标不是用户消息".to_string()),
    };
    // 被撤回区间内的所有 message_id：每条都可能触发过 agent_run，逐个清理，
    // 防止崩溃恢复机制把已撤回的内容重新补回（复活）。
    let removed_ids: Vec<String> = visible[position..]
        .iter()
        .map(|message| message.message_id.clone())
        .filter(|id| !id.trim().is_empty())
        .collect();

    let is_empty;
    {
        let mut session = ctx.memory.lock().await;
        // 在内存里按 message_id 定位同一条消息。压缩摘要等非可见消息仍会留在内存中，
        // 所以不能直接用"可见表下标"截断内存。
        let memory_position = session
            .message_ids
            .iter()
            .position(|id| id == &stored_target.message_id)
            .ok_or_else(|| "会话内存与消息表不一致，请刷新会话后重试".to_string())?;
        session.messages.truncate(memory_position);
        session.message_ids.truncate(memory_position);
        for message_id in &removed_ids {
            agent_runs::cleanup_by_message_id(message_id);
        }

        if let Some(cutoff) = prune_metadata_cutoff {
            if cutoff == 0 {
                session.plan_documents.clear();
            } else {
                session
                    .plan_documents
                    .retain(|document| document.created_at <= cutoff);
            }
        }
        is_empty = session.messages.is_empty();
    }

    // 顺序不能反：关联清理的子查询要靠 session_messages 里的 seq 找到 message_id。
    crate::core::rollback::patch_store::delete_checkpoint_user_message_links_from_seq(
        session_id,
        stored_target.seq,
    )?;
    session::delete_session_messages_from_seq(session_id, stored_target.seq)?;

    if is_empty {
        switch_away_and_delete_session(session_id, app).await?;
    } else {
        {
            let memory = ctx.memory.lock().await.clone();
            session::save_session(session_id, &memory, None);
        }
        // 保存后从 DB 重新加载，确保内存与持久化数据完全一致
        match session::load_session(session_id) {
            Ok(reloaded) => {
                let mut session = ctx.memory.lock().await;
                *session = reloaded;
            }
            Err(e) => {
                eprintln!("[Recall] 撤回后重新加载会话内存失败: {}", e);
            }
        }
        let _ = app.emit("session-updated", ());
        // 撤回会截断消息表：可见条数与最新 seq 同时变化，必须广播
        session::notify_messages_changed(app, session_id, "recall");
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

/// 应用退出（ExitRequested）收尾：把内存态会话整仓落库，并把所有仍为
/// Running 的 run 标记为 Interrupted。
pub fn finalize_active_runs_on_exit(handle: &tauri::AppHandle) {
    if let Some(sm) = handle.try_state::<SessionManager>() {
        // 先收集再落库：锁只覆盖 clone 的瞬间即释放，不与 pipeline 的短锁长持竞争
        let snapshots = sm.snapshot_all_memories_blocking();
        for (sid, memory) in &snapshots {
            let _ = session::save_session(sid, memory, None);
        }
    }
    agent_runs::mark_running_interrupted_on_exit();
}

/// 中断恢复**唯一闸门**（门卫）。
///
/// 全系统所有"把中断/崩溃遗留的消息补回会话"的路径（历史命令、全量/分页消息命令、
/// 恢复命令、发消息的前端前置与后端 pre_loop）都必须经过本函数，禁止各自实现恢复链。
///
/// 流程（锁 → 干活 → 先落库后盖章）：
/// 1. **活跃闸门**：本会话存在进程内活跃 run（`ctx.active_run_id`）→ 直接返回；
/// 2. **遗留发现**：`find_interrupted_run`（interrupted / running+STALE / recovering+超时）；
/// 3. **原子抢占（锁）**：`try_claim_run_for_recovery` 单条条件 UPDATE 置为 `recovering`；
/// 4. **重放增量**：从 `agent_run_events` 重建 + `diff_tail`，只补缺的部分；
/// 5. **先落库后盖章**：append 后 `save_session`，成功才 `mark_run_recovered`。
///
/// 返回是否补回了消息（调用方据此刷新界面状态）。
pub fn ensure_session_recovered(
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
    let Some(run) = agent_runs::find_interrupted_run(session_id) else {
        return false;
    };

    // 闸门 3：原子抢占。stale_cutoff 同时承担两个职责：
    // running 遗留的 STALE 判定 + recovering 半途而废的接管判定
    let now_ms = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as i64;
    let stale_cutoff = now_ms.saturating_sub(agent_runs::RUN_STALE_MS as i64);
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
            for message in messages {
                session::append_message(memory, message, MessageSource::Chat);
            }
            finish_recovery(session_id, memory, &run.run_id)
        }
        agent_runs::RecoveryOutcome::NeedsClosure => {
            println!(
                "[JARVIS][恢复] run {} 无内容可补，补中断占位收口（session {}）",
                run.run_id, session_id
            );
            session::append_message_with_kind(
                memory,
                Message::Assistant {
                    content: Content::Single(String::new()),
                },
                MessageSource::Interrupted,
                Some(InterruptKind::AppClosed.as_str()),
            );
            finish_recovery(session_id, memory, &run.run_id)
        }
    }
}

/// 恢复收尾：**先落库，后盖章；盖章失败必须记录日志**。
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

/// 对指定会话执行单级 LLM 摘要压缩并同步落库与广播。
pub async fn compact_session_memory(
    session_id: &str,
    session_manager: &SessionManager,
    app: &tauri::AppHandle,
) -> Result<String, String> {
    let ctx = session_manager.get_or_create(session_id).await;
    let mut memory = ctx.memory.lock().await;
    let keep = crate::infra::types::constants::COMPACT_MIN_MESSAGES;
    if memory.messages.len() <= keep {
        return Ok(format!(
            "消息不足（仅有 {} 条，下限 {} 条），无需压缩。",
            memory.messages.len(),
            keep
        ));
    }
    // 辅助调用统一走带超时的客户端
    let client = api_client::build_utility_client();
    let cfg = {
        let config = crate::infra::config::config::load_config();
        config.active_config().clone()
    };
    session::memory::compact_messages(
        &mut memory,
        &client,
        &cfg.api_key,
        &cfg.base_url,
        &cfg.utility_model,
        crate::infra::llm::api_format::ApiFormat::OpenAI,
    )
    .await
    .map_err(|e| format!("压缩失败: {}", e))?;

    // 从 DB 中删除刚才被压缩剔除的 Placeholder 填充消息行
    if let Err(e) = session::repository::delete_session_messages_by_source(
        session_id,
        &[MessageSource::Placeholder],
    ) {
        println!("[compact] 清理内部填充消息失败: {}", e);
    }

    let ids: Vec<String> = (0..memory.messages.len())
        .map(|i| format!("compact:{}:{}", i, uuid::Uuid::new_v4().simple()))
        .collect();
    memory.message_ids = ids;
    let cloned = memory.clone();
    drop(memory);
    drop(ctx);

    let _ = session::save_session(session_id, &cloned, None);
    // 压缩会隐藏被摘要的旧消息：可见条数变化，广播一次让其它视图标脏
    session::notify_messages_changed(app, session_id, "compaction");
    Ok("上下文已压缩。".to_string())
}
