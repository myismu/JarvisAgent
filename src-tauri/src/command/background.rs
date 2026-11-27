//! # background.rs — 后台任务、子代理与会话待办查询 Tauri 命令
//!
//! 承载开发者监控面板和聊天侧栏所需的后台进程控制、子代理运行态/轨迹查询、
//! 子代理取消与会话级待办清单读取命令。
//!
//! ## Key Exports
//! - `get_background_tasks()` / `dismiss_background_task()` / `kill_background_task()` / `clear_session_background_tasks()`
//! - `get_subagent_runs()` / `list_subagents()` / `list_subagent_trace_runs()` / `list_subagent_trace_events()` / `cancel_subagent_run()`
//! - `get_session_todos()`
//!
//! ## Dependencies
//! - Internal: `crate::infra::background`, `crate::core::orchestration::subagents`, `crate::core::runtime::SessionManager`
//! - External: `tauri`

use crate::core::orchestration::subagents::{
    SubAgentMonitor, SubAgentMonitorState, SubAgentRun, SubAgentTraceEvent, SubAgentTraceRunSummary,
};
use crate::core::runtime::SessionManager;
use crate::infra::background::{BackgroundManager, BackgroundState, BackgroundTask};
use crate::infra::types::models::TodoGroup;

#[tauri::command]
pub async fn get_background_tasks(
    session_id: Option<String>,
    bg_state: tauri::State<'_, BackgroundState>,
) -> Result<Vec<BackgroundTask>, String> {
    let bg = bg_state.0.lock().await;
    let tasks: Vec<_> = if let Some(sid) = session_id {
        bg.tasks
            .values()
            .filter(|t| t.session_id.as_deref() == Some(&sid))
            .cloned()
            .collect()
    } else {
        bg.tasks.values().cloned().collect()
    };
    Ok(tasks)
}

#[tauri::command]
pub async fn dismiss_background_task(
    task_id: String,
    app: tauri::AppHandle,
) -> Result<bool, String> {
    Ok(BackgroundManager::dismiss_task(&app, &task_id).await)
}

#[tauri::command]
pub async fn kill_background_task(task_id: String, app: tauri::AppHandle) -> Result<bool, String> {
    Ok(BackgroundManager::kill_task(&app, &task_id).await)
}

#[tauri::command]
pub async fn clear_session_background_tasks(
    session_id: String,
    app: tauri::AppHandle,
) -> Result<usize, String> {
    Ok(BackgroundManager::clear_session_tasks(&app, &session_id).await)
}

#[tauri::command]
pub async fn get_subagent_runs(
    session_id: Option<String>,
    monitor_state: tauri::State<'_, SubAgentMonitorState>,
) -> Result<Vec<SubAgentRun>, String> {
    let mut monitor = monitor_state.0.lock().await;
    Ok(monitor.list(session_id.as_deref()))
}

#[tauri::command]
pub async fn list_subagents(
    session_id: Option<String>,
    monitor_state: tauri::State<'_, SubAgentMonitorState>,
) -> Result<Vec<SubAgentRun>, String> {
    let mut monitor = monitor_state.0.lock().await;
    Ok(monitor.list(session_id.as_deref()))
}

#[tauri::command]
pub async fn list_subagent_trace_runs(
    session_id: String,
) -> Result<Vec<SubAgentTraceRunSummary>, String> {
    tokio::task::spawn_blocking(move || SubAgentMonitor::list_trace_runs(&session_id))
        .await
        .map_err(|e| format!("查询子代理轨迹列表后台任务异常: {}", e))?
}

#[tauri::command]
pub async fn list_subagent_trace_events(run_id: String) -> Result<Vec<SubAgentTraceEvent>, String> {
    tokio::task::spawn_blocking(move || SubAgentMonitor::list_trace_events(&run_id))
        .await
        .map_err(|e| format!("查询子代理轨迹事件后台任务异常: {}", e))?
}

#[tauri::command]
pub async fn cancel_subagent_run(
    run_id: String,
    app: tauri::AppHandle,
) -> Result<SubAgentRun, String> {
    SubAgentMonitor::cancel_run(&app, &run_id).await
}

#[tauri::command]
pub async fn get_session_todos(
    session_id: String,
    session_manager: tauri::State<'_, SessionManager>,
) -> Result<Vec<TodoGroup>, String> {
    let ctx = session_manager.get_or_create(&session_id).await;
    let todos = ctx.todos.lock().await;
    Ok(todos.clone())
}
