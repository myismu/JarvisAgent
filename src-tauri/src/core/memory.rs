use std::path::{Path, PathBuf};
use serde_json::json;
use reqwest::header::CONTENT_TYPE;
use crate::core::api_format::ApiFormat;
use crate::core::error::MemoryError;
use crate::core::models::*;
use crate::core::prompts::*;
use crate::get_agent_home;

// --- 记忆压缩与上下文管理 (Context Compact) ---

pub fn estimate_tokens(messages: &[Message]) -> usize {
    let mut total_chars = 0;
    for msg in messages {
        match msg {
            Message::User { content } | Message::Assistant { content } => {
                match content {
                    Content::Single(text) => {
                        total_chars += text.len();
                    }
                    Content::Multiple(blocks) => {
                        for block in blocks {
                            match block {
                                ContentBlock::Text { text } => {
                                    total_chars += text.len();
                                }
                                ContentBlock::Thinking { thinking, .. } => {
                                    total_chars += thinking.len();
                                }
                                ContentBlock::ToolUse { name, input, .. } => {
                                    total_chars += name.len();
                                    total_chars += input.to_string().len();
                                }
                                ContentBlock::ToolResult { content, .. } => {
                                    total_chars += content.len();
                                }
                                ContentBlock::Image { .. } => {
                                    total_chars += 1000;
                                }
                            }
                        }
                    }
                }
            }
        }
    }
    total_chars / 4
}

pub fn micro_compact(messages: &mut Vec<Message>) {
    let keep_recent = 3;
    let mut tool_results_pos = Vec::new();
    for (i, msg) in messages.iter().enumerate() {
        if let Message::User { content: Content::Multiple(blocks) } = msg {
            for (j, block) in blocks.iter().enumerate() {
                if let ContentBlock::ToolResult { .. } = block {
                    tool_results_pos.push((i, j));
                }
            }
        }
    }
    
    if tool_results_pos.len() <= keep_recent {
        return;
    }
    
    let mut tool_name_map = std::collections::HashMap::new();
    for msg in messages.iter() {
        if let Message::Assistant { content: Content::Multiple(blocks) } = msg {
            for block in blocks {
                if let ContentBlock::ToolUse { id, name, .. } = block {
                    tool_name_map.insert(id.clone(), name.clone());
                }
            }
        }
    }
    
    let to_clear_count = tool_results_pos.len() - keep_recent;
    for &(i, j) in tool_results_pos.iter().take(to_clear_count) {
        if let Message::User { content: Content::Multiple(ref mut blocks) } = messages[i] {
            if let ContentBlock::ToolResult { tool_use_id, content } = &mut blocks[j] {
                if content.len() > 100 {
                    let tool_name = tool_name_map.get(tool_use_id).cloned().unwrap_or_else(|| "unknown".to_string());
                    *content = format!("[Previous: used {}]", tool_name);
                }
            }
        }
    }
}

pub fn append_transcript(text: &str) -> Result<String, MemoryError> {
    let transcript_dir = get_agent_home().join(crate::core::constants::DIR_TRANSCRIPTS);
    if !transcript_dir.exists() {
        let _ = std::fs::create_dir_all(&transcript_dir);
    }

    let timestamp = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_secs();
    let transcript_path = transcript_dir.join(format!("transcript_{}.jsonl", timestamp));
    std::fs::write(&transcript_path, text).map_err(|e| MemoryError::FileRead(e.to_string()))?;
    Ok(transcript_path.to_string_lossy().to_string())
}

