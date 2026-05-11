//! # background.rs — 后台执行与状态查询
//!
//! 提供长周期 Shell 任务的后台执行和状态检查功能。
//!
//! ## Key Exports
//! - ackground_run_internal(): 内部调用后台运行逻辑
//! - ackground_run(): 工具入口：后台执行长时间运行的命令
//! - check_background(): 工具入口：检查后台任务状态
//!
//! ## Dependencies
//! - Internal: crate::infra::background::BackgroundManager, crate::core::tools::framework::permission
//! - External: serde_json, 	auri

use super::super::framework::permission::is_within_workspace;
use super::utils::*;

/// 内部函数：后台执行命令（委托 BackgroundManager）
pub async fn background_run_internal(
    app: &tauri::AppHandle,
    cmd: &str,
    workspace: &Option<std::path::PathBuf>,
) -> String {
    let exec_dir = workspace.as_ref().map(|p| p.to_string_lossy().into_owned());
    crate::infra::background::BackgroundManager::run(app.clone(), cmd.to_string(), exec_dir, None)
        .await
}

/// 后台执行长时间运行的命令（独立工具，保留供 UI 直接触发）
pub async fn background_run(
    app: &tauri::AppHandle,
    input: &serde_json::Value,
    session_id: &str,
) -> String {
    let cmd = input["command"].as_str().unwrap_or("");
    let dir = input["dir"].as_str().map(|s| s.to_string());

    let ws = get_workspace(app, session_id).await;

    // 如果是沙箱会话，验证 dir
    if let Some(ref workspace) = ws {
        if let Some(ref d) = dir {
            if !is_within_workspace(d, Some(workspace)) {
                return format!("沙箱限制：指定的目录 '{}' 不在沙箱内。", d);
            }
        }
    }

    // 如果没有提供路径，则用工作目录
    let exec_dir = if let Some(d) = dir {
        Some(d)
    } else {
        ws.map(|p| p.to_string_lossy().into_owned())
    };

    crate::infra::background::BackgroundManager::run(app.clone(), cmd.to_string(), exec_dir, Some(session_id.to_string()))
        .await
}

/// 检查后台任务状态
pub async fn check_background(
    app: &tauri::AppHandle,
    input: &serde_json::Value,
    _session_id: &str,
) -> String {
    let task_id = input["task_id"].as_str().map(|s| s.to_string());
    crate::infra::background::BackgroundManager::check(app, task_id).await
}
