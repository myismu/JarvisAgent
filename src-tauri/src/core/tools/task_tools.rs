//! # task_tools.rs — 任务管理工具模块
//!
//! 提供任务看板的 CRUD 操作和全景报告工具。
//! 任务支持状态流转（pending → in_progress → completed）、依赖关系和级联解锁。
//!
//! ## 关键导出
//! - `task_create()`: 创建持久化任务
//! - `task_update()`: 更新任务状态/依赖关系（status=deleted 路由到 delete）
//! - `task_delete()`: 永久删除任务
//! - `task_list()`: 列出所有任务
//! - `task_get()`: 获取单个任务详情
//! - `task_summary()`: 生成任务全景报告
//!
//! ## 依赖
//! - Internal: `crate::core::orchestration::tasks::TaskManager`
//! - External: `serde_json`
//!
//! ## 约束
//! - 任务完成时会提示 LLM 调用 task_list 查找下一个可用任务
//! - `task_update` 的 status=deleted 会自动路由到 `task_delete_inner`

use serde_json::json;
use crate::core::models::TaskStatus;
use crate::core::orchestration::tasks::{TaskManager, TaskUpdateParams};
use crate::core::tools::registry::ToolDef;

/// 创建任务
pub async fn task_create(
    _app: &tauri::AppHandle,
    input: &serde_json::Value,
    _session_id: &str,
) -> String {
    let subject = input["subject"].as_str().unwrap_or("").to_string();
    let description = input["description"].as_str().unwrap_or("").to_string();
    let active_form = input["activeForm"].as_str().map(|s| s.to_string());
    let metadata = input.get("metadata").cloned();
    let owner = input["owner"].as_str().map(|s| s.to_string());

    match TaskManager::new().create(subject, description, active_form, metadata, owner) {
        Ok(task) => serde_json::json!({
            "success": true,
            "task": {
                "id": task.id,
                "subject": task.subject,
            }
        }).to_string(),
        Err(e) => serde_json::json!({
            "success": false,
            "error": e
        }).to_string(),
    }
}

/// 更新任务
pub async fn task_update(
    _app: &tauri::AppHandle,
    input: &serde_json::Value,
    _session_id: &str,
) -> String {
    let id = input["task_id"].as_i64().unwrap_or(0) as i32;

    // status='deleted' 路由到 delete
    if input["status"].as_str() == Some("deleted") {
        return task_delete_inner(id);
    }

    let status = input["status"].as_str().map(|s| match s {
        "in_progress" => TaskStatus::InProgress,
        "completed" => TaskStatus::Completed,
        _ => TaskStatus::Pending,
    });
    let subject = input["subject"].as_str().map(|s| s.to_string());
    let description = input["description"].as_str().map(|s| s.to_string());
    let active_form = input["activeForm"].as_str().map(|s| s.to_string());
    let owner = input["owner"].as_str().map(|s| s.to_string());
    let add_blocked_by = input["add_blocked_by"].as_array().map(|arr| {
        arr.iter()
            .filter_map(|v| v.as_i64().map(|i| i as i32))
            .collect()
    });
    let add_blocks = input["add_blocks"].as_array().map(|arr| {
        arr.iter()
            .filter_map(|v| v.as_i64().map(|i| i as i32))
            .collect()
    });
    let metadata = input.get("metadata").cloned();

    let params = TaskUpdateParams {
        status,
        subject,
        description,
        active_form,
        owner,
        add_blocked_by,
        add_blocks,
        metadata,
    };

    match TaskManager::new().update(id, params) {
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
            // 任务完成时提示 LLM 查找下一个可用任务
            if let Some(ref sc) = result.status_change {
                if sc.to == "completed" {
                    let reminder = "\n\nTask completed. Call task_list now to find your next available task or see if your work unblocked others.";
                    output["result"] = serde_json::Value::String(
                        format!("Updated task #{} {}", result.task_id, result.updated_fields.join(", ")) + reminder
                    );
                }
            }
            if let Some(ref cascade) = result.cascade_message {
                output["cascadeMessage"] = serde_json::Value::String(cascade.clone());
            }
            // 如果没有 result 字段，生成默认的
            if output.get("result").is_none() {
                output["result"] = serde_json::Value::String(
                    format!("Updated task #{} {}", result.task_id, result.updated_fields.join(", "))
                );
            }
            output.to_string()
        }
        Err(e) => serde_json::json!({
            "success": false,
            "taskId": id,
            "updatedFields": [],
            "error": e
        }).to_string(),
    }
}

