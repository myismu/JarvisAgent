//! # api_client.rs — LLM API 客户端
//!
//! 提供与大语言模型交互的 HTTP 客户端功能：
//! - `build_streaming_client()` / `build_utility_client()`: 统一构造带超时与 keepalive 的客户端
//! - `api_call_with_retry`: 带指数退避重试的流式请求
//! - `call_llm_simple`: 简单的非流式单轮调用
//! - `call_llm_with_messages`: 非流式多轮调用（审查 Agent 等场景）
//!
//! 自动处理不同 API 格式的认证头和版本头。
//!
//! ## 依赖
//! - Internal: `crate::infra::types::error::ApiError`, `crate::infra::llm::api_format::ApiFormat`, `crate::infra::types::models`
//! - External: `reqwest`, `serde_json`, `tauri`, `tokio`
//!
//! ## 约束
//! - `call_llm_simple` 和 `call_llm_with_messages` 不注入 tools、不开启 thinking
//! - `api_call_with_retry` 支持 429 限流自动等待，4xx 立即返回
//! - **所有请求必须有超时**：客户端不可设置 `reqwest` 的整体 `timeout()`
//!   （会误杀长时间流式生成），改为「连接超时 + TCP keepalive」在客户端层兜底，
//!   「响应头超时」与「流内空闲超时」在调用层实现
//! - 非流式辅助调用统一由 `API_NON_STREAM_TIMEOUT_SECS` 包裹

use std::time::Duration;

use serde_json::json;
use tauri::Emitter;

use crate::infra::types::constants::{
    API_NON_STREAM_TIMEOUT_SECS, HTTP_CONNECT_TIMEOUT_SECS, HTTP_TCP_KEEPALIVE_SECS,
};
use crate::infra::types::error::ApiError;
use crate::infra::llm::api_format::ApiFormat;

/// 统一构造 HTTP 客户端（带连接超时与 TCP keepalive）。
///
/// 为什么**不**在此设置 `reqwest` 的整体 `timeout()`：
/// 该选项对 `send()` 之后仍在读取的流式响应同样生效，会把正常的超长生成
/// （如思考型模型的长输出）一并掐断。因此整体时限由各调用层按区间分别施加：
/// - 响应头阶段 → `API_RESPONSE_HEADER_TIMEOUT_SECS`
/// - 流式正文阶段 → `crate::core::agent::stream::STREAM_IDLE_TIMEOUT_SECS`
/// - 非流式整体 → `API_NON_STREAM_TIMEOUT_SECS`
///
/// TCP keepalive 的作用是发现"半开连接"（对方进程已死或中间设备静默丢包）——
/// 这类情况下双方都不会发 FIN/RST，只能靠内核探测。它属最后一道防线：
/// 探测周期为分钟级，不能替代应用层的空闲超时。
fn build_client() -> reqwest::Client {
    reqwest::Client::builder()
        .connect_timeout(Duration::from_secs(HTTP_CONNECT_TIMEOUT_SECS))
        .tcp_keepalive(Duration::from_secs(HTTP_TCP_KEEPALIVE_SECS))
        .build()
        .unwrap_or_else(|e| {
            // 构建失败（极罕见：TLS 后端初始化异常）不应让整个应用不可用，
            // 退回默认客户端仍保留 reqwest 自带的 30s 连接超时。
            println!("[JARVIS] HTTP 客户端构建失败，回退默认配置: {}", e);
            reqwest::Client::new()
        })
}

/// 流式请求用客户端（主 Agent / 子 Agent 的长连接）。
pub fn build_streaming_client() -> reqwest::Client {
    build_client()
}

/// 非流式辅助调用用客户端（标题生成 / 压缩 / 记忆整理 / 反思审查）。
///
/// 与流式客户端共用同一套连接与 keepalive 参数；"整体时限"由
/// `send_non_stream_and_read()` 施加，不写进客户端本身。
pub fn build_utility_client() -> reqwest::Client {
    build_client()
}

