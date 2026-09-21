//! # mod.rs — 工具系统入口模块
//!
//! 工具系统的中央枢纽：模块注册、技能加载、工具定义组装、路由分发。
//! 工具参数（tools）始终不变以保证 prompt cache 命中，意图+工作模式仅影响上下文注入和 ExecuteTool 运行时校验。
//!
//! ## 关键导出
//! - `get_tools_definition()`: 返回固定核心工具定义列表（不含意图参数，保证缓存命中）
//! - `handle_tool_call()` / `handle_tool_call_owned()`: 工具调用路由入口
//! - `load_all_skills()`: 从 skills 目录加载所有 SKILL.md 技能文件
//! - `should_block_write_tool()`: 判断是否应该阻止写操作工具（兜底防护）
//! - `is_write_tool()`: 判断是否是写操作工具
//!
//! ## 依赖
//! - Internal: 各工具子模块（file_tools, shell_tools, task_tools, agent_tools, system_tools, tool_search）
//! - External: `serde_json`, `tauri`
//!
//! ## 约束
//! - tools 参数始终不变（多厂商前缀缓存的命门），意图过滤仅通过上下文注入 + ExecuteTool 运行时校验
//! - **执行器分两支**（2026-09-21）：`dispatch_tool_call`（含 `RunSubagent`，只挂主 Agent 路径）
//!   与 `dispatch_core_tool`（子代理也走这条路，不含 `RunSubagent`）。合并回一个函数会触发
//!   opaque 类型自递归、直接编译失败——详见 `dispatch_tool_call` 的文档注释，别合并。
//!   附带效果：子代理不能再派子代理，从运行时过滤升级成了类型结构上的保证。
//! - 子代理（SUBAGENT）不能调用 RunSubagent / ConsolidateMemory / CompactConversation / RunSubagentsSequentially
//! - 写操作工具（WriteFile, EditFile, RunCommand 等）是按需工具，通过三步协议调用：GetToolCatalog → DiscoverTools → ExecuteTool
//! - 派子代理（RunSubagent / RunSubagentsSequentially）同为按需工具：规划模式下它们在工具目录里
//!   根本不出现（`PLAN_BLOCKED_EXTRA`），编辑模式下才可发现
//! - 兜底防护：CHAT/QUESTION 意图禁止写操作；规划模式按注册表名单拦下写工具
//! - 工具目录过滤（GetToolCatalog/DiscoverTools）与运行时校验（should_block_write_tool）
//!   共用 `ToolRegistry::is_available`，保证"目录里看不见 = 调用不成功"

pub mod agent_tools;
pub mod file_tools;
pub mod framework;
pub mod notebook_tools;
pub mod search_tools;
pub mod shell_tools;
pub mod system_tools;
pub mod task_tools;

use std::path::Path;

use crate::infra::types::models::Skill;
use crate::get_agent_home;
// `try_state` 是 Manager trait 的方法，不 import 就用不了（tool_filter_for 里要用）
use tauri::Manager;

// Re-export 供外部使用的公开接口
pub use agent_tools::run_subagent;
pub use file_tools::{generate_repo_map, search_in_dir};
pub use framework::agent_registry::{AgentRegistry, DEFAULT_AGENT_ROLE, IMPLEMENTATION_AGENT_ROLE};
pub use framework::permission::{
    ensure_path_permission, is_path_safe, request_permission, PermissionDecision, PermissionKind,
};
pub use framework::tool_search::{
    get_core_tool_definitions, get_deferred_tool_full_schema, get_deferred_tool_list,
    get_deferred_tool_search_entries, handle_search_tools,
    search_deferred_tools, DeferredToolSearchEntry,
};

/// 递归扫描 skills 目录，解析所有 SKILL.md 文件
///
/// 从项目根目录的 `skills/` 文件夹加载（与 `data/` 同级）。
pub fn load_all_skills() -> Vec<Skill> {
    let mut skills = Vec::new();
    let data_dir = get_agent_home();
    // skills 目录与 data 目录同级，位于项目根目录
    let skills_dir = data_dir.parent().unwrap_or(data_dir).join("skills");

    fn scan_skills(dir: &Path, skills: &mut Vec<Skill>) {
        if let Ok(entries) = std::fs::read_dir(dir) {
            for entry in entries.flatten() {
                let path = entry.path();
                if path.is_dir() {
                    scan_skills(&path, skills);
                } else if path.file_name().unwrap_or_default() == "SKILL.md" {
                    if let Ok(content) = std::fs::read_to_string(&path) {
                        if let Some(skill) = parse_skill(&content, &path) {
                            skills.push(skill);
                        }
                    }
                }
            }
        }
    }
    scan_skills(&skills_dir, &mut skills);
    skills
}

