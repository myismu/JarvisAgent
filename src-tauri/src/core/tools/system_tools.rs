// --- 系统信息工具模块 ---
// get_system_info, set_workspace

use super::permission::request_permission;
use std::path::Path;

/// 获取系统基本信息
pub async fn get_system_info(
    _app: &tauri::AppHandle,
    _input: &serde_json::Value,
) -> String {
    let home = std::env::var("USERPROFILE").unwrap_or_else(|_| "Unknown".to_string());
    format!(
        "OS: {}\nCWD: {}\nHome: {}",
        std::env::consts::OS,
        std::env::current_dir().unwrap().to_string_lossy(),
        home
    )
}

/// 设置工作区目录
pub async fn set_workspace(
    app: &tauri::AppHandle,
    input: &serde_json::Value,
) -> String {
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
            let decision = request_permission(app, &msg).await;
            if decision == "reject" {
                return "权限拒绝".to_string();
            }
        }
    }

    match std::env::set_current_dir(path) {
        Ok(_) => {
            let workspace_file = crate::get_agent_home().join(crate::core::constants::FILE_WORKSPACE);
            let _ = std::fs::write(&workspace_file, path_str);
            format!("全局工作区成功切换到: {}", path_str)
        }
        Err(e) => format!("切换工作区失败: {}", e),
    }
}