/// 发送非流式请求并读回完整响应体，整体受 `API_NON_STREAM_TIMEOUT_SECS` 限制。
///
/// 超时点覆盖「请求发出 → 响应体读完」全过程：上游崩溃时 `send()` 或
/// `text()` 会永久 pending，若不设时限，调用它的 Tauri 命令会一直不返回。
///
/// 入参直接收 `RequestBuilder`（已携带客户端与请求头），避免重复传递 client。
/// 超时时长作为参数传入，便于测试用极短时限验证超时路径本身。
pub(crate) async fn send_non_stream_and_read_with_timeout(
    req: reqwest::RequestBuilder,
    label: &str,
    timeout_secs: u64,
) -> Result<(u16, String), ApiError> {
    let future = async {
        let res = req
            .send()
            .await
            .map_err(|e| {
                // 打全成因链：只写 e.to_string() 会丢掉 connect/tls/body 等底层原因
                ApiError::Network(format!("{e:#}"))
            })?;
        let status = res.status();
        let body = res
            .text()
            .await
            .map_err(|e| ApiError::Parse(e.to_string()))?;
        Ok((status.as_u16(), body))
    };
    match tokio::time::timeout(Duration::from_secs(timeout_secs), future).await {
        Ok(result) => result,
        Err(_) => {
            let msg = format!("{} 超过 {} 秒未完成，已自动终止", label, timeout_secs);
            println!("[JARVIS] {}", msg);
            Err(ApiError::Network(msg))
        }
    }
}

/// 生产路径：使用 `API_NON_STREAM_TIMEOUT_SECS` 作为整体时限。
pub(crate) async fn send_non_stream_and_read(
    req: reqwest::RequestBuilder,
    label: &str,
) -> Result<(u16, String), ApiError> {
    send_non_stream_and_read_with_timeout(req, label, API_NON_STREAM_TIMEOUT_SECS).await
}

/// 重试退避阶梯（秒），依次对应第 1/2/3 次重试前的等待。
///
/// **完整时序**（用户确认的语义）：
/// ```text
/// 发出请求 ——①首次尝试最多等 HTTP_CONNECT_TIMEOUT_SECS(15s)—— 毫无反应
///          ——等 1s—— 第 1 次重试
///          ——等 3s—— 第 2 次重试
///          ——等 5s—— 第 3 次重试 —— 仍失败则放弃
/// ```
/// 即"首次 15 秒读秒 + 之后 1/3/5 秒间隔的三次重试"，合计等待 9 秒。
///
/// 设计意图：前 15 秒给对端充分的启动/建连宽限（服务重启通常需要十几秒），
/// 之后快速连试三次即可 —— 若这么久还没恢复，多半不是"正在启动"。
pub const RETRY_BACKOFF_SECS: [u64; 3] = [1, 3, 5];

/// 重试次数。**必须与 `RETRY_BACKOFF_SECS` 的长度一致**，
/// 否则末尾阶梯永远不会被用到。
pub const MAX_API_RETRIES: u32 = RETRY_BACKOFF_SECS.len() as u32;

/// 取第 `attempt` 次重试（从 1 开始）应等待的秒数；超出阶梯则用最后一级。
fn retry_backoff_secs(attempt: u32) -> u64 {
    if attempt == 0 {
        return 0;
    }
    let idx = (attempt as usize).saturating_sub(1).min(RETRY_BACKOFF_SECS.len() - 1);
    RETRY_BACKOFF_SECS[idx]
}

