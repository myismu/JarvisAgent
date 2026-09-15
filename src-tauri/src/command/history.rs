//! # history.rs — 会话历史渲染 Tauri 命令
//!
//! 将会话消息历史渲染为 HTML 格式，供前端展示。
//! 处理用户消息（含图片 base64 内联）、助手消息（思考过程折叠显示），
//! 并关联检查点信息以支持消息撤回按钮。
//!
//! ## 关键导出
//! - `get_session_history()`: 返回会话历史的 HTML 渲染结果
//! - `get_session_messages()`: 返回会话历史的结构化 JSON，供前端 Vue 组件渲染
//!
//! ## 约束
//! - 过滤内部消息（background-results 通知、内部 ack 回复）
//! - 助手多轮回复合并显示，思考过程用 `<details>` 折叠
//! - 用户消息关联检查点 ID，支持前端回滚按钮

use crate::infra::types::models::*;
use crate::core::orchestration::agent_runs;
use crate::core::session;
use crate::infra::state::state::*;
use std::collections::HashMap;

#[derive(Clone)]
struct RollbackInfo {
    checkpoint_id: String,
    has_file_edits: bool,
    created_at: u64,
}

struct UserDisplayMessage {
    memory_index: usize,
    message_id: Option<String>,
    seq: Option<usize>,
    display: String,
    rollback_info: Option<RollbackInfo>,
}

struct RollbackLookups {
    by_index: Vec<(usize, RollbackInfo)>,
    by_message_id: HashMap<String, RollbackInfo>,
}

use serde::Serialize;

// ═══════════════════════════════════════════════════════════════
//  新增：结构化消息类型（供前端 Vue 组件渲染）
// ═══════════════════════════════════════════════════════════════

#[derive(Serialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct SessionMessage {
    pub role: String,
    pub id: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub snapshot: Option<AgentTurnSnapshot>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub snapshot_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub message_id: Option<String>,
    // 用户消息的原始 HTML 内容
    #[serde(skip_serializing_if = "Option::is_none")]
    pub user_content: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub rollback_checkpoint_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub rollback_mode: Option<String>,
}

#[derive(Serialize, Clone, Default)]
#[serde(rename_all = "camelCase")]
pub struct AgentTurnTokens {
    input: u64,
    output: u64,
}

#[derive(Serialize, Clone, Default)]
#[serde(rename_all = "camelCase")]
pub struct AgentTurnSnapshot {
    version: u32,
    status: String,
    text_blocks: Vec<AgentTextBlock>,
    thinking_blocks: Vec<AgentThinkingBlock>,
    tool_calls: Vec<AgentToolCallView>,
    logs: Vec<AgentExecutionLog>,
    tokens: Option<AgentTurnTokens>,
    /// 气泡下方的小字说明（如"以上为部分结果""回复被中断"）。
    ///
    /// 与 `text_blocks` 的区别是展示位置：notice 渲染在回复气泡**之外**的下方，
    /// 属于状态标注；而 text_blocks 是模型正文，渲染在气泡内。
    /// 中断/取消这类"运行状态"信息一律走 notice，避免混进正文显得突兀。
    #[serde(skip_serializing_if = "Option::is_none")]
    notice: Option<String>,
    created_at: u64,
}

#[derive(Serialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct AgentTextBlock {
    id: String,
    #[serde(rename = "loop")]
    loop_: u32,
    kind: String,
    content: String,
    status: String,
    timestamp: u64,
}

#[derive(Serialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct AgentThinkingBlock {
    id: String,
    #[serde(rename = "loop")]
    loop_: u32,
    content: String,
    status: String,
    timestamp: u64,
}

#[derive(Serialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct AgentToolCallView {
    id: String,
    #[serde(rename = "loop")]
    loop_: u32,
    name: String,
    status: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    input: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    output: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    error: Option<String>,
    logs: Vec<String>,
    timestamp: u64,
    updated_at: u64,
}

#[derive(Serialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct AgentExecutionLog {
    id: String,
    #[serde(rename = "loop")]
    loop_: u32,
    content: String,
    timestamp: u64,
}

