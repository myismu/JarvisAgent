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
    pub pending_checkpoint: Mutex<Vec<crate::core::checkpoint::FileOperation>>,
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
            pending_checkpoint: Mutex::new(Vec::new()),
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
        if let Ok(memory) = crate::core::sessions::load_session(session_id) {
            *ctx.memory.lock().await = memory;
        }
        if let Ok(meta) = crate::core::sessions::get_session_meta(session_id) {
            *ctx.workspace.lock().await = meta.working_directory.map(std::path::PathBuf::from);
        }
        
        let arc_ctx = Arc::new(ctx);
        write_guard.insert(session_id.to_string(), arc_ctx.clone());
        arc_ctx
    }
}
