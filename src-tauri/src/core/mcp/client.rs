//! # core/mcp/client.rs — rmcp 客户端封装（stdio 本地子进程 + Streamable HTTP）
//!
//! 职责：
//! - 封装 `rmcp 3.11.0` 的 `RunningService<RoleClient, McpClientHandler>`；
//! - Windows stdio 子进程处理：`.cmd`/`.bat` 脚本解析 + 复用 `NoWindow` 消除黑窗 + `stderr` 采集入环形缓冲与日志文件；
//! - Streamable HTTP 传输构造与自定义请求头透传；
//! - 支持调用超时与 `CancellationToken` 中断；
//! - 第一期不声明 `roots` / `sampling` / `elicitation` 能力，监听 `list_changed` 通知。

use crate::core::mcp::config::{
    CachedPromptArgument, CachedPromptInfo, CachedResourceInfo, CachedServerCapabilities,
    CachedToolInfo, McpServerConfig, McpToolHints, McpTransportKind,
};
use crate::infra::config::data_paths;
use crate::infra::shell_command::NoWindow;
use chrono::Utc;
use http::{HeaderName, HeaderValue};
use rmcp::model::{
    CallToolRequestParams, ClientCapabilities, ClientConfig, ContentBlock, GetPromptRequestParams,
    Implementation, PromptMessage, ReadResourceRequestParams, ResourceContents, Role,
};
use rmcp::service::{NotificationContext, RoleClient, RunningService, ServiceExt};
use rmcp::transport::streamable_http_client::{
    StreamableHttpClientTransport, StreamableHttpClientTransportConfig,
};
use rmcp::transport::TokioChildProcess;
use rmcp::ClientHandler;
use serde::{Deserialize, Serialize};
use std::collections::{HashMap, VecDeque};
use std::fs::OpenOptions;
use std::io::Write;
use std::path::PathBuf;
use std::process::Stdio;
use std::sync::{Arc, Mutex};
use std::time::Duration;
use thiserror::Error;
use tokio::io::{AsyncBufReadExt, BufReader};
use tokio::process::Command;
use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;
use ts_rs::TS;

/// 内存日志环形缓冲上限（行数）
pub const MAX_LOG_RING_LINES: usize = 500;

/// MCP 工具返回文本最大字符数（与现有 tools_runner 50000 字符上限保持同一量级）
pub const MAX_MCP_TOOL_OUTPUT_CHARS: usize = 50_000;

/// MCP 客户端错误类型
#[derive(Debug, Error)]
pub enum McpClientError {
    #[error("配置无效: {0}")]
    InvalidConfig(String),
    #[error("启动子进程失败（请确认命令已安装并在 PATH 中）: {0}")]
    SpawnFailed(String),
    #[error("连接或握手超时（超过 {0} 秒）")]
    ConnectTimeout(u64),
    #[error("连接或握手失败: {0}")]
    ConnectFailed(String),
    #[error("调用超时（超过 {0} 秒）")]
    CallTimeout(u64),
    #[error("调用已被用户取消")]
    Cancelled,
    #[error("MCP 协议调用失败: {0}")]
    ProtocolError(String),
}

/// 服务端主动推送的列表变更事件（交由 McpManager 刷新缓存并推前端）
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum McpListChangedKind {
    Tools,
    Resources,
    Prompts,
}

/// 单个服务的内存日志环形缓冲 + 可选文件落盘
#[derive(Debug, Clone)]
pub struct McpLogRing {
    server_name: String,
    lines: Arc<Mutex<VecDeque<String>>>,
    log_file: Option<PathBuf>,
}

impl McpLogRing {
    pub fn new(server_name: impl Into<String>, persist_to_disk: bool) -> Self {
        let name = server_name.into();
        let log_file = if persist_to_disk && data_paths::try_data_root().is_some() {
            Some(data_paths::mcp_logs_dir().join(format!("{}.log", name)))
        } else {
            None
        };
        Self {
            server_name: name,
            lines: Arc::new(Mutex::new(VecDeque::with_capacity(MAX_LOG_RING_LINES))),
            log_file,
        }
    }

