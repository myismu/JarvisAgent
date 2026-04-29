//! # mod.rs — 工具系统入口模块
//!
//! 工具系统的中央枢纽：模块注册、技能加载、工具定义组装、路由分发。
//! 根据意图（intent）筛选工具集，支持渐进式披露（核心工具始终携带，延迟工具按需激活）。
//!
//! ## 关键导出
//! - `get_tools_definition()`: 按意图组装工具定义列表（含渐进式披露逻辑）
//! - `handle_tool_call()` / `handle_tool_call_owned()`: 工具调用路由入口
//! - `load_all_skills()`: 从 skills 目录加载所有 SKILL.md 技能文件
//!
//! ## 依赖
//! - Internal: 各工具子模块（file_tools, shell_tools, task_tools, agent_tools, system_tools, tool_search）
//! - External: `serde_json`, `tauri`
//!
//! ## 约束
//! - CHAT 意图不返回任何工具
//! - MEMORY_QUERY 意图只返回 read_file / compact / dream
//! - 子代理（SUBAGENT）不能调用 task / dream / compact / run_tasks

pub mod agent_registry;
pub mod agent_tools;
pub mod claude_code_tools;
pub mod file_tools;
pub mod notebook_tools;
pub mod permission;
pub mod registry;
pub mod shell_security;
pub mod shell_tools;
pub mod system_tools;
pub mod task_tools;
pub mod tool_search;

use serde_json::json;
use std::path::Path;

use crate::core::models::Skill;
use crate::get_agent_home;

// Re-export 供外部使用的公开接口
pub use agent_registry::{AgentRegistry, DEFAULT_AGENT_TYPE, IMPLEMENTATION_AGENT_TYPE};
pub use agent_tools::run_subagent;
pub use file_tools::{generate_repo_map, search_in_dir};
pub use permission::{ensure_path_permission, is_path_safe, request_permission};
pub use tool_search::{
    get_core_tool_definitions, get_deferred_tool_full_schema, get_deferred_tool_list,
    get_deferred_tool_search_entries, get_deferred_tools_context, handle_search_tools,
    search_deferred_tools, DeferredToolSearchEntry,
};

