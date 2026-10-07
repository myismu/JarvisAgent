//! # stream_parse.rs — SSE 帧协议解析（模型接入层）
//!
//! 职责单一：**把一帧 SSE 的 JSON 原文翻译成统一的 `ProtocolEvent` 事件**。
//! 这里是全项目唯一认识"线上协议长什么样"的解析模块：
//! - Anthropic 事件流：`message_start` / `content_block_start` / `content_block_delta` / `message_delta`
//! - OpenAI Chat Completions 流：`choices[0].delta`（content / reasoning_content / tool_calls）
//!
//! ## 为什么是"纯函数"
//!
//! `parse_frame` 不持有任何状态、不做 IO、不发事件——同样的帧输入永远得到同样的输出
//! （Java 类比：一个无副作用的静态方法，输入 JsonNode，输出事件列表）。
//! 状态（累积中的内容块、工具参数缓冲）全部由调用方 `core/agent/stream.rs` 持有；
//! 前端推送、取消、超时等副作用也全在调用方。这样协议翻译可以单独逐帧测试，
//! 不需要 mock 任何运行时环境。
//!
//! ## 关键导出
//! - `ProtocolEvent`: 统一事件枚举（运行时只见它，不见协议）
//! - `parse_frame`: 一帧 → 若干事件（OpenAI 一帧可同时带 usage 和 delta，多事件是正常的）
//! - `looks_like_textual_tool_call` / `parse_textual_tool_calls`: 模型输出文本的协议级解析
//!   （部分模型不支持原生 tool_calls，会把工具调用写成正文的 XML 块）
//!
//! ## 拆分说明
//! 逻辑自 `core/agent/stream.rs` 平移（2026-09-19），行为保持不变；
//! 唯一披露的变化见 `doc/模型接入层-流式解析拆分方案.md` §3。

use crate::infra::llm::api_format::ApiFormat;

/// 统一事件枚举 —— 会话运行时只消费这个，不再感知协议差异。
///
/// Rust 语法注（Java 类比）：这是一个"带数据的枚举"，相当于
/// `sealed interface ProtocolEvent` + 一批 record 子类；
/// 调用方用 `match` 解构处理，漏写某个变体编译器会直接报错。
#[derive(Debug, Clone, PartialEq)]
pub enum ProtocolEvent {
    /// 文本块开始（Anthropic 的 `content_block_start` type=text）。
    /// 运行时据此推入一个新的空文本块。
    TextStart {
        /// 线上块下标（Anthropic 的 `index` 字段原值）
        block: usize,
    },
    /// 思考块开始（Anthropic 的 `content_block_start` type=thinking）。
    /// `signature`：开始帧可能自带初始签名（真 Anthropic 是长 base64，
    /// DeepSeek 的 /anthropic 端点是 UUID），后续还有分片补发。
    ThinkingStart {
        block: usize,
        signature: Option<String>,
    },
    /// 文本增量。
    ///
    /// `block` 的语义（协议差异在此显式化）：
    /// - `Some(idx)`：精确改写第 idx 块（Anthropic 的 delta 帧带 `index`）
    /// - `None`：没有索引概念（OpenAI），运行时语义 = "追加进当前文本块；
    ///   当前块不是文本就新开一块"——与拆分前 OpenAI 分支的行为一致
    TextDelta { block: Option<usize>, text: String },
    /// 思考增量。`text` 可为空串（纯签名帧），`signature` 是本帧携带的签名分片。
    ThinkingDelta {
        block: Option<usize>,
        text: String,
        signature: Option<String>,
    },
    /// 工具调用开始。
    ///
    /// `wire_idx` 是**线上自己的索引**：Anthropic = 块下标，OpenAI = tool_calls 的 call index。
    /// 解析器只搬运原值；两条协议的"线上索引 → 本地块位置"记账由运行时统一用映射表完成。
    ToolStart {
        wire_idx: usize,
        id: String,
        name: String,
    },
    /// 工具参数分片。工具入参 JSON 是逐帧拼接的
    /// （Anthropic 叫 `partial_json`，OpenAI 叫 `function.arguments`），运行时负责累积后解析。
    ToolArgsDelta { wire_idx: usize, fragment: String },
    /// 原始 usage JSON。谁家字段长什么样解析器不管，
    /// 归一化（字段名、缓存口径）统一交给 `usage::UsageObservation`。
    UsageObserved(serde_json::Value),
    /// 终止原因（Anthropic: `stop_reason` / OpenAI: `finish_reason`）。
    /// 常见值: "end_turn", "tool_use", "max_tokens", "stop", "length"
    StopReason(String),
}