/// 解析 SKILL.md 的 YAML frontmatter（name/description）和正文
pub fn parse_skill(text: &str, path: &Path) -> Option<Skill> {
    if text.starts_with("---\n") || text.starts_with("---\r\n") {
        let parts: Vec<&str> = text.splitn(3, "---").collect();
        if parts.len() >= 3 {
            let frontmatter = parts[1];
            let body = parts[2].trim().to_string();

            let mut name = path
                .parent()
                .and_then(|p| p.file_name())
                .unwrap_or_default()
                .to_string_lossy()
                .to_string();
            let mut description = String::from("No description");

            for line in frontmatter.lines() {
                let parts: Vec<&str> = line.splitn(2, ':').collect();
                if parts.len() == 2 {
                    let k = parts[0].trim();
                    let v = parts[1].trim();
                    if k == "name" {
                        name = v.to_string();
                    } else if k == "description" {
                        description = v.to_string();
                    }
                }
            }
            return Some(Skill {
                name,
                description,
                body,
                path: path.to_string_lossy().to_string(),
            });
        }
    }
    None
}

// 获取工具定义（核心工具集，再按本会话的用户开关过滤）
//
// 注：这里**不是**"永远恒定"，而是"**会话内**恒定" —— 用户开关的快照在会话开始时
// 固化一次，此后所有 turn 复用，`tools` 参数因此保持字节恒定、prompt cache 照常命中。
// 用户改开关只会影响新会话（或显式点"应用到当前会话"）。
pub fn get_tools_definition(filter: &framework::registry::ToolFilter) -> Vec<serde_json::Value> {
    get_core_tool_definitions(filter)
}

/// 取本会话的工具开关快照（尚未快照时从配置读一次并固化，此后复用）。
///
/// 收口在这里：调用方只需 app + session_id，不必自己摸 SessionManager、
/// 也不必知道快照存在哪。**目录过滤与运行时校验必须走同一份** ——
/// 本函数就是它们的共同来源。
pub async fn tool_filter_for(
    app: &tauri::AppHandle,
    session_id: &str,
) -> framework::registry::ToolFilter {
    match app.try_state::<crate::infra::state::state::SessionManager>() {
        Some(manager) => manager.get_or_create(session_id).await.tool_filter().await,
        // 没有 SessionManager（测试 / 启动极早期）：全部启用 ——
        // 开关是"少给几个工具"的偏好，不该在拿不到会话时把功能整个锁死。
        None => framework::registry::ToolFilter::allow_all(),
    }
}

