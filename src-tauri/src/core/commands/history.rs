//! # history.rs — 会话历史渲染 Tauri 命令
//!
//! 将会话消息历史渲染为 HTML 格式，供前端展示。
//! 处理用户消息（含图片 base64 内联）、助手消息（思考过程折叠显示），
//! 并关联检查点信息以支持消息撤回按钮。
//!
//! ## 关键导出
//! - `get_session_history()`: 返回会话历史的 HTML 渲染结果
//!
//! ## 约束
//! - 过滤内部消息（background-results 通知、内部 ack 回复）
//! - 助手多轮回复合并显示，思考过程用 `<details>` 折叠
//! - 用户消息关联检查点 ID，支持前端回滚按钮

use crate::core::models::*;
use crate::core::orchestration::agent_runs;
use crate::core::session;
use crate::core::state::*;

#[derive(Clone)]
struct RollbackInfo {
    checkpoint_id: String,
    has_file_edits: bool,
    created_at: u64,
}

struct UserDisplayMessage {
    memory_index: usize,
    display: String,
    rollback_info: Option<RollbackInfo>,
}

use serde::Serialize;

#[derive(Serialize, Clone, Default)]
#[serde(rename_all = "camelCase")]
struct AgentTurnSnapshot {
    version: u32,
    status: String,
    text_blocks: Vec<AgentTextBlock>,
    thinking_blocks: Vec<AgentThinkingBlock>,
    tool_calls: Vec<AgentToolCallView>,
    logs: Vec<AgentExecutionLog>,
    created_at: u64,
}

