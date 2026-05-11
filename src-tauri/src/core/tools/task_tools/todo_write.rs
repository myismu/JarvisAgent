use crate::infra::types::models::{TodoItem, TodoStatus};
use crate::core::tools::framework::registry::ToolDef;
use serde_json::json;
use tauri::{Emitter, Manager};

pub(super) fn tool_def() -> ToolDef {
    ToolDef {
        name: "UpdateTodos",
        description: "更新当前会话的轻量待办清单，用于主 Agent 自己执行时展示进度",
        search_hint: "todo checklist progress lightweight task list activeForm 待办 进度",
        schema: json!({
            "name": "UpdateTodos",
            "description": "Update the lightweight todo list for the current session. Use this for main-agent progress tracking on non-trivial work such as editing several files, running tests, or following a short checklist. This does not create persistent tasks, does not delegate to subagents, and does not participate in dependency scheduling. For complex work needing persistence, dependencies, or subagent execution, use CreateTask/UpdateTask/RunSubagent/RunSubagentsSequentially instead.\n\nRules:\n- Use proactively for tasks with roughly 3+ meaningful steps or multiple files.\n- Do not use for a single trivial task or purely conversational answers.\n- Keep exactly one item in_progress while actively working, unless the list is empty or all work is done.\n- Mark items completed immediately after finishing them.\n- Each item must include both content (imperative form, e.g. \"Run tests\") and activeForm (present continuous form, e.g. \"Running tests\").",
            "input_schema": {
                "type": "object",
                "properties": {
                    "todos": {
                        "type": "array",
                        "description": "The updated todo list for this session.",
                        "items": {
                            "type": "object",
                            "properties": {
                                "id": {
                                    "type": "string",
                                    "description": "Stable optional ID for this todo item. If omitted, one is generated from position and content."
                                },
                                "content": {
                                    "type": "string",
                                    "description": "Imperative form describing what needs to be done, e.g. \"Run tests\"."
                                },
                                "activeForm": {
                                    "type": "string",
                                    "description": "Present continuous form shown while in progress, e.g. \"Running tests\"."
                                },
                                "status": {
                                    "type": "string",
                                    "enum": ["pending", "in_progress", "completed"],
                                    "description": "Current status of the todo item."
                                }
                            },
                            "required": ["content", "activeForm", "status"]
                        }
                    }
                },
                "required": ["todos"]
            }
        }),
        should_defer: true,
        is_read_only: false,
        is_concurrency_safe: false,
        is_enabled: true,
    }
}

fn parse_todo_status(value: &serde_json::Value) -> Result<TodoStatus, String> {
    match value.as_str().unwrap_or("") {
        "pending" => Ok(TodoStatus::Pending),
        "in_progress" => Ok(TodoStatus::InProgress),
        "completed" => Ok(TodoStatus::Completed),
        other => Err(format!(
            "Invalid todo status '{}'. Expected pending, in_progress, or completed.",
            other
        )),
    }
}

fn todo_id_for(item: &serde_json::Value, index: usize, content: &str) -> String {
    item["id"]
        .as_str()
        .filter(|id| !id.trim().is_empty())
        .map(|id| id.to_string())
        .unwrap_or_else(|| {
            let slug: String = content
                .chars()
                .filter(|ch| ch.is_ascii_alphanumeric())
                .take(24)
                .collect();
            if slug.is_empty() {
                format!("todo-{}", index + 1)
            } else {
                format!("todo-{}-{}", index + 1, slug.to_lowercase())
            }
        })
}

fn parse_todos(input: &serde_json::Value) -> Result<Vec<TodoItem>, String> {
    let todos = input["todos"]
        .as_array()
        .ok_or_else(|| "UpdateTodos requires a todos array.".to_string())?;

    let mut parsed = Vec::new();
    let mut in_progress_count = 0usize;
    for (index, item) in todos.iter().enumerate() {
        let content = item["content"]
            .as_str()
            .or_else(|| item["text"].as_str())
            .unwrap_or("")
            .trim();
        if content.is_empty() {
            return Err(format!("Todo item {} is missing content.", index + 1));
        }

        let active_form = item["activeForm"]
            .as_str()
            .or_else(|| item["active_form"].as_str())
            .unwrap_or(content)
            .trim();
        if active_form.is_empty() {
            return Err(format!("Todo item {} is missing activeForm.", index + 1));
        }

        let status = parse_todo_status(&item["status"])?;
        if status == TodoStatus::InProgress {
            in_progress_count += 1;
        }

        parsed.push(TodoItem {
            id: todo_id_for(item, index, content),
            content: content.to_string(),
            active_form: active_form.to_string(),
            status,
        });
    }

    if in_progress_count > 1 {
        return Err("Only one todo item may be in_progress at a time.".to_string());
    }

    Ok(parsed)
}

pub async fn todo_write(
    app: &tauri::AppHandle,
    input: &serde_json::Value,
    session_id: &str,
) -> String {
    let todos = match parse_todos(input) {
        Ok(todos) => todos,
        Err(e) => {
            return serde_json::json!({
                "success": false,
                "error": e
            })
            .to_string();
        }
    };

    let all_done = !todos.is_empty()
        && todos
            .iter()
            .all(|todo| todo.status == TodoStatus::Completed);
    let visible_todos = if all_done { Vec::new() } else { todos.clone() };

    let old_todos = if let Some(manager) = app.try_state::<crate::infra::state::state::SessionManager>() {
        let ctx = manager.get_or_create(session_id).await;
        let mut state = ctx.todos.lock().await;
        let old = state.clone();
        *state = visible_todos.clone();
        old
    } else {
        Vec::new()
    };

    let _ = app.emit(
        "todo-update",
        serde_json::json!({
            "todos": visible_todos,
            "sessionId": session_id,
        }),
    );

    serde_json::json!({
        "success": true,
        "oldTodos": old_todos,
        "newTodos": todos,
        "visibleTodos": visible_todos,
        "result": "Todos have been modified successfully. Continue using UpdateTodos to track progress for this session."
    })
    .to_string()
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn test_parse_todos_requires_single_in_progress() {
        let input = json!({
            "todos": [
                {"content": "Edit file A", "activeForm": "Editing file A", "status": "in_progress"},
                {"content": "Edit file B", "activeForm": "Editing file B", "status": "in_progress"}
            ]
        });
        let err = parse_todos(&input).unwrap_err();
        assert!(err.contains("Only one todo item"));
    }

    #[test]
    fn test_parse_todos_accepts_content_and_active_form() {
        let input = json!({
            "todos": [
                {"id": "one", "content": "Run tests", "activeForm": "Running tests", "status": "pending"}
            ]
        });
        let todos = parse_todos(&input).unwrap();
        assert_eq!(todos.len(), 1);
        assert_eq!(todos[0].id, "one");
        assert_eq!(todos[0].content, "Run tests");
        assert_eq!(todos[0].active_form, "Running tests");
        assert_eq!(todos[0].status, TodoStatus::Pending);
    }

    #[test]
    fn test_parse_todos_keeps_legacy_text_compatibility() {
        let input = json!({
            "todos": [
                {"text": "Build project", "status": "completed"}
            ]
        });
        let todos = parse_todos(&input).unwrap();
        assert_eq!(todos[0].content, "Build project");
        assert_eq!(todos[0].active_form, "Build project");
    }
}