/// 工具调用路由：根据工具名分发到对应模块
pub async fn handle_tool_call(
    app: &tauri::AppHandle,
    name: &str,
    input: &serde_json::Value,
    session_id: &str,
    intent: &str,
    work_mode: &str,
) -> (String, u64, u64) {
    // 说明：RunSubagent 曾在这里有一条独家的"直调分支"（自己接模式拦截与权限判定，
    // 绕开 dispatch）。该分支已退役——它现在是普通按需工具，与其他所有工具走同一条路：
    // ExecuteTool → dispatch_tool_call。退役理由见 dispatch_tool_call 里该工具分支的注释。
    if name == "ExecuteTool" {
        let filter = tool_filter_for(app, session_id).await;
        let result = framework::tool_search::handle_execute_tool(
            app, input, session_id, "main", intent, work_mode, &filter,
        ).await;
        // handle_execute_tool 内部已通过 log_deferred_call 记录了完整的审计日志
        // （含工具名、错误类型、纠正追踪）。此处不再重复记录，避免同一次调用产生
        // 两条日志条目（一条带实际工具名，一条仅显示 ExecuteTool）。
        {
            use tauri::Manager;
            let sm = app.state::<crate::infra::state::state::SessionManager>();
            let ctx = sm.get_or_create(session_id).await;
            let mut flags = ctx.tool_result_flags.lock().await;
            flags.insert(name.to_string(), (result.break_loop, result.is_error));
        }
        (result.output, result.input_tokens, result.output_tokens)
    } else {
        // 拦截直接调用按需工具：按需工具必须通过 ExecuteTool 代理执行。
        //
        // 但先判一次它在当前模式/意图下是否可用：不可用的（如规划模式下的 RunSubagent）
        // 直接回模式文案——否则模型会照着"请通过 ExecuteTool 代理执行"再试一次，
        // 到 ExecuteTool 里再吃一次拒绝，白烧一个来回。
        if let Some(tool_def) = framework::registry::ToolRegistry::global().get(name) {
            if tool_def.should_defer {
                let filter = tool_filter_for(app, session_id).await;
                if !framework::registry::ToolRegistry::is_available(
                    tool_def,
                    intent,
                    work_mode,
                    &filter,
                ) {
                    if let Some(message) = mode_block_message(name, intent, work_mode) {
                        return (message, 0, 0);
                    }
                }
                let available = get_deferred_tool_list(intent, work_mode, &filter);
                let names: Vec<String> = available.iter().map(|(n, _)| n.clone()).collect();
                return (
                    format!(
                        "工具 '{}' 是按需工具，不能直接调用。请通过 ExecuteTool 代理执行。\n\
                        用法: ExecuteTool(name=\"{}\", args={{...}})\n\
                        当前意图下可用的按需工具: {}",
                        name, name,
                        if names.is_empty() { "无".to_string() } else { names.join(", ") }
                    ),
                    0, 0,
                );
            }
        }

        let result = dispatch_tool_call(app, name, input, session_id, intent, work_mode, "main").await;
        // 记录工具调用审计日志
        let logger = framework::tool_call_logger::tool_call_logger();
        if result.is_blocked {
            logger.log_core_call(
                session_id, "main", name, input, intent, work_mode,
                framework::tool_call_logger::ToolCallStatus::Blocked,
                Some(result.output.chars().take(500).collect()),
            );
        } else if result.is_error {
            logger.log_core_call(
                session_id, "main", name, input, intent, work_mode,
                framework::tool_call_logger::ToolCallStatus::Error,
                Some(result.output.chars().take(500).collect()),
            );
        } else {
            logger.log_core_call(
                session_id, "main", name, input, intent, work_mode,
                framework::tool_call_logger::ToolCallStatus::Ok,
                None,
            );
        }
        // 将 break_loop/is_error 存入 session context，供 tools_runner 读取
        {
            use tauri::Manager;
            let sm = app.state::<crate::infra::state::state::SessionManager>();
            let ctx = sm.get_or_create(session_id).await;
            let mut flags = ctx.tool_result_flags.lock().await;
            flags.insert(name.to_string(), (result.break_loop, result.is_error));
        }
        (result.output, result.input_tokens, result.output_tokens)
    }
}

/// 并行执行用的 owned 版本，所有参数为 owned 值，可安全 move 进 tokio::spawn
pub async fn handle_tool_call_owned(
    app: tauri::AppHandle,
    name: String,
    input: serde_json::Value,
    session_id: String,
    intent: String,
    work_mode: String,
) -> (String, u64, u64) {
    handle_tool_call(&app, &name, &input, &session_id, &intent, &work_mode).await
}

/// 子Agent并行工具执行用的 owned 版本（不含 task 路由）
///
/// 返回 `(输出文本, 是否失败)`，供子代理侧统计"本轮是否全军覆没"。
pub async fn handle_tool_call_inner_owned(
    app: tauri::AppHandle,
    name: String,
    input: serde_json::Value,
    session_id: String,
    intent: String,
    work_mode: String,
    agent_type: String,
) -> (String, bool) {
    handle_tool_call_inner(&app, &name, &input, &session_id, &intent, &work_mode, &agent_type).await
}

