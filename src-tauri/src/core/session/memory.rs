//! # 记忆压缩与上下文管理 (Memory & Context Compaction)
//!
//! 管理对话上下文长度和持久化记忆：
//!
//! 1. **Token 估算** — tiktoken 精确计算，不可用时退化为 chars/4 估算
//! 2. **上下文压缩** — 单级 LLM 摘要：接近 token 上限时调模型压缩历史为一段摘要
//! 3. **记忆系统** — 全局记忆的读写，由记忆 Agent 自动维护

use crate::infra::types::error::MemoryError;
use crate::core::agent::prompts::*;
use crate::infra::llm::api_format::ApiFormat;
use crate::infra::types::models::*;
use reqwest::header::CONTENT_TYPE;
use serde_json::json;
use std::path::{Path, PathBuf};

/// 使用 tiktoken 精确计算 token 数，tokenizer 不可用时退化为 chars/4 估算
pub fn estimate_tokens(messages: &[Message]) -> usize {
    let mut text_buf = String::new();
    let mut total_image_estimate = 0;

    for msg in messages {
        match msg {
            Message::User { content } | Message::Assistant { content } => match content {
                Content::Single(text) => {
                    text_buf.push_str(text);
                }
                Content::Multiple(blocks) => {
                    for block in blocks {
                        match block {
                            ContentBlock::Text { text } => {
                                text_buf.push_str(text);
                            }
                            ContentBlock::Thinking { thinking, .. } => {
                                text_buf.push_str(thinking);
                            }
                            ContentBlock::ToolUse { name, input, .. } => {
                                text_buf.push_str(name);
                                text_buf.push_str(&input.to_string());
                            }
                            ContentBlock::ToolResult { content, .. } => {
                                text_buf.push_str(content);
                            }
                            ContentBlock::Image { .. } => {
                                total_image_estimate += 1000;
                            }
                        }
                    }
                }
            },
        }
    }

    // cl100k_base 是所有主流模型共用分词器（GPT-4/3.5/Claude），模型名仅用于查表
    crate::infra::llm::token_count::count_text("gpt-4", &text_buf).tokens + total_image_estimate
}

/// 将对话记录保存为 JSONL 转录文件（用于压缩前的备份）
pub fn append_transcript(session_id: &str, text: &str) -> Result<String, MemoryError> {
    let timestamp = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_secs();
    let filename = format!("transcript_{}.jsonl", timestamp);
    crate::core::session::resource_repository::save_transcript(
        session_id, &filename, text, timestamp,
    )
    .map_err(MemoryError::FileRead)
}

/// 对消息列表执行 LLM 摘要压缩（会话无关，不保存转录，供子 Agent 等场景使用）
pub async fn compact_messages(
    messages: &mut Vec<Message>,
    client: &reqwest::Client,
    api_key: &str,
    base_url: &str,
    model_id: &str,
    api_format: ApiFormat,
) -> Result<(), MemoryError> {
    // 保留最近 N 条消息不参与压缩
    let keep_recent = crate::infra::types::constants::COMPACT_KEEP_RECENT_MESSAGES;
    let recent: Vec<_> = if messages.len() > keep_recent {
        messages.drain(messages.len() - keep_recent..).collect()
    } else {
        Vec::new()
    };

    let summary = call_summarize_llm(messages, client, api_key, base_url, model_id, api_format).await?;

    messages.clear();
    messages.push(Message::User {
        content: Content::Single("[用户请求压缩上下文]".to_string()),
    });
    messages.push(Message::Assistant {
        content: Content::Single(format!(
            "[上下文压缩摘要]\n\n以下是对此前对话内容的自动摘要，用于保持上下文连贯性。\n\n---\n{}",
            summary
        )),
    });

    // 把保留的最近消息追加回来
    messages.extend(recent);

    Ok(())
}

