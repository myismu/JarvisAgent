use super::common::task_id;
use crate::core::orchestration::tasks::TaskManager;
use crate::core::tools::framework::registry::ToolDef;
use serde_json::json;

pub(super) fn tool_def() -> ToolDef {
    ToolDef {
        name: "GetTask",
        description: "获取单个任务的完整详细信息",
        search_hint: "get task detail info 获取 任务 详情",
        schema: json!({
            "name": "GetTask",
            "description": "Get full details for one task, including description, dependency relationships, activeForm, metadata, and owner.\n\nUse when:\n- Starting work and needing the complete task requirements.\n- Inspecting task dependencies.\n- Reviewing a task assigned to a specific agent.\n\nAfter fetching, confirm blockedBy is empty before starting the work.",
            "input_schema": {
                "type": "object",
                "properties": {
                    "task_id": {"type": "integer", "description": "Task ID."}
                },
                "required": ["task_id"]
            }
        }),
        should_defer: true,
        is_read_only: true,
        is_concurrency_safe: true,
        is_enabled: true,
    }
}

pub async fn task_get(
    _app: &tauri::AppHandle,
    input: &serde_json::Value,
    session_id: &str,
) -> String {
    match TaskManager::for_session(session_id).get(task_id(input)) {
        Ok(task) => serde_json::to_string_pretty(&task).unwrap_or_default(),
        Err(e) => e,
    }
}
