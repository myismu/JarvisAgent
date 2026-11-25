//! # command/mcp.rs — MCP 服务管理与能力查询 Tauri 命令层
//!
//! 为前端"扩展 → MCP 服务"页面与聊天输入框 Prompt 模板提供所有 `invoke` 入口，
//! 全部业务逻辑委派给全局 `McpManagerState`。

use crate::core::mcp::{
    CachedPromptArgument, McpImportItemResult, McpManagerState, McpPromptRenderResult,
    McpResourceReadResult, McpServerConfig, McpServerSummary, McpTestConfigResult, McpToolView,
};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use ts_rs::TS;

/// 前端展示的 MCP 资源列表项（含所属服务名）
#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(rename_all = "camelCase")]
pub struct McpResourceItemView {
    pub server_name: String,
    pub uri: String,
    pub name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub description: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub mime_type: Option<String>,
}

/// 前端展示的 MCP Prompt 模板列表项（含所属服务名）
#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(rename_all = "camelCase")]
pub struct McpPromptItemView {
    pub server_name: String,
    pub name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub description: Option<String>,
    pub arguments: Vec<CachedPromptArgument>,
}

#[tauri::command]
pub async fn mcp_list_servers(
    state: tauri::State<'_, McpManagerState>,
) -> Result<Vec<McpServerSummary>, String> {
    Ok(state.0.list_servers().await)
}

#[tauri::command]
pub async fn mcp_save_server(
    state: tauri::State<'_, McpManagerState>,
    name: String,
    config: McpServerConfig,
) -> Result<McpServerSummary, String> {
    state.0.save_server(name, config).await
}

#[tauri::command]
pub async fn mcp_delete_server(
    state: tauri::State<'_, McpManagerState>,
    name: String,
) -> Result<(), String> {
    state.0.delete_server(&name).await
}

#[tauri::command]
pub async fn mcp_import_json(
    state: tauri::State<'_, McpManagerState>,
    raw_json: String,
) -> Result<Vec<McpImportItemResult>, String> {
    state.0.import_json(&raw_json).await
}

#[tauri::command]
pub async fn mcp_set_server_enabled(
    state: tauri::State<'_, McpManagerState>,
    name: String,
    enabled: bool,
) -> Result<(), String> {
    state.0.set_server_enabled(&name, enabled).await
}

#[tauri::command]
pub async fn mcp_set_server_trusted(
    state: tauri::State<'_, McpManagerState>,
    name: String,
    trusted: bool,
) -> Result<(), String> {
    state.0.set_server_trusted(&name, trusted).await
}

#[tauri::command]
pub async fn mcp_restart_server(
    state: tauri::State<'_, McpManagerState>,
    name: String,
) -> Result<McpServerSummary, String> {
    state.0.restart_server(&name).await
}

#[tauri::command]
pub async fn mcp_stop_server(
    state: tauri::State<'_, McpManagerState>,
    name: String,
) -> Result<(), String> {
    state
        .0
        .stop_server(&name, crate::core::mcp::McpServerStatus::Idle)
        .await;
    Ok(())
}

#[tauri::command]
pub async fn mcp_test_config(
    state: tauri::State<'_, McpManagerState>,
    config: McpServerConfig,
) -> Result<McpTestConfigResult, String> {
    Ok(state.0.test_config(config).await)
}

#[tauri::command]
pub async fn mcp_list_tools(
    state: tauri::State<'_, McpManagerState>,
    server_name: Option<String>,
) -> Result<Vec<McpToolView>, String> {
    Ok(state.0.list_tools(server_name.as_deref()).await)
}

#[tauri::command]
pub async fn mcp_set_tool_enabled(
    state: tauri::State<'_, McpManagerState>,
    server_name: String,
    tool_name: String,
    enabled: bool,
) -> Result<(), String> {
    state
        .0
        .set_tool_enabled(&server_name, &tool_name, enabled)
        .await
}

#[tauri::command]
pub async fn mcp_list_resources(
    state: tauri::State<'_, McpManagerState>,
    server_name: Option<String>,
) -> Result<Vec<McpResourceItemView>, String> {
    let list = state.0.list_resources(server_name.as_deref()).await;
    Ok(list
        .into_iter()
        .map(|(srv, r)| McpResourceItemView {
            server_name: srv,
            uri: r.uri,
            name: r.name,
            description: r.description,
            mime_type: r.mime_type,
        })
        .collect())
}

#[tauri::command]
pub async fn mcp_read_resource(
    state: tauri::State<'_, McpManagerState>,
    server_name: String,
    uri: String,
) -> Result<McpResourceReadResult, String> {
    state
        .0
        .read_resource(&server_name, &uri, None)
        .await
        .map_err(|e| e.to_string())
}

#[tauri::command]
pub async fn mcp_list_prompts(
    state: tauri::State<'_, McpManagerState>,
    server_name: Option<String>,
) -> Result<Vec<McpPromptItemView>, String> {
    let list = state.0.list_prompts(server_name.as_deref()).await;
    Ok(list
        .into_iter()
        .map(|(srv, p)| McpPromptItemView {
            server_name: srv,
            name: p.name,
            description: p.description,
            arguments: p.arguments,
        })
        .collect())
}

#[tauri::command]
pub async fn mcp_get_prompt(
    state: tauri::State<'_, McpManagerState>,
    server_name: String,
    prompt_name: String,
    arguments: Option<HashMap<String, String>>,
) -> Result<McpPromptRenderResult, String> {
    let args = arguments.unwrap_or_default();
    state
        .0
        .get_prompt(&server_name, &prompt_name, &args)
        .await
        .map_err(|e| e.to_string())
}

#[tauri::command]
pub async fn mcp_get_server_logs(
    state: tauri::State<'_, McpManagerState>,
    name: String,
) -> Result<Vec<String>, String> {
    Ok(state.0.get_server_logs(&name).await)
}