/// 把 reqwest 错误转成**可读且不误导**的说明。
///
/// 背景（实测）：直接 `e.to_string()` 只会得到
/// `error sending request for url (...)` —— 既没有原因，还容易被误读成
/// "响应内容错误"。真实成因藏在 `source()` 链里。
///
/// 另外，成因链里的 `deadline has elapsed` 指 **reqwest 的连接超时**
/// （`HTTP_CONNECT_TIMEOUT_SECS`），**不是**本项目的请求超时。
/// 原样透出会让人以为"整个请求超时了"，故对连接类错误给出统一措辞。
fn describe_network_error(err: &reqwest::Error) -> String {
    let mut chain: Vec<String> = Vec::new();
    let mut source: Option<&(dyn std::error::Error + 'static)> = Some(err);
    while let Some(cur) = source {
        chain.push(cur.to_string());
        source = cur.source();
    }
    let joined = chain.join(" → ");

    // 连接建立阶段失败：原因通常是"对端未启动/不可达"，重试期间很常见
    let lower = joined.to_ascii_lowercase();
    if lower.contains("tcp connect error")
        || lower.contains("connection refused")
        || lower.contains("deadline has elapsed")
    {
        return format!("无法连接到服务（服务可能未启动或不可达）");
    }
    format!("网络错误: {joined}")
}

/// 记录模型请求日志
pub fn log_model_request(model: &str, url: &str, agent_kind: &str) {
    println!("请求【{}】，url：【{}】，【{}】", model, url, agent_kind);
}

/// 带退避重试的 API 调用
///
/// 重试策略：固定阶梯 `RETRY_BACKOFF_SECS`（5s → 10s → 15s，合计 30s）。
///
/// 为什么用固定阶梯而非指数退避（原为 1s/2s/4s）：
/// 后者三次合计仅 7 秒，对"服务刚重启、需要几秒才就绪"这类场景几乎等于不等待；
/// 而"服务彻底没起来"时等多久都没用。固定阶梯让总时长可控（约 30s），
/// 既覆盖短暂抖动，又不会让用户以为卡死。
///
/// 客户端错误（4xx）立即返回，不重试。
/// **连接被拒（服务未监听）直接失败**：那是确定性失败，重试只是白等。
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
            let wait_secs = retry_backoff_secs(attempt);
            // 走 agent-step 而非 chat-stream：
            // 重试进度属于**过程性状态**，经 chat-stream 会混进模型正文
            // （渲染在回复气泡内部、刷新后消失）。改由前端渲染成气泡下方小字，
            // 与"等待提示"一致，也不落库（避免历史里堆满重试噪音）。
            let _ = app.emit(
                "agent-step",
                json!({
                    "type": "retry",
                    "attempt": attempt,
                    "max": max_retries,
                    "content": format!(
                        "⚠ API 调用失败，{} 秒后进行第 {}/{} 次重试…",
                        wait_secs, attempt, max_retries
                    ),
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
                // 429 Rate Limit：按服务器建议的等待时间重试
                if status.as_u16() == 429 {
                    let retry_after = response
                        .headers()
                        .get("retry-after")
                        .and_then(|v| v.to_str().ok())
                        .and_then(|v| v.parse::<u64>().ok())
                        .unwrap_or(5);
                    println!(
                        "[JARVIS] 触发频率限制 (429)，按服务器建议等待 {}s...",
                        retry_after
                    );
                    tokio::time::sleep(std::time::Duration::from_secs(retry_after)).await;
                    last_error = format!("频率限制 (429)，等待 {}s 后重试", retry_after);
                    continue;
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
                // 注：曾尝试"连接被拒则跳过重试"，但实测**无法可靠识别** ——
                // Windows 上对未监听端口多半是 TCP 静默丢弃，reqwest 成因链只到
                // `tcp connect error → deadline has elapsed`（超时），
                // 拿不到 `Connection refused`。既然区分不可靠，就不做区分：
                // 统一走退避重试（也覆盖"服务正在启动、稍后就绪"的情况）。
                last_error = format!("网络错误: {}", describe_network_error(&e));
                println!("[JARVIS] API 请求失败（第 {} 次）: {}", attempt + 1, last_error);
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
    use crate::infra::types::models::*;

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
            use crate::infra::llm::adapters::translate_messages_to_openai;
            let openai_msgs = translate_messages_to_openai(system_prompt, &request_body.messages);
            let mut openai_req = OpenAIRequest {
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
                extra_body: None,
                parameters: None,
                temperature: None,
                top_p: None,
            };
            // 工具调用不需要深度思考，显式关闭（避免 DeepSeek 等默认开启的模型浪费 token）
            crate::infra::llm::registry::apply_thinking_for_model(
                &mut openai_req, model_id, false,
            );
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

    let label = format!("非流式调用({})", model_id);
    let (status, response_text) =
        send_non_stream_and_read(req.json(&req_json), &label).await?;

    if !(200..300).contains(&status) {
        return Err(ApiError::HttpError {
            status,
            body: response_text,
        });
    }

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

/// 非流式多轮 LLM 调用（审查 Agent 等场景）
///
/// 与 `call_llm_simple` 类似，但接受完整的消息历史而非单条 user message。
/// 不注入 tools，不开启 thinking，适用于反思审查等轻量判断场景。
pub async fn call_llm_with_messages(
    client: &reqwest::Client,
    api_key: &str,
    base_url: &str,
    model_id: &str,
    api_format: ApiFormat,
    system_prompt: &str,
    messages: Vec<crate::infra::types::models::Message>,
    max_tokens: i32,
) -> Result<String, ApiError> {
    use crate::infra::types::models::*;

    let request_body = AnthropicRequest {
        model: model_id.to_string(),
        max_tokens,
        system: system_prompt.to_string(),
        messages,
        tools: vec![],
        stream: false,
        thinking: None,
        temperature: None,
        top_p: None,
        top_k: None,
    };

    let (req_json, is_openai) = match api_format {
        ApiFormat::OpenAI => {
            use crate::infra::llm::adapters::translate_messages_to_openai;
            let openai_msgs = translate_messages_to_openai(system_prompt, &request_body.messages);
            let mut openai_req = OpenAIRequest {
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
                extra_body: None,
                parameters: None,
                temperature: None,
                top_p: None,
            };
            crate::infra::llm::registry::apply_thinking_for_model(
                &mut openai_req, model_id, false,
            );
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

    log_model_request(model_id, base_url, "审查 Agent");

    let label = format!("非流式调用({})", model_id);
    let (status, response_text) =
        send_non_stream_and_read(req.json(&req_json), &label).await?;

    if !(200..300).contains(&status) {
        return Err(ApiError::HttpError {
            status,
            body: response_text,
        });
    }

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

#[cfg(test)]
mod retry_policy_tests {
    use super::{
        describe_network_error, retry_backoff_secs, MAX_API_RETRIES, RETRY_BACKOFF_SECS,
    };

    /// 退避阶梯必须是固定且递增的，且不含首跳长等待 ——
    /// "首次 15 秒宽限"由 `HTTP_CONNECT_TIMEOUT_SECS` 承担，不再放进阶梯。
    #[test]
    fn backoff_follows_configured_ladder() {
        assert_eq!(retry_backoff_secs(0), 0, "首次尝试不等待");
        assert_eq!(retry_backoff_secs(1), RETRY_BACKOFF_SECS[0]);
        assert_eq!(retry_backoff_secs(2), RETRY_BACKOFF_SECS[1]);
        assert_eq!(retry_backoff_secs(3), RETRY_BACKOFF_SECS[2]);
    }

    /// **回归防护**：重试必须正好 3 次（用户明确要求 1/3/5 三次），
    /// 曾误做成 4 次。
    #[test]
    fn exactly_three_retries() {
        assert_eq!(MAX_API_RETRIES, 3, "重试次数应为 3 次");
        assert_eq!(RETRY_BACKOFF_SECS, [1, 3, 5]);
    }

    /// 退避总时长应短于首次等待，体现"先宽限、后快试"
    #[test]
    fn backoff_total_is_shorter_than_first_grace() {
        let total: u64 = RETRY_BACKOFF_SECS.iter().sum();
        assert_eq!(total, 9, "1+3+5 应为 9 秒");
        assert!(
            total < crate::infra::types::constants::HTTP_CONNECT_TIMEOUT_SECS,
            "重试总等待应短于首次 15 秒宽限"
        );
    }

    /// **防漏配**：重试次数必须等于阶梯长度，否则末尾阶梯永远不会被用到
    #[test]
    fn retry_count_matches_ladder_length() {
        assert_eq!(
            MAX_API_RETRIES as usize,
            RETRY_BACKOFF_SECS.len(),
            "max_retries 与退避阶梯长度不一致，末尾阶梯会被浪费"
        );
    }

    /// 首次宽限 15 秒：服务启动期需要十几秒，过早失败会被误判为不可达
    #[test]
    fn first_attempt_grace_is_fifteen_seconds() {
        assert_eq!(
            crate::infra::types::constants::HTTP_CONNECT_TIMEOUT_SECS,
            15,
            "首次尝试的读秒应为 15 秒"
        );
    }

    /// 超出阶梯时沿用最后一级，不得 panic 或返回 0（返回 0 会变成忙重试）
    #[test]
    fn backoff_saturates_beyond_ladder() {
        assert_eq!(retry_backoff_secs(99), RETRY_BACKOFF_SECS[RETRY_BACKOFF_SECS.len() - 1]);
    }

    /// **文案回归**：连接失败必须给出可读结论，而不是把 reqwest 的
    /// `error sending request for url (...)`（既无原因、又易被误读）
    /// 直接丢给用户 —— 这正是实测反馈"哪里是网络错误"的根源。
    #[tokio::test]
    async fn connect_failure_produces_readable_message() {
        let client = reqwest::Client::builder()
            .connect_timeout(std::time::Duration::from_secs(2))
            .build()
            .expect("构建客户端失败");
        // 127.0.0.1:1 不会有服务监听
        let err = client
            .post("http://127.0.0.1:1/v1/chat/completions")
            .json(&serde_json::json!({"model": "x"}))
            .send()
            .await
            .expect_err("连接 127.0.0.1:1 应当失败");

        let msg = describe_network_error(&err);
        assert!(
            !msg.contains("error sending request"),
            "不应把 reqwest 原始文案直接透出，实际: {msg}"
        );
        assert!(
            msg.contains("无法连接到服务"),
            "连接类失败应给出可读结论，实际: {msg}"
        );
    }
}

#[cfg(test)]
mod non_stream_timeout_tests {
    use super::{build_utility_client, send_non_stream_and_read_with_timeout};
    use crate::infra::types::error::ApiError;

    /// 起一个"收下请求但永不响应"的本地服务，用于验证超时确实生效。
    async fn silent_server() -> u16 {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
            .await
            .expect("绑定本地端口失败");
        let port = listener.local_addr().expect("读取端口失败").port();
        tokio::spawn(async move {
            // 接受连接后不读不写、也不关闭 —— 等价于上游进程卡死
            let mut held = Vec::new();
            while let Ok((socket, _)) = listener.accept().await {
                held.push(socket);
            }
        });
        port
    }

    /// 回归防护（核心）：上游静默时非流式辅助调用必须超时返回错误，而不是永久挂起。
    ///
    /// 这类调用（标题生成 / 压缩 / 记忆整理 / 反思审查）此前**完全没有超时**，
    /// 上游崩溃时会拖住调用它的 Tauri 命令，界面表现为莫名卡住且无任何提示。
    /// 用极短时限替代生产用的 60s，验证的是机制而非数值。
    #[tokio::test]
    async fn silent_upstream_times_out_instead_of_hanging() {
        let port = silent_server().await;
        let url = format!("http://127.0.0.1:{}/v1/chat/completions", port);
        let client = build_utility_client();

        let attempt = send_non_stream_and_read_with_timeout(
            client.post(&url).json(&serde_json::json!({"model": "any"})),
            "静默上游测试",
            1,
        );

        // 外层再兜一层，避免机制失效时测试自身永久挂起
        let outcome = tokio::time::timeout(std::time::Duration::from_secs(10), attempt)
            .await
            .expect("超时机制失效：调用未在限时内返回，等价于线上永久卡住");

        match outcome {
            Err(ApiError::Network(msg)) => {
                assert!(
                    msg.contains("未完成"),
                    "超时错误应带可读说明，实际为: {msg}"
                );
            }
            other => panic!("期望超时转为 Network 错误，实际为: {other:?}"),
        }
    }

    /// 反向防护：正常响应的服务不得被误判为超时。
    #[tokio::test]
    async fn responsive_upstream_is_not_treated_as_timeout() {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
            .await
            .expect("绑定本地端口失败");
        let port = listener.local_addr().expect("读取端口失败").port();
        tokio::spawn(async move {
            use tokio::io::{AsyncReadExt, AsyncWriteExt};
            loop {
                let Ok((mut socket, _)) = listener.accept().await else {
                    break;
                };
                tokio::spawn(async move {
                    // 必须把请求（至少请求头）读完再回写：若请求体分片到达而我们
                    // 提前响应并关闭，客户端会收到 RST，表现为偶发的
                    // `error sending request`。这是测试桩本身的坑，不是被测逻辑。
                    let mut acc: Vec<u8> = Vec::new();
                    let mut buf = [0u8; 2048];
                    loop {
                        match socket.read(&mut buf).await {
                            Ok(0) => break,
                            Ok(n) => {
                                acc.extend_from_slice(&buf[..n]);
                                // 请求头结束即视为已收到足够内容
                                if acc.windows(4).any(|w| w == b"\r\n\r\n") {
                                    break;
                                }
                            }
                            Err(_) => break,
                        }
                    }
                    let body = "ok";
                    let response = format!(
                        "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
                        body.len(),
                        body
                    );
                    let _ = socket.write_all(response.as_bytes()).await;
                    let _ = socket.flush().await;
                });
            }
        });

        let url = format!("http://127.0.0.1:{}/v1/chat/completions", port);
        let client = build_utility_client();
        let result = send_non_stream_and_read_with_timeout(
            client.post(&url).json(&serde_json::json!({"model": "any"})),
            "正常上游测试",
            10,
        )
        .await;

        let (status, body) = result.expect("正常响应不应报错");
        assert_eq!(status, 200);
        assert_eq!(body, "ok");
    }
}
