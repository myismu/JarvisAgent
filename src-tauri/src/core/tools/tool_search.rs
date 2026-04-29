//! # tool_search.rs — 渐进式工具披露模块
//!
//! 核心工具始终携带完整 schema，延迟工具仅先暴露名称+简述，
//! LLM 通过 `search_tools` 按需获取完整参数定义后再调用。
//!
//! 所有工具的 schema 和元数据已迁移到各模块的 `define_tools!` 注册，
//! 本模块从 `ToolRegistry` 统一查询，不再维护硬编码的 JSON Schema。
//!
//! ## 关键导出
//! - `get_core_tool_definitions()`: 获取核心工具（始终带完整 schema）
//! - `get_deferred_tool_list()`: 获取延迟工具列表（名称+简述）
//! - `get_deferred_tool_full_schema()`: 按名称获取延迟工具的完整 Schema
//! - `search_deferred_tools()`: 关键词搜索延迟工具（支持 `select:` 精确选择）
//! - `get_deferred_tools_context()`: 生成延迟工具名称列表（注入 system prompt）
//! - `handle_search_tools()`: search_tools 工具的处理函数
//!
//! ## 依赖
//! - Internal: `registry::ToolRegistry`
//! - External: `serde_json`
//!
//! ## 约束
//! - 搜索评分：精确名称匹配 12 分，名称包含 5 分，描述包含 2 分
//! - `select:` 前缀支持精确选择多个工具（逗号分隔）

use serde_json::json;
use super::registry::{ToolDef, ToolRegistry};

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

/// 按名称获取一个延迟工具的完整 JSON Schema
pub fn get_deferred_tool_full_schema(name: &str) -> Option<serde_json::Value> {
    ToolRegistry::global().get_deferred_full_schema(name)
}

/// 关键词搜索延迟工具，返回匹配的工具名列表
pub fn search_deferred_tools(
    query: &str,
    deferred_list: &[(String, String)],
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
            if let Some((exact_name, _)) = deferred_list
                .iter()
                .find(|(n, _)| n.to_lowercase() == name.to_lowercase())
            {
                if !found.contains(exact_name) {
                    found.push(exact_name.clone());
                }
            }
        }
        return found;
    }

    // 关键词搜索：按评分排序（名称精确匹配 12 > 名称包含 5 > 描述包含 2）
    let terms: Vec<&str> = query_lower.split_whitespace().collect();
    let mut scored: Vec<(String, usize)> = deferred_list
        .iter()
        .map(|(name, desc)| {
            let name_lower = name.to_lowercase();
            let desc_lower = desc.to_lowercase();
            let mut score = 0usize;
            for term in &terms {
                if name_lower == *term {
                    score += 12;
                } else if name_lower.contains(term) {
                    score += 5;
                }
                if desc_lower.contains(term) {
                    score += 2;
                }
            }
            (name.clone(), score)
        })
        .filter(|(_, s)| *s > 0)
        .collect();

    scored.sort_by(|a, b| b.1.cmp(&a.1));
    scored.truncate(max_results);
    scored.into_iter().map(|(name, _)| name).collect()
}

/// 生成延迟工具名称列表上下文（注入到 system prompt 区域的用户消息中）
pub fn get_deferred_tools_context(intent: &str) -> String {
    let deferred = get_deferred_tool_list(intent);
    if deferred.is_empty() {
        return String::new();
    }

    let names: Vec<String> = deferred
        .iter()
        .map(|(name, desc)| format!("- **{}**: {}", name, desc))
        .collect();

    format!(
        "\n\n【延迟加载工具】（使用 search_tools 获取完整参数定义后才能调用）:\n{}\n",
        names.join("\n")
    )
}

