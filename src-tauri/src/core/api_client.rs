use serde_json::json;
use tauri::Emitter;

pub async fn api_call_with_retry(
    client: &reqwest::Client,
    url: &str,
    body: &serde_json::Value,
    api_key: &str,
    api_format: &str,
    max_retries: u32,
    app: &tauri::AppHandle,
    session_id: &str,
) -> Result<reqwest::Response, String> {
    let mut last_error = String::new();
    for attempt in 0..=max_retries {
        if attempt > 0 {
            let wait_secs = 1u64 << (attempt - 1);
            let _ = app.emit(
                "chat-stream",
                json!({
                    "content": format!("\n> △ API 调用失败，{}秒后进行第 {}/{} 次重试...\n", wait_secs, attempt, max_retries),
                    "sessionId": session_id
                }),
            );
            let _ = app.emit("agent-step", json!({
                "type": "retry",
                "attempt": attempt,
                "max": max_retries,
                "sessionId": session_id
            }));
            println!(
                "[JARVIS] API 重试 {}/{}，等待 {}s...",
                attempt, max_retries, wait_secs
            );
            tokio::time::sleep(std::time::Duration::from_secs(wait_secs)).await;
        }

        let mut req = client
            .post(url)
            .header(reqwest::header::CONTENT_TYPE, "application/json");

        if api_format == "openai" {
            req = req.header("Authorization", format!("Bearer {}", api_key));
        } else {
            req = req
                .header("x-api-key", api_key)
                .header("anthropic-version", "2023-06-01");
        }

        match req.json(body).send().await {
            Ok(response) => {
                let status = response.status();
                if status.is_success() || status.as_u16() == 200 {
                    return Ok(response);
                }
                if status.is_client_error() {
                    let err_body = response.text().await.unwrap_or_default();
                    return Err(format!(
                        "API 客户端错误 ({}): {}",
                        status.as_u16(),
                        err_body
                    ));
                }
                last_error = format!("API 服务端错误: {}", status.as_u16());
            }
            Err(e) => {
                last_error = format!("网络错误: {}", e);
            }
        }
    }
    Err(format!(
        "API 调用在 {} 次重试后仍然失败: {}",
        max_retries, last_error
    ))
}

pub async fn call_llm_simple(
    client: &reqwest::Client,
    api_key: &str,
    base_url: &str,
    model_id: &str,
    api_format: &str,
    system_prompt: &str,
    user_message: &str,
    max_tokens: i32,
) -> Result<String, String> {
    use crate::core::models::*;

    let request_body = AnthropicRequest {
        model: model_id.to_string(),
        max_tokens,
        system: system_prompt.to_string(),
        messages: vec![Message::User {
            content: Content::Single(user_message.to_string()),
        }],
        tools: vec![],
        stream: false,
        thinking: None,
        temperature: None,
        top_p: None,
        top_k: None,
    };

    let (req_json, is_openai) = if api_format == "openai" {
        use crate::core::adapters::translate_messages_to_openai;
        let openai_msgs = translate_messages_to_openai(system_prompt, &request_body.messages);
        let openai_req = OpenAIRequest {
            model: model_id.to_string(),
            max_tokens: Some(max_tokens),
            messages: openai_msgs,
            tools: None,
            stream: false,
            stream_options: None,
            reasoning_effort: None,
            thinking: None,
            thinking_budget: None,
            enable_thinking: None,
            temperature: None,
            top_p: None,
        };
        (serde_json::to_value(openai_req).unwrap(), true)
    } else {
        (serde_json::to_value(request_body).unwrap(), false)
    };

    let mut req = client
        .post(base_url)
        .header(reqwest::header::CONTENT_TYPE, "application/json");

    if is_openai {
        req = req.header("Authorization", format!("Bearer {}", api_key));
    } else {
        req = req
            .header("x-api-key", api_key)
            .header("anthropic-version", "2023-06-01");
    }

    let res = req
        .json(&req_json)
        .send()
        .await
        .map_err(|e| e.to_string())?;

    if !res.status().is_success() {
        return Err(format!("LLM Request failed: {}", res.status()));
    }

    let response_text = res.text().await.map_err(|e| e.to_string())?;
    let parsed: serde_json::Value =
        serde_json::from_str(&response_text).map_err(|e| e.to_string())?;

    let text = if is_openai {
        parsed["choices"][0]["message"]["content"]
            .as_str()
            .unwrap_or("")
            .to_string()
    } else {
        parsed["content"][0]["text"]
            .as_str()
            .unwrap_or("")
            .to_string()
    };

    Ok(text)
}
