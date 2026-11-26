//! # core/mcp/catalog.rs — MCP 动态工具目录与唯一可用性判定口径
//!
//! 与静态 `ToolRegistry` 并列，向 `GetToolCatalog`、`DiscoverTools`、`ExecuteTool`
//! 提供统一的 MCP 工具可见性、搜索索引与 Schema 查询。
//!
//! ## 核心不变量
//! - **单一真相**：目录过滤、关键词搜索、运行期执行校验全部经过 `is_mcp_tool_available()`，
//!   绝不允许"目录里看不见、调用时却放行"或反之。
//! - **规划模式与子代理一律禁用**：`work_mode == "plan"` 或 `intent` 属于
//!   `SUBAGENT` / `CHAT` / `QUESTION` 时，MCP 工具不可见且不可调用。
//! - **懒启动兼容**：处于 `Ready` 或"处于 `Idle` 且有本地能力缓存"的已启用服务均出现在目录中，
//!   真正执行时由 `McpManager::ensure_ready()` 透明拉起。

use crate::core::mcp::config::{CachedToolInfo, McpCacheFile, McpServerConfig, McpServersFile};
use crate::core::mcp::manager::{McpManager, McpServerStatus};
use crate::core::mcp::naming::McpToolNameMap;
use crate::core::tools::framework::tool_search::DeferredToolSearchEntry;
use std::collections::HashMap;

/// 解析后的 MCP 工具完整描述符
#[derive(Debug, Clone)]
pub struct McpToolDescriptor {
    /// 暴露给大模型的命名空间工具名（如 `mcp__fs__read_file`）
    pub qualified_name: String,
    pub server_name: String,
    pub original_tool_name: String,
    pub description: String,
    pub input_schema: serde_json::Value,
    pub trusted: bool,
}

/// 第一层判定：当前工作模式与意图是否允许使用任何 MCP 工具（含资源工具）
///
/// 规则：
/// - 规划模式（`plan`）：一律禁用；
/// - `SUBAGENT` / `CHAT` / `QUESTION`：一律禁用；
/// - 仅在主 Agent 编辑模式（`edit` + `PROJECT_ACTION`）下放行。
pub fn is_mcp_allowed_in_context(intent: &str, work_mode: &str) -> bool {
    if work_mode == "plan" || work_mode == "chat" {
        return false;
    }
    !matches!(intent, "SUBAGENT" | "CHAT" | "QUESTION")
}

/// 第二层判定：单个服务的某个工具在当前状态下是否可用
pub fn is_server_tool_available(
    cfg: &McpServerConfig,
    status: McpServerStatus,
    has_cached_tools: bool,
    original_tool_name: &str,
    intent: &str,
    work_mode: &str,
) -> bool {
    if !is_mcp_allowed_in_context(intent, work_mode) {
        return false;
    }
    if !cfg.enabled {
        return false;
    }
    if !cfg.is_tool_enabled(original_tool_name) {
        return false;
    }
    match status {
        McpServerStatus::Ready => true,
        // 懒启动态：只要本地有能力缓存就对模型可见，真正执行时透明拉起
        McpServerStatus::Idle | McpServerStatus::Starting => has_cached_tools,
        McpServerStatus::Disabled | McpServerStatus::Failed | McpServerStatus::Stopped => false,
    }
}

