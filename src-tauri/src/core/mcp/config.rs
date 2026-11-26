//! # core/mcp/config.rs — MCP 服务配置与本地能力缓存读写
//!
//! 负责：
//! - `data/mcp-servers.json` 的读取、校验与原子写入（兼容 Cursor / Claude Desktop 的 `mcpServers` 格式）；
//! - `data/mcp-cache.json` 的读取与原子写入（支撑懒启动模式下未启动服务的工具目录展示）；
//! - 原始 JSON 批量导入（自动清洗不合规服务名并返回逐条校验报告）。
//!
//! ## 关键导出
//! - `McpTransport`: 传输方式枚举（`Stdio` / `StreamableHttp`）
//! - `McpIsolation`: 运行隔离级别（`Global` / `Session`，第一期仅执行 `Global`）
//! - `McpServerConfig`: 单个 MCP 服务配置
//! - `McpServersFile`: 配置文件顶层结构
//! - `CachedToolInfo` / `CachedResourceInfo` / `CachedPromptInfo` / `CachedServerCapabilities`
//! - `load_servers_config()` / `save_servers_config()`
//! - `load_capabilities_cache()` / `save_capabilities_cache()`
//! - `parse_import_json()`: 解析粘贴的 `mcpServers` JSON

use crate::core::mcp::naming::{sanitize_server_name, validate_server_name};
use crate::infra::config::data_paths;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::fs;
use std::path::Path;
use ts_rs::TS;

/// 默认启动超时（秒）：首次 `npx -y` 下载包可能耗时较长
pub const DEFAULT_STARTUP_TIMEOUT_SEC: u64 = 60;

/// 默认单次工具调用超时（秒）
pub const DEFAULT_CALL_TIMEOUT_SEC: u64 = 60;

/// 默认空闲自动休眠时长（分钟）；0 表示不自动休眠
pub const DEFAULT_IDLE_TIMEOUT_MIN: u64 = 30;

/// MCP 运行隔离级别（二期预留）
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default, TS)]
#[serde(rename_all = "lowercase")]
#[ts(rename_all = "lowercase")]
pub enum McpIsolation {
    /// 应用级全局共享（第一期默认且唯一执行模式）
    #[default]
    Global,
    /// 按会话独立实例（预留字段，二期实现）
    Session,
}

/// 单个 MCP 服务的传输配置（stdio 本地子进程 或 Streamable HTTP 远程端点）
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(rename_all = "camelCase")]
pub struct McpServerConfig {
    /// 本地 stdio 服务：可执行命令（如 `"npx"`、`"uvx"`、`"node"`）
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub command: Option<String>,
    /// 本地 stdio 服务：命令行参数
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub args: Vec<String>,
    /// 本地 stdio 服务：附加环境变量（常含 API Key，前端默认遮罩）
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub env: BTreeMap<String, String>,
    /// 本地 stdio 服务：可选工作目录
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub cwd: Option<String>,

    /// 远程 Streamable HTTP 服务：端点 URL
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub url: Option<String>,
    /// 远程 Streamable HTTP 服务：附加请求头（如 `Authorization`，前端默认遮罩）
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub headers: BTreeMap<String, String>,

    // ── JarvisAgent 扩展字段（从 Cursor/Claude Desktop 导入时均可省略） ──
    /// 是否启用该服务（默认 true）
    #[serde(default = "default_true")]
    pub enabled: bool,
    /// 是否信任该服务（默认 false；打开后才享受"帮我批准/完全批准"档位自动放行）
    #[serde(default)]
    pub trusted: bool,
    /// 是否随应用启动后台拉起（默认 false，即懒启动：首次用到时才拉起）
    #[serde(default)]
    pub auto_start: bool,
    /// 隔离级别（默认 global，预留 session）
    #[serde(default)]
    pub isolation: McpIsolation,
    /// 启动握手超时（秒，默认 60）
    #[serde(default = "default_startup_timeout")]
    #[ts(type = "number")]
    pub startup_timeout_sec: u64,
    /// 单次调用超时（秒，默认 60）
    #[serde(default = "default_call_timeout")]
    #[ts(type = "number")]
    pub call_timeout_sec: u64,
    /// 空闲自动休眠时长（分钟，默认 30；设为 0 表示不自动休眠）
    #[serde(default = "default_idle_timeout_min")]
    #[ts(type = "number")]
    pub idle_timeout_min: u64,
    /// 被用户单独关闭的原始工具名列表
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub disabled_tools: Vec<String>,
}