/// 主 Agent 路径的工具分发：比 [`dispatch_core_tool`] 多一个 `RunSubagent`。
///
/// **为什么必须拆成两个函数**（2026-09-21 实测结论，别再合并回去）：
/// `run_subagent` 内部会 `tokio::spawn` 子代理的工具执行（`subagent.rs`），
/// 那条路最终走到 `handle_tool_call_inner`。若两条路共用同一个 async fn，就形成
/// opaque 类型自递归——dispatch 的 future 里含 run_subagent 的 future，而 run_subagent
/// 那处 spawn 又要求 fetch dispatch 自己的 hidden type。rustc 拒绝这种形状：
/// `fetching the hidden types of an opaque inside of the defining scope is not supported`，
/// 对外报成 `future cannot be sent between threads safely`（E0277）。
///
/// 拆开之后类型图无环：**子代理那条路只能走到不含 `RunSubagent` 的执行器**，
/// 于是"子代理不能再派子代理"这条运行时过滤，升级成了类型结构上的保证。
pub async fn dispatch_tool_call(
    app: &tauri::AppHandle,
    name: &str,
    input: &serde_json::Value,
    session_id: &str,
    intent: &str,
    work_mode: &str,
    agent_type: &str,
) -> framework::ToolCallResult {
    if name != "RunSubagent" {
        return dispatch_core_tool(app, name, input, session_id, intent, work_mode, agent_type)
            .await;
    }

    // 全工程唯一一处派子代理。它现在是**按需工具**（defer: true），模型经
    // GetToolCatalog → DiscoverTools → ExecuteTool 抵达这里，与 RunSubagentsSequentially
    // 同级同口径；规划模式下它在工具目录里根本不出现（PLAN_BLOCKED_EXTRA）。
    //
    // 它曾长期是 handle_tool_call 里的独家「直调分支」，理由是「进 dispatch 会编译失败」——
    // 现象是真的（见上方文档注释里的 opaque 自递归），但结论「只能当核心工具」是绕路：
    // 真正的修法是让执行器分成两支，而不是让它留在恒定的 tools 参数里。
    //
    // 模式拦截与权限判定在这里各接一次：本分支直接 return，不会再经过
    // dispatch_core_tool 的统一前置检查。
    if let Some(message) = mode_block_message(name, intent, work_mode) {
        return framework::ToolCallResult::error(message);
    }
    if let Some(result) =
        framework::policy_guard::enforce(app, session_id, name, input, agent_type).await
    {
        return result;
    }
    let requested_agent_role =
        framework::agent_registry::normalize_agent_role(input["subagent_type"].as_str());
    let agent_registry = AgentRegistry::global();
    let Some(agent) = agent_registry.get(requested_agent_role) else {
        return framework::ToolCallResult::error(format!(
            "Unknown subagent_type '{}'. Available types: {}",
            requested_agent_role,
            agent_registry.available_types().join(", ")
        ));
    };
    let read_only = input["read_only"]
        .as_bool()
        .unwrap_or(agent.read_only_default);
    let task_id = input["task_id"].as_i64().map(|id| id as i32);
    let label = input["description"].as_str().map(|value| value.to_string());
    let model_override = input["model"]
        .as_str()
        .filter(|value| !value.trim().is_empty())
        .map(|value| value.to_string());
    let skills: Option<Vec<String>> = input["skills"]
        .as_array()
        .map(|arr| arr.iter().filter_map(|v| v.as_str().map(|s| s.to_string())).collect());
    let prompt = input["prompt"].as_str().unwrap_or("");
    // 这里可以直接 await：本函数是"主 Agent 专用"的那一支，子代理路径到不了它，
    // 自递归的 opaque 环已经不存在——这正是拆函数的全部意义。
    let (output, in_tokens, out_tokens) = run_subagent(
        app.clone(),
        prompt.to_string(),
        read_only,
        session_id.to_string(),
        task_id,
        label,
        Some(agent.agent_role.to_string()),
        model_override,
        skills,
    )
    .await;
    framework::ToolCallResult::ok(output).with_usage(in_tokens, out_tokens)
}

