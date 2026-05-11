//! # stream.rs — SSE 流式响应处理
//!
//! 解析 LLM 返回的 SSE 流式响应，支持 Anthropic 和 OpenAI 两种格式。
//! 实时提取文本、思考过程、工具调用等 ContentBlock，并通过 Tauri 事件推送到前端。
//!
//! ## 关键导出
//! - `process_stream()`: 解析 SSE 流，返回内容块、工具输入缓冲、token 统计等
//!
//! ## 依赖
//! - Internal: `crate::core::orchestration::agent_runs`, `crate::infra::debug_logger::DebugLogger`, `crate::infra::types::models`
//! - External: `futures_util`, `serde_json`, `eventsource_stream`, `tauri`
//!
//! ## 约束
//! - 支持中途取消（通过 `CancellationToken`）
//! - 工具调用的 `partial_json` 会累积到 `tool_input_buffers` 中，由调用方完成解析

use futures_util::StreamExt;
use serde_json::json;
use std::collections::HashMap;
use tauri::Emitter;

use crate::infra::debug_logger::DebugLogger;
use crate::infra::types::models::*;
use crate::core::orchestration::agent_runs;

/// 流式处理配置：控制事件发送行为
#[derive(Clone, Copy)]
pub struct StreamConfig {
    /// 是否为子代理模式（子代理不发送 chat-content/chat-tool-start，
    /// chat-thinking 携带 isSubAgent 标记，不写 agent_runs 日志）
    pub is_subagent: bool,
}

impl Default for StreamConfig {
    fn default() -> Self {
        Self { is_subagent: false }
    }
}

fn looks_like_textual_tool_call(text: &str) -> bool {
    text.contains("<tool_call") || text.contains("<function=") || text.contains("<parameter=")
}

/// 流式处理结果
pub struct StreamResult {
    pub blocks: Vec<ContentBlock>,
    pub tool_input_buffers: HashMap<usize, String>,
    pub text: String,
    pub thinking: String,
    pub has_tool: bool,
    pub input_tokens: u64,
    pub output_tokens: u64,
}

