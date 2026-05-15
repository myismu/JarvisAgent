//! # subagent.rs — 子代理执行引擎
//!
//! 包含完整的 SSE 流式处理和并行工具执行循环。
//! 这是工具系统中最复杂的模块，实现了独立 Agent Loop。
//!
//! ## 关键导出
//! - `run_subagent()`: 子代理执行引擎（独立 Agent Loop，支持只读/读写模式）
//!
//! ## 依赖
//! - Internal: `crate::core::orchestration::subagents`, `crate::infra::llm::adapters`
//! - External: `eventsource_stream`, `futures_util`, `serde_json`, `tauri`
//!
//! ## 约束
//! - 子代理与主代理共用同一模型（main_model）
//! - 只读模式会过滤掉 write_file / edit_file / run_shell 等写操作工具
//! - 子代理循环次数受 `MAX_AGENT_LOOP_BEFORE_CONFIRM` 限制

use eventsource_stream::Eventsource;
use serde_json::json;
use tauri::{Emitter, Manager};

use super::super::framework::agent_registry::{normalize_agent_role, AgentRegistry};
use super::super::{handle_tool_call_inner_owned, load_all_skills};
use crate::core::agent::{process_stream, StreamConfig};
use crate::infra::config::config::ConfigState;
use crate::core::agent::prompts::get_subagent_system_prompt;
use crate::infra::llm::adapters::parse_streamed_tool_input;
use crate::infra::types::models::{AnthropicRequest, Content, ContentBlock, Message};
use crate::core::orchestration::subagents::{SubAgentMonitor, SubAgentPhase};
use crate::core::session::memory::{compact_messages, estimate_tokens};
use crate::infra::state::state::{SessionManager, ToolDedupeCacheEntry};
use crate::core::tools::file_tools::generate_repo_map;
use std::collections::HashMap;

/// 提取工具调用的关键输入摘要（规则提取，不调用 LLM）
fn summarize_tool_input(name: &str, input: &serde_json::Value) -> String {
    match name {
        "ReadFile" | "ReadFileSkeleton" | "WriteFile" | "EditFile" | "ApplyPatch" => {
            let path = input["path"].as_str().unwrap_or("?");
            if let Some(start) = input["start_line"].as_u64() {
                if let Some(end) = input["end_line"].as_u64() {
                    format!("{} (L{}-{})", path, start, end)
                } else {
                    format!("{} (从 L{} 起)", path, start)
                }
            } else {
                format!("{}", path)
            }
        }
        "SearchText" => {
            let pattern = input["pattern"].as_str().unwrap_or("?");
            if let Some(dir) = input["path"].as_str() {
                if !dir.is_empty() {
                    format!("\"{}\" 在 {}", pattern, dir)
                } else {
                    format!("\"{}\"", pattern)
                }
            } else {
                format!("\"{}\"", pattern)
            }
        }
        "FindFiles" => {
            let pattern = input["pattern"].as_str().unwrap_or("?");
            format!("{}", pattern)
        }
        "SearchRepo" | "CodeSearch" => {
            let query = input["query"].as_str().unwrap_or("?");
            format!("\"{}\"", query)
        }
        "FindSymbol" | "FindReferences" => {
            let sym = input["name"].as_str().unwrap_or("?");
            format!("{}", sym)
        }
        "RunCommand" | "RunGitCommand" | "StartBackgroundCommand" => {
            let cmd = input["command"].as_str().unwrap_or("?");
            let truncated: String = cmd.chars().take(80).collect();
            if cmd.len() > 80 {
                format!("{}...", truncated)
            } else {
                truncated
            }
        }
        "ListDirectory" => {
            let path = input["path"].as_str().unwrap_or(".");
            format!("{}", path)
        }
        "LoadSkill" => {
            let skill_name = input["name"].as_str().unwrap_or("?");
            format!("{}", skill_name)
        }
        _ => {
            let raw = input.to_string();
            let truncated: String = raw.chars().take(60).collect();
            if raw.len() > 60 {
                format!("{}...", truncated)
            } else {
                truncated
            }
        }
    }
}

