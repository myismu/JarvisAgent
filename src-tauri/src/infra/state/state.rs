//! # state.rs — 状态管理模块
//!
//! 定义 Tauri 应用的全局状态管理器，包括会话管理、工作空间状态和快照注册表。
//! 使用 `Arc<Mutex<T>>` 和 `RwLock` 实现线程安全的状态共享。
//!
//! ## 关键导出
//! - `SessionManager`: 全局会话管理器，维护所有活跃会话的上下文
//! - `SessionContext`: 单个会话的上下文，包含记忆、取消令牌、待处理权限等
//! - `WorkspaceState`: 工作空间状态，记录当前工作目录
//! - `SnapshotRegistry`: 快照注册表，管理会话级快照
//! - `SessionCleanupResult`: 会话清理结果，用于返回删除和激活的会话 ID
//!
//! ## 依赖
//! - Internal: `crate::infra::types::models::SessionMemory`, `crate::core::session`, `crate::core::rollback::session_manager::SnapshotManagerRegistry`
//! - External: `tokio`, `std::sync::Arc`, `std::collections::HashMap`
//!
//! ## 约束
//! - 所有状态必须通过 Tauri 的 `.manage()` 注册
//! - 使用 `RwLock` 允许多读单写，`Mutex` 用于互斥访问
//! - `SessionManager::get_or_create()` 会自动从 SQLite 加载历史数据

use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::{Mutex, RwLock};

use crate::infra::types::models::SessionMemory;
use crate::core::rollback::session_manager::SnapshotManagerRegistry;

pub struct WorkspaceState(pub Mutex<Option<std::path::PathBuf>>);

#[derive(Clone, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SessionCleanupResult {
    pub deleted_session_id: Option<String>,
    pub active_session_id: Option<String>,
}

pub struct SnapshotRegistry(pub RwLock<SnapshotManagerRegistry>);

#[derive(Clone, Debug, serde::Serialize, serde::Deserialize)]
pub struct ToolDedupeCacheEntry {
    pub display: String,
    pub running: bool,
    pub suppressed_count: usize,
}

#[derive(Clone, Debug, serde::Serialize, serde::Deserialize)]
pub struct PendingPlanCacheEntry {
    pub display: String,
    pub title: String,
    pub id: String,
    pub suppressed_count: usize,
}

#[derive(Clone, Debug)]
pub struct PendingSnapshotPatch {
    pub run_id: String,
    pub seq: usize,
    pub patch: crate::core::rollback::Patch,
    pub message: Option<String>,
    pub trigger_user_memory_index: Option<usize>,
    pub trigger_user_message_id: Option<String>,
}

pub struct SessionContext {
    pub id: String,
    pub memory: Mutex<SessionMemory>,
    pub cancel_token: Mutex<Option<tokio_util::sync::CancellationToken>>,
    pub active_run_id: Mutex<Option<String>>,
    pub todos: Mutex<Vec<crate::infra::types::models::TodoItem>>,
    pub workspace: Mutex<Option<std::path::PathBuf>>,
    pub session_allowed: Mutex<bool>,
    pub pending_permissions: Mutex<HashMap<String, (std::time::Instant, tokio::sync::oneshot::Sender<String>)>>,
    pub pending_patches: Mutex<Vec<PendingSnapshotPatch>>,
    pub pending_plan_state: Mutex<HashMap<String, PendingPlanCacheEntry>>,
    /// 统一去重缓存：category → (key → entry)，替代分散的 compact/dream/skill/subagent 缓存
    pub dedupe_cache: Mutex<HashMap<String, HashMap<String, ToolDedupeCacheEntry>>>,
    /// 用户类型（"user" / "developer"），只有用户手动切换
    pub agent_audience: Mutex<String>,
    /// 工作模式（"chat" / "edit" / "plan"），用户可手动切换，Edit 下 Agent 可自动切 Plan
    pub agent_work_mode: Mutex<String>,
    /// 调度器事件接收端（异步模式）：RunSubagentsSequentially 存，pipeline 取
    pub scheduler_rx: Mutex<Option<tokio::sync::mpsc::UnboundedReceiver<crate::core::orchestration::scheduler::SchedulerEvent>>>,
    /// ReadFile 探索拦截：记录本会话已读取的文件路径，用于检测逐文件遍历模式
    pub read_file_paths: Mutex<Vec<String>>,
    /// 循环上限续跑标记：超时后用户仍可点"允许"来 resume
    pub loop_continuation_pending: Mutex<bool>,
}

