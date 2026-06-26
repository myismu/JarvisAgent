//! # tool_search.rs — 渐进式工具披露模块
//!
//! 核心工具始终携带完整 schema（在 tools 参数中，保证缓存命中），
//! 延迟工具和技能通过 `GetToolCatalog` 统一发现：
//!   - 延迟工具: `GetToolCatalog` → `DiscoverTools` → `ExecuteTool`
//!   - 技能: `GetToolCatalog` → `LoadSkill`
//!
//! 所有工具的 schema 和元数据已迁移到各模块的 `define_tools!` 注册，
//! 本模块从 `ToolRegistry` 统一查询，不再维护硬编码的 JSON Schema。
//!
//! ## 关键导出
//! - `get_core_tool_definitions()`: 获取核心工具（始终带完整 schema）
//! - `get_deferred_tool_list()`: 获取延迟工具列表（名称+简述，供内部筛选/兼容）
//! - `get_deferred_tool_search_entries()`: 获取延迟工具搜索索引（名称+简述+提示词）
//! - `get_deferred_tool_full_schema()`: 按名称获取延迟工具的完整 Schema
//! - `search_deferred_tools()`: 关键词搜索延迟工具（支持 `select:` 精确选择）
//! - `handle_search_tools()`: DiscoverTools 处理函数（纯搜索指引）
//! - `handle_execute_tool()`: ExecuteTool 处理函数（代理执行延迟工具，含兜底防护）
//!
//! ## 依赖
//! - Internal: `registry::ToolRegistry`
//! - External: `serde_json`
//!
//! ## 约束
//! - tools 参数始终不变，保证 prompt cache 命中
//! - 延迟工具只能通过 ExecuteTool 间接调用
//! - 搜索评分：精确名称匹配 12 分，名称包含 5 分，搜索提示包含 3 分，描述包含 2 分
//! - `select:` 前缀支持精确选择多个工具（逗号分隔）
//! - 兜底防护：CHAT/QUESTION 意图 或 chat 模式下禁止写操作

use super::registry::{ToolDef, ToolRegistry};
use serde_json::json;

/// 延迟工具搜索索引项。
///
/// `description` 和 `search_hint` 不直接暴露在首轮 prompt 中，只作为
/// DiscoverTools 的内部召回语义索引使用。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DeferredToolSearchEntry {
    pub name: String,
    pub description: String,
    pub search_hint: String,
}

/// 获取核心工具（始终带完整 schema，永不延迟）
/// 从 ToolRegistry 查询所有 should_defer == false 的工具
pub fn get_core_tool_definitions() -> Vec<serde_json::Value> {
    ToolRegistry::global().get_core_definitions()
}

/// 获取延迟工具列表 (名称, 简述)，按意图筛选
/// 从 ToolRegistry 查询所有 should_defer == true 且符合意图的工具
pub fn get_deferred_tool_list(intent: &str) -> Vec<(String, String)> {
    ToolRegistry::global()
        .get_deferred_list(intent)
        .into_iter()
        .map(|(name, desc)| (name.to_string(), desc.to_string()))
        .collect()
}

/// 获取延迟工具搜索索引，按意图筛选
pub fn get_deferred_tool_search_entries(intent: &str) -> Vec<DeferredToolSearchEntry> {
    ToolRegistry::global()
        .get_deferred_search_entries(intent)
        .into_iter()
        .map(|(name, description, search_hint)| DeferredToolSearchEntry {
            name: name.to_string(),
            description: description.to_string(),
            search_hint: search_hint.to_string(),
        })
        .collect()
}

/// 按名称获取一个延迟工具的完整 JSON Schema
pub fn get_deferred_tool_full_schema(name: &str) -> Option<serde_json::Value> {
    ToolRegistry::global().get_deferred_full_schema(name)
}