/// 提取工具调用结果的摘要（规则提取，不调用 LLM）
fn summarize_tool_result(name: &str, content: &str) -> String {
    match name {
        "ReadFile" | "ReadFileSkeleton" => {
            // 提取行数 + 首行预览
            if let Some(line) = content.lines().find(|l| l.contains("Total:")) {
                line.to_string()
            } else {
                let line_count = content.lines().count();
                if line_count > 5 {
                    format!("{} 行内容", line_count)
                } else {
                    String::new()
                }
            }
        }
        "SearchText" => {
            if content.contains("No files found") || content.contains("No matches") {
                "无匹配".to_string()
            } else {
                let file_count = content.lines().filter(|l| l.contains(':')).count();
                if file_count > 0 {
                    format!("{} 个文件匹配", file_count)
                } else {
                    String::new()
                }
            }
        }
        "FindFiles" => {
            if content.contains("No files found") {
                "无匹配".to_string()
            } else {
                let count = content.lines().count();
                if count > 0 {
                    format!("{} 个文件", count)
                } else {
                    String::new()
                }
            }
        }
        "RunCommand" | "RunGitCommand" => {
            if content.contains("[exit code: 0") {
                let preview: String = content
                    .lines()
                    .filter(|l| !l.starts_with("[exit code"))
                    .take(3)
                    .collect::<Vec<_>>()
                    .join(" ");
                let truncated: String = preview.chars().take(80).collect();
                if truncated.is_empty() {
                    "成功".to_string()
                } else {
                    truncated
                }
            } else if content.contains("[exit code:") {
                let err_preview: String = content
                    .lines()
                    .filter(|l| !l.starts_with("[exit code"))
                    .take(2)
                    .collect::<Vec<_>>()
                    .join(" ");
                let truncated: String = err_preview.chars().take(80).collect();
                format!("失败: {}", truncated)
            } else {
                String::new()
            }
        }
        "WriteFile" | "EditFile" => {
            if content.contains("成功创建") || content.contains("成功编辑") || content.contains("成功写入")
            {
                "成功".to_string()
            } else if content.contains("失败") || content.contains("编辑失败") {
                let line = content.lines().next().unwrap_or("失败");
                line.chars().take(80).collect()
            } else {
                String::new()
            }
        }
        "ListDirectory" => {
            let items: Vec<&str> = content
                .lines()
                .filter(|l| l.starts_with("[FILE]") || l.starts_with("[DIR]"))
                .take(5)
                .collect();
            if items.is_empty() {
                "空目录".to_string()
            } else {
                let names: Vec<&str> = items
                    .iter()
                    .map(|l| l.trim_start_matches("[FILE] ").trim_start_matches("[DIR] "))
                    .collect();
                format!("{}", names.join(", "))
            }
        }
        _ => String::new(),
    }
}

/// 判断工具是否需要去重（与主 Agent 去重目标一致）
fn is_dedup_target(name: &str) -> bool {
    matches!(
        name,
        "LoadSkill"
            | "CompactConversation"
            | "ConsolidateMemory"
            | "ProposePlan"
            | "RunSubagent"
    )
}