/// 把一帧 SSE 的 JSON 翻译成事件列表。
///
/// # 参数（Java 类比：方法参数）
/// - `api_format`: 本次流使用的协议格式（运行时从会话配置一路带来，解析器不再猜）
/// - `frame`: 一帧 SSE 的 `data:` 载荷（已反序列化成 JSON 树）
///
/// 不认识的帧（心跳、未知类型）返回空列表——与拆分前的静默跳过一致。
pub fn parse_frame(api_format: ApiFormat, frame: &serde_json::Value) -> Vec<ProtocolEvent> {
    // Vec<ProtocolEvent> ≈ ArrayList<ProtocolEvent>；Rust 里直接就是动态数组
    let mut events = Vec::new();
    match api_format {
        ApiFormat::OpenAI => parse_openai_frame(frame, &mut events),
        ApiFormat::Anthropic => parse_anthropic_frame(frame, &mut events),
    }
    events
}

// ───────────────────────── OpenAI Chat Completions 帧 ─────────────────────────

/// OpenAI 帧：`{"choices":[{"delta":{...},"finish_reason":...}],"usage":{...}}`
fn parse_openai_frame(frame: &serde_json::Value, events: &mut Vec<ProtocolEvent>) {
    // usage：末帧携带（含缓存明细）。原样上交，字段名翻译是 UsageObservation 的事。
    // `frame.get("usage")` ≈ jsonNode.get("usage")，键存在即 Some（哪怕值是 null）
    if let Some(usage) = frame.get("usage") {
        events.push(ProtocolEvent::UsageObserved(usage.clone()));
    }

    // `let Some(x) = ... else { return }`：取不到就整体返回（Java 里类似
    // if (choices == null) return; 但 Rust 用模式匹配表达）
    let Some(choices) = frame["choices"].as_array() else {
        return;
    };
    let Some(first) = choices.first() else {
        return;
    };

    // 终止原因：stop / length / tool_calls 等（每个流的最后一个 chunk 才有）
    if let Some(fr) = first["finish_reason"].as_str() {
        events.push(ProtocolEvent::StopReason(fr.to_string()));
    }

    // delta：增量载荷。`first.get("delta")` 取不到（如纯 usage 帧）就到此为止
    let Some(delta) = first.get("delta") else {
        return;
    };

    // 正文增量。空串跳过（拆分前 OpenAI 分支就有 is_empty 判断，保持一致）
    if let Some(t) = delta["content"].as_str() {
        if !t.is_empty() {
            events.push(ProtocolEvent::TextDelta {
                block: None, // OpenAI 的 delta 不带块索引 → None = "追加进当前块"
                text: t.to_string(),
            });
        }
    }

    // 思考增量。`reasoning_content` 是 DeepSeek 等推理模型的自有字段（OpenAI 官方 schema 没有），
    // 运行时会把它合成内部 Thinking 块（signature 为空串标记"外来思考链"）
    if let Some(t) = delta["reasoning_content"].as_str() {
        if !t.is_empty() {
            events.push(ProtocolEvent::ThinkingDelta {
                block: None,
                text: t.to_string(),
                signature: None,
            });
        }
    }

    // 工具调用增量：`tool_calls` 是数组，每个元素带自己的 `index`（call index）。
    // 首帧通常 {index, id, function:{name, arguments:""}}，后续帧只有 {index, function:{arguments:"分片"}}。
    if let Some(tool_calls) = delta["tool_calls"].as_array() {
        for tc in tool_calls {
            // `as_u64().unwrap_or(0) as usize`：缺失按 0（与拆分前一致）；u64→usize 无损
            let wire_idx = tc["index"].as_u64().unwrap_or(0) as usize;
            let id = tc["id"].as_str().unwrap_or("").to_string();
            let name = tc["function"]["name"].as_str().unwrap_or("").to_string();
            // 带 id/name 的帧视为"调用开始"。若某家实现每个分片都重复带 id/name
            // （非标准），运行时会按映射表去重，不会重复建块——与拆分前行为一致。
            if !id.is_empty() || !name.is_empty() {
                events.push(ProtocolEvent::ToolStart { wire_idx, id, name });
            }
            // 参数分片：只管搬运，累积与 JSON 解析在运行时（拆分前连空串也 push，
            // 空串 push 是无操作，这里跳过不影响结果）
            if let Some(args) = tc["function"]["arguments"].as_str() {
                if !args.is_empty() {
                    events.push(ProtocolEvent::ToolArgsDelta {
                        wire_idx,
                        fragment: args.to_string(),
                    });
                }
            }
        }
    }
}

