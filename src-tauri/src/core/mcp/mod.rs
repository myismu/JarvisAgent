//! # core/mcp/mod.rs — MCP (Model Context Protocol) 客户端与宿主管理模块
//!
//! JarvisAgent 作为 MCP Host（宿主），通过本模块管理所有外部 MCP Server 的配置、
//! 懒启动生命周期、本地能力缓存、命名空间映射以及工具/资源/模板调用。
//!
//! ## 子模块
//! - `config`: `data/mcp-servers.json` 配置与 `data/mcp-cache.json` 能力缓存读写
//! - `naming`: 服务名校验清洗与工具名 `mcp__{server}__{tool}` 双向映射
//! - `client`: 对官方 `rmcp` SDK 的薄封装（stdio 无黑窗子进程 + Streamable HTTP）
//! - `manager`: 全局 `McpManager`（懒启动、能力缓存刷新、单服务串行锁、退出清理、会话隔离预留）
//! - `catalog`: 动态 MCP 工具目录与唯一可用性判断口径（与静态 ToolRegistry 并列）

pub mod catalog;
pub mod client;
pub mod config;
pub mod manager;
pub mod naming;

pub use catalog::{
    format_mcp_catalog_section, is_mcp_allowed_in_context, list_available_mcp_search_entries,
    resolve_mcp_tool_schema, McpToolDescriptor,
};
pub use config::{
    CachedPromptArgument, CachedPromptInfo, CachedResourceInfo, CachedServerCapabilities,
    CachedToolInfo, McpImportItemResult, McpIsolation, McpServerConfig, McpServersFile,
    McpToolHints, McpTransportKind,
};
pub use manager::{
    McpManager, McpManagerState, McpPromptMessageView, McpPromptRenderResult,
    McpResourceReadResult, McpServerStatus, McpServerSummary, McpTestConfigResult, McpToolView,
};
pub use naming::{is_mcp_tool_name, McpToolNameMap, McpToolTarget, MCP_TOOL_PREFIX};