pub async fn auto_compact(messages: &mut Vec<Message>, client: &reqwest::Client, api_key: &str, base_url: &str, model_id: &str, api_format: ApiFormat) -> Result<(), MemoryError> {
    let mut json_content = String::new();
    for msg in messages.iter() {
        if let Ok(m) = serde_json::to_string(msg) {
            json_content.push_str(&m);
            json_content.push('\n');
        }
    }
    
    let summarized_text = if json_content.len() > 150000 {
        format!("(truncated...){}", &json_content[json_content.len() - 150000..])
    } else {
        json_content.clone()
    };
    
    let transcript_path = append_transcript(&json_content)?;
    println!("[auto_compact] Transcript saved to {}", transcript_path);

    let summary_prompt = format!("Summarize this conversation for continuity. Include: \n1) What was accomplished, 2) Current state, 3) Key decisions made. \nBe concise but preserve critical details.\n\n{}", summarized_text);

    let request_body = AnthropicRequest {
        model: model_id.to_string(),
        max_tokens: 2000,
        system: "You are a summarizing agent.".to_string(),
        messages: vec![Message::User { content: Content::Single(summary_prompt) }],
        tools: vec![],
        stream: false,
        thinking: None,
        temperature: None,
        top_p: None,
        top_k: None,
    };
    
    let (req_json, is_openai) = match api_format {
        ApiFormat::OpenAI => {
            use crate::core::adapters::translate_messages_to_openai;
            use crate::core::models::OpenAIRequest;
            let openai_msgs = translate_messages_to_openai(&request_body.system, &request_body.messages);
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
                temperature: request_body.temperature,
                top_p: request_body.top_p,
            };
            (serde_json::to_value(openai_req).unwrap(), true)
        }
        ApiFormat::Anthropic => {
            (serde_json::to_value(request_body).unwrap(), false)
        }
    };

    let (auth_header, auth_value) = api_format.auth_header(api_key);
    let mut req = client.post(base_url)
        .header(CONTENT_TYPE, "application/json")
        .header(auth_header, &auth_value);

    if api_format.requires_anthropic_version() {
        req = req.header("anthropic-version", "2023-06-01");
    }
    
    let response = req.json(&req_json)
        .send()
        .await
        .map_err(|e| MemoryError::CompactionFailed(format!("auto_compact request failed: {}", e)))?;

    let body: serde_json::Value = response.json().await
        .map_err(|e| MemoryError::CompactionFailed(format!("auto_compact response parse failed: {}", e)))?;
    
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
        return Err(MemoryError::CompactionFailed("Failed to get summary text".to_string()));
    };
    let summary = text;

    messages.clear();
    messages.push(Message::User {
        content: Content::Single(format!("[Conversation compressed. Transcript: {:?}]\n\n{}", transcript_path, summary))
    });
    messages.push(Message::Assistant {
        content: Content::Single("Understood. I have the context from the summary. Continuing.".to_string())
    });
    
    Ok(())
}

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
            use crate::core::adapters::translate_messages_to_openai;
            use crate::core::models::OpenAIRequest;
            let openai_msgs = translate_messages_to_openai(&request_body.system, &request_body.messages);
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
                temperature: request_body.temperature,
                top_p: request_body.top_p,
            };
            (serde_json::to_value(openai_req).unwrap(), true)
        }
        ApiFormat::Anthropic => {
            (serde_json::to_value(request_body).unwrap(), false)
        }
    };

    let (auth_header, auth_value) = api_format.auth_header(api_key);
    let mut req = client.post(base_url)
        .header(CONTENT_TYPE, "application/json")
        .header(auth_header, &auth_value);

    if api_format.requires_anthropic_version() {
        req = req.header("anthropic-version", "2023-06-01");
    }

    let response = req.json(&req_json)
        .send()
        .await
        .map_err(|e| MemoryError::CompactionFailed(format!("summary request failed: {}", e)))?;

    let body: serde_json::Value = response.json().await
        .map_err(|e| MemoryError::CompactionFailed(format!("summary response parse failed: {}", e)))?;

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
        return Err(MemoryError::CompactionFailed("Failed to get summary text".to_string()));
    }

    Ok(text)
}

// --- 4. Memory System (Claude Style) ---

pub fn get_global_memory_path() -> PathBuf {
    get_agent_home().join(crate::core::constants::FILE_GLOBAL_MEMORY)
}

pub fn get_project_memory_path() -> PathBuf {
    let mut path = get_agent_home().clone();
    path.push("memory");
    path.push("GEMINI.md");
    path
}

pub fn read_memory_file(path: &Path, header: &str) -> String {
    if let Ok(content) = std::fs::read_to_string(path) {
        content
    } else {
        let initial = format!("# {}\n\n(暂无记录)\n", header);
        let _ = std::fs::create_dir_all(path.parent().unwrap());
        let _ = std::fs::write(path, &initial);
        initial
    }
}

use crate::core::config::AgentConfig;

