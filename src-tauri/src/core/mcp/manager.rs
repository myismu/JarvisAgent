//! # core/mcp/manager.rs — MCP 全局连接管理器（懒启动 + 能力缓存 + 串行保护）
//!
//! 作为应用级全局单例注册到 Tauri `manage(McpManagerState::new())`。
//!
//! ## 核心设计
//! - **懒启动（默认 `autoStart = false`）**：应用启动时只加载配置与本地能力缓存
//!   （`data/mcp-cache.json`），不拉起子进程；仅对显式开启 `autoStart` 的服务在后台异步预热。
//! - **并发启动合并**：同一服务被多个调用同时触发启动时，共享启动锁避免重复拉起多个进程。
//! - **单服务串行调用锁**：为每个实例维护 `tokio::sync::Mutex`，防止多会话并发把请求同时砸向
//!   脆弱的 stdio 进程或强有状态服务（如浏览器自动化）。
//! - **会话隔离预留**：实例表使用复合键 `InstanceKey { server_name, session_id: Option<String> }`，
//!   第一期统一传 `None`（全局共享），并预留 `release_session(session_id)` 清理钩子。
//! - **退出清理**：`shutdown_all()` 接入 `lib.rs` 的 `RunEvent::ExitRequested`，应用关闭时优雅关闭所有子进程。

use crate::core::mcp::client::{
    McpClientError, McpClientSession, McpListChangedKind, McpLogRing, RenderedPromptMessage,
};
use crate::core::mcp::config::{
    load_capabilities_cache, load_servers_config, parse_import_json, save_capabilities_cache,
    save_servers_config, CachedPromptInfo, CachedResourceInfo, McpCacheFile, McpImportItemResult,
    McpServerConfig, McpServersFile, McpToolHints, McpTransportKind,
};
use crate::core::mcp::naming::{validate_server_name, McpToolNameMap};
use crate::infra::types::events::McpServerStatusEvent;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::sync::Arc;
use std::time::{Duration, Instant};
use tauri::Emitter;
use tokio::sync::{mpsc, Mutex, RwLock};
use tokio_util::sync::CancellationToken;
use ts_rs::TS;

/// MCP 服务运行状态
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "snake_case")]
#[ts(rename_all = "snake_case")]
pub enum McpServerStatus {
    /// 已停用（`enabled = false`）
    Disabled,
    /// 待机（已启用但尚未启动，懒启动默认态，零进程开销）
    Idle,
    /// 正在启动并握手
    Starting,
    /// 已就绪，可正常调用
    Ready,
    /// 启动失败或进程意外退出
    Failed,
    /// 已主动停止
    Stopped,
}

/// 实例索引复合键（预留二期会话级隔离）
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct InstanceKey {
    pub server_name: String,
    /// `None` 表示应用级全局共享；`Some(sid)` 预留给二期会话级隔离
    pub session_id: Option<String>,
}

impl InstanceKey {
    pub fn global(server_name: impl Into<String>) -> Self {
        Self {
            server_name: server_name.into(),
            session_id: None,
        }
    }
}

/// 前端展示的单个服务摘要
#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(rename_all = "camelCase")]
pub struct McpServerSummary {
    pub name: String,
    pub transport: McpTransportKind,
    pub status: McpServerStatus,
    pub enabled: bool,
    pub trusted: bool,
    pub auto_start: bool,
    pub tool_count: usize,
    pub resource_count: usize,
    pub prompt_count: usize,
    /// 是否已有本地能力缓存（用于前端标识"待首次连接"）
    pub has_cached_capabilities: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub last_error: Option<String>,
    /// 原始配置（用于编辑表单回显）
    pub config: McpServerConfig,
}

/// 前端展示的单个 MCP 工具视图
#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(rename_all = "camelCase")]
pub struct McpToolView {
    /// 暴露给大模型的命名空间工具名（如 `mcp__fs__read_file`）
    pub qualified_name: String,
    /// MCP Server 声明的原始工具名
    pub name: String,
    pub server_name: String,
    pub description: String,
    pub enabled: bool,
    #[ts(type = "Record<string, unknown>")]
    pub input_schema: serde_json::Value,
    pub hints: McpToolHints,
}

/// 测试连接返回结果
#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(rename_all = "camelCase")]
pub struct McpTestConfigResult {
    pub ok: bool,
    pub tool_count: usize,
    pub resource_count: usize,
    pub prompt_count: usize,
    pub message: String,
}

/// 资源读取返回结果（前端命令用）
#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(rename_all = "camelCase")]
pub struct McpResourceReadResult {
    pub uri: String,
    pub content: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub mime_type: Option<String>,
    pub is_binary: bool,
}

/// Prompt 渲染消息视图（跨端类型）
pub type McpPromptMessageView = RenderedPromptMessage;

/// Prompt 渲染返回结果
#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(rename_all = "camelCase")]
pub struct McpPromptRenderResult {
    pub name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub description: Option<String>,
    pub messages: Vec<McpPromptMessageView>,
    /// 拼合后的纯文本草稿（可直接填入聊天输入框）
    pub combined_text: String,
}

/// 单个运行中服务实例句柄
struct RunningInstance {
    session: McpClientSession,
    /// 单服务串行调用锁：防止多会话并发冲击同一个 stdio 进程或有状态服务
    call_lock: Arc<Mutex<()>>,
    /// 最近一次活跃时间（用于空闲自动休眠回收）
    last_used_at: Mutex<Instant>,
}