// ───────────────────────── Anthropic Messages 帧 ─────────────────────────

/// Anthropic 帧：按顶层 `type` 分派的事件流
/// （`message_start` / `content_block_start` / `content_block_delta` / `message_delta`，
/// 另有 `ping` 心跳与 `error`——本层不认识的一律静默，与拆分前一致）。
fn parse_anthropic_frame(frame: &serde_json::Value, events: &mut Vec<ProtocolEvent>) {
    match frame["type"].as_str().unwrap_or("") {
        "message_start" => {
            // 首帧：usage 在 message.usage 里。注意 Anthropic 口径的 `input_tokens`
            // 只是"未命中部分"，补齐命中量是 UsageObservation.resolve 的职责
            if let Some(usage) = frame.get("message").and_then(|m| m.get("usage")) {
                events.push(ProtocolEvent::UsageObserved(usage.clone()));
            }
        }
        "message_delta" => {
            // 收尾帧：规范上只带 output_tokens，但实测有出口回带 input_tokens ——
            // UsageObservation 的字段级覆盖天然免疫这种重复
            if let Some(usage) = frame.get("usage") {
                events.push(ProtocolEvent::UsageObserved(usage.clone()));
            }
            // 终止原因：end_turn / max_tokens / tool_use 等
            if let Some(sr) = frame["delta"]["stop_reason"].as_str() {
                events.push(ProtocolEvent::StopReason(sr.to_string()));
            }
        }
        "content_block_start" => {
            let index = frame["index"].as_u64().unwrap_or(0) as usize;
            let block = &frame["content_block"];
            match block["type"].as_str().unwrap_or("") {
                "text" => events.push(ProtocolEvent::TextStart { block: index }),
                "thinking" => events.push(ProtocolEvent::ThinkingStart {
                    block: index,
                    signature: non_empty_str(block["signature"].as_str()),
                }),
                "tool_use" => events.push(ProtocolEvent::ToolStart {
                    wire_idx: index,
                    id: block["id"].as_str().unwrap_or("").to_string(),
                    name: block["name"].as_str().unwrap_or("").to_string(),
                }),
                // 未知块类型（服务端工具等）：静默跳过，与拆分前一致
                _ => {}
            }
        }
        "content_block_delta" => {
            let index = frame["index"].as_u64().unwrap_or(0) as usize;
            let delta = &frame["delta"];
            // 拆分前按"块当前类型"分派；这里按 delta 载荷分派——
            // 文本块只会收到 text 分片、思考块只会收到 thinking/signature 分片，两者等价
            if let Some(t) = delta["text"].as_str() {
                if !t.is_empty() {
                    events.push(ProtocolEvent::TextDelta {
                        block: Some(index),
                        text: t.to_string(),
                    });
                }
            }
            if let Some(t) = delta["thinking"].as_str() {
                if !t.is_empty() {
                    events.push(ProtocolEvent::ThinkingDelta {
                        block: Some(index),
                        text: t.to_string(),
                        signature: None,
                    });
                }
            }
            // Anthropic 协议把 thinking 的签名放在**独立的** signature_delta 分片里
            // （content_block_start 里的 thinking 块不含完整签名）。不接住它，回放历史时
            // 该块就是"无签名"，而协议要求 thinking 块原样回传：真 Anthropic 会直接 400，
            // DeepSeek 的 /anthropic 端点（UUID 签名）会报
            // `content[].thinking in the thinking mode must be passed back to the API`。
            if let Some(sig) = thinking_signature_from_delta(delta) {
                events.push(ProtocolEvent::ThinkingDelta {
                    block: Some(index),
                    text: String::new(), // 纯签名帧没有文本，只有签名
                    signature: Some(sig.to_string()),
                });
            }
            if let Some(p) = delta["partial_json"].as_str() {
                if !p.is_empty() {
                    events.push(ProtocolEvent::ToolArgsDelta {
                        wire_idx: index,
                        fragment: p.to_string(),
                    });
                }
            }
        }
        _ => {}
    }
}