fn default_true() -> bool {
    true
}

fn default_startup_timeout() -> u64 {
    DEFAULT_STARTUP_TIMEOUT_SEC
}

fn default_call_timeout() -> u64 {
    DEFAULT_CALL_TIMEOUT_SEC
}

fn default_idle_timeout_min() -> u64 {
    DEFAULT_IDLE_TIMEOUT_MIN
}

/// 传输方式判别枚举（从配置字段推导）
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "snake_case")]
#[ts(rename_all = "snake_case")]
pub enum McpTransportKind {
    Stdio,
    StreamableHttp,
}

impl McpServerConfig {
    /// 判定并校验传输方式：`command` 与 `url` 必须且只能填一个
    pub fn transport_kind(&self) -> Result<McpTransportKind, String> {
        let has_cmd = self
            .command
            .as_deref()
            .is_some_and(|s| !s.trim().is_empty());
        let has_url = self.url.as_deref().is_some_and(|s| !s.trim().is_empty());

        match (has_cmd, has_url) {
            (true, false) => Ok(McpTransportKind::Stdio),
            (false, true) => {
                let url = self.url.as_deref().unwrap().trim();
                if !url.starts_with("http://") && !url.starts_with("https://") {
                    return Err(format!(
                        "远程服务 URL '{}' 必须以 http:// 或 https:// 开头",
                        url
                    ));
                }
                Ok(McpTransportKind::StreamableHttp)
            }
            (true, true) => Err("不能同时配置 command（stdio）和 url（HTTP）".to_string()),
            (false, false) => {
                Err("必须配置 command（本地 stdio）或 url（远程 HTTP）之一".to_string())
            }
        }
    }

    /// 校验单条服务配置的完整性
    pub fn validate(&self) -> Result<McpTransportKind, String> {
        let kind = self.transport_kind()?;
        if self.startup_timeout_sec == 0 || self.startup_timeout_sec > 600 {
            return Err("startupTimeoutSec 必须在 1~600 秒之间".to_string());
        }
        if self.call_timeout_sec == 0 || self.call_timeout_sec > 600 {
            return Err("callTimeoutSec 必须在 1~600 秒之间".to_string());
        }
        Ok(kind)
    }

    /// 判断某个原始工具名是否未被用户单独关闭
    pub fn is_tool_enabled(&self, original_tool_name: &str) -> bool {
        !self
            .disabled_tools
            .iter()
            .any(|name| name == original_tool_name)
    }
}

/// `data/mcp-servers.json` 顶层结构（兼容主流 `mcpServers` 格式）
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct McpServersFile {
    #[serde(default)]
    pub mcp_servers: BTreeMap<String, McpServerConfig>,
}

/// 工具注解提示（仅展示供参考，不作为权限放行依据）
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(rename_all = "camelCase")]
pub struct McpToolHints {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub read_only_hint: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub destructive_hint: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub idempotent_hint: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub open_world_hint: Option<bool>,
}

/// 缓存的 MCP 工具元数据
#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(rename_all = "camelCase")]
pub struct CachedToolInfo {
    /// MCP Server 声明的原始工具名
    pub name: String,
    /// 工具描述
    #[serde(default)]
    pub description: String,
    /// 输入参数 JSON Schema（与 MCP `inputSchema` 一致）
    #[ts(type = "Record<string, unknown>")]
    pub input_schema: serde_json::Value,
    /// 工具注解提示（仅供参考）
    #[serde(default)]
    pub hints: McpToolHints,
}

/// 缓存的 MCP 资源元数据
#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(rename_all = "camelCase")]
pub struct CachedResourceInfo {
    pub uri: String,
    pub name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub description: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub mime_type: Option<String>,
}

/// 缓存的 MCP Prompt 参数定义
#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(rename_all = "camelCase")]
pub struct CachedPromptArgument {
    pub name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub description: Option<String>,
    #[serde(default)]
    pub required: bool,
}

/// 缓存的 MCP Prompt 模板元数据
#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(rename_all = "camelCase")]
pub struct CachedPromptInfo {
    pub name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub description: Option<String>,
    #[serde(default)]
    pub arguments: Vec<CachedPromptArgument>,
}