impl RunningInstance {
    async fn touch(&self) {
        *self.last_used_at.lock().await = Instant::now();
    }
}

/// 单个服务的运行态元数据
struct ServerRuntimeState {
    status: McpServerStatus,
    last_error: Option<String>,
    logs: McpLogRing,
    /// 启动合并锁：防止同一服务被并发请求重复拉起
    start_lock: Arc<Mutex<()>>,
}

impl ServerRuntimeState {
    fn new(server_name: &str, enabled: bool) -> Self {
        Self {
            status: if enabled {
                McpServerStatus::Idle
            } else {
                McpServerStatus::Disabled
            },
            last_error: None,
            logs: McpLogRing::new(server_name, true),
            start_lock: Arc::new(Mutex::new(())),
        }
    }
}

/// Tauri 全局状态包装
pub struct McpManagerState(pub Arc<McpManager>);

impl McpManagerState {
    pub fn new() -> Self {
        Self(Arc::new(McpManager::new()))
    }
}

impl Default for McpManagerState {
    fn default() -> Self {
        Self::new()
    }
}

/// MCP 全局连接管理器
pub struct McpManager {
    /// 磁盘配置镜像（`data/mcp-servers.json`）
    configs: RwLock<McpServersFile>,
    /// 本地能力缓存镜像（`data/mcp-cache.json`）
    cache: RwLock<McpCacheFile>,
    /// 各服务运行态元数据（按服务名索引）
    runtimes: RwLock<HashMap<String, ServerRuntimeState>>,
    /// 活跃连接实例表（按复合键索引，预留二期会话隔离）
    instances: RwLock<HashMap<InstanceKey, Arc<RunningInstance>>>,
    /// 反向通知通道（用于接收 `list_changed` 并后台刷新缓存）
    notify_tx: mpsc::UnboundedSender<(String, McpListChangedKind)>,
    /// AppHandle（启动后注入，用于向前端 emit `mcp-server-status`）
    app_handle: RwLock<Option<tauri::AppHandle>>,
}

impl McpManager {
    pub fn new() -> Self {
        let (notify_tx, mut notify_rx) = mpsc::unbounded_channel::<(String, McpListChangedKind)>();
        let manager = Self {
            configs: RwLock::new(McpServersFile::default()),
            cache: RwLock::new(McpCacheFile::default()),
            runtimes: RwLock::new(HashMap::new()),
            instances: RwLock::new(HashMap::new()),
            notify_tx,
            app_handle: RwLock::new(None),
        };
        // 占位消费器会在 init_with_app 时由真正持有 Arc<McpManager> 的任务替换
        let _ = &mut notify_rx;
        // 将 receiver 通过内部方法绑定，见 spawn_notification_loop
        drop(notify_rx);
        manager
    }

    /// 创建并启动通知监听循环（生产路径推荐）
    pub fn new_arc() -> Arc<Self> {
        let (notify_tx, mut notify_rx) = mpsc::unbounded_channel::<(String, McpListChangedKind)>();
        let manager = Arc::new(Self {
            configs: RwLock::new(McpServersFile::default()),
            cache: RwLock::new(McpCacheFile::default()),
            runtimes: RwLock::new(HashMap::new()),
            instances: RwLock::new(HashMap::new()),
            notify_tx,
            app_handle: RwLock::new(None),
        });

        let weak = Arc::downgrade(&manager);
        tauri::async_runtime::spawn(async move {
            while let Some((server_name, _kind)) = notify_rx.recv().await {
                let Some(mgr) = weak.upgrade() else { break };
                let _ = mgr.refresh_capabilities_for(&server_name).await;
            }
        });

        // 后台空闲自动休眠巡检协程：每 60 秒检查一次，回收长时间未使用的运行中进程
        let weak_idle = Arc::downgrade(&manager);
        tauri::async_runtime::spawn(async move {
            let mut ticker = tokio::time::interval(Duration::from_secs(60));
            ticker.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
            loop {
                ticker.tick().await;
                let Some(mgr) = weak_idle.upgrade() else {
                    break;
                };
                mgr.reap_idle_instances().await;
            }
        });

        manager
    }

    /// 扫描并休眠空闲超时的 MCP 服务实例（正在执行调用中的实例不会被误杀）
    async fn reap_idle_instances(&self) {
        let entries: Vec<(InstanceKey, Arc<RunningInstance>)> = self
            .instances
            .read()
            .await
            .iter()
            .map(|(k, v)| (k.clone(), Arc::clone(v)))
            .collect();

        if entries.is_empty() {
            return;
        }

        let configs = self.configs.read().await.clone();
        let now = Instant::now();

        for (key, inst) in entries {
            let Some(cfg) = configs.mcp_servers.get(&key.server_name) else {
                continue;
            };
            if cfg.idle_timeout_min == 0 {
                continue;
            }
            // 若当前有在途调用（拿不到 call_lock），跳过本轮绝不打断
            let Ok(_busy_guard) = inst.call_lock.try_lock() else {
                continue;
            };
            let idle_elapsed = {
                let last = *inst.last_used_at.lock().await;
                now.saturating_duration_since(last)
            };
            if idle_elapsed >= Duration::from_secs(cfg.idle_timeout_min.saturating_mul(60)) {
                drop(_busy_guard);
                if let Some(rt) = self.runtimes.read().await.get(&key.server_name) {
                    rt.logs.push(&format!(
                        "[idle] 空闲已超过 {} 分钟，自动休眠释放进程",
                        cfg.idle_timeout_min
                    ));
                }
                self.stop_server(&key.server_name, McpServerStatus::Idle)
                    .await;
            }
        }
    }