/// 从 `content_block_delta` 的 `delta` 里取出 thinking 签名分片。
///
/// 空签名视为"没有"，避免拼进空串（否则等于把"无签名"伪装成"有签名"）。
#[inline]
fn thinking_signature_from_delta(delta: &serde_json::Value) -> Option<&str> {
    // `?` 运算符：左侧是 None 就整函数返回 None（Java 里近似
    // if (x == null) return null; 的链式早退）
    let signature = delta.get("signature")?.as_str()?;
    if signature.is_empty() {
        None
    } else {
        Some(signature)
    }
}

/// `&str` → `Option<String>`，空串视为 None（"没给"和"给了空的"在此合并成"没给"）
fn non_empty_str(s: Option<&str>) -> Option<String> {
    s.filter(|v| !v.is_empty()).map(|v| v.to_string())
}

// ───────────────────────── 文本形态的工具调用（协议级解析）─────────────────────────

/// 判定累积中的正文是否**开始长得像**文本形态的工具调用。
///
/// 供运行时做违规检测：模型不支持原生 tool_calls 时可能把
/// `<tool_call>…</tool_call>` 直接写进正文，这值得记一条协议违规日志。
pub(crate) fn looks_like_textual_tool_call(text: &str) -> bool {
    text.contains("<tool_call") || text.contains("<function=") || text.contains("<parameter=")
}