    /// 追加一行日志（自动附带 UTC 时间戳，超出上限时淘汰最旧行）
    pub fn push(&self, line: &str) {
        let ts = Utc::now().format("%Y-%m-%d %H:%M:%S");
        let entry = format!("[{}] {}", ts, line.trim_end());
        if let Ok(mut guard) = self.lines.lock() {
            if guard.len() >= MAX_LOG_RING_LINES {
                guard.pop_front();
            }
            guard.push_back(entry.clone());
        }
        if let Some(ref path) = self.log_file {
            if let Ok(mut f) = OpenOptions::new().create(true).append(true).open(path) {
                let _ = writeln!(f, "{}", entry);
            }
        }
    }

    /// 获取全部缓冲行（按时间从旧到新）
    pub fn snapshot(&self) -> Vec<String> {
        self.lines
            .lock()
            .map(|g| g.iter().cloned().collect())
            .unwrap_or_default()
    }

    /// 获取最近 N 行摘要（用于失败状态展示）
    pub fn recent_summary(&self, max_lines: usize) -> String {
        let lines = self.snapshot();
        let start = lines.len().saturating_sub(max_lines);
        lines[start..].join("\n")
    }

    pub fn server_name(&self) -> &str {
        &self.server_name
    }
}

/// 实现 rmcp `ClientHandler`：不声明 roots/sampling/elicitation，转发 list_changed 与日志
#[derive(Clone)]
pub struct McpClientHandler {
    server_name: String,
    logs: McpLogRing,
    notify_tx: Option<mpsc::UnboundedSender<(String, McpListChangedKind)>>,
}

impl McpClientHandler {
    pub fn new(
        server_name: impl Into<String>,
        logs: McpLogRing,
        notify_tx: Option<mpsc::UnboundedSender<(String, McpListChangedKind)>>,
    ) -> Self {
        Self {
            server_name: server_name.into(),
            logs,
            notify_tx,
        }
    }
}

impl ClientHandler for McpClientHandler {
    fn get_info(&self) -> ClientConfig {
        let mut info = ClientConfig::default();
        info.capabilities = ClientCapabilities::default();
        info.client_info = Implementation::new("JarvisAgent", env!("CARGO_PKG_VERSION"));
        info
    }

    async fn on_tool_list_changed(&self, _context: NotificationContext<RoleClient>) {
        self.logs.push("[notify] 收到 tools/list_changed，刷新工具缓存");
        if let Some(ref tx) = self.notify_tx {
            let _ = tx.send((self.server_name.clone(), McpListChangedKind::Tools));
        }
    }

    async fn on_resource_list_changed(&self, _context: NotificationContext<RoleClient>) {
        self.logs
            .push("[notify] 收到 resources/list_changed，刷新资源缓存");
        if let Some(ref tx) = self.notify_tx {
            let _ = tx.send((self.server_name.clone(), McpListChangedKind::Resources));
        }
    }

    async fn on_prompt_list_changed(&self, _context: NotificationContext<RoleClient>) {
        self.logs
            .push("[notify] 收到 prompts/list_changed，刷新模板缓存");
        if let Some(ref tx) = self.notify_tx {
            let _ = tx.send((self.server_name.clone(), McpListChangedKind::Prompts));
        }
    }
}

/// 渲染后的 Prompt 消息条目（供前端展示与填入输入框草稿）
#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(rename_all = "camelCase")]
pub struct RenderedPromptMessage {
    pub role: String,
    pub text: String,
}

/// 单个活跃的 MCP Client 连接会话
pub struct McpClientSession {
    service: RunningService<RoleClient, McpClientHandler>,
    logs: McpLogRing,
}

impl McpClientSession {
    /// 根据服务配置建立连接（stdio 或 Streamable HTTP），含启动超时保护
    pub async fn connect(
        server_name: &str,
        config: &McpServerConfig,
        logs: McpLogRing,
        notify_tx: Option<mpsc::UnboundedSender<(String, McpListChangedKind)>>,
    ) -> Result<Self, McpClientError> {
        let kind = config
            .validate()
            .map_err(McpClientError::InvalidConfig)?;
        let timeout_secs = config.startup_timeout_sec;

        match kind {
            McpTransportKind::Stdio => {
                Self::connect_stdio(server_name, config, logs, notify_tx, timeout_secs).await
            }
            McpTransportKind::StreamableHttp => {
                Self::connect_http(server_name, config, logs, notify_tx, timeout_secs).await
            }
        }
    }