/// 删除任务
pub async fn task_delete(
    _app: &tauri::AppHandle,
    input: &serde_json::Value,
    _session_id: &str,
) -> String {
    let id = input["task_id"].as_i64().unwrap_or(0) as i32;
    task_delete_inner(id)
}

fn task_delete_inner(id: i32) -> String {
    match TaskManager::new().delete(id) {
        Ok(deleted) => serde_json::json!({
            "success": deleted,
            "taskId": id,
            "updatedFields": ["deleted"],
            "statusChange": { "from": "unknown", "to": "deleted" },
        }).to_string(),
        Err(e) => serde_json::json!({
            "success": false,
            "taskId": id,
            "error": e
        }).to_string(),
    }
}

/// 列出所有任务
pub async fn task_list(
    _app: &tauri::AppHandle,
    _input: &serde_json::Value,
    _session_id: &str,
) -> String {
    TaskManager::new().list_all().unwrap_or_else(|e| e)
}

/// 获取单个任务详情
pub async fn task_get(
    _app: &tauri::AppHandle,
    input: &serde_json::Value,
    _session_id: &str,
) -> String {
    let id = input["task_id"].as_i64().unwrap_or(0) as i32;
    match TaskManager::new().get(id) {
        Ok(task) => serde_json::to_string_pretty(&task).unwrap_or_default(),
        Err(e) => e,
    }
}

/// 生成任务全景报告
pub async fn task_summary(
    _app: &tauri::AppHandle,
    _input: &serde_json::Value,
    _session_id: &str,
) -> String {
    TaskManager::new().summary().unwrap_or_else(|e| e)
}

