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

use crate::core::commands::checkpoint::Checkpoint;
use crate::core::models::*;
use crate::core::orchestration::agent_runs;
use crate::core::session;
use crate::core::state::*;

struct CheckpointRollbackInfo {
    id: String,
    has_operations_after: bool,
}

struct UserDisplayMessage {
    memory_index: usize,
    display: String,
}

#[derive(Default)]
struct AssistantDisplay {
    thinking: String,
    text_parts: Vec<String>,
}

impl AssistantDisplay {
    fn is_empty(&self) -> bool {
        self.thinking.trim().is_empty() && self.text_parts.iter().all(|s| s.trim().is_empty())
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

fn checkpoint_matches_user_message(trigger: &str, message: &str) -> bool {
    let trigger = trigger.trim();
    let message = message.trim();
    if trigger.is_empty() || message.is_empty() {
        return false;
    }

    message == trigger || message.starts_with(trigger) || trigger.starts_with(message)
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

fn append_assistant_content(target: &mut AssistantDisplay, content: &Content) {
    match content {
        Content::Single(s) => {
            let trimmed = s.trim();
            if !trimmed.is_empty() {
                target.text_parts.push(trimmed.to_string());
            }
        }
        Content::Multiple(blocks) => {
            for block in blocks {
                match block {
                    ContentBlock::Text { text } => {
                        let trimmed = text.trim();
                        if !trimmed.is_empty() {
                            target.text_parts.push(trimmed.to_string());
                        }
                    }
                    ContentBlock::Thinking { thinking, .. } => {
                        let trimmed = thinking.trim();
                        if !trimmed.is_empty() {
                            if !target.thinking.is_empty() {
                                target.thinking.push_str("\n\n");
                            }
                            target.thinking.push_str(trimmed);
                        }
                    }
                    _ => {}
                }
            }
        }
    }
}

/// 渲染用户消息 HTML，关联检查点信息以支持撤回按钮
fn render_user_message(
    history: &mut String,
    message: &UserDisplayMessage,
    checkpoint_info: Option<&CheckpointRollbackInfo>,
) {
    let display = &message.display;
    if display.trim().is_empty() {
        return;
    }

    let cp_attr = checkpoint_info
        .map(|info| format!(" data-checkpoint-id=\"{}\"", info.id))
        .unwrap_or_default();
    let cp_id = checkpoint_info
        .map(|info| info.id.as_str())
        .unwrap_or_default();
    let has_operations = checkpoint_info
        .map(|info| info.has_operations_after)
        .unwrap_or(false);
    let btn_title = if has_operations {
        "撤回此消息及操作"
    } else {
        "撤回此消息"
    };
    let btn_html = format!(
        "<button class=\"rollback-trigger\" data-cp-id=\"{}\" data-has-operations=\"{}\" title=\"{}\"></button>",
        cp_id, has_operations, btn_title
    );
    history.push_str(&format!(
        "<div class=\"chat-message user-message\" style=\"position: relative;\"><div class=\"message-content\"{} data-user-message-index=\"{}\">\n\n{}\n\n</div>{}</div>\n\n",
        cp_attr, message.memory_index, display, btn_html
    ));
}

/// 渲染助手消息 HTML，思考过程用 details 折叠，取最后一段非空文本作为可见回复
fn render_assistant_message(history: &mut String, assistant: &AssistantDisplay) {
    if assistant.is_empty() {
        return;
    }

    // 取最后一段非空文本作为最终回复（早期会话每个 agent loop 一条助手消息）
    let final_text = assistant
        .text_parts
        .iter()
        .rev()
        .find(|s| !s.trim().is_empty())
        .map(|s| s.trim())
        .unwrap_or("");
    let visible_text = if final_text.is_empty() {
        assistant.thinking.trim()
    } else {
        final_text
    };

    history
        .push_str("<div class=\"chat-message agent-message\"><div class=\"message-content\">\n\n");

    if !assistant.thinking.trim().is_empty() {
        history.push_str(&format!(
            "\n\n<details><summary><svg viewBox=\"0 0 24 24\" width=\"14\" height=\"14\" stroke=\"currentColor\" stroke-width=\"2\" fill=\"none\" stroke-linecap=\"round\" stroke-linejoin=\"round\" style=\"vertical-align: text-bottom; margin-right: 4px;\"><circle cx=\"12\" cy=\"12\" r=\"3\"></circle><path d=\"M12 2v3\"></path><path d=\"M12 19v3\"></path><path d=\"M4.93 4.93l2.12 2.12\"></path><path d=\"M16.95 16.95l2.12 2.12\"></path><path d=\"M2 12h3\"></path><path d=\"M19 12h3\"></path><path d=\"M4.93 19.07l2.12-2.12\"></path><path d=\"M16.95 7.05l2.12-2.12\"></path></svg> 贾维斯已完成思考与操作（点击查看完整决策链）</summary>\n\n{}\n\n</details>\n\n",
            assistant.thinking
        ));
    }

    if !visible_text.is_empty() {
        history.push_str(visible_text);
    }
    history.push_str("\n\n</div></div>\n\n");
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

    let manager = registry.0.read().await.get_or_create(&session_id).await?;
    let snapshots = manager.list_snapshots(None).await;
    let checkpoints: Vec<Checkpoint> = snapshots
        .iter()
        .filter(|s| s.is_checkpoint)
        .map(|s| Checkpoint {
            id: s.id.clone(),
            session_id: session_id.clone(),
            parent_id: s.parent_id.clone(),
            branch_name: s.branch_name.clone(),
            agent_id: s.agent_id.clone(),
            workspace_id: s.workspace_id.clone(),
            created_at: s.created_at,
            trigger_message: s.message.clone().unwrap_or_default(),
            operations: vec![],
            metadata: s.metadata.clone(),
        })
        .collect();
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
                    });
                }
            }
            None
        })
        .collect::<Vec<_>>();

    let mut checkpoint_by_user_index = std::collections::HashMap::new();
    for checkpoint in &checkpoints {
        let trigger_index = display_messages.iter().position(|message| {
            checkpoint_matches_user_message(&checkpoint.trigger_message, &message.display)
        });
        if let Some(index) = trigger_index {
            let target_index = index.saturating_sub(1);
            let has_operations_after = !checkpoint.operations.is_empty()
                || checkpoints.iter().any(|candidate| {
                    candidate.created_at > checkpoint.created_at && !candidate.operations.is_empty()
                });
            let entry = checkpoint_by_user_index
                .entry(target_index)
                .or_insert_with(|| CheckpointRollbackInfo {
                    id: checkpoint.id.clone(),
                    has_operations_after,
                });
            if has_operations_after {
                entry.has_operations_after = true;
            }
        }
    }

    let mut history = String::new();
    let mut pending_assistant = AssistantDisplay::default();
    let mut visible_user_index = 0usize;

    for msg in &memory.messages {
        match msg {
            Message::User { content } => {
                let display = user_display_content(content);
                if is_internal_user_text(&display) {
                    continue;
                }
                let Some(message) = display_messages.get(visible_user_index) else {
                    continue;
                };

                render_assistant_message(&mut history, &pending_assistant);
                pending_assistant = AssistantDisplay::default();
                render_user_message(
                    &mut history,
                    message,
                    checkpoint_by_user_index.get(&visible_user_index),
                );
                visible_user_index += 1;
            }
            Message::Assistant { content } => {
                if is_internal_assistant_message(content) {
                    continue;
                }
                append_assistant_content(&mut pending_assistant, content);
            }
        }
    }

    render_assistant_message(&mut history, &pending_assistant);
    Ok(history)
}