    /// 本地 stdio 子进程连接（Windows 上兼容 `.cmd` 脚本并强制 `NoWindow` 消除黑窗）
    async fn connect_stdio(
        server_name: &str,
        config: &McpServerConfig,
        logs: McpLogRing,
        notify_tx: Option<mpsc::UnboundedSender<(String, McpListChangedKind)>>,
        timeout_secs: u64,
    ) -> Result<Self, McpClientError> {
        let raw_cmd = config
            .command
            .as_deref()
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .ok_or_else(|| McpClientError::InvalidConfig("缺少 command".to_string()))?;

        let mut cmd = build_stdio_command(raw_cmd, &config.args);
        cmd.envs(&config.env);
        if let Some(ref cwd) = config.cwd {
            if !cwd.trim().is_empty() {
                cmd.current_dir(cwd.trim());
            }
        }
        cmd.kill_on_drop(true);
        cmd.no_window();

        logs.push(&format!(
            "[stdio] 启动进程: {} {}",
            raw_cmd,
            config.args.join(" ")
        ));

        let (transport, stderr_opt) = TokioChildProcess::builder(cmd)
            .stderr(Stdio::piped())
            .spawn()
            .map_err(|e| McpClientError::SpawnFailed(format!("{}: {}", raw_cmd, e)))?;

        // 后台异步读取子进程 stderr，写入环形缓冲和日志文件
        if let Some(stderr) = stderr_opt {
            let log_clone = logs.clone();
            tokio::spawn(async move {
                let mut reader = BufReader::new(stderr).lines();
                while let Ok(Some(line)) = reader.next_line().await {
                    log_clone.push(&format!("[stderr] {}", line));
                }
            });
        }

        let handler = McpClientHandler::new(server_name, logs.clone(), notify_tx);
        Self::serve_with_timeout(handler, transport, logs, timeout_secs).await
    }

    /// 远程 Streamable HTTP 连接（透传自定义请求头）
    async fn connect_http(
        server_name: &str,
        config: &McpServerConfig,
        logs: McpLogRing,
        notify_tx: Option<mpsc::UnboundedSender<(String, McpListChangedKind)>>,
        timeout_secs: u64,
    ) -> Result<Self, McpClientError> {
        let url = config
            .url
            .as_deref()
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .ok_or_else(|| McpClientError::InvalidConfig("缺少 url".to_string()))?;

        let mut custom_headers: HashMap<HeaderName, HeaderValue> = HashMap::new();
        for (k, v) in &config.headers {
            let name = HeaderName::from_bytes(k.trim().as_bytes()).map_err(|e| {
                McpClientError::InvalidConfig(format!("非法 HTTP Header 名 '{}': {}", k, e))
            })?;
            let value = HeaderValue::from_str(v).map_err(|e| {
                McpClientError::InvalidConfig(format!("非法 HTTP Header 值 '{}': {}", k, e))
            })?;
            custom_headers.insert(name, value);
        }

        logs.push(&format!("[http] 连接远程服务: {}", url));
        let transport_cfg = StreamableHttpClientTransportConfig::with_uri(url.to_string())
            .custom_headers(custom_headers);
        let transport = StreamableHttpClientTransport::from_config(transport_cfg);

        let handler = McpClientHandler::new(server_name, logs.clone(), notify_tx);
        Self::serve_with_timeout(handler, transport, logs, timeout_secs).await
    }

    /// 通用握手入口（带超时；进程内 Mock 测试也可直接调用）
    pub async fn serve_with_timeout<T, E, A>(
        handler: McpClientHandler,
        transport: T,
        logs: McpLogRing,
        timeout_secs: u64,
    ) -> Result<Self, McpClientError>
    where
        T: rmcp::transport::IntoTransport<RoleClient, E, A>,
        E: std::error::Error + Send + Sync + 'static,
    {
        let service = tokio::time::timeout(
            Duration::from_secs(timeout_secs),
            handler.serve(transport),
        )
        .await
        .map_err(|_| McpClientError::ConnectTimeout(timeout_secs))?
        .map_err(|e| McpClientError::ConnectFailed(e.to_string()))?;

        logs.push("[client] MCP 握手成功");
        Ok(Self { service, logs })
    }

    /// 检查底层连接任务是否已结束（用于探测进程意外退出）
    pub fn is_closed(&self) -> bool {
        self.service.is_closed()
    }