// --- 工具注册 ---
// 将工具 schema 和元数据定义在工具实现文件中（而非集中在 tool_search.rs）
crate::define_tools! {
    pub fn register_tools(registry) {
        ToolDef {
            name: "task_create",
            description: "创建持久化任务条目到任务看板",
            search_hint: "create task todo",
            schema: json!({
                "name": "task_create",
                "description": "创建一个持久化任务条目到任务看板。用于跟踪复杂多步骤任务的进度。\n\n适用场景：\n- 复杂任务需要 3 个以上步骤\n- 用户提供了多个待办事项\n- 需要组织和展示工作进度\n\n不适用：\n- 只有单个简单任务\n- 纯对话性任务\n\n提示：\n- 创建后用 task_update 设置依赖关系（addBlockedBy/addBlocks）\n- 用 task 查看已有任务避免重复创建\n- 此工具仅创建记录，不会执行任何实际操作！要真正执行任务请使用 task 工具委派子代理。",
                "input_schema": {
                    "type": "object",
                    "properties": {
                        "subject": {"type": "string", "description": "任务简述，使用祈使句（如 'Fix authentication bug'）"},
                        "description": {"type": "string", "description": "详细说明需要完成的工作"},
                        "activeForm": {"type": "string", "description": "进行中时显示的动态文本（如 'Fixing authentication bug'）"},
                        "metadata": {"type": "object", "description": "任意附加元数据键值对"},
                        "owner": {"type": "string", "description": "任务负责人（agent 名称）"}
                    },
                    "required": ["subject"]
                }
            }),
            should_defer: true,
            is_read_only: false,
            is_concurrency_safe: false,
            is_enabled: true,
        },
        ToolDef {
            name: "task_update",
            description: "更新任务状态或依赖关系",
            search_hint: "update task status progress",
            schema: json!({
                "name": "task_update",
                "description": "更新任务状态或依赖关系。\n\n状态流转：pending → in_progress → completed\n设为 deleted 可永久删除任务。\n\n可更新字段：\n- status: 任务状态\n- subject: 任务标题\n- description: 任务描述\n- activeForm: 进行中显示文本\n- owner: 负责人\n- metadata: 合并元数据（设 null 删除 key）\n- addBlockedBy: 添加前置依赖\n- addBlocks: 添加后续阻塞\n\n重要：\n- 只有完全完成时才标记 completed\n- 遇到错误/阻塞时保持 in_progress\n- 更新前先用 task_get 确认最新状态",
                "input_schema": {
                    "type": "object",
                    "properties": {
                        "task_id": {"type": "integer", "description": "任务 ID"},
                        "status": {"type": "string", "enum": ["pending", "in_progress", "completed", "deleted"], "description": "新状态"},
                        "subject": {"type": "string", "description": "新标题"},
                        "description": {"type": "string", "description": "新描述"},
                        "activeForm": {"type": "string", "description": "进行中显示文本"},
                        "owner": {"type": "string", "description": "负责人"},
                        "metadata": {"type": "object", "description": "要合并的元数据"},
                        "add_blocked_by": {"type": "array", "items": {"type": "integer"}, "description": "添加前置依赖任务 ID"},
                        "add_blocks": {"type": "array", "items": {"type": "integer"}, "description": "添加后续阻塞任务 ID"}
                    },
                    "required": ["task_id"]
                }
            }),
            should_defer: true,
            is_read_only: false,
            is_concurrency_safe: false,
            is_enabled: true,
        },
        ToolDef {
            name: "task_delete",
            description: "永久删除一个任务",
            search_hint: "delete remove task",
            schema: json!({
                "name": "task_delete",
                "description": "永久删除一个任务。同时清理所有对它的依赖引用。",
                "input_schema": {
                    "type": "object",
                    "properties": {
                        "task_id": {"type": "integer", "description": "要删除的任务 ID"}
                    },
                    "required": ["task_id"]
                }
            }),
            should_defer: true,
            is_read_only: false,
            is_concurrency_safe: false,
            is_enabled: true,
        },
        ToolDef {
            name: "task_list",
            description: "列出所有任务及其状态概要",
            search_hint: "list tasks status overview",
            schema: json!({
                "name": "task_list",
                "description": "列出所有任务及其状态概要。\n\n返回信息：\n- id: 任务标识符（用于 task_get、task_update）\n- subject: 任务简述\n- status: pending/in_progress/completed\n- owner: 负责人（如有）\n- blockedBy: 未完成的前置依赖（已完成的自动过滤）\n\n使用建议：\n- 优先按 ID 顺序处理（较小 ID 通常为前置任务）\n- 完成任务后调用此工具查找下一个可用任务\n- 查看被阻塞的任务以确定需要先解决什么",
                "input_schema": { "type": "object", "properties": {} }
            }),
            should_defer: true,
            is_read_only: true,
            is_concurrency_safe: true,
            is_enabled: true,
        },
        ToolDef {
            name: "task_summary",
            description: "生成任务全景报告（进度、瓶颈、下一步）",
            search_hint: "summary report progress",
            schema: json!({
                "name": "task_summary",
                "description": "生成任务全景报告，包括进度、关键路径瓶颈、以及推荐的下一步待办任务。建议在开始新工作或完成重要任务后使用。",
                "input_schema": { "type": "object", "properties": {} }
            }),
            should_defer: true,
            is_read_only: true,
            is_concurrency_safe: true,
            is_enabled: true,
        },
        ToolDef {
            name: "task_get",
            description: "获取单个任务的完整详细信息",
            search_hint: "get task detail info",
            schema: json!({
                "name": "task_get",
                "description": "获取单个任务的完整详细信息，包括描述、依赖关系、activeForm、metadata 等。\n\n使用场景：\n- 开始工作前获取完整需求\n- 了解任务依赖关系\n- 查看分配给自己的任务详情\n\n提示：获取后确认 blockedBy 为空再开始工作。",
                "input_schema": {
                    "type": "object",
                    "properties": { "task_id": {"type": "integer", "description": "任务 ID"} },
                    "required": ["task_id"]
                }
            }),
            should_defer: true,
            is_read_only: true,
            is_concurrency_safe: true,
            is_enabled: true,
        }
    }
}