impl AgentTurnSnapshot {
    fn is_empty(&self) -> bool {
        self.text_blocks.is_empty()
            && self.thinking_blocks.is_empty()
            && self.tool_calls.is_empty()
            // notice 也是要展示的内容：只统计正文会让"仅有一条中断说明"的
            // 收尾轮次被判为空而整轮丢弃 —— 实测出现于"中断后没有后续消息"的场景
            // （最后一条就是 interrupted 消息，结果界面什么都不显示）。
            && self
                .notice
                .as_deref()
                .map(|n| n.trim().is_empty())
                .unwrap_or(true)
    }
}

/// 提取用户消息的展示文本。
///
/// 只取真实 Text 块；动态上下文是独立的 Context 块（工作目录 / 项目结构 /
/// 全局记忆），属于发给模型的运行时信息，不进 UI。
fn user_display_content(content: &Content) -> String {
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
    let mut by_index = Vec::new();
    let mut by_message_id = HashMap::new();
    for link in crate::infra::db::list_checkpoint_user_message_links(session_id)
        .unwrap_or_default()
        .into_iter()
        .filter(|link| link.has_file_edits && !link.checkpoint_id.is_empty())
    {
        let info = RollbackInfo {
            checkpoint_id: link.checkpoint_id,
            has_file_edits: true,
            created_at: link.created_at,
        };
        if let Some(message_id) = link.message_id.filter(|value| !value.trim().is_empty()) {
            by_message_id.insert(message_id, info.clone());
        }
        by_index.push((link.user_message_index, info));
    }
    by_index.sort_by_key(|(trigger_index, info)| (*trigger_index, info.created_at));
    RollbackLookups {
        by_index,
        by_message_id,
    }
}

fn parse_snapshot_usize(snapshot: &crate::core::rollback::Snapshot, key: &str) -> Option<usize> {
    snapshot
        .metadata
        .get(key)
        .and_then(|value| value.parse::<usize>().ok())
}

fn parse_snapshot_string(snapshot: &crate::core::rollback::Snapshot, key: &str) -> Option<String> {
    snapshot
        .metadata
        .get(key)
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty())
}

fn find_rollback_info(
    linked_rollbacks: &RollbackLookups,
    metadata_rollbacks_by_index: &[(usize, u64, String)],
    metadata_rollbacks_by_message_id: &HashMap<String, RollbackInfo>,
    memory_index: usize,
    message_id: Option<&str>,
) -> Option<RollbackInfo> {
    if let Some(message_id) = message_id {
        if let Some(info) = linked_rollbacks
            .by_message_id
            .get(message_id)
            .or_else(|| metadata_rollbacks_by_message_id.get(message_id))
        {
            return Some(info.clone());
        }
    }

    let linked = linked_rollbacks
        .by_index
        .iter()
        .filter(|(trigger_index, _)| *trigger_index >= memory_index)
        .min_by_key(|(trigger_index, info)| (*trigger_index, info.created_at))
        .map(|(_, info)| info.clone());
    let metadata = metadata_rollbacks_by_index
        .iter()
        .filter(|(trigger_index, _, _)| *trigger_index >= memory_index)
        .min_by_key(|(trigger_index, created_at, _)| (*trigger_index, *created_at))
        .map(|(_, created_at, id)| RollbackInfo {
            checkpoint_id: id.clone(),
            has_file_edits: true,
            created_at: *created_at,
        });

    match (linked, metadata) {
        (Some(linked), Some(metadata)) if metadata.created_at < linked.created_at => Some(metadata),
        (Some(linked), _) => Some(linked),
        (None, Some(metadata)) => Some(metadata),
        (None, None) => None,
    }
}

/// 渲染用户消息 HTML，撤回按钮由前端统一补齐
fn render_user_message(history: &mut String, message: &UserDisplayMessage) {
    let display = &message.display;
    if display.trim().is_empty() {
        return;
    }

    let rollback_mode = if message
        .rollback_info
        .as_ref()
        .map(|info| info.has_file_edits)
        .unwrap_or(false)
    {
        "both"
    } else {
        "session"
    };
    let rollback_checkpoint_id = message
        .rollback_info
        .as_ref()
        .map(|info| info.checkpoint_id.as_str())
        .unwrap_or("");

    history.push_str(&format!(
        "<div class=\"chat-message user-message\" style=\"position: relative;\"><div class=\"message-content\" data-user-message-index=\"{}\"{}{} data-rollback-mode=\"{}\" data-rollback-checkpoint-id=\"{}\">\n\n{}\n\n</div></div>\n\n",
        message.memory_index,
        message
            .message_id
            .as_ref()
            .map(|id| format!(" data-message-id=\"{}\"", id))
            .unwrap_or_default(),
        message
            .seq
            .map(|seq| format!(" data-message-seq=\"{}\"", seq))
            .unwrap_or_default(),
        rollback_mode,
        rollback_checkpoint_id,
        display
    ));
}

