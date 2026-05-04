//! # subagent.rs — 子代理执行引擎
//!
//! 包含完整的 SSE 流式处理和并行工具执行循环。
//! 这是工具系统中最复杂的模块，实现了独立 Agent Loop。
//!
//! ## 关键导出
//! - `run_subagent()`: 子代理执行引擎（独立 Agent Loop，支持只读/读写模式）
//!
//! ## 依赖
//! - Internal: `crate::core::orchestration::subagents`, `crate::core::llm::adapters`
//! - External: `eventsource_stream`, `futures_util`, `serde_json`, `tauri`
//!
//! ## 约束
//! - 子代理与主代理共用同一模型（main_model）
//! - 只读模式会过滤掉 write_file / edit_file / run_shell 等写操作工具
//! - 子代理循环次数受 `MAX_AGENT_LOOP_BEFORE_CONFIRM` 限制

use eventsource_stream::Eventsource;
use serde_json::json;
use tauri::{Emitter, Manager};

use super::super::framework::agent_registry::{normalize_agent_type, AgentRegistry};
use super::super::{handle_tool_call_inner_owned, load_all_skills};
use crate::core::agent::{process_stream, StreamConfig};
use crate::core::config::ConfigState;
use crate::core::infra::prompts::get_subagent_system_prompt;
use crate::core::llm::adapters::parse_streamed_tool_input;
use crate::core::models::{AnthropicRequest, Content, ContentBlock, Message};
use crate::core::orchestration::subagents::{SubAgentMonitor, SubAgentPhase};
use crate::core::session::memory::micro_compact;