    /// 应用启动时调用：加载磁盘配置与能力缓存，仅后台预热 `enabled && auto_start` 的服务
    pub async fn initialize(self: &Arc<Self>, app: tauri::AppHandle) {
        *self.app_handle.write().await = Some(app);

        let cfg_file = load_servers_config();
        let cache_file = load_capabilities_cache();

        let mut auto_start_list = Vec::new();
        {
            let mut runtimes = self.runtimes.write().await;
            for (name, cfg) in &cfg_file.mcp_servers {
                runtimes.insert(name.clone(), ServerRuntimeState::new(name, cfg.enabled));
                if cfg.enabled && cfg.auto_start {
                    auto_start_list.push(name.clone());
                }
            }
        }

        *self.configs.write().await = cfg_file;
        *self.cache.write().await = cache_file;

        // 仅对显式勾选了 autoStart 的高频服务在后台异步拉起，不阻塞窗口
        for name in auto_start_list {
            let mgr = Arc::clone(self);
            tokio::spawn(async move {
                let _ = mgr.ensure_ready(&name).await;
            });
        }
    }

    /// 向前端推送 `mcp-server-status` 事件
    async fn emit_status(&self, server_name: &str) {
        let app_opt = self.app_handle.read().await.clone();
        let Some(app) = app_opt else { return };
        if let Some(summary) = self.get_server_summary(server_name).await {
            let payload = McpServerStatusEvent {
                server_name: summary.name,
                status: summary.status,
                tool_count: summary.tool_count,
                resource_count: summary.resource_count,
                prompt_count: summary.prompt_count,
                last_error: summary.last_error,
            };
            let _ = app.emit("mcp-server-status", payload);
        }
    }

    /// 获取单个服务摘要
    pub async fn get_server_summary(&self, server_name: &str) -> Option<McpServerSummary> {
        let cfg = self
            .configs
            .read()
            .await
            .mcp_servers
            .get(server_name)?
            .clone();
        let transport = cfg.transport_kind().unwrap_or(McpTransportKind::Stdio);
        let (status, last_error) = {
            let rts = self.runtimes.read().await;
            match rts.get(server_name) {
                Some(rt) => (rt.status, rt.last_error.clone()),
                None => (
                    if cfg.enabled {
                        McpServerStatus::Idle
                    } else {
                        McpServerStatus::Disabled
                    },
                    None,
                ),
            }
        };
        let (tool_count, resource_count, prompt_count, has_cached) = {
            let cache = self.cache.read().await;
            match cache.servers.get(server_name) {
                Some(c) => (c.tools.len(), c.resources.len(), c.prompts.len(), true),
                None => (0, 0, 0, false),
            }
        };

        Some(McpServerSummary {
            name: server_name.to_string(),
            transport,
            status,
            enabled: cfg.enabled,
            trusted: cfg.trusted,
            auto_start: cfg.auto_start,
            tool_count,
            resource_count,
            prompt_count,
            has_cached_capabilities: has_cached,
            last_error,
            config: cfg,
        })
    }

    /// 列出所有已配置的 MCP 服务摘要
    pub async fn list_servers(&self) -> Vec<McpServerSummary> {
        let names: Vec<String> = self
            .configs
            .read()
            .await
            .mcp_servers
            .keys()
            .cloned()
            .collect();
        let mut out = Vec::with_capacity(names.len());
        for name in names {
            if let Some(s) = self.get_server_summary(&name).await {
                out.push(s);
            }
        }
        out
    }

    /// 读取当前全部配置的快照（供 catalog 与权限判定读取）
    pub async fn snapshot_configs_and_cache(
        &self,
    ) -> (
        McpServersFile,
        McpCacheFile,
        HashMap<String, McpServerStatus>,
    ) {
        let configs = self.configs.read().await.clone();
        let cache = self.cache.read().await.clone();
        let statuses = self
            .runtimes
            .read()
            .await
            .iter()
            .map(|(k, v)| (k.clone(), v.status))
            .collect();
        (configs, cache, statuses)
    }

    /// 查询某服务是否被用户标记为"信任"
    pub async fn is_server_trusted(&self, server_name: &str) -> bool {
        self.configs
            .read()
            .await
            .mcp_servers
            .get(server_name)
            .map(|c| c.trusted)
            .unwrap_or(false)
    }

    /// 构建全量工具名映射表（遍历所有已配置服务的缓存工具）
    pub async fn build_tool_name_map(&self) -> McpToolNameMap {
        let cache = self.cache.read().await;
        let mut map = McpToolNameMap::new();
        for (server_name, caps) in &cache.servers {
            for tool in &caps.tools {
                map.register(server_name, &tool.name);
            }
        }
        map
    }

