//! 消息格式转换适配器
//!
//! 负责 Anthropic 内部格式与 OpenAI 格式之间的双向转换：
//! - 消息结构转换（含多模态内容、工具调用、思考块）
//! - 工具定义格式转换
//! - 流式工具输入的 JSON 规范化
//!
//! DeepSeek 模型的 reasoning_content 字段特殊处理也在此模块。

use crate::infra::types::models::{
    Content, ContentBlock, Message, OpenAIContentPart, OpenAIFunctionCall,
    OpenAIFunctionDefinition, OpenAIImageUrl, OpenAIMessage, OpenAITool, OpenAIToolCall,
    OpenAIUserContent,
};
use crate::core::session;

/// 规范化 JSON 字符串中的控制字符（换行、制表符等）
fn normalize_json_string_control_chars(raw: &str) -> String {
    let mut normalized = String::with_capacity(raw.len());
    let mut in_string = false;
    let mut escaping = false;

    for ch in raw.chars() {
        if escaping {
            normalized.push(ch);
            escaping = false;
            continue;
        }

        match ch {
            '\\' => {
                normalized.push(ch);
                escaping = true;
            }
            '"' => {
                normalized.push(ch);
                in_string = !in_string;
            }
            '\n' if in_string => normalized.push_str("\\n"),
            '\r' if in_string => normalized.push_str("\\r"),
            '\t' if in_string => normalized.push_str("\\t"),
            c if in_string && c.is_control() => {
                normalized.push_str(&format!("\\u{:04x}", c as u32));
            }
            _ => normalized.push(ch),
        }
    }

    normalized
}

/// 解析流式工具调用的输入 JSON
///
/// 返回 (解析结果, 是否经过规范化修正)
pub fn parse_streamed_tool_input(raw: &str) -> Result<(serde_json::Value, bool), String> {
    match serde_json::from_str::<serde_json::Value>(raw) {
        Ok(value) => Ok((value, false)),
        Err(first_err) => {
            // Pass 1: 控制字符规范化
            let normalized = normalize_json_string_control_chars(raw);
            if let Ok(value) = serde_json::from_str::<serde_json::Value>(&normalized) {
                return Ok((value, normalized != raw));
            }
            // Pass 2: 修复字符串值中未转义的双引号（常见于 HTML/XML 内容）
            let repaired = repair_unescaped_quotes(&normalized);
            if let Ok(value) = serde_json::from_str::<serde_json::Value>(&repaired) {
                return Ok((value, true));
            }
            Err(format!("工具参数解析失败：{}", first_err))
        }
    }
}

/// 修复 JSON 字符串值中未转义的双引号（LLM 生成 HTML 时常漏掉）
fn repair_unescaped_quotes(raw: &str) -> String {
    let mut result = String::with_capacity(raw.len());
    let mut in_string = false;
    let mut escaping = false;
    let bytes = raw.as_bytes();
    let mut i = 0;
    while i < bytes.len() {
        let ch = bytes[i] as char;
        if escaping {
            result.push(ch);
            escaping = false;
            i += 1;
            continue;
        }
        match ch {
            '\\' => {
                result.push(ch);
                if in_string { escaping = true; }
                i += 1;
            }
            '"' => {
                if in_string {
                    // 在字符串内部遇到引号 → 检查是否为字符串结束
                    // 向前看：跳过空白，下一个非空字符是 , } ] : → 字符串结束
                    let next = skip_whitespace(raw, i + 1);
                    if next == Some(',') || next == Some('}') || next == Some(']') || next == Some(':') || next.is_none() {
                        in_string = false;
                        result.push(ch);
                    } else {
                        // 字符串内的引号 → 转义它
                        result.push('\\');
                        result.push(ch);
                    }
                } else {
                    in_string = true;
                    result.push(ch);
                }
                i += 1;
            }
            _ => {
                result.push(ch);
                i += 1;
            }
        }
    }
    result
}

fn skip_whitespace(raw: &str, start: usize) -> Option<char> {
    raw[start..].chars().find(|c| !c.is_whitespace())
}

/// 为没有思考块的消息生成一个最小 reasoning_content，保持 OpenAI 对话格式一致
fn non_deepseek_reasoning_placeholder() -> serde_json::Value {
    serde_json::Value::String(String::new())
}

/// 将思考块内容转为 reasoning_content 字符串（OpenAI 格式要求纯文本，不能是对象）
fn reasoning_content_from_thinking(thinking: &str) -> serde_json::Value {
    serde_json::Value::String(thinking.to_string())
}

/// 将 Anthropic 消息格式转换为 OpenAI 格式
pub fn translate_messages_to_openai(system: &str, messages: &[Message]) -> Vec<OpenAIMessage> {
    translate_messages_to_openai_with_reasoning_backfill(system, messages, false)
}

