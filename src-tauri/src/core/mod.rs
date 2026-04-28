pub mod adapters;
pub mod agent;
pub mod agent_runs;
pub mod api_client;
pub mod api_format;
pub mod background;
pub mod checkpoint;
pub mod commands;
pub mod config;
pub mod constants;
pub mod debug_logger;
pub mod error;
pub mod intent;
pub mod intent_rules;
pub mod memory;
pub mod models;
pub mod prompts;
pub mod providers;
pub mod registry;
pub mod sessions;
pub mod snapshot_engine;
pub mod snapshot_manager;
pub mod state;
pub mod subagents;
pub mod tasks;
pub mod tools;
pub mod traits;

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
