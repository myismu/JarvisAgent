//! # permission.rs — 权限管理模块
//!
//! 路径安全检查、沙箱边界校验、用户权限确认（通过 oneshot channel 阻塞等待前端决策）。
//!
//! ## 关键导出
//! - `is_path_safe()`: 检查路径是否包含 `..` 遍历
//! - `is_within_workspace()`: 检查路径是否在沙箱工作目录内
//! - `ensure_path_permission()`: 综合权限检查（安全 + 沙箱边界）
//! - `request_permission()`: 向前端发送权限确认请求，阻塞等待用户决策
//!
//! ## 约束
//! - 沙箱会话强制执行路径边界检查
//! - 非沙箱会话不做额外拦截，允许访问最大范围
//! - `allow_session` 决策会被缓存，后续请求自动放行

use serde_json::json;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};
use tauri::{Emitter, Manager};
use tokio::sync::oneshot;

use crate::infra::state::state::SessionManager;

pub fn is_path_safe(path_str: &str) -> bool {
    !path_str.contains("..")
}

fn normalize_path(path: &Path) -> PathBuf {
    // 剥掉 Windows 长路径前缀 \\?\，否则组件序列不匹配
    let path_str = path.to_string_lossy();
    let path = if path_str.starts_with(r"\\?\") {
        Path::new(&path_str[4..])
    } else {
        path
    };
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

/// 检查路径是否在工作目录沙箱内（解析 `.` 和 `..` 后比较前缀）
pub fn is_within_workspace(path_str: &str, workspace_dir: Option<&Path>) -> bool {
    if !is_path_safe(path_str) {
        return false;
    }
    // 无沙箱限制时直接放行
    let ws = match workspace_dir {
        Some(d) => d,
        None => return true,
    };
    let path = Path::new(path_str);
    // 相对路径先拼接 CWD 再归一化
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

/// 向前端发送权限确认请求，通过 oneshot channel 阻塞等待用户决策
pub async fn request_permission(app: &tauri::AppHandle, session_id: &str, message: &str) -> String {
    let session_manager = app.state::<SessionManager>();
    let ctx = session_manager.get_or_create(session_id).await;

    // 会话级授权已开启，直接放行
    let is_session_allowed = *ctx.session_allowed.lock().await;
    if is_session_allowed {
        return "allow_session".to_string();
    }

    // 生成唯一请求 ID，创建 oneshot channel 等待前端回调
    static REQ_ID: AtomicUsize = AtomicUsize::new(1);
    let id = REQ_ID.fetch_add(1, Ordering::SeqCst).to_string();

    let (tx, rx) = oneshot::channel();
    // 插入前清理超时条目（5 分钟未响应）
    {
        let mut perms = ctx.pending_permissions.lock().await;
        let now = std::time::Instant::now();
        perms.retain(|_, (ts, _)| now.duration_since(*ts).as_secs() < 300);
        perms.insert(id.clone(), (std::time::Instant::now(), tx));
    }

    let _ = app.emit(
        "permission-request",
        json!({ "id": id, "message": message, "sessionId": session_id }),
    );
    let decision = tokio::time::timeout(std::time::Duration::from_secs(30), rx)
        .await
        .map(|r| r.unwrap_or_else(|_| "reject".to_string()))
        .unwrap_or_else(|_| "reject".to_string());

    if decision == "allow_session" {
        *ctx.session_allowed.lock().await = true;
    }
    decision
}
