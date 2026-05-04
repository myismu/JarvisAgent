use crate::core::orchestration::tasks::TaskManager;
use crate::core::tools::framework::registry::ToolDef;
use serde_json::json;

pub(super) fn tool_def() -> ToolDef {
    ToolDef {
        name: "ListTasks",
        description: "列出所有任务及其状态概要",
        search_hint: "list tasks status overview 列出 任务 状态",
        schema: json!({
            "name": "ListTasks",
            "description": "List all tasks and their status summary.\n\nReturned information includes:\n- id: task identifier used by GetTask and UpdateTask.\n- subject: short task title.\n- status: pending/in_progress/completed.\n- owner: responsible agent when present.\n- blockedBy: incomplete prerequisite dependencies.\n\nGuidance:\n- Prefer processing in ID order when dependency constraints allow it.\n- Call this after completing a task to find the next available task.\n- Inspect blocked tasks to determine which prerequisite must be resolved first.",
            "input_schema": { "type": "object", "properties": {} }
        }),
        should_defer: true,
        is_read_only: true,
        is_concurrency_safe: true,
        is_enabled: true,
    }
}

pub async fn task_list(
    _app: &tauri::AppHandle,
    _input: &serde_json::Value,
    session_id: &str,
) -> String {
    TaskManager::for_session(session_id)
        .list_all()
        .unwrap_or_else(|e| e)
}
