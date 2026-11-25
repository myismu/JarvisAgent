//! # todo_sync.rs — 方案任务到 TODO 清单的显示投影
//!
//! 设计边界（重要）：
//! - 调度器仍以 SQLite 里的 `TaskManager` 任务图为**唯一执行事实**（依赖、状态、恢复）；
//! - TODO 只是用户可见的**内存态**清单，关软件即清空；
//! - 方案审批通过时把任务图投影成一个 TODO 组，调度器状态变化时同步更新。
//!
//! 这样用户只看到一套清单（TodoPanel），不会再出现 TASKS 卡片与 TODO 并存。

use std::collections::HashMap;

use tauri::{Emitter, Manager};

use crate::infra::state::state::{SessionContext, SessionManager};
use crate::infra::types::models::{Task, TaskStatus, TodoGroup, TodoItem, TodoStatus};

/// 方案对应的 TODO 组 id。`plan:` 前缀避免与模型生成的 todo id 冲突。
pub fn plan_group_id(plan_id: &str) -> String {
    format!("plan:{}", plan_id)
}

/// 任务对应的 TODO 条目 id。
fn todo_item_id(task_id: i32) -> String {
    format!("task:{}", task_id)
}

fn map_task_status(status: &TaskStatus) -> TodoStatus {
    match status {
        TaskStatus::Pending => TodoStatus::Pending,
        TaskStatus::InProgress => TodoStatus::InProgress,
        TaskStatus::Completed => TodoStatus::Completed,
    }
}

/// 把任务图映射成 TODO 条目，顺序与任务 id 一致（即建图顺序）。
pub fn plan_items_from_tasks(tasks: &[Task]) -> Vec<TodoItem> {
    tasks
        .iter()
        .map(|task| TodoItem {
            id: todo_item_id(task.id),
            content: task.subject.clone(),
            active_form: task
                .active_form
                .clone()
                .unwrap_or_else(|| task.subject.clone()),
            status: map_task_status(&task.status),
            progress: None,
        })
        .collect()
}

/// 幂等写入方案对应的 TODO 组。
///
/// 重复审批时保留已有条目的状态（避免把已经 completed 的任务打回 pending），
/// 同时用最新任务列表覆盖组内条目（方案修订后不会留下旧条目）。
pub fn upsert_plan_group(groups: &mut Vec<TodoGroup>, plan_id: &str, tasks: &[Task]) {
    let group_id = plan_group_id(plan_id);
    let previous: HashMap<String, TodoStatus> = groups
        .iter()
        .find(|group| group.id == group_id)
        .map(|group| {
            group
                .items
                .iter()
                .map(|item| (item.id.clone(), item.status.clone()))
                .collect()
        })
        .unwrap_or_default();

    let mut items = plan_items_from_tasks(tasks);
    for item in &mut items {
        if let Some(status) = previous.get(&item.id) {
            item.status = status.clone();
        }
    }

    match groups.iter_mut().find(|group| group.id == group_id) {
        Some(group) => group.items = items,
        None => groups.push(TodoGroup {
            id: group_id,
            items,
        }),
    }
}

/// 更新单个任务对应的 TODO 条目状态。返回是否真的发生了变化。
pub fn set_task_status(groups: &mut [TodoGroup], task_id: i32, status: TodoStatus) -> bool {
    let item_id = todo_item_id(task_id);
    for group in groups.iter_mut() {
        if let Some(item) = group.items.iter_mut().find(|item| item.id == item_id) {
            if item.status == status {
                return false;
            }
            item.status = status;
            return true;
        }
    }
    false
}

/// 把方案任务图投影到会话的 TODO 组，并广播 `todo-update`。
pub async fn mirror_plan_tasks(
    app: &tauri::AppHandle,
    ctx: &SessionContext,
    session_id: &str,
    plan_id: &str,
    tasks: &[Task],
) {
    let groups = {
        let mut groups = ctx.todos.lock().await;
        upsert_plan_group(&mut groups, plan_id, tasks);
        groups.clone()
    };
    emit_todo_update(app, session_id, &groups);
}

/// 调度器更新任务状态后，同步更新对应的 TODO 条目。
pub async fn update_task_status(
    app: &tauri::AppHandle,
    session_id: &str,
    task_id: i32,
    status: TodoStatus,
) {
    let Some(manager) = app.try_state::<SessionManager>() else {
        return;
    };
    let ctx = manager.get_or_create(session_id).await;
    let groups = {
        let mut groups = ctx.todos.lock().await;
        if !set_task_status(&mut groups, task_id, status) {
            return;
        }
        groups.clone()
    };
    emit_todo_update(app, session_id, &groups);
}

fn emit_todo_update(app: &tauri::AppHandle, session_id: &str, groups: &[TodoGroup]) {
    let flat: Vec<TodoItem> = groups
        .iter()
        .flat_map(|group| group.items.iter().cloned())
        .collect();
    let _ = app.emit(
        "todo-update",
        crate::infra::types::events::TodoUpdateEvent {
            todos: flat,
            groups: groups.to_vec(),
            session_id: session_id.to_string(),
        },
    );
}

#[cfg(test)]
mod tests {
    use super::*;

    fn task(id: i32, subject: &str, status: TaskStatus) -> Task {
        Task {
            id,
            subject: subject.to_string(),
            description: String::new(),
            status,
            blocked_by: Vec::new(),
            blocks: Vec::new(),
            owner: String::new(),
            active_form: None,
            metadata: None,
            subagent_type: None,
        }
    }

    #[test]
    fn plan_items_keep_task_order_and_status() {
        let tasks = vec![
            task(3, "#1 建库", TaskStatus::Pending),
            task(7, "#2 写接口", TaskStatus::InProgress),
        ];

        let items = plan_items_from_tasks(&tasks);
        assert_eq!(items.len(), 2);
        assert_eq!(items[0].id, "task:3");
        assert_eq!(items[1].id, "task:7");
        assert_eq!(items[1].status, TodoStatus::InProgress);
        assert_eq!(items[1].active_form, "#2 写接口");
    }

    /// 重复审批不能把已完成的条目打回 pending。
    #[test]
    fn upsert_preserves_completed_status() {
        let mut groups = Vec::new();
        upsert_plan_group(
            &mut groups,
            "plan_1",
            &[task(1, "#1 建库", TaskStatus::Completed)],
        );

        upsert_plan_group(
            &mut groups,
            "plan_1",
            &[task(1, "#1 建库", TaskStatus::Pending)],
        );

        assert_eq!(groups.len(), 1, "同一方案只能有一个 TODO 组");
        assert_eq!(groups[0].items[0].status, TodoStatus::Completed);
    }

    #[test]
    fn scheduler_status_update_hits_matching_item() {
        let mut groups = Vec::new();
        upsert_plan_group(
            &mut groups,
            "plan_1",
            &[task(12, "#3 联调", TaskStatus::Pending)],
        );

        assert!(set_task_status(&mut groups, 12, TodoStatus::InProgress));
        assert_eq!(groups[0].items[0].status, TodoStatus::InProgress);
        // 状态没变时返回 false，避免重复广播
        assert!(!set_task_status(&mut groups, 12, TodoStatus::InProgress));
        // 不存在的任务不动任何组
        assert!(!set_task_status(&mut groups, 99, TodoStatus::Completed));
    }
}