#[derive(Serialize, Clone)]
#[serde(rename_all = "camelCase")]
struct AgentTextBlock {
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
struct AgentThinkingBlock {
    id: String,
    #[serde(rename = "loop")]
    loop_: u32,
    content: String,
    status: String,
    timestamp: u64,
}

#[derive(Serialize, Clone)]
#[serde(rename_all = "camelCase")]
struct AgentToolCallView {
    id: String,
    #[serde(rename = "loop")]
    loop_: u32,
    name: String,
    status: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    input_summary: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    output_summary: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    error: Option<String>,
    logs: Vec<String>,
    timestamp: u64,
    updated_at: u64,
}

#[derive(Serialize, Clone)]
#[serde(rename_all = "camelCase")]
struct AgentExecutionLog {
    id: String,
    #[serde(rename = "loop")]
    loop_: u32,
    content: String,
    timestamp: u64,
}

impl AgentTurnSnapshot {
    fn is_empty(&self) -> bool {
        self.text_blocks.is_empty() && self.thinking_blocks.is_empty() && self.tool_calls.is_empty()
    }
}

/// 去除注入的动态上下文前缀，只保留用户原始输入
fn clean_user_text(value: &str) -> String {
    if let Some(pos) = value.find("[User Input]:") {
        value[pos + 13..].trim().to_string()
    } else {
        value.trim().to_string()
    }
}

/// 判断是否为系统内部注入的消息（后台任务结果通知等）
fn is_internal_user_text(value: &str) -> bool {
    let trimmed = value.trim();
    trimmed.starts_with("<background-results>") || trimmed.starts_with("<background-results")
}

fn is_internal_assistant_message(content: &Content) -> bool {
    matches!(content, Content::Single(s) if s.trim() == "Noted background results.")
}

fn user_display_content(content: &Content) -> String {
    match content {
        Content::Single(s) => clean_user_text(s),
        Content::Multiple(blocks) => {
            let mut parts = String::new();
            for block in blocks {
                match block {
                    ContentBlock::Text { text } => {
                        let t = clean_user_text(text);
                        if !t.is_empty() {
                            parts.push_str(&t);
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
                        let input_summary = serde_json::to_string_pretty(input).unwrap_or_default();
                        target.tool_calls.push(AgentToolCallView {
                            id: id.clone(),
                            loop_: loop_idx,
                            name: name.clone(),
                            status: "running".to_string(),
                            input_summary: Some(format!(
                                "`json
{}
`",
                                input_summary
                            )),
                            output_summary: None,
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
        tool.output_summary = Some(format!(
            "`
{}
`",
            content.trim()
        ));
        tool.updated_at = timestamp;
    }
}

fn build_linked_rollbacks(session_id: &str) -> Vec<(usize, RollbackInfo)> {
    let mut links = crate::core::db::list_checkpoint_user_message_links(session_id)
        .unwrap_or_default()
        .into_iter()
        .filter(|link| link.has_file_edits && !link.checkpoint_id.is_empty())
        .map(|link| {
            (
                link.user_message_index,
                RollbackInfo {
                    checkpoint_id: link.checkpoint_id,
                    has_file_edits: true,
                    created_at: link.created_at,
                },
            )
        })
        .collect::<Vec<_>>();
    links.sort_by_key(|(trigger_index, info)| (*trigger_index, info.created_at));
    links
}

fn parse_snapshot_usize(snapshot: &crate::core::rollback::Snapshot, key: &str) -> Option<usize> {
    snapshot
        .metadata
        .get(key)
        .and_then(|value| value.parse::<usize>().ok())
}

fn find_rollback_info(
    linked_rollbacks: &[(usize, RollbackInfo)],
    metadata_rollbacks: &[(usize, u64, String)],
    memory_index: usize,
) -> Option<RollbackInfo> {
    let linked = linked_rollbacks
        .iter()
        .filter(|(trigger_index, _)| *trigger_index >= memory_index)
        .min_by_key(|(trigger_index, info)| (*trigger_index, info.created_at))
        .map(|(_, info)| info.clone());
    let metadata = metadata_rollbacks
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
        "<div class=\"chat-message user-message\" style=\"position: relative;\"><div class=\"message-content\" data-user-message-index=\"{}\" data-rollback-mode=\"{}\" data-rollback-checkpoint-id=\"{}\">\n\n{}\n\n</div></div>\n\n",
        message.memory_index, rollback_mode, rollback_checkpoint_id, display
    ));
}

/// 渲染助手消息 HTML，思考过程用 details 折叠，取最后一段非空文本作为可见回复

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
    history.push_str(
        "

</div></div>

",
    );
}

#[tauri::command]
pub async fn get_session_history(
    session_id: String,
    session_manager: tauri::State<'_, SessionManager>,
    registry: tauri::State<'_, SnapshotRegistry>,
) -> Result<String, String> {
    let ctx = session_manager.get_or_create(&session_id).await;
    let mut memory = session::load_session(&session_id)?;

    // ── 中断恢复：检测并补回崩溃/中断时丢失的消息 ──
    if let Some((extra_messages, partial_content, partial_thinking)) =
        agent_runs::recover_interrupted_messages(&session_id, &memory.messages)
    {
        // 补回 checkpoint 中多出的消息（用户消息、工具结果等）
        memory.messages.extend(extra_messages);

        // 如果有半截助手回复，追加为一条助手消息
        // 在半截文本末尾追加中断标记，让 LLM 知道自己的回复被中断了
        let has_partial_content = !partial_content.trim().is_empty();
        let has_partial_thinking = !partial_thinking.trim().is_empty();
        if has_partial_content || has_partial_thinking {
            let mut blocks = Vec::new();
            if has_partial_thinking {
                blocks.push(ContentBlock::Thinking {
                    thinking: partial_thinking,
                    signature: String::new(),
                });
            }
            if has_partial_content {
                // 在半截文本后追加中断标记
                let marked_content = format!(
                    "{}\n\n> ⚠️ **[回复被中断]** 上次回复在此处中断，请基于上下文继续完成。",
                    partial_content.trim_end()
                );
                blocks.push(ContentBlock::Text {
                    text: marked_content,
                });
            }
            memory.messages.push(Message::Assistant {
                content: Content::Multiple(blocks),
            });
        }

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

    if memory.messages.is_empty() {
        return Ok(String::new());
    }

    let linked_rollbacks = build_linked_rollbacks(&session_id);
    let metadata_rollbacks = {
        let manager = registry.0.read().await.get_or_create(&session_id).await?;
        let mut items = manager
            .list_snapshots(None)
            .await
            .into_iter()
            .filter(|snapshot| snapshot.is_checkpoint)
            .filter_map(|snapshot| {
                let trigger_index = parse_snapshot_usize(&snapshot, "trigger_user_memory_index")?;
                let patch_count = parse_snapshot_usize(&snapshot, "patch_count").unwrap_or(0);
                if patch_count == 0 {
                    return None;
                }
                Some((trigger_index, snapshot.created_at, snapshot.id))
            })
            .collect::<Vec<_>>();
        items.sort_by_key(|(trigger_index, created_at, _)| (*trigger_index, *created_at));
        items
    };

    let display_messages = memory
        .messages
        .iter()
        .enumerate()
        .filter_map(|(memory_index, msg)| {
            if let Message::User { content } = msg {
                let display = user_display_content(content);
                if !is_internal_user_text(&display) && !display.trim().is_empty() {
                    return Some(UserDisplayMessage {
                        memory_index,
                        display,
                        rollback_info: find_rollback_info(
                            &linked_rollbacks,
                            &metadata_rollbacks,
                            memory_index,
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

    for msg in &memory.messages {
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

                if is_internal_user_text(&display) || display.trim().is_empty() {
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
                if is_internal_assistant_message(content) {
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
