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
//! - 支持流内空闲超时（`STREAM_IDLE_TIMEOUT_SECS`）：连续无 SSE 帧即判定上游失联，
//!   优雅终止并保留已累积内容，不抛错、不清状态
//! - 工具调用的 `partial_json` 会累积到 `tool_input_buffers` 中，由调用方完成解析

use futures_util::StreamExt;
use serde_json::json;
use std::collections::HashMap;
use std::time::Duration;
use tauri::Emitter;

use crate::infra::debug_logger;
use crate::infra::types::models::*;
use crate::core::orchestration::agent_runs;

/// 流内空闲超时（秒）：连续该时长未收到任何 SSE 帧即判定上游失联。
///
/// 上游服务崩溃／半开连接时不会发 FIN/RST，TCP 认为连接仍在，`stream.next()`
/// 会永久 pending。此计时器以「收到任意 SSE 事件」为重置基准，因此不会误杀
/// 正常的长时间生成（只要还在吐字就一直续命）。
///
/// **取值取舍（实测反馈"等太久"后从 90s 下调）**：
/// 正常情况下流式输出很密集，几万 token 也会持续吐字，连续 30s 无帧基本可直接
/// 判定异常。但需注意：部分厂商（如 DeepSeek）在**思考阶段**就已建立 SSE 连接
/// 并推送 thinking 增量，若某个模型"静默思考"超过该阈值就会被误判。
/// 若实测遇到正常请求被截断，把此值调回 60~90 即可。
pub const STREAM_IDLE_TIMEOUT_SECS: u64 = 30;

/// 连续流错误容忍次数：超出即终止，避免底层流已死时无限空转
const STREAM_MAX_CONSECUTIVE_ERRORS: u32 = 5;

/// 判定本次流是否"零产出"：正文、思考、工具调用三者皆空。
///
/// 这是决定能否重试的**唯一**依据：已收到任何内容就不该重试
/// （正文会造成界面重复拼接，思考会破坏「思考链必须回传」的协议要求）。
pub(crate) fn is_zero_output(text_empty: bool, thinking_empty: bool, has_tool: bool) -> bool {
    text_empty && thinking_empty && !has_tool
}

/// 流式处理配置：控制事件发送行为
#[derive(Clone, Default)]
pub struct StreamConfig {
    /// 是否为子代理模式（子代理不发送 chat-content/chat-tool-start，
    /// chat-thinking 携带 isSubAgent 标记，不写 agent_runs 日志）
    pub is_subagent: bool,
    /// 注册表可选的缓存字段写法覆盖（`cacheUsageStyle`）；None = 按候选表自动探测
    pub cache_usage_style: Option<String>,
    /// 每收到一个 SSE 帧时触发，用于重置"等待提示"看门狗的静默计时。
    ///
    /// 用 `Arc<dyn Fn()>` 而非泛型，是为了让 `StreamConfig` 保持 `Clone`
    /// 且不污染 `process_stream` 的签名。不需要提示的调用点留 `None`。
    pub on_frame: Option<std::sync::Arc<dyn Fn() + Send + Sync>>,
}

fn looks_like_textual_tool_call(text: &str) -> bool {
    text.contains("<tool_call") || text.contains("<function=") || text.contains("<parameter=")
}