/// 递归扫描 skills 目录，解析所有 SKILL.md 文件
pub fn load_all_skills() -> Vec<Skill> {
    let mut skills = Vec::new();
    let home = get_agent_home();
    let mut skills_dir = home.join(crate::core::constants::DIR_SKILLS);
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

// 获取工具定义（按意图筛选 + 渐进式披露）
// activated_tools: 由 search_tools 激活的延迟工具名称列表
pub fn get_tools_definition(intent: &str, activated_tools: &[String]) -> Vec<serde_json::Value> {
    if intent == "CHAT" {
        return vec![];
    }

    // MEMORY_QUERY / QUESTION 工具集小，直接返回完整 schema
    if intent == "MEMORY_QUERY" {
        let mut tools = get_core_tool_definitions();
        // 核心工具中移除 search_tools（记忆查询不需要渐进式披露）
        tools.retain(|t| t["name"] != "search_tools");
        tools.extend(vec![
            json!({
                "name": "read_file",
                "description": "读取文件内容。支持语义化点读技术，可通过 start_line 和 end_line 获取特定代码块，避免 Context 过长。",
                "input_schema": {
                    "type": "object",
                    "properties": {
                        "path": {"type": "string"},
                        "start_line": {"type": "integer", "description": "可选。起始行号（从 1 开始）"},
                        "end_line": {"type": "integer", "description": "可选。结束行号（包含）"}
                    },
                    "required": ["path"]
                }
            }),
            json!({
                "name": "compact",
                "description": "手动触发对话上下文压缩。当对话上下文过长觉得需要清理或重置记忆时使用该工具。",
                "input_schema": {
                    "type": "object",
                    "properties": {
                        "focus": { "type": "string", "description": "摘要时需要特别保留的重点方向" }
                    }
                }
            }),
            json!({
                "name": "dream",
                "description": "主动触发记忆整理（Dream Agent）。将当前的零散碎片记忆提炼并合并进结构化用户画像中。",
                "input_schema": { "type": "object", "properties": {} }
            })
        ]);
        return tools;
    }

    // PROJECT_ACTION / SUBAGENT: 渐进式披露
    let mut tools = get_core_tool_definitions();
    if intent != "SUBAGENT" {
        if let Some(schema) = get_deferred_tool_full_schema("propose_plan") {
            tools.push(schema);
        }
    }

    // 添加已激活的延迟工具（完整 schema）
    let deferred_list = get_deferred_tool_list(intent);
    let deferred_names: Vec<&str> = deferred_list.iter().map(|(n, _)| n.as_str()).collect();

    for tool_name in activated_tools {
        if deferred_names.contains(&tool_name.as_str()) {
            if tool_name == "propose_plan" && intent != "SUBAGENT" {
                continue;
            }
            if let Some(schema) = get_deferred_tool_full_schema(tool_name) {
                tools.push(schema);
            }
        }
    }

    tools
}

/// 工具调用路由：根据工具名分发到对应模块
pub async fn handle_tool_call(
    app: &tauri::AppHandle,
    name: &str,
    input: &serde_json::Value,
    session_id: &str,
    intent: &str,
) -> (String, u64, u64) {
    if name == "task" {
        let prompt = input["prompt"].as_str().unwrap_or("");
        let requested_agent_type = agent_registry::normalize_agent_type(
            input["subagent_type"]
                .as_str()
                .or_else(|| input["agent_type"].as_str()),
        );
        let agent_registry = AgentRegistry::global();
        let Some(agent) = agent_registry.get(requested_agent_type) else {
            return (
                format!(
                    "Unknown subagent_type '{}'. Available types: {}",
                    requested_agent_type,
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
            Some(agent.agent_type.to_string()),
            model_override,
        );
        Box::pin(fut).await
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

/// 内部工具调用分发（非子代理工具）
/// 路由策略：match 分发（Rust 惯用方式）
/// 工具注册信息（schema、元数据）通过 registry::ToolRegistry 查询
pub async fn handle_tool_call_inner(
    app: &tauri::AppHandle,
    name: &str,
    input: &serde_json::Value,
    session_id: &str,
    intent: &str,
) -> String {
    match name {
        // 系统工具
        "set_workspace" => system_tools::set_workspace(app, input, session_id).await,
        "get_system_info" => system_tools::get_system_info(app, input, session_id).await,

        // 文件工具
        "list_directory" => file_tools::list_directory(app, input, session_id).await,
        "search_repo" => file_tools::search_repo(app, input, session_id).await,
        "glob" => claude_code_tools::glob(app, input, session_id).await,
        "grep" => claude_code_tools::grep(app, input, session_id).await,
        "notebook_edit" => notebook_tools::notebook_edit(app, input, session_id).await,
        "read_file" => file_tools::read_file(app, input, session_id).await,
        "read_file_skeleton" => file_tools::read_file_skeleton(app, input, session_id).await,
        "write_file" => file_tools::write_file(app, input, session_id).await,
        "edit_file" => file_tools::edit_file(app, input, session_id).await,

        // Shell 工具
        "git_command" => shell_tools::git_command(app, input, session_id).await,
        "run_shell" => shell_tools::run_shell(app, input, session_id).await,
        "background_run" => shell_tools::background_run(app, input, session_id).await,
        "check_background" => shell_tools::check_background(app, input, session_id).await,

        // 任务工具
        "todo_write" => task_tools::todo_write(app, input, session_id).await,
        "task_create" => task_tools::task_create(app, input, session_id).await,
        "task_update" => task_tools::task_update(app, input, session_id).await,
        "task_delete" => task_tools::task_delete(app, input, session_id).await,
        "task_list" => task_tools::task_list(app, input, session_id).await,
        "task_get" => task_tools::task_get(app, input, session_id).await,
        "task_summary" => task_tools::task_summary(app, input, session_id).await,

        // Agent 工具
        "load_skill" => agent_tools::load_skill(app, input, session_id).await,
        "compact" => agent_tools::compact(app, input, session_id).await,
        "dream" => agent_tools::dream(app, input, session_id).await,

        // 方案审批工具
        "propose_plan" => agent_tools::propose_plan(app, input, session_id).await,

        // 工具搜索
        "search_tools" => tool_search::handle_search_tools(input, intent).await,

        _ => format!("未知工具: {}", name),
    }
}
