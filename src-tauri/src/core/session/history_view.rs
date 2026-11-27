//! # history_view.rs — 会话历史结构化读模型投影服务
//!
//! 将数据库中的会话消息、工具调用结果、中断状态与撤回检查点关联投影为前端可直接渲染的
//! 结构化视图（`SessionMessage` / `AgentTurnSnapshot` / `PagedSessionMessages`）。
//!
//! ## Key Exports
//! - `SessionMessage` / `AgentTurnTokens` / `AgentTurnSnapshot` / `AgentTextBlock` / `AgentThinkingBlock` / `AgentToolCallView` / `AgentExecutionLog` / `PagedSessionMessages`
//! - `load_session_messages()`: 带中断恢复前置的全量结构化消息投影
//! - `load_session_messages_paged()`: 带中断恢复前置的按轮游标分页投影
//!
//! ## Dependencies
//! - Internal: `crate::core::session`, `crate::core::rollback::patch_store`, `crate::core::orchestration::{agent_run_repository, agent_runs}`, `crate::core::runtime::SessionManager`
//! - External: `serde`, `serde_json`, `ts_rs`

use crate::core::orchestration::agent_runs::AgentRun;
use crate::core::rollback::patch_store;
use crate::core::runtime::SessionManager;
use crate::core::session;
use crate::infra::types::models::{Content, ContentBlock, InterruptKind, Message, MessageSource};
use serde::Serialize;
use std::collections::HashMap;

#[derive(Clone)]
struct RollbackInfo {
    checkpoint_id: String,
    has_file_edits: bool,
    created_at: u64,
}

/// 一条**可渲染用户消息**的附加信息（编号 / 撤回入口，不装正文）。
struct UserDisplayMessage {
    message_id: Option<String>,
    seq: Option<usize>,
    rollback_info: Option<RollbackInfo>,
}

struct RollbackLookups {
    /// 消息自身就是"本轮有文件改动"的轮次 → 直接命中。
    by_message_id: HashMap<String, RollbackInfo>,
    /// 按触发消息的**全局 seq** 升序排列。
    by_seq: Vec<(usize, RollbackInfo)>,
}

// ═══════════════════════════════════════════════════════════════
//  结构化消息视图类型（供前端 Vue 组件渲染与 ts_bindings 导出）
// ═══════════════════════════════════════════════════════════════

#[derive(Serialize, Clone, ts_rs::TS)]
#[serde(rename_all = "camelCase")]
#[ts(rename_all = "camelCase")]
pub struct SessionMessage {
    pub role: String,
    pub id: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub snapshot: Option<AgentTurnSnapshot>,
    #[serde(skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub snapshot_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub message_id: Option<String>,
    // 用户消息的原始 HTML 内容
    #[serde(skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub user_content: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub rollback_checkpoint_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub rollback_mode: Option<String>,
}

#[derive(Serialize, Clone, Default, ts_rs::TS)]
#[serde(rename_all = "camelCase")]
#[ts(rename = "HistoryTurnTokens", rename_all = "camelCase")]
pub struct AgentTurnTokens {
    #[ts(type = "number")]
    input: u64,
    #[ts(type = "number")]
    output: u64,
}