    /// 列出指定服务（或全部服务）的工具视图（供前端详情抽屉展示）
    pub async fn list_tools(&self, server_name: Option<&str>) -> Vec<McpToolView> {
        let configs = self.configs.read().await.clone();
        let cache = self.cache.read().await.clone();
        let mut map = McpToolNameMap::new();
        let mut out = Vec::new();

        for (srv, caps) in &cache.servers {
            let Some(cfg) = configs.mcp_servers.get(srv) else {
                continue;
            };
            for tool in &caps.tools {
                let qualified_name = map.register(srv, &tool.name);
                if server_name.is_none_or(|target| target == srv) {
                    out.push(McpToolView {
                        qualified_name,
                        name: tool.name.clone(),
                        server_name: srv.clone(),
                        description: tool.description.clone(),
                        enabled: cfg.is_tool_enabled(&tool.name),
                        input_schema: tool.input_schema.clone(),
                        hints: tool.hints.clone(),
                    });
                }
            }
        }
        out
    }

    /// 列出指定服务（或全部服务）缓存的资源列表
    pub async fn list_resources(
        &self,
        server_name: Option<&str>,
    ) -> Vec<(String, CachedResourceInfo)> {
        let configs = self.configs.read().await.clone();
        let cache = self.cache.read().await.clone();
        let mut out = Vec::new();
        for (srv, caps) in &cache.servers {
            let Some(cfg) = configs.mcp_servers.get(srv) else {
                continue;
            };
            if !cfg.enabled {
                continue;
            }
            if server_name.is_some_and(|target| target != srv) {
                continue;
            }
            for res in &caps.resources {
                out.push((srv.clone(), res.clone()));
            }
        }
        out
    }

    /// 列出指定服务（或全部服务）缓存的 Prompt 模板列表
    pub async fn list_prompts(&self, server_name: Option<&str>) -> Vec<(String, CachedPromptInfo)> {
        let configs = self.configs.read().await.clone();
        let cache = self.cache.read().await.clone();
        let mut out = Vec::new();
        for (srv, caps) in &cache.servers {
            let Some(cfg) = configs.mcp_servers.get(srv) else {
                continue;
            };
            if !cfg.enabled {
                continue;
            }
            if server_name.is_some_and(|target| target != srv) {
                continue;
            }
            for prompt in &caps.prompts {
                out.push((srv.clone(), prompt.clone()));
            }
        }
        out
    }

    /// 核心懒启动入口：确保指定服务已连接就绪（若在 Idle/Failed 则透明拉起并刷新缓存）
    async fn ensure_ready(
        &self,
        server_name: &str,
    ) -> Result<Arc<RunningInstance>, McpClientError> {
        let key = InstanceKey::global(server_name);

        // 快速路径：已有存活实例直接复用
        if let Some(inst) = self.instances.read().await.get(&key).cloned() {
            if !inst.session.is_closed() {
                return Ok(inst);
            }
            // 底层连接已断，清理旧句柄准备重建
            self.instances.write().await.remove(&key);
        }

        // 获取该服务的启动合并锁，确保并发调用只拉起一次
        let (start_lock, logs) = {
            let mut rts = self.runtimes.write().await;
            let cfg_enabled = self
                .configs
                .read()
                .await
                .mcp_servers
                .get(server_name)
                .map(|c| c.enabled)
                .unwrap_or(false);
            let rt = rts
                .entry(server_name.to_string())
                .or_insert_with(|| ServerRuntimeState::new(server_name, cfg_enabled));
            (Arc::clone(&rt.start_lock), rt.logs.clone())
        };

        let _guard = start_lock.lock().await;

        // 拿锁后二次检查（可能已被前一个并发请求拉起）
        if let Some(inst) = self.instances.read().await.get(&key).cloned() {
            if !inst.session.is_closed() {
                return Ok(inst);
            }
            self.instances.write().await.remove(&key);
        }

        let cfg = self
            .configs
            .read()
            .await
            .mcp_servers
            .get(server_name)
            .cloned()
            .ok_or_else(|| {
                McpClientError::InvalidConfig(format!("MCP 服务 '{}' 不存在", server_name))
            })?;

        if !cfg.enabled {
            return Err(McpClientError::InvalidConfig(format!(
                "MCP 服务 '{}' 已停用",
                server_name
            )));
        }

        // 标记为 Starting 并通知前端
        {
            let mut rts = self.runtimes.write().await;
            if let Some(rt) = rts.get_mut(server_name) {
                rt.status = McpServerStatus::Starting;
                rt.last_error = None;
            }
        }
        self.emit_status(server_name).await;

        let session = match McpClientSession::connect(
            server_name,
            &cfg,
            logs.clone(),
            Some(self.notify_tx.clone()),
        )
        .await
        {
            Ok(s) => s,
            Err(err) => {
                let recent = logs.recent_summary(5);
                let err_text = if recent.is_empty() {
                    err.to_string()
                } else {
                    format!("{}\n最近日志:\n{}", err, recent)
                };
                {
                    let mut rts = self.runtimes.write().await;
                    if let Some(rt) = rts.get_mut(server_name) {
                        rt.status = McpServerStatus::Failed;
                        rt.last_error = Some(err_text);
                    }
                }
                self.emit_status(server_name).await;
                return Err(err);
            }
        };

        // 握手成功后立刻拉取能力并落盘缓存
        if let Ok(caps) = session.fetch_capabilities(cfg.startup_timeout_sec).await {
            let mut cache = self.cache.write().await;
            cache.servers.insert(server_name.to_string(), caps);
            let _ = save_capabilities_cache(&cache);
        }

        let inst = Arc::new(RunningInstance {
            session,
            call_lock: Arc::new(Mutex::new(())),
            last_used_at: Mutex::new(Instant::now()),
        });
        self.instances.write().await.insert(key, Arc::clone(&inst));

        {
            let mut rts = self.runtimes.write().await;
            if let Some(rt) = rts.get_mut(server_name) {
                rt.status = McpServerStatus::Ready;
                rt.last_error = None;
            }
        }
        self.emit_status(server_name).await;

        Ok(inst)
    }

