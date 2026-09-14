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

/// 将思考块内容转为 reasoning_content 字符串（OpenAI 格式要求纯文本，不能是对象）
fn reasoning_content_from_thinking(thinking: &str) -> serde_json::Value {
    serde_json::Value::String(thinking.to_string())
}

/// 摘掉所有内部 `Context` 块，返回一份新的消息列表。
///
/// 用于不需要运行时上下文的场景：历史摘要、标题生成、开发视图等——
/// ctx 里塞着工作目录 / repo map / 全局记忆，混进去只会白烧 token。
pub fn strip_context_blocks(messages: &[Message]) -> Vec<Message> {
    messages
        .iter()
        .map(|msg| {
            if let Message::User {
                content: Content::Multiple(blocks),
            } = msg
            {
                let kept: Vec<ContentBlock> = blocks
                    .iter()
                    .filter(|b| !matches!(b, ContentBlock::Context { .. }))
                    .cloned()
                    .collect();
                if kept.len() == 1 {
                    if let ContentBlock::Text { text } = &kept[0] {
                        return Message::User {
                            content: Content::Single(text.clone()),
                        };
                    }
                }
                return Message::User {
                    content: Content::Multiple(kept),
                };
            }
            msg.clone()
        })
        .collect()
}

/// 出网时是否要剥掉"无签名"的 thinking 块。
///
/// 两个方向的要求是相反的，必须按服务商区分：
/// - 真 Anthropic：回传 `signature` 为空的 thinking 会被判 400 → **必须剥**；
/// - DeepSeek 这类端点：思考模式下**要求**把 thinking 原样带回，剥掉会报
///   `content[].thinking in the thinking mode must be passed back to the API` → **必须留**。
///
/// 判据与 OpenAI 出口的 `reasoning_content` 回填共用（model 或 baseUrl 含 deepseek），
/// 保证同一家服务商在两条出口上的思考链策略一致。
pub fn should_strip_unsigned_thinking(model_id: &str, base_url: &str, should_think: bool) -> bool {
    !should_backfill_deepseek_reasoning_content(model_id, base_url, should_think)
}

/// Anthropic 出口专用：丢弃 `signature` 为空的 thinking 块，返回一份新的消息列表。
///
/// OpenAI 格式模型（DeepSeek 等）的 reasoning 会被流解析器合成为
/// `Thinking { signature: "" }`。这种块回传给 Anthropic 会被直接判 400
/// （thinking 块必须携带服务商签发的原始 signature），所以跨模型复用同一条会话时，
/// 必须在出网前把它摘掉。
///
/// 代价只是这一轮的思考链不再回传（推理连续性弱一点），对话正确性不受影响。
/// 若整条 assistant 消息摘完只剩空，就整条丢弃——Anthropic 不接受空 content。
pub fn strip_unsigned_thinking_for_anthropic(messages: &[Message]) -> Vec<Message> {
    messages
        .iter()
        .filter_map(|msg| {
            let Message::Assistant {
                content: Content::Multiple(blocks),
            } = msg
            else {
                return Some(msg.clone());
            };

            let unsigned = |b: &ContentBlock| {
                matches!(b, ContentBlock::Thinking { signature, .. } if signature.is_empty())
            };
            if !blocks.iter().any(unsigned) {
                return Some(msg.clone());
            }

            let kept: Vec<ContentBlock> = blocks.iter().filter(|b| !unsigned(b)).cloned().collect();
            if kept.is_empty() {
                return None;
            }
            if kept.len() == 1 {
                if let ContentBlock::Text { text } = &kept[0] {
                    return Some(Message::Assistant {
                        content: Content::Single(text.clone()),
                    });
                }
            }
            Some(Message::Assistant {
                content: Content::Multiple(kept),
            })
        })
        .collect()
}

