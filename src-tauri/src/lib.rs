/// JarvisAgent 后端入口模块
///
/// 本文件是 Tauri 桌面应用的 Rust 后端入口点，负责：
/// - 初始化运行时环境（工作目录、会话恢复等）
/// - 配置并启动 Tauri 应用框架
/// - 注册状态管理器与前端可调用的命令（invoke handler）
/// - 加载各类 Tauri 官方插件（文件系统、对话框、窗口状态等）
pub mod core;

// ───────────────────────────────────────────────
// 核心模块导入：状态管理器
// ───────────────────────────────────────────────
use crate::core::state::{
    WorkspaceState,           // 工作空间状态，记录当前工作目录
    SnapshotRegistry,         // 快照注册表，管理会话级快照
    SessionManager,           // 会话管理器，管理活跃会话生命周期
};

// ───────────────────────────────────────────────
// 核心模块导入：背景任务、配置、子代理
// ───────────────────────────────────────────────
use crate::core::background::BackgroundState;       // 后台任务状态
use crate::core::config::{ConfigState, load_config}; // 配置状态与加载函数
use crate::core::snapshot_manager::session_manager::SessionManagerRegistry;
use crate::core::subagents::SubAgentMonitorState;    // 子代理监控状态

// ───────────────────────────────────────────────
// 标准库与异步运行时导入
// ───────────────────────────────────────────────
use tokio::sync::Mutex;      // 异步互斥锁，用于跨线程安全共享状态
use std::path::PathBuf;      // 路径缓冲区，处理文件系统路径
use std::sync::OnceLock;     // 线程安全的一次性初始化锁，用于全局静态路径

/// 全局静态变量：Agent 主目录路径
///
/// 在应用启动时通过 `get_agent_home` 初始化，后续所有模块可通过
/// 该变量获取 JarvisAgent 的根目录，确保路径一致性。
static AGENT_HOME_DIR: OnceLock<PathBuf> = OnceLock::new();

/// 获取已初始化的 Agent 主目录路径
///
/// # Panics
/// 如果在 `AGENT_HOME_DIR` 初始化前调用，将触发 panic。
pub fn get_agent_home() -> &'static PathBuf {
    AGENT_HOME_DIR.get().expect("AGENT_HOME_DIR not initialized")
}

/// Tauri 应用入口函数
///
/// 负责整个桌面应用后端的初始化流程，包括：
/// 1. 加载环境变量
/// 2. 锁定并记录 Agent 主目录
/// 3. 恢复上次工作目录
/// 4. 恢复或创建启动会话
/// 5. 构建 Tauri 应用，注册状态与命令
#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    // ─── 环境变量初始化 ───
    // 从 `.env` 文件加载环境变量，若不存在则静默忽略
    dotenvy::dotenv().ok();

    // ─── 锁定 Agent 主目录 ───
    // 获取当前进程所在目录作为 Agent 根目录，并将其写入全局静态变量
    let startup_dir = std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."));
    let _ = AGENT_HOME_DIR.set(startup_dir.clone());
    println!("[System] Agent home directory locked to: {}", startup_dir.display());

    // ─── 恢复工作目录 ───
    // 读取 `.jarvis_workspace` 文件中记录的上次工作目录，若目录有效则恢复
    let workspace_file = startup_dir.join(core::constants::FILE_WORKSPACE);
    if let Ok(path) = std::fs::read_to_string(workspace_file) {
        let path = path.trim();
        if std::path::Path::new(path).exists() {
            let _ = std::env::set_current_dir(path);
            println!("[System] Restored working directory to: {}", path);
        }
    }

    // ─── 会话恢复 ───
    // 尝试获取上次活跃的会话 ID；若该会话元数据有效则恢复，否则新建会话
    let startup_session_id = core::sessions::get_last_active_session_id()
        .filter(|id| core::sessions::get_session_meta(id).is_ok())
        .unwrap_or_else(|| core::sessions::create_session(None).id);
    println!("[System] 启动应用，恢复会话: {}", startup_session_id);

    // ─── Tauri Builder 配置 ───
    // 构建 Tauri 应用实例，注册状态管理器与插件
    tauri::Builder::default()
        // 注册各模块状态管理器，供命令处理器注入使用
        .manage(SessionManager::new())
        .manage(BackgroundState::default())
        .manage(SubAgentMonitorState::default())
        .manage(ConfigState(std::sync::Arc::new(Mutex::new(load_config()))))
        .manage(WorkspaceState(Mutex::new(None)))
        .manage(SnapshotRegistry(tokio::sync::RwLock::new(SessionManagerRegistry::new())))
        // 注册 Tauri 官方插件
        .plugin(tauri_plugin_opener::init())       // 文件/URL 打开器
        .plugin(tauri_plugin_dialog::init())       // 系统对话框
        .plugin(tauri_plugin_fs::init())           // 文件系统操作
        .plugin(tauri_plugin_window_state::Builder::new().build()) // 窗口状态持久化
        // ─── 命令注册（前端 invoke 调用入口）───
        .invoke_handler(tauri::generate_handler![
            // 核心 AI 对话
            core::agent::ask_jarvis,

            // ── 权限控制 ──
            core::commands::permission::cancel_jarvis,
            core::commands::permission::resolve_permission,

            // ── 会话管理 ──
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

            // ── 配置管理 ──
            core::commands::config::get_config,
            core::commands::config::save_config_cmd,
            core::commands::config::get_image_compress_config,

            // ── 历史记录 ──
            core::commands::history::get_session_history,

            // ── 检查点与分支 ──
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

            // ── 快照管理 ──
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

            // ── 沙盒会话 ──
            core::commands::sandbox::sandbox_create,
            core::commands::sandbox::sandbox_get,
            core::commands::sandbox::sandbox_list,
            core::commands::sandbox::sandbox_complete,
            core::commands::sandbox::sandbox_abandon,
            core::commands::sandbox::sandbox_publish,
            core::commands::sandbox::sandbox_compare,

            // ── 合并冲突 ──
            core::commands::merge::merge_preview,
            core::commands::merge::merge_execute,
            core::commands::merge::merge_get_conflicts,

            // ── 模型注册表 ──
            core::registry::get_model_capabilities,
            core::registry::list_model_registry,
        ])
        .run(tauri::generate_context!())
        .expect("运行失败");
}