    /// 刷新已连接服务的能力缓存（收到 `list_changed` 时调用）
    pub async fn refresh_capabilities_for(&self, server_name: &str) -> Result<(), McpClientError> {
        let key = InstanceKey::global(server_name);
        let Some(inst) = self.instances.read().await.get(&key).cloned() else {
            return Ok(());
        };
        let timeout_secs = self
            .configs
            .read()
            .await
            .mcp_servers
            .get(server_name)
            .map(|c| c.call_timeout_sec)
            .unwrap_or(60);

        let caps = inst.session.fetch_capabilities(timeout_secs).await?;
        {
            let mut cache = self.cache.write().await;
            cache.servers.insert(server_name.to_string(), caps);
            let _ = save_capabilities_cache(&cache);
        }
        self.emit_status(server_name).await;
        Ok(())
    }

    /// 调用指定 MCP 服务的工具（自动按需拉起 + 单服务串行锁保护）
    pub async fn call_tool(
        &self,
        server_name: &str,
        original_tool_name: &str,
        args: &serde_json::Value,
        cancel_token: Option<&CancellationToken>,
    ) -> Result<(String, bool, Vec<(String, String)>), McpClientError> {
        let inst = self.ensure_ready(server_name).await?;
        let timeout_secs = self
            .configs
            .read()
            .await
            .mcp_servers
            .get(server_name)
            .map(|c| c.call_timeout_sec)
            .unwrap_or(60);

        // 单服务串行调用锁，防止多会话并发冲击同一个 stdio 进程
        let _call_guard = inst.call_lock.lock().await;
        inst.touch().await;
        let res = inst
            .session
            .call_tool(original_tool_name, args, timeout_secs, cancel_token)
            .await;
        inst.touch().await;

        if res.is_err() && inst.session.is_closed() {
            self.mark_disconnected(server_name, "子进程连接已断开")
                .await;
        }
        res
    }

    /// 读取指定服务的资源（自动按需拉起）
    pub async fn read_resource(
        &self,
        server_name: &str,
        uri: &str,
        cancel_token: Option<&CancellationToken>,
    ) -> Result<McpResourceReadResult, McpClientError> {
        let inst = self.ensure_ready(server_name).await?;
        let timeout_secs = self
            .configs
            .read()
            .await
            .mcp_servers
            .get(server_name)
            .map(|c| c.call_timeout_sec)
            .unwrap_or(60);

        let _call_guard = inst.call_lock.lock().await;
        inst.touch().await;
        let (content, mime_type, is_binary) = inst
            .session
            .read_resource(uri, timeout_secs, cancel_token)
            .await?;
        inst.touch().await;
        Ok(McpResourceReadResult {
            uri: uri.to_string(),
            content,
            mime_type,
            is_binary,
        })
    }

    /// 渲染指定服务的 Prompt 模板（自动按需拉起）
    pub async fn get_prompt(
        &self,
        server_name: &str,
        prompt_name: &str,
        arguments: &HashMap<String, String>,
    ) -> Result<McpPromptRenderResult, McpClientError> {
        let inst = self.ensure_ready(server_name).await?;
        let timeout_secs = self
            .configs
            .read()
            .await
            .mcp_servers
            .get(server_name)
            .map(|c| c.call_timeout_sec)
            .unwrap_or(60);

        let _call_guard = inst.call_lock.lock().await;
        inst.touch().await;
        let (description, messages) = inst
            .session
            .get_prompt(prompt_name, arguments, timeout_secs)
            .await?;
        inst.touch().await;

        let combined_text = messages
            .iter()
            .map(|m| m.text.as_str())
            .filter(|s| !s.is_empty())
            .collect::<Vec<_>>()
            .join("\n\n");

        Ok(McpPromptRenderResult {
            name: prompt_name.to_string(),
            description,
            messages,
            combined_text,
        })
    }

    /// 标记连接断开
    async fn mark_disconnected(&self, server_name: &str, reason: &str) {
        let key = InstanceKey::global(server_name);
        self.instances.write().await.remove(&key);
        {
            let mut rts = self.runtimes.write().await;
            if let Some(rt) = rts.get_mut(server_name) {
                rt.status = McpServerStatus::Failed;
                rt.last_error = Some(reason.to_string());
            }
        }
        self.emit_status(server_name).await;
    }

    /// 停止单个正在运行的实例（不改配置）
    pub async fn stop_server(&self, server_name: &str, next_status: McpServerStatus) {
        let key = InstanceKey::global(server_name);
        if let Some(inst) = self.instances.write().await.remove(&key) {
            if let Ok(owned) = Arc::try_unwrap(inst) {
                owned.session.shutdown().await;
            }
        }
        {
            let mut rts = self.runtimes.write().await;
            if let Some(rt) = rts.get_mut(server_name) {
                rt.status = next_status;
                rt.last_error = None;
            }
        }
        self.emit_status(server_name).await;
    }

