//! # mod.rs — 核心模块入口
//!
//! 这是 JarvisAgent 核心功能的入口模块。组织并重导出所有子模块和关键类型。
//! 分为基础模块（广泛依赖）和业务子目录（特定功能领域）。
//!
//! ## 关键导出
//! - 基础模块: `config`, `constants`, `error`, `models`, `state`, `traits`
//! - 业务模块: `agent`, `commands`, `infra`, `intent`, `llm`, `orchestration`, `providers`, `session`, `rollback`, `tools`
//! - 重导出类型: `SessionManager`, `SessionContext`, `WorkspaceState`, `SnapshotRegistry`
//! - 重导出函数: `ask_jarvis`, `resolve_permission`, `cancel_jarvis`, 以及所有命令函数
//!
//! ## 依赖
//! - Internal: 所有子模块
//! - External: 无直接依赖
//!
//! ## 约束
//! - 新增模块必须在此处声明并重导出
//! - 重导出的函数是前端可调用的 Tauri 命令

// ===== 基础模块（广泛依赖，保留在根目录）=====
pub mod config;
pub mod constants;
pub mod data_paths;
pub mod db;
pub mod error;
pub mod events;
pub mod models;
pub mod state;
pub mod traits;

// ===== 业务子目录 =====
pub mod agent;
pub mod commands;
pub mod infra;
pub mod intent;
pub mod llm;
pub mod orchestration;
pub mod providers;
pub mod rollback;
pub mod session;
pub mod tools;

#[macro_export]
macro_rules! jarvis_debug { ($tag:expr, $($arg:tt)*) => { println!($($arg)*); } }
#[macro_export]
macro_rules! jarvis_info { ($tag:expr, $($arg:tt)*) => { println!($($arg)*); } }
#[macro_export]
macro_rules! jarvis_warn { ($tag:expr, $($arg:tt)*) => { eprintln!($($arg)*); } }

pub use state::{
    SessionCleanupResult, SessionContext, SessionManager, SnapshotRegistry, WorkspaceState,
};

pub use agent::ask_jarvis;
pub use commands::checkpoint::{
    clear_pending_operations, commit_checkpoint, create_branch, delete_branch, get_active_branch,
    get_checkpoint_tree, list_branches, list_checkpoints,
    preview_rollback_to_checkpoint_with_recall, rollback_to_checkpoint,
    rollback_to_checkpoint_with_recall, switch_branch,
};
pub use commands::config::{get_config, get_image_compress_config, save_config_cmd};
pub use commands::history::get_session_history;
pub use commands::merge::{merge_execute, merge_get_conflicts, merge_preview};
pub use commands::permission::{cancel_jarvis, resolve_permission};
pub use commands::sandbox::{
    sandbox_abandon, sandbox_compare, sandbox_complete, sandbox_create, sandbox_get, sandbox_list,
    sandbox_publish,
};
pub use commands::session::{
    cancel_subagent_run, create_session, delete_session, get_active_session_id, get_agent_steps,
    get_background_tasks, get_session_context_snapshot, get_session_meta, get_subagent_runs,
    get_workspace_dir, list_agent_run_events, list_agent_runs, list_plan_documents, list_sessions,
    list_subagent_events, list_subagents, prepare_resume_agent_run, recall_last_message,
    rename_session, save_agent_steps, switch_session, update_session_profile,
};
pub use commands::snapshot::{
    snapshot_create, snapshot_create_branch, snapshot_get_current, snapshot_get_detail,
    snapshot_get_summaries, snapshot_get_tree_view, snapshot_list, snapshot_list_branches,
    snapshot_rollback, snapshot_switch_branch,
};
pub use commands::window_state::{
    clear_custom_window_states, get_custom_window_state, get_ui_preferences,
    list_custom_window_states, save_custom_window_state, save_ui_preferences,
};
