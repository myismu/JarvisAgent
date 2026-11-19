//! # background.rs — 后台执行与状态查询
//!
//! 提供长周期 Shell 任务的后台执行和状态检查功能。
//!
//! ## Key Exports
//! - background_run_internal(): 内部调用后台运行逻辑（唯一后台入口，由 RunCommand 的 run_in_background 模式调用）
//! - check_background(): 工具入口：检查后台任务状态
//!
//! ## Dependencies
//! - Internal: crate::infra::background::BackgroundManager, crate::core::tools::framework::permission
//! - External: serde_json, tauri

use super::super::framework;

/// 内部函数：后台执行命令（委托 BackgroundManager）。
///
/// `exec_dir` 由调用方解析好再传（2026-09-21 起）：原先这里只接 workspace，
/// 于是 RunCommand 的 `dir` 参数在后台模式下会被静默忽略 —— 模型以为换了目录，
/// 实际仍在工作区根目录跑。
///
/// `session_id` 于 2026-11-20 合并 StartBackgroundCommand 后补齐：后台任务必须
/// 带会话归属（完成通知与任务面板都按它路由），不再允许匿名任务。
pub async fn background_run_internal(
    app: &tauri::AppHandle,
    cmd: &str,
    exec_dir: &std::path::Path,
    session_id: &str,
) -> String {
    crate::infra::background::BackgroundManager::run(
        app.clone(),
        cmd.to_string(),
        Some(exec_dir.to_string_lossy().into_owned()),
        Some(session_id.to_string()),
    )
    .await
}

/// 检查后台任务状态
pub async fn check_background(
    app: &tauri::AppHandle,
    input: &serde_json::Value,
    _session_id: &str,
) -> framework::ToolCallResult {
    let task_id = input["task_id"].as_str().map(|s| s.to_string());
    let output = crate::infra::background::BackgroundManager::check(app, task_id).await;
    framework::ToolCallResult::ok(output)
}
