//! # lib.rs — Tauri 应用后端入口与初始化模块
//!
//! ## 三层架构
//! - `infra`: 基础设施层 — 数据模型、LLM 客户端、数据库、配置
//! - `core`: 业务层 — Agent 主循环、调度、会话、回滚、工具
//! - `command`: 命令层 — Tauri invoke handler 胶水

pub mod infra;
pub mod core;
pub mod command;

use crate::infra::state::state::{
    SessionManager,
    SnapshotRegistry,
    WorkspaceState,
};
use crate::infra::config::config::{load_config, ConfigState, RuntimeConfigState, RuntimeSettings};
use crate::infra::background::BackgroundState;
use crate::core::orchestration::subagents::SubAgentMonitorState;
use crate::core::rollback::session_manager::SnapshotManagerRegistry;

use std::path::PathBuf;
use std::sync::OnceLock;
use tokio::sync::Mutex;

static AGENT_HOME_DIR: OnceLock<PathBuf> = OnceLock::new();

pub fn get_agent_home() -> &'static PathBuf {
    AGENT_HOME_DIR
        .get()
        .expect("AGENT_HOME_DIR not initialized")
}

fn detect_data_dir() -> PathBuf {
    let cwd = std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."));
    if cwd.join("src-tauri").join("Cargo.toml").exists() {
        cwd.join("data")
    } else if cwd.join("Cargo.toml").exists() && cwd.join("src").join("lib.rs").exists() {
        cwd.parent()
            .map(|p| p.join("data"))
            .unwrap_or_else(|| cwd.join("data"))
    } else {
        std::env::current_exe()
            .ok()
            .and_then(|p| p.parent().map(|d| d.join("data")))
            .unwrap_or_else(|| cwd.join("data"))
    }
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    let data_dir = detect_data_dir();
    let _ = std::fs::create_dir_all(&data_dir);
    let _ = AGENT_HOME_DIR.set(data_dir.clone());
    println!("[System] Agent data directory locked to: {}", data_dir.display());

    infra::config::data_paths::ensure_base_layout();
    if let Err(err) = infra::db::init() {
        panic!("初始化 SQLite 数据库失败: {}", err);
    }
    let workspace_file = infra::config::data_paths::workspace_file_path();
    if let Ok(path) = std::fs::read_to_string(workspace_file) {
        let path = path.trim();
        if std::path::Path::new(path).exists() {
            let _ = std::env::set_current_dir(path);
            println!("[System] Restored working directory to: {}", path);
        }
    }

    let startup_session_id = core::session::get_last_active_session_id()
        .filter(|id| core::session::get_session_meta(id).is_ok());
    if let Some(id) = startup_session_id {
        println!("[System] 启动应用，恢复会话: {}", id);
    } else {
        println!("[System] 启动应用，暂无可恢复会话");
    }

    tauri::Builder::default()
        .manage(SessionManager::new())
        .manage(BackgroundState::default())
        .manage(SubAgentMonitorState::default())
        .manage(ConfigState(std::sync::Arc::new(Mutex::new(load_config()))))
        .manage(RuntimeConfigState(RuntimeSettings::default()))
        .manage(WorkspaceState(Mutex::new(None)))
        .manage(SnapshotRegistry(tokio::sync::RwLock::new(
            SnapshotManagerRegistry::new(),
        )))
        .plugin(tauri_plugin_opener::init())
        .plugin(tauri_plugin_dialog::init())
        .plugin(tauri_plugin_fs::init())
        .plugin(tauri_plugin_window_state::Builder::new().build())
        .invoke_handler(tauri::generate_handler![
            core::agent::ask_jarvis,
            command::permission::cancel_jarvis,
            command::permission::resolve_permission,
            command::permission::get_permission_state,
            command::permission::revoke_session_permission,
            command::session::recall_last_message,
            command::session::recall_message,
            command::session::recall_message_from_index,
            command::session::get_active_session_id,
            command::session::list_sessions,
            command::session::create_session,
            command::session::switch_session,
            command::session::delete_session,
            command::session::rename_session,
            command::session::update_session_profile,
            command::session::get_session_meta,
            command::session::get_session_context_snapshot,
            command::session::get_workspace_dir,
            command::session::list_plan_documents,
            command::session::list_agent_runs,
            command::session::list_agent_run_events,
            command::session::prepare_resume_agent_run,
            command::session::recover_interrupted_session_messages,
            command::session::get_background_tasks,
            command::session::dismiss_background_task,
            command::session::kill_background_task,
            command::session::clear_session_background_tasks,
            command::session::get_subagent_runs,
            command::session::list_subagents,
            command::session::list_subagent_events,
            command::session::cancel_subagent_run,
            command::session::get_session_todos,
            command::config::get_config,
            command::config::save_config_cmd,
            command::config::get_image_compress_config,
            command::history::get_session_history,
            command::checkpoint::list_checkpoints,
            command::checkpoint::get_checkpoint_tree,
            command::checkpoint::rollback_to_checkpoint,
            command::checkpoint::rollback_to_checkpoint_with_recall,
            command::checkpoint::preview_rollback_to_checkpoint_with_recall,
            command::checkpoint::create_branch,
            command::checkpoint::switch_branch,
            command::checkpoint::list_branches,
            command::checkpoint::delete_branch,
            command::checkpoint::get_active_branch,
            command::checkpoint::commit_checkpoint,
            command::checkpoint::clear_pending_operations,
            command::snapshot::snapshot_create,
            command::snapshot::snapshot_get_tree_view,
            command::snapshot::snapshot_get_summaries,
            command::snapshot::snapshot_get_detail,
            command::snapshot::snapshot_create_branch,
            command::snapshot::snapshot_switch_branch,
            command::snapshot::snapshot_rollback,
            command::snapshot::snapshot_list,
            command::snapshot::snapshot_list_branches,
            command::snapshot::snapshot_get_current,
            command::window_state::clear_custom_window_states,
            command::window_state::get_custom_window_state,
            command::window_state::list_custom_window_states,
            command::window_state::save_custom_window_state,
            command::window_state::get_ui_preferences,
            command::window_state::save_ui_preferences,
            command::sandbox::sandbox_create,
            command::sandbox::sandbox_get,
            command::sandbox::sandbox_list,
            command::sandbox::sandbox_complete,
            command::sandbox::sandbox_abandon,
            command::sandbox::sandbox_publish,
            command::sandbox::sandbox_compare,
            command::merge::merge_preview,
            command::merge::merge_execute,
            command::merge::merge_get_conflicts,
            infra::llm::registry::get_model_capabilities,
            infra::llm::registry::list_model_registry,
        ])
        .run(tauri::generate_context!())
        .expect("运行失败");
}