    /// 一次性拉取该服务的 tools / resources / prompts 能力列表
    ///
    /// 对未实现 resources 或 prompts 的服务容错返回空列表，不阻断整体握手。
    pub async fn fetch_capabilities(
        &self,
        timeout_secs: u64,
    ) -> Result<CachedServerCapabilities, McpClientError> {
        let timeout = Duration::from_secs(timeout_secs);

        // 1. 工具列表
        let tools_raw = tokio::time::timeout(timeout, self.service.list_all_tools())
            .await
            .map_err(|_| McpClientError::CallTimeout(timeout_secs))?
            .unwrap_or_else(|err| {
                self.logs
                    .push(&format!("[warn] list_all_tools 失败（视为无工具）: {}", err));
                Vec::new()
            });

        let tools = tools_raw
            .into_iter()
            .map(|t| {
                let hints = t
                    .annotations
                    .as_ref()
                    .map(|a| McpToolHints {
                        read_only_hint: a.read_only_hint,
                        destructive_hint: a.destructive_hint,
                        idempotent_hint: a.idempotent_hint,
                        open_world_hint: a.open_world_hint,
                    })
                    .unwrap_or_default();
                let input_schema = serde_json::Value::Object((*t.input_schema).clone());
                CachedToolInfo {
                    name: t.name.into_owned(),
                    description: t.description.map(|d| d.into_owned()).unwrap_or_default(),
                    input_schema,
                    hints,
                }
            })
            .collect();

        // 2. 资源列表（服务端可能未开启 resources 能力，失败降级为空）
        let resources_raw = tokio::time::timeout(timeout, self.service.list_all_resources())
            .await
            .ok()
            .and_then(|r| r.ok())
            .unwrap_or_default();

        let resources = resources_raw
            .into_iter()
            .map(|r| CachedResourceInfo {
                uri: r.uri.clone(),
                name: r.name.clone(),
                description: r.description.clone(),
                mime_type: r.mime_type.clone(),
            })
            .collect();

        // 3. Prompt 模板列表（服务端可能未开启 prompts 能力，失败降级为空）
        let prompts_raw = tokio::time::timeout(timeout, self.service.list_all_prompts())
            .await
            .ok()
            .and_then(|r| r.ok())
            .unwrap_or_default();

        let prompts = prompts_raw
            .into_iter()
            .map(|p| CachedPromptInfo {
                name: p.name,
                description: p.description,
                arguments: p
                    .arguments
                    .unwrap_or_default()
                    .into_iter()
                    .map(|a| CachedPromptArgument {
                        name: a.name,
                        description: a.description,
                        required: a.required.unwrap_or(false),
                    })
                    .collect(),
            })
            .collect();

        Ok(CachedServerCapabilities {
            tools,
            resources,
            prompts,
            updated_at_ms: Utc::now().timestamp_millis(),
        })
    }

    /// 调用 MCP 工具（支持超时与用户取消令牌，结果超长自动截断；同时返回附带的 base64 图片列表）
    pub async fn call_tool(
        &self,
        original_tool_name: &str,
        args: &serde_json::Value,
        timeout_secs: u64,
        cancel_token: Option<&CancellationToken>,
    ) -> Result<(String, bool, Vec<(String, String)>), McpClientError> {
        let mut params = CallToolRequestParams::new(original_tool_name.to_string());
        if let Some(obj) = args.as_object() {
            params.arguments = Some(obj.clone());
        }

        let fut = self.service.call_tool(params);
        let result = run_with_timeout_and_cancel(fut, timeout_secs, cancel_token).await?;

        let is_error = result.is_error.unwrap_or(false);
        let images = extract_image_blocks(&result.content);
        let mut text = format_content_blocks(&result.content);
        if text.is_empty() {
            if let Some(structured) = result.structured_content {
                text = serde_json::to_string_pretty(&structured).unwrap_or_default();
            }
        }
        if text.is_empty() {
            text = if is_error {
                "MCP 工具返回错误（无详细内容）".to_string()
            } else {
                "（调用成功，无文本输出）".to_string()
            };
        }

        Ok((truncate_output(text), is_error, images))
    }

