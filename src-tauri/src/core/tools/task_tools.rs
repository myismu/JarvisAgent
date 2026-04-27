// --- 任务管理工具模块 ---
// task_create, task_update, task_list, task_get, task_summary

use crate::core::models::TaskStatus;
use crate::core::tasks::TaskManager;

/// 创建任务
pub async fn task_create(
    _app: &tauri::AppHandle,
    input: &serde_json::Value,
    _session_id: &str,
) -> String {
    let subject = input["subject"].as_str().unwrap_or("").to_string();
    let description = input["description"].as_str().unwrap_or("").to_string();
    TaskManager::new()
        .create(subject, description)
        .unwrap_or_else(|e| e)
}

/// 更新任务状态
pub async fn task_update(
    _app: &tauri::AppHandle,
    input: &serde_json::Value,
    _session_id: &str,
) -> String {
    let id = input["task_id"].as_i64().unwrap_or(0) as i32;
    let status = input["status"].as_str().map(|s| match s {
        "in_progress" => TaskStatus::InProgress,
        "completed" => TaskStatus::Completed,
        _ => TaskStatus::Pending,
    });
    let add_blocked_by = input["add_blocked_by"].as_array().map(|arr| {
        arr.iter()
            .filter_map(|v| v.as_i64().map(|i| i as i32))
            .collect()
    });
    let add_blocks = input["add_blocks"].as_array().map(|arr| {
        arr.iter()
            .filter_map(|v| v.as_i64().map(|i| i as i32))
            .collect()
    });
    TaskManager::new()
        .update(id, status, add_blocked_by, add_blocks)
        .unwrap_or_else(|e| e)
}

/// 列出所有任务
pub async fn task_list(
    _app: &tauri::AppHandle,
    _input: &serde_json::Value,
    _session_id: &str,
) -> String {
    TaskManager::new().list_all().unwrap_or_else(|e| e)
}

/// 获取单个任务详情
pub async fn task_get(
    _app: &tauri::AppHandle,
    input: &serde_json::Value,
    _session_id: &str,
) -> String {
    let id = input["task_id"].as_i64().unwrap_or(0) as i32;
    TaskManager::new().get(id).unwrap_or_else(|e| e)
}

/// 生成任务全景报告
pub async fn task_summary(
    _app: &tauri::AppHandle,
    _input: &serde_json::Value,
    _session_id: &str,
) -> String {
    TaskManager::new().summary().unwrap_or_else(|e| e)
}