/// 单个服务的本地能力缓存（支撑懒启动下零进程开销展示目录）
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CachedServerCapabilities {
    #[serde(default)]
    pub tools: Vec<CachedToolInfo>,
    #[serde(default)]
    pub resources: Vec<CachedResourceInfo>,
    #[serde(default)]
    pub prompts: Vec<CachedPromptInfo>,
    /// 缓存刷新时间戳（毫秒）
    #[serde(default)]
    pub updated_at_ms: i64,
}

/// `data/mcp-cache.json` 顶层结构
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct McpCacheFile {
    #[serde(default)]
    pub servers: BTreeMap<String, CachedServerCapabilities>,
}

/// 批量导入 JSON 的单条结果（返回给前端展示）
#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(rename_all = "camelCase")]
pub struct McpImportItemResult {
    /// 原始 JSON 中的 key
    pub original_name: String,
    /// 最终落盘的规范服务名
    pub saved_name: String,
    /// 是否导入成功
    pub ok: bool,
    /// 提示或失败原因（如名称已自动清洗、字段非法等）
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub message: Option<String>,
}

/// 原子写 JSON 文件（先写同目录 `.tmp`，再 rename，避免写入中途崩溃损坏配置）
fn atomic_write_json<T: Serialize>(path: &Path, value: &T) -> Result<(), String> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).map_err(|e| format!("创建目录失败: {}", e))?;
    }
    let content =
        serde_json::to_string_pretty(value).map_err(|e| format!("序列化 JSON 失败: {}", e))?;
    let tmp_path = path.with_extension("json.tmp");
    fs::write(&tmp_path, format!("{}\n", content))
        .map_err(|e| format!("写入临时文件失败: {}", e))?;

    // Windows 下目标文件存在时 rename 可能失败，先删旧再替换或直接覆盖
    #[cfg(target_os = "windows")]
    if path.exists() {
        let _ = fs::remove_file(path);
    }
    fs::rename(&tmp_path, path).map_err(|e| format!("原子替换文件失败: {}", e))?;
    Ok(())
}

/// 读取 `data/mcp-servers.json`（文件不存在时返回空配置）
pub fn load_servers_config_from(path: &Path) -> McpServersFile {
    if !path.exists() {
        return McpServersFile::default();
    }
    match fs::read_to_string(path) {
        Ok(raw) => serde_json::from_str(&raw).unwrap_or_else(|err| {
            eprintln!("[MCP] 解析配置文件 {} 失败: {}", path.display(), err);
            McpServersFile::default()
        }),
        Err(err) => {
            eprintln!("[MCP] 读取配置文件 {} 失败: {}", path.display(), err);
            McpServersFile::default()
        }
    }
}

pub fn load_servers_config() -> McpServersFile {
    load_servers_config_from(&data_paths::mcp_servers_path())
}

pub fn save_servers_config_to(path: &Path, file: &McpServersFile) -> Result<(), String> {
    atomic_write_json(path, file)
}

pub fn save_servers_config(file: &McpServersFile) -> Result<(), String> {
    save_servers_config_to(&data_paths::mcp_servers_path(), file)
}

/// 读取 `data/mcp-cache.json`
pub fn load_capabilities_cache_from(path: &Path) -> McpCacheFile {
    if !path.exists() {
        return McpCacheFile::default();
    }
    match fs::read_to_string(path) {
        Ok(raw) => serde_json::from_str(&raw).unwrap_or_default(),
        Err(_) => McpCacheFile::default(),
    }
}

pub fn load_capabilities_cache() -> McpCacheFile {
    load_capabilities_cache_from(&data_paths::mcp_cache_path())
}

pub fn save_capabilities_cache_to(path: &Path, cache: &McpCacheFile) -> Result<(), String> {
    atomic_write_json(path, cache)
}

pub fn save_capabilities_cache(cache: &McpCacheFile) -> Result<(), String> {
    save_capabilities_cache_to(&data_paths::mcp_cache_path(), cache)
}

/// JSON 批量导入解析结果：`(校验通过的待保存配置列表, 逐条结果报告)`
pub type ParsedImportBatch = (Vec<(String, McpServerConfig)>, Vec<McpImportItemResult>);

