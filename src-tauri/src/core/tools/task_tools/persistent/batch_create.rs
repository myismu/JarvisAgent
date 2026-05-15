use super::common::optional_string;
use crate::core::orchestration::tasks::{TaskManager, TaskUpdateParams};
use crate::core::tools::framework::registry::ToolDef;
use serde_json::json;

pub(super) fn tool_def() -> ToolDef {
    ToolDef {
        name: "BatchCreateTasks",
        description: "批量创建任务并自动建立依赖",
        search_hint: "batch create tasks 批量 创建 任务 依赖",
        schema: json!({
            "name": "BatchCreateTasks",
            "description": "Create multiple tasks at once with automatic dependency setup.\n\nProvide a `tasks` array where each entry has:\n- subject (required): short imperative title\n- description (optional): detailed description\n- activeForm (optional): present-continuous text\n- depends_on (optional): array of 1-based indices within this tasks array (e.g. [1, 2] means this task depends on tasks #1 and #2 in the same batch)\n- owner (optional): responsible agent name\n\nThis replaces N individual CreateTask + M UpdateTask calls with a single round trip. All tasks and their dependencies are created atomically.\n\nExample:\n```json\n{\n  \"tasks\": [\n    {\"subject\": \"初始化后端项目\", \"description\": \"...\"},\n    {\"subject\": \"初始化前端项目\", \"description\": \"...\"},\n    {\"subject\": \"实现后端API\", \"description\": \"...\", \"depends_on\": [1]},\n    {\"subject\": \"实现前端页面\", \"description\": \"...\", \"depends_on\": [2, 3]}\n  ]\n}\n```\n\nAfter creation, call RunSubagentsSequentially to start execution.",
            "input_schema": {
                "type": "object",
                "properties": {
                    "tasks": {
                        "type": "array",
                        "items": {
                            "type": "object",
                            "properties": {
                                "subject": {"type": "string", "description": "Short imperative task title."},
                                "description": {"type": "string", "description": "Detailed description of the work to complete."},
                                "activeForm": {"type": "string", "description": "Present-continuous text shown while active."},
                                "depends_on": {"type": "array", "items": {"type": "integer"}, "description": "1-based indices within this tasks array that must complete first."},
                                "owner": {"type": "string", "description": "Responsible agent name."}
                            },
                            "required": ["subject"]
                        }
                    }
                },
                "required": ["tasks"]
            }
        }),
        should_defer: true,
        is_read_only: false,
        is_concurrency_safe: false,
        is_enabled: true,
    }
}

pub async fn task_batch_create(
    _app: &tauri::AppHandle,
    input: &serde_json::Value,
    session_id: &str,
) -> String {
    let tasks = match input["tasks"].as_array() {
        Some(arr) if !arr.is_empty() => arr,
        _ => return json!({"success": false, "error": "tasks 数组不能为空"}).to_string(),
    };

    let n = tasks.len();
    let mut created_ids: Vec<i32> = Vec::with_capacity(n);
    let mut subjects: Vec<String> = Vec::with_capacity(n);
    let tm = TaskManager::for_session(session_id);

    // 第一遍：校验 + 创建所有任务，记录 ID 映射
    for (i, task) in tasks.iter().enumerate() {
        let subject = task["subject"].as_str().unwrap_or("").to_string();
        if subject.trim().is_empty() {
            return json!({
                "success": false,
                "error": format!("第 {} 个任务的 subject 不能为空", i + 1),
            }).to_string();
        }

        // 校验 depends_on 序号
        if let Some(deps) = task["depends_on"].as_array() {
            for dep in deps {
                if let Some(dep_num) = dep.as_u64() {
                    let dep_num = dep_num as usize;
                    if dep_num < 1 || dep_num > n {
                        return json!({
                            "success": false,
                            "error": format!("任务「{}」的 depends_on 引用了不存在的序号 {}（共 {} 个任务，序号范围 1~{}）", subject, dep_num, n, n)
                        }).to_string();
                    }
                    if dep_num == i + 1 {
                        return json!({
                            "success": false,
                            "error": format!("任务「{}」的 depends_on 不能包含自身", subject)
                        }).to_string();
                    }
                }
            }
        }

        let description = task["description"].as_str().unwrap_or("").to_string();
        let active_form = optional_string(task, "activeForm");
        let metadata = task.get("metadata").cloned();
        let owner = optional_string(task, "owner");

        match tm.create(subject.clone(), description, active_form, metadata, owner) {
            Ok(created) => {
                created_ids.push(created.id);
                subjects.push(created.subject);
            }
            Err(e) => {
                return json!({"success": false, "error": e}).to_string();
            }
        }
    }

    // 第二遍：建立依赖关系（此时所有 ID 已知）
    let mut dep_count = 0u32;
    for (i, task) in tasks.iter().enumerate() {
        if let Some(deps) = task["depends_on"].as_array() {
            if !deps.is_empty() {
                let blocked_by: Vec<i32> = deps.iter()
                    .filter_map(|d| d.as_u64())
                    .map(|d| created_ids[d as usize - 1])
                    .collect();
                if !blocked_by.is_empty() {
                    let _ = tm.update(created_ids[i], TaskUpdateParams {
                        status: None,
                        subject: None, description: None, active_form: None,
                        owner: None,
                        add_blocked_by: Some(blocked_by),
                        add_blocks: None, metadata: None,
                    });
                    dep_count += 1;
                }
            }
        }
    }

    let task_list: Vec<serde_json::Value> = created_ids.iter().enumerate().map(|(i, &id)| {
        json!({"id": id, "subject": subjects[i]})
    }).collect();

    json!({
        "success": true,
        "created": created_ids.len(),
        "dependenciesSet": dep_count,
        "tasks": task_list,
        "result": format!("批量创建了 {} 个任务，{} 个任务设置了依赖关系。请调用 RunSubagentsSequentially 启动调度。", created_ids.len(), dep_count)
    }).to_string()
}
