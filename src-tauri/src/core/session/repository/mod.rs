//! # repository/mod.rs — 会话 SQLite 仓储门面
//!
//! 统一编排并重导出会话元数据、消息索引、项目信息与 token 用量明细四个领域仓储子模块。
//!
//! ## Key Exports
//! - `upsert_session()` / `list_sessions()` / `get_session_meta()`: 会话元数据读写
//! - `load_session()` / `append_or_upsert_session_messages()` / `load_visible_turns_page()`: 消息与快照读写
//! - `create_project()` / `list_projects()` / `rename_project()`: 项目元数据读写
//! - `insert_request_usage()` / `get_session_usage_breakdown()`: 请求级 token 用量读写
//!
//! ## Dependencies
//! - Internal: `messages`, `projects`, `sessions`, `usage`
//! - External: `rusqlite`

mod messages;
mod projects;
mod sessions;
mod usage;

#[cfg(test)]
mod tests;

pub use messages::{
    append_or_upsert_session_messages, delete_session_messages_by_source,
    delete_session_messages_from_seq, find_session_message_by_id, get_context_snapshot,
    hide_orphan_session_messages, hide_session_messages_from_seq, list_visible_session_messages,
    list_visible_session_messages_paged, load_session, load_visible_turns_page,
    session_message_stats, session_messages_count, update_context_snapshot_usage,
    upsert_context_snapshot, ContextSnapshotUsageUpdate, StoredSessionMessage,
};
pub use projects::{
    create_project, get_project, get_project_by_path, get_project_path, is_project_archived,
    list_projects, rename_project,
};
pub use sessions::{
    archive_project, clear_last_active_session_id, count_deleted_sessions, delete_session,
    ensure_session_exists, get_last_active_session_id, get_session_meta, get_session_runtime_prefs,
    hard_delete_session, list_deleted_sessions, list_sessions, purge_sessions, rename_session,
    restore_project, restore_session, restore_sessions, set_last_active_session_id,
    update_session_agent_audience, update_session_approval_mode, update_session_profile,
    update_session_thinking_mode, update_session_work_mode, upsert_session, SessionRuntimePrefs,
};
#[allow(deprecated)]
pub use usage::{
    accumulate_session_token_usage, get_session_usage_breakdown, insert_request_usage,
};

pub(super) use usage::{
    add_session_token_totals_in, insert_request_usage_in, session_usage_breakdown_in,
    session_usage_writable_in,
};

#[cfg(test)]
use crate::infra::types::models::{Message, MessageSource};
#[cfg(test)]
use messages::{
    find_session_message_by_id_in, is_turn_start_message, session_message_stats_in,
    stored_session_message_from_row, SESSION_MESSAGE_COLUMNS,
};
#[cfg(test)]
use projects::{list_projects_in, PROJECT_META_COLUMNS};
#[cfg(test)]
use sessions::{
    archive_project_in, count_deleted_sessions_in, list_deleted_sessions_in, purge_sessions_in,
    restore_sessions_in, SESSION_META_COLUMNS,
};
