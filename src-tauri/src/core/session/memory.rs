//! # 记忆压缩与上下文管理 (Memory & Context Compaction)
//!
//! 管理对话上下文长度和持久化记忆：
//!
//! 1. **Token 估算** — tiktoken 精确计算，不可用时退化为 chars/4 估算
//! 2. **上下文压缩** — 单级 LLM 摘要：接近 token 上限时调模型压缩历史为一段摘要
//! 3. **记忆系统** — 全局记忆的读写；主 Agent 通过记忆工具维护，超预算时后台整理压缩

use crate::infra::types::error::MemoryError;
use crate::core::agent::prompts::*;
use crate::infra::llm::api_format::ApiFormat;
use crate::infra::types::models::*;
use reqwest::header::CONTENT_TYPE;
use std::path::{Path, PathBuf};
use tauri::Emitter;

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
                            ContentBlock::Context { text } => {
                                text_buf.push_str(text);
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
///
/// ⚠️ **本函数会压掉传进来的全部消息**（2026-09-17 起不再保留尾部）：
/// 调用方必须先把「本轮任务区间」从 `memory` 里摘出去，只把**已完成轮次的前缀**传进来。
/// 主 Agent 的做法见 `pipeline::compact_if_needed`（按 `initial_msg_index` 切分后
/// 把前缀压缩、把本轮区间拼回）。
pub async fn compact_messages(
    memory: &mut SessionMemory,
    client: &reqwest::Client,
    api_key: &str,
    base_url: &str,
    model_id: &str,
    api_format: ApiFormat,
) -> Result<(), MemoryError> {
    // 1. 清理 internal/background 消息（系统内部通知无需保留，直接删除）
    let mut keep_indices = Vec::new();
    for (i, src) in memory.sources.iter().enumerate() {
        if matches!(src.as_str(), "chat" | "compact" | "context") {
            keep_indices.push(i);
        }
    }
    if keep_indices.len() < memory.messages.len() {
        memory.messages = keep_indices.iter().map(|&i| memory.messages[i].clone()).collect();
        memory.sources = keep_indices.iter().map(|&i| memory.sources[i].clone()).collect();
    }

    let messages = &mut memory.messages;
    let sources = &mut memory.sources;

    // 2. 不留尾巴：整个前缀都参与压缩，压缩后历史只剩「压缩请求 + 摘要」两条。
    //
    // 2026-09-17 起撤掉「固定保留最近 N 条」：以消息条数为轴切不出任务边界
    // （一次工具调用就占 2 条，6 条可能只覆盖 3 次调用，也可能横跨多个已完结任务）。
    // 改为按任务边界切 —— 由调用方（`pipeline::compact_if_needed`）保证
    // **本轮任务不出现在传进来的 messages 里**（本轮区间由 `initial_msg_index` 白名单保护），
    // 因此这里可以放心压掉全部输入。
    //
    // ⚠️ 调用方必须先把本轮区间摘出去，否则会把正在执行的任务也压掉。

    let summary = call_summarize_llm(messages, client, api_key, base_url, model_id, api_format).await?;

    messages.clear();
    sources.clear();
    messages.push(Message::User {
        content: Content::Single("[用户请求压缩上下文]".to_string()),
    });
    sources.push("compact".to_string());
    messages.push(Message::Assistant {
        content: Content::Single(format!(
            "[上下文压缩摘要]\n\n以下是对此前对话内容的自动摘要，用于保持上下文连贯性。\n\n---\n{}",
            summary
        )),
    });
    sources.push("compact".to_string());

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
    // 摘要只需要真实对话：动态上下文（工作目录 / repo map / 全局记忆）整段丢掉，别白烧 token
    let messages = crate::infra::llm::adapters::strip_context_blocks(messages);

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
         6) If the conversation shows tasks that were explicitly requested, list which ones are \
         DONE and which ones are still UNFINISHED (including anything half-applied, \
         pending verification, or blocked on a decision). Only include this list if such tasks \
         actually exist — do not invent items or pad with generic statements.\n\
         Be concise but prioritize technical precision over brevity. \
         Never drop an unfinished task in order to save space.\n\n{}",
        summarized_text
    );

    let request_body = AnthropicRequest {
        model: model_id.to_string(),
        max_tokens: 2000,
        system: "You are a technical conversation summarizer. Your job is to compress chat history while preserving all information needed for an AI coding agent to continue work without losing context. Prioritize: file paths, code snippets, error messages, technical decisions, current task state, and — above all — any task that was requested but not yet finished. Never omit an unfinished task to save space. If in doubt, include it.".to_string(),
        messages: vec![Message::User {
            content: Content::Single(summary_prompt),
        }],
        tools: vec![],
        stream: false,
        thinking: None,
        temperature: None,
        top_p: None,
        top_k: None,
        output_config: None,
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

    // 委托核心压缩逻辑（内部保留最近 N 条不压缩，同时清理 internal/background）
    compact_messages(memory, client, api_key, base_url, model_id, api_format).await?;

    // 从 DB 中删除已清理的 internal/background 消息
    if let Err(e) = crate::core::session::repository::delete_session_messages_by_source(
        session_id,
        &["internal", "background"],
    ) {
        println!("[auto_compact] 清理 internal/background 消息失败: {}", e);
    }

    // 生成 message_ids（同样走 message_ids 重建：本函数只负责把整体压缩掉，
    // 拼接本轮区间、重算下标由调用方完成 —— 见 `pipeline::compact_if_needed`）
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
        &memory.sources,
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
        output_config: None,
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
/// 全局记忆文件的进程内互斥锁。
///
/// 记忆工具是并行执行的（`tools_runner` 用 `tokio::spawn` + `join_all`）。
/// 一次「读 → 改 → 写」如果不串起来，多个并发调用会各自读到同一份旧内容、
/// 各自通过 `write_if_unchanged` 的校验，再各自整份覆盖写回——最后一个赢，
/// 其余条目静默丢失，而每个调用都返回成功。
///
/// 约定：持锁期间不得 `await`（当前持锁的都是纯同步逻辑）。
static MEMORY_FILE_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

/// 取得全局记忆文件的锁；锁被 panic 污染时取回内部值，避免连带失败。
pub fn lock_memory_file() -> std::sync::MutexGuard<'static, ()> {
    MEMORY_FILE_LOCK
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}

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

