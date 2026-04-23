pub mod core;

use crate::core::models::SessionMemory;
use crate::core::{SessionState, ActiveSession, SecurityState, PendingPermissions};
use crate::core::background::BackgroundState;
use crate::core::cancellation::CancellationState;
use crate::core::config::{ConfigState, load_config};
use tokio::sync::Mutex;
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::OnceLock;

/// Agent 启动时的"家目录"，用于锚定所有内部基础设施路径（.tasks、.logs、memory、skills）。
/// 它会在 run() 函数中被初始化一次，之后任何 set_current_dir 都不会影响它。
static AGENT_HOME_DIR: OnceLock<PathBuf> = OnceLock::new();

/// 获取 Agent 的不可变家目录。所有内部基础设施路径都应该基于此目录。
pub fn get_agent_home() -> &'static PathBuf {
    AGENT_HOME_DIR.get().expect("AGENT_HOME_DIR not initialized")
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    dotenvy::dotenv().ok();

    // 在任何 CWD 操作之前锁定 Agent 的家目录
    let startup_dir = std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."));
    let _ = AGENT_HOME_DIR.set(startup_dir.clone());
    println!("[System] Agent home directory locked to: {}", startup_dir.display());

    // 尝试读取上次的工作区路径并恢复 (存放在安装目录/运行目录)
    let workspace_file = startup_dir.join(core::constants::FILE_WORKSPACE);
    if let Ok(path) = std::fs::read_to_string(workspace_file) {
        let path = path.trim();
        if std::path::Path::new(path).exists() {
            let _ = std::env::set_current_dir(path);
            println!("[System] Restored working directory to: {}", path);
        }
    }

    // 启动时始终创建新会话作为默认会话
    let meta = core::sessions::create_session();
    println!("[System] 启动应用，创建新会话: {}", meta.id);
    let initial_memory = SessionMemory::default();
    let initial_session_id = Some(meta.id);

    tauri::Builder::default()
        .manage(SessionState(Mutex::new(initial_memory)))
        .manage(ActiveSession(Mutex::new(initial_session_id)))
        .manage(SecurityState { session_allowed: Mutex::new(false) })
        .manage(PendingPermissions(Mutex::new(HashMap::new())))
        .manage(BackgroundState::default())
        .manage(CancellationState::new())
        .manage(ConfigState(std::sync::Arc::new(Mutex::new(load_config()))))
        .plugin(tauri_plugin_opener::init())
        .invoke_handler(tauri::generate_handler![
            core::ask_jarvis,
            core::cancel_jarvis,
            core::resolve_permission,
            core::list_sessions,
            core::create_session,
            core::switch_session,
            core::delete_session,
            core::rename_session,
            core::get_session_history,
            core::get_active_session_id,
            core::get_config,
            core::save_config_cmd,
        ])
        .run(tauri::generate_context!())
        .expect("运行失败");
}