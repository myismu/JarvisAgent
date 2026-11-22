use crate::core::tools::framework;
use crate::core::tools::framework::registry::ToolDef;
use crate::infra::types::models::{TodoGroup, TodoItem, TodoStatus};
use serde_json::json;
use tauri::{Emitter, Manager};

pub(super) fn tool_def() -> ToolDef {
    ToolDef {
        name: "UpdateTodos",
        description: "更新当前会话的轻量待办清单，用于主 Agent 自己执行时展示进度",
        search_hint: "todo checklist progress lightweight task list activeForm 待办 进度",
        category: "任务管理",
        schema: json!({
            "name": "UpdateTodos",
            "description": "Update the lightweight todo list for the current session. Use this for main-agent progress tracking on non-trivial work such as editing several files, running tests, or following a short checklist. This does not create persistent tasks, does not delegate to subagents, and does not participate in dependency scheduling. For complex work needing persistence, dependencies, or subagent execution, use CreateTask/UpdateTask/RunSubagent/RunSubagentsSequentially instead.\n\nRules:\n- Use proactively for tasks with roughly 3+ meaningful steps or multiple files — even when the path and content are already clear, list the plan first so the user can see what you are about to do.\n- Do not use for a single trivial task or purely conversational answers.\n- Keep exactly one item in_progress while actively working, unless the list is empty or all work is done.\n- Mark items completed immediately after finishing them.\n- Each item must include both content (imperative form, e.g. \"Run tests\") and activeForm (present continuous form, e.g. \"Running tests\").",
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
            // 进度只由调度器写，模型输入里即便带了也不采信
            progress: None,
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
) -> framework::ToolCallResult {
    let todos = match parse_todos(input) {
        Ok(todos) => todos,
        Err(e) => {
            return framework::ToolCallResult::error(
                serde_json::json!({
                    "success": false,
                    "error": e
                })
                .to_string(),
            );
        }
    };

    let mut replaced_group: Option<String> = None;
    let mut groups_snapshot: Vec<TodoGroup> = Vec::new();

    if let Some(manager) = app.try_state::<crate::infra::state::state::SessionManager>() {
        let ctx = manager.get_or_create(session_id).await;
        let mut groups = ctx.todos.lock().await;

        // 归属判定：传入条目的 id 与哪一组有交集，就更新哪一组（组内替换）；
        // 没有交集则新建一组追加。这是“追加”与“状态更新”能共存的关键——
        // 纯 push 会让改状态变成重复条目，纯覆盖则会丢历史清单。
        let incoming: std::collections::HashSet<&str> =
            todos.iter().map(|t| t.id.as_str()).collect();
        let target = groups
            .iter()
            .position(|g| g.items.iter().any(|i| incoming.contains(i.id.as_str())));

        match target {
            Some(idx) => {
                // 调度器写入的进度不在模型输入里，合并时不能被冲掉
                let mut merged = todos;
                for item in &mut merged {
                    if item.progress.is_none() {
                        if let Some(old) = groups[idx].items.iter().find(|o| o.id == item.id) {
                            item.progress = old.progress.clone();
                        }
                    }
                }
                replaced_group = Some(groups[idx].id.clone());
                groups[idx].items = merged;
            }
            None => {
                let id = todos
                    .first()
                    .map(|t| t.id.clone())
                    .unwrap_or_else(|| format!("group-{}", groups.len() + 1));
                replaced_group = Some(id.clone());
                groups.push(TodoGroup { id, items: todos });
            }
        }

        groups_snapshot = groups.clone();
    }

    // 不再“全完成即清空”：已完成的组也要推给前端，否则前端无从折叠展示
    let flat: Vec<TodoItem> = groups_snapshot
        .iter()
        .flat_map(|g| g.items.iter().cloned())
        .collect();

    let _ = app.emit(
        "todo-update",
        serde_json::json!({
            "todos": flat,
            "groups": groups_snapshot,
            "sessionId": session_id,
        }),
    );

    framework::ToolCallResult::ok(
        serde_json::json!({
            "success": true,
            "groupId": replaced_group,
            "newTodos": flat,
            "groups": groups_snapshot,
            "result": "Todos have been modified successfully. Continue using UpdateTodos to track progress for this session."
        })
        .to_string(),
    )
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