/// 解析用户粘贴的 JSON：兼容 `{ "mcpServers": { ... } }` 或直接粘贴 `{ "fs": { "command": ... } }`
pub fn parse_import_json(raw_json: &str) -> Result<ParsedImportBatch, String> {
    let value: serde_json::Value =
        serde_json::from_str(raw_json).map_err(|e| format!("JSON 语法错误: {}", e))?;

    let obj = value
        .as_object()
        .ok_or_else(|| "顶层必须是 JSON 对象".to_string())?;

    let servers_map = if let Some(inner) = obj.get("mcpServers").and_then(|v| v.as_object()) {
        inner
    } else {
        obj
    };

    if servers_map.is_empty() {
        return Err("未找到任何服务配置项".to_string());
    }

    let mut valid = Vec::new();
    let mut report = Vec::new();

    for (raw_name, cfg_val) in servers_map {
        let (clean_name, renamed) = sanitize_server_name(raw_name);
        if let Err(err) = validate_server_name(&clean_name) {
            report.push(McpImportItemResult {
                original_name: raw_name.clone(),
                saved_name: clean_name,
                ok: false,
                message: Some(err),
            });
            continue;
        }

        match serde_json::from_value::<McpServerConfig>(cfg_val.clone()) {
            Ok(cfg) => match cfg.validate() {
                Ok(_) => {
                    let msg = if renamed {
                        Some(format!("服务名已自动规范为 '{}'", clean_name))
                    } else {
                        None
                    };
                    valid.push((clean_name.clone(), cfg));
                    report.push(McpImportItemResult {
                        original_name: raw_name.clone(),
                        saved_name: clean_name,
                        ok: true,
                        message: msg,
                    });
                }
                Err(err) => {
                    report.push(McpImportItemResult {
                        original_name: raw_name.clone(),
                        saved_name: clean_name,
                        ok: false,
                        message: Some(err),
                    });
                }
            },
            Err(err) => {
                report.push(McpImportItemResult {
                    original_name: raw_name.clone(),
                    saved_name: clean_name,
                    ok: false,
                    message: Some(format!("配置结构解析失败: {}", err)),
                });
            }
        }
    }

    Ok((valid, report))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_cursor_style_mcp_servers_json() {
        let raw = r#"{
            "mcpServers": {
                "File System": {
                    "command": "npx",
                    "args": ["-y", "@modelcontextprotocol/server-filesystem", "D:/work"],
                    "env": { "DEBUG": "1" }
                },
                "remote-docs": {
                    "url": "https://example.com/mcp",
                    "headers": { "Authorization": "Bearer secret" }
                },
                "broken": {
                    "command": "npx",
                    "url": "https://conflict.example.com"
                }
            }
        }"#;

        let (valid, report) = parse_import_json(raw).expect("应能解析顶层 JSON");
        assert_eq!(valid.len(), 2);
        assert_eq!(report.len(), 3);

        let fs_cfg = valid
            .iter()
            .find(|(n, _)| n == "file_system")
            .map(|(_, c)| c)
            .expect("File System 应清洗为 file_system");
        assert!(fs_cfg.enabled);
        assert!(!fs_cfg.trusted, "导入的服务默认不信任");
        assert!(!fs_cfg.auto_start, "默认懒启动（auto_start = false）");
        assert_eq!(fs_cfg.isolation, McpIsolation::Global);

        let broken_report = report.iter().find(|r| r.original_name == "broken").unwrap();
        assert!(!broken_report.ok);
    }

    #[test]
    fn config_roundtrip_atomic_write() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("mcp-servers.json");
        let mut file = McpServersFile::default();
        file.mcp_servers.insert(
            "demo".to_string(),
            McpServerConfig {
                command: Some("node".to_string()),
                args: vec!["server.js".to_string()],
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
                idle_timeout_min: DEFAULT_IDLE_TIMEOUT_MIN,
                disabled_tools: vec!["dangerous_tool".to_string()],
            },
        );

        save_servers_config_to(&path, &file).unwrap();
        let loaded = load_servers_config_from(&path);
        assert_eq!(loaded.mcp_servers.len(), 1);
        assert!(!loaded.mcp_servers["demo"].is_tool_enabled("dangerous_tool"));
        assert!(loaded.mcp_servers["demo"].is_tool_enabled("read_only"));
    }
}
