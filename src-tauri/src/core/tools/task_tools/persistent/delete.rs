use super::common::{task_delete_inner, task_id};
use crate::core::tools::registry::ToolDef;
use serde_json::json;

pub(super) fn tool_def() -> ToolDef {
    ToolDef {
        name: "task_delete",
        description: "永久删除一个任务",
        search_hint: "delete remove task 删除 任务",
        schema: json!({
            "name": "task_delete",
            "description": "Permanently delete one task and clean up dependency references to it.",
            "input_schema": {
                "type": "object",
                "properties": {
                    "task_id": {"type": "integer", "description": "Task ID to delete."}
                },
                "required": ["task_id"]
            }
        }),
        should_defer: true,
        is_read_only: false,
        is_concurrency_safe: false,
        is_enabled: true,
    }
}

pub async fn task_delete(
    _app: &tauri::AppHandle,
    input: &serde_json::Value,
    session_id: &str,
) -> String {
    task_delete_inner(session_id, task_id(input))
}