/// 关键词搜索延迟工具，返回匹配的工具名列表
pub fn search_deferred_tools(
    query: &str,
    deferred_list: &[DeferredToolSearchEntry],
    max_results: usize,
) -> Vec<String> {
    let query_lower = query.to_lowercase().trim().to_string();

    // `select:` 前缀：精确选择指定工具（逗号分隔）
    if let Some(select_query) = query_lower.strip_prefix("select:") {
        let requested: Vec<&str> = select_query
            .split(',')
            .map(|s| s.trim())
            .filter(|s| !s.is_empty())
            .collect();
        let mut found = Vec::new();
        for name in requested {
            // 不区分大小写匹配
            if let Some(entry) = deferred_list
                .iter()
                .find(|entry| entry.name.to_lowercase() == name.to_lowercase())
            {
                if !found.contains(&entry.name) {
                    found.push(entry.name.clone());
                }
            }
        }
        return found;
    }

    // 关键词搜索：按评分排序（名称精确匹配 12 > 名称包含 5 > 搜索提示包含 3 > 描述包含 2）
    let terms: Vec<&str> = query_lower.split_whitespace().collect();
    let mut scored: Vec<(String, usize)> = deferred_list
        .iter()
        .map(|entry| {
            let name_lower = entry.name.to_lowercase();
            let desc_lower = entry.description.to_lowercase();
            let hint_lower = entry.search_hint.to_lowercase();
            let mut score = 0usize;
            for term in &terms {
                if name_lower == *term {
                    score += 12;
                } else if name_lower.contains(term) {
                    score += 5;
                }
                if hint_lower.contains(term) {
                    score += 3;
                }
                if desc_lower.contains(term) {
                    score += 2;
                }
            }
            (entry.name.clone(), score)
        })
        .filter(|(_, s)| *s > 0)
        .collect();

    scored.sort_by(|a, b| b.1.cmp(&a.1));
    scored.truncate(max_results);
    scored.into_iter().map(|(name, _)| name).collect()
}

/// DiscoverTools 工具的处理函数（纯搜索指引，不触发激活）
pub async fn handle_search_tools(input: &serde_json::Value, intent: &str, session_id: &str) -> String {
    let query = input["query"].as_str().unwrap_or("");
    let max_results = input["max_results"].as_u64().unwrap_or(5).clamp(1, 20) as usize;

    let deferred = get_deferred_tool_search_entries(intent);

    if deferred.is_empty() {
        return "当前意图下没有可用的延迟加载工具。".to_string();
    }

    let matches = search_deferred_tools(query, &deferred, max_results);

    if matches.is_empty() {
        let all_names: Vec<String> = deferred.iter().map(|entry| entry.name.clone()).collect();
        return format!(
            "未找到匹配 '{}' 的工具。\n\n当前可用的延迟加载工具: {}\n\n请使用 'select:工具名' 精确选择，或使用关键词重新搜索。",
            query,
            all_names.join(", ")
        );
    }

    // 记录 DiscoverTools 命中的工具名，用于后续 ExecuteTool 的协议遵从度诊断
    super::tool_call_logger::tool_call_logger().record_search_tools(session_id, matches.clone());

    let mut result = format!("匹配到 {} 个工具，完整参数定义如下：\n", matches.len());

    for name in &matches {
        if let Some(schema) = get_deferred_tool_full_schema(name) {
            let json_str = serde_json::to_string_pretty(&schema).unwrap_or_default();
            result.push_str(&format!("\n工具: {}\n```json\n{}\n```\n", name, json_str));
        }
    }

    result.push_str(&format!(
        "\n需要使用以上工具时，请调用 ExecuteTool 执行。示例: ExecuteTool(name=\"{}\", args={{...}})",
        matches.first().unwrap_or(&String::new())
    ));

    result
}