/// 尝试从 ExecuteTool 的参数中提取 ProposePlan 的 content 字段
/// ExecuteTool 的参数格式: {"name": "ProposePlan", "args": {"title": "...", "content": "..."}}
fn extract_deferred_propose_plan_content(partial_json: &str) -> Option<String> {
    // 快速过滤：必须是 ExecuteTool 且包含 ProposePlan
    if !partial_json.contains("ExecuteTool") && !partial_json.contains("\"name\"") {
        return None;
    }
    if !partial_json.contains("ProposePlan") {
        return None;
    }

    // 尝试解析为完整 JSON
    if let Ok(parsed) = serde_json::from_str::<serde_json::Value>(partial_json) {
        if let Some(name) = parsed.get("name").and_then(|v| v.as_str()) {
            if name == "ProposePlan" {
                if let Some(args) = parsed.get("args") {
                    if let Some(content) = args.get("content").and_then(|v| v.as_str()) {
                        return Some(content.to_string());
                    }
                }
            }
        }
        return None;
    }

    // 不完整 JSON：尝试提取 args.content 字段
    // 先检查 name 是否为 ProposePlan
    if let Some(name_start) = partial_json.find("\"name\"") {
        let after_name = &partial_json[name_start + 6..];
        if let Some(colon_pos) = after_name.find(':') {
            let value_part = after_name[colon_pos + 1..].trim_start();
            if value_part.starts_with('"') {
                let name_value = &value_part[1..];
                if let Some(end_quote) = name_value.find('"') {
                    let name = &name_value[..end_quote];
                    if name != "ProposePlan" {
                        return None;
                    }
                }
            }
        }
    }

    // 提取 args 中的 content
    if let Some(args_start) = partial_json.find("\"args\"") {
        let after_args = &partial_json[args_start + 6..];
        if let Some(content_start) = after_args.find("\"content\"") {
            let after_content_key = &after_args[content_start + 9..];
            if let Some(colon_pos) = after_content_key.find(':') {
                let value_part = after_content_key[colon_pos + 1..].trim_start();
                if value_part.starts_with('"') {
                    let after_quote = &value_part[1..];
                    let mut content = String::new();
                    let mut chars = after_quote.chars().peekable();
                    while let Some(c) = chars.next() {
                        if c == '\\' {
                            if let Some(next) = chars.next() {
                                match next {
                                    'n' => content.push('\n'),
                                    't' => content.push('\t'),
                                    'r' => content.push('\r'),
                                    '\\' => content.push('\\'),
                                    '"' => content.push('"'),
                                    '/' => content.push('/'),
                                    'u' => {
                                        let mut hex = String::new();
                                        for _ in 0..4 {
                                            if let Some(hc) = chars.next() {
                                                hex.push(hc);
                                            }
                                        }
                                        if let Ok(code) = u32::from_str_radix(&hex, 16) {
                                            if let Some(ch) = char::from_u32(code) {
                                                content.push(ch);
                                            }
                                        }
                                    }
                                    _ => content.push(next),
                                }
                            }
                        } else if c == '"' {
                            break;
                        } else {
                            content.push(c);
                        }
                    }
                    return Some(content);
                }
            }
        }
    }

    None
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
    /// 缓存命中 / 未命中的输入 token；None = 该 provider 未报告（≠ 0）
    pub cache_hit_tokens: Option<u64>,
    pub cache_miss_tokens: Option<u64>,
    /// 命中的字段名（排查"这家为什么显示未知"用）
    pub cache_source: Option<&'static str>,
    /// 原始 usage 原文（截断）：遇到未知写法时可直接从日志取出自诊断
    pub usage_raw: Option<String>,
    /// API 返回的终止原因（Anthropic: stop_reason, OpenAI: finish_reason）
    /// 常见值: "end_turn", "tool_use", "max_tokens", "stop", "length"
    pub stop_reason: Option<String>,
    /// 是否因流内空闲超时而终止（上游失联）。true 表示正常收尾路径未走完
    /// —— 既包括"零帧静默"，也包括"吐了半截后静默"。调用方据此结束本轮，
    /// **不要**用它判断能否重试（那是 `should_retry` 的职责）。
    pub idle_timed_out: bool,
    /// 本次是否允许自动重试一次。
    ///
    /// 只在**零产出**（一个有效帧都没收到，且无正文/思考/工具）时为 true。
    /// 已收到内容时为 false：界面已显示半截文本，重试会拼出重复内容，
    /// 且丢弃已收到的思考块会破坏「思考链必须回传」的协议要求。
    ///
    /// 历史教训：该判据曾与 `idle_timed_out` 合用一个字段，导致"吐了半截后静默"
    /// 时超时判定被一并置为 false —— 循环既不重试也不结束，run 永久卡死，
    /// 进而把会话锁成"正在执行"，用户说「继续」会被拒绝。
    pub should_retry: bool,
}

/// 安全截断（按字符，避免切断多字节字符）
fn truncate_sample(text: &str, max_chars: usize) -> String {
    if text.chars().count() <= max_chars {
        return text.to_string();
    }
    let head: String = text.chars().take(max_chars).collect();
    format!("{}…(truncated)", head)
}