    /// 供 Agent 的 `ConnectMcpServer` 工具使用：若未连接则拉起并建缓存；若已就绪则直接复用不重杀进程
    pub async fn connect_or_refresh_server(
        &self,
        server_name: &str,
    ) -> Result<Vec<McpToolView>, String> {
        let inst = self
            .ensure_ready(server_name)
            .await
            .map_err(|e| e.to_string())?;
        inst.touch().await;
        Ok(self.list_tools(Some(server_name)).await)
    }

    /// 手动启动或重启某个服务
    pub async fn restart_server(&self, server_name: &str) -> Result<McpServerSummary, String> {
        self.stop_server(server_name, McpServerStatus::Idle).await;
        self.ensure_ready(server_name)
            .await
            .map_err(|e| e.to_string())?;
        self.get_server_summary(server_name)
            .await
            .ok_or_else(|| format!("MCP 服务 '{}' 不存在", server_name))
    }

    /// 新增或更新单个 MCP 服务配置
    pub async fn save_server(
        &self,
        name: String,
        config: McpServerConfig,
    ) -> Result<McpServerSummary, String> {
        validate_server_name(&name)?;
        config.validate()?;

        // 若该服务正在运行，先停掉让新配置生效
        self.stop_server(
            &name,
            if config.enabled {
                McpServerStatus::Idle
            } else {
                McpServerStatus::Disabled
            },
        )
        .await;

        {
            let mut cfgs = self.configs.write().await;
            cfgs.mcp_servers.insert(name.clone(), config.clone());
            save_servers_config(&cfgs)?;
        }
        {
            let mut rts = self.runtimes.write().await;
            let rt = rts
                .entry(name.clone())
                .or_insert_with(|| ServerRuntimeState::new(&name, config.enabled));
            rt.status = if config.enabled {
                McpServerStatus::Idle
            } else {
                McpServerStatus::Disabled
            };
            rt.last_error = None;
        }
        self.emit_status(&name).await;

        self.get_server_summary(&name)
            .await
            .ok_or_else(|| "保存后读取摘要失败".to_string())
    }

    /// 删除单个 MCP 服务配置与缓存
    pub async fn delete_server(&self, name: &str) -> Result<(), String> {
        self.stop_server(name, McpServerStatus::Stopped).await;
        {
            let mut cfgs = self.configs.write().await;
            cfgs.mcp_servers.remove(name);
            save_servers_config(&cfgs)?;
        }
        {
            let mut cache = self.cache.write().await;
            if cache.servers.remove(name).is_some() {
                let _ = save_capabilities_cache(&cache);
            }
        }
        self.runtimes.write().await.remove(name);
        Ok(())
    }

    /// 批量导入粘贴的 `mcpServers` JSON
    pub async fn import_json(&self, raw_json: &str) -> Result<Vec<McpImportItemResult>, String> {
        let (valid, report) = parse_import_json(raw_json)?;
        for (name, cfg) in valid {
            let _ = self.save_server(name, cfg).await;
        }
        Ok(report)
    }

    /// 切换服务启用状态
    pub async fn set_server_enabled(&self, name: &str, enabled: bool) -> Result<(), String> {
        {
            let mut cfgs = self.configs.write().await;
            let cfg = cfgs
                .mcp_servers
                .get_mut(name)
                .ok_or_else(|| format!("MCP 服务 '{}' 不存在", name))?;
            cfg.enabled = enabled;
            save_servers_config(&cfgs)?;
        }
        if !enabled {
            self.stop_server(name, McpServerStatus::Disabled).await;
        } else {
            let mut rts = self.runtimes.write().await;
            if let Some(rt) = rts.get_mut(name) {
                rt.status = McpServerStatus::Idle;
                rt.last_error = None;
            }
            drop(rts);
            self.emit_status(name).await;
        }
        Ok(())
    }

    /// 切换服务信任开关
    pub async fn set_server_trusted(&self, name: &str, trusted: bool) -> Result<(), String> {
        let mut cfgs = self.configs.write().await;
        let cfg = cfgs
            .mcp_servers
            .get_mut(name)
            .ok_or_else(|| format!("MCP 服务 '{}' 不存在", name))?;
        cfg.trusted = trusted;
        save_servers_config(&cfgs)
    }

    /// 切换单个工具开关（立即生效）
    pub async fn set_tool_enabled(
        &self,
        server_name: &str,
        original_tool_name: &str,
        enabled: bool,
    ) -> Result<(), String> {
        let mut cfgs = self.configs.write().await;
        let cfg = cfgs
            .mcp_servers
            .get_mut(server_name)
            .ok_or_else(|| format!("MCP 服务 '{}' 不存在", server_name))?;
        if enabled {
            cfg.disabled_tools.retain(|n| n != original_tool_name);
        } else if !cfg.disabled_tools.iter().any(|n| n == original_tool_name) {
            cfg.disabled_tools.push(original_tool_name.to_string());
        }
        save_servers_config(&cfgs)
    }

