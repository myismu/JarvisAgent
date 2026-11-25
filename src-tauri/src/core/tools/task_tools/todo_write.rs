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
            "description": "Update the lightweight todo list for the current session. Use this for main-agent progress tracking on non-trivial work such as editing several files, running tests, or following a short checklist. This does not create persistent tasks, does not delegate to subagents, and does not participate in dependency scheduling. For complex work needing persistence, dependencies, or subagent execution, use CreateTask/UpdateTask/RunSubagent/RunSubagentsSequentially instead.\n\nRules:\n- Use proactively for tasks with roughly 3+ meaningful steps or multiple files — even when the path and content are already clear, list the plan first so the user can see what you are about to do.\n- Do not use for a single trivial task or purely conversational answers.\n- Keep exactly one item in_progress while actively working, unless the list is empty or all work is done.\n- Mark items completed immediately after finishing them.\n- Each item must include both content (imperative form, e.g. \"Run tests\") and activeForm (present continuous form, e.g. \"Running tests\").\n- Finish ordering (hard rule): bring the todo list to its final state BEFORE writing the final answer. The final answer must be the last action of the turn; never call UpdateTodos (or any other tool) after it.",
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

/// 把一次 UpdateTodos 的条目并入清单组，返回本次归属的组 id。
///
/// 归属判定：传入条目的 id 与哪一组有交集，就更新哪一组（组内整体替换）；
/// 没有交集则新建一组追加。这是「追加」与「状态更新」能共存的关键——
/// 纯 push 会让改状态变成重复条目，纯覆盖则会丢历史清单。
///
/// 抽成纯函数是为了可测：`todo_write` 本体要 `AppHandle` 与 `SessionManager`，
/// 单测里造不出来，而这段归属判定恰恰是本工具唯一有分支的语义。
fn apply_todo_update(groups: &mut Vec<TodoGroup>, todos: Vec<TodoItem>) -> Option<String> {
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
            let id = groups[idx].id.clone();
            groups[idx].items = merged;
            Some(id)
        }
        None => {
            let id = todos
                .first()
                .map(|t| t.id.clone())
                .unwrap_or_else(|| format!("group-{}", groups.len() + 1));
            groups.push(TodoGroup { id: id.clone(), items: todos });
            Some(id)
        }
    }
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

