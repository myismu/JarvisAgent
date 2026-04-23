// --- Agent 工具模块 ---
// run_subagent, load_skill, compact, dream

use eventsource_stream::Eventsource;
use futures_util::StreamExt;
use serde_json::json;
use std::collections::HashMap;
use tauri::{Emitter, Manager};

use crate::core::models::{AnthropicRequest, Content, ContentBlock, Message};
use crate::core::config::ConfigState;
use crate::core::prompts::get_subagent_system_prompt;
use crate::get_agent_home;
use super::{load_all_skills, get_tools_definition, handle_tool_call_inner};
use crate::core::tasks::TaskManager;

/// 加载技能
pub async fn load_skill(
    _app: &tauri::AppHandle,
    input: &serde_json::Value,
) -> String {
    let skill_name = input["name"].as_str().unwrap_or("");
    let skills = load_all_skills();
    match skills.into_iter().find(|s| s.name == skill_name) {
        Some(skill) => format!("<skill name=\"{}\">\n{}\n</skill>", skill.name, skill.body),
        None => {
            let available: Vec<String> =
                load_all_skills().into_iter().map(|s| s.name).collect();
            format!(
                "错误：未找到技能 '{}'。可用技能: {:?}",
                skill_name, available
            )
        }
    }
}

/// 手动压缩上下文
pub async fn compact(
    _app: &tauri::AppHandle,
    _input: &serde_json::Value,
) -> String {
    "手动触发上下文压缩中...".to_string()
}

/// 触发记忆整理（Dream Agent）
pub async fn dream(
    _app: &tauri::AppHandle,
    _input: &serde_json::Value,
) -> String {
    let summary = TaskManager::new()
        .summary()
        .unwrap_or_else(|e| format!("生成摘要失败: {}", e));
    format!("主动触发记忆整理（Dream Agent）已启动。\n\n[记忆归档与状态同步报告]\n当前项目的全局任务状态已更新：\n\n{}\n\n请根据上述进度报告，评估下一步需要启动的核心任务，或者判断是否可以进入休息/总结状态。", summary)
}

