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

use crate::core::session::checkpoint;
use crate::core::models::*;
use crate::core::session;
use crate::core::state::*;

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
    display: &str,
    checkpoint_map: &std::collections::HashMap<String, (String, bool)>,
) {
    if display.trim().is_empty() {
        return;
    }
    // 多级模糊匹配：精确匹配 → 100 字符前缀 → 50 字符前缀 → 互为前缀
    let preview_50 = display.chars().take(50).collect::<String>();
    let preview_100 = display.chars().take(100).collect::<String>();
    let cp_info = checkpoint_map
        .get(display)
        .or_else(|| checkpoint_map.get(&preview_100))
        .or_else(|| checkpoint_map.get(&preview_50))
        .cloned()
        .or_else(|| {
            checkpoint_map
                .iter()
                .find(|(trigger, _)| {
                    let trigger = trigger.trim();
                    let display = display.trim();
                    !trigger.is_empty()
                        && !display.is_empty()
                        && (display.starts_with(trigger) || trigger.starts_with(display))
                })
                .map(|(_, value)| value.clone())
        });
    let cp_attr = cp_info
        .as_ref()
        .map(|(id, _)| format!(" data-checkpoint-id=\"{}\"", id))
        .unwrap_or_default();
    let btn_html = cp_info
        .as_ref()
        .map(|(id, has_ops)| {
            format!(
                "<button class=\"rollback-trigger\" data-cp-id=\"{}\" data-has-operations=\"{}\" title=\"撤回此消息及操作\"></button>",
                id, has_ops
            )
        })
        .unwrap_or_default();
    history.push_str(&format!(
        "<div class=\"chat-message user-message\" style=\"position:relative\"{}><div class=\"message-content\">\n\n{}\n\n</div>{}</div>\n\n",
        cp_attr, display, btn_html
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

    history.push_str("<div class=\"chat-message agent-message\"><div class=\"message-content\">\n\n");

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
) -> Result<String, String> {
    let ctx = session_manager.get_or_create(&session_id).await;
    let memory = ctx.memory.lock().await.clone();

    if memory.messages.is_empty() {
        return Ok(String::new());
    }

    let checkpoint_map: std::collections::HashMap<String, (String, bool)> = {
        let checkpoints = checkpoint::list_checkpoints(&session_id, None);

        checkpoints
            .iter()
            .enumerate()
            .map(|(i, cp)| {
                let has_operations_after = checkpoints[i..]
                    .iter()
                    .any(|later_cp| !later_cp.operations.is_empty());

                (cp.trigger_message.clone(), (cp.id.clone(), has_operations_after))
            })
            .collect()
    };

    let mut history = String::new();
    let mut pending_assistant = AssistantDisplay::default();

    for msg in &memory.messages {
        match msg {
            Message::User { content } => {
                let display = user_display_content(content);
                if is_internal_user_text(&display) {
                    continue;
                }

                render_assistant_message(&mut history, &pending_assistant);
                pending_assistant = AssistantDisplay::default();
                render_user_message(&mut history, &display, &checkpoint_map);
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