#[derive(Serialize, Clone, Default, ts_rs::TS)]
#[serde(rename_all = "camelCase")]
#[ts(rename = "HistoryTurnSnapshot", rename_all = "camelCase")]
pub struct AgentTurnSnapshot {
    version: u32,
    status: String,
    text_blocks: Vec<AgentTextBlock>,
    thinking_blocks: Vec<AgentThinkingBlock>,
    tool_calls: Vec<AgentToolCallView>,
    logs: Vec<AgentExecutionLog>,
    #[ts(optional = nullable)]
    tokens: Option<AgentTurnTokens>,
    /// 气泡下方的小字说明 —— 只给 **i18n key**，文案由前端出。
    ///
    /// 与 `text_blocks` 的区别是展示位置：notice 渲染在回复气泡**之外**的下方，
    /// 属于状态标注；而 text_blocks 是模型正文，渲染在气泡内。
    /// 中断/取消这类"运行状态"信息一律走 notice，避免混进正文显得突兀。
    ///
    /// 注意：这里曾经是 `notice: Option<String>`、装后端拼好的**中文句子** ——
    /// 界面切英文后中断轮次仍冒中文。现在后端只发 key（如 `notice.userCancel`），
    /// 文案统一由前端语言包提供，与运行期 `JarvisResult::notice_i18n_key` 同源。
    #[serde(skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    notice_i18n_key: Option<String>,
    /// 本轮**净执行用时**（毫秒）：run 的 `started_at → finished_at`，再扣掉等待用户决策的时间。
    ///
    /// `None` = 拿不到可信口径（旧数据没有 run 关联、或该轮尚未收尾）。
    /// 前端只在有值时显示「用时 N 秒」，不再自己按条目时间戳推算——
    /// 那种推算会因「文本块时间戳 = 块开始时刻」而把最后一段长回答整段漏掉。
    #[serde(skip_serializing_if = "Option::is_none")]
    #[ts(optional, type = "number | null")]
    duration_ms: Option<u64>,
    #[ts(type = "number")]
    created_at: u64,
}

#[derive(Serialize, Clone, ts_rs::TS)]
#[serde(rename_all = "camelCase")]
#[ts(rename = "HistoryTextBlock", rename_all = "camelCase")]
pub struct AgentTextBlock {
    id: String,
    #[serde(rename = "loop")]
    #[ts(rename = "loop")]
    loop_: u32,
    kind: String,
    content: String,
    status: String,
    #[ts(type = "number")]
    timestamp: u64,
}

#[derive(Serialize, Clone, ts_rs::TS)]
#[serde(rename_all = "camelCase")]
#[ts(rename = "HistoryThinkingBlock", rename_all = "camelCase")]
pub struct AgentThinkingBlock {
    id: String,
    #[serde(rename = "loop")]
    #[ts(rename = "loop")]
    loop_: u32,
    content: String,
    status: String,
    #[ts(type = "number")]
    timestamp: u64,
}

#[derive(Serialize, Clone, ts_rs::TS)]
#[serde(rename_all = "camelCase")]
#[ts(rename = "HistoryToolCallView", rename_all = "camelCase")]
pub struct AgentToolCallView {
    id: String,
    #[serde(rename = "loop")]
    #[ts(rename = "loop")]
    loop_: u32,
    name: String,
    status: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    input: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    output: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    error: Option<String>,
    logs: Vec<String>,
    #[ts(type = "number")]
    timestamp: u64,
    #[ts(type = "number")]
    updated_at: u64,
}

#[derive(Serialize, Clone, ts_rs::TS)]
#[serde(rename_all = "camelCase")]
#[ts(rename = "HistoryExecutionLog", rename_all = "camelCase")]
pub struct AgentExecutionLog {
    id: String,
    #[serde(rename = "loop")]
    #[ts(rename = "loop")]
    loop_: u32,
    content: String,
    #[ts(type = "number")]
    timestamp: u64,
}

/// 分页元数据
#[derive(Serialize, Clone, ts_rs::TS)]
#[serde(rename_all = "camelCase")]
#[ts(rename_all = "camelCase")]
pub struct PagedSessionMessages {
    pub messages: Vec<SessionMessage>,
    /// 是否还有更早的历史可加载（向上翻页游标）
    pub has_more: bool,
    /// 本页最老消息的 seq（下次请求的游标）；页为空时为 None
    #[ts(optional, type = "number | null")]
    pub oldest_seq: Option<i64>,
    /// 本页最新消息的 seq；页为空时为 None。
    ///
    /// 与 `oldest_seq` 的区别：`oldest_seq` 是**上滑游标**（向更早翻页），
    /// `newest_seq` 是**本页窗口的最新边界**（首屏时即会话最新端；向前翻页时
    /// 仍是那一页的尾部，不是会话最新端）。前端用它与后端 `total_count` 比较，
    /// 判断本地窗口是否已经落后于数据库（stale）。
    #[ts(optional, type = "number | null")]
    pub newest_seq: Option<i64>,
    /// 该会话**可见消息**的总条数（与分页查询同一套可见性条件）。
    ///
    /// 注意口径是「消息条数」而不是「轮数」：`limit` 是轮数，一页可能装几十条消息。
    /// 空会话为 0。
    #[ts(type = "number")]
    pub total_count: i64,
}