impl SessionContext {
    pub fn new(id: String) -> Self {
        Self {
            id,
            memory: Mutex::new(SessionMemory::default()),
            cancel_token: Mutex::new(None),
            active_run_id: Mutex::new(None),
            todos: Mutex::new(Vec::new()),
            workspace: Mutex::new(None),
            session_allowed: Mutex::new(false),
            pending_permissions: Mutex::new(HashMap::new()),
            pending_patches: Mutex::new(Vec::new()),
            pending_plan_state: Mutex::new(HashMap::new()),
            dedupe_cache: Mutex::new(HashMap::new()),
            agent_audience: Mutex::new("developer".to_string()),
            agent_work_mode: Mutex::new("edit".to_string()),
            scheduler_rx: Mutex::new(None),
            read_file_paths: Mutex::new(Vec::new()),
            loop_continuation_pending: Mutex::new(false),
        }
    }
}

/// 全局管理器，维护所有活跃会话的 SessionContext
pub struct SessionManager(pub RwLock<HashMap<String, Arc<SessionContext>>>);

impl SessionManager {
    pub fn new() -> Self {
        Self(RwLock::new(HashMap::new()))
    }

    pub async fn remove(&self, session_id: &str) -> Option<Arc<SessionContext>> {
        self.0.write().await.remove(session_id)
    }

    pub async fn get_or_create(&self, session_id: &str) -> Arc<SessionContext> {
        let read_guard = self.0.read().await;
        if let Some(ctx) = read_guard.get(session_id) {
            return ctx.clone();
        }
        drop(read_guard);

        let mut write_guard = self.0.write().await;
        // Double check
        if let Some(ctx) = write_guard.get(session_id) {
            return ctx.clone();
        }

        let ctx = SessionContext::new(session_id.to_string());
        // 尝试从磁盘加载历史数据和工作目录
        if let Ok(memory) = crate::core::session::load_session(session_id) {
            *ctx.memory.lock().await = memory;
        }
        if let Ok(meta) = crate::core::session::get_session_meta(session_id) {
            *ctx.workspace.lock().await = meta.working_directory.map(std::path::PathBuf::from);
        }

        let arc_ctx = Arc::new(ctx);
        write_guard.insert(session_id.to_string(), arc_ctx.clone());
        arc_ctx
    }
}

use tauri::Manager;

pub async fn active_run_scope_key(app: &tauri::AppHandle, session_id: &str) -> String {
    if let Some(manager) = app.try_state::<SessionManager>() {
        let ctx = manager.get_or_create(session_id).await;
        let run_id_lock = ctx.active_run_id.lock().await;
        if let Some(run_id) = run_id_lock.as_ref() {
            return format!("{}:{}", session_id, run_id);
        }
    }
    session_id.to_string()
}

pub fn stable_hash(text: &str) -> String {
    use std::collections::hash_map::DefaultHasher;
    use std::hash::{Hash, Hasher};
    let mut hasher = DefaultHasher::new();
    text.hash(&mut hasher);
    format!("{:x}", hasher.finish())
}

pub async fn effective_workspace(
    app: &tauri::AppHandle,
    session_id: &str,
) -> Option<std::path::PathBuf> {
    if let Some(manager) = app.try_state::<SessionManager>() {
        let ctx = manager.get_or_create(session_id).await;
        return ctx.workspace.lock().await.clone();
    }
    None
}