/// 从模型输出的文本中解析 `<tool_call>` XML 块，转为 (工具名, 入参 JSON) 列表。
///
/// 这是"模型不支持原生 tool_calls"时的兜底协议：形如
/// `<tool_call><function=RunCommand><parameter=cmd>ls</parameter></function></tool_call>`
/// 何时启用兜底（本轮没有原生工具调用且正文包含标记）由运行时决定，本函数只管解析。
pub(crate) fn parse_textual_tool_calls(text: &str) -> Vec<(String, serde_json::Value)> {
    let mut results = Vec::new();
    let mut rest = text;

    // 逐个截取 <tool_call>…</tool_call> 区段
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

// ───────────────────────── 测试（帧级样例锁线）─────────────────────────
//
// 与 registry.rs 的 wire 锁线测试同思路：这些样例按真实 SSE 帧的原文形态构造，
// 解析器的输出形状错了（字段名、层级、索引），在这里当场报错，而不是等线上 400。

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    // ── Anthropic 帧 ──

    #[test]
    fn anthropic_text_delta_targets_block_index() {
        let frame = json!({
            "type": "content_block_delta", "index": 0,
            "delta": { "type": "text_delta", "text": "你好" }
        });
        assert_eq!(
            parse_frame(ApiFormat::Anthropic, &frame),
            vec![ProtocolEvent::TextDelta {
                block: Some(0),
                text: "你好".to_string()
            }]
        );
    }

    #[test]
    fn anthropic_content_block_start_emits_start_events() {
        let text_start = json!({
            "type": "content_block_start", "index": 0,
            "content_block": { "type": "text", "text": "" }
        });
        assert_eq!(
            parse_frame(ApiFormat::Anthropic, &text_start),
            vec![ProtocolEvent::TextStart { block: 0 }]
        );

        let tool_start = json!({
            "type": "content_block_start", "index": 1,
            "content_block": { "type": "tool_use", "id": "toolu_01", "name": "ReadFile" }
        });
        assert_eq!(
            parse_frame(ApiFormat::Anthropic, &tool_start),
            vec![ProtocolEvent::ToolStart {
                wire_idx: 1,
                id: "toolu_01".to_string(),
                name: "ReadFile".to_string()
            }]
        );
    }

    #[test]
    fn captures_anthropic_signature_delta() {
        // 真 Anthropic：长 base64 签名，独立分片帧、不带 thinking 文本
        let frame = json!({
            "type": "content_block_delta", "index": 2,
            "delta": { "type": "signature_delta",
                       "signature": "EqQBCgIYAhIM1gbcDa9GJwZA2b3hGgxBdjrkzLoky3dl1pki" }
        });
        assert_eq!(
            parse_frame(ApiFormat::Anthropic, &frame),
            vec![ProtocolEvent::ThinkingDelta {
                block: Some(2),
                text: String::new(),
                signature: Some("EqQBCgIYAhIM1gbcDa9GJwZA2b3hGgxBdjrkzLoky3dl1pki".to_string())
            }]
        );
    }

    #[test]
    fn captures_deepseek_uuid_signature() {
        // DeepSeek 的 /anthropic 端点用 UUID 形式的签名，必须原样回传
        let frame = json!({
            "type": "content_block_delta", "index": 0,
            "delta": { "type": "signature_delta",
                       "signature": "3f7c1f5e-2b6a-4f1e-9a5d-0b2c8e7d4a11" }
        });
        let events = parse_frame(ApiFormat::Anthropic, &frame);
        match &events[0] {
            ProtocolEvent::ThinkingDelta { signature, .. } => {
                assert_eq!(
                    signature.as_deref(),
                    Some("3f7c1f5e-2b6a-4f1e-9a5d-0b2c8e7d4a11")
                )
            }
            other => panic!("应为 ThinkingDelta，实际 {:?}", other),
        }
    }

    #[test]
    fn thinking_delta_without_signature_is_ignored() {
        // 普通的 thinking_delta 不带 signature，不能当成签名
        let frame = json!({
            "type": "content_block_delta", "index": 0,
            "delta": { "type": "thinking_delta", "thinking": "先看看目录" }
        });
        assert_eq!(
            parse_frame(ApiFormat::Anthropic, &frame),
            vec![ProtocolEvent::ThinkingDelta {
                block: Some(0),
                text: "先看看目录".to_string(),
                signature: None
            }]
        );
        // 空签名不拼进块里
        let empty_sig = json!({
            "type": "content_block_delta", "index": 0,
            "delta": { "type": "signature_delta", "signature": "" }
        });
        assert!(parse_frame(ApiFormat::Anthropic, &empty_sig).is_empty());
        assert!(parse_frame(ApiFormat::Anthropic, &json!({})).is_empty());
    }

    #[test]
    fn anthropic_usage_and_stop_reason() {
        let start = json!({
            "type": "message_start",
            "message": { "usage": { "input_tokens": 190, "output_tokens": 1 } }
        });
        assert_eq!(
            parse_frame(ApiFormat::Anthropic, &start),
            vec![ProtocolEvent::UsageObserved(
                json!({ "input_tokens": 190, "output_tokens": 1 })
            )]
        );

        // message_delta 一帧可同时带 usage 和 stop_reason → 两个事件
        let delta = json!({
            "type": "message_delta",
            "usage": { "output_tokens": 19 },
            "delta": { "stop_reason": "tool_use" }
        });
        assert_eq!(
            parse_frame(ApiFormat::Anthropic, &delta),
            vec![
                ProtocolEvent::UsageObserved(json!({ "output_tokens": 19 })),
                ProtocolEvent::StopReason("tool_use".to_string())
            ]
        );
    }

    #[test]
    fn anthropic_unknown_block_type_and_ping_are_silent() {
        let unknown_block = json!({
            "type": "content_block_start", "index": 0,
            "content_block": { "type": "server_tool_use" }
        });
        assert!(parse_frame(ApiFormat::Anthropic, &unknown_block).is_empty());
        assert!(parse_frame(ApiFormat::Anthropic, &json!({ "type": "ping" })).is_empty());
    }

    // ── OpenAI 帧 ──

    #[test]
    fn openai_text_delta_has_no_block_index() {
        let frame = json!({
            "choices": [{ "delta": { "content": "hello" } }]
        });
        assert_eq!(
            parse_frame(ApiFormat::OpenAI, &frame),
            vec![ProtocolEvent::TextDelta {
                block: None,
                text: "hello".to_string()
            }]
        );
    }

    #[test]
    fn openai_reasoning_content_maps_to_thinking() {
        // DeepSeek 等推理模型的自有字段 → 统一事件里的思考增量（无索引、无签名）
        let frame = json!({
            "choices": [{ "delta": { "reasoning_content": "先分析需求" } }]
        });
        assert_eq!(
            parse_frame(ApiFormat::OpenAI, &frame),
            vec![ProtocolEvent::ThinkingDelta {
                block: None,
                text: "先分析需求".to_string(),
                signature: None
            }]
        );
    }

    #[test]
    fn openai_tool_calls_first_chunk_then_args_chunks() {
        // 首帧：带 id/name（+ 空参数）
        let first = json!({
            "choices": [{ "delta": { "tool_calls": [
                { "index": 0, "id": "call_1", "type": "function",
                  "function": { "name": "RunCommand", "arguments": "" } }
            ] } }]
        });
        assert_eq!(
            parse_frame(ApiFormat::OpenAI, &first),
            vec![ProtocolEvent::ToolStart {
                wire_idx: 0,
                id: "call_1".to_string(),
                name: "RunCommand".to_string()
            }]
        );

        // 后续帧：只有参数分片（id/name 缺失）
        let args = json!({
            "choices": [{ "delta": { "tool_calls": [
                { "index": 0, "function": { "arguments": "{\"cmd\":" } }
            ] } }]
        });
        assert_eq!(
            parse_frame(ApiFormat::OpenAI, &args),
            vec![ProtocolEvent::ToolArgsDelta {
                wire_idx: 0,
                fragment: "{\"cmd\":".to_string()
            }]
        );
    }

    #[test]
    fn openai_usage_and_delta_can_share_one_frame() {
        // 中转站常见形态：末帧同时带 usage 和收尾 delta → 必须产出多个事件
        let frame = json!({
            "choices": [{ "delta": {}, "finish_reason": "stop" }],
            "usage": { "prompt_tokens": 1726, "completion_tokens": 24 }
        });
        assert_eq!(
            parse_frame(ApiFormat::OpenAI, &frame),
            vec![
                ProtocolEvent::UsageObserved(
                    json!({ "prompt_tokens": 1726, "completion_tokens": 24 })
                ),
                ProtocolEvent::StopReason("stop".to_string())
            ]
        );
    }

    #[test]
    fn openai_frame_without_choices_is_usage_only() {
        let frame = json!({ "usage": { "prompt_tokens": 10, "completion_tokens": 2 } });
        assert_eq!(
            parse_frame(ApiFormat::OpenAI, &frame),
            vec![ProtocolEvent::UsageObserved(
                json!({ "prompt_tokens": 10, "completion_tokens": 2 })
            )]
        );
        // 两家都不认识的帧 → 空事件，不炸
        assert!(parse_frame(ApiFormat::OpenAI, &json!({"note": "hello"})).is_empty());
    }

    // ── 文本形态工具调用 ──

    #[test]
    fn parses_textual_tool_call_xml() {
        let text = "前言 <tool_call><function=RunCommand>\
                    <parameter=cmd>ls -la</parameter>\
                    <parameter=timeout>30</parameter>\
                    </function></tool_call> 后记";
        let parsed = parse_textual_tool_calls(text);
        assert_eq!(parsed.len(), 1);
        assert_eq!(parsed[0].0, "RunCommand");
        assert_eq!(parsed[0].1, json!({ "cmd": "ls -la", "timeout": 30 }));
        assert!(looks_like_textual_tool_call(text));
        assert!(!looks_like_textual_tool_call("普通回复，没有任何标记"));
    }
}
