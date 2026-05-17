use crate::core::orchestration::tasks::TaskManager;
use crate::core::tools::framework::registry::ToolDef;
use serde_json::json;

pub(super) fn tool_def() -> ToolDef {
    ToolDef {
        name: "SummarizeTasks",
        description: "生成任务全景报告（进度、瓶颈、下一步）",
        search_hint: "summary report progress 总结 报告 进度",
        category: "任务管理",
        schema: json!({
            "name": "SummarizeTasks",
            "description": "Generate a task overview report, including progress, critical path, bottlenecks, and recommended next work. Use this before starting a new work batch or after completing significant tasks.",
            "input_schema": { "type": "object", "properties": {} }
        }),
        should_defer: true,
        is_read_only: true,
        is_concurrency_safe: true,
        is_enabled: true,
    }
}

pub async fn task_summary(
    _app: &tauri::AppHandle,
    _input: &serde_json::Value,
    session_id: &str,
) -> String {
    TaskManager::for_session(session_id)
        .summary()
        .unwrap_or_else(|e| e)
}