    /// 不落盘试连，返回握手结果与工具/资源/模板数量（供编辑表单"测试连接"使用）
    pub async fn test_config(&self, config: McpServerConfig) -> McpTestConfigResult {
        if let Err(err) = config.validate() {
            return McpTestConfigResult {
                ok: false,
                tool_count: 0,
                resource_count: 0,
                prompt_count: 0,
                message: err,
            };
        }

        let logs = McpLogRing::new("__test__", false);
        match McpClientSession::connect("__test__", &config, logs.clone(), None).await {
            Ok(session) => {
                let caps_res = session.fetch_capabilities(config.startup_timeout_sec).await;
                session.shutdown().await;
                match caps_res {
                    Ok(caps) => McpTestConfigResult {
                        ok: true,
                        tool_count: caps.tools.len(),
                        resource_count: caps.resources.len(),
                        prompt_count: caps.prompts.len(),
                        message: format!(
                            "连接成功：发现 {} 个工具、{} 个资源、{} 个模板",
                            caps.tools.len(),
                            caps.resources.len(),
                            caps.prompts.len()
                        ),
                    },
                    Err(err) => McpTestConfigResult {
                        ok: false,
                        tool_count: 0,
                        resource_count: 0,
                        prompt_count: 0,
                        message: format!("握手成功但拉取能力失败: {}", err),
                    },
                }
            }
            Err(err) => {
                let recent = logs.recent_summary(5);
                let msg = if recent.is_empty() {
                    err.to_string()
                } else {
                    format!("{}\n{}", err, recent)
                };
                McpTestConfigResult {
                    ok: false,
                    tool_count: 0,
                    resource_count: 0,
                    prompt_count: 0,
                    message: msg,
                }
            }
        }
    }

    /// 获取指定服务的内存日志环形缓冲
    pub async fn get_server_logs(&self, server_name: &str) -> Vec<String> {
        self.runtimes
            .read()
            .await
            .get(server_name)
            .map(|rt| rt.logs.snapshot())
            .unwrap_or_default()
    }

    /// 预留二期会话隔离回收钩子（第一期为空操作）
    pub async fn release_session(&self, _session_id: &str) {
        // 二期开启 isolation == Session 时，在此关闭并移除对应 session_id 的所有实例
    }

    /// 应用退出时优雅关闭全部 MCP 子进程（供 `lib.rs` ExitRequested 调用）
    pub async fn shutdown_all(&self) {
        let all: Vec<(InstanceKey, Arc<RunningInstance>)> = {
            let mut map = self.instances.write().await;
            map.drain().collect()
        };
        for (key, inst) in all {
            println!("[MCP] 正在关闭服务实例: {}", key.server_name);
            if let Ok(owned) = Arc::try_unwrap(inst) {
                owned.session.shutdown().await;
            }
        }
    }
}

impl Default for McpManager {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::mcp::client::McpClientHandler;
    use crate::core::mcp::config::{CachedServerCapabilities, CachedToolInfo};
    use rmcp::model::{Implementation, ServerCapabilities, ServerConfig};
    use rmcp::{ServerHandler, ServiceExt};

    #[derive(Clone)]
    struct DummyMcpServer;

    impl ServerHandler for DummyMcpServer {
        fn get_info(&self) -> ServerConfig {
            let mut info = ServerConfig::default();
            info.server_info = Implementation::new("dummy-mcp", "0.1.0");
            info.capabilities = ServerCapabilities::default();
            info
        }
    }

    async fn spawn_in_process_session(
        server_name: &str,
    ) -> (McpClientSession, tokio::task::JoinHandle<()>) {
        let (client_io, server_io) = tokio::io::duplex(32 * 1024);
        let server_task = tokio::spawn(async move {
            if let Ok(svc) = DummyMcpServer.serve(server_io).await {
                let _ = svc.waiting().await;
            }
        });
        let logs = McpLogRing::new(server_name, false);
        let handler = McpClientHandler::new(server_name, logs.clone(), None);
        let session = McpClientSession::serve_with_timeout(handler, client_io, logs, 5)
            .await
            .expect("进程内 Mock MCP 握手应成功");
        (session, server_task)
    }

    fn make_stdio_config(idle_timeout_min: u64) -> McpServerConfig {
        McpServerConfig {
            command: Some("dummy".to_string()),
            args: vec![],
            env: std::collections::BTreeMap::new(),
            cwd: None,
            url: None,
            headers: std::collections::BTreeMap::new(),
            enabled: true,
            trusted: false,
            auto_start: false,
            idle_timeout_min,
            startup_timeout_sec: 10,
            call_timeout_sec: 10,
            isolation: Default::default(),
            disabled_tools: vec![],
        }
    }

    async fn inject_running_server(
        mgr: &McpManager,
        name: &str,
        idle_timeout_min: u64,
        last_used_at: Instant,
    ) -> (Arc<RunningInstance>, tokio::task::JoinHandle<()>) {
        let (session, handle) = spawn_in_process_session(name).await;
        mgr.configs
            .write()
            .await
            .mcp_servers
            .insert(name.to_string(), make_stdio_config(idle_timeout_min));
        let mut rt = ServerRuntimeState::new(name, true);
        rt.status = McpServerStatus::Ready;
        mgr.runtimes.write().await.insert(name.to_string(), rt);
        let inst = Arc::new(RunningInstance {
            session,
            call_lock: Arc::new(Mutex::new(())),
            last_used_at: Mutex::new(last_used_at),
        });
        mgr.instances
            .write()
            .await
            .insert(InstanceKey::global(name), Arc::clone(&inst));
        (inst, handle)
    }