/// ExecuteTool 工具的处理函数（代理执行延迟工具，含兜底防护）
/// 返回 `ToolCallResult` 而非 `String`，以保留内部工具的 `break_loop`/`is_error` 标志
pub async fn handle_execute_tool(
    app: &tauri::AppHandle,
    input: &serde_json::Value,
    session_id: &str,
    agent_type: &str,
    intent: &str,
    work_mode: &str,
) -> super::ToolCallResult {
    let logger = super::tool_call_logger::tool_call_logger();

    let name = match input["name"].as_str() {
        Some(n) => n,
        None => {
            logger.log_deferred_call(
                session_id, agent_type, "", input, intent, work_mode,
                super::tool_call_logger::ToolCallStatus::Error,
                Some(super::tool_call_logger::ErrorType::MissingParam),
                Some("缺少必填参数 'name'".to_string()),
                false,
            );
            return super::ToolCallResult::error("缺少必填参数 'name'（要执行的工具名称）。".to_string());
        }
    };
    let args = input.get("args").cloned().unwrap_or(serde_json::json!({}));

    // 诊断：本 session 内是否 DiscoverTools 过该工具
    let searched_before = logger.has_searched(session_id, name);

    // 校验：工具是否存在
    let tool_def = match ToolRegistry::global().get(name) {
        Some(def) => def,
        None => {
            logger.log_deferred_call(
                session_id, agent_type, name, &input, intent, work_mode,
                super::tool_call_logger::ToolCallStatus::Error,
                Some(super::tool_call_logger::ErrorType::ToolNotFound),
                Some(format!("工具 '{}' 不存在", name)),
                searched_before,
            );
            return super::ToolCallResult::error(format!("工具 '{}' 不存在。请使用 DiscoverTools 查询可用工具。", name));
        }
    };

    // 校验：是否为延迟工具
    if !tool_def.should_defer {
        logger.log_deferred_call(
            session_id, agent_type, name, &input, intent, work_mode,
            super::tool_call_logger::ToolCallStatus::Error,
            Some(super::tool_call_logger::ErrorType::NotDeferred),
            Some(format!("工具 '{}' 是核心工具", name)),
            searched_before,
        );
        return super::ToolCallResult::error(format!(
            "工具 '{}' 是核心工具，请直接调用，无需通过 ExecuteTool。",
            name
        ));
    }

    // 校验：当前意图是否允许
    if !super::registry::ToolRegistry::is_available_for_intent(tool_def, intent) {
        let available: Vec<String> = get_deferred_tool_list(intent)
            .into_iter()
            .map(|(n, _)| n)
            .collect();
        logger.log_deferred_call(
            session_id, agent_type, name, &input, intent, work_mode,
            super::tool_call_logger::ToolCallStatus::Error,
            Some(super::tool_call_logger::ErrorType::IntentBlocked),
            Some(format!("工具 '{}' 在 {} 意图下不可用", name, intent)),
            searched_before,
        );
        return super::ToolCallResult::error(format!(
            "工具 '{}' 在当前 {} 意图下不可用。\n当前可用的延迟工具: {}",
            name,
            intent,
            available.join(", ")
        ));
    }

    // 兜底防护：写操作工具在 CHAT/QUESTION 意图或聊天模式下被拦截
    if crate::core::tools::should_block_write_tool(name, intent, work_mode) {
        let msg = if work_mode == "chat" {
            "聊天模式下只能使用只读工具，请切换到编辑模式后再试。"
        } else {
            "当前意图下只能使用只读工具。"
        };
        logger.log_deferred_call(
            session_id, agent_type, name, &input, intent, work_mode,
            super::tool_call_logger::ToolCallStatus::Blocked,
            Some(super::tool_call_logger::ErrorType::ModeBlocked),
            Some(msg.to_string()),
            searched_before,
        );
        return super::ToolCallResult::error(format!("工具 '{}' 在当前状态下不可用。{}", name, msg));
    }

    // 执行工具（调用 dispatch_tool_call 避免递归）
    let result = crate::core::tools::dispatch_tool_call(app, name, &args, session_id, intent, work_mode).await;

    // 记录执行结果（通过 ToolCallResult.is_error 结构化判断，不再扫描字符串关键词）
    if result.is_error {
        logger.log_deferred_call(
            session_id, agent_type, name, &input, intent, work_mode,
            super::tool_call_logger::ToolCallStatus::Error,
            Some(super::tool_call_logger::ErrorType::ExecutionFailed),
            Some(result.output.chars().take(500).collect()),
            searched_before,
        );
    } else {
        logger.log_deferred_call(
            session_id, agent_type, name, &input, intent, work_mode,
            super::tool_call_logger::ToolCallStatus::Ok,
            None, None,
            searched_before,
        );
    }

    // 直接返回 ToolCallResult，保留 break_loop/is_error 标志
    // （如 ProposePlan 的 break_loop=true 需要传递到 pipeline 主循环）
    result
}