/// 调 LLM 重写全局记忆，返回重写后的完整内容。
///
/// 不落盘、不发事件：由调用方决定何时写入（并做乐观并发校验）。
pub async fn rewrite_global_memory(
    session_id: &str,
    config: &AgentConfig,
    system_prompt: &str,
    user_content: String,
    log_label: &str,
) -> Option<String> {
    // 辅助调用统一走带超时的客户端（整体时限由 send_non_stream_and_read 施加）
    let client = crate::infra::llm::api_client::build_utility_client();
    let request_body = AnthropicRequest {
        model: config.utility_model.clone(),
        max_tokens: crate::infra::types::constants::MAX_TOKENS_CONTEXT,
        system: system_prompt.to_string(),
        messages: vec![Message::User {
            content: Content::Single(user_content),
        }],
        // 整理只要求纯文本输出：不带工具，省 token 也省一次 tool_use 解析
        tools: Vec::new(),
        stream: false,
        thinking: None,
        temperature: config.temperature,
        top_p: config.top_p,
        top_k: config.top_k,
        output_config: None,
    };

    let api_format = ApiFormat::from_str(&config.api_format);
    let is_openai = api_format.is_openai();
    let req_json = match api_format {
        ApiFormat::OpenAI => {
            use crate::infra::llm::adapters::{
                translate_messages_to_openai, translate_tools_to_openai,
            };
            use crate::infra::types::models::OpenAIRequest;
            let openai_msgs =
                translate_messages_to_openai(&request_body.system, &request_body.messages);
            let openai_tools = translate_tools_to_openai(&request_body.tools);
            let openai_req = OpenAIRequest {
                model: request_body.model.clone(),
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
            serde_json::to_value(openai_req).ok()?
        }
        ApiFormat::Anthropic => serde_json::to_value(request_body).ok()?,
    };

    // 记忆整理提示词每次都是新内容（不参与增量），pretty 串只用于 log_memory 阅读；
    // 请求日志走增量通道，直接传结构化 Value
    let request_json_str = serde_json::to_string_pretty(&req_json).unwrap_or_default();
    println!("[MEMORY] {} request ({} bytes)", log_label, request_json_str.len());
    crate::infra::debug_logger::debug_logger().log_request(session_id, "MEMORY", 1, &req_json);

    let (auth_header, auth_value) = api_format.auth_header(&config.api_key);
    let mut req = client
        .post(&config.base_url)
        .header(CONTENT_TYPE, "application/json")
        .header(auth_header, &auth_value);

    if api_format.requires_anthropic_version() {
        req = req.header("anthropic-version", "2023-06-01");
    }

    crate::infra::llm::api_client::log_model_request(&config.utility_model, &config.base_url, "记忆整理");

    let body: serde_json::Value = req.json(&req_json).send().await.ok()?.json().await.ok()?;

    let raw_text = if is_openai {
        body["choices"]
            .as_array()
            .and_then(|choices| choices.first())
            .and_then(|first| first["message"]["content"].as_str())
            .map(|text| text.to_string())
    } else {
        body["content"].as_array().and_then(|blocks| {
            blocks
                .iter()
                .find(|block| block["type"] == "text")
                .and_then(|block| block["text"].as_str())
                .map(|text| text.to_string())
        })
    }?;

    let cleaned = strip_code_fence(&raw_text);
    if cleaned.is_empty() {
        return None;
    }

    crate::infra::debug_logger::debug_logger().log_memory(session_id, &request_json_str, log_label);
    Some(cleaned)
}

/// 去掉模型可能额外加上的 ``` 围栏
fn strip_code_fence(text: &str) -> String {
    let trimmed = text.trim();
    if !trimmed.starts_with("```") {
        return trimmed.to_string();
    }
    let mut lines = trimmed.lines();
    lines.next();
    let mut body: Vec<&str> = lines.collect();
    if let Some(last) = body.last() {
        if last.trim() == "```" {
            body.pop();
        }
    }
    body.join("\n").trim().to_string()
}

/// 乐观写入：只有当文件内容仍等于 `expected` 时才写入，避免覆盖期间发生的其它修改
pub fn write_if_unchanged(path: &Path, expected: &str, new_content: &str) -> bool {
    let current = std::fs::read_to_string(path).unwrap_or_default();
    if current != expected {
        return false;
    }
    if let Some(parent) = path.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    std::fs::write(path, new_content).is_ok()
}

/// 全局记忆的当前字符数（用于判断是否该触发整理）
pub fn global_memory_char_count() -> usize {
    let _guard = lock_memory_file();
    std::fs::read_to_string(get_global_memory_path())
        .map(|content| content.chars().count())
        .unwrap_or(0)
}

/// 记忆整理：全局记忆超过阈值时后台全量重写一次。
///
/// 不再是「每轮结束都跑一次 LLM」——那样既贵，又会把记忆越滚越乱。
/// 新增事实由主 Agent 的 UpdateMemory 负责，这里只做合并与压缩。
///
/// 与 `ConsolidateMemory` 工具共用 `rewrite_global_memory` + `MEMORY_CURATOR_SYSTEM`，
/// 区别只在触发时机：这里是记忆超阈值时自动跑，那边是模型主动调。
pub async fn run_memory_curator(app: tauri::AppHandle, config: AgentConfig, session_id: String) {
    println!("\n[MEMORY] --- Memory Curator Started ---");

    if config.api_key.is_empty() {
        return;
    }

    let path = get_global_memory_path();
    let original = read_memory_file(&path, "Global Memory");
    let user_content = format!(
        "【当前全局记忆】\n{}\n\n【整理要求】\n按系统提示的结构与预算重写这份记忆：合并同类项、删除过期与不合格条目、压缩冗余表述，不要新增没有依据的事实。",
        original.trim()
    );

    match rewrite_global_memory(
        &session_id,
        &config,
        MEMORY_CURATOR_SYSTEM,
        user_content,
        "curator",
    )
    .await
    {
        Some(new_content) => {
            // 持锁重读校验：整理期间若有并发的 UpdateMemory 写入，本轮整理作废（下一轮再来）
            let written = {
                let _guard = lock_memory_file();
                write_if_unchanged(&path, &original, &new_content)
            };
            if written {
                println!(
                    "[MEMORY] Consolidated: {} -> {} chars",
                    original.chars().count(),
                    new_content.chars().count()
                );
                let _ = app.emit(
                    "memory-updated",
                    serde_json::json!({
                        "sessionId": session_id,
                        "summary": "全局记忆已整理",
                    }),
                );
            } else {
                println!("[MEMORY] Skipped: memory was modified during consolidation");
            }
        }
        None => println!("[MEMORY] Curator returned no content"),
    }

    println!("[MEMORY] --- Memory Curator Finished ---");
}