/// 核心工具执行器（**不含** `RunSubagent`）：主 Agent 与子代理共用的一支。
///
/// ExecuteTool 代理执行时直接调用此函数（避免与 `handle_tool_call` 递归）。
///
/// 返回 `ToolCallResult`，日志层通过 `is_error` 字段判断成败，
/// 而非扫描返回文本中的关键词（工具 schema 描述中天然包含 "error" 等字样，
/// 朴素字符串匹配会导致误判）。
pub async fn dispatch_core_tool(
    app: &tauri::AppHandle,
    name: &str,
    input: &serde_json::Value,
    session_id: &str,
    intent: &str,
    work_mode: &str,
    agent_type: &str,
) -> framework::ToolCallResult {
    // 兜底防护：CHAT/QUESTION 意图，或 chat/plan 模式下禁止写操作
    if let Some(message) = mode_block_message(name, intent, work_mode) {
        return framework::ToolCallResult::error(message);
    }

    // ── 执行前权限判定（第二步）──
    // 能力/模式允许之后，才轮到"要不要问用户"或"直接拒绝"。
    // 判定依据是结构化事实（工具、路径、是否覆盖、影响几个文件），
    // 与观察层共用同一份事实采集，保证"预判"和"真实判定"口径一致。
    if let Some(result) =
        framework::policy_guard::enforce(app, session_id, name, input, agent_type).await
    {
        return result;
    }

    let result = match name {
        // 文件工具
        "ListDirectory" => file_tools::list_directory(app, input, session_id).await,
        "SearchRepo" => file_tools::search_repo(app, input, session_id).await,
        "FindFiles" => search_tools::glob(app, input, session_id).await,
        "SearchText" => search_tools::grep(app, input, session_id).await,
        "EditNotebook" => notebook_tools::notebook_edit(app, input, session_id).await,
        "ReadFile" => file_tools::read_file(app, input, session_id).await,
        "ReadFileSkeleton" => file_tools::read_file_skeleton(app, input, session_id).await,
        "FindSymbol" => file_tools::find_symbol(app, input, session_id).await,
        "ReadSymbol" => file_tools::read_symbol(app, input, session_id).await,
        "FindReferences" => file_tools::find_references(app, input, session_id).await,
        "CodeSearch" => file_tools::code_search(app, input, session_id).await,
        "DeleteFile" => file_tools::delete_file(app, input, session_id).await,
        "RenameFile" => file_tools::rename_file(app, input, session_id).await,
        "WriteFile" => file_tools::write_file(app, input, session_id).await,
        "EditFile" => file_tools::edit_file(app, input, session_id).await,
        "ApplyPatch" => file_tools::apply_patch(app, input, session_id).await,

        // Shell 工具
        // RunCommand：外层人审在上方 enforce 已完成（弹卡或会话允许放行），
        // 传 true = 跳过二道门重复弹卡（防同一次执行连弹两张卡）；直调路径才传 false 兜底
        "RunCommand" => shell_tools::run_shell(app, input, session_id, true).await,
        "StartBackgroundCommand" => shell_tools::background_run(app, input, session_id).await,
        "CheckBackgroundCommand" => shell_tools::check_background(app, input, session_id).await,

        // 任务工具
        "UpdateTodos" => task_tools::todo_write(app, input, session_id).await,
        "CreateTask" => task_tools::task_create(app, input, session_id).await,
        "UpdateTask" => task_tools::task_update(app, input, session_id).await,
        "DeleteTask" => task_tools::task_delete(app, input, session_id).await,
        "ListTasks" => task_tools::task_list(app, input, session_id).await,
        "GetTask" => task_tools::task_get(app, input, session_id).await,
        "SummarizeTasks" => task_tools::task_summary(app, input, session_id).await,

        // Agent 工具
        "LoadSkill" => agent_tools::load_skill(app, input, session_id).await,
        "GetToolCatalog" => {
            agent_tools::get_tool_catalog(app, input, session_id, intent, work_mode).await
        }
        "CompactConversation" => agent_tools::compact(app, input, session_id).await,
        "ConsolidateMemory" => agent_tools::consolidate_memory(app, input, session_id).await,
        "ReadMemory" => agent_tools::read_memory(app, input, session_id).await,
        "UpdateMemory" => agent_tools::update_memory(app, input, session_id).await,

        // 任务调度器 — 同步等待所有任务完成，实时推送进度到前端
        "RunSubagentsSequentially" => {
            use crate::core::orchestration::scheduler::TaskScheduler;
            let cancel_token = tokio_util::sync::CancellationToken::new();
            let (report, in_tokens, out_tokens) = TaskScheduler::run_schedule(
                app, session_id, "", &cancel_token,
            ).await;
            return framework::ToolCallResult::ok(report).with_usage(in_tokens, out_tokens);
        }

        // 方案审批工具（提交后 LLM 自行决定结束 loop）
        "ProposePlan" => {
            return framework::ToolCallResult::ok(
                agent_tools::propose_plan(app, input, session_id).await,
            );
        }

        // 工作模式切换
        "SwitchWorkMode" => agent_tools::switch_work_mode(app, input, session_id).await,

        // 工具搜索（纯搜索，始终成功）
        "DiscoverTools" => {
            let filter = tool_filter_for(app, session_id).await;
            return framework::ToolCallResult::ok(
                framework::tool_search::handle_search_tools(
                    input, intent, session_id, work_mode, &filter,
                )
                .await,
            );
        }

        _ => return framework::ToolCallResult::error(format!("未知工具: {}", name)),
    };

    // 所有工具已内置 ToolCallResult 错误标记，直接返回
    result
}