/// 出网前的最后一道翻译 + 防御。
///
/// `Context` 块是 JarvisAgent 内部类型（意图标签 / 工作目录 / 项目结构 / 全局记忆），
/// Anthropic 与 OpenAI 协议都没有这个类型，原样序列化出去会直接 400。
/// 所以构造请求体的最后一步，必须把它降级成普通 `Text` 块。
///
/// 降级后若仍残留 `Context` 块，说明有新的调用路径绕过了这里，直接 panic：
/// 宁可本地炸掉，也不要静默把非法字段发给模型服务商。
pub fn materialize_context_blocks_for_wire(messages: &mut [Message]) {
    for msg in messages.iter_mut() {
        let Message::User {
            content: Content::Multiple(blocks),
        } = msg
        else {
            continue;
        };
        for block in blocks.iter_mut() {
            if let ContentBlock::Context { text } = block {
                *block = ContentBlock::Text {
                    text: std::mem::take(text),
                };
            }
        }
    }

    for msg in messages.iter() {
        let Message::User {
            content: Content::Multiple(blocks),
        } = msg
        else {
            continue;
        };
        assert!(
            !blocks
                .iter()
                .any(|b| matches!(b, ContentBlock::Context { .. })),
            "Context 块未在出网前翻译成 Text，这会把非法协议字段发给模型服务商"
        );
    }
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
                            ContentBlock::Context { text } => {
                                // 正常路径下 Context 已在出网前被 materialize 成 Text，
                                // 这里兜底，防止其它调用方漏翻译时静默丢内容
                                text_parts.push(text.clone());
                                content_parts.push(OpenAIContentPart::Text { text: text.clone() });
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
                        reasoning_content: None,
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
                            reasoning_content: if backfill_assistant_reasoning_content && !thinking_segments.is_empty() {
                                Some(reasoning_content_from_thinking(&thinking_segments.join("\n")))
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::infra::types::models::{Content, ContentBlock, Message};

    fn user_with_ctx(ctx_text: &str, user_text: &str) -> Message {
        Message::User {
            content: Content::Multiple(vec![
                ContentBlock::Context {
                    text: ctx_text.to_string(),
                },
                ContentBlock::Text {
                    text: user_text.to_string(),
                },
            ]),
        }
    }

    #[test]
    fn materialize_turns_context_into_plain_text_for_anthropic() {
        let mut messages = vec![user_with_ctx("工作目录: E:\\x", "帮我看看这个 bug")];
        materialize_context_blocks_for_wire(&mut messages);

        let json = serde_json::to_value(&messages[0]).unwrap();
        let blocks = json["content"].as_array().unwrap();
        assert_eq!(blocks.len(), 2);
        assert_eq!(blocks[0]["type"], "text");
        assert_eq!(blocks[0]["text"], "工作目录: E:\\x");
        assert_eq!(blocks[1]["type"], "text");
        assert_eq!(blocks[1]["text"], "帮我看看这个 bug");
    }

    #[test]
    fn materialize_keeps_the_two_turns_byte_identical() {
        // prompt cache 的前提：上一轮的 Context 块必须原样出现在下一轮请求里
        let mut first = vec![user_with_ctx("项目结构: src/", "第一问")];
        materialize_context_blocks_for_wire(&mut first);
        let wire_first = serde_json::to_string(&first).unwrap();

        let mut second = vec![
            user_with_ctx("项目结构: src/", "第一问"),
            Message::User {
                content: Content::Single("第二问".to_string()),
            },
        ];
        materialize_context_blocks_for_wire(&mut second);
        let wire_second = serde_json::to_string(&second).unwrap();

        assert!(
            wire_second.starts_with(&wire_first[..wire_first.len() - 2]),
            "第二轮请求没有原样复现上一轮的 user 消息前缀：\n{}\n{}",
            wire_first,
            wire_second
        );
    }

    #[test]
    fn strip_context_blocks_keeps_only_real_conversation() {
        let messages = vec![
            user_with_ctx("工作目录: E:\\x", "第一句"),
            Message::User {
                content: Content::Single("第二句".to_string()),
            },
        ];
        let stripped = strip_context_blocks(&messages);

        assert_eq!(stripped.len(), 2);
        assert!(matches!(
            &stripped[0],
            Message::User {
                content: Content::Single(t)
            } if t == "第一句"
        ));
        assert!(matches!(
            &stripped[1],
            Message::User {
                content: Content::Single(t)
            } if t == "第二句"
        ));
        assert!(!serde_json::to_string(&stripped).unwrap().contains("context"));
    }

    #[test]
    fn openai_translation_keeps_context_as_text() {
        let messages = vec![user_with_ctx("项目结构: src/", "改一下这里")];
        let openai_msgs = translate_messages_to_openai("sys", &messages);
        let json = serde_json::to_string(&openai_msgs).unwrap();

        assert!(json.contains("项目结构: src/"));
        assert!(json.contains("改一下这里"));
        assert!(!json.contains("context"));
    }

    #[test]
    fn strip_unsigned_thinking_drops_empty_signature_blocks() {
        let signed = |signature: &str| Message::Assistant {
            content: Content::Multiple(vec![
                ContentBlock::Thinking {
                    thinking: "想一下".to_string(),
                    signature: signature.to_string(),
                },
                ContentBlock::Text {
                    text: "回答".to_string(),
                },
            ]),
        };
        let stripped = strip_unsigned_thinking_for_anthropic(&[signed(""), signed("sig-123")]);

        assert_eq!(stripped.len(), 2);
        let first = serde_json::to_string(&stripped[0]).unwrap();
        assert!(!first.contains("thinking"));
        assert!(first.contains("回答"));
        assert!(serde_json::to_string(&stripped[1]).unwrap().contains("sig-123"));
    }

    #[test]
    fn strip_unsigned_thinking_keeps_tool_use_pairs_intact() {
        // thinking 被摘掉后 tool_use 必须还在，否则下一条 tool_result 会失去配对
        let messages = vec![Message::Assistant {
            content: Content::Multiple(vec![
                ContentBlock::Thinking {
                    thinking: "调用工具".to_string(),
                    signature: String::new(),
                },
                ContentBlock::ToolUse {
                    id: "call_1".to_string(),
                    name: "ReadFile".to_string(),
                    input: serde_json::json!({}),
                },
            ]),
        }];
        let stripped = strip_unsigned_thinking_for_anthropic(&messages);
        let json = serde_json::to_string(&stripped).unwrap();

        assert!(json.contains("call_1"));
        assert!(!json.contains("thinking"));
    }

    #[test]
    fn strip_unsigned_thinking_drops_message_left_empty() {
        let messages = vec![Message::Assistant {
            content: Content::Multiple(vec![ContentBlock::Thinking {
                thinking: "只有思考块".to_string(),
                signature: String::new(),
            }]),
        }];
        assert!(strip_unsigned_thinking_for_anthropic(&messages).is_empty());
    }

    #[test]
    fn thinking_strip_policy_follows_provider() {
        // 真 Anthropic：必须剥（回传无签名 thinking 会被判 400）
        assert!(should_strip_unsigned_thinking("claude-sonnet-4-6", "https://api.anthropic.com", true));
        // DeepSeek（按 model 或 baseUrl 判定）：必须留，否则报 thinking must be passed back
        assert!(!should_strip_unsigned_thinking(
            "deepseek-flash",
            "https://api.deepseek.com/anthropic",
            true
        ));
        assert!(!should_strip_unsigned_thinking(
            "some-alias",
            "https://api.deepseek.com/anthropic",
            true
        ));
        // thinking 关闭时不存在链要求，剥不剥都无所谓（保持旧行为：剥）
        assert!(should_strip_unsigned_thinking("deepseek-flash", "https://api.deepseek.com", false));
    }

    #[test]
    fn deepseek_thinking_chain_survives_anthropic_exit() {
        // 复现线上 400 的场景：assistant 带 signature="" 的 thinking，
        // DeepSeek 端点要求原样回传，所以在该策略下不能被剥掉
        let messages = vec![Message::Assistant {
            content: Content::Multiple(vec![
                ContentBlock::Thinking {
                    thinking: "先调用目录工具".to_string(),
                    signature: String::new(),
                },
                ContentBlock::ToolUse {
                    id: "call_9".to_string(),
                    name: "GetToolCatalog".to_string(),
                    input: serde_json::json!({}),
                },
            ]),
        }];
        let keep = !should_strip_unsigned_thinking(
            "deepseek-flash",
            "https://api.deepseek.com/anthropic",
            true,
        );
        let sent = if keep {
            messages.clone()
        } else {
            strip_unsigned_thinking_for_anthropic(&messages)
        };
        let json = serde_json::to_string(&sent).unwrap();
        assert!(json.contains("thinking"), "DeepSeek 出口必须保留思考链：{}", json);
        assert!(json.contains("call_9"));
    }
}