pub async fn process_stream(
    stream: &mut (impl StreamExt<
        Item = Result<
            eventsource_stream::Event,
            eventsource_stream::EventStreamError<reqwest::Error>,
        >,
    > + Unpin),
    is_openai: bool,
    app: &tauri::AppHandle,
    sid: &str,
    run_id: &str,
    loop_count: usize,
    cancel_token: &tokio_util::sync::CancellationToken,
    config: StreamConfig,
) -> StreamResult {
    let mut current_blocks: Vec<ContentBlock> = Vec::new();
    let mut tool_input_buffers: HashMap<usize, String> = HashMap::new();
    let mut openai_tool_block_map: HashMap<usize, usize> = HashMap::new();
    let mut current_text_this_turn = String::new();
    let mut current_thinking_this_turn = String::new();
    let mut turn_has_tool = false;
    let mut req_input_tokens: u64 = 0;
    let mut req_output_tokens: u64 = 0;
    let mut logged_textual_tool_violation = false;

    let logger = DebugLogger::new();
    if !config.is_subagent {
        let _ = app.emit(
            "chat-turn-start",
            json!({ "sessionId": sid, "loopCount": loop_count }),
        );
    }

    loop {
        let event_result = tokio::select! {
            next = stream.next() => next,
            _ = cancel_token.cancelled() => {
                println!("[JARVIS] 流式接收中途被用户取消");
                break;
            }
        };
        let Some(event_result) = event_result else {
            break;
        };
        let event = match event_result {
            Ok(e) => e,
            Err(_) => continue,
        };
        let data = event.data;
        // 记录原始 SSE 事件到调试日志
        logger.log_raw_sse_event(loop_count, &data);
        if data == "[DONE]" {
            break;
        }
        let json_val: serde_json::Value = serde_json::from_str(&data).unwrap_or(json!({}));

        if is_openai {
            if let Some(usage) = json_val.get("usage") {
                if let Some(in_toks) = usage.get("prompt_tokens").and_then(|v| v.as_u64()) {
                    req_input_tokens += in_toks;
                }
                if let Some(out_toks) = usage.get("completion_tokens").and_then(|v| v.as_u64()) {
                    req_output_tokens += out_toks;
                }
            }

            if let Some(choices) = json_val["choices"].as_array() {
                if let Some(first) = choices.first() {
                    if let Some(delta) = first.get("delta") {
                        if let Some(t) = delta["content"].as_str() {
                            if !t.is_empty() {
                                let is_text = matches!(
                                    current_blocks.last(),
                                    Some(ContentBlock::Text { .. })
                                );
                                if !is_text {
                                    current_blocks.push(ContentBlock::Text {
                                        text: String::new(),
                                    });
                                }
                                if let Some(ContentBlock::Text { text }) = current_blocks.last_mut()
                                {
                                    text.push_str(t);
                                    current_text_this_turn.push_str(t);
                                    if !logged_textual_tool_violation
                                        && looks_like_textual_tool_call(&current_text_this_turn)
                                    {
                                        logged_textual_tool_violation = true;
                                        let agent_type = if config.is_subagent {
                                            "SUBAGENT"
                                        } else {
                                            "MAIN"
                                        };
                                        logger.log_textual_tool_protocol_violation(
                                            agent_type,
                                            loop_count,
                                            &current_text_this_turn,
                                        );
                                    }
                                    if !config.is_subagent {
                                        let _ = app.emit(
                                            "chat-content",
                                            json!({ "content": t, "sessionId": sid, "loopCount": loop_count }),
                                        );
                                        agent_runs::append_content(app, run_id, t, loop_count);
                                    }
                                }
                            }
                        }
                        if let Some(t) = delta["reasoning_content"].as_str() {
                            if !t.is_empty() {
                                let is_thinking = matches!(
                                    current_blocks.last(),
                                    Some(ContentBlock::Thinking { .. })
                                );
                                if !is_thinking {
                                    current_blocks.push(ContentBlock::Thinking {
                                        thinking: String::new(),
                                        signature: String::new(),
                                    });
                                }
                                if let Some(ContentBlock::Thinking { thinking, .. }) =
                                    current_blocks.last_mut()
                                {
                                    thinking.push_str(t);
                                    current_thinking_this_turn.push_str(t);
                                    let _ = app.emit(
                                        "chat-thinking",
                                        if config.is_subagent {
                                            json!({ "content": t, "sessionId": sid, "isSubAgent": true })
                                        } else {
                                            json!({ "content": t, "sessionId": sid, "loopCount": loop_count })
                                        },
                                    );
                                    if !config.is_subagent {
                                        agent_runs::append_thinking(app, run_id, t, loop_count);
                                    }
                                }
                            }
                        }
                        if let Some(tool_calls) = delta["tool_calls"].as_array() {
                            for tc in tool_calls {
                                let tool_call_index = tc["index"].as_u64().unwrap_or(0) as usize;

                                if !openai_tool_block_map.contains_key(&tool_call_index) {
                                    let id = tc["id"].as_str().unwrap_or("").to_string();
                                    let name =
                                        tc["function"]["name"].as_str().unwrap_or("").to_string();
                                    current_blocks.push(ContentBlock::ToolUse {
                                        id: id.clone(),
                                        name: name.clone(),
                                        input: json!({}),
                                    });
                                    let block_index = current_blocks.len() - 1;
                                    openai_tool_block_map.insert(tool_call_index, block_index);
                                    tool_input_buffers.insert(block_index, String::new());
                                    turn_has_tool = true;
                                    if !config.is_subagent {
                                        let _ = app.emit(
                                            "chat-tool-start",
                                            json!({
                                                "sessionId": sid,
                                                "loopCount": loop_count,
                                                "toolCallId": id,
                                                "tool": name
                                            }),
                                        );
                                        agent_runs::append_tool_log(
                                            app,
                                            run_id,
                                            "\n> 工具参数接收中\n",
                                            loop_count,
                                        );
                                    }
                                }

                                if let Some(args) = tc["function"]["arguments"].as_str() {
                                    if let Some(block_index) =
                                        openai_tool_block_map.get(&tool_call_index)
                                    {
                                        if let Some(buf) = tool_input_buffers.get_mut(block_index) {
                                            buf.push_str(args);
                                        }
                                    }
                                }
                            }
                        }
                    }
                }
            }
        } else {
            match json_val["type"].as_str().unwrap_or("") {
                "message_start" => {
                    if let Some(usage) = json_val.get("message").and_then(|m| m.get("usage")) {
                        req_input_tokens += usage
                            .get("input_tokens")
                            .and_then(|v| v.as_u64())
                            .unwrap_or(0);
                    }
                }
                "message_delta" => {
                    if let Some(usage) = json_val.get("usage") {
                        if let Some(in_toks) = usage.get("input_tokens").and_then(|v| v.as_u64()) {
                            req_input_tokens += in_toks;
                        }
                        if let Some(out_toks) = usage.get("output_tokens").and_then(|v| v.as_u64())
                        {
                            req_output_tokens += out_toks;
                        }
                    }
                }
                "content_block_start" => {
                    let block = &json_val["content_block"];
                    match block["type"].as_str().unwrap_or("") {
                        "text" => current_blocks.push(ContentBlock::Text {
                            text: String::new(),
                        }),
                        "thinking" => current_blocks.push(ContentBlock::Thinking {
                            thinking: String::new(),
                            signature: block["signature"].as_str().unwrap_or("").to_string(),
                        }),
                        "tool_use" => {
                            let tool_name = block["name"].as_str().unwrap_or("").to_string();
                            current_blocks.push(ContentBlock::ToolUse {
                                id: block["id"].as_str().unwrap_or("").to_string(),
                                name: tool_name.clone(),
                                input: json!({}),
                            });
                            tool_input_buffers.insert(current_blocks.len() - 1, String::new());
                            turn_has_tool = true;
                            if !config.is_subagent {
                                let _ = app.emit(
                                    "chat-tool-start",
                                    json!({
                                        "sessionId": sid,
                                        "loopCount": loop_count,
                                        "toolCallId": block["id"].as_str().unwrap_or(""),
                                        "tool": tool_name
                                    }),
                                );
                                agent_runs::append_tool_log(
                                    app,
                                    run_id,
                                    "\n> 工具参数接收中\n",
                                    loop_count,
                                );
                            }
                        }
                        _ => {}
                    }
                }
                "content_block_delta" => {
                    let index = json_val["index"].as_u64().unwrap_or(0) as usize;
                    let delta = &json_val["delta"];
                    if let Some(block) = current_blocks.get_mut(index) {
                        match block {
                            ContentBlock::Text { text } => {
                                if let Some(t) = delta["text"].as_str() {
                                    text.push_str(t);
                                    current_text_this_turn.push_str(t);
                                    if !logged_textual_tool_violation
                                        && looks_like_textual_tool_call(&current_text_this_turn)
                                    {
                                        logged_textual_tool_violation = true;
                                        let agent_type = if config.is_subagent {
                                            "SUBAGENT"
                                        } else {
                                            "MAIN"
                                        };
                                        logger.log_textual_tool_protocol_violation(
                                            agent_type,
                                            loop_count,
                                            &current_text_this_turn,
                                        );
                                    }
                                    if !config.is_subagent {
                                        let _ = app.emit(
                                            "chat-content",
                                            json!({ "content": t, "sessionId": sid, "loopCount": loop_count }),
                                        );
                                        agent_runs::append_content(app, run_id, t, loop_count);
                                    }
                                }
                            }
                            ContentBlock::Thinking { thinking, .. } => {
                                if let Some(t) = delta["thinking"].as_str() {
                                    thinking.push_str(t);
                                    current_thinking_this_turn.push_str(t);
                                    let _ = app.emit(
                                        "chat-thinking",
                                        if config.is_subagent {
                                            json!({ "content": t, "sessionId": sid, "isSubAgent": true })
                                        } else {
                                            json!({ "content": t, "sessionId": sid, "loopCount": loop_count })
                                        },
                                    );
                                    if !config.is_subagent {
                                        agent_runs::append_thinking(app, run_id, t, loop_count);
                                    }
                                }
                            }
                            ContentBlock::ToolUse { .. } => {
                                if let Some(partial) = delta["partial_json"].as_str() {
                                    if let Some(buf) = tool_input_buffers.get_mut(&index) {
                                        buf.push_str(partial);
                                    }
                                }
                            }
                            _ => {}
                        }
                    }
                }
                _ => {}
            }
        }
    }

    // 兼容不支持原生 tool_calls 的模型：从文本中提取 <tool_call> XML 块
    if !turn_has_tool && current_text_this_turn.contains("<tool_call") {
        let parsed = parse_textual_tool_calls(&current_text_this_turn);
        if !parsed.is_empty() {
            // 移除原有的纯文本块，替换为解析出的工具调用块
            current_blocks.retain(|b| !matches!(b, ContentBlock::Text { .. }));
            for (i, (name, input_json)) in parsed.iter().enumerate() {
                let id = format!("call_{}", uuid::Uuid::new_v4().simple().to_string()[..8].to_string());
                current_blocks.push(ContentBlock::ToolUse {
                    name: name.clone(),
                    input: input_json.clone(),
                    id: id.clone(),
                });
                tool_input_buffers.insert(i, input_json.to_string());
            }
            turn_has_tool = true;
        }
    }

    StreamResult {
        blocks: current_blocks,
        tool_input_buffers,
        text: current_text_this_turn,
        thinking: current_thinking_this_turn,
        has_tool: turn_has_tool,
        input_tokens: req_input_tokens,
        output_tokens: req_output_tokens,
    }
}