/// 从快照收集当前上下文下所有可用的 MCP 工具描述符（纯函数，便于测试）
pub fn collect_available_mcp_tools(
    configs: &McpServersFile,
    cache: &McpCacheFile,
    statuses: &HashMap<String, McpServerStatus>,
    intent: &str,
    work_mode: &str,
) -> Vec<McpToolDescriptor> {
    if !is_mcp_allowed_in_context(intent, work_mode) {
        return Vec::new();
    }

    let mut name_map = McpToolNameMap::new();
    let mut result = Vec::new();

    for (server_name, cfg) in &configs.mcp_servers {
        let Some(caps) = cache.servers.get(server_name) else {
            continue;
        };
        let status = statuses
            .get(server_name)
            .copied()
            .unwrap_or(if cfg.enabled {
                McpServerStatus::Idle
            } else {
                McpServerStatus::Disabled
            });
        let has_cached = !caps.tools.is_empty();

        for tool in &caps.tools {
            let qualified_name = name_map.register(server_name, &tool.name);
            if is_server_tool_available(cfg, status, has_cached, &tool.name, intent, work_mode) {
                result.push(McpToolDescriptor {
                    qualified_name,
                    server_name: server_name.clone(),
                    original_tool_name: tool.name.clone(),
                    description: tool.description.clone(),
                    input_schema: tool.input_schema.clone(),
                    trusted: cfg.trusted,
                });
            }
        }
    }

    result
}

/// 找出"已启用但尚无工具缓存（或处于 Failed）"的服务名（纯函数，供目录提示与测试复用）
pub fn pending_mcp_servers(
    configs: &McpServersFile,
    cache: &McpCacheFile,
    statuses: &HashMap<String, McpServerStatus>,
) -> Vec<String> {
    let mut pending = Vec::new();
    for (server_name, cfg) in &configs.mcp_servers {
        if !cfg.enabled {
            continue;
        }
        let status = statuses
            .get(server_name)
            .copied()
            .unwrap_or(McpServerStatus::Idle);
        let has_cached = cache
            .servers
            .get(server_name)
            .is_some_and(|c| !c.tools.is_empty());
        if !has_cached || status == McpServerStatus::Failed {
            pending.push(server_name.clone());
        }
    }
    pending
}

/// 格式化 `GetToolCatalog` 中的"【MCP 工具】"分组文本
///
/// 同时展示两类信息：
/// 1. 已有能力缓存的可用工具（按服务分组，可直接 DiscoverTools + ExecuteTool）；
/// 2. 已启用但尚未首次连接（无缓存）或连接已断开（Failed）的服务，引导按需调用 ConnectMcpServer。
pub async fn format_mcp_catalog_section(
    manager: &McpManager,
    intent: &str,
    work_mode: &str,
) -> Option<String> {
    if !is_mcp_allowed_in_context(intent, work_mode) {
        return None;
    }

    let (configs, cache, statuses) = manager.snapshot_configs_and_cache().await;
    let tools = collect_available_mcp_tools(&configs, &cache, &statuses, intent, work_mode);

    // 收集已启用但尚无缓存或处于 Failed 状态的服务（需要首次连接/重连）
    let pending_servers = pending_mcp_servers(&configs, &cache, &statuses);

    if tools.is_empty() && pending_servers.is_empty() {
        return None;
    }

    let mut out = String::new();
    if !tools.is_empty() {
        // 按服务名分组保持确定顺序
        let mut groups: Vec<(String, Vec<String>)> = Vec::new();
        for t in tools {
            if let Some((_, list)) = groups.iter_mut().find(|(s, _)| s == &t.server_name) {
                list.push(t.qualified_name);
            } else {
                groups.push((t.server_name, vec![t.qualified_name]));
            }
        }

        out.push_str("\n【MCP 外部工具】（需通过 DiscoverTools + ExecuteTool 执行，服务待机时会自动懒启动）:\n");
        for (srv, names) in groups {
            out.push_str(&format!("- {}: {}\n", srv, names.join(", ")));
        }
    }

    if !pending_servers.is_empty() {
        out.push_str(&format!(
            "\n【待连接/待重连的 MCP 服务】（已启用但尚无工具缓存或已断开，如需使用请先经 ExecuteTool 调用 ConnectMcpServer）:\n- {}\n",
            pending_servers.join(", ")
        ));
    }

    Some(out)
}

