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

/// 判断该 source 是否属于"中断收尾消息"。
///
/// ⚠️ 别据此断言它"不是模型正文" —— 新数据的正文块里装的恰恰是半截内容，
/// 该不该渲染见 [`interrupted_body_is_content`]。
fn is_interrupted_source(source: &str) -> bool {
    source == "interrupted"
}

/// `interrupted` 消息的正文块是否应当渲染进 `text_blocks`。
///
/// 同一条 `source = "interrupted"` 的消息承载两种截然不同的东西，只有 kind 能区分：
/// - **kind 有值**（新数据）：正文块里是**半截内容**（`pipeline.rs` 中断收尾的分支二
///   写明"正文即半截内容；正文块恒存在"），必须渲染 —— 否则用户取消后只剩一句
///   "用户已取消执行"，已经生成的部分凭空消失（实测 bug）；
/// - **kind 缺失**（旧库数据）：正文是 `**[回复被中断]** …` 这类标记文本，已由
///   notice 承载，再渲染会把它当模型正文重复显示一遍。
///
/// 判据与 notice 的生成路径**对称**：新数据由 kind 驱动，旧数据回退正文清洗。
fn interrupted_body_is_content(kind: Option<&str>) -> bool {
    kind.is_some()
}

/// 从 `interrupted` 消息里取出给用户看的小字说明。
///
/// 标记文案现行格式为 `**[标签]** …`（如 `**[回复被中断]** …`，不带引用符）；
/// 旧库遗留格式为 `> ⚠️ **[回复被中断]** …` 与 `> ✕ **用户已取消执行…**`。
/// 这里剥掉引用符、加粗与 emoji 哨兵，只留纯文本，
/// 交给前端渲染成气泡下方的小字。
///
/// ⚠️ `.replace("⚠️", "⚠")` 专为**旧库数据**保留：新格式已不含 emoji，
/// 但旧会话历史不会自动改写——删掉它旧小字会显示带变体选择符的 `⚠️`。
/// 新格式与旧格式的识别锚点是 `strip_interrupt_markers`（agent_runs.rs）。
/// 前端原有的那份 `INTERRUPT_MARKER_LINE_RE` 已于 2026-09-21 删除：
/// 它对新数据永不命中，却会误伤模型正文里自己写的 `⚠` 并截断整条回复。
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

