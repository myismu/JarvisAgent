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
//! - Internal: `crate::core::models::SessionMemory`, `crate::core::snapshot_manager::session_manager::SessionManagerRegistry`
//! - External: `tokio`, `std::sync::Arc`, `std::collections::HashMap`
//!
//! ## 约束
//! - 所有状态必须通过 Tauri 的 `.manage()` 注册
//! - 使用 `RwLock` 允许多读单写，`Mutex` 用于互斥访问
//! - `SessionManager::get_or_create()` 会自动从磁盘加载历史数据

use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::{Mutex, RwLock};

use crate::core::models::SessionMemory;
use crate::core::snapshot_manager::session_manager::SessionManagerRegistry;

pub struct WorkspaceState(pub Mutex<Option<std::path::PathBuf>>);

#[derive(Clone, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SessionCleanupResult {
    pub deleted_session_id: Option<String>,
    pub active_session_id: Option<String>,
}

pub struct SnapshotRegistry(pub RwLock<SessionManagerRegistry>);

pub struct SessionContext {
    pub id: String,
    pub memory: Mutex<SessionMemory>,
    pub cancel_token: Mutex<Option<tokio_util::sync::CancellationToken>>,
    pub active_run_id: Mutex<Option<String>>,
    pub pending_checkpoint: Mutex<Vec<crate::core::session::checkpoint::FileOperation>>,
    pub todos: Mutex<Vec<crate::core::models::TodoItem>>,
    pub workspace: Mutex<Option<std::path::PathBuf>>,
    pub session_allowed: Mutex<bool>,
    pub pending_permissions: Mutex<HashMap<String, tokio::sync::oneshot::Sender<String>>>,
}

impl SessionContext {
    pub fn new(id: String) -> Self {
        Self {
            id,
            memory: Mutex::new(SessionMemory::default()),
            cancel_token: Mutex::new(None),
            active_run_id: Mutex::new(None),
            pending_checkpoint: Mutex::new(Vec::new()),
            todos: Mutex::new(Vec::new()),
            workspace: Mutex::new(None),
            session_allowed: Mutex::new(false),
            pending_permissions: Mutex::new(HashMap::new()),
        }
    }
}

/// 全局管理器，维护所有活跃会话的 SessionContext
pub struct SessionManager(pub RwLock<HashMap<String, Arc<SessionContext>>>);

impl SessionManager {
    pub fn new() -> Self {
        Self(RwLock::new(HashMap::new()))
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