/// 从模型输出的文本中解析 <tool_call> XML 块，转为 (name, input_json) 列表
fn parse_textual_tool_calls(text: &str) -> Vec<(String, serde_json::Value)> {
    let mut results = Vec::new();
    let mut rest = text;

    while let Some(tc_start) = rest.find("<tool_call>") {
        let after_start = &rest[tc_start + "<tool_call>".len()..];
        let tc_end = match after_start.find("</tool_call>") {
            Some(pos) => pos,
            None => break,
        };
        let tc_body = &after_start[..tc_end].trim();
        rest = &after_start[tc_end + "</tool_call>".len()..];

        // 解析 <function=NAME>
        let fn_start = match tc_body.find("<function=") {
            Some(pos) => pos + "<function=".len(),
            None => continue,
        };
        let fn_body = &tc_body[fn_start..];
        let fn_end = match fn_body.find('>') {
            Some(pos) => pos,
            None => continue,
        };
        let fn_name = fn_body[..fn_end].trim().to_string();
        let after_fn = &fn_body[fn_end + 1..];

        // 找到 </function> 来界定参数范围
        let fn_close = match after_fn.find("</function>") {
            Some(pos) => pos,
            None => continue,
        };
        let params_text = &after_fn[..fn_close];

        // 解析 <parameter=KEY>VALUE</parameter>
        let mut input_map = serde_json::Map::new();
        let mut param_rest = params_text;
        while let Some(p_start) = param_rest.find("<parameter=") {
            let after_p_start = &param_rest[p_start + "<parameter=".len()..];
            let p_name_end = match after_p_start.find('>') {
                Some(pos) => pos,
                None => break,
            };
            let p_name = after_p_start[..p_name_end].trim().to_string();
            let after_p_name = &after_p_start[p_name_end + 1..];
            let p_value_end = match after_p_name.find("</parameter>") {
                Some(pos) => pos,
                None => break,
            };
            let p_value = after_p_name[..p_value_end].trim().to_string();
            param_rest = &after_p_name[p_value_end + "</parameter>".len()..];

            // 尝试将值解析为 JSON（数字/布尔/字符串），失败则保持字符串
            let value = if let Ok(n) = p_value.parse::<i64>() {
                serde_json::Value::Number(n.into())
            } else if let Ok(b) = p_value.parse::<bool>() {
                serde_json::Value::Bool(b)
            } else if (p_value.starts_with('{') && p_value.ends_with('}'))
                || (p_value.starts_with('[') && p_value.ends_with(']'))
            {
                serde_json::from_str(&p_value).unwrap_or(serde_json::Value::String(p_value))
            } else {
                serde_json::Value::String(p_value)
            };
            input_map.insert(p_name, value);
        }

        results.push((fn_name, serde_json::Value::Object(input_map)));
    }

    results
}