/// 按**结构化中断类型**生成用户可见的小字说明（阶段二路径）。
///
/// 与 [`interrupted_notice_text`]（从正文文本清洗，旧库兼容路径）的分工：
/// 新数据正文是干净的（标记不再拼进正文），notice 只能由 kind 生成；
/// 旧库数据 kind 列为 NULL，回退到文本清洗。
/// 返回值**不含**方括号标签与 emoji —— 它是给用户看的小字，不是给模型的标记。
fn interrupt_notice_for(kind: Option<&str>) -> Option<String> {
    use crate::infra::types::models::InterruptKind;
    let kind = InterruptKind::from_db(kind?)?;
    Some(match kind {
        InterruptKind::StreamTimeout => {
            "上游长时间未返回数据，本轮已自动终止。以上为已保留的部分结果，回复「继续」即可接着做。"
                .to_string()
        }
        InterruptKind::UserCancel => {
            "用户已取消执行，以上为保留的部分结果，历史未截断。".to_string()
        }
        InterruptKind::PipelineError => {
            "本轮执行中断，以上为已保留的部分结果，回复「继续」即可接着做。".to_string()
        }
        InterruptKind::AppClosed => "本次执行因应用关闭而中断，未产生回复内容。".to_string(),
        InterruptKind::LoopLimit => "已达回合上限且未获续跑授权，本轮在此停下。".to_string(),
        InterruptKind::PlanLimit => "规划探索已达到阈值，本轮自动停下，等待用户决策。".to_string(),
    })
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

    // ── 中断恢复：唯一闸门 `ensure_session_recovered`（含原子抢占锁 + 先落库后盖章），
    // 各入口禁止自行实现恢复链或 save/mark 收尾。──
    let active_run_id = ctx.active_run_id.lock().await.clone();
    let _ = crate::command::session::ensure_session_recovered(
        &session_id,
        &mut memory,
        active_run_id.as_deref(),
    );
    // 读取语义：以 DB 为准的 memory 写回 ctx（无论是否发生恢复）
    *ctx.memory.lock().await = memory.clone();

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
                let kind = memory.interrupt_kinds.get(idx).cloned().flatten();
                (idx, memory.message_ids.get(idx).cloned(), None, message.clone(), source, kind)
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

    let display_messages = render_messages
        .iter()
        .filter_map(|(memory_index, message_id, seq, msg, source, _kind)| {
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

    for (_, _, _, msg, source, kind) in &render_messages {
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
                // `interrupted` 消息承载两种东西，按 kind 是否存在区分 —— 这里曾经搞反过：
                // 渲染侧按"它不是模型正文"整条丢弃，而落库侧（`pipeline.rs` 中断收尾的
                // 分支二）写的恰恰是"正文即半截内容；正文块恒存在"，于是用户取消后
                // 只剩一句状态说明，已经生成的部分凭空消失。
                // - kind 有值（新数据）：正文块里是**半截内容**，必须一并渲染；
                // - kind 缺失（旧库数据）：正文是 `**[回复被中断]** …` 这类标记文本，
                //   已由上面的 notice 承载，再 append 会把它当模型正文重复显示一遍。
                // notice 本身仍渲染在气泡**下方**的小字里，不挤进回复气泡内部。
                if is_interrupted_source(source.as_str()) {
                    // 阶段二：notice 优先按**结构化 kind** 生成（不再从正文猜）；
                    // kind 缺失（旧库数据 / 恢复重建未打标）才回退文本清洗路径。
                    let notice = interrupt_notice_for(kind.as_deref())
                        .or_else(|| interrupted_notice_text(content));
                    if let Some(notice) = notice {
                        pending_assistant.notice = Some(notice);
                    }
                    // 只有旧库的标记文本拦在这里；新数据的半截正文继续往下走去 append
                    if !interrupted_body_is_content(kind.as_deref()) {
                        continue;
                    }
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
    // 全量语义：窗口不设限（分页元数据被丢弃）
    let (messages, _, _) =
        extract_session_messages_window(session_id, session_manager, registry, None, None).await?;
    Ok(messages)
}

/// 分页元数据
#[derive(Serialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct PagedSessionMessages {
    pub messages: Vec<SessionMessage>,
    /// 是否还有更早的历史可加载（向上翻页游标）
    pub has_more: bool,
    /// 本页最老消息的 seq（下次请求的游标）；页为空时为 None
    pub oldest_seq: Option<i64>,
}

/// 懒加载核心：从 events 恢复后，按窗口取消息并走与全量完全相同的加工逻辑
/// （turn 聚合 / checkpoint 关联 / snapshot 组装），只是消息来源换成
/// `load_visible_turns_page` 的窗口结果。
///
/// - `before_seq = None`：首屏（最新一页）；`Some(seq)`：以 seq 为游标向更早取
/// - `limit = None`：默认 50
async fn extract_session_messages_window(
    session_id: &str,
    session_manager: &SessionManager,
    registry: &SnapshotRegistry,
    before_seq: Option<i64>,
    max_turns: Option<usize>,
) -> Result<(Vec<SessionMessage>, bool, Option<i64>), String> {
    // max_turns = None 表示全量语义（不分页）；Some(n) 表示按轮凑页（n = 每页轮数，
    // 默认 5：2K 屏一屏约 2-3 轮，每页约 2 屏内容，翻页节奏适中）
    let paged = max_turns.is_some();
    let max_turns = max_turns.unwrap_or(5).max(1);
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

    // 中断恢复：唯一闸门 `ensure_session_recovered`（含原子抢占锁 + 先落库后盖章）。
    //
    // 该闸门内含"半截内容是否已存在"的去重守卫，而它的判定需要处理
    // 「正文 / 中断标记分体存储」与「正文+标记合并」两种形态。
    // 曾在此内联复制过一份无去重的实现，导致**每加载一次就多一条合并副本**
    // （实测"删掉再刷新，副本又回来"，且因未走到守卫分支连日志都不打印）。
    let active_run_id = ctx.active_run_id.lock().await.clone();
    let _ = crate::command::session::ensure_session_recovered(session_id, &mut memory, active_run_id.as_deref());
    *ctx.memory.lock().await = memory.clone();

    if memory.messages.is_empty() && session::session_messages_count(session_id).unwrap_or(0) == 0 {
        // 空会话：窗口版返回空页 + 无更早数据
        return Ok((Vec::new(), false, None));
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

    // 消息来源二选一：
    // - 按轮凑页：每页 max_turns 个完整轮（长任务轮密度自适应）
    // - 全量：原逻辑不变（max_turns = None）
    let (stored_messages, has_more) = if paged {
        let (page, more) = session::load_visible_turns_page(session_id, before_seq, max_turns)?;
        (page, Some(more))
    } else {
        (
            session::list_visible_session_messages(session_id)?,
            None,
        )
    };
    let render_messages: Vec<_> = if stored_messages.is_empty() {
        memory
            .messages
            .iter()
            .enumerate()
            .map(|(idx, message)| {
                let source = memory.sources.get(idx).cloned().unwrap_or_else(|| "chat".to_string());
                let kind = memory.interrupt_kinds.get(idx).cloned().flatten();
                (idx, memory.message_ids.get(idx).cloned(), None, message.clone(), source, kind)
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

    // 窗口最老消息的 seq（分页游标）；memory 回退分支无 seq，返回 None
    let oldest_seq = render_messages
        .first()
        .and_then(|(_, _, seq, _, _, _)| seq.map(|s| s as i64));

    let display_messages = render_messages
        .iter()
        .filter_map(|(memory_index, message_id, seq, msg, source, _kind)| {
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
    // 当前正在处理的消息 seq（懒加载分页下用 seq 生成**跨页稳定唯一**的 id——
    // 旧的 result.len() 位置编号在每页都从 0 开始，头部插入后 key 大量重复，
    // Vue diff 整表重建导致翻页时界面闪烁）
    let mut last_seen_seq: Option<usize> = None;

    for (_, _, seq, msg, source, kind) in &render_messages {
        current_ts += 1;
        last_seen_seq = *seq;
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
                        // seq 稳定唯一：分页/全量、跨页都不会撞 key
                        id: format!("agent_{}", last_seen_seq.map(|s| s.to_string()).unwrap_or_else(|| format!("m{}", result.len()))),
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
                    id: format!("user_{}", message.seq.map(|s| s.to_string()).unwrap_or_else(|| format!("m{}", result.len()))),
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
                // `interrupted` 消息承载两种东西，按 kind 是否存在区分 —— 这里曾经搞反过：
                // 渲染侧按"它不是模型正文"整条丢弃，而落库侧（`pipeline.rs` 中断收尾的
                // 分支二）写的恰恰是"正文即半截内容；正文块恒存在"，于是用户取消后
                // 只剩一句状态说明，已经生成的部分凭空消失。
                // - kind 有值（新数据）：正文块里是**半截内容**，必须一并渲染；
                // - kind 缺失（旧库数据）：正文是 `**[回复被中断]** …` 这类标记文本，
                //   已由上面的 notice 承载，再 append 会把它当模型正文重复显示一遍。
                // notice 本身仍渲染在气泡**下方**的小字里，不挤进回复气泡内部。
                if is_interrupted_source(source.as_str()) {
                    // 阶段二：notice 优先按**结构化 kind** 生成（不再从正文猜）；
                    // kind 缺失（旧库数据 / 恢复重建未打标）才回退文本清洗路径。
                    let notice = interrupt_notice_for(kind.as_deref())
                        .or_else(|| interrupted_notice_text(content));
                    if let Some(notice) = notice {
                        pending_assistant.notice = Some(notice);
                    }
                    // 只有旧库的标记文本拦在这里；新数据的半截正文继续往下走去 append
                    if !interrupted_body_is_content(kind.as_deref()) {
                        continue;
                    }
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
            id: format!("agent_{}", last_seen_seq.map(|s| s.to_string()).unwrap_or_else(|| format!("m{}", result.len()))),
            snapshot: Some(pending_assistant),
            snapshot_id: None,
            message_id: None,
            user_content: None,
            rollback_checkpoint_id: None,
            rollback_mode: None,
        });
    }

    Ok((result, has_more.unwrap_or(false), oldest_seq))
}

#[tauri::command]
pub async fn get_session_messages(
    session_id: String,
    session_manager: tauri::State<'_, SessionManager>,
    registry: tauri::State<'_, SnapshotRegistry>,
) -> Result<Vec<SessionMessage>, String> {
    // 重建时机前置：把 interrupted+resumable 的半截内容从 agent_run_events
    // 重放并落进 session_messages，使崩溃/中断后**仅查看会话**（无需发消息）
    // 即可看到完整对话。此前恢复只在发消息时触发（chat.ts 发送流程开头），
    // 重启后界面会一直缺半截内容，直到用户下一次发消息。
    //
    // 幂等性：走唯一闸门 `ensure_session_recovered`（原子抢占 + 先落库后盖章），
    // 无遗留 run 时零开销快速路径，高频调用无额外负担。
    let recovered = {
        let ctx = session_manager.get_or_create(&session_id).await;
        let active_run_id = ctx.active_run_id.lock().await.clone();
        let mut memory = ctx.memory.lock().await;
        crate::command::session::ensure_session_recovered(
            &session_id,
            &mut memory,
            active_run_id.as_deref(),
        )
    };
    if recovered {
        // 恢复出的增量已由闸门内统一落库；此处仅提示日志口径与恢复命令一致
        println!("[JARVIS] get_session_messages：中断恢复已补回消息并落库（session {}）", session_id);
    }
    extract_session_messages(&session_id, &session_manager, &registry).await
}

/// 懒加载分页命令：**按轮凑页**——一轮 = 一条可渲染 user 消息 + 其后所有消息
/// （assistant / tool_result 等，直到下一条轮起点）。长任务一轮几十条消息时，
/// 页大小随轮密度自适应，永远不会出现半轮。
///
/// - 首屏（`beforeSeq = None`）：最新的 `maxTurns` 个完整轮；
/// - 上滑翻页（`beforeSeq = 游标`）：更早的 `maxTurns` 个完整轮；
/// - 游标 = 本页最老消息的 seq（`oldestSeq`）。
#[tauri::command]
pub async fn get_session_messages_paged(
    session_id: String,
    before_seq: Option<i64>,
    limit: Option<usize>,
    session_manager: tauri::State<'_, SessionManager>,
    registry: tauri::State<'_, SnapshotRegistry>,
) -> Result<PagedSessionMessages, String> {
    // 中断恢复前置（唯一闸门 `ensure_session_recovered`，原子抢占 + 先落库后盖章）：
    // 恢复落库的是尾部消息，天然包含在首屏窗口内。
    // 下方 extract_session_messages_window 内部还有一次同闸门调用——
    // 首次已抢占成功并盖章，此处第二次只会走"无遗留 run"快速路径，幂等无害。
    let recovered = {
        let ctx = session_manager.get_or_create(&session_id).await;
        let active_run_id = ctx.active_run_id.lock().await.clone();
        let mut memory = ctx.memory.lock().await;
        crate::command::session::ensure_session_recovered(
            &session_id,
            &mut memory,
            active_run_id.as_deref(),
        )
    };
    if recovered {
        println!("[JARVIS] get_session_messages_paged：中断恢复已补回消息并落库（session {}）", session_id);
    }
    // limit 在此命令中语义为 **每页轮数**（默认 5 轮）
    let (messages, has_more, oldest_seq) =
        extract_session_messages_window(&session_id, &session_manager, &registry, before_seq, limit)
            .await?;
    Ok(PagedSessionMessages {
        messages,
        has_more,
        oldest_seq,
    })
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
        assert!(!snapshot_with_notice(Some("⚠ 上游长时间未返回数据")).is_empty());
    }

    /// 空白 notice 仍算空，避免渲染出一个空壳气泡
    #[test]
    fn blank_notice_is_still_empty() {
        assert!(snapshot_with_notice(Some("   ")).is_empty());
        assert!(snapshot_with_notice(None).is_empty());
    }
}

#[cfg(test)]
mod interrupted_body_tests {
    //! **实测 bug 的防护**：`source = "interrupted"` 的消息曾被渲染层整条丢弃
    //! （理由写的是"它不是模型正文"），而落库侧 `pipeline.rs` 写的恰恰是
    //! "正文即半截内容；正文块恒存在"。
    //!
    //! 后果：用户点「停止生成」后，界面只剩一句"用户已取消执行"，
    //! 已经生成的那部分正文凭空消失。
    //!
    //! 两边的契约以 **kind** 为准：别再按 source 一刀切地丢正文。
    use super::interrupted_body_is_content;

    /// 新数据（kind 有值）：正文是半截内容，必须渲染
    #[test]
    fn interrupt_with_kind_renders_body() {
        assert!(interrupted_body_is_content(Some("user_cancel")));
        assert!(interrupted_body_is_content(Some("stream_timeout")));
        assert!(interrupted_body_is_content(Some("app_closed")));
    }

    /// 旧库数据（kind 为 NULL）：正文是中断标记文本，不得渲染
    /// （notice 已由文本清洗路径生成并承载，再渲染会重复一遍）
    #[test]
    fn legacy_marker_body_stays_hidden() {
        assert!(!interrupted_body_is_content(None));
    }
}
