// --- 权限管理模块 ---
// 路径安全检查、工作区外访问授权、用户权限确认

use serde_json::json;
use std::path::Path;
use std::sync::atomic::{AtomicUsize, Ordering};
use tauri::{Emitter, Manager};
use tokio::sync::oneshot;

use crate::core::{PendingPermissions, SecurityState};

/// 检查路径是否安全（不包含 ".." 遍历）
pub fn is_path_safe(path_str: &str) -> bool {
    !path_str.contains("..")
}

/// 确保路径权限安全：对工作区外的路径请求用户确认
pub async fn ensure_path_permission(
    app: &tauri::AppHandle,
    path_str: &str,
    action: &str,
) -> Result<(), String> {
    if !is_path_safe(path_str) {
        return Err("路径不安全".to_string());
    }
    let path = Path::new(path_str);
    if path.is_absolute() {
        if let Ok(cwd) = std::env::current_dir() {
            if !path.starts_with(cwd) {
                let msg = format!("警告：尝试{}工作区外路径：{}", action, path_str);
                let decision = request_permission(app, &msg).await;
                if decision == "reject" {
                    return Err("权限拒绝".to_string());
                }
            }
        }
    }
    Ok(())
}

/// 请求用户权限确认（通过前端弹窗）
pub async fn request_permission(app: &tauri::AppHandle, message: &str) -> String {
    let is_session_allowed = *app.state::<SecurityState>().session_allowed.lock().await;
    if is_session_allowed {
        return "allow_session".to_string();
    }

    static REQ_ID: AtomicUsize = AtomicUsize::new(1);
    let id = REQ_ID.fetch_add(1, Ordering::SeqCst).to_string();

    let (tx, rx) = oneshot::channel();
    app.state::<PendingPermissions>()
        .0
        .lock()
        .await
        .insert(id.clone(), tx);

    let _ = app.emit(
        "permission-request",
        json!({ "id": id, "message": message }),
    );
    let decision = rx.await.unwrap_or_else(|_| "reject".to_string());

    if decision == "allow_session" {
        *app.state::<SecurityState>().session_allowed.lock().await = true;
    }
    decision
}
