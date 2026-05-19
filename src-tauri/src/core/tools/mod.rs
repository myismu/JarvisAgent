//! # mod.rs — 工具系统入口模块
//!
//! 工具系统的中央枢纽：模块注册、技能加载、工具定义组装、路由分发。
//! 工具参数（tools）始终不变以保证 prompt cache 命中，意图+工作模式仅影响上下文注入和 RunDeferredTool 运行时校验。
//!
//! ## 关键导出
//! - `get_tools_definition()`: 返回固定核心工具定义列表（不含意图参数，保证缓存命中）
//! - `handle_tool_call()` / `handle_tool_call_owned()`: 工具调用路由入口
//! - `load_all_skills()`: 从 skills 目录加载所有 SKILL.md 技能文件
//!
//! ## 依赖
//! - Internal: 各工具子模块（file_tools, shell_tools, task_tools, agent_tools, system_tools, tool_search）
//! - External: `serde_json`, `tauri`
//!
//! ## 约束
//! - tools 参数始终不变，意图过滤仅通过上下文注入 + RunDeferredTool 运行时校验
//! - 子代理（SUBAGENT）不能调用 RunSubagent / ConsolidateMemory / CompactConversation / RunSubagentsSequentially

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
pub use framework::permission::{ensure_path_permission, is_path_safe, request_permission};
pub use framework::tool_search::{
    get_core_tool_definitions, get_deferred_tool_full_schema, get_deferred_tool_list,
    get_deferred_tool_search_entries, get_deferred_tools_context,
    get_deferred_tools_context_compact, handle_search_tools,
    search_deferred_tools, DeferredToolSearchEntry,
};

/// 递归扫描 skills 目录，解析所有 SKILL.md 文件
pub fn load_all_skills() -> Vec<Skill> {
    let mut skills = Vec::new();
    let home = get_agent_home();
    let mut skills_dir = home.join(crate::infra::types::constants::DIR_SKILLS);
    if !skills_dir.exists() {
        skills_dir = home.join("..").join("skills");
    }

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
            });
        }
    }
    None
}

// 获取工具定义（始终返回固定核心工具集，保证 prompt cache 命中）
// 意图+工作模式过滤仅通过上下文注入 + RunDeferredTool 运行时校验实现
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
) -> (String, u64, u64) {
    if name == "RunSubagent" {
        let prompt = input["prompt"].as_str().unwrap_or("");
        let requested_agent_role = framework::agent_registry::normalize_agent_role(
            input["subagent_role"]
                .as_str()
                .or_else(|| input["agent_role"].as_str()),
        );
        let agent_registry = AgentRegistry::global();
        let Some(agent) = agent_registry.get(requested_agent_role) else {
            return (
                format!(
                    "Unknown subagent_role '{}'. Available types: {}",
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
        let fut = run_subagent(
            app.clone(),
            prompt.to_string(),
            read_only,
            session_id.to_string(),
            task_id,
            label,
            Some(agent.agent_role.to_string()),
            model_override,
        );
        Box::pin(fut).await
    } else if name == "RunSubagentsSequentially" {
        use tauri::Manager;
        use crate::core::orchestration::scheduler::{TaskScheduler, SchedulerEvent};
        use crate::infra::state::state::SessionManager;
        let ctx = app.state::<SessionManager>().get_or_create(session_id).await;
        let (tx, rx) = tokio::sync::mpsc::unbounded_channel::<SchedulerEvent>();
        *ctx.scheduler_rx.lock().await = Some(rx);
        let cancel_token = tokio_util::sync::CancellationToken::new();
        TaskScheduler::run_schedule_async(
            app.clone(), session_id.to_string(), cancel_token, tx,
        );
        ("调度已启动，任务正在后台执行。你将实时收到每个任务的完成/失败通知。".to_string(), 0, 0)
    } else {
        (
            handle_tool_call_inner(app, name, input, session_id, intent).await,
            0,
            0,
        )
    }
}

/// 并行执行用的 owned 版本，所有参数为 owned 值，可安全 move 进 tokio::spawn
pub async fn handle_tool_call_owned(
    app: tauri::AppHandle,
    name: String,
    input: serde_json::Value,
    session_id: String,
    intent: String,
) -> (String, u64, u64) {
    handle_tool_call(&app, &name, &input, &session_id, &intent).await
}

/// 子Agent并行工具执行用的 owned 版本（不含 task 路由）
pub async fn handle_tool_call_inner_owned(
    app: tauri::AppHandle,
    name: String,
    input: serde_json::Value,
    session_id: String,
    intent: String,
) -> String {
    handle_tool_call_inner(&app, &name, &input, &session_id, &intent).await
}

/// 工具调用核心分发（不含 RunDeferredTool 路由，避免递归）
/// RunDeferredTool 代理执行时直接调用此函数
pub async fn dispatch_tool_call(
    app: &tauri::AppHandle,
    name: &str,
    input: &serde_json::Value,
    session_id: &str,
    intent: &str,
) -> String {
    match name {
        // 系统工具
        "SetWorkspace" => system_tools::set_workspace(app, input, session_id).await,

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
        "CompactConversation" => agent_tools::compact(app, input, session_id).await,
        "ConsolidateMemory" => agent_tools::dream(app, input, session_id).await,

        // 方案审批工具
        "ProposePlan" => agent_tools::propose_plan(app, input, session_id).await,

        // 工作模式切换
        "SwitchWorkMode" => agent_tools::switch_work_mode(app, input, session_id).await,

        // 工具搜索
        "SearchTools" => framework::tool_search::handle_search_tools(input, intent).await,

        _ => format!("未知工具: {}", name),
    }
}

/// 内部工具调用分发（含 RunDeferredTool 路由）
pub async fn handle_tool_call_inner(
    app: &tauri::AppHandle,
    name: &str,
    input: &serde_json::Value,
    session_id: &str,
    intent: &str,
) -> String {
    // RunDeferredTool 单独路由，避免与 dispatch_tool_call 递归
    if name == "RunDeferredTool" {
        return framework::tool_search::handle_run_deferred_tool(app, input, session_id, intent)
            .await;
    }
    dispatch_tool_call(app, name, input, session_id, intent).await
}