/// 子代理执行引擎
pub async fn run_subagent(
    app: tauri::AppHandle,
    prompt: String,
    read_only: bool,
) -> (String, u64, u64) {
    // 从 ConfigState 读取配置（子代理使用 sub_model）
    let app_cfg = app.state::<ConfigState>().0.lock().await.clone();
    let cfg = app_cfg.active_config();
    if cfg.api_key.is_empty() {
        return ("子代理启动失败：未配置 API Key".to_string(), 0, 0);
    }
    let api_key = cfg.api_key;
    let base_url = cfg.base_url;
    let model_id = cfg.sub_model; // 子代理使用独立的模型配置

    let client = reqwest::Client::new();
    let mut system_prompt =
        get_subagent_system_prompt(&std::env::current_dir().unwrap().to_string_lossy());

    let skills = load_all_skills();
    if !skills.is_empty() {
        system_prompt.push_str("\n\n可用技能 (使用 load_skill 工具获取完整内容)：\n");
        for skill in &skills {
            system_prompt.push_str(&format!("  - {}: {}\n", skill.name, skill.description));
        }
    }

    let mut messages = vec![Message::User {
        content: Content::Single(prompt.clone()),
    }];

    let mut loop_count = 0;
    let mut final_answer = String::new();
    let mut sub_input_tokens: u64 = 0;
    let mut sub_output_tokens: u64 = 0;

    let mut tools = get_tools_definition("SUBAGENT");

    if read_only {
        let mutating_tools = [
            "write_file",
            "edit_file",
            "run_shell",
            "task_create",
            "task_update",
        ];
        tools.retain(|t| {
            if let Some(name) = t["name"].as_str() {
                !mutating_tools.contains(&name)
            } else {
                true
            }
        });
    }

    let mode_str = if read_only {
        "只读模式"
    } else {
        "读写模式"
    };
    let _ = app.emit(
        "chat-stream",
        json!({
            "content": format!("\n> 🤖 **[启动子代理]** ({}) 任务: `{}`\n", mode_str, prompt)
        }),
    );

    while loop_count < crate::core::constants::MAX_AGENT_LOOP_BEFORE_CONFIRM {
        let request_body = AnthropicRequest {
            model: model_id.clone(),
            max_tokens: crate::core::constants::MAX_TOKENS_CONTEXT,
            system: system_prompt.clone(),
            messages: messages.clone(),
            tools: tools.clone(),
            stream: true,
        };

        let (req_json, is_openai) = if cfg.api_format == "openai" {
            use crate::core::adapters::{translate_messages_to_openai, translate_tools_to_openai};
            use crate::core::models::OpenAIRequest;
            let openai_msgs = translate_messages_to_openai(&request_body.system, &request_body.messages);
            let openai_tools = translate_tools_to_openai(&request_body.tools);
            let openai_req = OpenAIRequest {
                model: model_id.clone(),
                max_tokens: Some(crate::core::constants::MAX_TOKENS_CONTEXT),
                messages: openai_msgs,
                tools: if openai_tools.is_empty() { None } else { Some(openai_tools) },
                stream: true,
                stream_options: Some(crate::core::models::StreamOptions { include_usage: true }),
            };
            (serde_json::to_value(openai_req).unwrap(), true)
        } else {
            (serde_json::to_value(request_body).unwrap(), false)
        };

        let mut req = client.post(&base_url)
            .header(reqwest::header::CONTENT_TYPE, "application/json");

        if is_openai {
            req = req.header("Authorization", format!("Bearer {}", api_key));
        } else {
            req = req
                .header("x-api-key", &api_key)
                .header("anthropic-version", "2023-06-01");
        }

        let response_res = req.json(&req_json).send().await;

        let response = match response_res {
            Ok(r) => r,
            Err(e) => {
                return (
                    format!("子代理请求失败: {}", e),
                    sub_input_tokens,
                    sub_output_tokens,
                )
            }
        };

        let mut stream = response.bytes_stream().eventsource();
        let mut current_blocks: Vec<ContentBlock> = Vec::new();
        let mut tool_input_buffers: HashMap<usize, String> = HashMap::new();
        let mut current_text_this_turn = String::new();

        while let Some(event_result) = stream.next().await {
            let event = match event_result {
                Ok(e) => e,
                Err(_) => continue,
            };
            let data = event.data;
            if data == "[DONE]" {
                break;
            }
            let json: serde_json::Value = serde_json::from_str(&data).unwrap_or(json!({}));

            if is_openai {
                if let Some(usage) = json.get("usage") {
                    if let Some(in_toks) = usage.get("prompt_tokens").and_then(|v| v.as_u64()) {
                        sub_input_tokens += in_toks;
                    }
                    if let Some(out_toks) = usage.get("completion_tokens").and_then(|v| v.as_u64()) {
                        sub_output_tokens += out_toks;
                    }
                }

                if let Some(choices) = json["choices"].as_array() {
                    if let Some(first) = choices.first() {
                        if let Some(delta) = first.get("delta") {
                            // Handle text content
                            if let Some(t) = delta["content"].as_str() {
                                if current_blocks.is_empty() {
                                    current_blocks.push(ContentBlock::Text { text: String::new() });
                                }
                                if let Some(ContentBlock::Text { text }) = current_blocks.last_mut() {
                                    text.push_str(t);
                                    current_text_this_turn.push_str(t);
                                }
                            }
                            // Handle tool calls
                            if let Some(tool_calls) = delta["tool_calls"].as_array() {
                                for tc in tool_calls {
                                    let index = tc["index"].as_u64().unwrap_or(0) as usize;
                                    
                                    // If first time seeing this index
                                    if !tool_input_buffers.contains_key(&index) {
                                        let id = tc["id"].as_str().unwrap_or("").to_string();
                                        let name = tc["function"]["name"].as_str().unwrap_or("").to_string();
                                        current_blocks.push(ContentBlock::ToolUse {
                                            id,
                                            name,
                                            input: json!({}),
                                        });
                                        tool_input_buffers.insert(index, String::new());
                                    }
                                    
                                    // Append arguments string
                                    if let Some(args) = tc["function"]["arguments"].as_str() {
                                        if let Some(buf) = tool_input_buffers.get_mut(&index) {
                                            buf.push_str(args);
                                        }
                                    }
                                }
                            }
                        }
                    }
                }
            } else {
                match json["type"].as_str().unwrap_or("") {
                    "message_start" => {
                        if let Some(usage) = json.get("message").and_then(|m| m.get("usage")) {
                            sub_input_tokens += usage
                                .get("input_tokens")
                                .and_then(|v| v.as_u64())
                                .unwrap_or(0);
                        }
                    }
                    "message_delta" => {
                        if let Some(usage) = json.get("usage") {
                            if let Some(in_toks) = usage.get("input_tokens").and_then(|v| v.as_u64()) {
                                sub_input_tokens += in_toks;
                            }
                            if let Some(out_toks) = usage.get("output_tokens").and_then(|v| v.as_u64())
                            {
                                sub_output_tokens += out_toks;
                            }
                        }
                    }
                    "content_block_start" => {
                        let index = json["index"].as_u64().unwrap_or(0) as usize;
                        let block = &json["content_block"];
                        match block["type"].as_str().unwrap_or("") {
                            "text" => current_blocks.push(ContentBlock::Text {
                                text: String::new(),
                            }),
                            "tool_use" => {
                                let tool_name = block["name"].as_str().unwrap_or("").to_string();
                                current_blocks.push(ContentBlock::ToolUse {
                                    id: block["id"].as_str().unwrap_or("").to_string(),
                                    name: tool_name.clone(),
                                    input: json!({}),
                                });
                                tool_input_buffers.insert(index, String::new());
                            }
                            _ => {}
                        }
                    }
                    "content_block_delta" => {
                        let index = json["index"].as_u64().unwrap_or(0) as usize;
                        let delta = &json["delta"];
                        if let Some(block) = current_blocks.get_mut(index) {
                            match block {
                                ContentBlock::Text { text } => {
                                    if let Some(t) = delta["text"].as_str() {
                                        text.push_str(t);
                                        current_text_this_turn.push_str(t);
                                    }
                                }
                                ContentBlock::ToolUse { .. } => {
                                    if let Some(partial) = delta["partial_json"].as_str() {
                                        if let Some(buf) = tool_input_buffers.get_mut(&index) {
                                            buf.push_str(partial);
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

        // 记录思考日志
        let log_dir = get_agent_home().join(crate::core::constants::DIR_LOGS);
        if !log_dir.exists() {
            let _ = std::fs::create_dir_all(&log_dir);
        }
        let thoughts_log_path = log_dir.join(crate::core::constants::FILE_THOUGHTS_LOG);
        if let Ok(mut file) = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(&thoughts_log_path)
        {
            use std::io::Write;
            let mut log_content = format!("\n## [SUB AGENT] --- Loop {} ---\n", loop_count + 1);
            if !current_text_this_turn.trim().is_empty() {
                log_content.push_str(&format!(
                    "### 子代理思考：\n{}\n\n",
                    current_text_this_turn.trim()
                ));
            }
            if tool_input_buffers.is_empty() {
                log_content.push_str("### 最终决断：\n子代理完成任务并返回结果。\n");
            } else {
                log_content.push_str("### 决定执行操作：\n");
                for (idx, buf) in tool_input_buffers.iter() {
                    if let Some(ContentBlock::ToolUse { name, .. }) = current_blocks.get(*idx) {
                        log_content.push_str(&format!("- 工具: `{}`\n  参数: `{}`\n", name, buf));
                    }
                }
            }
            let _ = writeln!(file, "{}\n---\n", log_content);
        }

        // 执行工具调用
        let mut tool_results = Vec::new();
        for (index, buf) in tool_input_buffers {
            if let Some(ContentBlock::ToolUse {
                name, input, id, ..
            }) = current_blocks.get_mut(index)
            {
                if let Ok(parsed_input) = serde_json::from_str::<serde_json::Value>(&buf) {
                    *input = parsed_input;

                    let _ = app.emit(
                        "chat-stream",
                        json!({
                            "content": format!("\n>   └─ 子代理使用工具: `{}`\n", name)
                        }),
                    );

                    let output = handle_tool_call_inner(&app, name, input).await;

                    tool_results.push(ContentBlock::ToolResult {
                        tool_use_id: id.clone(),
                        content: output,
                    });
                }
            }
        }

        messages.push(Message::Assistant {
            content: Content::Multiple(current_blocks),
        });

        if tool_results.is_empty() {
            final_answer = current_text_this_turn;
            break;
        } else {
            messages.push(Message::User {
                content: Content::Multiple(tool_results),
            });
        }
        loop_count += 1;
    }

    let _ = app.emit(
        "chat-stream",
        json!({
            "content": format!("\n> 🤖 **[子代理执行完毕]**\n")
        }),
    );

    if loop_count >= crate::core::constants::MAX_AGENT_LOOP_BEFORE_CONFIRM && final_answer.is_empty() {
        return (
            format!("子代理执行达到 {} 轮上限，已停止。", crate::core::constants::MAX_AGENT_LOOP_BEFORE_CONFIRM),
            sub_input_tokens,
            sub_output_tokens,
        );
    } else {
        (final_answer, sub_input_tokens, sub_output_tokens)
    }
}

/// 方案审批工具：将实施方案推送到前端预览面板，等待用户确认或拒绝
pub async fn propose_plan(
    app: &tauri::AppHandle,
    input: &serde_json::Value,
) -> String {
    let title = input["title"].as_str().unwrap_or("实施方案");
    let content = input["content"].as_str().unwrap_or("");

    if content.is_empty() {
        return "错误：方案内容不能为空。".to_string();
    }

    // 生成唯一 ID
    use std::sync::atomic::{AtomicUsize, Ordering};
    static PLAN_REQ_ID: AtomicUsize = AtomicUsize::new(1);
    let id = format!("plan_{}", PLAN_REQ_ID.fetch_add(1, Ordering::SeqCst));

    // 创建 oneshot channel 等待用户决策
    let (tx, rx) = tokio::sync::oneshot::channel();
    app.state::<crate::core::PendingPermissions>()
        .0
        .lock()
        .await
        .insert(id.clone(), tx);

    // 同时将方案文件保存到 .plans 目录以便持久化
    let plans_dir = get_agent_home().join(crate::core::constants::DIR_PLANS);
    if !plans_dir.exists() {
        let _ = std::fs::create_dir_all(&plans_dir);
    }
    let plan_filename = format!("{}_{}.md", id, title.chars().take(20).collect::<String>());
    let plan_path = plans_dir.join(&plan_filename);
    let full_content = format!("# {}\n\n{}", title, content);
    let _ = std::fs::write(&plan_path, &full_content);

    println!("[JARVIS] 方案已推送到前端预览: {} ({})", title, id);

    // 发送事件到前端，触发方案预览面板
    let _ = app.emit(
        "plan-proposal",
        json!({
            "id": id,
            "title": title,
            "content": content,
        }),
    );

    // 在聊天流中也提示一下
    let _ = app.emit(
        "chat-stream",
        json!({
            "content": format!("\n> 📋 **方案已提交审阅**: 「{}」\n> 请在弹出的方案预览面板中查看详情并决策。\n", title)
        }),
    );

    // 阻塞等待用户决策（通过 resolve_permission 回调）
    let decision = rx.await.unwrap_or_else(|_| "reject".to_string());

    if decision == "reject" {
        println!("[JARVIS] 用户拒绝了方案: {}", title);
        format!("用户已拒绝此方案「{}」。请根据用户意见进行调整，或询问用户想要修改的部分。严禁继续创建 task_create 任务！", title)
    } else {
        println!("[JARVIS] 用户同意了方案: {}", title);
        format!("用户已同意方案「{}」！现在可以使用 task_create 创建持久化任务，并使用 task 工具委派子代理开始执行。", title)
    }
}
