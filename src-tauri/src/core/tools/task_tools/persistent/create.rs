use super::common::optional_string;
use crate::core::orchestration::tasks::TaskManager;
use crate::core::tools::framework::registry::ToolDef;
use serde_json::json;

pub(super) fn tool_def() -> ToolDef {
    ToolDef {
        name: "CreateTask",
        description: "创建持久化任务条目到任务看板",
        search_hint: "create task todo 创建 任务",
        category: "任务管理",
        schema: json!({
            "name": "CreateTask",
            "description": "创建持久化任务条目。支持批量创建：传 tasks 数组可一次创建多个任务并自动建立依赖关系，代替多次调用。\n\n单个任务示例：{\"subject\": \"修复登录Bug\", \"description\": \"...\"}\n批量任务示例：{\"tasks\": [{\"subject\": \"初始化后端\", \"depends_on\": []}, {\"subject\": \"实现API\", \"depends_on\": [1]}]}",
            "input_schema": {
                "type": "object",
                "properties": {
                    "subject": {"type": "string", "description": "单个任务标题。与 tasks 二选一。"},
                    "description": {"type": "string", "description": "单个任务描述。"},
                    "activeForm": {"type": "string", "description": "进行时描述，如\"修复登录Bug中\"。"},
                    "metadata": {"type": "object", "description": "可选元数据。"},
                    "owner": {"type": "string", "description": "负责人名称。"},
                    "tasks": {"type": "array", "items": {"type": "object", "properties": {
                        "subject": {"type": "string"},
                        "description": {"type": "string"},
                        "activeForm": {"type": "string"},
                        "depends_on": {"type": "array", "items": {"type": "integer"}, "description": "依赖的任务在数组中的 1-based 索引"},
                        "owner": {"type": "string"}
                    }, "required": ["subject"]}, "description": "批量创建的任务列表。每项的 depends_on 为数组内索引。"}
                }
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
    // 批量模式
    if let Some(tasks) = input["tasks"].as_array() {
        if tasks.is_empty() {
            return serde_json::json!({"success": false, "error": "tasks 数组不能为空"}).to_string();
        }
        let tm = TaskManager::for_session(session_id);
        let mut created_ids: Vec<i32> = Vec::new();
        for item in tasks {
            let subject = item["subject"].as_str().unwrap_or("").to_string();
            if subject.trim().is_empty() { continue; }
            let description = item["description"].as_str().unwrap_or("").to_string();
            let active_form = optional_string(item, "activeForm");
            let metadata = item.get("metadata").cloned();
            let owner = optional_string(item, "owner");
            match tm.create(subject, description, active_form, metadata, owner) {
                Ok(t) => created_ids.push(t.id),
                Err(_) => {}
            }
        }
        // 处理 depends_on 依赖
        for (idx, item) in tasks.iter().enumerate() {
            if let Some(task_id) = created_ids.get(idx) {
                if let Some(deps) = item["depends_on"].as_array() {
                    let blocked_by: Vec<i32> = deps.iter()
                        .filter_map(|d| d.as_u64().map(|n| (n as usize).saturating_sub(1)))
                        .filter_map(|di| created_ids.get(di).copied())
                        .map(|id| id as i32)
                        .collect();
                    if !blocked_by.is_empty() {
                        let _ = tm.update(*task_id, crate::core::orchestration::tasks::TaskUpdateParams {
                            status: None, subject: None, description: None, active_form: None,
                            owner: None, metadata: None, add_blocked_by: Some(blocked_by),
                            add_blocks: None,
                        });
                    }
                }
            }
        }
        return serde_json::json!({"success": true, "created": created_ids.len()}).to_string();
    }

    // 单个模式
    let subject = input["subject"].as_str().unwrap_or("").to_string();
    if subject.trim().is_empty() {
        return serde_json::json!({
            "success": false,
            "error": "subject 不能为空 — 每个任务必须有明确标题，例如「实现 /api/users POST 路由」"
        }).to_string();
    }
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