/// 提取主 Agent 上下文供子 Agent 继承（规则提取，不调用 LLM）
async fn extract_subagent_context(
    app: &tauri::AppHandle,
    session_id: &str,
    ws: &Option<std::path::PathBuf>,
) -> String {
    let mut ctx = String::new();

    // 1. Repo Map（项目文件树，深度 3）
    let repo_dir = ws
        .clone()
        .unwrap_or_else(|| std::env::current_dir().unwrap_or_default());
    let repo_map = generate_repo_map(&repo_dir, "", 0, 3);
    if !repo_map.trim().is_empty() {
        ctx.push_str("【项目结构（主Agent已探索）】\n```\n");
        ctx.push_str(&repo_map);
        ctx.push_str("```\n\n");
    }

    // 2. 主 Agent 工具调用 + 结果摘要（配对展示，含结果信息）
    if let Some(manager) = app.try_state::<SessionManager>() {
        let session_ctx = manager.get_or_create(session_id).await;
        let memory = session_ctx.memory.lock().await;

        let msgs = &memory.messages;
        let mut tool_entries: Vec<String> = Vec::new();

        // 遍历消息，将 ToolUse 与紧随的 ToolResult 配对
        for window in msgs.windows(2) {
            if let (Message::Assistant {
                content: Content::Multiple(assistant_blocks),
            }, Message::User {
                content: Content::Multiple(user_blocks),
            }) = (&window[0], &window[1])
            {
                for block in assistant_blocks {
                    if let ContentBlock::ToolUse {
                        name, input, id, ..
                    } = block
                    {
                        if matches!(
                            name.as_str(),
                            "SearchTools"
                                | "RunSubagent"
                                | "RunSubagentsSequentially"
                                | "CompactConversation"
                                | "ConsolidateMemory"
                                | "UpdateTodos"
                        ) {
                            continue;
                        }
                        let call_info = summarize_tool_input(name, input);
                        // 查找对应的 ToolResult
                        let result_info = user_blocks
                            .iter()
                            .find_map(|b| {
                                if let ContentBlock::ToolResult {
                                    tool_use_id, content, ..
                                } = b
                                {
                                    if tool_use_id == id {
                                        Some(summarize_tool_result(name, content))
                                    } else {
                                        None
                                    }
                                } else {
                                    None
                                }
                            })
                            .unwrap_or_default();

                        let entry = if result_info.is_empty() {
                            format!("- {}: {}", name, call_info)
                        } else {
                            format!("- {}: {} → {}", name, call_info, result_info)
                        };
                        tool_entries.push(entry);
                        if tool_entries.len() >= 10 {
                            break;
                        }
                    }
                }
            }
            if tool_entries.len() >= 10 {
                break;
            }
        }

        if !tool_entries.is_empty() {
            ctx.push_str("【主Agent已执行的关键操作（无需重复）】\n");
            for entry in &tool_entries {
                ctx.push_str(entry);
                ctx.push('\n');
            }
            ctx.push('\n');
        }

        // 3. 主 Agent 最近的结论（最后一条非工具 Assistant 消息的 text 部分）
        for msg in memory.messages.iter().rev() {
            if let Message::Assistant {
                content: Content::Multiple(blocks),
            } = msg
            {
                let has_tool = blocks
                    .iter()
                    .any(|b| matches!(b, ContentBlock::ToolUse { .. }));
                if !has_tool {
                    let text: String = blocks
                        .iter()
                        .filter_map(|b| {
                            if let ContentBlock::Text { text } = b {
                                Some(text.as_str())
                            } else {
                                None
                            }
                        })
                        .collect::<Vec<_>>()
                        .join(" ");
                    if !text.trim().is_empty() {
                        let truncated: String = text.chars().take(300).collect();
                        ctx.push_str(&format!(
                            "【主Agent的分析结论】\n{}\n\n",
                            truncated
                        ));
                    }
                    break;
                }
            }
        }
    }

    // 4. 当前会话的后台任务状态（避免子Agent重复启动已运行的服务）
    if let Some(bg_state) = app.try_state::<crate::infra::background::BackgroundState>() {
        let bg = bg_state.0.lock().await;
        let session_tasks: Vec<_> = bg
            .tasks
            .iter()
            .filter(|(_, t)| t.session_id.as_deref() == Some(session_id))
            .collect();
        if !session_tasks.is_empty() {
            ctx.push_str("【会话中已在运行的后台任务（绝对不要重复启动！）】\n");
            for (_, task) in &session_tasks {
                let port_info = task.port.map(|p| format!(" :{}", p)).unwrap_or_default();
                ctx.push_str(&format!(
                    "- [{}] {} {}{}\n",
                    task.status,
                    task.command,
                    task.task_type.as_deref().unwrap_or("unknown"),
                    port_info
                ));
            }
            ctx.push_str("如果上述任务已覆盖你要执行的命令，跳过它，不要重复启动。\n\n");
        }
    }

    ctx
}

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
    let requested_agent_role = normalize_agent_role(subagent_type.as_deref());
    let agent = agent_registry
        .get(requested_agent_role)
        .unwrap_or_else(|| agent_registry.default_agent());
    let agent_role = agent.agent_role.to_string();
    let max_loops = agent
        .max_turns
        .unwrap_or(crate::infra::types::constants::MAX_AGENT_LOOP_BEFORE_CONFIRM);

    // 注册子代理运行记录
    let run_id = SubAgentMonitor::start_run(
        &app,
        &session_id,
        &prompt,
        read_only,
        task_id,
        label,
        agent_role.clone(),
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
    let ws = crate::infra::state::state::effective_workspace(&app, &session_id).await;
    let cwd = ws
        .as_ref()
        .map(|p| p.to_string_lossy().to_string())
        .unwrap_or_else(|| "No session workspace is configured".to_string());
    let ws_str = ws.as_ref().map(|p| p.to_string_lossy().to_string());
    let mut system_prompt = get_subagent_system_prompt(&cwd, ws_str.as_deref());
    system_prompt.push_str(&format!(
        "\n\n[Subagent type]\n- type: {}\n- when to use: {}\n\n[Role instructions]\n{}\n\n[Tool boundary]\nOnly use the tools provided in this run. Do not attempt to call parent-control tools such as RunSubagent, RunSubagentsSequentially, UpdateTodos, CompactConversation, or ConsolidateMemory.",
        agent.agent_role, agent.when_to_use, agent.system_prompt
    ));

    let skills = load_all_skills();
    if !skills.is_empty() {
        system_prompt.push_str("\n\n可用技能 (使用 LoadSkill 工具获取完整内容)：\n");
        for skill in &skills {
            system_prompt.push_str(&format!("  - {}: {}\n", skill.name, skill.description));
        }
    }

    // 提取主 Agent 上下文注入到子 Agent 初始消息中
    let subagent_context = extract_subagent_context(&app, &session_id, &ws).await;
    let augmented_prompt = if subagent_context.is_empty() {
        prompt.clone()
    } else {
        format!("{}\n【委派任务】\n{}", subagent_context, prompt)
    };

    let mut messages = vec![Message::User {
        content: Content::Single(augmented_prompt),
    }];

    let mut loop_count = 0;
    let mut final_answer = String::new();
    let mut sub_input_tokens: u64 = 0;
    let mut sub_output_tokens: u64 = 0;
    // 子 Agent 工具去重（与主 Agent 机制一致，scope 为子 Agent run_id）
    let mut agent_dedup_state: HashMap<String, ToolDedupeCacheEntry> = HashMap::new();

    let tools = agent_registry.resolve_tools(agent, read_only);
    if tools.is_empty() {
        let msg = format!(
            "Subagent '{}' has no available tools after permission filtering.",
            agent.agent_role
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
            "content": format!("\n> ◆ **[启动子代理]** ({}, {}) 任务: `{}`\n", agent_role, mode_str, prompt),
            "sessionId": session_id.clone(),
            "isSubAgent": true
        }),
    );
    let _ = app.emit(
        "agent-step",
        json!({
            "type": "subagent_start",
            "task": format!("{} {} - {}", agent_role, mode_str, prompt.chars().take(100).collect::<String>()),
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
            max_tokens: crate::infra::types::constants::MAX_TOKENS_CONTEXT,
            system: system_prompt.clone(),
            messages: messages.clone(),
            tools: tools.clone(),
            stream: true,
            thinking: None,
            temperature: cfg.temperature,
            top_p: cfg.top_p,
            top_k: cfg.top_k,
        };

        let should_think = cfg.enable_thinking.unwrap_or(false);
        request_body.thinking = Some(crate::infra::types::models::ThinkingConfig {
            r#type: Some(if should_think { "enabled" } else { "disabled" }.to_string()),
            budget_tokens: if should_think { Some(1024) } else { None },
            enable: None,
        });
        if should_think && request_body.max_tokens <= 1024 {
            request_body.max_tokens = 4096;
        }

        let (req_json, is_openai) = if api_format_enum.is_openai() {
            use crate::infra::llm::adapters::{
                should_backfill_deepseek_reasoning_content,
                translate_messages_to_openai_with_reasoning_backfill, translate_tools_to_openai,
            };
            use crate::infra::types::models::OpenAIRequest;
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
                max_tokens: Some(crate::infra::types::constants::MAX_TOKENS_CONTEXT),
                messages: openai_msgs,
                tools: if openai_tools.is_empty() {
                    None
                } else {
                    Some(openai_tools)
                },
                stream: true,
                stream_options: Some(crate::infra::types::models::StreamOptions {
                    include_usage: true,
                }),
                reasoning_effort: None,
                thinking: None,
                thinking_budget: None,
                enable_thinking: None,
                extra_body: None,
                parameters: None,
                temperature: request_body.temperature,
                top_p: request_body.top_p,
            };

            let should_think = cfg.enable_thinking.unwrap_or(false);
            crate::infra::llm::registry::apply_thinking_for_model(
                &mut openai_req, &model_id, should_think,
            );
            (serde_json::to_value(openai_req).unwrap(), true)
        } else {
            (serde_json::to_value(request_body).unwrap(), false)
        };

        let request_json_str = serde_json::to_string_pretty(&req_json).unwrap_or_default();
        let logger = crate::infra::debug_logger::DebugLogger::new();
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

        crate::infra::llm::api_client::log_model_request(&model_id, &base_url, "子agent");

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

                        // 去重检查：与主 Agent 机制一致，阻止重复调用 Agent 控制工具
                        if is_dedup_target(name) {
                            let dedup_key = name.to_lowercase();
                            if let Some(entry) = agent_dedup_state.get_mut(&dedup_key) {
                                entry.suppressed_count += 1;
                                let blocked = format!(
                                    "重复调用被阻止: 工具 {} 已在本子Agent运行中被调用。请使用已有的结果继续，不要重复调用。 (第{}次抑制)",
                                    name, entry.suppressed_count
                                );
                                SubAgentMonitor::record_tool_result(
                                    &app, &run_id, name,
                                    Some(blocked.chars().take(180).collect::<String>()),
                                    loop_count + 1, sub_input_tokens, sub_output_tokens,
                                ).await;
                                immediate_results.push(SubToolTaskResult {
                                    index,
                                    tool_use_id: id.clone(),
                                    name: name.clone(),
                                    output: blocked,
                                });
                                continue;
                            }
                            agent_dedup_state.insert(
                                dedup_key,
                                ToolDedupeCacheEntry {
                                    display: name.to_string(),
                                    suppressed_count: 0,
                                    running: false,
                                },
                            );
                        }
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
            // Token > 70% 上限时触发 LLM 摘要压缩
            let estimated = estimate_tokens(&messages);
            if estimated > crate::infra::types::constants::MAX_TOKENS_COMPACT_TRIGGER * 70 / 100 {
                println!(
                    "[SUBAGENT] 上下文估算值 > {} ({})，触发自动压缩",
                    crate::infra::types::constants::MAX_TOKENS_COMPACT_TRIGGER,
                    estimated
                );
                let _ = compact_messages(
                    &mut messages,
                    &client,
                    &api_key,
                    &base_url,
                    &cfg.utility_model,
                    api_format_enum,
                )
                .await;
            }
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

    if loop_count >= max_loops {
        // 收集最后的工具输出作为部分成果，而非直接标记失败
        let mut partial_results: Vec<String> = messages
            .iter()
            .rev()
            .filter_map(|msg| {
                if let Message::User {
                    content: Content::Multiple(blocks),
                } = msg
                {
                    let summaries: Vec<&str> = blocks
                        .iter()
                        .filter_map(|block| {
                            if let ContentBlock::ToolResult { content, .. } = block {
                                Some(content.as_str())
                            } else {
                                None
                            }
                        })
                        .collect();
                    if summaries.is_empty() {
                        None
                    } else {
                        Some(summaries.join("\n"))
                    }
                } else {
                    None
                }
            })
            .take(5)
            .collect();
        partial_results.reverse();

        let partial_summary = if !partial_results.is_empty() {
            let joined: String = partial_results
                .iter()
                .map(|r| {
                    let truncated: String = r.chars().take(200).collect();
                    if r.len() > 200 {
                        format!("{}...", truncated)
                    } else {
                        truncated
                    }
                })
                .collect::<Vec<_>>()
                .join("\n---\n");
            format!(
                "\n\n【部分成果（{}轮已达上限）】\n{}",
                max_loops, joined
            )
        } else {
            String::new()
        };

        let stop_message = format!(
            "子代理执行达到 {} 轮上限，已停止。{}",
            max_loops, partial_summary
        );

        if final_answer.is_empty() && partial_results.is_empty() {
            SubAgentMonitor::fail_run(
                &app,
                &run_id,
                "Subagent reached loop limit with no results".to_string(),
                sub_input_tokens,
                sub_output_tokens,
            )
            .await;
        } else {
            SubAgentMonitor::complete_run(
                &app,
                &run_id,
                sub_input_tokens,
                sub_output_tokens,
                Some(stop_message.clone()),
            )
            .await;
        }
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
