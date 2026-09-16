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
//! - tools 参数始终不变，意图过滤仅通过上下文注入 + ExecuteTool 运行时校验
//! - 子代理（SUBAGENT）不能调用 RunSubagent / ConsolidateMemory / CompactConversation / RunSubagentsSequentially
//! - 写操作工具（WriteFile, EditFile, RunCommand 等）是延迟工具，通过三步协议调用：GetToolCatalog → DiscoverTools → ExecuteTool
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

// 获取工具定义（始终返回固定核心工具集，保证 prompt cache 命中）
// 意图+工作模式过滤仅通过上下文注入 + ExecuteTool 运行时校验实现
pub fn get_tools_definition() -> Vec<serde_json::Value> {
    get_core_tool_definitions()
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
    if name == "RunSubagent" {
        // 这条分支**不经过** dispatch_tool_call 的兜底防护（它自己直接跑子代理），
        // 所以模式拦截与权限判定都必须在这里各接一次。
        // 漏掉模式拦截的后果：规划模式下派个子代理就能写文件（子代理内层固定 edit 模式）；
        // 漏掉权限判定的后果："派子代理"成为绕过检查的后门。
        if let Some(message) = mode_block_message(name, intent, work_mode) {
            return (message, 0, 0);
        }
        if let Some(result) =
            framework::policy_guard::enforce(app, session_id, name, input, "main").await
        {
            return (result.output, 0, 0);
        }
        let prompt = input["prompt"].as_str().unwrap_or("");
        let requested_agent_role = framework::agent_registry::normalize_agent_role(
            input["subagent_type"]
                .as_str()
                .or_else(|| input["subagent_role"].as_str())
                .or_else(|| input["agent_role"].as_str()),
        );
        let agent_registry = AgentRegistry::global();
        let Some(agent) = agent_registry.get(requested_agent_role) else {
            return (
                format!(
                    "Unknown subagent_type '{}'. Available types: {}",
                    requested_agent_role,
                    agent_registry.available_types().join(", ")
                ),
                0,
                0,
            );
        };
        let read_only = input["read_only"]
            .as_bool()
            .unwrap_or(agent.read_only_default);
        let task_id = input["task_id"]
            .as_i64()
            .or_else(|| input["taskId"].as_i64())
            .map(|id| id as i32);
        let label = input["description"]
            .as_str()
            .or_else(|| input["label"].as_str())
            .map(|value| value.to_string());
        let model_override = input["model"]
            .as_str()
            .filter(|value| !value.trim().is_empty())
            .map(|value| value.to_string());
        let skills: Option<Vec<String>> = input["skills"]
            .as_array()
            .map(|arr| arr.iter().filter_map(|v| v.as_str().map(|s| s.to_string())).collect());
        let fut = run_subagent(
            app.clone(),
            prompt.to_string(),
            read_only,
            session_id.to_string(),
            task_id,
            label,
            Some(agent.agent_role.to_string()),
            model_override,
            skills,
        );
        Box::pin(fut).await
    } else if name == "ExecuteTool" {
        let result = framework::tool_search::handle_execute_tool(
            app, input, session_id, "main", intent, work_mode,
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
        (result.output, 0, 0)
    } else {
        // 拦截直接调用延迟工具：延迟工具必须通过 ExecuteTool 代理执行
        if let Some(tool_def) = framework::registry::ToolRegistry::global().get(name) {
            if tool_def.should_defer {
                let available = get_deferred_tool_list(intent, work_mode);
                let names: Vec<String> = available.iter().map(|(n, _)| n.clone()).collect();
                return (
                    format!(
                        "工具 '{}' 是延迟工具，不能直接调用。请通过 ExecuteTool 代理执行。\n\
                        用法: ExecuteTool(name=\"{}\", args={{...}})\n\
                        当前意图下可用的延迟工具: {}",
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
        (result.output, 0, 0)
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
pub async fn handle_tool_call_inner_owned(
    app: tauri::AppHandle,
    name: String,
    input: serde_json::Value,
    session_id: String,
    intent: String,
    work_mode: String,
) -> String {
    handle_tool_call_inner(&app, &name, &input, &session_id, &intent, &work_mode).await
}

/// 工具调用核心分发（不含 ExecuteTool 路由，避免递归）
/// ExecuteTool 代理执行时直接调用此函数
///
/// 返回 `ToolCallResult`，日志层通过 `is_error` 字段判断成败，
/// 而非扫描返回文本中的关键词（工具 schema 描述中天然包含 "error" 等字样，
/// 朴素字符串匹配会导致误判）。
pub async fn dispatch_tool_call(
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
        "RunGitCommand" => shell_tools::git_command(app, input, session_id).await,
        "RunCommand" => shell_tools::run_shell(app, input, session_id).await,
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
            let (report, _in_tokens, _out_tokens) = TaskScheduler::run_schedule(
                app, session_id, "", &cancel_token,
            ).await;
            return framework::ToolCallResult::ok(report);
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
            return framework::ToolCallResult::ok(
                framework::tool_search::handle_search_tools(input, intent, session_id, work_mode)
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
) -> String {
    // ExecuteTool 单独路由，避免与 dispatch_tool_call 递归
    if name == "ExecuteTool" {
        let result = framework::tool_search::handle_execute_tool(app, input, session_id, "subagent", intent, work_mode)
            .await;
        // 子 Agent 不需要传播 break_loop，但需要记录 is_error
        let logger = framework::tool_call_logger::tool_call_logger();
        if result.is_error {
            logger.log_core_call(
                session_id, "subagent", name, input, intent, work_mode,
                framework::tool_call_logger::ToolCallStatus::Error,
                Some(result.output.chars().take(500).collect()),
            );
        }
        return result.output;
    }

    // 核心工具直接调用，通过 ToolCallResult 结构化判断成败
    let result =
        dispatch_tool_call(app, name, input, session_id, intent, work_mode, "subagent").await;
    let logger = framework::tool_call_logger::tool_call_logger();
    if result.is_error {
        logger.log_core_call(
            session_id, "subagent", name, input, intent, work_mode,
            framework::tool_call_logger::ToolCallStatus::Error,
            Some(result.output.chars().take(500).collect()),
        );
    } else {
        logger.log_core_call(
            session_id, "subagent", name, input, intent, work_mode,
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
    result.output
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
/// 抽出来是因为 `RunSubagent` 走的是 `handle_tool_call` 里一条**独立分支**，
/// 不经过 `dispatch_tool_call` 的检查（它其实是更容易被忽略的那条路）。
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
}
