//! # core/tools/agent_tools/mcp_resource.rs — MCP 资源读取按需工具
//!
//! 为主 Agent 提供两个按需工具（`defer: true`）：
//! - `ListMcpResources`：列出已启用 MCP 服务缓存的资源清单；
//! - `ReadMcpResource`：按服务名与 URI 读取资源内容（文本直接返回，二进制返回元信息占位）。
//!
//! 两者同样遵循规划模式与子代理一律禁用的规则，并走 `ExternalService` 权限判定。

use crate::core::mcp::{is_mcp_allowed_in_context, McpManagerState};
use crate::core::tools::framework::ToolCallResult;
use tauri::Manager;

/// `ListMcpResources` 执行入口
pub async fn list_mcp_resources(
    app: &tauri::AppHandle,
    input: &serde_json::Value,
    intent: &str,
    work_mode: &str,
) -> ToolCallResult {
    if !is_mcp_allowed_in_context(intent, work_mode) {
        return ToolCallResult::error(
            "MCP 资源工具在规划模式、子代理或纯对话模式下不可用。".to_string(),
        );
    }

    let Some(state) = app.try_state::<McpManagerState>() else {
        return ToolCallResult::error("MCP 管理器未初始化。".to_string());
    };

    let server_filter = input["server"]
        .as_str()
        .map(str::trim)
        .filter(|s| !s.is_empty());

    let resources = state.0.list_resources(server_filter).await;
    if resources.is_empty() {
        return ToolCallResult::ok(match server_filter {
            Some(s) => format!("MCP 服务 '{}' 当前没有可用资源（或尚未建立缓存）。", s),
            None => "当前没有可用的 MCP 资源。".to_string(),
        });
    }

    let mut lines = Vec::with_capacity(resources.len() + 1);
    lines.push(format!("共找到 {} 个 MCP 资源：", resources.len()));
    for (srv, res) in resources {
        let desc = res
            .description
            .as_deref()
            .filter(|s| !s.is_empty())
            .map(|s| format!(" — {}", s))
            .unwrap_or_default();
        let mime = res
            .mime_type
            .as_deref()
            .map(|m| format!(" [{}]", m))
            .unwrap_or_default();
        lines.push(format!(
            "- [{}] {} ({}){}{}",
            srv, res.name, res.uri, mime, desc
        ));
    }
    ToolCallResult::ok(lines.join("\n"))
}

/// `ReadMcpResource` 执行入口
pub async fn read_mcp_resource(
    app: &tauri::AppHandle,
    input: &serde_json::Value,
    session_id: &str,
    intent: &str,
    work_mode: &str,
) -> ToolCallResult {
    if !is_mcp_allowed_in_context(intent, work_mode) {
        return ToolCallResult::error(
            "MCP 资源工具在规划模式、子代理或纯对话模式下不可用。".to_string(),
        );
    }

    let server = match input["server"]
        .as_str()
        .map(str::trim)
        .filter(|s| !s.is_empty())
    {
        Some(s) => s,
        None => return ToolCallResult::error("缺少必填参数 'server'（MCP 服务名）。".to_string()),
    };
    let uri = match input["uri"]
        .as_str()
        .map(str::trim)
        .filter(|s| !s.is_empty())
    {
        Some(u) => u,
        None => return ToolCallResult::error("缺少必填参数 'uri'（资源 URI）。".to_string()),
    };

    let Some(state) = app.try_state::<McpManagerState>() else {
        return ToolCallResult::error("MCP 管理器未初始化。".to_string());
    };

    let cancel_token =
        if let Some(sm) = app.try_state::<crate::infra::state::state::SessionManager>() {
            let ctx = sm.get_or_create(session_id).await;
            ctx.current_cancel_token()
        } else {
            None
        };

    match state
        .0
        .read_resource(server, uri, cancel_token.as_ref())
        .await
    {
        Ok(res) => ToolCallResult::ok(res.content),
        Err(err) => ToolCallResult::error(format!("读取 MCP 资源失败: {}", err)),
    }
}

/// `ConnectMcpServer` 执行入口：仅用于首次连接建缓存或故障重连已启用的 MCP 服务
pub async fn connect_mcp_server(
    app: &tauri::AppHandle,
    input: &serde_json::Value,
    intent: &str,
    work_mode: &str,
) -> ToolCallResult {
    if !is_mcp_allowed_in_context(intent, work_mode) {
        return ToolCallResult::error(
            "ConnectMcpServer 在规划模式、子代理或纯对话模式下不可用。".to_string(),
        );
    }

    let server = match input["server"]
        .as_str()
        .map(str::trim)
        .filter(|s| !s.is_empty())
    {
        Some(s) => s,
        None => {
            return ToolCallResult::error(
                "缺少必填参数 'server'（要连接的 MCP 服务名）。".to_string(),
            )
        }
    };

    let Some(state) = app.try_state::<McpManagerState>() else {
        return ToolCallResult::error("MCP 管理器未初始化。".to_string());
    };

    match state.0.connect_or_refresh_server(server).await {
        Ok(tools) => {
            let enabled_names: Vec<String> = tools
                .into_iter()
                .filter(|t| t.enabled)
                .map(|t| t.qualified_name)
                .collect();
            if enabled_names.is_empty() {
                ToolCallResult::ok(format!(
                    "MCP 服务 '{}' 已连接就绪，但当前没有启用的工具。",
                    server
                ))
            } else {
                ToolCallResult::ok(format!(
                    "MCP 服务 '{}' 已连接就绪，可用工具 ({} 个)：\n- {}\n\n请先使用 DiscoverTools 查询具体工具参数，再通过 ExecuteTool 执行。",
                    server,
                    enabled_names.len(),
                    enabled_names.join(", ")
                ))
            }
        }
        Err(err) => ToolCallResult::error(format!("连接 MCP 服务 '{}' 失败: {}", server, err)),
    }
}
