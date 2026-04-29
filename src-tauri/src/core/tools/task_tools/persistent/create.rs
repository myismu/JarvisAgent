use super::common::optional_string;
use crate::core::orchestration::tasks::TaskManager;
use crate::core::tools::registry::ToolDef;
use serde_json::json;

pub(super) fn tool_def() -> ToolDef {
    ToolDef {
        name: "task_create",
        description: "创建持久化任务条目到任务看板",
        search_hint: "create task todo 创建 任务",
        schema: json!({
            "name": "task_create",
            "description": "Create one persistent task entry on the task board. Use this for complex workflows that need persistence, dependency graph tracking, or subagent scheduling.\n\nUse for:\n- Complex work that should persist on the task board.\n- Work that needs blockedBy/blocks dependency relationships.\n- Work that should be delegated through task/run_tasks.\n\nDo not use for:\n- A short checklist that the main agent will execute directly; use todo_write instead.\n- A single trivial task.\n- Purely conversational work.\n\nNotes:\n- After creating tasks, use task_update to add dependency relationships with add_blocked_by/add_blocks.\n- Use task_list or task_get first when needed to avoid duplicate tasks.\n- This tool creates records only; it does not execute the task.",
            "input_schema": {
                "type": "object",
                "properties": {
                    "subject": {"type": "string", "description": "Short imperative task title, e.g. \"Fix authentication bug\"."},
                    "description": {"type": "string", "description": "Detailed description of the work to complete."},
                    "activeForm": {"type": "string", "description": "Present-continuous text shown while active, e.g. \"Fixing authentication bug\"."},
                    "metadata": {"type": "object", "description": "Optional metadata key/value object."},
                    "owner": {"type": "string", "description": "Responsible agent name."}
                },
                "required": ["subject"]
            }
        }),
        should_defer: true,
        is_read_only: false,
        is_concurrency_safe: false,
        is_enabled: true,
    }
}

pub async fn task_create(
    _app: &tauri::AppHandle,
    input: &serde_json::Value,
    session_id: &str,
) -> String {
    let subject = input["subject"].as_str().unwrap_or("").to_string();
    let description = input["description"].as_str().unwrap_or("").to_string();
    let active_form = optional_string(input, "activeForm");
    let metadata = input.get("metadata").cloned();
    let owner = optional_string(input, "owner");

    match TaskManager::for_session(session_id).create(
        subject,
        description,
        active_form,
        metadata,
        owner,
    ) {
        Ok(task) => serde_json::json!({
            "success": true,
            "task": {
                "id": task.id,
                "subject": task.subject,
            }
        })
        .to_string(),
        Err(e) => serde_json::json!({
            "success": false,
            "error": e
        })
        .to_string(),
    }
}
