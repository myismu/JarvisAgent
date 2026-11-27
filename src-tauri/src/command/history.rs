//! # history.rs — 会话历史读取 Tauri 命令薄封装
//!
//! 仅暴露结构化历史消息与按轮分页两个 Tauri 命令，核心投影与中断恢复前置委托给
//! `crate::core::session::history_view`。
//!
//! ## Key Exports
//! - `get_session_messages()`: 全量结构化消息命令
//! - `get_session_messages_paged()`: 按轮懒加载分页命令
//! - 重导出历史读模型类型供 `ts_bindings` 统一引用
//!
//! ## Dependencies
//! - Internal: `crate::core::session::history_view`, `crate::core::runtime::SessionManager`
//! - External: `tauri`

use crate::core::runtime::SessionManager;
pub use crate::core::session::history_view::{
    AgentExecutionLog, AgentTextBlock, AgentThinkingBlock, AgentToolCallView, AgentTurnSnapshot,
    AgentTurnTokens, PagedSessionMessages, SessionMessage,
};

#[tauri::command]
pub async fn get_session_messages(
    session_id: String,
    session_manager: tauri::State<'_, SessionManager>,
) -> Result<Vec<SessionMessage>, String> {
    crate::core::session::history_view::load_session_messages(&session_id, session_manager.inner())
        .await
}

/// 懒加载分页命令：**按轮凑页**（默认 5 轮）。
#[tauri::command]
pub async fn get_session_messages_paged(
    session_id: String,
    before_seq: Option<i64>,
    limit: Option<usize>,
    session_manager: tauri::State<'_, SessionManager>,
) -> Result<PagedSessionMessages, String> {
    crate::core::session::history_view::load_session_messages_paged(
        &session_id,
        before_seq,
        limit,
        session_manager.inner(),
    )
    .await
}
