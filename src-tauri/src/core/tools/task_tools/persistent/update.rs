use super::common::{optional_i32_vec, optional_string, task_delete_inner, task_id};
use crate::core::models::TaskStatus;
use crate::core::orchestration::tasks::{TaskManager, TaskUpdateParams};
use crate::core::tools::registry::ToolDef;
use serde_json::json;

pub(super) fn tool_def() -> ToolDef {
    ToolDef {
        name: "task_update",
        description: "更新任务状态、字段或依赖关系",
        search_hint: "update task status progress 更新 任务 状态 进度",
        schema: json!({
            "name": "task_update",
            "description": "Update task status, editable fields, or dependency relationships.\n\nStatus flow: pending -> in_progress -> completed. Setting status to deleted permanently deletes the task.\n\nEditable fields:\n- status: task status.\n- subject: task title.\n- description: task description.\n- activeForm: present-continuous status text.\n- owner: responsible agent.\n- metadata: metadata object to merge; use null values to delete keys when supported by the task manager.\n- add_blocked_by: add prerequisite task IDs.\n- add_blocks: add downstream blocked task IDs.\n\nImportant:\n- Mark completed only when the task is fully done.\n- Keep in_progress when blocked by errors or incomplete work.\n- Use task_get first when you need to confirm the latest task state.",
            "input_schema": {
                "type": "object",
                "properties": {
                    "task_id": {"type": "integer", "description": "Task ID."},
                    "status": {"type": "string", "enum": ["pending", "in_progress", "completed", "deleted"], "description": "New status."},
                    "subject": {"type": "string", "description": "New title."},
                    "description": {"type": "string", "description": "New description."},
                    "activeForm": {"type": "string", "description": "Present-continuous text shown while active."},
                    "owner": {"type": "string", "description": "Responsible agent."},
                    "metadata": {"type": "object", "description": "Metadata to merge."},
                    "add_blocked_by": {"type": "array", "items": {"type": "integer"}, "description": "Prerequisite task IDs to add."},
                    "add_blocks": {"type": "array", "items": {"type": "integer"}, "description": "Downstream task IDs to mark as blocked by this task."}
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

pub async fn task_update(
    _app: &tauri::AppHandle,
    input: &serde_json::Value,
    session_id: &str,
) -> String {
    let id = task_id(input);

    if input["status"].as_str() == Some("deleted") {
        return task_delete_inner(session_id, id);
    }

    let status = input["status"].as_str().map(|s| match s {
        "in_progress" => TaskStatus::InProgress,
        "completed" => TaskStatus::Completed,
        _ => TaskStatus::Pending,
    });

    let params = TaskUpdateParams {
        status,
        subject: optional_string(input, "subject"),
        description: optional_string(input, "description"),
        active_form: optional_string(input, "activeForm"),
        owner: optional_string(input, "owner"),
        add_blocked_by: optional_i32_vec(input, "add_blocked_by"),
        add_blocks: optional_i32_vec(input, "add_blocks"),
        metadata: input.get("metadata").cloned(),
    };

    match TaskManager::for_session(session_id).update(id, params) {
        Ok(result) => {
            let mut output = serde_json::json!({
                "success": result.success,
                "taskId": result.task_id,
                "updatedFields": result.updated_fields,
            });
            if let Some(ref err) = result.error {
                output["error"] = serde_json::Value::String(err.clone());
            }
            if let Some(ref sc) = result.status_change {
                output["statusChange"] = serde_json::json!({
                    "from": format!("{:?}", sc.from).to_lowercase(),
                    "to": sc.to,
                });
            }
            if let Some(ref sc) = result.status_change {
                if sc.to == "completed" {
                    let reminder = "\n\nTask completed. Call task_list now to find your next available task or see if your work unblocked others.";
                    output["result"] = serde_json::Value::String(
                        format!(
                            "Updated task #{} {}",
                            result.task_id,
                            result.updated_fields.join(", ")
                        ) + reminder,
                    );
                }
            }
            if let Some(ref cascade) = result.cascade_message {
                output["cascadeMessage"] = serde_json::Value::String(cascade.clone());
            }
            if output.get("result").is_none() {
                output["result"] = serde_json::Value::String(format!(
                    "Updated task #{} {}",
                    result.task_id,
                    result.updated_fields.join(", ")
                ));
            }
            output.to_string()
        }
        Err(e) => serde_json::json!({
            "success": false,
            "taskId": id,
            "updatedFields": [],
            "error": e
        })
        .to_string(),
    }
}
