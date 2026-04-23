
use crate::core::models::{
    Message, Content, ContentBlock, OpenAIMessage, OpenAITool, OpenAIFunctionDefinition, OpenAIToolCall, OpenAIFunctionCall
};

/// 将 Anthropic 内部格式的 Message 翻译为 OpenAI 格式
pub fn translate_messages_to_openai(system: &str, messages: &[Message]) -> Vec<OpenAIMessage> {
    let mut openai_msgs = Vec::new();

    // 1. 系统提示词转换
    if !system.is_empty() {
        openai_msgs.push(OpenAIMessage::System {
            content: system.to_string(),
        });
    }

    // 2. 遍历消息记录
    for msg in messages {
        match msg {
            Message::User { content } => {
                match content {
                    Content::Single(text) => {
                        openai_msgs.push(OpenAIMessage::User {
                            content: text.clone(),
                        });
                    }
                    Content::Multiple(blocks) => {
                        // User message might contain tool_result in Anthropic format
                        for block in blocks {
                            match block {
                                ContentBlock::Text { text } => {
                                    openai_msgs.push(OpenAIMessage::User {
                                        content: text.clone(),
                                    });
                                }
                                ContentBlock::ToolResult { tool_use_id, content } => {
                                    openai_msgs.push(OpenAIMessage::Tool {
                                        content: content.clone(),
                                        tool_call_id: tool_use_id.clone(),
                                    });
                                }
                                _ => {}
                            }
                        }
                    }
                }
            }
            Message::Assistant { content } => {
                match content {
                    Content::Single(text) => {
                        openai_msgs.push(OpenAIMessage::Assistant {
                            content: Some(text.clone()),
                            tool_calls: None,
                        });
                    }
                    Content::Multiple(blocks) => {
                        let mut text_content = String::new();
                        let mut tool_calls = Vec::new();

                        for block in blocks {
                            match block {
                                ContentBlock::Text { text } => {
                                    text_content.push_str(text);
                                }
                                ContentBlock::ToolUse { id, name, input } => {
                                    tool_calls.push(OpenAIToolCall {
                                        id: id.clone(),
                                        r#type: "function".to_string(),
                                        function: OpenAIFunctionCall {
                                            name: name.clone(),
                                            arguments: serde_json::to_string(&input).unwrap_or_else(|_| "{}".to_string()),
                                        },
                                    });
                                }
                                _ => {}
                            }
                        }

                        if !text_content.is_empty() || !tool_calls.is_empty() {
                            openai_msgs.push(OpenAIMessage::Assistant {
                                content: if text_content.is_empty() { None } else { Some(text_content) },
                                tool_calls: if tool_calls.is_empty() { None } else { Some(tool_calls) },
                            });
                        }
                    }
                }
            }
        }
    }

    openai_msgs
}

/// 将 Anthropic 内部格式的工具定义翻译为 OpenAI 格式
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
