use crate::core::models::*;
use crate::core::sessions;
use crate::core::state::*;
use crate::core::checkpoint;

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
    for msg in &memory.messages {
        match msg {
            Message::User { content } => {
                let display = match content {
                    Content::Single(s) => {
                        let text = if let Some(pos) = s.find("[User Input]:") {
                            s[pos + 13..].trim().to_string()
                        } else {
                            s.trim().to_string()
                        };
                        text
                    }
                    Content::Multiple(blocks) => {
                        let mut parts = String::new();
                        for block in blocks {
                            match block {
                                ContentBlock::Text { text } => {
                                    let t = if let Some(pos) = text.find("[User Input]:") {
                                        text[pos + 13..].trim().to_string()
                                    } else {
                                        text.trim().to_string()
                                    };
                                    if !t.is_empty() {
                                        parts.push_str(&t);
                                        parts.push('\n');
                                    }
                                }
                                ContentBlock::Image { source } => {
                                    let data = if !source.data.is_empty() {
                                        source.data.clone()
                                    } else if let Some(ref fp) = source.file_path {
                                        sessions::load_image_data(fp).unwrap_or_default()
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
                };
                if !display.is_empty() {
                    let preview = display.chars().take(100).collect::<String>();
                    let cp_info = checkpoint_map.get(&preview).cloned();
                    let cp_attr = cp_info.as_ref()
                        .map(|(id, _)| format!(" data-checkpoint-id=\"{}\"", id))
                        .unwrap_or_default();
                    let btn_html = cp_info.as_ref()
                        .map(|(id, has_ops)| format!("<button class=\"rollback-trigger\" data-cp-id=\"{}\" data-has-operations=\"{}\" title=\"撤回此消息及操作\"></button>", id, has_ops))
                        .unwrap_or_default();
                    history.push_str(&format!(
                        "<div class=\"chat-message user-message\" style=\"position:relative\"{}><div class=\"message-content\">\n\n{}\n\n</div>{}</div>\n\n",
                        cp_attr, display, btn_html
                    ));
                }
            }
            Message::Assistant { content } => {
                let mut all_thinking = String::new();
                let mut all_text = String::new();

                match content {
                    Content::Single(s) => {
                        all_text.push_str(s.trim());
                    }
                    Content::Multiple(blocks) => {
                        for block in blocks {
                            match block {
                                ContentBlock::Text { text } => {
                                    let trimmed = text.trim();
                                    if !trimmed.is_empty() {
                                        all_text.push_str(trimmed);
                                    }
                                }
                                ContentBlock::Thinking { thinking, .. } => {
                                    let trimmed = thinking.trim();
                                    if !trimmed.is_empty() {
                                        if !all_thinking.is_empty() {
                                            all_thinking.push('\n');
                                        }
                                        all_thinking.push_str(trimmed);
                                    }
                                }
                                _ => {}
                            }
                        }
                    }
                }

                if all_text.is_empty() && all_thinking.is_empty() {
                    continue;
                }

                history.push_str(
                    "<div class=\"chat-message agent-message\"><div class=\"message-content\">\n\n",
                );

                if !all_thinking.is_empty() {
                    history.push_str(&format!(
                        "\n\n<details><summary><svg viewBox=\"0 0 24 24\" width=\"14\" height=\"14\" stroke=\"currentColor\" stroke-width=\"2\" fill=\"none\" stroke-linecap=\"round\" stroke-linejoin=\"round\" style=\"vertical-align: text-bottom; margin-right: 4px;\"><circle cx=\"12\" cy=\"12\" r=\"3\"></circle><path d=\"M19.4 15a1.65 1.65 0 0 0 .33 1.82l.06.06a2 2 0 0 1 0 2.83 2 2 0 0 1-2.83 0l-.06-.06a1.65 1.65 0 0 0-1.82-.33 1.65 1.65 0 0 0-1 1.51V21a2 2 0 0 1-2 2 2 2 0 0 1-2-2v-.09A1.65 1.65 0 0 0 9 19.4a1.65 1.65 0 0 0-1.82.33l-.06.06a2 2 0 0 1-2.83 0 2 2 0 0 1 0-2.83l.06-.06a1.65 1.65 0 0 0 .33-1.82 1.65 1.65 0 0 0-1.51-1H3a2 2 0 0 1-2-2 2 2 0 0 1 2-2h.09A1.65 1.65 0 0 0 4.6 9a1.65 1.65 0 0 0-.33-1.82l-.06-.06a2 2 0 0 1 0-2.83 2 2 0 0 1 2.83 0l.06.06a1.65 1.65 0 0 0 1.82.33H9a1.65 1.65 0 0 0 1-1.51V3a2 2 0 0 1 2-2 2 2 0 0 1 2 2v.09a1.65 1.65 0 0 0 1 1.51 1.65 1.65 0 0 0 1.82-.33l.06-.06a2 2 0 0 1 2.83 0 2 2 0 0 1 0 2.83l-.06.06a1.65 1.65 0 0 0-.33 1.82V9a1.65 1.65 0 0 0 1.51 1H21a2 2 0 0 1 2 2 2 2 0 0 1-2 2h-.09a1.65 1.65 0 0 0-1.51 1z\"></path></svg> 贾维斯已完成思考与操作 (点击查看完整决策链)</summary>\n\n{}\n\n</details>\n\n",
                        all_thinking
                    ));
                }

                history.push_str(&all_text);
                history.push_str("\n\n</div></div>\n\n");
            }
        }
    }
    Ok(history)
}
