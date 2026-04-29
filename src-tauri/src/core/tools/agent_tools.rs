//! # agent_tools.rs — Agent 专用工具模块
//!
//! 包含子代理执行引擎（`run_subagent`）、技能加载、上下文压缩、记忆整理、方案审批等工具。
//! `run_subagent` 是最复杂的工具，包含完整的 SSE 流式处理和并行工具执行循环。
//!
//! ## 关键导出
//! - `run_subagent()`: 子代理执行引擎（独立 Agent Loop，支持只读/读写模式）
//! - `load_skill()`: 按名称加载技能知识
//! - `compact()`: 手动触发上下文压缩
//! - `dream()`: 触发记忆整理（Dream Agent）
//! - `propose_plan()`: 方案审批工具，推送方案到前端并阻塞等待用户决策
//!
//! ## 依赖
//! - Internal: `crate::core::orchestration::subagents`, `crate::core::orchestration::tasks`, `crate::core::llm::adapters`
//! - External: `eventsource_stream`, `futures_util`, `serde_json`, `tauri`
//!
//! ## 约束
//! - 子代理与主代理共用同一模型（main_model）
//! - 只读模式会过滤掉 write_file / edit_file / run_shell 等写操作工具
//! - 子代理循环次数受 `MAX_AGENT_LOOP_BEFORE_CONFIRM` 限制

use eventsource_stream::Eventsource;
use futures_util::StreamExt;
use serde_json::json;
use std::collections::HashMap;
use tauri::{Emitter, Manager};

use super::{get_tools_definition, handle_tool_call_inner_owned, load_all_skills};
use crate::core::llm::adapters::parse_streamed_tool_input;
use crate::core::config::ConfigState;
use crate::core::models::{AnthropicRequest, Content, ContentBlock, Message, PlanDocument};
use crate::core::infra::prompts::get_subagent_system_prompt;
use crate::core::orchestration::subagents::{SubAgentMonitor, SubAgentPhase};
use crate::core::orchestration::tasks::TaskManager;
use crate::core::tools::registry::ToolDef;
use crate::get_agent_home;