/// LLM 摘要核心逻辑：序列化消息 → 构建请求 → 调用 LLM → 返回摘要文本
async fn call_summarize_llm(
    messages: &[Message],
    client: &reqwest::Client,
    api_key: &str,
    base_url: &str,
    model_id: &str,
    api_format: ApiFormat,
) -> Result<String, MemoryError> {
    let mut json_content = String::new();
    for msg in messages.iter() {
        if let Ok(m) = serde_json::to_string(msg) {
            json_content.push_str(&m);
            json_content.push('\n');
        }
    }

    let summarized_text = if json_content.len() > 150000 {
        format!(
            "(truncated...){}",
            &json_content[json_content.len() - 150000..]
        )
    } else {
        json_content.clone()
    };

    let summary_prompt = format!(
        "Summarize this conversation for continuity. Include:\n\
         1) What was accomplished (specific files modified, commands run)\n\
         2) Current state (what's working, what's broken, what's in progress)\n\
         3) Key technical decisions made and WHY\n\
         4) Any error messages encountered and how they were resolved\n\
         5) Preserve exact code snippets, file paths, and API signatures where present\n\
         Be concise but prioritize technical precision over brevity.\n\n{}",
        summarized_text
    );

    let request_body = AnthropicRequest {
        model: model_id.to_string(),
        max_tokens: 2000,
        system: "You are a technical conversation summarizer. Your job is to compress chat history while preserving all information needed for an AI coding agent to continue work without losing context. Prioritize: file paths, code snippets, error messages, technical decisions, and current task state. If in doubt, include it.".to_string(),
        messages: vec![Message::User {
            content: Content::Single(summary_prompt),
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
            use crate::infra::types::models::OpenAIRequest;
            let openai_msgs =
                translate_messages_to_openai(&request_body.system, &request_body.messages);
            let openai_req = OpenAIRequest {
                model: model_id.to_string(),
                max_tokens: Some(2000),
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
                temperature: request_body.temperature,
                top_p: request_body.top_p,
            };
            (serde_json::to_value(openai_req).unwrap(), true)
        }
        ApiFormat::Anthropic => (serde_json::to_value(request_body).unwrap(), false),
    };

    let (auth_header, auth_value) = api_format.auth_header(api_key);
    let mut req = client
        .post(base_url)
        .header(CONTENT_TYPE, "application/json")
        .header(auth_header, &auth_value);

    if api_format.requires_anthropic_version() {
        req = req.header("anthropic-version", "2023-06-01");
    }

    crate::infra::llm::api_client::log_model_request(model_id, base_url, "摘要压缩");

    let response = req.json(&req_json).send().await.map_err(|e| {
        MemoryError::CompactionFailed(format!("compact request failed: {}", e))
    })?;

    let body: serde_json::Value = response.json().await.map_err(|e| {
        MemoryError::CompactionFailed(format!("compact response parse failed: {}", e))
    })?;

    let mut text = String::new();
    if is_openai {
        if let Some(choices) = body["choices"].as_array() {
            if let Some(first) = choices.first() {
                if let Some(content) = first["message"]["content"].as_str() {
                    text = content.to_string();
                }
            }
        }
    } else {
        if let Some(content_array) = body["content"].as_array() {
            for block in content_array {
                if block["type"] == "text" {
                    if let Some(t) = block["text"].as_str() {
                        text.push_str(t);
                    }
                }
            }
        }
    }

    if text.is_empty() {
        return Err(MemoryError::CompactionFailed(
            "Failed to get summary text".to_string(),
        ));
    };

    Ok(text)
}

/// 自动压缩：保存转录 → 调用 LLM 生成摘要 → 替换会话历史
pub async fn auto_compact(
    session_id: &str,
    memory: &mut SessionMemory,
    client: &reqwest::Client,
    api_key: &str,
    base_url: &str,
    model_id: &str,
    api_format: ApiFormat,
) -> Result<(), MemoryError> {
    // 保存转录
    let mut json_content = String::new();
    for msg in memory.messages.iter() {
        if let Ok(m) = serde_json::to_string(msg) {
            json_content.push_str(&m);
            json_content.push('\n');
        }
    }
    let transcript_path = append_transcript(session_id, &json_content)?;
    println!("[auto_compact] Transcript saved to {}", transcript_path);

    // 委托核心压缩逻辑（内部保留最近 N 条不压缩）
    compact_messages(&mut memory.messages, client, api_key, base_url, model_id, api_format).await?;

    // 生成 message_ids（前两条是压缩摘要，后面是保留的最近消息）
    let message_ids: Vec<String> = (0..memory.messages.len())
        .map(|i| format!("compact:{}:{}", i, uuid::Uuid::new_v4().simple()))
        .collect();

    // 将转录路径补充到 Assistant 摘要消息中（messages[1]）
    if memory.messages.len() >= 2 {
        if let Message::Assistant {
            content: Content::Single(ref mut text),
        } = memory.messages[1]
        {
            let prefix = "[上下文压缩摘要]\n\n以下是对此前对话内容的自动摘要，用于保持上下文连贯性。\n\n---\n";
            let summary_body = text.strip_prefix(prefix).unwrap_or(text);
            *text = format!(
                "[上下文压缩摘要 · 转录: {:?}]\n\n以下是对此前对话内容的自动摘要，用于保持上下文连贯性。\n\n---\n{}",
                transcript_path, summary_body
            );
        }
    }

    // 持久化压缩消息到 session_messages 表
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs();
    if let Err(e) = crate::core::session::repository::append_or_upsert_session_messages(
        session_id,
        &memory.messages,
        &message_ids,
        "compact",
        now,
    ) {
        println!("[auto_compact] 保存压缩消息到 session_messages 失败: {}", e);
    }

    memory.message_ids = message_ids;

    Ok(())
}

/// 独立摘要接口：对任意 prompt 生成摘要文本（不替换消息）
pub async fn auto_compact_summary(
    client: &reqwest::Client,
    api_key: &str,
    base_url: &str,
    model_id: &str,
    api_format: ApiFormat,
    prompt: &str,
) -> Result<String, MemoryError> {
    let request_body = AnthropicRequest {
        model: model_id.to_string(),
        max_tokens: 1000,
        system: "You are a summarizing agent. Respond in the same language as the user's prompt. Be concise and structured.".to_string(),
        messages: vec![Message::User { content: Content::Single(prompt.to_string()) }],
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
            use crate::infra::llm::adapters::translate_messages_to_openai;
            use crate::infra::types::models::OpenAIRequest;
            let openai_msgs =
                translate_messages_to_openai(&request_body.system, &request_body.messages);
            let openai_req = OpenAIRequest {
                model: model_id.to_string(),
                max_tokens: Some(1000),
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
                temperature: request_body.temperature,
                top_p: request_body.top_p,
            };
            (serde_json::to_value(openai_req).unwrap(), true)
        }
        ApiFormat::Anthropic => (serde_json::to_value(request_body).unwrap(), false),
    };

    let (auth_header, auth_value) = api_format.auth_header(api_key);
    let mut req = client
        .post(base_url)
        .header(CONTENT_TYPE, "application/json")
        .header(auth_header, &auth_value);

    if api_format.requires_anthropic_version() {
        req = req.header("anthropic-version", "2023-06-01");
    }

    crate::infra::llm::api_client::log_model_request(model_id, base_url, "记忆agent");

    let response = req
        .json(&req_json)
        .send()
        .await
        .map_err(|e| MemoryError::CompactionFailed(format!("summary request failed: {}", e)))?;

    let body: serde_json::Value = response.json().await.map_err(|e| {
        MemoryError::CompactionFailed(format!("summary response parse failed: {}", e))
    })?;

    let mut text = String::new();
    if is_openai {
        if let Some(choices) = body["choices"].as_array() {
            if let Some(first) = choices.first() {
                if let Some(content) = first["message"]["content"].as_str() {
                    text = content.to_string();
                }
            }
        }
    } else {
        if let Some(content_array) = body["content"].as_array() {
            for block in content_array {
                if block["type"] == "text" {
                    if let Some(t) = block["text"].as_str() {
                        text.push_str(t);
                    }
                }
            }
        }
    }

    if text.is_empty() {
        return Err(MemoryError::CompactionFailed(
            "Failed to get summary text".to_string(),
        ));
    }

    Ok(text)
}

