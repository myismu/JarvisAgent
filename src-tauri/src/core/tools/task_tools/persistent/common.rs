use crate::core::orchestration::tasks::TaskManager;
use crate::core::tools::framework;

pub(super) fn task_id(input: &serde_json::Value) -> i32 {
    input["task_id"].as_i64().unwrap_or(0) as i32
}

pub(super) fn optional_string(input: &serde_json::Value, key: &str) -> Option<String> {
    input[key].as_str().map(|s| s.to_string())
}

pub(super) fn optional_i32_vec(input: &serde_json::Value, key: &str) -> Option<Vec<i32>> {
    input[key].as_array().map(|arr| {
        arr.iter()
            .filter_map(|v| v.as_i64().map(|i| i as i32))
            .collect()
    })
}

pub(super) fn task_delete_inner(session_id: &str, id: i32) -> framework::ToolCallResult {
    match TaskManager::for_session(session_id).delete(id) {
        Ok(deleted) => framework::ToolCallResult::ok(serde_json::json!({
            "success": deleted,
            "taskId": id,
            "updatedFields": ["deleted"],
            "statusChange": { "from": "unknown", "to": "deleted" },
        })
        .to_string()),
        Err(e) => framework::ToolCallResult::error(serde_json::json!({
            "success": false,
            "taskId": id,
            "error": e
        })
        .to_string()),
    }
}