/// 加载技能
pub async fn load_skill(
    _app: &tauri::AppHandle,
    input: &serde_json::Value,
    _session_id: &str,
) -> String {
    let skill_name = input["name"].as_str().unwrap_or("");
    let skills = load_all_skills();
    match skills.into_iter().find(|s| s.name == skill_name) {
        Some(skill) => format!("<skill name=\"{}\">\n{}\n</skill>", skill.name, skill.body),
        None => {
            let available: Vec<String> = load_all_skills().into_iter().map(|s| s.name).collect();
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
    _session_id: &str,
) -> String {
    "手动触发上下文压缩中...".to_string()
}

/// 触发记忆整理（Dream Agent）
pub async fn dream(
    _app: &tauri::AppHandle,
    _input: &serde_json::Value,
    _session_id: &str,
) -> String {
    let summary = TaskManager::new()
        .summary()
        .unwrap_or_else(|e| format!("生成摘要失败: {}", e));
    format!("主动触发记忆整理（Dream Agent）已启动。\n\n[记忆归档与状态同步报告]\n当前项目的全局任务状态已更新：\n\n{}\n\n请根据上述进度报告，评估下一步需要启动的核心任务，或者判断是否可以进入休息/总结状态。", summary)
}

/// 子代理执行引擎：独立 Agent Loop，支持只读/读写模式，返回 (结果, 输入token, 输出token)
pub async fn run_subagent(
    app: tauri::AppHandle,
    prompt: String,
    read_only: bool,
    session_id: String,
    task_id: Option<i32>,
    label: Option<String>,
) -> (String, u64, u64) {
    // 注册子代理运行记录
    let run_id =
        SubAgentMonitor::start_run(&app, &session_id, &prompt, read_only, task_id, label).await;

    // 从 ConfigState 读取配置
    let app_cfg = app.state::<ConfigState>().0.lock().await.clone();
    let cfg = app_cfg.active_config();
    if cfg.api_key.is_empty() {
        SubAgentMonitor::fail_run(&app, &run_id, "Missing API key".to_string(), 0, 0).await;
        return ("子代理启动失败：未配置 API Key".to_string(), 0, 0);
    }
    let api_format_enum = cfg.api_format_enum();
    let api_key = cfg.api_key;
    let base_url = cfg.base_url;
    let model_id = cfg.main_model; // 子代理与主代理共用同一模型

    let client = reqwest::Client::new();
    // 优先使用会话工作目录，否则回退到进程 CWD
    let ws = if let Some(manager) = app.try_state::<crate::core::state::SessionManager>() {
        let ctx = manager.get_or_create(&session_id).await;
        let ws_val = ctx.workspace.lock().await.clone();
        ws_val
    } else {
        None
    };
    let cwd = ws
        .as_ref()
        .map(|p| p.to_string_lossy().to_string())
        .unwrap_or_else(|| {
            std::env::current_dir()
                .unwrap()
                .to_string_lossy()
                .to_string()
        });
    let ws_str = ws.as_ref().map(|p| p.to_string_lossy().to_string());
    let mut system_prompt = get_subagent_system_prompt(&cwd, ws_str.as_deref());

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

    let mut tools = get_tools_definition("SUBAGENT", &[]);

    // 只读模式：过滤掉所有写操作工具
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
            "content": format!("\n> ◆ **[启动子代理]** ({}) 任务: `{}`\n", mode_str, prompt),
            "sessionId": session_id.clone(),
            "isSubAgent": true
        }),
    );
    let _ = app.emit(
        "agent-step",
        json!({
            "type": "subagent_start",
            "task": format!("{} - {}", mode_str, prompt.chars().take(100).collect::<String>()),
            "sessionId": session_id.clone(),
            "isSubAgent": true
        }),
    );

    while loop_count < crate::core::constants::MAX_AGENT_LOOP_BEFORE_CONFIRM {
        if SubAgentMonitor::is_cancelled(&app, &run_id).await {
            SubAgentMonitor::acknowledge_cancelled(&app, &run_id).await;
            return (
                "子代理已取消。".to_string(),
                sub_input_tokens,
                sub_output_tokens,
            );
        }

        SubAgentMonitor::update_phase(
            &app,
            &run_id,
            SubAgentPhase::WaitingModel,
            loop_count + 1,
            sub_input_tokens,
            sub_output_tokens,
        )
        .await;

        let mut request_body = AnthropicRequest {
            model: model_id.clone(),
            max_tokens: crate::core::constants::MAX_TOKENS_CONTEXT,
            system: system_prompt.clone(),
            messages: messages.clone(),
            tools: tools.clone(),
            stream: true,
            thinking: None,
            temperature: cfg.temperature,
            top_p: cfg.top_p,
            top_k: cfg.top_k,
        };

        if cfg.enable_thinking.unwrap_or(false) {
            request_body.thinking = Some(crate::core::models::ThinkingConfig {
                r#type: "enabled".to_string(),
                budget_tokens: Some(1024),
            });
            if request_body.max_tokens <= 1024 {
                request_body.max_tokens = 4096;
            }
        }

        let (req_json, is_openai) = if api_format_enum.is_openai() {
            use crate::core::llm::adapters::{
                should_backfill_deepseek_reasoning_content,
                translate_messages_to_openai_with_reasoning_backfill, translate_tools_to_openai,
            };
            use crate::core::models::OpenAIRequest;
            let backfill_reasoning_content = should_backfill_deepseek_reasoning_content(
                &model_id,
                &base_url,
                cfg.enable_thinking.unwrap_or(false),
            );
            let openai_msgs = translate_messages_to_openai_with_reasoning_backfill(
                &request_body.system,
                &request_body.messages,
                backfill_reasoning_content,
            );
            let openai_tools = translate_tools_to_openai(&request_body.tools);
            let mut openai_req = OpenAIRequest {
                model: model_id.clone(),
                max_tokens: Some(crate::core::constants::MAX_TOKENS_CONTEXT),
                messages: openai_msgs,
                tools: if openai_tools.is_empty() {
                    None
                } else {
                    Some(openai_tools)
                },
                stream: true,
                stream_options: Some(crate::core::models::StreamOptions {
                    include_usage: true,
                }),
                reasoning_effort: None,
                thinking: None,
                thinking_budget: None,
                enable_thinking: None,
                temperature: request_body.temperature,
                top_p: request_body.top_p,
            };

            let should_think = cfg.enable_thinking.unwrap_or(false);
            let thinking_param =
                crate::core::llm::registry::query_capabilities(&model_id).and_then(|c| c.thinking_param);

            match thinking_param.as_deref() {
                Some("reasoning_effort") => {
                    if should_think {
                        openai_req.reasoning_effort = Some("high".to_string());
                    }
                }
                Some("thinking") => {
                    openai_req.thinking = Some(crate::core::models::ThinkingConfig {
                        r#type: if should_think {
                            "enabled".to_string()
                        } else {
                            "disabled".to_string()
                        },
                        budget_tokens: None,
                    });
                }
                Some("thinkingBudget") => {
                    openai_req.thinking_budget = Some(if should_think { 8192 } else { 0 });
                }
                Some("enable_thinking") => {
                    openai_req.enable_thinking = Some(should_think);
                }
                _ => {
                    if should_think {
                        openai_req.reasoning_effort = Some("high".to_string());
                    }
                }
            }
            (serde_json::to_value(openai_req).unwrap(), true)
        } else {
            (serde_json::to_value(request_body).unwrap(), false)
        };

        let request_json_str = serde_json::to_string_pretty(&req_json).unwrap_or_default();
        let logger = crate::core::infra::debug_logger::DebugLogger::new();
        logger.log_request_to_terminal("SUB AGENT", loop_count + 1, &request_json_str);
        logger.log_request_to_file("SUB AGENT", loop_count + 1, &request_json_str);

        let (auth_header, auth_value) = api_format_enum.auth_header(&api_key);
        let mut req = client
            .post(&base_url)
            .header(reqwest::header::CONTENT_TYPE, "application/json")
            .header(auth_header, &auth_value);

        if api_format_enum.requires_anthropic_version() {
            req = req.header("anthropic-version", "2023-06-01");
        }

        crate::core::llm::api_client::log_model_request(&model_id, &base_url, "子agent");

        let response_res = req.json(&req_json).send().await;

        if SubAgentMonitor::is_cancelled(&app, &run_id).await {
            SubAgentMonitor::acknowledge_cancelled(&app, &run_id).await;
            return (
                "子代理已取消。".to_string(),
                sub_input_tokens,
                sub_output_tokens,
            );
        }

        let response = match response_res {
            Ok(r) => r,
            Err(e) => {
                SubAgentMonitor::fail_run(
                    &app,
                    &run_id,
                    format!("Subagent request failed: {}", e),
                    sub_input_tokens,
                    sub_output_tokens,
                )
                .await;
                return (
                    format!("子代理请求失败: {}", e),
                    sub_input_tokens,
                    sub_output_tokens,
                );
            }
        };

        SubAgentMonitor::update_phase(
            &app,
            &run_id,
            SubAgentPhase::Streaming,
            loop_count + 1,
            sub_input_tokens,
            sub_output_tokens,
        )
        .await;

        let mut stream = response.bytes_stream().eventsource();
        let mut current_blocks: Vec<ContentBlock> = Vec::new();
        let mut tool_input_buffers: HashMap<usize, String> = HashMap::new();
        let mut openai_tool_block_map: HashMap<usize, usize> = HashMap::new();
        let mut current_text_this_turn = String::new();
        let mut current_thinking_this_turn = String::new();
        let mut reported_thinking_this_turn = false;
        let sub_cancel_token = SubAgentMonitor::cancel_token(&app, &run_id).await;

        loop {
            let event_result = if let Some(token) = sub_cancel_token.as_ref() {
                tokio::select! {
                    next = stream.next() => next,
                    _ = token.cancelled() => {
                        SubAgentMonitor::acknowledge_cancelled(&app, &run_id).await;
                        return ("子代理已取消。".to_string(), sub_input_tokens, sub_output_tokens);
                    }
                }
            } else {
                stream.next().await
            };
            let Some(event_result) = event_result else {
                break;
            };
            if SubAgentMonitor::is_cancelled(&app, &run_id).await {
                SubAgentMonitor::acknowledge_cancelled(&app, &run_id).await;
                return (
                    "子代理已取消。".to_string(),
                    sub_input_tokens,
                    sub_output_tokens,
                );
            }

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
                    if let Some(out_toks) = usage.get("completion_tokens").and_then(|v| v.as_u64())
                    {
                        sub_output_tokens += out_toks;
                    }
                }

                if let Some(choices) = json["choices"].as_array() {
                    if let Some(first) = choices.first() {
                        if let Some(delta) = first.get("delta") {
                            // Handle text content
                            if let Some(t) = delta["content"].as_str() {
                                let is_text = matches!(
                                    current_blocks.last(),
                                    Some(ContentBlock::Text { .. })
                                );
                                if !is_text {
                                    current_blocks.push(ContentBlock::Text {
                                        text: String::new(),
                                    });
                                }
                                if let Some(ContentBlock::Text { text }) = current_blocks.last_mut()
                                {
                                    text.push_str(t);
                                    current_text_this_turn.push_str(t);
                                }
                            }
                            // Handle reasoning content (DeepSeek OpenAI format)
                            if let Some(t) = delta["reasoning_content"].as_str() {
                                let is_thinking = matches!(
                                    current_blocks.last(),
                                    Some(ContentBlock::Thinking { .. })
                                );
                                if !is_thinking {
                                    current_blocks.push(ContentBlock::Thinking {
                                        thinking: String::new(),
                                        signature: String::new(),
                                    });
                                }
                                if let Some(ContentBlock::Thinking { thinking, .. }) =
                                    current_blocks.last_mut()
                                {
                                    thinking.push_str(t);
                                    current_thinking_this_turn.push_str(t);
                                    if !reported_thinking_this_turn {
                                        SubAgentMonitor::update_phase(
                                            &app,
                                            &run_id,
                                            SubAgentPhase::Thinking,
                                            loop_count + 1,
                                            sub_input_tokens,
                                            sub_output_tokens,
                                        )
                                        .await;
                                        reported_thinking_this_turn = true;
                                    }
                                    let _ = app.emit(
                                        "chat-thinking",
                                        json!({ "content": t, "sessionId": session_id.clone(), "isSubAgent": true }),
                                    );
                                }
                            }
                            // Handle tool calls
                            if let Some(tool_calls) = delta["tool_calls"].as_array() {
                                for tc in tool_calls {
                                    let tool_call_index =
                                        tc["index"].as_u64().unwrap_or(0) as usize;

                                    // If first time seeing this index
                                    if !openai_tool_block_map.contains_key(&tool_call_index) {
                                        let id = tc["id"].as_str().unwrap_or("").to_string();
                                        let name = tc["function"]["name"]
                                            .as_str()
                                            .unwrap_or("")
                                            .to_string();
                                        current_blocks.push(ContentBlock::ToolUse {
                                            id,
                                            name,
                                            input: json!({}),
                                        });
                                        let block_index = current_blocks.len() - 1;
                                        openai_tool_block_map.insert(tool_call_index, block_index);
                                        tool_input_buffers.insert(block_index, String::new());
                                    }

                                    // Append arguments string
                                    if let Some(args) = tc["function"]["arguments"].as_str() {
                                        if let Some(block_index) =
                                            openai_tool_block_map.get(&tool_call_index)
                                        {
                                            if let Some(buf) =
                                                tool_input_buffers.get_mut(block_index)
                                            {
                                                buf.push_str(args);
                                            }
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
                            if let Some(in_toks) =
                                usage.get("input_tokens").and_then(|v| v.as_u64())
                            {
                                sub_input_tokens += in_toks;
                            }
                            if let Some(out_toks) =
                                usage.get("output_tokens").and_then(|v| v.as_u64())
                            {
                                sub_output_tokens += out_toks;
                            }
                        }
                    }
                    "content_block_start" => {
                        let _index = json["index"].as_u64().unwrap_or(0) as usize;
                        let block = &json["content_block"];
                        match block["type"].as_str().unwrap_or("") {
                            "text" => current_blocks.push(ContentBlock::Text {
                                text: String::new(),
                            }),
                            "thinking" => current_blocks.push(ContentBlock::Thinking {
                                thinking: String::new(),
                                signature: block["signature"].as_str().unwrap_or("").to_string(),
                            }),
                            "tool_use" => {
                                let tool_name = block["name"].as_str().unwrap_or("").to_string();
                                current_blocks.push(ContentBlock::ToolUse {
                                    id: block["id"].as_str().unwrap_or("").to_string(),
                                    name: tool_name.clone(),
                                    input: json!({}),
                                });
                                tool_input_buffers.insert(current_blocks.len() - 1, String::new());
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
                                ContentBlock::Thinking { thinking, .. } => {
                                    if let Some(t) = delta["thinking"].as_str() {
                                        thinking.push_str(t);
                                        current_thinking_this_turn.push_str(t);
                                        if !reported_thinking_this_turn {
                                            SubAgentMonitor::update_phase(
                                                &app,
                                                &run_id,
                                                SubAgentPhase::Thinking,
                                                loop_count + 1,
                                                sub_input_tokens,
                                                sub_output_tokens,
                                            )
                                            .await;
                                            reported_thinking_this_turn = true;
                                        }
                                        let _ = app.emit("chat-thinking", json!({ "content": t, "sessionId": session_id.clone(), "isSubAgent": true }));
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

        let tool_calls: Vec<(String, String)> = tool_input_buffers
            .iter()
            .filter_map(|(idx, buf)| {
                if let Some(ContentBlock::ToolUse { name, .. }) = current_blocks.get(*idx) {
                    Some((name.clone(), buf.clone()))
                } else {
                    None
                }
            })
            .collect();
        logger.log_thoughts(
            "SUB AGENT",
            loop_count + 1,
            &current_thinking_this_turn,
            &current_text_this_turn,
            &tool_calls,
            sub_input_tokens,
            sub_output_tokens,
        );

        // 执行工具调用（并行模式）
        // 子Agent工具任务数据
        struct SubToolTaskData {
            index: usize,
            tool_use_id: String,
            name: String,
            input: serde_json::Value,
        }
        struct SubToolTaskResult {
            index: usize,
            tool_use_id: String,
            name: String,
            output: String,
        }

        // 阶段 1：预处理（串行） — 解析参数、emit 事件、收集任务
        if SubAgentMonitor::is_cancelled(&app, &run_id).await {
            SubAgentMonitor::acknowledge_cancelled(&app, &run_id).await;
            return (
                "子代理已取消。".to_string(),
                sub_input_tokens,
                sub_output_tokens,
            );
        }

        let mut spawn_tasks: Vec<SubToolTaskData> = Vec::new();
        let mut immediate_results: Vec<SubToolTaskResult> = Vec::new();

        for (index, buf) in tool_input_buffers {
            if let Some(ContentBlock::ToolUse {
                name, input, id, ..
            }) = current_blocks.get_mut(index)
            {
                match parse_streamed_tool_input(&buf) {
                    Ok((parsed_input, recovered)) => {
                        *input = parsed_input;
                        let input_summary = {
                            let raw = input.to_string();
                            if raw.chars().count() > 160 {
                                format!("{}...", raw.chars().take(160).collect::<String>())
                            } else {
                                raw
                            }
                        };
                        SubAgentMonitor::update_tool(
                            &app,
                            &run_id,
                            name,
                            Some(input_summary.clone()),
                            loop_count + 1,
                            sub_input_tokens,
                            sub_output_tokens,
                        )
                        .await;

                        if recovered {
                            let _ = app.emit(
                                "chat-stream",
                                json!({
                                    "content": format!("\n>   └─ 子代理自动修复了工具 `{}` 的流式参数格式\n", name),
                                    "sessionId": session_id.clone(),
                                    "isSubAgent": true
                                }),
                            );
                        }

                        let _ = app.emit(
                            "chat-stream",
                            json!({
                                "content": format!("\n>   └─ 子代理使用工具: `{}`\n", name),
                                "sessionId": session_id.clone(),
                                "isSubAgent": true
                            }),
                        );

                        spawn_tasks.push(SubToolTaskData {
                            index,
                            tool_use_id: id.clone(),
                            name: name.clone(),
                            input: input.clone(),
                        });
                    }
                    Err(err) => {
                        let preview: String = buf.chars().take(500).collect();
                        let truncated = if buf.chars().count() > 500 {
                            format!("{}...(truncated)", preview)
                        } else {
                            preview
                        };
                        SubAgentMonitor::update_tool(
                            &app,
                            &run_id,
                            name,
                            Some(truncated.clone()),
                            loop_count + 1,
                            sub_input_tokens,
                            sub_output_tokens,
                        )
                        .await;
                        let failure = format!(
                            "子代理工具 `{}` 参数解析失败：{}\n原始参数片段：{}",
                            name, err, truncated
                        );
                        println!("[SUBAGENT] {}", failure);
                        let _ = app.emit(
                            "chat-stream",
                            json!({
                                "content": format!("\n>   └─ 子代理工具 `{}` 参数解析失败\n>   错误: `{}`\n", name, err),
                                "sessionId": session_id.clone(),
                                "isSubAgent": true
                            }),
                        );
                        SubAgentMonitor::record_tool_result(
                            &app,
                            &run_id,
                            name,
                            Some(failure.chars().take(180).collect::<String>()),
                            loop_count + 1,
                            sub_input_tokens,
                            sub_output_tokens,
                        )
                        .await;
                        immediate_results.push(SubToolTaskResult {
                            index,
                            tool_use_id: id.clone(),
                            name: name.clone(),
                            output: failure,
                        });
                    }
                }
            }
        }

        // 阶段 2：并行执行
        let mut all_results = immediate_results;

        if !spawn_tasks.is_empty() && !SubAgentMonitor::is_cancelled(&app, &run_id).await {
            let handles: Vec<_> = spawn_tasks
                .into_iter()
                .map(|task| {
                    let app_clone = app.clone();
                    let sid_clone = session_id.clone();
                    tokio::spawn(async move {
                        let output = handle_tool_call_inner_owned(
                            app_clone.clone(),
                            task.name.clone(),
                            task.input.clone(),
                            sid_clone,
                            "SUBAGENT".to_string(),
                        )
                        .await;
                        SubToolTaskResult {
                            index: task.index,
                            tool_use_id: task.tool_use_id,
                            name: task.name,
                            output,
                        }
                    })
                })
                .collect();

            let spawned_results = futures_util::future::join_all(handles).await;
            for result in spawned_results {
                if let Ok(r) = result {
                    all_results.push(r);
                }
            }
        }

        // 阶段 3：排序 + 汇总
        all_results.sort_by_key(|r| r.index);

        let mut tool_results = Vec::new();
        for result in all_results {
            if SubAgentMonitor::is_cancelled(&app, &run_id).await {
                SubAgentMonitor::acknowledge_cancelled(&app, &run_id).await;
                return (
                    "子代理已取消。".to_string(),
                    sub_input_tokens,
                    sub_output_tokens,
                );
            }

            let output_summary = {
                if result.output.chars().count() > 180 {
                    format!("{}...", result.output.chars().take(180).collect::<String>())
                } else {
                    result.output.clone()
                }
            };
            SubAgentMonitor::record_tool_result(
                &app,
                &run_id,
                &result.name,
                Some(output_summary),
                loop_count + 1,
                sub_input_tokens,
                sub_output_tokens,
            )
            .await;

            tool_results.push(ContentBlock::ToolResult {
                tool_use_id: result.tool_use_id,
                content: result.output,
            });
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
            "content": format!("\n> ◆ **[子代理执行完毕]**\n"),
            "sessionId": session_id.clone(),
            "isSubAgent": true
        }),
    );
    let _ = app.emit(
        "agent-step",
        json!({
            "type": "subagent_end",
            "sessionId": session_id.clone(),
            "isSubAgent": true
        }),
    );

    if loop_count >= crate::core::constants::MAX_AGENT_LOOP_BEFORE_CONFIRM
        && final_answer.is_empty()
    {
        SubAgentMonitor::fail_run(
            &app,
            &run_id,
            "Subagent reached loop limit".to_string(),
            sub_input_tokens,
            sub_output_tokens,
        )
        .await;
        return (
            format!(
                "子代理执行达到 {} 轮上限，已停止。",
                crate::core::constants::MAX_AGENT_LOOP_BEFORE_CONFIRM
            ),
            sub_input_tokens,
            sub_output_tokens,
        );
    } else {
        SubAgentMonitor::complete_run(
            &app,
            &run_id,
            sub_input_tokens,
            sub_output_tokens,
            Some(final_answer.clone()),
        )
        .await;
        (final_answer, sub_input_tokens, sub_output_tokens)
    }
}

/// 方案审批工具：推送方案到前端预览面板，通过 oneshot channel 阻塞等待用户决策
pub async fn propose_plan(
    app: &tauri::AppHandle,
    input: &serde_json::Value,
    session_id: &str,
) -> String {
    let title = input["title"]
        .as_str()
        .or_else(|| input["plan_title"].as_str())
        .unwrap_or("实施方案");
    let mut content = input["content"]
        .as_str()
        .or_else(|| input["plan_content"].as_str())
        .or_else(|| input["plan_description"].as_str())
        .unwrap_or("")
        .to_string();

    if let Some(tasks) = input["task_breakdown"].as_array() {
        if !tasks.is_empty() {
            content.push_str("\n\n## 任务分解\n\n```json\n");
            content.push_str(&serde_json::to_string_pretty(tasks).unwrap_or_default());
            content.push_str("\n```\n");
        }
    } else if let Some(tasks) = input["task_breakdown"].as_str() {
        if !tasks.trim().is_empty() {
            content.push_str("\n\n## 任务分解\n\n");
            content.push_str(tasks);
            content.push('\n');
        }
    }

    if let Some(estimated_time) = input["estimated_time"].as_str() {
        if !estimated_time.trim().is_empty() {
            content.push_str("\n\n## 预估时间\n\n");
            content.push_str(estimated_time);
            content.push('\n');
        }
    }

    if content.trim().is_empty() {
        return "错误：方案内容不能为空。".to_string();
    }

    // 生成唯一 ID
    use std::sync::atomic::{AtomicUsize, Ordering};
    static PLAN_REQ_ID: AtomicUsize = AtomicUsize::new(1);
    let id = format!("plan_{}", PLAN_REQ_ID.fetch_add(1, Ordering::SeqCst));

    // 创建 oneshot channel 等待用户决策
    let (tx, rx) = tokio::sync::oneshot::channel();
    let session_manager = app.state::<crate::core::state::SessionManager>();
    let ctx = session_manager.get_or_create(session_id).await;
    ctx.pending_permissions.lock().await.insert(id.clone(), tx);

    // 同时将方案文件保存到 .plans 目录以便持久化
    let plans_dir = get_agent_home().join(crate::core::constants::DIR_PLANS);
    if !plans_dir.exists() {
        let _ = std::fs::create_dir_all(&plans_dir);
    }
    let safe_title: String = title
        .chars()
        .take(20)
        .map(|ch| if r#"<>:"/\|?*"#.contains(ch) { '_' } else { ch })
        .collect();
    let plan_filename = format!("{}_{}.md", id, safe_title);
    let plan_path = plans_dir.join(&plan_filename);
    let full_content = format!("# {}\n\n{}", title, content);
    let _ = std::fs::write(&plan_path, &full_content);

    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs();
    let plan_document = PlanDocument {
        id: id.clone(),
        session_id: session_id.to_string(),
        title: title.to_string(),
        content: content.clone(),
        status: "pending".to_string(),
        path: Some(plan_path.to_string_lossy().to_string()),
        created_at: now,
        updated_at: now,
        decided_at: None,
    };
    {
        let mut memory = ctx.memory.lock().await;
        if let Some(existing) = memory
            .plan_documents
            .iter_mut()
            .find(|item| item.id == plan_document.id)
        {
            *existing = plan_document.clone();
        } else {
            memory.plan_documents.push(plan_document.clone());
        }
    }
    let _ = crate::core::session::upsert_plan_document(session_id, plan_document.clone());

    println!("[JARVIS] 方案已推送到前端预览: {} ({})", title, id);

    let _ = app.emit("plan-document-updated", &plan_document);

    // 发送事件到前端，触发方案预览面板
    let _ = app.emit(
        "plan-proposal",
        json!({
            "id": id,
            "title": title,
            "content": content,
            "sessionId": session_id
        }),
    );

    // 在聊天流中也提示一下
    let _ = app.emit(
        "chat-stream",
        json!({
            "content": format!("\n> 📋 **方案已提交审阅**: 「{}」\n> 请在弹出的方案预览面板中查看详情并决策。\n", title),
            "sessionId": session_id
        }),
    );

    // 阻塞等待用户决策（通过 resolve_permission 回调）
    let decision = rx.await.unwrap_or_else(|_| "reject".to_string());

    // 解析决策和可能的修改内容
    let (final_decision, modified_content) = if decision.contains("|||") {
        let parts: Vec<&str> = decision.splitn(2, "|||").collect();
        (parts[0].to_string(), Some(parts[1].to_string()))
    } else {
        (decision, None)
    };

    if final_decision == "reject" {
        println!("[JARVIS] 用户拒绝了方案: {}", title);
        format!("用户已拒绝此方案「{}」。请根据用户意见进行调整，或询问用户想要修改的部分。严禁继续创建 task_create 任务！", title)
    } else {
        println!("[JARVIS] 用户同意了方案: {}", title);
        if let Some(content) = modified_content {
            format!("用户已同意方案「{}」并做了修改！修改后的方案内容：\n\n{}\n\n现在可以使用 task_create 创建持久化任务，并使用 task 工具委派子代理开始执行。", title, content)
        } else {
            format!("用户已同意方案「{}」！现在可以使用 task_create 创建持久化任务，并使用 task 工具委派子代理开始执行。", title)
        }
    }
}

// --- 工具注册 ---
crate::define_tools! {
    pub fn register_tools(registry) {
        ToolDef {
            name: "load_skill",
            description: "按名称加载专业技能知识",
            search_hint: "load skill knowledge domain",
            schema: json!({
                "name": "load_skill",
                "description": "按名称加载专业技能知识。在你需要处理特定领域（如查阅API、审查代码）的不熟悉知识时使用。",
                "input_schema": {
                    "type": "object",
                    "properties": {
                        "name": {"type": "string", "description": "要加载的技能名称"}
                    },
                    "required": ["name"]
                }
            }),
            should_defer: false,
            is_read_only: true,
            is_concurrency_safe: true,
            is_enabled: true,
        },
        ToolDef {
            name: "compact",
            description: "手动触发对话上下文压缩",
            search_hint: "compact context compress summarize",
            schema: json!({
                "name": "compact",
                "description": "手动触发对话上下文压缩。当对话上下文过长觉得需要清理或重置记忆时使用该工具。",
                "input_schema": {
                    "type": "object",
                    "properties": {
                        "focus": { "type": "string", "description": "摘要时需要特别保留的重点方向" }
                    }
                }
            }),
            should_defer: true,
            is_read_only: true,
            is_concurrency_safe: false,
            is_enabled: true,
        },
        ToolDef {
            name: "dream",
            description: "主动触发记忆整理（Dream Agent）",
            search_hint: "dream memory organize consolidate",
            schema: json!({
                "name": "dream",
                "description": "主动触发记忆整理（Dream Agent）。将当前的零散碎片记忆提炼并合并进结构化用户画像中。",
                "input_schema": { "type": "object", "properties": {} }
            }),
            should_defer: true,
            is_read_only: true,
            is_concurrency_safe: false,
            is_enabled: true,
        },
        ToolDef {
            name: "propose_plan",
            description: "提交复杂任务实施方案给用户审阅",
            search_hint: "propose plan review approval",
            schema: json!({
                "name": "propose_plan",
                "description": "【方案审批工具】将实施方案提交给用户审阅。当面对复杂任务（涉及多步骤修改、架构变更等），必须使用此工具提交方案文档，等待用户确认后才能继续执行。方案内容使用 Markdown 格式。前端会以专门的预览面板展示方案，用户可以选择同意或拒绝。",
                "input_schema": {
                    "type": "object",
                    "properties": {
                        "title": {"type": "string", "description": "方案标题"},
                        "content": {"type": "string", "description": "方案正文（Markdown 格式），包含需求理解、变更范围、具体步骤、风险评估等"}
                    },
                    "required": ["title", "content"]
                }
            }),
            should_defer: true,
            is_read_only: false,
            is_concurrency_safe: false,
            is_enabled: true,
        },
        ToolDef {
            name: "task",
            description: "产生具有干净上下文的子代理执行具体操作",
            search_hint: "task subagent delegate spawn worker",
            schema: json!({
                "name": "task",
                "description": "【真正执行】产生一个具有干净上下文环境的子代理 (Subagent) 去实际执行探索或具体操作任务。这是唯一能让子代理实际干活的工具！主 Agent 必须使用此工具来委派文件读取、代码搜索和修改等具体工作，避免污染主对话上下文。与父进程共享文件系统但不共享对话历史。支持与其他工具（包括其他 task）并行执行，可同时委派多个子代理。",
                "input_schema": {
                    "type": "object",
                    "properties": {
                        "prompt": {"type": "string", "description": "要子代理完成的任务说明，越详细越好。包括你想要子代理返回什么数据。"},
                        "read_only": {"type": "boolean", "description": "是否以只读模式运行子代理。默认为 true。如果需要子代理修改文件、写代码或执行高风险命令，【必须】显式设置为 false，否则子代理将没有写入文件的权限！"}
                    },
                    "required": ["prompt"]
                }
            }),
            should_defer: true,
            is_read_only: false,
            is_concurrency_safe: true,
            is_enabled: true,
        },
        ToolDef {
            name: "run_tasks",
            description: "启动任务调度器，根据依赖关系自动并行执行任务",
            search_hint: "run tasks scheduler execute parallel",
            schema: json!({
                "name": "run_tasks",
                "description": "【任务调度器】启动自动任务调度。系统将根据任务依赖关系（blocked_by）自动执行：无依赖的任务并行运行，阻塞任务等待前置完成后自动启动。创建完所有任务和依赖关系后调用此工具一次性调度执行。",
                "input_schema": { "type": "object", "properties": {} }
            }),
            should_defer: true,
            is_read_only: false,
            is_concurrency_safe: false,
            is_enabled: true,
        }
    }
}