impl AgentTurnSnapshot {
    fn is_empty(&self) -> bool {
        self.text_blocks.is_empty()
            && self.thinking_blocks.is_empty()
            && self.tool_calls.is_empty()
            && self
                .notice_i18n_key
                .as_deref()
                .map(|n| n.trim().is_empty())
                .unwrap_or(true)
    }
}

/// 提取用户消息的展示文本（自动排除 tool_result 载体消息）。
fn user_display_content(content: &Content) -> String {
    if content_carries_tool_result(content) {
        return String::new();
    }
    match content {
        Content::Single(s) => s.trim().to_string(),
        Content::Multiple(blocks) => {
            let mut parts = String::new();
            for block in blocks {
                match block {
                    ContentBlock::Text { text } => {
                        let t = text.trim();
                        if !t.is_empty() {
                            parts.push_str(t);
                            parts.push('\n');
                        }
                    }
                    ContentBlock::Image { source } => {
                        let data = if !source.data.is_empty() {
                            source.data.clone()
                        } else if let Some(ref fp) = source.file_path {
                            session::load_image_data(fp).unwrap_or_default()
                        } else {
                            String::new()
                        };
                        parts.push_str(&format!(
                            "<img src=\"data:{};base64,{}\" style=\"max-width: 200px; max-height: 200px; border-radius: 8px; margin: 4px 4px 4px 0; display: inline-block; vertical-align: middle;\" alt=\"图片\" />",
                            source.media_type, data
                        ));
                        parts.push('\n');
                    }
                    _ => {}
                }
            }
            parts.trim_end().to_string()
        }
    }
}

/// 该 user 消息是否携带 `tool_result`（即工具结果载体，而非用户输入）
fn content_carries_tool_result(content: &Content) -> bool {
    match content {
        Content::Multiple(blocks) => blocks
            .iter()
            .any(|b| matches!(b, ContentBlock::ToolResult { .. })),
        Content::Single(_) => false,
    }
}

/// 把一轮对应的所有 run 合并成**净执行**毫秒数（扣掉等待用户决策的时间）。
fn aggregate_run_duration(runs: &[AgentRun]) -> Option<u64> {
    if runs.is_empty() {
        return None;
    }
    let mut started = u64::MAX;
    let mut finished = 0u64;
    let mut wait_ms = 0u64;
    for run in runs {
        let end = run.finished_at?;
        started = started.min(run.started_at);
        finished = finished.max(end);
        wait_ms = wait_ms.saturating_add(run.wait_ms.unwrap_or(0));
    }
    if started == u64::MAX {
        return None;
    }
    Some(finished.saturating_sub(started).saturating_sub(wait_ms))
}

fn run_duration_ms(message_id: Option<&str>) -> Option<u64> {
    let id = message_id?;
    let runs =
        crate::core::orchestration::agent_run_repository::list_runs_by_message_id(id).ok()?;
    aggregate_run_duration(&runs)
}

