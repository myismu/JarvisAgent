//! # compact.rs — 上下文压缩与记忆整理工具
//!
//! ## 关键导出
//! - `compact()`: 手动触发上下文压缩
//! - `dream()`: 触发记忆整理（Dream Agent）

use tauri::Manager;

use crate::core::orchestration::tasks::TaskManager;

/// 手动压缩上下文
pub async fn compact(
    app: &tauri::AppHandle,
    _input: &serde_json::Value,
    session_id: &str,
) -> String {
    if let Some(manager) = app.try_state::<crate::core::state::SessionManager>() {
        let ctx = manager.get_or_create(session_id).await;
        let scope = crate::core::state::active_run_scope_key(app, session_id).await;
        let mut state = ctx.compact_state.lock().await;
        if let Some(entry) = state.get_mut(&scope) {
            entry.suppressed_count += 1;
            return format!(
                "Repeated compact blocked: compact was already requested in this agent run. Continue using the existing context and answer or proceed. Suppressed duplicate #{}.",
                entry.suppressed_count
            );
        }
        state.insert(
            scope,
            crate::core::state::ToolDedupeCacheEntry {
                display: "compact".to_string(),
                suppressed_count: 0,
                running: false,
            },
        );
    }
    "手动触发上下文压缩中...".to_string()
}

/// 触发记忆整理（Dream Agent）
pub async fn dream(app: &tauri::AppHandle, _input: &serde_json::Value, session_id: &str) -> String {
    if let Some(manager) = app.try_state::<crate::core::state::SessionManager>() {
        let ctx = manager.get_or_create(session_id).await;
        let scope = crate::core::state::active_run_scope_key(app, session_id).await;
        let mut state = ctx.dream_state.lock().await;
        if let Some(entry) = state.get_mut(&scope) {
            entry.suppressed_count += 1;
            return format!(
                "Repeated dream blocked: dream was already requested in this agent run. Use the existing task summary or answer the user now. Suppressed duplicate #{}.",
                entry.suppressed_count
            );
        }
        state.insert(
            scope,
            crate::core::state::ToolDedupeCacheEntry {
                display: "".to_string(),
                suppressed_count: 0,
                running: false,
            },
        );
    }
    let summary = TaskManager::for_session(session_id)
        .summary()
        .unwrap_or_else(|e| format!("生成摘要失败: {}", e));
    format!("主动触发记忆整理（Dream Agent）已启动。\n\n[记忆归档与状态同步报告]\n当前项目的全局任务状态已更新：\n\n{}\n\n请根据上述进度报告，评估下一步需要启动的核心任务，或者判断是否可以进入休息/总结状态。", summary)
}
