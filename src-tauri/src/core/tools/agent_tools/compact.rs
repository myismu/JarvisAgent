//! # compact.rs — 上下文压缩与记忆整理工具
//!
//! ## 关键导出
//! - `compact()`: 手动触发上下文压缩

use tauri::Manager;

use crate::core::tools::framework;

/// 手动压缩上下文
pub async fn compact(
    app: &tauri::AppHandle,
    _input: &serde_json::Value,
    session_id: &str,
) -> framework::ToolCallResult {
    if let Some(manager) = app.try_state::<crate::infra::state::state::SessionManager>() {
        let ctx = manager.get_or_create(session_id).await;
        let scope = crate::infra::state::state::active_run_scope_key(app, session_id).await;
        let mut cache = ctx.dedupe_cache.lock().await;
        let state = cache.entry("compact".to_string()).or_default();
        if let Some(entry) = state.get_mut(&scope) {
            entry.suppressed_count += 1;
            return framework::ToolCallResult::ok(format!(
                "Repeated CompactConversation blocked: CompactConversation was already requested in this agent run. Continue using the existing context and answer or proceed. Suppressed duplicate #{}.",
                entry.suppressed_count
            ));
        }
        state.insert(
            scope,
            crate::infra::state::state::ToolDedupeCacheEntry {
                display: "compact".to_string(),
                suppressed_count: 0,
                running: false,
            },
        );
    }
    framework::ToolCallResult::ok("手动触发上下文压缩中...".to_string())
}