fn append_assistant_content(
    target: &mut AgentTurnSnapshot,
    content: &Content,
    loop_idx: u32,
    timestamp: u64,
) {
    match content {
        Content::Single(s) => {
            let trimmed = s.trim();
            if !trimmed.is_empty() {
                target.text_blocks.push(AgentTextBlock {
                    id: format!("text_{}", timestamp),
                    loop_: loop_idx,
                    kind: "assistant".to_string(),
                    content: trimmed.to_string(),
                    status: "done".to_string(),
                    timestamp,
                });
            }
        }
        Content::Multiple(blocks) => {
            for (i, block) in blocks.iter().enumerate() {
                let ts = timestamp + i as u64;
                match block {
                    ContentBlock::Text { text } => {
                        let trimmed = text.trim();
                        if !trimmed.is_empty() {
                            target.text_blocks.push(AgentTextBlock {
                                id: format!("text_{}", ts),
                                loop_: loop_idx,
                                kind: "assistant".to_string(),
                                content: trimmed.to_string(),
                                status: "done".to_string(),
                                timestamp: ts,
                            });
                        }
                    }
                    ContentBlock::Thinking { thinking, .. } => {
                        let trimmed = thinking.trim();
                        if !trimmed.is_empty() {
                            target.thinking_blocks.push(AgentThinkingBlock {
                                id: format!("thinking_{}", ts),
                                loop_: loop_idx,
                                content: trimmed.to_string(),
                                status: "done".to_string(),
                                timestamp: ts,
                            });
                        }
                    }
                    ContentBlock::ToolUse { id, name, input } => {
                        let input_json = serde_json::to_string_pretty(input).unwrap_or_default();
                        target.tool_calls.push(AgentToolCallView {
                            id: id.clone(),
                            loop_: loop_idx,
                            name: name.clone(),
                            status: "running".to_string(),
                            input: Some(input_json),
                            output: None,
                            error: None,
                            logs: vec![],
                            timestamp: ts,
                            updated_at: ts,
                        });
                    }
                    _ => {}
                }
            }
        }
    }
}

fn append_tool_result(
    target: &mut AgentTurnSnapshot,
    tool_use_id: &str,
    content: &str,
    timestamp: u64,
) {
    if let Some(tool) = target.tool_calls.iter_mut().find(|t| t.id == tool_use_id) {
        tool.status = "completed".to_string();
        tool.output = Some(content.trim().to_string());
        tool.updated_at = timestamp;
    }
}

fn build_linked_rollbacks(session_id: &str) -> RollbackLookups {
    let mut by_seq = Vec::new();
    let mut by_message_id = HashMap::new();
    for link in patch_store::list_checkpoint_user_message_links(session_id)
        .unwrap_or_default()
        .into_iter()
        .filter(|link| link.has_file_edits && !link.checkpoint_id.is_empty())
    {
        let info = RollbackInfo {
            checkpoint_id: link.checkpoint_id,
            has_file_edits: true,
            created_at: link.created_at,
        };
        by_message_id.insert(link.message_id.clone(), info.clone());
        if let Some(seq) = link.message_seq {
            by_seq.push((seq, info));
        }
    }
    by_seq.sort_by_key(|(seq, info)| (*seq, info.created_at));
    RollbackLookups {
        by_message_id,
        by_seq,
    }
}

fn find_rollback_info(
    linked_rollbacks: &RollbackLookups,
    message_seq: Option<usize>,
    message_id: Option<&str>,
) -> Option<RollbackInfo> {
    if let Some(message_id) = message_id {
        if let Some(info) = linked_rollbacks.by_message_id.get(message_id) {
            return Some(info.clone());
        }
    }
    let message_seq = message_seq?;
    linked_rollbacks
        .by_seq
        .iter()
        .find(|(trigger_seq, _)| *trigger_seq > message_seq)
        .map(|(_, info)| info.clone())
}

fn is_interrupted_source(source: MessageSource) -> bool {
    source == MessageSource::Interrupted
}

fn interrupted_body_is_content(kind: Option<&str>) -> bool {
    kind.is_some()
}

fn interrupt_notice_key(kind: Option<&str>) -> Option<&'static str> {
    match InterruptKind::from_db(kind?)? {
        InterruptKind::StreamTimeout => Some("notice.streamTimeout"),
        InterruptKind::UserCancel => Some("notice.userCancel"),
        InterruptKind::PipelineError => Some("notice.pipelineError"),
        InterruptKind::AppClosed => Some("notice.appClosed"),
        InterruptKind::LoopLimit => Some("notice.loopLimit"),
        InterruptKind::PlanLimit => None,
    }
}

