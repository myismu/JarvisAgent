pub mod core;

use crate::core::state::{
    WorkspaceState,
    SnapshotRegistry, SessionManager,
};
use crate::core::background::BackgroundState;
use crate::core::config::{ConfigState, load_config};
use crate::core::snapshot_manager::session_manager::SessionManagerRegistry;
use crate::core::subagents::SubAgentMonitorState;
use tokio::sync::Mutex;
use std::path::PathBuf;
use std::sync::OnceLock;

static AGENT_HOME_DIR: OnceLock<PathBuf> = OnceLock::new();

pub fn get_agent_home() -> &'static PathBuf {
    AGENT_HOME_DIR.get().expect("AGENT_HOME_DIR not initialized")
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    dotenvy::dotenv().ok();

    let startup_dir = std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."));
    let _ = AGENT_HOME_DIR.set(startup_dir.clone());
    println!("[System] Agent home directory locked to: {}", startup_dir.display());

    let workspace_file = startup_dir.join(core::constants::FILE_WORKSPACE);
    if let Ok(path) = std::fs::read_to_string(workspace_file) {
        let path = path.trim();
        if std::path::Path::new(path).exists() {
            let _ = std::env::set_current_dir(path);
            println!("[System] Restored working directory to: {}", path);
        }
    }

    let startup_session_id = core::sessions::get_last_active_session_id()
        .filter(|id| core::sessions::get_session_meta(id).is_ok())
        .unwrap_or_else(|| core::sessions::create_session(None).id);
    println!("[System] 启动应用，恢复会话: {}", startup_session_id);

    tauri::Builder::default()
        .manage(SessionManager::new())
        .manage(BackgroundState::default())
        .manage(SubAgentMonitorState::default())
        .manage(ConfigState(std::sync::Arc::new(Mutex::new(load_config()))))
        .manage(WorkspaceState(Mutex::new(None)))
        .manage(SnapshotRegistry(tokio::sync::RwLock::new(SessionManagerRegistry::new())))
        .plugin(tauri_plugin_opener::init())
        .plugin(tauri_plugin_dialog::init())
        .plugin(tauri_plugin_fs::init())
        .plugin(tauri_plugin_window_state::Builder::new().build())
        .invoke_handler(tauri::generate_handler![
            core::agent::ask_jarvis,
            core::commands::permission::cancel_jarvis,
            core::commands::permission::resolve_permission,
            core::commands::session::recall_last_message,
            core::commands::session::get_active_session_id,
            core::commands::session::list_sessions,
            core::commands::session::create_session,
            core::commands::session::switch_session,
            core::commands::session::delete_session,
            core::commands::session::rename_session,
            core::commands::session::update_session_profile,
            core::commands::session::get_session_meta,
            core::commands::session::get_workspace_dir,
            core::commands::session::save_agent_steps,
            core::commands::session::get_agent_steps,
            core::commands::session::list_plan_documents,
            core::commands::session::list_agent_runs,
            core::commands::session::list_agent_run_events,
            core::commands::session::prepare_resume_agent_run,
            core::commands::session::get_background_tasks,
            core::commands::session::get_subagent_runs,
            core::commands::session::list_subagents,
            core::commands::session::list_subagent_events,
            core::commands::session::cancel_subagent_run,
            core::commands::config::get_config,
            core::commands::config::save_config_cmd,
            core::commands::config::get_image_compress_config,
            core::commands::history::get_session_history,
            core::commands::checkpoint::list_checkpoints,
            core::commands::checkpoint::get_checkpoint_tree,
            core::commands::checkpoint::rollback_to_checkpoint,
            core::commands::checkpoint::create_branch,
            core::commands::checkpoint::switch_branch,
            core::commands::checkpoint::list_branches,
            core::commands::checkpoint::delete_branch,
            core::commands::checkpoint::get_active_branch,
            core::commands::checkpoint::commit_checkpoint,
            core::commands::checkpoint::clear_pending_operations,
            core::commands::snapshot::snapshot_create,
            core::commands::snapshot::snapshot_get_tree_view,
            core::commands::snapshot::snapshot_get_summaries,
            core::commands::snapshot::snapshot_get_detail,
            core::commands::snapshot::snapshot_create_branch,
            core::commands::snapshot::snapshot_switch_branch,
            core::commands::snapshot::snapshot_rollback,
            core::commands::snapshot::snapshot_list,
            core::commands::snapshot::snapshot_list_branches,
            core::commands::snapshot::snapshot_get_current,
            core::commands::sandbox::sandbox_create,
            core::commands::sandbox::sandbox_get,
            core::commands::sandbox::sandbox_list,
            core::commands::sandbox::sandbox_complete,
            core::commands::sandbox::sandbox_abandon,
            core::commands::sandbox::sandbox_publish,
            core::commands::sandbox::sandbox_compare,
            core::commands::merge::merge_preview,
            core::commands::merge::merge_execute,
            core::commands::merge::merge_get_conflicts,
            core::registry::get_model_capabilities,
            core::registry::list_model_registry,
        ])
        .run(tauri::generate_context!())
        .expect("运行失败");
}