/// 渲染助手消息 HTML，思考过程用 details 折叠，取最后一段非空文本作为可见回复

/// 判断该 source 是否属于"中断标记消息"（只作状态说明，不是模型正文）。
fn is_interrupted_source(source: &str) -> bool {
    source == "interrupted"
}

/// 从 `interrupted` 消息里取出给用户看的小字说明。
///
/// 写入历史的标记文案形如 `> ⚠️ **[回复被中断]** …`，早期还带过 `> ✕ **用户已取消执行…**`。
/// 这里剥掉 Markdown 引用符号与加粗，只留纯文本，交给前端渲染成气泡下方的小字。
fn interrupted_notice_text(content: &Content) -> Option<String> {
    let raw = match content {
        Content::Single(s) => s.as_str(),
        Content::Multiple(blocks) => blocks.iter().find_map(|b| match b {
            ContentBlock::Text { text } => Some(text.as_str()),
            _ => None,
        })?,
    };
    let cleaned = raw
        .trim()
        .trim_start_matches('>')
        .trim()
        .replace("**", "")
        .replace("⚠️", "⚠")
        .trim()
        .to_string();
    if cleaned.is_empty() {
        None
    } else {
        Some(cleaned)
    }
}

fn render_assistant_message(history: &mut String, assistant: &mut AgentTurnSnapshot) {
    if assistant.is_empty() {
        return;
    }

    assistant.status = "FINISH".to_string();
    assistant.version = 1;
    if assistant.created_at == 0 {
        assistant.created_at = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_millis() as u64;
    }

    let json_data = serde_json::to_string(assistant)
        .unwrap_or_default()
        .replace('<', "\\u003c");

    // Fetch the final visible text for fallback
    let final_text = assistant
        .text_blocks
        .last()
        .map(|b| b.content.as_str())
        .unwrap_or("");
    let visible_text = if final_text.is_empty() {
        assistant
            .thinking_blocks
            .last()
            .map(|b| b.content.as_str())
            .unwrap_or("")
    } else {
        final_text
    };

    history
        .push_str("<div class=\"chat-message agent-message\"><div class=\"message-content current-turn-content\">

");

    history.push_str(&format!(
        "<script type=\"application/json\" class=\"agent-turn-data\">{}</script>
",
        json_data
    ));

    // Fallback rendering
    if !assistant.thinking_blocks.is_empty() {
        let thinking_all = assistant
            .thinking_blocks
            .iter()
            .map(|b| b.content.as_str())
            .collect::<Vec<_>>()
            .join(
                "

",
            );
        history.push_str(&format!(
            "

<details><summary><svg viewBox=\"0 0 24 24\" width=\"14\" height=\"14\" stroke=\"currentColor\" stroke-width=\"2\" fill=\"none\" stroke-linecap=\"round\" stroke-linejoin=\"round\" style=\"vertical-align: text-bottom; margin-right: 4px;\"><circle cx=\"12\" cy=\"12\" r=\"3\"></circle><path d=\"M12 2v3\"></path><path d=\"M12 19v3\"></path><path d=\"M4.93 4.93l2.12 2.12\"></path><path d=\"M16.95 16.95l2.12 2.12\"></path><path d=\"M2 12h3\"></path><path d=\"M19 12h3\"></path><path d=\"M4.93 19.07l2.12-2.12\"></path><path d=\"M16.95 7.05l2.12-2.12\"></path></svg> 贾维斯已完成思考与操作（点击查看完整决策链）</summary>

{}

</details>

",
            thinking_all
        ));
    }

    if !visible_text.is_empty() {
        history.push_str(visible_text);
    }

    if let Some(tokens) = &assistant.tokens {
        history.push_str(&format!(
            "\n\n<div class=\"token-usage\"><b>本次消耗</b>: 输入 {} / 输出 {} Token</div>",
            tokens.input, tokens.output
        ));
    }

    history.push_str("\n\n</div></div>\n\n");
}

/// 该 source 的消息是否参与会话历史的界面渲染。
///
/// 白名单语义：
/// - `chat`：常规对话（用户输入 / 助手回复）；
/// - `interrupted`：运行被打断（用户取消 / 上游失联 / 执行报错）时的收尾消息。
///   必须渲染，否则中断轮次会从界面消失，用户会以为「根本没执行」。
///
/// 其余 source（`compact` 压缩摘要、`internal` 内部通知、`background` 后台结果、
/// `context` 上下文注入）仍由 SQL 与内容规则过滤，不进界面。
fn is_renderable_source(source: &str) -> bool {
    matches!(source, "chat" | "interrupted")
}

#[tauri::command]
pub async fn get_session_history(
    session_id: String,
    session_manager: tauri::State<'_, SessionManager>,
    registry: tauri::State<'_, SnapshotRegistry>,
) -> Result<String, String> {
    let ctx = session_manager.get_or_create(&session_id).await;
    // 注意：**不要**在这里把内存 flush 到 DB（与 extract_session_messages 同理）。
    // 该 flush 会把内存中已删除的消息复活，"删掉后刷新又回来"即由此产生。
    let mut memory = session::load_session(&session_id)?;
    let _runs = agent_runs::list_runs(Some(&session_id));

    // ── 中断恢复：检测并补回崩溃/中断时丢失的消息 ──
    //
    // 复用 `recover_interrupted_into_memory`（含去重守卫），不要内联重写：
    // 内联副本没有去重，会**每加载一次就多一条合并副本**。
    if crate::command::session::recover_interrupted_into_memory(&session_id, &mut memory) {
        // 将恢复后的内存同步回去，并保存到数据库
        *ctx.memory.lock().await = memory.clone();
        session::save_session(&session_id, &memory, None);

        // 标记该 run 为已恢复，避免下次重复恢复
        if let Some(interrupted_run) = agent_runs::find_interrupted_run(&session_id) {
            let _ = agent_runs::mark_run_recovered(&interrupted_run.run_id);
        }
    } else {
        *ctx.memory.lock().await = memory.clone();
    }

    if memory.messages.is_empty() && session::session_messages_count(&session_id).unwrap_or(0) == 0 {
        return Ok(String::new());
    }

    let linked_rollbacks = build_linked_rollbacks(&session_id);
    let metadata_rollbacks_by_index;
    let metadata_rollbacks_by_message_id;
    {
        let manager = registry.0.read().await.get_or_create(&session_id).await?;
        let mut by_index = Vec::new();
        let mut by_message_id = HashMap::new();
        for snapshot in manager
            .list_snapshots(None)
            .await
            .into_iter()
            .filter(|snapshot| snapshot.is_checkpoint)
        {
            let patch_count = parse_snapshot_usize(&snapshot, "patch_count").unwrap_or(0);
            if patch_count == 0 {
                continue;
            }
            let info = RollbackInfo {
                checkpoint_id: snapshot.id.clone(),
                has_file_edits: true,
                created_at: snapshot.created_at,
            };
            if let Some(message_id) = parse_snapshot_string(&snapshot, "trigger_user_message_id") {
                by_message_id.insert(message_id, info);
            }
            if let Some(trigger_index) = parse_snapshot_usize(&snapshot, "trigger_user_memory_index") {
                by_index.push((trigger_index, snapshot.created_at, snapshot.id));
            }
        }
        by_index.sort_by_key(|(trigger_index, created_at, _)| (*trigger_index, *created_at));
        metadata_rollbacks_by_index = by_index;
        metadata_rollbacks_by_message_id = by_message_id;
    };

    let stored_messages = session::list_visible_session_messages(&session_id)?;
    // stored_messages 和 session_memory 表是同一次 save_session 写入的，
    // 顺序完全一致，直接用 enumerate() 的 idx 作为 memory_index，无需 HashMap 查找
    let render_messages: Vec<_> = if stored_messages.is_empty() {
        memory
            .messages
            .iter()
            .enumerate()
            .map(|(idx, message)| {
                let source = memory.sources.get(idx).cloned().unwrap_or_else(|| "chat".to_string());
                (idx, memory.message_ids.get(idx).cloned(), None, message.clone(), source)
            })
            .collect()
    } else {
        stored_messages
            .into_iter()
            .enumerate()
            .map(|(idx, stored)| {
                (idx, Some(stored.message_id), Some(stored.seq), stored.content, stored.source)
            })
            .collect()
    };

    let display_messages = render_messages
        .iter()
        .filter_map(|(memory_index, message_id, seq, msg, source)| {
            if let Message::User { content } = msg {
                let display = user_display_content(content);
                if is_renderable_source(source) && !display.trim().is_empty() {
                    return Some(UserDisplayMessage {
                        memory_index: *memory_index,
                        message_id: message_id.clone(),
                        seq: *seq,
                        display,
                        rollback_info: find_rollback_info(
                            &linked_rollbacks,
                            &metadata_rollbacks_by_index,
                            &metadata_rollbacks_by_message_id,
                            *memory_index,
                            message_id.as_deref(),
                        ),
                    });
                }
            }
            None
        })
        .collect::<Vec<_>>();

    let mut history = String::new();
    let mut pending_assistant = AgentTurnSnapshot::default();
    let mut visible_user_index = 0usize;
    let mut loop_idx = 1;
    let mut current_ts = 1000u64;

    for (_, _, _, msg, source) in &render_messages {
        current_ts += 1;
        match msg {
            Message::User { content } => {
                let display = user_display_content(content);

                // Process ToolResults inside User messages before checking if we should skip
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

                if !is_renderable_source(source.as_str()) || display.trim().is_empty() {
                    continue;
                }
                let Some(message) = display_messages.get(visible_user_index) else {
                    continue;
                };

                render_assistant_message(&mut history, &mut pending_assistant);
                pending_assistant = AgentTurnSnapshot::default();
                loop_idx = 1;
                render_user_message(&mut history, message);
                visible_user_index += 1;
            }
            Message::Assistant { content } => {
                if !is_renderable_source(source.as_str()) {
                    continue;
                }
                // `interrupted` 消息不是模型正文，而是"运行被打断"的状态说明。
                // 把它挂到本轮快照的 notice 上，由前端渲染在气泡**下方**的小字里；
                // 若混进 text_blocks 会挤进回复气泡内部，既突兀又会与
                // "已保留的部分结果"重复，看起来像模型自己说的话。
                if is_interrupted_source(source.as_str()) {
                    if let Some(notice) = interrupted_notice_text(content) {
                        pending_assistant.notice = Some(notice);
                    }
                    continue;
                }
                append_assistant_content(&mut pending_assistant, content, loop_idx, current_ts);
                loop_idx += 1;
            }
        }
    }

    render_assistant_message(&mut history, &mut pending_assistant);
    Ok(history)
}

// ═══════════════════════════════════════════════════════════════
//  新增：get_session_messages — 返回结构化 JSON 消息列表
// ═══════════════════════════════════════════════════════════════

/// 提取消息列表的公共逻辑（与 get_session_history 共享）
async fn extract_session_messages(
    session_id: &str,
    session_manager: &SessionManager,
    registry: &SnapshotRegistry,
) -> Result<Vec<SessionMessage>, String> {
    let ctx = session_manager.get_or_create(session_id).await;
    // 注意：**不要**在这里把内存 flush 到 DB。
    //
    // 曾经的实现是"先把 ctx.memory 存库，再读回来"，注释说是"确保读到最新状态"，
    // 但实际效果相反：后端进程常驻，内存里可能残留**已被删除**的消息，
    // 这次 flush 会把它们全部复活 —— 实测表现为"手动删掉某条消息后 F5，
    // 它立刻回到数据库"，看起来阴魂不散。
    //
    // 本函数语义是**读取**，应以数据库为准；需要持久化的写入点各自负责落库。
    let mut memory = session::load_session(session_id)?;
    let _runs = agent_runs::list_runs(Some(session_id));

    // 中断恢复
    //
    // 这里**必须**复用 `recover_interrupted_into_memory`，不要内联重写：
    // 该函数含有"半截内容是否已存在"的去重守卫，而它的判定需要处理
    // 「正文 / 中断标记分体存储」与「正文+标记合并」两种形态。
    // 曾在此内联复制过一份无去重的实现，导致**每加载一次就多一条合并副本**
    // （实测"删掉再刷新，副本又回来"，且因未走到守卫分支连日志都不打印）。
    if crate::command::session::recover_interrupted_into_memory(session_id, &mut memory) {
        *ctx.memory.lock().await = memory.clone();
        session::save_session(session_id, &memory, None);
        if let Some(interrupted_run) = agent_runs::find_interrupted_run(session_id) {
            let _ = agent_runs::mark_run_recovered(&interrupted_run.run_id);
        }
    } else {
        *ctx.memory.lock().await = memory.clone();
    }

    if memory.messages.is_empty() && session::session_messages_count(session_id).unwrap_or(0) == 0 {
        return Ok(Vec::new());
    }

    let linked_rollbacks = build_linked_rollbacks(session_id);
    let metadata_rollbacks_by_index;
    let metadata_rollbacks_by_message_id;
    {
        let manager = registry.0.read().await.get_or_create(session_id).await?;
        let mut by_index = Vec::new();
        let mut by_message_id = HashMap::new();
        for snapshot in manager
            .list_snapshots(None)
            .await
            .into_iter()
            .filter(|snapshot| snapshot.is_checkpoint)
        {
            let patch_count = parse_snapshot_usize(&snapshot, "patch_count").unwrap_or(0);
            if patch_count == 0 {
                continue;
            }
            let info = RollbackInfo {
                checkpoint_id: snapshot.id.clone(),
                has_file_edits: true,
                created_at: snapshot.created_at,
            };
            if let Some(message_id) = parse_snapshot_string(&snapshot, "trigger_user_message_id") {
                by_message_id.insert(message_id, info);
            }
            if let Some(trigger_index) = parse_snapshot_usize(&snapshot, "trigger_user_memory_index") {
                by_index.push((trigger_index, snapshot.created_at, snapshot.id));
            }
        }
        by_index.sort_by_key(|(trigger_index, created_at, _)| (*trigger_index, *created_at));
        metadata_rollbacks_by_index = by_index;
        metadata_rollbacks_by_message_id = by_message_id;
    };

    let stored_messages = session::list_visible_session_messages(session_id)?;
    let render_messages: Vec<_> = if stored_messages.is_empty() {
        memory
            .messages
            .iter()
            .enumerate()
            .map(|(idx, message)| {
                let source = memory.sources.get(idx).cloned().unwrap_or_else(|| "chat".to_string());
                (idx, memory.message_ids.get(idx).cloned(), None, message.clone(), source)
            })
            .collect()
    } else {
        stored_messages
            .into_iter()
            .enumerate()
            .map(|(idx, stored)| {
                (idx, Some(stored.message_id), Some(stored.seq), stored.content, stored.source)
            })
            .collect()
    };

    let display_messages = render_messages
        .iter()
        .filter_map(|(memory_index, message_id, seq, msg, source)| {
            if let Message::User { content } = msg {
                let display = user_display_content(content);
                if is_renderable_source(source) && !display.trim().is_empty() {
                    return Some(UserDisplayMessage {
                        memory_index: *memory_index,
                        message_id: message_id.clone(),
                        seq: *seq,
                        display,
                        rollback_info: find_rollback_info(
                            &linked_rollbacks,
                            &metadata_rollbacks_by_index,
                            &metadata_rollbacks_by_message_id,
                            *memory_index,
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

    for (_, _, _, msg, source) in &render_messages {
        current_ts += 1;
        match msg {
            Message::User { content } => {
                let display = user_display_content(content);

                if let Content::Multiple(blocks) = content {
                    for block in blocks {
                        if let ContentBlock::ToolResult { tool_use_id, content: res_content } = block {
                            append_tool_result(&mut pending_assistant, tool_use_id, res_content, current_ts);
                        }
                    }
                }

                if !is_renderable_source(source.as_str()) || display.trim().is_empty() {
                    continue;
                }
                let Some(message) = display_messages.get(visible_user_index) else {
                    continue;
                };

                // 先 finalize 之前的 assistant 消息
                if !pending_assistant.is_empty() {
                    pending_assistant.status = "FINISH".to_string();
                    pending_assistant.version = 1;
                    if pending_assistant.created_at == 0 {
                        pending_assistant.created_at = current_ts;
                    }
                    result.push(SessionMessage {
                        role: "agent".to_string(),
                        id: format!("agent_{}", result.len()),
                        snapshot: Some(pending_assistant.clone()),
                        snapshot_id: None,
                        message_id: None,
                        user_content: None,
                        rollback_checkpoint_id: None,
                        rollback_mode: None,
                    });
                    pending_assistant = AgentTurnSnapshot::default();
                }

                let rollback_mode = if message.rollback_info.as_ref().map(|info| info.has_file_edits).unwrap_or(false) {
                    Some("both".to_string())
                } else {
                    Some("session".to_string())
                };
                let rollback_checkpoint_id = message.rollback_info.as_ref().map(|info| info.checkpoint_id.clone());

                result.push(SessionMessage {
                    role: "user".to_string(),
                    id: format!("user_{}", result.len()),
                    snapshot: None,
                    snapshot_id: None,
                    message_id: message.message_id.clone(),
                    user_content: Some(display),
                    rollback_checkpoint_id,
                    rollback_mode,
                });

                visible_user_index += 1;
                loop_idx = 1;
            }
            Message::Assistant { content } => {
                if !is_renderable_source(source.as_str()) {
                    continue;
                }
                // `interrupted` 消息不是模型正文，而是"运行被打断"的状态说明。
                // 把它挂到本轮快照的 notice 上，由前端渲染在气泡**下方**的小字里；
                // 若混进 text_blocks 会挤进回复气泡内部，既突兀又会与
                // "已保留的部分结果"重复，看起来像模型自己说的话。
                if is_interrupted_source(source.as_str()) {
                    if let Some(notice) = interrupted_notice_text(content) {
                        pending_assistant.notice = Some(notice);
                    }
                    continue;
                }
                append_assistant_content(&mut pending_assistant, content, loop_idx, current_ts);
                loop_idx += 1;
            }
        }
    }

    // finalize 最后一条 assistant 消息
    if !pending_assistant.is_empty() {
        pending_assistant.status = "FINISH".to_string();
        pending_assistant.version = 1;
        if pending_assistant.created_at == 0 {
            pending_assistant.created_at = current_ts;
        }
        result.push(SessionMessage {
            role: "agent".to_string(),
            id: format!("agent_{}", result.len()),
            snapshot: Some(pending_assistant),
            snapshot_id: None,
            message_id: None,
            user_content: None,
            rollback_checkpoint_id: None,
            rollback_mode: None,
        });
    }

    Ok(result)
}

#[tauri::command]
pub async fn get_session_messages(
    session_id: String,
    session_manager: tauri::State<'_, SessionManager>,
    registry: tauri::State<'_, SnapshotRegistry>,
) -> Result<Vec<SessionMessage>, String> {
    extract_session_messages(&session_id, &session_manager, &registry).await
}

#[cfg(test)]
mod renderable_source_tests {
    use super::is_renderable_source;

    /// 常规对话必须渲染
    #[test]
    fn chat_is_renderable() {
        assert!(is_renderable_source("chat"));
    }

    /// 回归防护：中断收尾消息必须渲染。
    /// 否则用户取消/上游失联后，中断轮次会从界面整体消失，
    /// 用户会误以为「根本没执行」——这正是本次要修掉的问题。
    #[test]
    fn interrupted_is_renderable() {
        assert!(is_renderable_source("interrupted"));
    }

    /// 内部消息仍不得泄漏到界面
    #[test]
    fn internal_sources_stay_hidden() {
        for src in ["compact", "internal", "background", "context"] {
            assert!(
                !is_renderable_source(src),
                "{src} 不应参与界面渲染"
            );
        }
    }
}

#[cfg(test)]
mod notice_not_empty_tests {
    //! **实测 bug 的防护**：`is_empty()` 只统计正文块时，
    //! 「仅有一条中断说明」的收尾轮次会被判为空而整轮丢弃。
    //!
    //! 触发场景很常见：中断发生后**没有后续消息**（会话最后一条即 interrupted），
    //! 于是界面什么都不显示、连提示都没有。
    //!
    //! 同一处判定在前端也有一份（`ChatArea.vue hasCurrentTurnContent`），
    //! 两边都必须把 notice 计入 —— 只改一边仍会漏。
    use super::AgentTurnSnapshot;

    fn snapshot_with_notice(notice: Option<&str>) -> AgentTurnSnapshot {
        AgentTurnSnapshot {
            notice: notice.map(|s| s.to_string()),
            ..Default::default()
        }
    }

    /// 只有 notice 时不算空：否则整轮被丢弃，用户看不到任何中断/等待提示
    #[test]
    fn notice_only_is_not_empty() {
        assert!(!snapshot_with_notice(Some("⚠ 上游服务已停止响应")).is_empty());
    }

    /// 空白 notice 仍算空，避免渲染出一个空壳气泡
    #[test]
    fn blank_notice_is_still_empty() {
        assert!(snapshot_with_notice(Some("   ")).is_empty());
        assert!(snapshot_with_notice(None).is_empty());
    }
}