fn is_renderable_source(source: MessageSource) -> bool {
    source.rendered_in_ui()
}

/// 同步投影结果（在 Tokio 阻塞线程池内一次性完成 DB 读取、中断恢复与视图组装）。
struct BlockingMessageProjection {
    recovered: bool,
    memory: crate::infra::types::models::SessionMemory,
    messages: Vec<SessionMessage>,
    has_more: bool,
    oldest_seq: Option<i64>,
    newest_seq: Option<i64>,
    total_count: i64,
}

/// 在阻塞线程池执行的纯同步消息投影核心：合并加载、恢复、分页、附件编码与 run 时长回查，
/// 避免这些同步 I/O 与序列化计算占据 Tokio 异步工作线程。
fn extract_session_messages_window_blocking(
    session_id: &str,
    active_run_id: Option<&str>,
    before_seq: Option<i64>,
    max_turns: Option<usize>,
    include_total_count: bool,
) -> Result<BlockingMessageProjection, String> {
    let paged = max_turns.is_some();
    let max_turns = max_turns.unwrap_or(5).max(1);
    let mut memory = session::load_session(session_id)?;
    let recovered =
        session::lifecycle::ensure_session_recovered(session_id, &mut memory, active_run_id);

    if memory.messages.is_empty() && session::session_messages_count(session_id).unwrap_or(0) == 0 {
        return Ok(BlockingMessageProjection {
            recovered,
            memory,
            messages: Vec::new(),
            has_more: false,
            oldest_seq: None,
            newest_seq: None,
            total_count: 0,
        });
    }

    let linked_rollbacks = build_linked_rollbacks(session_id);

    let (stored_messages, has_more) = if paged {
        let (page, more) = session::load_visible_turns_page(session_id, before_seq, max_turns)?;
        (page, Some(more))
    } else {
        (session::list_visible_session_messages(session_id)?, None)
    };
    let render_messages: Vec<_> = if stored_messages.is_empty() {
        memory
            .messages
            .iter()
            .enumerate()
            .map(|(idx, message)| {
                let source = memory
                    .sources
                    .get(idx)
                    .copied()
                    .unwrap_or(MessageSource::Chat);
                let kind = memory.interrupt_kinds.get(idx).cloned().flatten();
                (
                    idx,
                    memory.message_ids.get(idx).cloned(),
                    None,
                    message.clone(),
                    source,
                    kind,
                )
            })
            .collect()
    } else {
        stored_messages
            .into_iter()
            .enumerate()
            .map(|(idx, stored)| {
                (
                    idx,
                    Some(stored.message_id),
                    Some(stored.seq),
                    stored.content,
                    stored.source,
                    stored.interrupt_kind,
                )
            })
            .collect()
    };

    let oldest_seq = render_messages
        .first()
        .and_then(|(_, _, seq, _, _, _)| seq.map(|s| s as i64));
    let newest_seq = render_messages
        .last()
        .and_then(|(_, _, seq, _, _, _)| seq.map(|s| s as i64));

    let display_messages = render_messages
        .iter()
        .filter_map(|(_idx, message_id, seq, msg, source, _kind)| {
            if let Message::User { content } = msg {
                let display = user_display_content(content);
                if is_renderable_source(*source) && !display.trim().is_empty() {
                    return Some(UserDisplayMessage {
                        message_id: message_id.clone(),
                        seq: *seq,
                        rollback_info: find_rollback_info(
                            &linked_rollbacks,
                            *seq,
                            message_id.as_deref(),
                        ),
                    });
                }
            }
            None
        })
        .collect::<Vec<_>>();

    let mut result = Vec::new();
    let mut pending_assistant = AgentTurnSnapshot::default();
    let mut visible_user_index = 0usize;
    let mut loop_idx = 1;
    let mut current_ts = 1000u64;
    let mut last_seen_seq: Option<usize> = None;
    let mut pending_run_message_id: Option<String> = None;

    for (_, _, seq, msg, source, kind) in &render_messages {
        current_ts += 1;
        last_seen_seq = *seq;
        match msg {
            Message::User { content } => {
                let display = user_display_content(content);

                if let Content::Multiple(blocks) = content {
                    for block in blocks {
                        if let ContentBlock::ToolResult {
                            tool_use_id,
                            content: res_content,
                        } = block
                        {
                            append_tool_result(
                                &mut pending_assistant,
                                tool_use_id,
                                res_content,
                                current_ts,
                            );
                        }
                    }
                }

                if !is_renderable_source(*source) || display.trim().is_empty() {
                    continue;
                }
                let Some(message) = display_messages.get(visible_user_index) else {
                    continue;
                };

                if !pending_assistant.is_empty() {
                    pending_assistant.status = "FINISH".to_string();
                    pending_assistant.version = 1;
                    pending_assistant.duration_ms =
                        run_duration_ms(pending_run_message_id.as_deref());
                    if pending_assistant.created_at == 0 {
                        pending_assistant.created_at = current_ts;
                    }
                    result.push(SessionMessage {
                        role: "agent".to_string(),
                        id: format!(
                            "agent_{}",
                            last_seen_seq
                                .map(|s| s.to_string())
                                .unwrap_or_else(|| format!("m{}", result.len()))
                        ),
                        snapshot: Some(pending_assistant.clone()),
                        snapshot_id: None,
                        message_id: None,
                        user_content: None,
                        rollback_checkpoint_id: None,
                        rollback_mode: None,
                    });
                    pending_assistant = AgentTurnSnapshot::default();
                }

                let rollback_mode = if message
                    .rollback_info
                    .as_ref()
                    .map(|info| info.has_file_edits)
                    .unwrap_or(false)
                {
                    Some("both".to_string())
                } else {
                    Some("session".to_string())
                };
                let rollback_checkpoint_id = message
                    .rollback_info
                    .as_ref()
                    .map(|info| info.checkpoint_id.clone());

                result.push(SessionMessage {
                    role: "user".to_string(),
                    id: format!(
                        "user_{}",
                        message
                            .seq
                            .map(|s| s.to_string())
                            .unwrap_or_else(|| format!("m{}", result.len()))
                    ),
                    snapshot: None,
                    snapshot_id: None,
                    message_id: message.message_id.clone(),
                    user_content: Some(display),
                    rollback_checkpoint_id,
                    rollback_mode,
                });

                visible_user_index += 1;
                loop_idx = 1;
                pending_run_message_id = message.message_id.clone();
            }
            Message::Assistant { content } => {
                if !is_renderable_source(*source) {
                    continue;
                }
                if is_interrupted_source(*source) {
                    if let Some(key) = interrupt_notice_key(kind.as_deref()) {
                        pending_assistant.notice_i18n_key = Some(key.to_string());
                    }
                    if !interrupted_body_is_content(kind.as_deref()) {
                        continue;
                    }
                }
                append_assistant_content(&mut pending_assistant, content, loop_idx, current_ts);
                loop_idx += 1;
            }
        }
    }

    if !pending_assistant.is_empty() {
        pending_assistant.status = "FINISH".to_string();
        pending_assistant.version = 1;
        pending_assistant.duration_ms = run_duration_ms(pending_run_message_id.as_deref());
        if pending_assistant.created_at == 0 {
            pending_assistant.created_at = current_ts;
        }
        result.push(SessionMessage {
            role: "agent".to_string(),
            id: format!(
                "agent_{}",
                last_seen_seq
                    .map(|s| s.to_string())
                    .unwrap_or_else(|| format!("m{}", result.len()))
            ),
            snapshot: Some(pending_assistant),
            snapshot_id: None,
            message_id: None,
            user_content: None,
            rollback_checkpoint_id: None,
            rollback_mode: None,
        });
    }

    let total_count = if include_total_count {
        session::session_message_stats(session_id)
            .map(|(count, _)| count)
            .unwrap_or(0)
    } else {
        0
    };

    Ok(BlockingMessageProjection {
        recovered,
        memory,
        messages: result,
        has_more: has_more.unwrap_or(false),
        oldest_seq,
        newest_seq,
        total_count,
    })
}

