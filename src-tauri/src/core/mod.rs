//! # mod.rs — 核心模块入口
//!
//! 这是 JarvisAgent 核心功能的入口模块。组织并重导出所有子模块和关键类型。
//! 分为基础模块（广泛依赖）和业务子目录（特定功能领域）。
//!
//! ## 关键导出
//! - 基础模块: `config`, `constants`, `error`, `models`, `state`, `traits`
//! - 业务模块: `agent`, `commands`, `infra`, `intent`, `llm`, `orchestration`, `providers`, `session`, `snapshot_engine`, `snapshot_manager`, `tools`
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
pub mod error;
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
pub mod session;
pub mod snapshot_engine;
pub mod snapshot_manager;
pub mod tools;

pub use state::{
    WorkspaceState, SessionCleanupResult,
    SnapshotRegistry,
    SessionManager, SessionContext
};

pub use agent::ask_jarvis;
pub use commands::permission::{resolve_permission, cancel_jarvis};
pub use commands::config::{get_config, save_config_cmd, get_image_compress_config};
pub use commands::session::{
    get_active_session_id, list_sessions, create_session, switch_session,
    delete_session, rename_session, update_session_profile, get_session_meta,
    get_workspace_dir, save_agent_steps, get_agent_steps, list_plan_documents,
    list_agent_runs, list_agent_run_events, prepare_resume_agent_run, get_background_tasks,
    cancel_subagent_run, get_subagent_runs, list_subagent_events, list_subagents,
    recall_last_message,
};
pub use commands::history::get_session_history;
pub use commands::checkpoint::{
    list_checkpoints, get_checkpoint_tree, rollback_to_checkpoint,
    create_branch, switch_branch, list_branches, delete_branch,
    get_active_branch, commit_checkpoint, clear_pending_operations,
};
pub use commands::snapshot::{
    snapshot_create, snapshot_get_tree_view, snapshot_get_summaries,
    snapshot_get_detail, snapshot_create_branch, snapshot_switch_branch,
    snapshot_rollback, snapshot_list, snapshot_list_branches, snapshot_get_current,
};
pub use commands::sandbox::{
    sandbox_create, sandbox_get, sandbox_list, sandbox_complete,
    sandbox_abandon, sandbox_publish, sandbox_compare,
};
pub use commands::merge::{
    merge_preview, merge_execute, merge_get_conflicts,
};
