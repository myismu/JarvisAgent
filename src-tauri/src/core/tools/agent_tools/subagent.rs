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
//! - 子代理使用 "SUBAGENT" 意图和 "edit" 工作模式

use eventsource_stream::Eventsource;
use serde_json::json;
use tauri::{Emitter, Manager};

use super::super::framework::agent_registry::{normalize_agent_role, AgentRegistry};
use super::super::handle_tool_call_inner_owned;
use crate::core::agent::prompts::get_subagent_system_prompt;
use crate::core::agent::{process_stream, StreamConfig};
use crate::core::orchestration::subagents::{SubAgentMonitor, SubAgentPhase};
use crate::core::session::memory::{compact_messages, estimate_tokens};
use crate::core::tools::file_tools::generate_repo_map;
use crate::infra::config::config::ConfigState;
use crate::infra::llm::adapters::parse_streamed_tool_input;
use crate::infra::state::state::{SessionManager, ToolDedupeCacheEntry};
use crate::infra::types::models::{AnthropicRequest, Content, ContentBlock, Message};
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
        "RunCommand" | "StartBackgroundCommand" => {
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
        "RunCommand" => {
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
            if content.contains("成功创建")
                || content.contains("成功编辑")
                || content.contains("成功写入")
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
        "LoadSkill" | "CompactConversation" | "ConsolidateMemory" | "ProposePlan" | "RunSubagent"
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
            if let (
                Message::Assistant {
                    content: Content::Multiple(assistant_blocks),
                },
                Message::User {
                    content: Content::Multiple(user_blocks),
                },
            ) = (&window[0], &window[1])
            {
                for block in assistant_blocks {
                    if let ContentBlock::ToolUse {
                        name, input, id, ..
                    } = block
                    {
                        if matches!(
                            name.as_str(),
                            // ExecuteTool 一并跳过：RunSubagent 已是按需工具，父代理的委派
                            // 现在以 ExecuteTool(name="RunSubagent") 的形式出现；不跳过就会把
                            // "派子代理"当成一条普通操作摘要喂进子代理上下文（2026-09-21）
                            "ExecuteTool"
                                | "DiscoverTools"
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
                                    tool_use_id,
                                    content,
                                    ..
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
                        ctx.push_str(&format!("【主Agent的分析结论】\n{}\n\n", truncated));
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

/// 连续多少轮"本轮所有工具调用都失败"就判定结构性受阻、提前收口。
///
/// 取 3 不取 1：单轮失败很常见（参数写错、路径不存在），模型通常下一轮能自行修正；
/// 连续 3 轮全败才说明这条路根本走不通。
///
/// 背景（2026-09-21 实测）：子代理被派去删 `.jarvis_trash` —— 在当时的工具集下这是
/// **结构性无解**（回收站建在目标自己内部，rename 必然失败）。它于是逐轮换策略重试，
/// 把轮数上限耗光、空转好几分钟且没有任何进度；主 Agent 收到失败结果后又派了一个
/// **新子代理**从零再走一遍。这条护栏让"走不通"在 3 轮内收敛，并明确要求不要重复委派。
const MAX_CONSECUTIVE_FAILED_LOOPS: u32 = 3;

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
    skills: Option<Vec<String>>,
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

    // 与主 Agent 共用统一的客户端构造（连接超时 + TCP keepalive）
    let client = crate::infra::llm::api_client::build_streaming_client();
    // Only a session workspace is treated as a project/work directory.
    // The app process CWD is JarvisAgent's own runtime location and must not
    // leak into non-sandbox subagent context as the user's project.
    let ws = crate::infra::state::state::effective_workspace(&app, &session_id).await;
    let cwd = ws
        .as_ref()
        .map(|p| p.to_string_lossy().to_string())
        .unwrap_or_else(|| "No session workspace is configured".to_string());
    let ws_str = ws.as_ref().map(|p| p.to_string_lossy().to_string());

    // 用户开关（会话快照）。子代理**不走** `is_available` 那条路，所以在
    // resolve_tools 与下面的技能提示里各把一道闸（见 resolve_tools 的注释）。
    // 在这之前取好：system_prompt 的拼接要用到它。
    let filter = crate::core::tools::tool_filter_for(&app, &session_id).await;

    let mut system_prompt = get_subagent_system_prompt(&cwd, ws_str.as_deref());
    system_prompt.push_str(&format!(
        "\n\n[Subagent type]\n- type: {}\n- when to use: {}\n\n[Role instructions]\n{}\n\n[Tool boundary]\nOnly use the tools provided in this run. Do not attempt to call parent-control tools such as RunSubagent, RunSubagentsSequentially, UpdateTodos, CompactConversation, or ConsolidateMemory.",
        agent.agent_role, agent.when_to_use, agent.system_prompt
    ));

    // 注入主 Agent 指定的技能
    if let Some(ref skill_names) = skills {
        if !skill_names.is_empty() {
            let all_skills = super::super::load_all_skills();
            // 只注入**已激活**的技能（2026-09-21 补）：
            // 主 Agent 可能传一个被用户在设置里关掉的技能名。那种技能连正文都加载不了
            // （LoadSkill 会直接拒），却仍会出现在子代理的 system prompt 里 ——
            // 属"看得见调不动"，正是本项目在别处反复批判的两套真相。
            let activations = crate::command::app_config::get_all_skill_activations();
            let matched: Vec<&crate::infra::types::models::Skill> = all_skills
                .iter()
                .filter(|s| skill_names.iter().any(|name| name == &s.name))
                .filter(|s| activations.get(&s.name).copied().unwrap_or(true))
                .collect();
            // 提示里点名 LoadSkill，所以要确认它真在这个子代理的工具集里 ——
            // 用户可能把它关掉了，那时 resolve_tools 已经剔除它，这句"Use LoadSkill"
            // 就成了调用必失败的死指引。
            if !matched.is_empty() && filter.is_enabled("LoadSkill") {
                system_prompt
                    .push_str("\n\n[Available skills]\nUse LoadSkill tool to load full content.\n");
                for skill in &matched {
                    system_prompt.push_str(&format!("  - {}: {}\n", skill.name, skill.description));
                }
            }
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
    // 空转保护计数：连续多少轮"本轮所有工具调用都失败"（见 `MAX_CONSECUTIVE_FAILED_LOOPS`）
    let mut consecutive_failed_loops: u32 = 0;
    let mut final_answer = String::new();
    let mut sub_input_tokens: u64 = 0;
    let mut sub_output_tokens: u64 = 0;
    // 子 Agent 工具去重（与主 Agent 机制一致，scope 为子 Agent run_id）
    let mut agent_dedup_state: HashMap<String, ToolDedupeCacheEntry> = HashMap::new();

    let tools = agent_registry.resolve_tools(agent, read_only, &filter);
    if tools.is_empty() {
        let msg = format!(
            "Subagent '{}' has no available tools after permission filtering.",
            agent.agent_role
        );
        SubAgentMonitor::fail_run(&app, &run_id, msg.clone(), 0, 0).await;
        return (msg, 0, 0);
    }

    // 压缩判据（`infra::llm::context_budget`）要用「本地估算 vs 厂商实测」的比值标定，
    // 而 system prompt 与工具 schema 不进 messages、却真实占用 prompt token。不把它们
    // 补进估算，比值会被系统性抬高，阈值就判不准。
    let fixed_overhead_tokens = {
        let tools_json = serde_json::to_string(&tools).unwrap_or_default();
        crate::infra::llm::token_count::count_text("gpt-4", &system_prompt).tokens
            + crate::infra::llm::token_count::count_text("gpt-4", &tools_json).tokens
    };

    let mode_str = if read_only {
        "只读模式"
    } else {
        "读写模式"
    };
    let _ = app.emit(
        "chat-stream",
        json!({
            "content": format!("\n> **[启动子代理]** ({}, {}) 任务: `{}`\n", agent_role, mode_str, prompt),
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

    // 子代理深度思考：**继承主 Agent 本轮档位**（设计文档 K3）。
    //
    // 改造前这里按全局 `agent_audience` 重新推导，结果可能与主 Agent 相反
    // （例如主 Agent 被用户/模型夹紧为关闭，子代理却因 audience=developer 而开启）。
    // 现在统一取 `SessionContext.turn_think`——主 Agent 每轮开始时写入的裁决结果。
    // 若该会话尚未跑过主 Agent（直连工具调用等边缘路径），回落到 audience 默认值，
    // 与改造前行为一致，不引入新的不确定性。
    let should_think = {
        let ctx = app
            .state::<crate::infra::state::state::SessionManager>()
            .get_or_create(&session_id)
            .await;
        let inherited = *ctx.turn_think.lock().await;
        match inherited {
            Some(value) => value,
            // 兜底：主 Agent 本轮尚未裁决（`turn_think` 为空）时，
            // 按**本会话的档位**推导——而不是回落受众默认。
            // 会话档位已是确定布尔，只需再按本模型能力夹紧一次，口径与
            // `pipeline` 的裁决层一致（DeepSeek 类模型恒为开）。
            None => {
                let session_mode =
                    crate::core::session::thinking::ThinkingMode(*ctx.thinking_mode.lock().await);
                let caps = crate::infra::llm::registry::query_capabilities(&model_id);
                crate::core::session::thinking::decide(None, session_mode, caps.as_ref()).enabled
            }
        }
    };

    while loop_count < max_loops {
        // 空转保护：连续多轮工具调用全部失败 → 结构性受阻，提前收口。
        // 汇报里必须写清"为什么停"，并要求不要重复委派同一任务 —— 否则主 Agent 会再派一个
        // 新子代理从头试一遍（这正是那次空转被放大成好几分钟的原因）。
        if consecutive_failed_loops >= MAX_CONSECUTIVE_FAILED_LOOPS {
            let msg = format!(
                "子代理连续 {} 轮工具调用全部失败，判定为结构性受阻，已提前停止（未耗满 {} 轮上限）。\
                 失败原文见上方各轮的工具结果。请换方案，或如实告知用户受阻点；**不要重复委派同一任务**。",
                consecutive_failed_loops, max_loops
            );
            SubAgentMonitor::complete_run(
                &app,
                &run_id,
                sub_input_tokens,
                sub_output_tokens,
                Some(msg.clone()),
            )
            .await;
            return (msg, sub_input_tokens, sub_output_tokens);
        }

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

        // 本轮的本地估算（含 system/tools 固定开销）：与 stream_result.input_tokens 是
        // 同一份请求的一对，本轮结束后作为压缩判据的标定基准（实测 ÷ 估算）。
        let est_before_request = estimate_tokens(&messages) + fixed_overhead_tokens;

        // 与主 Agent 同源：用户覆盖 > 注册表 > 常量兜底（压缩判据的输出预算也用这个函数）
        let max_tokens =
            crate::infra::llm::context_budget::resolve_output_budget(&model_id, cfg.max_tokens)
                as i32;

        // 采样参数与思考参数都与主 Agent 同源（注册表驱动），不再各自硬编码。
        let sampling_ok = crate::infra::llm::registry::supports_sampling_params(&model_id);
        let mut request_body = AnthropicRequest {
            model: model_id.clone(),
            max_tokens,
            system: system_prompt.clone(),
            messages: messages.clone(),
            tools: tools.clone(),
            stream: true,
            thinking: None,
            temperature: if sampling_ok { cfg.temperature } else { None },
            top_p: if sampling_ok { cfg.top_p } else { None },
            top_k: if sampling_ok { cfg.top_k } else { None },
            output_config: None,
        };

        let thinking_plan =
            crate::infra::llm::registry::plan_anthropic_thinking(&model_id, should_think, None);
        let thinking_active = thinking_plan.thinking_active();
        request_body.thinking = thinking_plan.thinking;
        request_body.output_config = thinking_plan.output_config;
        if thinking_active && request_body.max_tokens <= 1024 {
            request_body.max_tokens = 4096;
        }

        let req_json = if api_format_enum.is_openai() {
            use crate::infra::llm::adapters::{
                should_backfill_deepseek_reasoning_content,
                translate_messages_to_openai_with_reasoning_backfill, translate_tools_to_openai,
            };
            use crate::infra::types::models::OpenAIRequest;
            let backfill_reasoning_content =
                should_backfill_deepseek_reasoning_content(&model_id, &base_url, should_think);
            let openai_msgs = translate_messages_to_openai_with_reasoning_backfill(
                &request_body.system,
                &request_body.messages,
                backfill_reasoning_content,
            );
            let openai_tools = translate_tools_to_openai(&request_body.tools);
            let mut openai_req = OpenAIRequest {
                model: model_id.clone(),
                max_tokens: Some(max_tokens),
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

            crate::infra::llm::registry::apply_thinking_for_model(
                &mut openai_req,
                &model_id,
                should_think,
            );
            serde_json::to_value(openai_req).unwrap()
        } else {
            serde_json::to_value(request_body).unwrap()
        };

        println!(
            "[SUB AGENT] loop {} request ({} bytes)",
            loop_count + 1,
            serde_json::to_string(&req_json)
                .map(|s| s.len())
                .unwrap_or(0)
        );
        crate::infra::debug_logger::debug_logger().log_request(
            &session_id,
            "SUB",
            loop_count + 1,
            &req_json,
        );

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
            api_format_enum,
            &app,
            &session_id,
            &run_id,
            loop_count + 1,
            cancel_ref,
            StreamConfig {
                is_subagent: true,
                cache_usage_style: crate::infra::llm::registry::cache_usage_style_for(&model_id),
                // 子 Agent 有自己的监控面板与阶段提示，不需要主聊天流的等待提示
                on_frame: None,
                model_id: Some(model_id.clone()),
                // 子 Agent 的轮次不进 `agent_run_events`（那是主 Agent 崩溃重建的数据源），
                // 故这里恒关——开了只会往一张不参与重建的表里写无关行。
                crash_protection: false,
            },
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

        // 本轮请求的厂商实测输入量（配 est_before_request 作压缩判据的标定基准）
        let request_input_tokens = stream_result.input_tokens;
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
        crate::infra::debug_logger::debug_logger().log_thoughts(
            &session_id,
            "SUB",
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
            /// 本次调用是否失败。用于空转保护：连续多轮"本轮全部失败"就判定结构性受阻（见
            /// `MAX_CONSECUTIVE_FAILED_LOOPS`）。
            is_error: bool,
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
                                    "content": format!("\n>   - 子代理自动修复了工具 `{}` 的流式参数格式\n", name),
                                    "sessionId": session_id.clone(),
                                    "isSubAgent": true
                                }),
                            );
                        }

                        // 带上"这个工具在操作什么"的规则化摘要（复用子代理上下文用的同一个函数），
                        // 让用户在聊天区就能看出子代理在干什么，而不是只看到一个工具名。
                        // 摘要里若含换行会破坏 `> ` 引用块的逐行前缀，这里统一压成单行。
                        let call_detail =
                            summarize_tool_input(name, input).replace(['\n', '\r'], " ");
                        let _ = app.emit(
                            "chat-stream",
                            json!({
                                "content": format!(
                                    "\n>   - 子代理使用工具: `{}` {}\n",
                                    name, call_detail
                                ),
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
                                    &app,
                                    &run_id,
                                    name,
                                    Some(blocked.chars().take(180).collect::<String>()),
                                    loop_count + 1,
                                    sub_input_tokens,
                                    sub_output_tokens,
                                )
                                .await;
                                immediate_results.push(SubToolTaskResult {
                                    index,
                                    tool_use_id: id.clone(),
                                    name: name.clone(),
                                    output: blocked,
                                    // 被去重拦下 = 这次调用没产生任何进展，按失败计入空转统计
                                    is_error: true,
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
                        // 与主 Agent 同口径：追加分类化处置建议（见 tools_runner.rs）
                        let failure = format!(
                            "子代理工具 `{}` 参数解析失败：{}\n原始参数片段：{}\n\n{}",
                            name,
                            err,
                            truncated,
                            err.advice()
                        );
                        crate::jarvis_warn!("SUBAGENT", "[SUBAGENT] {}", failure);
                        let _ = app.emit(
                            "chat-stream",
                            json!({
                                "content": format!("\n>   - 子代理工具 `{}` 参数解析失败\n>   错误: `{}`\n", name, err),
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
                            is_error: true,
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
                    // 把子代理身份带进日志与审计（agent_type = "subagent:<role>"），
                    // 供 log-viewer / 审计区分"哪种类型的子代理在干活"（2026-09-20）
                    let agent_tag = format!("subagent:{}", agent_role);
                    tokio::spawn(async move {
                        let (output, is_error) = handle_tool_call_inner_owned(
                            app_clone.clone(),
                            task.name.clone(),
                            task.input.clone(),
                            sid_clone,
                            "SUBAGENT".to_string(), // intent：子代理执行的意图标记
                            "edit".to_string(),     // 子agent使用 edit 模式
                            agent_tag,              // agent_type：带子代理类型（2026-09-20）
                        )
                        .await;
                        SubToolTaskResult {
                            index: task.index,
                            tool_use_id: task.tool_use_id,
                            name: task.name,
                            output,
                            is_error,
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

        // 空转保护计数：本轮"所有工具调用都失败"就 +1，只要有一个成功立刻清零。
        // （`immediate_results` 里的去重拦截与参数解析失败也按失败计入。）
        if !all_results.is_empty() && all_results.iter().all(|r| r.is_error) {
            consecutive_failed_loops += 1;
        } else {
            consecutive_failed_loops = 0;
        }

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
            // 超阈值时触发 LLM 摘要压缩。判据与主 Agent 共用
            // （`infra::llm::context_budget`）：阈值 =（模型窗口 − 输出预算）×
            // `COMPACT_TRIGGER_PERCENT`（当前 85%），
            // 占用 = 本地估算 ×（本轮实测 ÷ 本轮估算）。
            let window = crate::infra::llm::context_budget::resolve_context_window(&model_id);
            let output_budget =
                crate::infra::llm::context_budget::resolve_output_budget(&model_id, cfg.max_tokens);
            let trigger =
                crate::infra::llm::context_budget::compact_trigger_tokens(window, output_budget);
            let est_now = estimate_tokens(&messages) + fixed_overhead_tokens;
            let context_tokens = crate::infra::llm::context_budget::calibrated_context_tokens(
                Some(request_input_tokens),
                Some(est_before_request),
                est_now,
            );
            if crate::infra::llm::context_budget::should_compact(context_tokens, trigger) {
                println!(
                    "[SUBAGENT] 上下文占用 {} > {}% 可用窗口（窗口 {} − 输出预算 {} = {}, 本地估算 {}, 本轮实测 {}），触发自动压缩",
                    context_tokens,
                    crate::infra::llm::context_budget::COMPACT_TRIGGER_PERCENT,
                    window,
                    output_budget,
                    window.saturating_sub(output_budget),
                    est_now,
                    request_input_tokens
                );
                let mut temp_memory = crate::infra::types::models::SessionMemory {
                    messages: std::mem::take(&mut messages),
                    ..Default::default()
                };
                // 子 Agent 的压缩是纯函数式调用：临时 memory 里全是普通对话消息
                temp_memory.sources = temp_memory
                    .messages
                    .iter()
                    .map(|_| crate::infra::types::models::MessageSource::Chat)
                    .collect();
                let _ = compact_messages(
                    &mut temp_memory,
                    &client,
                    &api_key,
                    &base_url,
                    &cfg.utility_model,
                    api_format_enum,
                )
                .await;
                messages = temp_memory.messages;
            }
        }
        loop_count += 1;
    }

    let _ = app.emit(
        "chat-stream",
        json!({
            "content": format!("\n> **[子代理执行完毕]**\n"),
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
            format!("\n\n【部分成果（{}轮已达上限）】\n{}", max_loops, joined)
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