/// 内部工具调用分发（含 ExecuteTool 路由）
pub async fn handle_tool_call_inner(
    app: &tauri::AppHandle,
    name: &str,
    input: &serde_json::Value,
    session_id: &str,
    intent: &str,
    work_mode: &str,
    // 调用来源标记：主循环传 "main"；子代理传 "subagent:<role>"（如 subagent:explore）。
    // 它会一路传进工具审计日志与权限审计，用于区分"哪种类型的子代理在干活"（2026-09-20）。
    agent_type: &str,
) -> (String, bool) {
    // 返回 `(输出文本, 是否失败)` —— 子代理侧要用失败标志做空转保护
    // （连续多轮全失败 = 结构性受阻，提前收口，见 `subagent.rs`）。
    //
    // 子代理路径只能走**不含 RunSubagent** 的执行器。两个理由：
    //
    // 1. 类型上必须如此：`run_subagent` 内部 `tokio::spawn` 的那条路会回到本函数，
    //    这里若还能走到含 RunSubagent 的分支，就构成 opaque 类型自递归
    //    （细节见 `dispatch_tool_call` 的文档注释）。
    // 2. 语义上也本该如此：子代理不能再派子代理。
    //
    // 这里也不再单独路由 ExecuteTool：按需工具协议是主 Agent 的上下文经济手段；
    // 子代理的工具集由 agent 名单直接给出完整 schema（`resolve_tools` 不看
    // should_defer），而 ExecuteTool 不在任何 agent 的名单里。走到这里只可能是协议误用，
    // 交给下游的「未知工具」报错即可。
    let result = dispatch_core_tool(app, name, input, session_id, intent, work_mode, agent_type)
        .await;
    let logger = framework::tool_call_logger::tool_call_logger();
    if result.is_error {
        logger.log_core_call(
            session_id, agent_type, name, input, intent, work_mode,
            framework::tool_call_logger::ToolCallStatus::Error,
            Some(result.output.chars().take(500).collect()),
        );
    } else {
        logger.log_core_call(
            session_id, agent_type, name, input, intent, work_mode,
            framework::tool_call_logger::ToolCallStatus::Ok,
            None,
        );
    }
    // 将 break_loop/is_error 存入 session context，供 tools_runner 读取
    {
        use tauri::Manager;
        let sm = app.state::<crate::infra::state::state::SessionManager>();
        let ctx = sm.get_or_create(session_id).await;
        let mut flags = ctx.tool_result_flags.lock().await;
        flags.insert(name.to_string(), (result.break_loop, result.is_error));
    }
    (result.output, result.is_error)
}

/// 判断是否应该阻止工具调用（意图 + 工作模式兜底防护）
///
/// 注意它拦的不只是"写文件"：规划模式下 `WRITE_TOOLS` 之外的派子代理、改工作目录
/// 也一并拦下（名单见 `registry::ToolRegistry::PLAN_BLOCKED_EXTRA`）。
/// 函数名沿用历史叫法，语义以 `is_blocked_in_plan_mode` 为准。
pub fn should_block_write_tool(name: &str, intent: &str, work_mode: &str) -> bool {
    // 条件1：意图是 CHAT 或 QUESTION
    if matches!(intent, "CHAT" | "QUESTION") {
        return is_write_tool(name);
    }

    // 条件2：工作模式是 plan（只探索 + 提方案）
    if work_mode == "plan" {
        return framework::registry::ToolRegistry::is_blocked_in_plan_mode(name);
    }

    false
}

