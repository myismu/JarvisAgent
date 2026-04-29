//! LLM API 客户端
//!
//! 提供与大语言模型交互的 HTTP 客户端功能：
//! - `api_call_with_retry`: 带指数退避重试的流式请求
//! - `call_llm_simple`: 简单的非流式单轮调用
//!
//! 自动处理不同 API 格式的认证头和版本头。

use serde_json::json;
use tauri::Emitter;

use crate::core::llm::api_format::ApiFormat;
use crate::core::error::ApiError;

/// 记录模型请求日志
pub fn log_model_request(model: &str, url: &str, agent_kind: &str) {
    println!("请求【{}】，url：【{}】，【{}】", model, url, agent_kind);
}

/// 带指数退避重试的 API 调用
///
/// 重试策略：1s, 2s, 4s... 最多重试 max_retries 次
/// 客户端错误（4xx）立即返回，不重试
pub async fn api_call_with_retry(
    client: &reqwest::Client,
    url: &str,
    body: &serde_json::Value,
    api_key: &str,
    api_format: ApiFormat,
    max_retries: u32,
    app: &tauri::AppHandle,
    session_id: &str,
) -> Result<reqwest::Response, ApiError> {
    let (auth_header_name, auth_header_value) = api_format.auth_header(api_key);

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
            let _ = app.emit(
                "agent-step",
                json!({
                    "type": "retry",
                    "attempt": attempt,
                    "max": max_retries,
                    "sessionId": session_id
                }),
            );
            println!(
                "[JARVIS] API 重试 {}/{}，等待 {}s...",
                attempt, max_retries, wait_secs
            );
            tokio::time::sleep(std::time::Duration::from_secs(wait_secs)).await;
        }

        let mut req = client
            .post(url)
            .header(reqwest::header::CONTENT_TYPE, "application/json")
            .header(auth_header_name, &auth_header_value);

        if api_format.requires_anthropic_version() {
            req = req.header("anthropic-version", "2023-06-01");
        }

        let model = body
            .get("model")
            .and_then(|v| v.as_str())
            .unwrap_or("unknown");
        log_model_request(model, url, "主agent");

        match req.json(body).send().await {
            Ok(response) => {
                let status = response.status();
                if status.is_success() || status.as_u16() == 200 {
                    return Ok(response);
                }
                if status.is_client_error() {
                    let err_body = response.text().await.unwrap_or_default();
                    return Err(ApiError::HttpError {
                        status: status.as_u16(),
                        body: err_body,
                    });
                }
                last_error = format!("API 服务端错误: {}", status.as_u16());
            }
            Err(e) => {
                last_error = format!("网络错误: {}", e);
            }
        }
    }
    Err(ApiError::RetriesExhausted {
        max_retries,
        last_error,
    })
}

/// 简单的非流式单轮 LLM 调用
///
/// 用于意图分类等不需要流式输出的场景
pub async fn call_llm_simple(
    client: &reqwest::Client,
    api_key: &str,
    base_url: &str,
    model_id: &str,
    api_format: ApiFormat,
    system_prompt: &str,
    user_message: &str,
    max_tokens: i32,
) -> Result<String, ApiError> {
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

    let (req_json, is_openai) = match api_format {
        ApiFormat::OpenAI => {
            use crate::core::llm::adapters::translate_messages_to_openai;
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
        }
        ApiFormat::Anthropic => (serde_json::to_value(request_body).unwrap(), false),
    };

    let (auth_header_name, auth_header_value) = api_format.auth_header(api_key);
    let mut req = client
        .post(base_url)
        .header(reqwest::header::CONTENT_TYPE, "application/json")
        .header(auth_header_name, &auth_header_value);

    if api_format.requires_anthropic_version() {
        req = req.header("anthropic-version", "2023-06-01");
    }

    log_model_request(model_id, base_url, "主agent");

    let res = req
        .json(&req_json)
        .send()
        .await
        .map_err(|e| ApiError::Network(e.to_string()))?;

    if !res.status().is_success() {
        let status = res.status().as_u16();
        let err_body = res.text().await.unwrap_or_default();
        return Err(ApiError::HttpError {
            status,
            body: err_body,
        });
    }

    let response_text = res
        .text()
        .await
        .map_err(|e| ApiError::Parse(e.to_string()))?;
    let parsed: serde_json::Value =
        serde_json::from_str(&response_text).map_err(|e| ApiError::Parse(e.to_string()))?;

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