    /// 读取 MCP 资源（文本直接返回，二进制返回元信息占位）
    pub async fn read_resource(
        &self,
        uri: &str,
        timeout_secs: u64,
        cancel_token: Option<&CancellationToken>,
    ) -> Result<(String, Option<String>, bool), McpClientError> {
        let params = ReadResourceRequestParams::new(uri.to_string());
        let fut = self.service.read_resource(params);
        let result = run_with_timeout_and_cancel(fut, timeout_secs, cancel_token).await?;

        let mut parts = Vec::new();
        let mut first_mime: Option<String> = None;
        let mut has_binary = false;

        for item in result.contents {
            match item {
                ResourceContents::TextResourceContents {
                    mime_type, text, ..
                } => {
                    if first_mime.is_none() {
                        first_mime = mime_type;
                    }
                    parts.push(text);
                }
                ResourceContents::BlobResourceContents {
                    uri: item_uri,
                    mime_type,
                    blob,
                    ..
                } => {
                    has_binary = true;
                    if first_mime.is_none() {
                        first_mime = mime_type.clone();
                    }
                    let approx_bytes = blob.len() * 3 / 4;
                    parts.push(format!(
                        "[二进制资源未读取] uri={}, mime={}, 约 {} 字节",
                        item_uri,
                        mime_type.as_deref().unwrap_or("application/octet-stream"),
                        approx_bytes
                    ));
                }
                _ => {
                    parts.push("[未知资源内容类型]".to_string());
                }
            }
        }

        let combined = truncate_output(parts.join("\n\n"));
        Ok((combined, first_mime, has_binary))
    }

    /// 渲染 MCP Prompt 模板（返回消息列表与拼合草稿）
    pub async fn get_prompt(
        &self,
        name: &str,
        arguments: &HashMap<String, String>,
        timeout_secs: u64,
    ) -> Result<(Option<String>, Vec<RenderedPromptMessage>), McpClientError> {
        let mut params = GetPromptRequestParams::new(name.to_string());
        if !arguments.is_empty() {
            let mut map = serde_json::Map::new();
            for (k, v) in arguments {
                map.insert(k.clone(), serde_json::Value::String(v.clone()));
            }
            params.arguments = Some(map);
        }

        let result = run_with_timeout_and_cancel(
            self.service.get_prompt(params),
            timeout_secs,
            None,
        )
        .await?;

        let messages = result
            .messages
            .into_iter()
            .map(render_prompt_message)
            .collect();
        Ok((result.description, messages))
    }

    /// 优雅关闭连接（stdio 下会关闭 stdin 并等待子进程退出，超时强杀）
    pub async fn shutdown(self) {
        self.logs.push("[client] 正在关闭 MCP 连接");
        let _ = tokio::time::timeout(Duration::from_secs(5), self.service.cancel()).await;
    }
}

/// 执行带超时和取消令牌的异步协议调用
async fn run_with_timeout_and_cancel<F, T>(
    fut: F,
    timeout_secs: u64,
    cancel_token: Option<&CancellationToken>,
) -> Result<T, McpClientError>
where
    F: std::future::Future<Output = Result<T, rmcp::ServiceError>>,
{
    let timeout = Duration::from_secs(timeout_secs);
    match cancel_token {
        Some(token) => {
            tokio::select! {
                _ = token.cancelled() => Err(McpClientError::Cancelled),
                res = tokio::time::timeout(timeout, fut) => {
                    res.map_err(|_| McpClientError::CallTimeout(timeout_secs))?
                        .map_err(|e| McpClientError::ProtocolError(e.to_string()))
                }
            }
        }
        None => tokio::time::timeout(timeout, fut)
            .await
            .map_err(|_| McpClientError::CallTimeout(timeout_secs))?
            .map_err(|e| McpClientError::ProtocolError(e.to_string())),
    }
}

/// 构造 stdio 子进程命令：Windows 下对 `.cmd`/`.bat` 或 `npx`/`npm`/`pnpm`/`yarn`/`uvx` 经 `cmd /C` 拉起
fn build_stdio_command(raw_cmd: &str, args: &[String]) -> Command {
    #[cfg(target_os = "windows")]
    {
        let lower = raw_cmd.to_ascii_lowercase();
        let needs_cmd_shim = lower.ends_with(".cmd")
            || lower.ends_with(".bat")
            || matches!(
                lower.as_str(),
                "npx" | "npm" | "pnpm" | "pnpx" | "yarn" | "bunx"
            );
        if needs_cmd_shim {
            let mut cmd = Command::new("cmd");
            cmd.arg("/C").arg(raw_cmd).args(args);
            return cmd;
        }
    }

    let mut cmd = Command::new(raw_cmd);
    cmd.args(args);
    cmd
}