/// 子代理执行引擎：独立 Agent Loop，支持只读/读写模式，返回 (结果, 输入token, 输出token)
pub async fn run_subagent(
    app: tauri::AppHandle,
    prompt: String,
    read_only: bool,
    session_id: String,
    task_id: Option<i32>,
    label: Option<String>,
    subagent_type: Option<String>,
    model_override: Option<String>,
) -> (String, u64, u64) {
    let agent_registry = AgentRegistry::global();
    let requested_agent_type = normalize_agent_type(subagent_type.as_deref());
    let agent = agent_registry
        .get(requested_agent_type)
        .unwrap_or_else(|| agent_registry.default_agent());
    let agent_type = agent.agent_type.to_string();
    let max_loops = agent
        .max_turns
        .unwrap_or(crate::core::constants::MAX_AGENT_LOOP_BEFORE_CONFIRM);

    // 注册子代理运行记录
    let run_id = SubAgentMonitor::start_run(
        &app,
        &session_id,
        &prompt,
        read_only,
        task_id,
        label,
        agent_type.clone(),
        max_loops,
    )
    .await;

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
    let model_id = model_override
        .filter(|model| !model.trim().is_empty())
        .or_else(|| agent.model.map(|model| model.to_string()))
        .unwrap_or(cfg.main_model);

    let client = reqwest::Client::new();
    // Only a session workspace is treated as a project/work directory.
    // The app process CWD is JarvisAgent's own runtime location and must not
    // leak into non-sandbox subagent context as the user's project.
    let ws = crate::core::state::effective_workspace(&app, &session_id).await;
    let cwd = ws
        .as_ref()
        .map(|p| p.to_string_lossy().to_string())
        .unwrap_or_else(|| "No session workspace is configured".to_string());
    let ws_str = ws.as_ref().map(|p| p.to_string_lossy().to_string());
    let mut system_prompt = get_subagent_system_prompt(&cwd, ws_str.as_deref());
    system_prompt.push_str(&format!(
        "\n\n[Subagent type]\n- type: {}\n- when to use: {}\n\n[Role instructions]\n{}\n\n[Tool boundary]\nOnly use the tools provided in this run. Do not attempt to call parent-control tools such as RunSubagent, RunSubagentsSequentially, UpdateTodos, CompactConversation, or ConsolidateMemory.",
        agent.agent_type, agent.when_to_use, agent.system_prompt
    ));

    let skills = load_all_skills();
    if !skills.is_empty() {
        system_prompt.push_str("\n\n可用技能 (使用 LoadSkill 工具获取完整内容)：\n");
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

    let tools = agent_registry.resolve_tools(agent, read_only);
    if tools.is_empty() {
        let msg = format!(
            "Subagent '{}' has no available tools after permission filtering.",
            agent.agent_type
        );
        SubAgentMonitor::fail_run(&app, &run_id, msg.clone(), 0, 0).await;
        return (msg, 0, 0);
    }

    let mode_str = if read_only {
        "只读模式"
    } else {
        "读写模式"
    };
    let _ = app.emit(
        "chat-stream",
        json!({
            "content": format!("\n> ◆ **[启动子代理]** ({}, {}) 任务: `{}`\n", agent_type, mode_str, prompt),
            "sessionId": session_id.clone(),
            "isSubAgent": true
        }),
    );
    let _ = app.emit(
        "agent-step",
        json!({
            "type": "subagent_start",
            "task": format!("{} {} - {}", agent_type, mode_str, prompt.chars().take(100).collect::<String>()),
            "sessionId": session_id.clone(),
            "isSubAgent": true
        }),
    );

    while loop_count < max_loops {
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
            let thinking_param = crate::core::llm::registry::query_capabilities(&model_id)
                .and_then(|c| c.thinking_param);

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
        let sub_cancel_token = SubAgentMonitor::cancel_token(&app, &run_id).await;
        let default_cancel = tokio_util::sync::CancellationToken::new();
        let cancel_ref = sub_cancel_token.as_ref().unwrap_or(&default_cancel);

        let stream_result = process_stream(
            &mut stream,
            is_openai,
            &app,
            &session_id,
            &run_id,
            loop_count + 1,
            cancel_ref,
            StreamConfig { is_subagent: true },
        )
        .await;

        // 检查流式接收期间是否被取消
        if SubAgentMonitor::is_cancelled(&app, &run_id).await {
            SubAgentMonitor::acknowledge_cancelled(&app, &run_id).await;
            return (
                "子代理已取消。".to_string(),
                stream_result.input_tokens,
                stream_result.output_tokens,
            );
        }

        // 更新思考阶段
        if !stream_result.thinking.is_empty() {
            SubAgentMonitor::update_phase(
                &app,
                &run_id,
                SubAgentPhase::Thinking,
                loop_count + 1,
                stream_result.input_tokens,
                stream_result.output_tokens,
            )
            .await;
        }

        sub_input_tokens += stream_result.input_tokens;
        sub_output_tokens += stream_result.output_tokens;

        let mut current_blocks = stream_result.blocks;
        let tool_input_buffers = stream_result.tool_input_buffers;
        let current_text_this_turn = stream_result.text;
        let current_thinking_this_turn = stream_result.thinking;

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
                        crate::jarvis_warn!("SUBAGENT", "[SUBAGENT] {}", failure);
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
            // 轻量压缩：清理空内容块，避免上下文无限增长
            micro_compact(&mut messages);
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

    if loop_count >= max_loops && final_answer.is_empty() {
        let stop_message = format!(
            "子代理执行达到 {} 轮上限，已停止。\n\nSubagent stopped because it reached the loop limit. Treat this as a failed delegated attempt. Do not rerun the same dependency install/startup loop, do not launch another identical subagent, and do not continue trying the same commands. Summarize the failure, include the last useful command output already in context, and ask the user how to proceed.",
            max_loops
        );
        SubAgentMonitor::fail_run(
            &app,
            &run_id,
            "Subagent reached loop limit".to_string(),
            sub_input_tokens,
            sub_output_tokens,
        )
        .await;
        return (stop_message, sub_input_tokens, sub_output_tokens);
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