/// 兜底拦截的统一文案：返回 `Some(文本)` 表示这个调用不要执行，把文本回灌给模型。
///
/// 三个调用点，口径必须一致：
/// - `dispatch_core_tool` 的统一前置检查（核心工具）；
/// - `dispatch_tool_call` 的 `RunSubagent` 分支（它直接 return，不走上面那条）；
/// - `handle_tool_call` 里的按需工具直调拦截（当前模式不可用时优先回这段文案）。
pub fn mode_block_message(name: &str, intent: &str, work_mode: &str) -> Option<String> {
    if !should_block_write_tool(name, intent, work_mode) {
        return None;
    }
    Some(format!(
        "工具 '{}' 在当前状态下不可用。{}",
        name,
        match work_mode {
            "plan" => {
                "规划模式下只能探索代码和提交方案：改文件、跑写命令、派子代理、改工作目录都被禁用。\
                 请先用 ProposePlan 提交方案；确实需要直接改动，请先切换到编辑模式。"
            }
            _ => "当前意图下只能使用只读工具。",
        }
    ))
}

/// 判断是否是写操作工具
///
/// 名单的唯一来源是 `ToolRegistry::WRITE_TOOLS`：目录过滤、搜索、执行、能力清单
/// 都从这里推导，避免"目录里看得见、调用时才拦"的两套真相。
pub fn is_write_tool(name: &str) -> bool {
    framework::registry::ToolRegistry::is_write_tool_name(name)
}

#[cfg(test)]
mod write_guard_tests {
    use super::*;
    // ToolFilter 是给可见性判断加的"会话维度"参数；这些用例验证的是模式/意图过滤，
    // 与用户开关无关，统一传 allow_all()
    use crate::core::tools::framework::registry::ToolFilter;

    #[test]
    fn plan_mode_blocks_write_tools() {
        // 规划模式只允许探索 + 提方案，写操作必须在编辑模式里做
        assert!(should_block_write_tool("WriteFile", "ACTION", "plan"));
        assert!(should_block_write_tool("RunCommand", "ACTION", "plan"));
        assert!(!should_block_write_tool("ReadFile", "ACTION", "plan"));
        assert!(!should_block_write_tool("ProposePlan", "ACTION", "plan"));
        // 任务编排（写的是会话自己的任务列表，不碰用户代码）在规划模式里要留着：
        // 规划模式的产物之一就是任务分解
        assert!(!should_block_write_tool("CreateTask", "ACTION", "plan"));
    }

    #[test]
    fn plan_mode_blocks_subagent_paths() {
        // 这两个都不在 WRITE_TOOLS 里，但都绕得过它：
        // 子代理内层固定 edit 模式（能写文件）、调度器派子代理时 read_only 恒为 false。
        // 规划模式下必须一起收走。
        // （SetWorkspace 已随工具退役移出名单，见 system_tools/mod.rs 模块注释）
        assert!(should_block_write_tool("RunSubagent", "ACTION", "plan"));
        assert!(should_block_write_tool(
            "RunSubagentsSequentially",
            "ACTION",
            "plan"
        ));
    }

    #[test]
    fn mode_block_message_only_fires_when_blocked() {
        assert!(mode_block_message("RunSubagent", "ACTION", "plan").is_some());
        assert!(mode_block_message("RunSubagent", "ACTION", "edit").is_none());
        // 文案要让模型知道"别换写法重试"，而不是只说不可用
        let msg = mode_block_message("WriteFile", "ACTION", "plan").unwrap();
        assert!(msg.contains("派子代理"));
        assert!(msg.contains("ProposePlan"));
    }

    #[test]
    fn edit_mode_allows_write_tools() {
        assert!(!should_block_write_tool("WriteFile", "ACTION", "edit"));
        assert!(!should_block_write_tool("RunCommand", "ACTION", "edit"));
    }