/// 获取当前上下文下可用于 `DiscoverTools` 搜索的 MCP 工具索引项
pub async fn list_available_mcp_search_entries(
    manager: &McpManager,
    intent: &str,
    work_mode: &str,
) -> Vec<DeferredToolSearchEntry> {
    let (configs, cache, statuses) = manager.snapshot_configs_and_cache().await;
    collect_available_mcp_tools(&configs, &cache, &statuses, intent, work_mode)
        .into_iter()
        .map(|t| DeferredToolSearchEntry {
            name: t.qualified_name,
            description: t.description,
            search_hint: format!("mcp {} {}", t.server_name, t.original_tool_name),
        })
        .collect()
}

/// 按命名空间工具名查询完整 JSON Schema（供 `DiscoverTools` 展示）
pub async fn resolve_mcp_tool_schema(
    manager: &McpManager,
    qualified_name: &str,
    intent: &str,
    work_mode: &str,
) -> Option<serde_json::Value> {
    let (configs, cache, statuses) = manager.snapshot_configs_and_cache().await;
    let desc = collect_available_mcp_tools(&configs, &cache, &statuses, intent, work_mode)
        .into_iter()
        .find(|t| t.qualified_name == qualified_name)?;

    Some(build_anthropic_schema_for_mcp(&desc))
}

/// 按命名空间工具名解析可用的目标工具描述符（供 `ExecuteTool` 运行期校验与路由）
pub async fn resolve_mcp_tool_for_execution(
    manager: &McpManager,
    qualified_name: &str,
    intent: &str,
    work_mode: &str,
) -> Result<McpToolDescriptor, String> {
    if !is_mcp_allowed_in_context(intent, work_mode) {
        if work_mode == "plan" {
            return Err(format!(
                "MCP 工具 '{}' 在规划模式下不可用：规划模式只做代码探索与方案提交，请切回编辑模式后再使用 MCP 工具。",
                qualified_name
            ));
        }
        return Err(format!(
            "MCP 工具 '{}' 在当前 {} 意图下不可用（子代理与纯对话模式禁用 MCP 工具）。",
            qualified_name, intent
        ));
    }

    let (configs, cache, statuses) = manager.snapshot_configs_and_cache().await;
    let tools = collect_available_mcp_tools(&configs, &cache, &statuses, intent, work_mode);
    tools
        .into_iter()
        .find(|t| t.qualified_name == qualified_name)
        .ok_or_else(|| {
            format!(
                "MCP 工具 '{}' 当前不可用（可能对应服务已停用、工具已关闭或尚未建立能力缓存）。",
                qualified_name
            )
        })
}

/// 将 MCP 工具转换为与内置工具同构的 JSON Schema 视图（供 DiscoverTools 返回给模型）
fn build_anthropic_schema_for_mcp(desc: &McpToolDescriptor) -> serde_json::Value {
    let mut input_schema = desc.input_schema.clone();
    if !input_schema.is_object() {
        input_schema = serde_json::json!({ "type": "object", "properties": {} });
    }
    serde_json::json!({
        "name": desc.qualified_name,
        "description": format!("[MCP:{}] {}", desc.server_name, desc.description),
        "input_schema": input_schema,
    })
}

