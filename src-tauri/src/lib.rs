//! # lib.rs — Tauri 应用后端入口与初始化模块
//!
//! 这是 JarvisAgent 桌面应用的 Rust 后端核心入口文件。负责初始化运行时环境、
//! 配置 Tauri 应用框架、注册状态管理器与前端可调用的命令（invoke handler），
//! 以及加载各类 Tauri 官方插件。
//!
//! ## 关键导出
//! - `run()`: Tauri 应用主入口函数，初始化并启动整个后端服务
//! - `get_agent_home()`: 获取已初始化的 Agent 数据目录路径
//! - `core`: 核心功能模块，包含所有业务逻辑和命令处理器
//!
//! ## 依赖
//! - Internal: `crate::core::*` (状态管理、配置、会话、快照等模块)
//! - External: `tauri`, `tokio`, `std::path`, `std::sync`
//!
//! ## 约束
//! - `run()` 函数必须在 Tauri 应用启动时调用，且只能调用一次
//! - `get_agent_home()` 必须在 `run()` 初始化后调用，否则会 panic
//! - 所有前端可调用的命令都必须在 `invoke_handler` 中注册
//! - 数据目录自动检测：开发模式指向项目根目录的 `data/`，打包后指向 exe 所在目录的 `data/`

pub mod core;

// 状态管理器
use crate::core::state::{
    SessionManager,   // 会话管理器，管理活跃会话生命周期
    SnapshotRegistry, // 快照注册表，管理会话级快照
    WorkspaceState,   // 工作空间状态，记录当前工作目录
};

// 后台任务、配置、子代理
use crate::core::config::{load_config, ConfigState}; // 配置状态与加载函数
use crate::core::infra::background::BackgroundState; // 后台任务状态
use crate::core::orchestration::subagents::SubAgentMonitorState;
use crate::core::snapshot_manager::session_manager::SessionManagerRegistry; // 子代理监控状态

// 标准库与异步运行时
use std::path::PathBuf; // 路径缓冲区，处理文件系统路径
use std::sync::OnceLock;
use tokio::sync::Mutex; // 异步互斥锁，用于跨线程安全共享状态

/// 全局静态变量：Agent 数据目录路径
static AGENT_HOME_DIR: OnceLock<PathBuf> = OnceLock::new();

/// 获取已初始化的 Agent 数据目录路径
///
/// # Panics
/// 如果在 `AGENT_HOME_DIR` 初始化前调用，将触发 panic。
pub fn get_agent_home() -> &'static PathBuf {
    AGENT_HOME_DIR
        .get()
        .expect("AGENT_HOME_DIR not initialized")
}

/// 检测并返回专用数据目录
///
/// 自动判断运行环境：
/// - 开发模式（`pnpm tauri dev`）：CWD = 项目根目录 → 返回 `<项目根>/data/`
/// - 开发模式（`cargo run`）：CWD = src-tauri → 返回 `<项目根>/data/`
/// - 打包后：返回 `<exe 所在目录>/data/`
fn detect_data_dir() -> PathBuf {
    let cwd = std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."));

    if cwd.join("src-tauri").join("Cargo.toml").exists() {
        // pnpm tauri dev 时 CWD = 项目根目录
        cwd.join("data")
    } else if cwd.join("Cargo.toml").exists() && cwd.join("src").join("lib.rs").exists() {
        // 直接 cargo run，在 src-tauri 内，上跳一级到项目根目录
        cwd.parent()
            .map(|p| p.join("data"))
            .unwrap_or_else(|| cwd.join("data"))
    } else {
        // 打包后：exe 所在目录的 data/
        std::env::current_exe()
            .ok()
            .and_then(|p| p.parent().map(|d| d.join("data")))
            .unwrap_or_else(|| cwd.join("data"))
    }
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
    // 锁定 Agent 数据目录
    let data_dir = detect_data_dir();

    // 确保数据目录存在
    let _ = std::fs::create_dir_all(&data_dir);

    let _ = AGENT_HOME_DIR.set(data_dir.clone());
    println!(
        "[System] Agent data directory locked to: {}",
        data_dir.display()
    );

    core::data_paths::ensure_base_layout();

    // 恢复工作目录
    let workspace_file = core::data_paths::workspace_file_path();
    if let Ok(path) = std::fs::read_to_string(workspace_file) {
        let path = path.trim();
        if std::path::Path::new(path).exists() {
            let _ = std::env::set_current_dir(path);
            println!("[System] Restored working directory to: {}", path);
        }
    }

    // 会话恢复
    let startup_session_id = core::session::get_last_active_session_id()
        .filter(|id| core::session::get_session_meta(id).is_ok())
        .unwrap_or_else(|| core::session::create_session(None).id);
    println!("[System] 启动应用，恢复会话: {}", startup_session_id);

    // 构建 Tauri 应用实例，注册状态管理器与插件
    tauri::Builder::default()
        .manage(SessionManager::new())
        .manage(BackgroundState::default())
        .manage(SubAgentMonitorState::default())
        .manage(ConfigState(std::sync::Arc::new(Mutex::new(load_config()))))
        .manage(WorkspaceState(Mutex::new(None)))
        .manage(SnapshotRegistry(tokio::sync::RwLock::new(
            SessionManagerRegistry::new(),
        )))
        // 注册 Tauri 官方插件
        .plugin(tauri_plugin_opener::init()) // 文件/URL 打开器
        .plugin(tauri_plugin_dialog::init()) // 系统对话框
        .plugin(tauri_plugin_fs::init()) // 文件系统操作
        .plugin(tauri_plugin_window_state::Builder::new().build()) // 窗口状态持久化
        // 命令注册（前端 invoke 调用入口）
        .invoke_handler(tauri::generate_handler![
            // 核心 AI 对话
            core::agent::ask_jarvis,
            // 权限控制
            core::commands::permission::cancel_jarvis,
            core::commands::permission::resolve_permission,
            // 会话管理
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
            // 配置管理
            core::commands::config::get_config,
            core::commands::config::save_config_cmd,
            core::commands::config::get_image_compress_config,
            // 历史记录
            core::commands::history::get_session_history,
            // 检查点与分支
            core::commands::checkpoint::list_checkpoints,
            core::commands::checkpoint::get_checkpoint_tree,
            core::commands::checkpoint::rollback_to_checkpoint,
            core::commands::checkpoint::rollback_to_checkpoint_with_recall,
            core::commands::checkpoint::create_branch,
            core::commands::checkpoint::switch_branch,
            core::commands::checkpoint::list_branches,
            core::commands::checkpoint::delete_branch,
            core::commands::checkpoint::get_active_branch,
            core::commands::checkpoint::commit_checkpoint,
            core::commands::checkpoint::clear_pending_operations,
            // 快照管理
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
            // 沙盒会话
            core::commands::sandbox::sandbox_create,
            core::commands::sandbox::sandbox_get,
            core::commands::sandbox::sandbox_list,
            core::commands::sandbox::sandbox_complete,
            core::commands::sandbox::sandbox_abandon,
            core::commands::sandbox::sandbox_publish,
            core::commands::sandbox::sandbox_compare,
            // 合并冲突
            core::commands::merge::merge_preview,
            core::commands::merge::merge_execute,
            core::commands::merge::merge_get_conflicts,
            // 模型注册表
            core::llm::registry::get_model_capabilities,
            core::llm::registry::list_model_registry,
        ])
        .run(tauri::generate_context!())
        .expect("运行失败");
}
