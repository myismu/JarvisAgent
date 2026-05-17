//! # tool_search.rs — 渐进式工具披露模块
//!
//! 核心工具始终携带完整 schema，延迟工具仅先暴露名称，
//! LLM 通过 `SearchTools` 按需获取完整参数定义后再调用。
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
//! - `get_deferred_tools_context()`: 生成延迟工具名称列表（注入 system prompt）
//! - `handle_SearchTools()`: SearchTools 工具的处理函数
//!
//! ## 依赖
//! - Internal: `registry::ToolRegistry`
//! - External: `serde_json`
//!
//! ## 约束
//! - 首轮上下文只披露延迟工具名称，避免把所有简述灌入 prompt
//! - 搜索评分：精确名称匹配 12 分，名称包含 5 分，搜索提示包含 3 分，描述包含 2 分
//! - `select:` 前缀支持精确选择多个工具（逗号分隔）

use super::registry::{ToolDef, ToolRegistry};
use serde_json::json;

/// 延迟工具搜索索引项。
///
/// `description` 和 `search_hint` 不直接暴露在首轮 prompt 中，只作为
/// SearchTools 的内部召回语义索引使用。
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

/// 生成延迟工具名称列表上下文（注入到 system prompt 区域的用户消息中）
pub fn get_deferred_tools_context(intent: &str) -> String {
    let groups = ToolRegistry::global().get_deferred_by_category(intent);

    if groups.is_empty() {
        return String::new();
    }

    let mut out = String::from(
        "\n\n【延迟加载工具】（使用 SearchTools 获取完整参数定义后才能调用）:\n",
    );
    for (category, names) in &groups {
        let name_list: Vec<String> = names.iter().map(|n| format!("`{}`", n)).collect();
        out.push_str(&format!("- **{}**: {}\n", category, name_list.join(", ")));
    }
    out
}

/// 生成延迟工具上下文（紧凑格式，用于 user message 参考索引）
pub fn get_deferred_tools_context_compact(intent: &str) -> String {
    let groups = ToolRegistry::global().get_deferred_by_category(intent);
    if groups.is_empty() {
        return String::new();
    }
    // 核心工具前置
    let core = ToolRegistry::global().get_core_definitions();
    let core_names: Vec<&str> = core
        .iter()
        .filter_map(|schema| schema["name"].as_str())
        .filter(|n| *n != "SearchTools")
        .collect();
    let mut out = String::new();
    if !core_names.is_empty() {
        out.push_str(&format!("  · 核心: {}\n", core_names.join(", ")));
    }
    for (category, names) in &groups {
        out.push_str(&format!("  · {}: {}\n", category, names.join(", ")));
    }
    out
}

/// SearchTools 工具的处理函数
pub async fn handle_search_tools(input: &serde_json::Value, intent: &str) -> String {
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

    let mut result = format!("匹配到 {} 个工具，完整参数定义如下：\n", matches.len());

    for name in &matches {
        if let Some(schema) = get_deferred_tool_full_schema(name) {
            let json_str = serde_json::to_string_pretty(&schema).unwrap_or_default();
            result.push_str(&format!("\n工具: {}\n```json\n{}\n```\n", name, json_str));
        }
    }

    result
        .push_str("\n以上工具已经激活；需要使用时请发起结构化工具调用，不要把工具调用写在正文里。");

    result
}

// --- 工具注册 ---
crate::define_tools! {
    pub fn register_tools(registry) {
        ToolDef {
            name: "SearchTools",
            description: "搜索并获取延迟加载工具的完整参数定义",
            search_hint: "search tools find discover lookup",
            category: "",
            schema: json!({
                "name": "SearchTools",
                "description": "搜索并获取延迟加载工具的完整参数定义。代码搜索用 'FindSymbol FindReferences CodeSearch SearchRepo'，文件操作用 'ReadFile WriteFile EditFile'，命令执行用 'RunCommand RunGitCommand'，任务管理用 'CreateTask UpdateTask'，Agent 调度用 'RunSubagent ProposePlan'。支持 'select:ToolName1,ToolName2' 精确选择。",
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
        let deferred = get_deferred_tool_search_entries("PROJECT_ACTION");
        let result = search_deferred_tools("select:ReadFile,WriteFile", &deferred, 5);
        assert_eq!(result, vec!["ReadFile", "WriteFile"]);
    }

    #[test]
    fn test_search_select_case_insensitive() {
        let deferred = get_deferred_tool_search_entries("PROJECT_ACTION");
        let result = search_deferred_tools("select:readfile", &deferred, 5);
        assert_eq!(result, vec!["ReadFile"]);
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
    fn test_deferred_context_exposes_names_without_descriptions() {
        let context = get_deferred_tools_context("PROJECT_ACTION");
        assert!(context.contains("ReadFile"));
        assert!(context.contains("StartBackgroundCommand"));
        assert!(!context.contains("读取文件内容"));
        assert!(!context.contains("在后台执行长时间运行的命令"));
    }

    #[test]
    fn test_get_full_schema_returns_valid_json() {
        let schema = get_deferred_tool_full_schema("ReadFile");
        assert!(schema.is_some());
        let s = schema.unwrap();
        assert_eq!(s["name"], "ReadFile");
        assert!(s["input_schema"]["properties"]["path"].is_object());
    }

    #[test]
    fn test_handle_search_tools_does_not_return_xml_function_wrappers() {
        let output = tauri::async_runtime::block_on(handle_search_tools(
            &json!({ "query": "select:ReadFile" }),
            "PROJECT_ACTION",
        ));

        assert!(output.contains("工具: ReadFile"));
        assert!(output.contains("```json"));
        assert!(!output.contains("<function>"));
        assert!(!output.contains("</function>"));
    }

    #[test]
    fn test_core_tools_include_SearchTools() {
        let core = get_core_tool_definitions();
        let names: Vec<&str> = core.iter().map(|t| t["name"].as_str().unwrap()).collect();
        assert!(names.contains(&"SearchTools"));
        assert!(names.contains(&"GetSystemInfo"));
        assert!(names.contains(&"LoadSkill"));
    }
}
