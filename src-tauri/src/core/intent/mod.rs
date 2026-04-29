//! # 意图分类模块 (Intent Classification)
//!
//! 采用三层分级策略将用户输入归类为预定义意图：
//! 1. 规则层 — 关键词正则匹配（覆盖 ~90% 明确请求，零延迟）
//! 2. 上下文层 — 结合上一轮对话特征解析短回复歧义
//! 3. LLM 层 — 轻量模型兜底处理真正模糊的输入
//!
//! 返回值为意图字符串（如 `"CODE_READ"`、`"DANGEROUS"`），供下游
//! 工具加载和 Agent 路由使用。

pub mod rules;

use crate::core::infra::debug_logger;
use crate::core::llm::api_format::ApiFormat;
use crate::core::models::*;

fn fallback_intent_for_unresolved(msg: &str) -> String {
    use crate::core::intent::rules::{classify_by_rules, Intent};

    let rules = classify_by_rules(msg);
    match rules {
        Intent::NeedsContext => {
            if msg.trim().is_empty() {
                "UNCLEAR".to_string()
            } else {
                "CHAT".to_string()
            }
        }
        _ => rules.as_str().to_string(),
    }
}

/// 意图分类入口：依次尝试规则 → 上下文 → LLM 三层策略
pub async fn classify_intent(
    client: &reqwest::Client,
    api_key: &str,
    base_url: &str,
    model_id: &str,
    api_format: ApiFormat,
    msg: &str,
    history: &[Message],
) -> String {
    use crate::core::intent::rules::{
        analyze_last_assistant_message, classify_by_rules, classify_with_context, Intent,
        LastAssistantAction,
    };

    let logger = debug_logger::DebugLogger::new();

    // 第一层：纯规则匹配
    let rule_intent = classify_by_rules(msg);
    println!("[INTENT] Rule-based classification: {:?}", rule_intent);

    if rule_intent != Intent::NeedsContext {
        let result = rule_intent.as_str().to_string();
        println!("[INTENT] Final intent (by rules): {}", result);
        logger.log_intent_classifier(msg, "RULE", "", "", &result);
        return result;
    }

    // 第二层：从历史消息中提取上一轮助手行为特征
    let last_assistant_action: Option<LastAssistantAction> =
        history.iter().rev().find_map(|m| match m {
            Message::Assistant { content } => {
                let text = match content {
                    Content::Single(s) => s.clone(),
                    Content::Multiple(_) => return None,
                };
                if !text.is_empty() {
                    Some(analyze_last_assistant_message(&text))
                } else {
                    None
                }
            }
            _ => None,
        });

    let context_intent = classify_with_context(msg, last_assistant_action.as_ref());
    println!(
        "[INTENT] Context-aware classification: {:?}",
        context_intent
    );

    if context_intent != Intent::NeedsContext {
        let result = context_intent.as_str().to_string();
        println!("[INTENT] Final intent (by context): {}", result);
        logger.log_intent_classifier(msg, "CONTEXT", "", "", &result);
        return result;
    }

    // 第三层：规则和上下文均无法判定，调用轻量 LLM 兜底
    println!("[INTENT] Rules inconclusive, falling back to LLM...");
    classify_intent_by_llm(
        client, api_key, base_url, model_id, api_format, msg, history,
    )
    .await
}