// --- 记忆系统：全局记忆 + 项目记忆 ---

/// 全局记忆文件路径（agent_home/global/global_memory.md）
pub fn get_global_memory_path() -> PathBuf {
    crate::infra::config::data_paths::global_memory_path()
}

/// 读取记忆文件，不存在则创建带默认头部的空文件
pub fn read_memory_file(path: &Path, header: &str) -> String {
    if let Ok(content) = std::fs::read_to_string(path) {
        content
    } else {
        create_memory_file(path, header)
    }
}

fn create_memory_file(path: &Path, header: &str) -> String {
    let initial = format!("# {}\n\n(暂无记录)\n", header);
    let _ = std::fs::create_dir_all(path.parent().unwrap());
    let _ = std::fs::write(path, &initial);
    initial
}

use crate::infra::config::config::AgentConfig;

/// 记忆 Agent：根据最新对话自动更新全局/项目记忆文件
pub async fn run_memory_agent(user_msg: String, assistant_reply: String, config: AgentConfig) {
    println!("\n[MEMORY] --- Memory Agent Started ---");

    if config.api_key.is_empty() {
        return;
    }
    let api_key = config.api_key;
    let base_url = config.base_url;
    let model_id = config.utility_model; // 记忆 Agent 使用工具模型（更便宜）

    let global_path = get_global_memory_path();
    let global_content = read_memory_file(&global_path, "Global Memory");

    let user_content = format!(
        "【当前全局记忆】\n{}\n\n【最新对话】\nUser: {}\nAssistant: {}",
        global_content, user_msg, assistant_reply
    );

    let tools = vec![json!({
        "name": "update_memory",
        "description": "更新全局记忆文件。",
        "input_schema": {
            "type": "object",
            "properties": {
                "content": { "type": "string", "description": "更新后的完整 Markdown 内容" }
            },
            "required": ["content"]
        }
    })];

    let client = reqwest::Client::new();
    let request_body = AnthropicRequest {
        model: model_id.clone(),
        max_tokens: crate::infra::types::constants::MAX_TOKENS_CONTEXT,
        system: MEMORY_AGENT_SYSTEM.to_string(),
        messages: vec![Message::User {
            content: Content::Single(user_content),
        }],
        tools,
        stream: false,
        thinking: None,
        temperature: config.temperature,
        top_p: config.top_p,
        top_k: config.top_k,
    };

    let api_format = ApiFormat::from_str(&config.api_format);
    let is_openai = api_format.is_openai();
    let (req_json, _) = match api_format {
        ApiFormat::OpenAI => {
            use crate::infra::llm::adapters::{
                translate_messages_to_openai, translate_tools_to_openai,
            };
            use crate::infra::types::models::OpenAIRequest;
            let openai_msgs =
                translate_messages_to_openai(&request_body.system, &request_body.messages);
            let openai_tools = translate_tools_to_openai(&request_body.tools);
            let openai_req = OpenAIRequest {
                model: model_id.clone(),
                max_tokens: Some(crate::infra::types::constants::MAX_TOKENS_CONTEXT),
                messages: openai_msgs,
                tools: if openai_tools.is_empty() {
                    None
                } else {
                    Some(openai_tools)
                },
                stream: false,
                stream_options: None,
                reasoning_effort: None,
                thinking: None,
                thinking_budget: None,
                enable_thinking: None,
                extra_body: None,
                parameters: None,
                temperature: request_body.temperature,
                top_p: request_body.top_p,
            };
            (serde_json::to_value(openai_req).unwrap(), true)
        }
        ApiFormat::Anthropic => (serde_json::to_value(request_body).unwrap(), false),
    };

    let request_json_str = serde_json::to_string_pretty(&req_json).unwrap_or_default();
    let logger = crate::infra::debug_logger::DebugLogger::new();
    logger.log_request_to_terminal("MEMORY AGENT", 1, &request_json_str);

    let (auth_header, auth_value) = api_format.auth_header(&api_key);
    let mut req = client
        .post(&base_url)
        .header(CONTENT_TYPE, "application/json")
        .header(auth_header, &auth_value);

    if api_format.requires_anthropic_version() {
        req = req.header("anthropic-version", "2023-06-01");
    }

    crate::infra::llm::api_client::log_model_request(&model_id, &base_url, "记忆agent");

    if let Ok(response) = req.json(&req_json).send().await {
        if let Ok(body) = response.json::<serde_json::Value>().await {
            if is_openai {
                if let Some(choices) = body["choices"].as_array() {
                    if let Some(first) = choices.first() {
                        if let Some(tool_calls) = first["message"]["tool_calls"].as_array() {
                            for tc in tool_calls {
                                if tc["type"] == "function"
                                    && tc["function"]["name"] == "update_memory"
                                {
                                    if let Some(args_str) = tc["function"]["arguments"].as_str() {
                                        if let Ok(args_json) =
                                            serde_json::from_str::<serde_json::Value>(args_str)
                                        {
                                            let content =
                                                args_json["content"].as_str().unwrap_or("");
                                            if !content.is_empty() {
                                                println!("[MEMORY] Updating global memory (OpenAI)...");
                                                let _ = std::fs::write(&global_path, content);
                                                logger.log_memory_agent(
                                                    &request_json_str,
                                                    "Updated global memory",
                                                );
                                            }
                                        }
                                    }
                                }
                            }
                        }
                    }
                }
            } else {
                if let Some(content_array) = body["content"].as_array() {
                    for block in content_array {
                        if block["type"] == "tool_use" && block["name"] == "update_memory" {
                            let content = block["input"]["content"].as_str().unwrap_or("");
                            if !content.is_empty() {
                                println!("[MEMORY] Updating global memory (Anthropic)...");
                                let _ = std::fs::write(&global_path, content);
                                logger.log_memory_agent(
                                    &request_json_str,
                                    "Updated global memory",
                                );
                            }
                        }
                    }
                }
            }
        }
    }
    println!("[MEMORY] --- Memory Agent Finished ---");
}