/// 提取 MCP 返回块中的图片数据（`(media_type, base64_data)`），供视觉模型原生看图
fn extract_image_blocks(blocks: &[ContentBlock]) -> Vec<(String, String)> {
    let mut out = Vec::new();
    for block in blocks {
        if let ContentBlock::Image(img) = block {
            if !img.data.trim().is_empty() {
                out.push((img.mime_type.clone(), img.data.clone()));
            }
        }
    }
    out
}

/// 将 MCP `ContentBlock` 列表格式化为可读文本（图片块由上层统一存入 `session_attachments` 附件表后追加引用）
fn format_content_blocks(blocks: &[ContentBlock]) -> String {
    let mut parts = Vec::new();
    for block in blocks {
        match block {
            ContentBlock::Text(t) => parts.push(t.text.clone()),
            ContentBlock::Image(_) => {
                // 图片数据已通过 extract_image_blocks 提取，由调用方统一存入 session_attachments
            }
            ContentBlock::Audio(audio) => {
                let approx = audio.data.len() * 3 / 4;
                parts.push(format!(
                    "[音频内容] mime={}, 约 {} 字节",
                    audio.mime_type, approx
                ));
            }
            ContentBlock::Resource(res) => match &res.resource {
                ResourceContents::TextResourceContents { uri, text, .. } => {
                    parts.push(format!("[嵌入资源 {}]\n{}", uri, text));
                }
                ResourceContents::BlobResourceContents {
                    uri,
                    mime_type,
                    blob,
                    ..
                } => {
                    parts.push(format!(
                        "[嵌入二进制资源 {}] mime={}, 约 {} 字节",
                        uri,
                        mime_type.as_deref().unwrap_or("application/octet-stream"),
                        blob.len() * 3 / 4
                    ));
                }
                _ => {
                    parts.push("[未知嵌入资源类型]".to_string());
                }
            },
            ContentBlock::ResourceLink(link) => {
                parts.push(format!("[资源链接] {} ({})", link.name, link.uri));
            }
            _ => {
                parts.push("[其他非文本内容]".to_string());
            }
        }
    }
    parts.join("\n")
}

fn render_prompt_message(msg: PromptMessage) -> RenderedPromptMessage {
    let role = match msg.role {
        Role::User => "user",
        Role::Assistant => "assistant",
    }
    .to_string();
    let text = format_content_blocks(&[msg.content]);
    RenderedPromptMessage { role, text }
}