/// 从 `content_block_delta` 的 `delta` 里取出 thinking 签名分片。
///
/// Anthropic 协议：thinking 块的签名不在 `content_block_start` 里，而是随后以
/// `{"type":"signature_delta","signature":"…"}` 逐片到达（真 Anthropic 是长 base64，
/// DeepSeek 的 /anthropic 端点用 UUID 字符串）。空签名视为"没有"，避免拼进空串。
fn thinking_signature_from_delta(delta: &serde_json::Value) -> Option<&str> {
    let signature = delta.get("signature")?.as_str()?;
    if signature.is_empty() {
        None
    } else {
        Some(signature)
    }
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
    let mut stop_reason: Option<String> = None;
    let mut logged_textual_tool_violation = false;
    // 缓存命中：跨事件合并（Anthropic 的 message_start / message_delta 会先后带 usage）
    let mut cache_usage = crate::infra::llm::usage::CacheUsage::default();
    let mut usage_raw: Option<String> = None;
    // 追踪 ProposePlan 工具调用的流式内容，用于实时推送到前端
    let mut propose_plan_stream_sent: HashMap<usize, usize> = HashMap::new();
    // 空闲超时：只统计"一个有效帧都没收到"的情况。
    // 已收到帧却中断时，部分内容已推送给前端并写入 live_content，
    // 此时重试会在界面上拼出重复文本，因此仅零产出才标记为可重试。
    let mut idle_timed_out = false;
    let mut should_retry = false;
    let mut received_event = false;
    let mut consecutive_errors: u32 = 0;

    let logger = debug_logger::debug_logger();
    // SSE 聚合按 (session, agent_type, loop) 归档，才能落到对应循环卡上
    let agent_type = if config.is_subagent { "SUB" } else { "MAIN" };
    if !config.is_subagent {
        let _ = app.emit(
            "chat-turn-start",
            json!({ "sessionId": sid, "loopCount": loop_count }),
        );
    }

    loop {
        // 每次迭代重建计时器 = 每收到一个 SSE 事件即重置空闲计时。
        // 上游静默超过阈值即判定失联，优雅终止（保留已累积结果）。
        let idle_deadline = tokio::time::sleep(Duration::from_secs(STREAM_IDLE_TIMEOUT_SECS));
        tokio::pin!(idle_deadline);

        let event_result = tokio::select! {
            next = stream.next() => next,
            _ = &mut idle_deadline => {
                println!(
                    "[JARVIS] SSE 流空闲超过 {}s，判定上游服务已失联，优雅终止本轮接收",
                    STREAM_IDLE_TIMEOUT_SECS
                );
                // 两个含义必须分开：
                // - idle_timed_out：计时器确实触发了 → 调用方据此结束本轮（含"吐了半截后静默"）
                // - should_retry：只有零产出时才允许重试
                // 曾把二者合一，导致收到过帧时超时判定被置 false → run 卡死。
                idle_timed_out = true;
                should_retry = !received_event;
                logger.log_sse_event(
                    sid,
                    agent_type,
                    loop_count,
                    &format!("[idle-timeout] {}s 无 SSE 帧", STREAM_IDLE_TIMEOUT_SECS),
                );
                break;
            }
            _ = cancel_token.cancelled() => {
                println!("[JARVIS] 流式接收中途被用户取消");
                break;
            }
        };
        let Some(event_result) = event_result else {
            break;
        };
        let event = match event_result {
            Ok(e) => {
                consecutive_errors = 0;
                e
            }
            Err(e) => {
                // 不再静默 continue：底层流持续报错时无限空转且不留痕，
                // 会掩盖超时问题本身。有限次容错后终止。
                consecutive_errors += 1;
                println!(
                    "[JARVIS] SSE 流错误（连续第 {}/{} 次）: {}",
                    consecutive_errors, STREAM_MAX_CONSECUTIVE_ERRORS, e
                );
                if consecutive_errors >= STREAM_MAX_CONSECUTIVE_ERRORS {
                    // 流已判定不可用，与空闲超时同样按"中断"收尾（而非静默 continue）
                    idle_timed_out = true;
                    should_retry = !received_event;
                    break;
                }
                continue;
            }
        };
        received_event = true;
        // 通知等待提示看门狗：有数据到达，重置静默计时。
        // 放在解析之前——即使该帧解析不出内容，"链路仍在流动"这一事实也成立。
        if let Some(on_frame) = &config.on_frame {
            on_frame();
        }
        let data = event.data;
        // 记录 SSE（默认只累加计数，收尾时落一条 sse_summary）
        logger.log_sse_event(sid, agent_type, loop_count, &data);
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
                // 缓存命中字段（各家写法不同，交给归一化模块按候选表探测）
                cache_usage = crate::infra::llm::usage::merge_cache_usage(
                    cache_usage,
                    crate::infra::llm::usage::extract_cache_usage_with_style(usage, config.cache_usage_style.as_deref()),
                );
                usage_raw = Some(truncate_sample(&usage.to_string(), 600));
            }

            if let Some(choices) = json_val["choices"].as_array() {
                if let Some(first) = choices.first() {
                    // 提取终止原因（stop / length / tool_calls 等）
                    if let Some(fr) = first["finish_reason"].as_str() {
                        stop_reason = Some(fr.to_string());
                    }
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
                                        logger.log_protocol_violation(
                                            sid,
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
                                            // 实时提取 ProposePlan 的 content 并推送到前端（通过 ExecuteTool 调用）
                                            if let Some(ContentBlock::ToolUse { name, .. }) = current_blocks.get(*block_index) {
                                                let content = if name == "ExecuteTool" {
                                                    extract_deferred_propose_plan_content(buf)
                                                } else {
                                                    None
                                                };
                                                if let Some(content) = content {
                                                    let sent_len = propose_plan_stream_sent.get(block_index).copied().unwrap_or(0);
                                                    if content.len() > sent_len {
                                                        let new_chunk = &content[sent_len..];
                                                        let _ = app.emit(
                                                            "plan-proposal-stream",
                                                            json!({
                                                                "content": new_chunk,
                                                                "sessionId": sid
                                                            }),
                                                        );
                                                        propose_plan_stream_sent.insert(*block_index, content.len());
                                                    }
                                                }
                                            }
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
                        cache_usage = crate::infra::llm::usage::merge_cache_usage(
                            cache_usage,
                            crate::infra::llm::usage::extract_cache_usage_with_style(usage, config.cache_usage_style.as_deref()),
                        );
                        usage_raw = Some(truncate_sample(&usage.to_string(), 600));
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
                        cache_usage = crate::infra::llm::usage::merge_cache_usage(
                            cache_usage,
                            crate::infra::llm::usage::extract_cache_usage_with_style(usage, config.cache_usage_style.as_deref()),
                        );
                        usage_raw = Some(truncate_sample(&usage.to_string(), 600));
                    }
                    // 提取终止原因（end_turn / max_tokens / tool_use 等）
                    if let Some(sr) = json_val["delta"]["stop_reason"].as_str() {
                        stop_reason = Some(sr.to_string());
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
                                        logger.log_protocol_violation(
                                            sid,
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
                            ContentBlock::Thinking { thinking, signature } => {
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
                                // Anthropic 协议把 thinking 的签名放在**独立的** signature_delta 分片里
                                // （content_block_start 里的 thinking 块不含 signature）。
                                // 不接住它，回放历史时该块就是"无签名"，而 Anthropic 协议要求
                                // thinking 块原样回传：真 Anthropic 会直接 400，DeepSeek 的
                                // /anthropic 端点（UUID 签名）会报
                                // `content[].thinking in the thinking mode must be passed back to the API`。
                                if let Some(sig) = thinking_signature_from_delta(delta) {
                                    signature.push_str(sig);
                                }
                            }
                            ContentBlock::ToolUse { name, .. } => {
                                if let Some(partial) = delta["partial_json"].as_str() {
                                    if let Some(buf) = tool_input_buffers.get_mut(&index) {
                                        buf.push_str(partial);
                                        // 实时提取 ProposePlan 的 content 并推送到前端（通过 ExecuteTool 调用）
                                        let content = if name == "ExecuteTool" {
                                            extract_deferred_propose_plan_content(buf)
                                        } else {
                                            None
                                        };
                                        if let Some(content) = content {
                                            let sent_len = propose_plan_stream_sent.get(&index).copied().unwrap_or(0);
                                            if content.len() > sent_len {
                                                let new_chunk = &content[sent_len..];
                                                let _ = app.emit(
                                                    "plan-proposal-stream",
                                                    json!({
                                                        "content": new_chunk,
                                                        "sessionId": sid
                                                    }),
                                                );
                                                propose_plan_stream_sent.insert(index, content.len());
                                            }
                                        }
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

    // 流结束时把本 loop 的 SSE 聚合落盘（MAIN 路径 log_response 也会刷一次，幂等）
    logger.flush_sse_summary(sid, agent_type, loop_count);

    // 零产出判据在此一次算清：正文/思考/工具调用三者皆空。
    // 放在 stream 层是因为这里同时掌握"是否收到过帧"与"解析出什么"，
    // 调用方只需读一个布尔值，避免再次翻车（见 should_retry 字段注释）。
    let zero_output = is_zero_output(
        current_text_this_turn.trim().is_empty(),
        current_thinking_this_turn.trim().is_empty(),
        turn_has_tool,
    );
    let should_retry = should_retry && zero_output;

    StreamResult {
        blocks: current_blocks,
        tool_input_buffers,
        text: current_text_this_turn,
        thinking: current_thinking_this_turn,
        has_tool: turn_has_tool,
        input_tokens: req_input_tokens,
        output_tokens: req_output_tokens,
        cache_hit_tokens: cache_usage.hit,
        cache_miss_tokens: cache_usage.miss,
        cache_source: cache_usage.is_known().then_some(cache_usage.source),
        usage_raw,
        stop_reason,
        idle_timed_out,
        should_retry,
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

#[cfg(test)]
mod signature_delta_tests {
    use super::thinking_signature_from_delta;
    use serde_json::json;

    #[test]
    fn captures_anthropic_signature_delta() {
        // 真 Anthropic：长 base64 签名
        let delta = json!({
            "type": "signature_delta",
            "signature": "EqQBCgIYAhIM1gbcDa9GJwZA2b3hGgxBdjrkzLoky3dl1pki"
        });
        assert_eq!(
            thinking_signature_from_delta(&delta),
            Some("EqQBCgIYAhIM1gbcDa9GJwZA2b3hGgxBdjrkzLoky3dl1pki")
        );
    }

    #[test]
    fn captures_deepseek_uuid_signature() {
        // DeepSeek 的 /anthropic 端点用 UUID 形式的签名，必须原样回传
        let delta = json!({
            "type": "signature_delta",
            "signature": "3f7c1f5e-2b6a-4f1e-9a5d-0b2c8e7d4a11"
        });
        assert_eq!(
            thinking_signature_from_delta(&delta),
            Some("3f7c1f5e-2b6a-4f1e-9a5d-0b2c8e7d4a11")
        );
    }

    #[test]
    fn thinking_delta_without_signature_is_ignored() {
        // 普通的 thinking_delta 不带 signature，不能当成签名
        assert!(thinking_signature_from_delta(&json!({
            "type": "thinking_delta",
            "thinking": "先看看目录"
        }))
        .is_none());
        // 空签名不拼进块里（否则等于把"无签名"伪装成"有签名"）
        assert!(thinking_signature_from_delta(&json!({
            "type": "signature_delta",
            "signature": ""
        }))
        .is_none());
        assert!(thinking_signature_from_delta(&json!({})).is_none());
    }
}

#[cfg(test)]
mod idle_timeout_tests {
    use super::{STREAM_IDLE_TIMEOUT_SECS, STREAM_MAX_CONSECUTIVE_ERRORS};
    use futures_util::StreamExt;
    use std::time::Duration;

    /// 空闲阈值必须是有限正数：0 会误杀一切正常流，过大则失去兜底意义。
    ///
    /// 这里刻意**不锁死具体数值**（曾是仅断言 == 90 的脆弱测试）：
    /// 该值会随体验调优变化（实测反馈"等太久"后已从 90s 下调到 30s），
    /// 断言区间既能防住"误改成 0 或超大值"，又不会阻碍合理调优。
    #[test]
    fn idle_timeout_is_a_sane_finite_bound() {
        assert!(
            STREAM_IDLE_TIMEOUT_SECS >= 10,
            "过小会误杀正常生成：{}",
            STREAM_IDLE_TIMEOUT_SECS
        );
        assert!(
            STREAM_IDLE_TIMEOUT_SECS <= 300,
            "过大等于没有兜底：{}",
            STREAM_IDLE_TIMEOUT_SECS
        );
    }

    /// 连续错误容忍次数必须有限，否则底层流已死时会无限空转（原实现即如此）。
    #[test]
    fn consecutive_error_tolerance_is_bounded() {
        assert!(STREAM_MAX_CONSECUTIVE_ERRORS > 0);
        assert!(
            STREAM_MAX_CONSECUTIVE_ERRORS <= 10,
            "容忍次数过大等于没有兜底"
        );
    }

    /// 与 `process_stream` 内部相同的 select 形态：
    /// 每轮重建 sleep，`next` 先就绪则不让计时器影响取值。
    /// 返回 `Some(())` 表示取到值，`None` 表示空闲超时触发。
    async fn race_next_against_idle<S>(stream: &mut S, idle: Duration) -> Option<()>
    where
        S: futures_util::Stream + Unpin,
    {
        let deadline = tokio::time::sleep(idle);
        tokio::pin!(deadline);
        tokio::select! {
            item = stream.next() => item.map(|_| ()),
            _ = &mut deadline => None,
        }
    }

    /// 永不产出任何元素的流，模拟"上游收下请求后彻底静默"（TCP 半开连接下 read 永久 pending）。
    ///
    /// 必须用 `pending()` 而非 `iter(large_range)`：后者内部有缓冲队列，
    /// 第一次 poll 就能立刻拿到元素，无法复现"静默"。
    fn silent_stream() -> impl futures_util::Stream<Item = u8> + Unpin {
        futures_util::stream::pending::<u8>()
    }

    /// 回归防护（核心）：上游不发任何数据时必须能被空闲计时器打断。
    ///
    /// 用极短的空闲窗口替代 90s，验证的是**机制**而非具体数值：
    /// 若 select 里漏掉计时分支，本用例会一直挂住直到外层测试超时，即复现线上
    /// 「DeepSeek 崩溃后界面无限转圈」的原始缺陷。
    #[tokio::test]
    async fn idle_timeout_fires_when_upstream_goes_silent() {
        let mut stream = silent_stream();
        let result = tokio::time::timeout(
            Duration::from_secs(5),
            race_next_against_idle(&mut stream, Duration::from_millis(30)),
        )
        .await
        .expect("空闲超时未触发：select 缺少计时分支，等价于线上无限等待");
        assert!(result.is_none(), "上游静默时必须由计时器胜出");
    }

    /// 反向防护：流里有数据时必须取到数据，不能让计时器把正常流误杀。
    #[tokio::test]
    async fn data_wins_over_idle_timer() {
        let mut stream = futures_util::stream::iter(vec![7u8]);
        let idle = Duration::from_secs(30);
        let won = tokio::time::timeout(idle, race_next_against_idle(&mut stream, idle))
            .await
            .expect("数据已就绪，不应等满空闲窗口");
        assert!(won.is_some(), "有数据时不得被判为空闲超时");
    }

    /// 流正常结束（EOF）时 `next()` 就绪并返回 None，不是计时器胜出。
    /// 这正是 `process_stream` 用 `received_event` 区分「正常收尾」与「空闲超时」的依据。
    #[tokio::test]
    async fn eof_resolves_via_next_not_via_timer() {
        let mut stream = futures_util::stream::empty::<u8>();
        let won: Option<()> = tokio::time::timeout(
            Duration::from_secs(5),
            race_next_against_idle(&mut stream, Duration::from_millis(30)),
        )
        .await
        .expect("空流应立即返回 None，而不是挂起");
        assert!(won.is_none());
    }
}