    #[test]
    fn chat_intent_blocks_writes_in_edit_mode() {
        // 意图层的兜底还在：闲聊/提问意图下不允许写操作
        assert!(should_block_write_tool("WriteFile", "CHAT", "edit"));
        assert!(should_block_write_tool("EditFile", "QUESTION", "edit"));
    }

    #[test]
    fn unknown_tool_falls_through_to_unknown_tool_error() {
        // 未注册的工具名交给下游的「未知工具」错误路径，不该被误判成策略拦截
        assert!(!should_block_write_tool("NoSuchTool", "ACTION", "edit"));
    }

    #[test]
    fn edit_mode_still_allows_orchestration_tools() {
        assert!(!should_block_write_tool("SwitchWorkMode", "ACTION", "edit"));
        assert!(!should_block_write_tool("CreateTask", "ACTION", "edit"));
        assert!(!should_block_write_tool("RunSubagent", "ACTION", "edit"));
    }

    #[test]
    fn run_subagent_is_deferred_not_core() {
        // 反向锁（2026-09-21）：这里原本锁的是"RunSubagent 必须保持核心工具"，
        // 理由是 2026-09-20 那次"进 dispatch 会编译失败"的排查结论。
        //
        // 现象是真的（opaque 类型自递归，见 `dispatch_tool_call` 的文档注释），
        // 但结论绕路了：当时的另一条修法——把执行器拆成"含 RunSubagent 的主 Agent 支"
        // 与"子代理也走的共享支"——才是正解。赖在核心集合里的代价是：核心工具集恒定
        // （多厂商前缀缓存），而 RunSubagent 在 PLAN_BLOCKED_EXTRA 里，于是规划模式下
        // 模型会一直看见一个"调用必被拦"的工具，还会为它绕路。
        let registry = framework::registry::ToolRegistry::global();
        let def = registry
            .get("RunSubagent")
            .expect("RunSubagent 必须已注册");
        assert!(
            def.should_defer,
            "RunSubagent 必须是按需工具：它进 PLAN_BLOCKED_EXTRA，留在恒定核心集里等于在规划模式下白占 schema"
        );
        // 先绑定到变量：get_core_definitions() 返回 owned Vec，
        // 直接 .iter() 链式会借用临时值，语句结束即释放（E0716）。
        let core_defs = registry.get_core_definitions(&ToolFilter::allow_all());
        let core_names: Vec<&str> = core_defs
            .iter()
            .filter_map(|d| d["name"].as_str())
            .collect();
        assert!(
            !core_names.contains(&"RunSubagent"),
            "RunSubagent 不得出现在核心工具定义（模型的 tools 参数）里"
        );
    }

    /// 目标行为锁：规划模式看不见 RunSubagent，编辑模式找得到。
    ///
    /// 「看不见」由两条一起保证：它不在恒定的 tools 参数里（按需工具），
    /// 且按需目录按 `is_available` 过滤（PLAN_BLOCKED_EXTRA）。
    #[test]
    fn plan_mode_hides_run_subagent_edit_mode_exposes_it() {
        let registry = framework::registry::ToolRegistry::global();
        let def = registry
            .get("RunSubagent")
            .expect("RunSubagent 必须已注册");

        assert!(
            !framework::registry::ToolRegistry::is_available(def, "PROJECT_ACTION", "plan", &ToolFilter::allow_all()),
            "规划模式下 RunSubagent 必须不可用（否则子代理内层 edit 模式=写保护旁路）"
        );
        assert!(
            framework::registry::ToolRegistry::is_available(def, "PROJECT_ACTION", "edit", &ToolFilter::allow_all()),
            "编辑模式下必须可用，否则这个工具等于被删了"
        );

        let plan_list = framework::tool_search::get_deferred_tool_list("PROJECT_ACTION", "plan", &ToolFilter::allow_all());
        assert!(
            !plan_list.iter().any(|(n, _)| n == "RunSubagent"),
            "规划模式的按需工具目录里不得出现 RunSubagent"
        );
        let edit_list = framework::tool_search::get_deferred_tool_list("PROJECT_ACTION", "edit", &ToolFilter::allow_all());
        assert!(
            edit_list.iter().any(|(n, _)| n == "RunSubagent"),
            "编辑模式的按需工具目录里必须有 RunSubagent，否则模型永远发现不了它"
        );
    }
}