/// 工具返回给模型的收尾提示。
///
/// 全部完成时给出**有条件**的收尾建议，而不是强制结束：
/// - 清单勾完不等于任务真的完成，模型仍要自己判断实际工作量；
/// - 确实完成时才停止调用本工具、直接输出最终正文，避免正文后补一次 UpdateTodos
///   导致正文被前端当成执行过程折叠。
fn todo_result_text(all_completed: bool) -> &'static str {
    if all_completed {
        "待办清单已全部标记为完成。请先确认任务是否真的已经实际完成：\
         如果确实完成，不要再调用 UpdateTodos，直接输出最终回答，且最终正文之后不要再调用任何工具；\
         如果仍有未完成的工作，请继续执行，不要仅因为清单已全部勾选就提前结束。"
    } else {
        "Todos have been modified successfully. Continue using UpdateTodos to track progress for this session."
    }
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

    // 本轮提交的清单是否已全部完成。用于收尾提示：全部完成后不要再让模型多调一次
    // 本工具，否则正文之后会多出一个工具调用，前端会把正文当成执行过程折叠起来。
    let all_completed = todos
        .iter()
        .all(|todo| todo.status == TodoStatus::Completed);

    let mut replaced_group: Option<String> = None;
    let mut groups_snapshot: Vec<TodoGroup> = Vec::new();

    if let Some(manager) = app.try_state::<crate::infra::state::state::SessionManager>() {
        let ctx = manager.get_or_create(session_id).await;
        let mut groups = ctx.todos.lock().await;
        replaced_group = apply_todo_update(&mut groups, todos);
        groups_snapshot = groups.clone();
    }

    // 不再“全完成即清空”：已完成的组也要推给前端，否则前端无从折叠展示
    let flat: Vec<TodoItem> = groups_snapshot
        .iter()
        .flat_map(|g| g.items.iter().cloned())
        .collect();

    let _ = app.emit(
        "todo-update",
        crate::infra::types::events::TodoUpdateEvent {
            todos: flat.clone(),
            groups: groups_snapshot.clone(),
            session_id: session_id.to_string(),
        },
    );

    framework::ToolCallResult::ok(
        serde_json::json!({
            "success": true,
            "groupId": replaced_group,
            "newTodos": flat,
            "groups": groups_snapshot,
            // 全部完成时明确要求收尾：本轮不要再调任何工具，直接输出最终正文。
            // 正文必须是最后一个动作，否则它会被当作中间叙述折叠进执行过程。
            "result": todo_result_text(all_completed)
        })
        .to_string(),
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::infra::types::models::TodoProgress;
    use serde_json::json;

    fn item(id: &str, status: TodoStatus) -> TodoItem {
        TodoItem {
            id: id.to_string(),
            content: format!("task {}", id),
            active_form: format!("doing {}", id),
            status,
            progress: None,
        }
    }

    fn group(id: &str, items: Vec<TodoItem>) -> TodoGroup {
        TodoGroup {
            id: id.to_string(),
            items,
        }
    }

    #[test]
    fn update_with_overlapping_ids_replaces_that_group() {
        let mut groups = vec![group(
            "g1",
            vec![
                item("a", TodoStatus::Pending),
                item("b", TodoStatus::Pending),
            ],
        )];

        let replaced = apply_todo_update(
            &mut groups,
            vec![
                item("a", TodoStatus::Completed),
                item("b", TodoStatus::Pending),
            ],
        );

        assert_eq!(replaced.as_deref(), Some("g1"));
        assert_eq!(groups.len(), 1, "命中已有组时不应新建");
        assert_eq!(groups[0].items.len(), 2);
        assert_eq!(groups[0].items[0].status, TodoStatus::Completed);
    }

    #[test]
    fn update_without_overlap_appends_new_group() {
        let mut groups = vec![group("g1", vec![item("a", TodoStatus::Completed)])];

        let replaced = apply_todo_update(&mut groups, vec![item("c", TodoStatus::Pending)]);

        assert_eq!(replaced.as_deref(), Some("c"), "新组 id 取首条目 id");
        assert_eq!(groups.len(), 2);
        assert_eq!(groups[0].items.len(), 1, "旧组不应被覆盖");
        assert_eq!(groups[1].items[0].id, "c");
    }

    #[test]
    fn progress_survives_model_update() {
        let mut existing = item("a", TodoStatus::InProgress);
        existing.progress = Some(TodoProgress { done: 2, total: 5 });
        let mut groups = vec![group("g1", vec![existing])];

        // 模型重发同一批条目时不会带 progress（parse_todos 恒置 None）
        apply_todo_update(&mut groups, vec![item("a", TodoStatus::InProgress)]);

        assert_eq!(
            groups[0].items[0].progress,
            Some(TodoProgress { done: 2, total: 5 }),
            "调度器写入的进度不能被模型输入冲掉"
        );
    }

    #[test]
    fn all_done_group_is_not_cleared() {
        let mut groups = vec![group("g1", vec![item("a", TodoStatus::Completed)])];

        apply_todo_update(&mut groups, vec![item("b", TodoStatus::Completed)]);

        assert_eq!(groups.len(), 2, "已完成的组也要保留，否则前端无从折叠展示");
        assert_eq!(groups[0].items.len(), 1);
        assert!(groups
            .iter()
            .all(|g| g.items.iter().all(|i| i.status == TodoStatus::Completed)));
    }

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

    /// 回归锁：全部完成后必须明确阻止模型再调一次本工具。
    ///
    /// 真实事故：模型先输出收尾正文，再补一次 UpdateTodos，前端按“text 后面还有工具”
    /// 把整段正文判定为中间叙述折叠掉，用户看不到结论。
    #[test]
    fn all_completed_result_tells_model_to_finish() {
        let done = todo_result_text(true);
        assert!(done.contains("确认任务是否真的已经实际完成"));
        assert!(done.contains("不要再调用 UpdateTodos"));
        assert!(done.contains("如果仍有未完成的工作，请继续执行"));

        let pending = todo_result_text(false);
        assert!(pending.contains("Continue using UpdateTodos"));
    }
}