/// 懒加载核心：短锁获取活跃 run 状态，将同步 DB 与投影卸载到 `spawn_blocking`，再按需回写内存。
async fn extract_session_messages_window(
    session_id: &str,
    session_manager: &SessionManager,
    before_seq: Option<i64>,
    max_turns: Option<usize>,
    include_total_count: bool,
    log_source: &'static str,
) -> Result<(Vec<SessionMessage>, bool, Option<i64>, Option<i64>, i64), String> {
    let ctx = session_manager.get_or_create(session_id).await;
    let active_run_id = ctx.active_run_id.lock().await.clone();

    let sid = session_id.to_string();
    let active_run_id_for_blocking = active_run_id.clone();
    let projection = tokio::task::spawn_blocking(move || {
        extract_session_messages_window_blocking(
            &sid,
            active_run_id_for_blocking.as_deref(),
            before_seq,
            max_turns,
            include_total_count,
        )
    })
    .await
    .map_err(|e| format!("会话消息投影后台任务异常: {}", e))??;

    if projection.recovered {
        println!(
            "[JARVIS] {}：中断恢复已补回消息并落库（session {}）",
            log_source, session_id
        );
    }

    // 仅在会话无活跃执行或刚刚完成崩溃恢复时回写内存，避免流式执行中途被旧落库状态覆盖
    if projection.recovered || active_run_id.is_none() {
        *ctx.memory.lock().await = projection.memory;
    }

    Ok((
        projection.messages,
        projection.has_more,
        projection.oldest_seq,
        projection.newest_seq,
        projection.total_count,
    ))
}