/// 供单测与内部使用的辅助构造器
#[allow(dead_code)]
pub fn sample_cached_tool(name: &str, desc: &str) -> CachedToolInfo {
    CachedToolInfo {
        name: name.to_string(),
        description: desc.to_string(),
        input_schema: serde_json::json!({ "type": "object", "properties": {} }),
        hints: Default::default(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::mcp::config::{CachedServerCapabilities, McpIsolation};
    use std::collections::BTreeMap;

    fn sample_setup() -> (
        McpServersFile,
        McpCacheFile,
        HashMap<String, McpServerStatus>,
    ) {
        let mut configs = McpServersFile::default();
        configs.mcp_servers.insert(
            "fs".to_string(),
            McpServerConfig {
                command: Some("npx".to_string()),
                args: vec![],
                env: BTreeMap::new(),
                cwd: None,
                url: None,
                headers: BTreeMap::new(),
                enabled: true,
                trusted: false,
                auto_start: false,
                isolation: McpIsolation::Global,
                startup_timeout_sec: 60,
                call_timeout_sec: 60,
                idle_timeout_min: 30,
                disabled_tools: vec!["delete_file".to_string()],
            },
        );

        let mut cache = McpCacheFile::default();
        cache.servers.insert(
            "fs".to_string(),
            CachedServerCapabilities {
                tools: vec![
                    sample_cached_tool("read_file", "读取文件"),
                    sample_cached_tool("delete_file", "删除文件"),
                ],
                resources: vec![],
                prompts: vec![],
                updated_at_ms: 1,
            },
        );

        let mut statuses = HashMap::new();
        statuses.insert("fs".to_string(), McpServerStatus::Idle);
        (configs, cache, statuses)
    }

    #[test]
    fn plan_mode_and_subagent_block_all_mcp_tools() {
        let (configs, cache, statuses) = sample_setup();

        // 规划模式：全禁
        let in_plan =
            collect_available_mcp_tools(&configs, &cache, &statuses, "PROJECT_ACTION", "plan");
        assert!(in_plan.is_empty(), "规划模式下绝不允许出现任何 MCP 工具");

        // 子代理：全禁
        let in_subagent =
            collect_available_mcp_tools(&configs, &cache, &statuses, "SUBAGENT", "edit");
        assert!(
            in_subagent.is_empty(),
            "子代理模式下绝不允许出现任何 MCP 工具"
        );

        // CHAT / QUESTION：全禁
        assert!(
            collect_available_mcp_tools(&configs, &cache, &statuses, "CHAT", "edit").is_empty()
        );
        assert!(
            collect_available_mcp_tools(&configs, &cache, &statuses, "QUESTION", "edit").is_empty()
        );
    }

    #[test]
    fn pending_servers_lists_uncached_and_failed_but_not_disabled_or_ready() {
        let (mut configs, mut cache, mut statuses) = sample_setup();

        // fs：已启用 + 有缓存 + Idle → 不算待连接
        assert!(pending_mcp_servers(&configs, &cache, &statuses).is_empty());

        // 新导入、从未连接过（无缓存）→ 待连接
        let mut fresh = configs.mcp_servers["fs"].clone();
        fresh.disabled_tools.clear();
        configs
            .mcp_servers
            .insert("fresh".to_string(), fresh.clone());
        // 已停用且无缓存 → 不应出现
        let mut off = fresh.clone();
        off.enabled = false;
        configs.mcp_servers.insert("off".to_string(), off);
        assert_eq!(
            pending_mcp_servers(&configs, &cache, &statuses),
            vec!["fresh".to_string()]
        );

        // 有缓存但连接断开（Failed）→ 待重连
        statuses.insert("fs".to_string(), McpServerStatus::Failed);
        let pending = pending_mcp_servers(&configs, &cache, &statuses);
        assert!(pending.contains(&"fs".to_string()));
        assert!(pending.contains(&"fresh".to_string()));
        assert!(!pending.contains(&"off".to_string()));

        // 缓存里工具为空也视为尚未建好缓存
        cache.servers.get_mut("fs").unwrap().tools.clear();
        statuses.insert("fs".to_string(), McpServerStatus::Idle);
        assert!(pending_mcp_servers(&configs, &cache, &statuses).contains(&"fs".to_string()));
    }

    #[test]
    fn edit_mode_respects_idle_cache_and_disabled_tools() {
        let (configs, cache, mut statuses) = sample_setup();

        // Idle 且有缓存 → read_file 可见，delete_file 被单工具开关隐藏
        let tools =
            collect_available_mcp_tools(&configs, &cache, &statuses, "PROJECT_ACTION", "edit");
        assert_eq!(tools.len(), 1);
        assert_eq!(tools[0].qualified_name, "mcp__fs__read_file");

        // 服务 Failed → 不可见
        statuses.insert("fs".to_string(), McpServerStatus::Failed);
        assert!(
            collect_available_mcp_tools(&configs, &cache, &statuses, "PROJECT_ACTION", "edit")
                .is_empty()
        );
    }
}
