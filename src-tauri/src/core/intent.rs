use crate::core::api_format::ApiFormat;
use crate::core::debug_logger;
use crate::core::models::*;

pub async fn classify_intent(
    client: &reqwest::Client,
    api_key: &str,
    base_url: &str,
    model_id: &str,
    api_format: ApiFormat,
    msg: &str,
    history: &[Message],
) -> String {
    use crate::core::intent_rules::{
        analyze_last_assistant_message, classify_by_rules, classify_with_context, Intent,
        LastAssistantAction,
    };

    let logger = debug_logger::DebugLogger::new();

    let rule_intent = classify_by_rules(msg);
    println!("[INTENT] Rule-based classification: {:?}", rule_intent);

    if rule_intent != Intent::NeedsContext {
        let result = rule_intent.as_str().to_string();
        println!("[INTENT] Final intent (by rules): {}", result);
        logger.log_intent_classifier(msg, "RULE", "", "", &result);
        return result;
    }

    let last_assistant_action: Option<LastAssistantAction> = history
        .iter()
        .rev()
        .find_map(|m| match m {
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

    println!("[INTENT] Rules inconclusive, falling back to LLM...");
    classify_intent_by_llm(client, api_key, base_url, model_id, api_format, msg, history).await
}

async fn classify_intent_by_llm(
    client: &reqwest::Client,
    api_key: &str,
    base_url: &str,
    model_id: &str,
    api_format: ApiFormat,
    msg: &str,
    history: &[Message],
) -> String {
    let system_prompt = crate::core::prompts::INTENT_CLASSIFIER_PROMPT_LIGHT;

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

    let prompt_msg = format!(
        "Context:\n{}\nInput: {}",
        context_str, msg
    );

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

    let is_openai = api_format.is_openai();
    let (req_json, _) = match api_format {
        ApiFormat::OpenAI => {
            use crate::core::adapters::translate_messages_to_openai;
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
        ApiFormat::Anthropic => {
            (serde_json::to_value(request_body).unwrap(), false)
        }
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

    if let Ok(response) = req.json(&req_json).send().await {
        if let Ok(json) = response.json::<serde_json::Value>().await {
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

            let detected_intent = match serde_json::from_str::<serde_json::Value>(text_resp.trim()) {
                Ok(val) => {
                    let category = val["category"]
                        .as_str()
                        .unwrap_or("UNCLEAR")
                        .to_uppercase();
                    match category.as_str() {
                        "CODE_READ" | "CODE_WRITE" | "CODE_REVIEW"
                        | "TASK_EXECUTE" | "TASK_PLAN" | "TASK_CONTINUE"
                        | "QUESTION" | "MEMORY_QUERY" | "SETTINGS"
                        | "CHAT" | "DANGEROUS" | "UNCLEAR" => category,
                        _ => {
                            let rules = crate::core::intent_rules::classify_by_rules(msg);
                            rules.as_str().to_string()
                        }
                    }
                }
                Err(_) => {
                    let t = text_resp.trim().to_uppercase();
                    if t.contains("DANGEROUS") { "DANGEROUS".to_string() }
                    else if t.contains("MEMORY_QUERY") { "MEMORY_QUERY".to_string() }
                    else if t.contains("QUESTION") { "QUESTION".to_string() }
                    else if t.contains("CODE_READ") || t.contains("CODE_WRITE") || t.contains("CODE_REVIEW")
                        || t.contains("TASK_EXECUTE") || t.contains("TASK_PLAN") || t.contains("TASK_CONTINUE") {
                        "CODE_WRITE".to_string()
                    } else if t.contains("SETTINGS") { "SETTINGS".to_string() }
                    else if t.contains("CHAT") { "CHAT".to_string() }
                    else {
                        let rules = crate::core::intent_rules::classify_by_rules(msg);
                        rules.as_str().to_string()
                    }
                }
            };

            let logger = debug_logger::DebugLogger::new();
            logger.log_intent_classifier(msg, "LLM", &request_json_str, &text_resp, &detected_intent);

            println!("[INTENT] Final intent (by LLM): {}", detected_intent);
            return detected_intent;
        }
    }
    println!("[INTENT] LLM failed, defaulting to UNCLEAR");
    "UNCLEAR".to_string()
}