/// 全量结构化消息投影（内置中断恢复前置）。
pub async fn load_session_messages(
    session_id: &str,
    session_manager: &SessionManager,
) -> Result<Vec<SessionMessage>, String> {
    let (messages, _, _, _, _) = extract_session_messages_window(
        session_id,
        session_manager,
        None,
        None,
        false,
        "get_session_messages",
    )
    .await?;
    Ok(messages)
}

/// 按轮游标分页投影（内置中断恢复前置与总条数统计）。
pub async fn load_session_messages_paged(
    session_id: &str,
    before_seq: Option<i64>,
    limit: Option<usize>,
    session_manager: &SessionManager,
) -> Result<PagedSessionMessages, String> {
    let (messages, has_more, oldest_seq, newest_seq, total_count) =
        extract_session_messages_window(
            session_id,
            session_manager,
            before_seq,
            limit,
            true,
            "get_session_messages_paged",
        )
        .await?;
    Ok(PagedSessionMessages {
        messages,
        has_more,
        oldest_seq,
        newest_seq,
        total_count,
    })
}

#[cfg(test)]
mod duration_tests {
    use super::aggregate_run_duration;
    use crate::core::orchestration::agent_runs::{AgentRun, AgentRunStatus};

    fn run(started: u64, finished: Option<u64>, wait: Option<u64>) -> AgentRun {
        AgentRun {
            run_id: format!("ar_{started}"),
            session_id: "s1".to_string(),
            status: AgentRunStatus::Completed,
            loop_count: 1,
            input_tokens: 0,
            output_tokens: 0,
            started_at: started,
            updated_at: finished.unwrap_or(started),
            finished_at: finished,
            last_safe_point: None,
            summary: None,
            resumable: false,
            resumed_from_run_id: None,
            message_id: Some("m1".to_string()),
            interrupt_kind: None,
            wait_ms: wait,
        }
    }

    #[test]
    fn empty_runs_yield_none() {
        assert_eq!(aggregate_run_duration(&[]), None);
    }

    #[test]
    fn unfinished_run_yields_none() {
        assert_eq!(aggregate_run_duration(&[run(1_000, None, Some(0))]), None);
        assert_eq!(
            aggregate_run_duration(&[run(1_000, Some(2_000), Some(0)), run(2_100, None, None)]),
            None
        );
    }