/// 超长文本截断保护，避免撑爆大模型上下文
fn truncate_output(text: String) -> String {
    let char_count = text.chars().count();
    if char_count <= MAX_MCP_TOOL_OUTPUT_CHARS {
        return text;
    }
    let truncated: String = text.chars().take(MAX_MCP_TOOL_OUTPUT_CHARS).collect();
    format!(
        "{}\n\n[输出已截断：原始长度 {} 字符，超过 {} 字符上限]",
        truncated, char_count, MAX_MCP_TOOL_OUTPUT_CHARS
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use rmcp::model::{
        CallToolResponse, CallToolResult, GetPromptResponse, GetPromptResult, ListPromptsResult,
        ListResourcesResult, ListToolsResult, PaginatedRequestParams, Prompt, PromptArgument,
        ReadResourceResponse, ReadResourceResult, Resource, ServerCapabilities, ServerConfig, Tool,
    };
    use rmcp::service::{RequestContext, RoleServer};
    use rmcp::{ErrorData as McpError, ServerHandler};
    use std::borrow::Cow;
    use std::sync::Arc;

    /// 进程内 Mock MCP Server：覆盖 tools / resources / prompts 全链路
    #[derive(Clone)]
    struct MockMcpServer;

    impl ServerHandler for MockMcpServer {
        fn get_info(&self) -> ServerConfig {
            let mut info = ServerConfig::default();
            info.capabilities = ServerCapabilities::builder()
                .enable_tools()
                .enable_resources()
                .enable_prompts()
                .build();
            info.server_info = Implementation::new("mock-mcp", "0.1.0");
            info
        }

        async fn list_tools(
            &self,
            _request: Option<PaginatedRequestParams>,
            _context: RequestContext<RoleServer>,
        ) -> Result<ListToolsResult, McpError> {
            let schema = serde_json::json!({
                "type": "object",
                "properties": {
                    "msg": { "type": "string", "description": "要回显的文本" }
                },
                "required": ["msg"]
            });
            let tool = Tool::new(
                Cow::Borrowed("echo"),
                Cow::Borrowed("回显输入消息"),
                Arc::new(schema.as_object().unwrap().clone()),
            );
            Ok(ListToolsResult::with_all_items(vec![tool]))
        }

        async fn call_tool(
            &self,
            request: CallToolRequestParams,
            _context: RequestContext<RoleServer>,
        ) -> Result<CallToolResponse, McpError> {
            let msg = request
                .arguments
                .as_ref()
                .and_then(|a| a.get("msg"))
                .and_then(|v| v.as_str())
                .unwrap_or("");
            Ok(CallToolResponse::Complete(CallToolResult::success(vec![
                ContentBlock::text(format!("ECHO: {}", msg)),
            ])))
        }

        async fn list_resources(
            &self,
            _request: Option<PaginatedRequestParams>,
            _context: RequestContext<RoleServer>,
        ) -> Result<ListResourcesResult, McpError> {
            let res = Resource::new("memo://readme", "readme.md");
            Ok(ListResourcesResult::with_all_items(vec![res]))
        }

        async fn read_resource(
            &self,
            request: ReadResourceRequestParams,
            _context: RequestContext<RoleServer>,
        ) -> Result<ReadResourceResponse, McpError> {
            Ok(ReadResourceResponse::Complete(ReadResourceResult::new(
                vec![ResourceContents::text("Hello MCP Resource", request.uri)],
            )))
        }

        async fn list_prompts(
            &self,
            _request: Option<PaginatedRequestParams>,
            _context: RequestContext<RoleServer>,
        ) -> Result<ListPromptsResult, McpError> {
            let prompt = Prompt::new(
                "code_review",
                Some("代码审查模板"),
                Some(vec![PromptArgument::new("file").with_required(true)]),
            );
            Ok(ListPromptsResult::with_all_items(vec![prompt]))
        }

        async fn get_prompt(
            &self,
            request: GetPromptRequestParams,
            _context: RequestContext<RoleServer>,
        ) -> Result<GetPromptResponse, McpError> {
            let file = request
                .arguments
                .as_ref()
                .and_then(|a| a.get("file"))
                .and_then(|v| v.as_str())
                .unwrap_or("unknown");
            Ok(GetPromptResponse::Complete(
                GetPromptResult::new(vec![PromptMessage::new_text(
                    Role::User,
                    format!("请审查文件 {}", file),
                )])
                .with_description("审查提示"),
            ))
        }
    }

    #[tokio::test]
    async fn in_process_mcp_roundtrip_tools_resources_prompts() {
        let (client_io, server_io) = tokio::io::duplex(64 * 1024);

        let server_task = tokio::spawn(async move {
            let svc = MockMcpServer.serve(server_io).await.unwrap();
            let _ = svc.waiting().await;
        });

        let logs = McpLogRing::new("mock", false);
        let handler = McpClientHandler::new("mock", logs.clone(), None);
        let session = McpClientSession::serve_with_timeout(handler, client_io, logs, 10)
            .await
            .expect("进程内 MCP 握手应成功");

        let caps = session.fetch_capabilities(10).await.unwrap();
        assert_eq!(caps.tools.len(), 1);
        assert_eq!(caps.tools[0].name, "echo");
        assert_eq!(caps.resources.len(), 1);
        assert_eq!(caps.resources[0].uri, "memo://readme");
        assert_eq!(caps.prompts.len(), 1);
        assert_eq!(caps.prompts[0].name, "code_review");

        let (out, is_err, images) = session
            .call_tool("echo", &serde_json::json!({ "msg": "jarvis" }), 10, None)
            .await
            .unwrap();
        assert!(!is_err);
        assert!(images.is_empty());
        assert_eq!(out, "ECHO: jarvis");

        let (res_text, _mime, is_bin) = session
            .read_resource("memo://readme", 10, None)
            .await
            .unwrap();
        assert!(!is_bin);
        assert_eq!(res_text, "Hello MCP Resource");

        let mut prompt_args = HashMap::new();
        prompt_args.insert("file".to_string(), "main.rs".to_string());
        let (desc, msgs) = session
            .get_prompt("code_review", &prompt_args, 10)
            .await
            .unwrap();
        assert_eq!(desc.as_deref(), Some("审查提示"));
        assert_eq!(msgs.len(), 1);
        assert_eq!(msgs[0].text, "请审查文件 main.rs");

        session.shutdown().await;
        server_task.abort();
    }
}