// --- 工具注册 ---
crate::define_tools! {
    pub fn register_tools(registry) {
        ToolDef {
            name: "DiscoverTools",
            description: "搜索延迟加载工具，返回完整参数定义和用法示例",
            search_hint: "search tools find discover lookup",
            category: "",
            schema: json!({
                "name": "DiscoverTools",
                "description": "搜索延迟加载工具，返回完整参数定义和用法示例。不确定有哪些可用工具时，先调用 GetToolCatalog 获取目录。支持 'select:ToolName1,ToolName2' 精确选择。获取 schema 后使用 ExecuteTool 执行。",
                "input_schema": {
                    "type": "object",
                    "properties": {
                        "query": {
                            "type": "string",
                            "description": "查询字符串。使用 'select:ToolName1,ToolName2' 精确选择指定工具，或使用空格分隔的关键词搜索（如 'file read'、'git'、'task create'）"
                        },
                        "max_results": {
                            "type": "integer",
                            "description": "最大返回结果数，默认 5"
                        }
                    },
                    "required": ["query"]
                }
            }),
            should_defer: false,
            is_read_only: true,
            is_concurrency_safe: true,
            is_enabled: true,
        },
        ToolDef {
            name: "ExecuteTool",
            description: "代理执行延迟加载工具（先用 DiscoverTools 获取参数定义，再用此工具执行）",
            search_hint: "run execute deferred tool invoke call",
            category: "",
            schema: json!({
                "name": "ExecuteTool",
                "description": "代理执行延迟加载工具。先用 GetToolCatalog 获取可用工具目录，再用 DiscoverTools 了解工具参数，最后用此工具执行。name 传工具名，args 传该工具的参数对象。",
                "input_schema": {
                    "type": "object",
                    "properties": {
                        "name": {
                            "type": "string",
                            "description": "要执行的延迟工具名称（如 'ReadFile'、'RunCommand'）"
                        },
                        "args": {
                            "type": "object",
                            "description": "该工具的输入参数（JSON 对象），具体参数请先通过 DiscoverTools 查询"
                        }
                    },
                    "required": ["name"]
                }
            }),
            should_defer: false,
            is_read_only: false,
            is_concurrency_safe: false,
            is_enabled: true,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_search_select_exact() {
        let deferred = get_deferred_tool_search_entries("PROJECT_ACTION");
        let result = search_deferred_tools("select:ReadFileSkeleton,WriteFile", &deferred, 5);
        assert_eq!(result, vec!["ReadFileSkeleton", "WriteFile"]);
    }

    #[test]
    fn test_search_select_case_insensitive() {
        let deferred = get_deferred_tool_search_entries("PROJECT_ACTION");
        let result = search_deferred_tools("select:readfileskeleton", &deferred, 5);
        assert_eq!(result, vec!["ReadFileSkeleton"]);
    }

    #[test]
    fn test_search_keyword() {
        let deferred = get_deferred_tool_search_entries("PROJECT_ACTION");
        let result = search_deferred_tools("git command", &deferred, 5);
        // RunGitCommand should score highest
        assert!(result.contains(&"RunGitCommand".to_string()));
    }

    #[test]
    fn test_search_hint_matches_dev_server() {
        let deferred = get_deferred_tool_search_entries("PROJECT_ACTION");
        let result = search_deferred_tools("dev server", &deferred, 5);
        assert_eq!(result.first(), Some(&"StartBackgroundCommand".to_string()));
    }

    #[test]
    fn test_search_description_still_matches_chinese_query() {
        let deferred = get_deferred_tool_search_entries("PROJECT_ACTION");
        let result = search_deferred_tools("函数签名", &deferred, 5);
        assert!(result.contains(&"ReadFileSkeleton".to_string()));
    }

    #[test]
    fn test_search_no_match() {
        let deferred = get_deferred_tool_search_entries("PROJECT_ACTION");
        let result = search_deferred_tools("nonexistent_xyz_tool", &deferred, 5);
        assert!(result.is_empty());
    }

    #[test]
    fn test_deferred_list_subagent_excludes_task() {
        let deferred = get_deferred_tool_list("SUBAGENT");
        let names: Vec<&str> = deferred.iter().map(|(n, _)| n.as_str()).collect();
        assert!(!names.contains(&"RunSubagent"));
        assert!(!names.contains(&"ConsolidateMemory"));
        assert!(!names.contains(&"CompactConversation"));
    }

    #[test]
    fn test_deferred_list_chat_empty() {
        let deferred = get_deferred_tool_list("CHAT");
        assert!(deferred.is_empty());
    }

    #[test]
    fn test_get_full_schema_returns_valid_json() {
        let schema = get_deferred_tool_full_schema("WriteFile");
        assert!(schema.is_some());
        let s = schema.unwrap();
        assert_eq!(s["name"], "WriteFile");
        assert!(s["input_schema"]["properties"]["path"].is_object());
    }

    #[test]
    fn test_handle_discover_tools_does_not_return_xml_function_wrappers() {
        let output = tauri::async_runtime::block_on(handle_search_tools(
            &json!({ "query": "select:EditFile" }),
            "PROJECT_ACTION",
            "test-session",
        ));

        assert!(output.contains("工具: EditFile"));
        assert!(output.contains("```json"));
        assert!(!output.contains("<function>"));
        assert!(!output.contains("</function>"));
    }

    #[test]
    fn test_core_tools_include_DiscoverTools() {
        let core = get_core_tool_definitions();
        let names: Vec<&str> = core.iter().map(|t| t["name"].as_str().unwrap()).collect();
        assert!(names.contains(&"DiscoverTools"));
        assert!(names.contains(&"LoadSkill"));
        assert!(names.contains(&"GetToolCatalog"));
    }

    #[test]
    fn test_get_deferred_tools_list_returns_grouped_names() {
        let groups = ToolRegistry::global().get_deferred_by_category("PROJECT_ACTION");
        assert!(!groups.is_empty());
        // 验证包含写操作工具
        let all_names: Vec<&str> = groups.iter().flat_map(|(_, names)| names.iter().copied()).collect();
        assert!(all_names.contains(&"WriteFile"));
        assert!(all_names.contains(&"RunCommand"));
    }
}
