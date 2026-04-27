// --- 权限管理模块 ---
// 路径安全检查、沙箱边界校验、工作区外访问授权、用户权限确认

use serde_json::json;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};
use tauri::{Emitter, Manager};
use tokio::sync::oneshot;

use crate::core::state::SessionManager;

pub fn is_path_safe(path_str: &str) -> bool {
    !path_str.contains("..")
}

fn normalize_path(path: &Path) -> PathBuf {
    let mut components = Vec::new();
    for comp in path.components() {
        match comp {
            std::path::Component::ParentDir => {
                components.pop();
            }
            std::path::Component::CurDir => {}
            other => components.push(other),
        }
    }
    components.iter().collect()
}

pub fn is_within_workspace(path_str: &str, workspace_dir: Option<&Path>) -> bool {
    if !is_path_safe(path_str) {
        return false;
    }
    let ws = match workspace_dir {
        Some(d) => d,
        None => return true,
    };
    let path = Path::new(path_str);
    let resolved = if path.is_absolute() {
        normalize_path(path)
    } else {
        let cwd = std::env::current_dir().unwrap_or_default();
        normalize_path(&cwd.join(path))
    };
    let ws_normalized = normalize_path(ws);
    resolved.starts_with(&ws_normalized)
}

pub async fn ensure_path_permission(
    _app: &tauri::AppHandle,
    path_str: &str,
    _action: &str,
    workspace_dir: Option<&Path>,
) -> Result<(), String> {
    if !is_path_safe(path_str) {
        return Err("路径不安全：包含 '..' 遍历".to_string());
    }

    // 如果指定了工作目录（即处于沙箱会话），则强制执行边界检查
    if let Some(ws) = workspace_dir {
        if !is_within_workspace(path_str, Some(ws)) {
            return Err(format!(
                "沙箱限制：路径 '{}' 不在沙箱目录 '{}' 内，拒绝访问。如果您需要访问此路径，请切换到非沙箱会话。",
                path_str,
                ws.display()
            ));
        }
    }

    // 非沙箱会话（workspace_dir 为 None）下，此处不做额外拦截，允许访问最大范围。
    Ok(())
}

pub async fn request_permission(app: &tauri::AppHandle, session_id: &str, message: &str) -> String {
    let session_manager = app.state::<SessionManager>();
    let ctx = session_manager.get_or_create(session_id).await;

    let is_session_allowed = *ctx.session_allowed.lock().await;
    if is_session_allowed {
        return "allow_session".to_string();
    }

    static REQ_ID: AtomicUsize = AtomicUsize::new(1);
    let id = REQ_ID.fetch_add(1, Ordering::SeqCst).to_string();

    let (tx, rx) = oneshot::channel();
    ctx.pending_permissions
        .lock()
        .await
        .insert(id.clone(), tx);

    let _ = app.emit(
        "permission-request",
        json!({ "id": id, "message": message, "sessionId": session_id }),
    );
    let decision = rx.await.unwrap_or_else(|_| "reject".to_string());

    if decision == "allow_session" {
        *ctx.session_allowed.lock().await = true;
    }
    decision
}
