// --- 系统信息工具模块 ---
// get_system_info, set_workspace

use super::permission::request_permission;
use std::path::Path;
use tauri::Manager;

/// 获取当前会话的工作目录沙箱
async fn get_workspace(app: &tauri::AppHandle, session_id: &str) -> Option<std::path::PathBuf> {
    if let Some(manager) = app.try_state::<crate::core::state::SessionManager>() {
        let ctx = manager.get_or_create(session_id).await;
        let ws = ctx.workspace.lock().await.clone();
        return ws;
    }
    None
}

/// 获取系统基本信息
pub async fn get_system_info(
    app: &tauri::AppHandle,
    _input: &serde_json::Value,
    session_id: &str,
) -> String {
    let home = std::env::var("USERPROFILE").unwrap_or_else(|_| "Unknown".to_string());
    let cwd = std::env::current_dir()
        .unwrap()
        .to_string_lossy()
        .to_string();

    // 如果有沙箱，显示沙箱目录
    let ws = get_workspace(app, session_id).await;
    let workspace_info = match ws {
        Some(ref ws_path) => format!("{} (沙箱限制)", ws_path.display()),
        None => cwd,
    };

    format!(
        "OS: {}\nCWD: {}\nHome: {}",
        std::env::consts::OS,
        workspace_info,
        home
    )
}

/// 设置工作区目录
pub async fn set_workspace(
    app: &tauri::AppHandle,
    input: &serde_json::Value,
    session_id: &str,
) -> String {
    // 沙箱会话中禁用此功能
    let ws = get_workspace(app, session_id).await;
    if ws.is_some() {
        return "当前会话已配置沙箱，禁止修改全局工作区。如需更改工作目录，请创建新的沙箱会话。"
            .to_string();
    }

    let path_str = input["path"].as_str().unwrap_or("");
    if path_str.contains("..") {
        return "路径不安全".to_string();
    }
    let path = Path::new(path_str);
    if !path.is_absolute() {
        return "必须使用绝对路径".to_string();
    }
    if !path.exists() || !path.is_dir() {
        return format!("目录不存在或不是文件夹: {}", path_str);
    }

    if let Ok(cwd) = std::env::current_dir() {
        if path != cwd {
            let msg = format!("警告：尝试将全局工作区更改为：{}", path_str);
            let decision = request_permission(app, session_id, &msg).await;
            if decision == "reject" {
                return "权限拒绝".to_string();
            }
        }
    }

    match std::env::set_current_dir(path) {
        Ok(_) => {
            let workspace_file =
                crate::get_agent_home().join(crate::core::constants::FILE_WORKSPACE);
            let _ = std::fs::write(&workspace_file, path_str);
            format!("全局工作区成功切换到: {}", path_str)
        }
        Err(e) => format!("切换工作区失败: {}", e),
    }
}