    #[tokio::test]
    async fn reap_idle_instances_stops_expired_and_skips_zero_timeout_or_busy() {
        let mgr = McpManager::new();
        let old_time = Instant::now()
            .checked_sub(Duration::from_secs(180))
            .unwrap_or_else(Instant::now);

        let (_expired_inst, h1) = inject_running_server(&mgr, "idle_expired", 1, old_time).await;
        // 丢弃测试侧局部 Arc，保证 stop_server 时 try_unwrap 能优雅关闭
        drop(_expired_inst);
        let (_no_timeout_inst, h2) = inject_running_server(&mgr, "no_timeout", 0, old_time).await;
        let (busy_inst, h3) = inject_running_server(&mgr, "busy_server", 1, old_time).await;

        // 模拟 busy_server 正在执行工具调用（持有单服务串行锁）
        let _busy_guard = busy_inst.call_lock.lock().await;

        mgr.reap_idle_instances().await;

        // 1. idle_expired 应被自动休眠并移出活跃表
        assert!(
            !mgr.instances
                .read()
                .await
                .contains_key(&InstanceKey::global("idle_expired")),
            "空闲超时的实例应被回收"
        );
        assert_eq!(
            mgr.get_server_summary("idle_expired").await.unwrap().status,
            McpServerStatus::Idle
        );
        assert!(
            mgr.get_server_logs("idle_expired")
                .await
                .iter()
                .any(|line| line.contains("[idle]")),
            "自动休眠应写入 [idle] 日志"
        );

        // 2. idle_timeout_min == 0 不应被回收
        assert!(
            mgr.instances
                .read()
                .await
                .contains_key(&InstanceKey::global("no_timeout")),
            "关闭空闲回收的服务不应被休眠"
        );
        assert_eq!(
            mgr.get_server_summary("no_timeout").await.unwrap().status,
            McpServerStatus::Ready
        );

        // 3. 正在调用中的服务绝不能被误杀
        assert!(
            mgr.instances
                .read()
                .await
                .contains_key(&InstanceKey::global("busy_server")),
            "有在途调用的服务不应被休眠"
        );
        assert_eq!(
            mgr.get_server_summary("busy_server").await.unwrap().status,
            McpServerStatus::Ready
        );

        drop(_busy_guard);
        drop(_no_timeout_inst);
        drop(busy_inst);
        mgr.shutdown_all().await;
        h1.abort();
        h2.abort();
        h3.abort();
    }

    #[tokio::test]
    async fn stop_server_removes_instance_updates_status_and_keeps_cache() {
        let mgr = McpManager::new();
        let (inst, handle) = inject_running_server(&mgr, "srv", 30, Instant::now()).await;
        drop(inst);

        mgr.cache.write().await.servers.insert(
            "srv".to_string(),
            CachedServerCapabilities {
                tools: vec![CachedToolInfo {
                    name: "ping".to_string(),
                    description: "测试工具".to_string(),
                    input_schema: serde_json::json!({ "type": "object" }),
                    hints: McpToolHints::default(),
                }],
                ..Default::default()
            },
        );

        mgr.stop_server("srv", McpServerStatus::Idle).await;

        assert!(!mgr
            .instances
            .read()
            .await
            .contains_key(&InstanceKey::global("srv")));
        let summary = mgr.get_server_summary("srv").await.unwrap();
        assert_eq!(summary.status, McpServerStatus::Idle);
        assert_eq!(summary.tool_count, 1, "休眠后应保留本地能力缓存");
        assert!(summary.has_cached_capabilities);

        let tools = mgr.list_tools(Some("srv")).await;
        assert_eq!(tools.len(), 1);
        assert_eq!(tools[0].name, "ping");

        handle.abort();
    }

    #[tokio::test]
    async fn connect_or_refresh_server_reuses_ready_instance_without_respawn() {
        let mgr = McpManager::new();
        let old_time = Instant::now()
            .checked_sub(Duration::from_secs(60))
            .unwrap_or_else(Instant::now);
        let (inst, handle) = inject_running_server(&mgr, "ready_srv", 30, old_time).await;
        mgr.cache.write().await.servers.insert(
            "ready_srv".to_string(),
            CachedServerCapabilities {
                tools: vec![CachedToolInfo {
                    name: "echo".to_string(),
                    description: "回显".to_string(),
                    input_schema: serde_json::json!({ "type": "object" }),
                    hints: McpToolHints::default(),
                }],
                ..Default::default()
            },
        );

        let tools = mgr
            .connect_or_refresh_server("ready_srv")
            .await
            .expect("已就绪服务应直接复用成功");
        assert_eq!(tools.len(), 1);
        assert_eq!(tools[0].name, "echo");

        // 确认复用了同一个实例并刷新了 last_used_at
        let current = mgr
            .instances
            .read()
            .await
            .get(&InstanceKey::global("ready_srv"))
            .cloned()
            .expect("实例应保持存活");
        assert!(Arc::ptr_eq(&inst, &current), "不应重建运行中的实例");
        assert!(
            *current.last_used_at.lock().await > old_time,
            "复用后应刷新 last_used_at"
        );

        drop(current);
        drop(inst);
        mgr.shutdown_all().await;
        handle.abort();
    }
}