pub async fn run_memory_agent(user_msg: String, assistant_reply: String, config: AgentConfig) {
    println!("\n[MEMORY] --- Memory Agent Started ---");

    if config.api_key.is_empty() {
        return;
    }
    let api_key = config.api_key;
    let base_url = config.base_url;
    let model_id = config.utility_model; // 记忆 Agent 使用工具模型（更便宜）

    let global_path = get_global_memory_path();
    let project_path = get_project_memory_path();
    
    let global_content = read_memory_file(&global_path, "Global Memory");
    let project_content = read_memory_file(&project_path, "Project Memory");

    // let system = MEMORY_AGENT_SYSTEM;

    let user_content = format!(
        "【当前全局记忆】\n{}\n\n【当前项目记忆】\n{}\n\n【最新对话】\nUser: {}\nAssistant: {}",
        global_content, project_content, user_msg, assistant_reply
    );

    let tools = vec![json!({
        "name": "update_memory",
        "description": "更新记忆文件。",
        "input_schema": {
            "type": "object",
            "properties": {
                "scope": { "type": "string", "enum": ["global", "project"], "description": "更新范围" },
                "content": { "type": "string", "description": "更新后的完整 Markdown 内容" }
            },
            "required": ["scope", "content"]
        }
    })];

    let client = reqwest::Client::new();
    let request_body = AnthropicRequest {
        model: model_id.clone(),
        max_tokens: crate::core::constants::MAX_TOKENS_CONTEXT,
        system: MEMORY_AGENT_SYSTEM.to_string(),
        messages: vec![Message::User { content: Content::Single(user_content) }],
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
            use crate::core::adapters::{translate_messages_to_openai, translate_tools_to_openai};
            use crate::core::models::OpenAIRequest;
            let openai_msgs = translate_messages_to_openai(&request_body.system, &request_body.messages);
            let openai_tools = translate_tools_to_openai(&request_body.tools);
            let openai_req = OpenAIRequest {
                model: model_id.clone(),
                max_tokens: Some(crate::core::constants::MAX_TOKENS_CONTEXT),
                messages: openai_msgs,
                tools: if openai_tools.is_empty() { None } else { Some(openai_tools) },
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
    let logger = crate::core::debug_logger::DebugLogger::new();
    logger.log_request_to_terminal("MEMORY AGENT", 1, &request_json_str);

    let (auth_header, auth_value) = api_format.auth_header(&api_key);
    let mut req = client.post(&base_url)
        .header(CONTENT_TYPE, "application/json")
        .header(auth_header, &auth_value);

    if api_format.requires_anthropic_version() {
        req = req.header("anthropic-version", "2023-06-01");
    }

    if let Ok(response) = req.json(&req_json).send().await {
        if let Ok(body) = response.json::<serde_json::Value>().await {
            if is_openai {
                if let Some(choices) = body["choices"].as_array() {
                    if let Some(first) = choices.first() {
                        if let Some(tool_calls) = first["message"]["tool_calls"].as_array() {
                            for tc in tool_calls {
                                if tc["type"] == "function" && tc["function"]["name"] == "update_memory" {
                                    if let Some(args_str) = tc["function"]["arguments"].as_str() {
                                        if let Ok(args_json) = serde_json::from_str::<serde_json::Value>(args_str) {
                                            let scope = args_json["scope"].as_str().unwrap_or("");
                                            let content = args_json["content"].as_str().unwrap_or("");
                                            let target_path = if scope == "global" { &global_path } else { &project_path };
                                            
                                            if !content.is_empty() {
                                                println!("[MEMORY] Updating {} memory (OpenAI)...", scope);
                                                let _ = std::fs::write(target_path, content);
                                                logger.log_memory_agent(&request_json_str, &format!("Updated {} memory", scope));
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
                            let scope = block["input"]["scope"].as_str().unwrap_or("");
                            let content = block["input"]["content"].as_str().unwrap_or("");
                            let target_path = if scope == "global" { &global_path } else { &project_path };
                            
                            if !content.is_empty() {
                                println!("[MEMORY] Updating {} memory (Anthropic)...", scope);
                                let _ = std::fs::write(target_path, content);
                                logger.log_memory_agent(&request_json_str, &format!("Updated {} memory", scope));
                            }
                        }
                    }
                }
            }
        }
    }
    println!("[MEMORY] --- Memory Agent Finished ---");
}