/// LLM 兜底分类：构建精简 prompt 调用轻量模型，解析返回的 JSON 意图标签
async fn classify_intent_by_llm(
    client: &reqwest::Client,
    api_key: &str,
    base_url: &str,
    model_id: &str,
    api_format: ApiFormat,
    msg: &str,
    history: &[Message],
) -> String {
    let system_prompt = crate::core::infra::prompts::INTENT_CLASSIFIER_PROMPT_LIGHT;

    // 拼接最近 4 条对话作为上下文（每条截断至 100 字符以节省 token）
    let mut context_str = String::new();
    let recent: Vec<_> = history.iter().rev().take(4).rev().collect();
    for m in recent {
        match m {
            Message::User { content } => {
                let text = match content {
                    Content::Single(s) => s.clone(),
                    Content::Multiple(_) => "[Complex User Input]".to_string(),
                };
                context_str.push_str(&format!(
                    "User: {}\n",
                    text.chars().take(100).collect::<String>()
                ));
            }
            Message::Assistant { content } => {
                let text = match content {
                    Content::Single(s) => s.clone(),
                    Content::Multiple(_) => "[Complex Assistant Action]".to_string(),
                };
                context_str.push_str(&format!(
                    "Assistant: {}\n",
                    text.chars().take(100).collect::<String>()
                ));
            }
        }
    }

    let prompt_msg = format!("Context:\n{}\nInput: {}", context_str, msg);

    let request_body = AnthropicRequest {
        model: model_id.to_string(),
        max_tokens: 50,
        system: system_prompt.to_string(),
        messages: vec![Message::User {
            content: Content::Single(prompt_msg.clone()),
        }],
        tools: vec![],
        stream: false,
        thinking: None,
        temperature: None,
        top_p: None,
        top_k: None,
    };

    // 根据 API 格式（Anthropic / OpenAI）构建请求体
    let is_openai = api_format.is_openai();
    let (req_json, _) = match api_format {
        ApiFormat::OpenAI => {
            use crate::core::llm::adapters::translate_messages_to_openai;
            use crate::core::models::OpenAIRequest;
            let openai_msgs = translate_messages_to_openai(&system_prompt, &request_body.messages);
            let openai_req = OpenAIRequest {
                model: model_id.to_string(),
                max_tokens: Some(30),
                messages: openai_msgs,
                tools: None,
                stream: false,
                stream_options: None,
                reasoning_effort: None,
                thinking: None,
                thinking_budget: None,
                enable_thinking: None,
                temperature: request_body.temperature,
                top_p: request_body.top_p,
            };
            (serde_json::to_value(openai_req).unwrap(), true)
        }
        ApiFormat::Anthropic => (serde_json::to_value(request_body).unwrap(), false),
    };

    let request_json_str = serde_json::to_string_pretty(&req_json).unwrap_or_default();

    let (auth_header, auth_value) = api_format.auth_header(api_key);
    let mut req = client
        .post(base_url)
        .header(reqwest::header::CONTENT_TYPE, "application/json")
        .header(auth_header, &auth_value);

    if api_format.requires_anthropic_version() {
        req = req.header("anthropic-version", "2023-06-01");
    }

    crate::core::llm::api_client::log_model_request(model_id, base_url, "意图分类器agent");

    // 发送请求并解析响应
    if let Ok(response) = req.json(&req_json).send().await {
        if let Ok(json) = response.json::<serde_json::Value>().await {
            // 根据 API 格式提取文本响应
            let mut text_resp = String::new();
            if is_openai {
                if let Some(choices) = json["choices"].as_array() {
                    if let Some(first) = choices.first() {
                        if let Some(content) = first["message"]["content"].as_str() {
                            text_resp = content.to_string();
                        }
                    }
                }
            } else {
                if let Some(content) = json["content"].as_array() {
                    if let Some(text_block) = content.first() {
                        if let Some(text) = text_block["text"].as_str() {
                            text_resp = text.to_string();
                        }
                    }
                }
            }

            // 从 LLM 响应中提取意图标签，非法值回退到规则分类
            let detected_intent = match serde_json::from_str::<serde_json::Value>(text_resp.trim())
            {
                Ok(val) => {
                    let category = val["category"].as_str().unwrap_or("UNCLEAR").to_uppercase();
                    match category.as_str() {
                        "CODE_READ" | "CODE_WRITE" | "CODE_REVIEW" | "TASK_EXECUTE"
                        | "TASK_PLAN" | "TASK_CONTINUE" | "QUESTION" | "MEMORY_QUERY"
                        | "SETTINGS" | "CHAT" | "DANGEROUS" | "UNCLEAR" => category,
                        _ => fallback_intent_for_unresolved(msg),
                    }
                }
                Err(_) => {
                    let t = text_resp.trim().to_uppercase();
                    if t.contains("DANGEROUS") {
                        "DANGEROUS".to_string()
                    } else if t.contains("MEMORY_QUERY") {
                        "MEMORY_QUERY".to_string()
                    } else if t.contains("CODE_REVIEW") {
                        "CODE_REVIEW".to_string()
                    } else if t.contains("CODE_READ") {
                        "CODE_READ".to_string()
                    } else if t.contains("CODE_WRITE") {
                        "CODE_WRITE".to_string()
                    } else if t.contains("TASK_EXECUTE") {
                        "TASK_EXECUTE".to_string()
                    } else if t.contains("TASK_PLAN") {
                        "TASK_PLAN".to_string()
                    } else if t.contains("TASK_CONTINUE") {
                        "TASK_CONTINUE".to_string()
                    } else if t.contains("QUESTION") {
                        "QUESTION".to_string()
                    } else if t.contains("SETTINGS") {
                        "SETTINGS".to_string()
                    } else if t.contains("CHAT") {
                        "CHAT".to_string()
                    } else {
                        fallback_intent_for_unresolved(msg)
                    }
                }
            };

            let logger = debug_logger::DebugLogger::new();
            logger.log_intent_classifier(
                msg,
                "LLM",
                &request_json_str,
                &text_resp,
                &detected_intent,
            );

            println!("[INTENT] Final intent (by LLM): {}", detected_intent);
            return detected_intent;
        }
    }
    let fallback = fallback_intent_for_unresolved(msg);
    println!("[INTENT] LLM failed, fallback intent: {}", fallback);
    fallback
}