/// 将 Anthropic 消息格式转换为 OpenAI 格式（支持 DeepSeek reasoning_content 回填）
pub fn translate_messages_to_openai_with_reasoning_backfill(
    system: &str,
    messages: &[Message],
    backfill_assistant_reasoning_content: bool,
) -> Vec<OpenAIMessage> {
    let mut openai_msgs = Vec::new();

    if !system.is_empty() {
        openai_msgs.push(OpenAIMessage::System {
            content: system.to_string(),
        });
    }

    for msg in messages {
        match msg {
            Message::User { content } => match content {
                Content::Single(text) => {
                    openai_msgs.push(OpenAIMessage::User {
                        content: OpenAIUserContent::Text(text.clone()),
                    });
                }
                Content::Multiple(blocks) => {
                    let mut has_complex_content = false;
                    let mut text_parts = Vec::new();
                    let mut content_parts: Vec<OpenAIContentPart> = Vec::new();

                    for block in blocks {
                        match block {
                            ContentBlock::Text { text } => {
                                text_parts.push(text.clone());
                                content_parts.push(OpenAIContentPart::Text { text: text.clone() });
                            }
                            ContentBlock::Image { source } => {
                                has_complex_content = true;
                                let data = if !source.data.is_empty() {
                                    source.data.clone()
                                } else if let Some(ref fp) = source.file_path {
                                    session::load_image_data(fp).unwrap_or_default()
                                } else {
                                    String::new()
                                };
                                let url = format!("data:{};base64,{}", source.media_type, data);
                                content_parts.push(OpenAIContentPart::ImageUrl {
                                    image_url: OpenAIImageUrl { url },
                                });
                            }
                            ContentBlock::ToolResult {
                                tool_use_id,
                                content,
                            } => {
                                openai_msgs.push(OpenAIMessage::Tool {
                                    content: content.clone(),
                                    tool_call_id: tool_use_id.clone(),
                                });
                            }
                            _ => {}
                        }
                    }

                    if has_complex_content {
                        openai_msgs.push(OpenAIMessage::User {
                            content: OpenAIUserContent::Parts(content_parts),
                        });
                    } else if !text_parts.is_empty() {
                        openai_msgs.push(OpenAIMessage::User {
                            content: OpenAIUserContent::Text(text_parts.join("\n")),
                        });
                    }
                }
            },
            Message::Assistant { content } => match content {
                Content::Single(text) => {
                    openai_msgs.push(OpenAIMessage::Assistant {
                        content: Some(text.clone()),
                        tool_calls: None,
                        reasoning_content: if backfill_assistant_reasoning_content {
                            Some(non_deepseek_reasoning_placeholder())
                        } else {
                            None
                        },
                    });
                }
                Content::Multiple(blocks) => {
                    let mut text_content = String::new();
                    let mut tool_calls = Vec::new();
                    let mut thinking_segments = Vec::new();

                    for block in blocks {
                        match block {
                            ContentBlock::Text { text } => {
                                text_content.push_str(text);
                            }
                            ContentBlock::Thinking { thinking, .. } => {
                                if backfill_assistant_reasoning_content {
                                    thinking_segments.push(thinking.clone());
                                } else {
                                    text_content.push_str(&format!(
                                        "\n<thought>\n{}\n</thought>\n",
                                        thinking
                                    ));
                                }
                            }
                            ContentBlock::ToolUse { id, name, input } => {
                                tool_calls.push(OpenAIToolCall {
                                    id: id.clone(),
                                    r#type: "function".to_string(),
                                    function: OpenAIFunctionCall {
                                        name: name.clone(),
                                        arguments: serde_json::to_string(&input)
                                            .unwrap_or_else(|_| "{}".to_string()),
                                    },
                                });
                            }
                            _ => {}
                        }
                    }

                    if !text_content.is_empty()
                        || !tool_calls.is_empty()
                        || (backfill_assistant_reasoning_content && !thinking_segments.is_empty())
                    {
                        openai_msgs.push(OpenAIMessage::Assistant {
                            content: if text_content.is_empty() {
                                None
                            } else {
                                Some(text_content)
                            },
                            tool_calls: if tool_calls.is_empty() {
                                None
                            } else {
                                Some(tool_calls)
                            },
                            reasoning_content: if backfill_assistant_reasoning_content {
                                Some(if thinking_segments.is_empty() {
                                    non_deepseek_reasoning_placeholder()
                                } else {
                                    reasoning_content_from_thinking(&thinking_segments.join("\n"))
                                })
                            } else {
                                None
                            },
                        });
                    }
                }
            },
        }
    }

    openai_msgs
}

/// 将 Anthropic 内部格式的工具定义翻译为 OpenAI 格式
/// 判断是否需要为 DeepSeek 模型回填 reasoning_content
pub fn should_backfill_deepseek_reasoning_content(
    model_id: &str,
    base_url: &str,
    should_think: bool,
) -> bool {
    if !should_think {
        return false;
    }

    let model = model_id.to_lowercase();
    let url = base_url.to_lowercase();
    model.contains("deepseek") || url.contains("deepseek")
}

/// 将 Anthropic 工具定义转换为 OpenAI 格式
pub fn translate_tools_to_openai(tools: &[serde_json::Value]) -> Vec<OpenAITool> {
    tools
        .iter()
        .filter_map(|t| {
            let name = t.get("name")?.as_str()?.to_string();
            let description = t.get("description")?.as_str()?.to_string();
            let parameters = t.get("input_schema")?.clone();

            Some(OpenAITool {
                r#type: "function".to_string(),
                function: OpenAIFunctionDefinition {
                    name,
                    description,
                    parameters,
                },
            })
        })
        .collect()
}