/// search_tools 工具的处理函数
pub async fn handle_search_tools(
    input: &serde_json::Value,
    intent: &str,
) -> String {
    let query = input["query"].as_str().unwrap_or("");
    let max_results = input["max_results"]
        .as_u64()
        .unwrap_or(5)
        .clamp(1, 20) as usize;

    let deferred = get_deferred_tool_list(intent);

    if deferred.is_empty() {
        return "当前意图下没有可用的延迟加载工具。".to_string();
    }

    let matches = search_deferred_tools(query, &deferred, max_results);

    if matches.is_empty() {
        let all_names: Vec<String> = deferred.iter().map(|(n, _)| n.clone()).collect();
        return format!(
            "未找到匹配 '{}' 的工具。\n\n当前可用的延迟加载工具: {}\n\n请使用 'select:工具名' 精确选择，或使用关键词重新搜索。",
            query,
            all_names.join(", ")
        );
    }

    let mut result = format!("匹配到 {} 个工具，完整参数定义如下：\n", matches.len());

    for name in &matches {
        if let Some(schema) = get_deferred_tool_full_schema(name) {
            let json_str = serde_json::to_string(&schema).unwrap_or_default();
            result.push_str(&format!("\n<function>{}</function>", json_str));
        }
    }

    result.push_str("\n\n现在可以直接调用以上工具。");

    result
}

// --- 工具注册 ---
crate::define_tools! {
    pub fn register_tools(registry) {
        ToolDef {
            name: "search_tools",
            description: "搜索并获取延迟加载工具的完整参数定义",
            search_hint: "search tools find discover lookup",
            schema: json!({
                "name": "search_tools",
                "description": "搜索并获取延迟加载工具的完整参数定义。在使用任何名称已知但参数未知的工具前，必须先调用此工具获取其完整 JSON Schema。支持 'select:ToolName1,ToolName2' 精确选择，或关键词搜索（如 'file read write'）。",
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
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_search_select_exact() {
        let deferred = get_deferred_tool_list("PROJECT_ACTION");
        let result = search_deferred_tools("select:read_file,write_file", &deferred, 5);
        assert_eq!(result, vec!["read_file", "write_file"]);
    }

    #[test]
    fn test_search_select_case_insensitive() {
        let deferred = get_deferred_tool_list("PROJECT_ACTION");
        let result = search_deferred_tools("select:Read_File", &deferred, 5);
        assert_eq!(result, vec!["read_file"]);
    }

    #[test]
    fn test_search_keyword() {
        let deferred = get_deferred_tool_list("PROJECT_ACTION");
        let result = search_deferred_tools("git command", &deferred, 5);
        // git_command should score highest
        assert!(result.contains(&"git_command".to_string()));
    }

    #[test]
    fn test_search_no_match() {
        let deferred = get_deferred_tool_list("PROJECT_ACTION");
        let result = search_deferred_tools("nonexistent_xyz_tool", &deferred, 5);
        assert!(result.is_empty());
    }

    #[test]
    fn test_deferred_list_subagent_excludes_task() {
        let deferred = get_deferred_tool_list("SUBAGENT");
        let names: Vec<&str> = deferred.iter().map(|(n, _)| n.as_str()).collect();
        assert!(!names.contains(&"task"));
        assert!(!names.contains(&"dream"));
        assert!(!names.contains(&"compact"));
    }

    #[test]
    fn test_deferred_list_chat_empty() {
        let deferred = get_deferred_tool_list("CHAT");
        assert!(deferred.is_empty());
    }

    #[test]
    fn test_get_full_schema_returns_valid_json() {
        let schema = get_deferred_tool_full_schema("read_file");
        assert!(schema.is_some());
        let s = schema.unwrap();
        assert_eq!(s["name"], "read_file");
        assert!(s["input_schema"]["properties"]["path"].is_object());
    }

    #[test]
    fn test_core_tools_include_search_tools() {
        let core = get_core_tool_definitions();
        let names: Vec<&str> = core.iter().map(|t| t["name"].as_str().unwrap()).collect();
        assert!(names.contains(&"search_tools"));
        assert!(names.contains(&"get_system_info"));
        assert!(names.contains(&"load_skill"));
    }
}