    #[test]
    fn wait_is_subtracted() {
        assert_eq!(
            aggregate_run_duration(&[run(1_000, Some(11_000), Some(4_000))]),
            Some(6_000)
        );
        assert_eq!(
            aggregate_run_duration(&[run(1_000, Some(11_000), None)]),
            Some(10_000)
        );
    }

    #[test]
    fn resumed_runs_span_from_earliest_to_latest() {
        let runs = vec![
            run(1_000, Some(5_000), Some(0)),
            run(6_000, Some(12_000), Some(1_000)),
        ];
        assert_eq!(aggregate_run_duration(&runs), Some(10_000));
    }

    #[test]
    fn wait_larger_than_span_saturates_to_zero() {
        assert_eq!(
            aggregate_run_duration(&[run(1_000, Some(2_000), Some(5_000))]),
            Some(0)
        );
    }
}

#[cfg(test)]
mod tool_result_carrier_tests {
    use super::user_display_content;
    use crate::infra::types::models::{Content, ContentBlock, ImageSource};

    fn image_block() -> ContentBlock {
        ContentBlock::Image {
            source: ImageSource {
                r#type: "base64".to_string(),
                media_type: "image/jpeg".to_string(),
                data: String::new(),
                file_path: Some("s1_abcd.jpg".to_string()),
            },
        }
    }

    #[test]
    fn tool_result_with_image_is_not_a_user_message() {
        let content = Content::Multiple(vec![
            ContentBlock::ToolResult {
                tool_use_id: "call_1".to_string(),
                content: "已重新加载图片".to_string(),
            },
            image_block(),
        ]);
        assert!(user_display_content(&content).trim().is_empty());
    }

    #[test]
    fn real_user_image_is_still_displayed() {
        let content = Content::Multiple(vec![
            ContentBlock::Text {
                text: "这是谁".to_string(),
            },
            image_block(),
        ]);
        let shown = user_display_content(&content);
        assert!(shown.contains("这是谁"));
        assert!(shown.contains("<img"));
    }
}

#[cfg(test)]
mod renderable_source_tests {
    use super::is_renderable_source;
    use crate::infra::types::models::MessageSource;

    #[test]
    fn chat_is_renderable() {
        assert!(is_renderable_source(MessageSource::Chat));
    }

    #[test]
    fn interrupted_is_renderable() {
        assert!(is_renderable_source(MessageSource::Interrupted));
    }

    #[test]
    fn other_sources_stay_hidden() {
        for src in [
            MessageSource::Compact,
            MessageSource::Inject,
            MessageSource::Placeholder,
        ] {
            assert!(!is_renderable_source(src), "{:?} 不应参与界面渲染", src);
        }
    }
}

#[cfg(test)]
mod notice_not_empty_tests {
    use super::AgentTurnSnapshot;

    fn snapshot_with_notice(key: Option<&str>) -> AgentTurnSnapshot {
        AgentTurnSnapshot {
            notice_i18n_key: key.map(|s| s.to_string()),
            ..Default::default()
        }
    }

    #[test]
    fn notice_only_is_not_empty() {
        assert!(!snapshot_with_notice(Some("notice.streamTimeout")).is_empty());
    }

    #[test]
    fn blank_notice_is_still_empty() {
        assert!(snapshot_with_notice(Some("   ")).is_empty());
        assert!(snapshot_with_notice(None).is_empty());
    }
}

#[cfg(test)]
mod interrupted_body_tests {
    use super::interrupted_body_is_content;

    #[test]
    fn interrupt_with_kind_renders_body() {
        assert!(interrupted_body_is_content(Some("user_cancel")));
        assert!(interrupted_body_is_content(Some("stream_timeout")));
        assert!(interrupted_body_is_content(Some("app_closed")));
    }

    #[test]
    fn legacy_marker_body_stays_hidden() {
        assert!(!interrupted_body_is_content(None));
    }
}
