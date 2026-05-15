//! # mod.rs — Agent 子系统入口模块
//!
//! 组织并暴露 Agent 核心子模块（pipeline、stream、context、tools_runner），
//! 并定义前端可调用的 `ask_jarvis` Tauri 命令。
//!
//! ## 关键导出
//! - `ask_jarvis()`: Tauri 命令入口，接收用户消息并触发完整 Agent 流程
//!
//! ## 依赖
//! - Internal: `pipeline::run_pipeline`, `crate::infra::types::error::AgentError`, `crate::infra::types::models::JarvisResult`
//! - External: `tauri`
//!
//! ## 约束
//! - `ask_jarvis` 必须在 Tauri 运行时中调用，依赖 `SessionManager` 和 `ConfigState` 状态

use crate::infra::types::error::AgentError;
use crate::infra::types::models::JarvisResult;

mod context;
mod pipeline;
pub mod prompts;
pub mod stream;
mod tools_runner;

// Re-export stream types for use by tools/agent_tools.rs
pub use stream::{process_stream, StreamConfig, StreamResult};

use pipeline::{resume_pipeline, run_pipeline};

#[tauri::command]
pub async fn ask_jarvis(
    session_id: String,
    msg: String,
    thinking_override: Option<bool>,
    image_base64_list: Option<Vec<String>>,
    agent_display_mode: Option<String>,
    reflection_mode: Option<String>,
    app: tauri::AppHandle,
    session_manager: tauri::State<'_, crate::infra::state::state::SessionManager>,
    config_state: tauri::State<'_, crate::infra::config::config::ConfigState>,
) -> Result<JarvisResult, AgentError> {
    run_pipeline(
        session_id,
        msg,
        thinking_override,
        image_base64_list,
        agent_display_mode,
        reflection_mode,
        app,
        session_manager,
        config_state,
    )
    .await
}

#[tauri::command]
pub async fn resume_jarvis(
    session_id: String,
    reason: String,
    app: tauri::AppHandle,
    session_manager: tauri::State<'_, crate::infra::state::state::SessionManager>,
    config_state: tauri::State<'_, crate::infra::config::config::ConfigState>,
) -> Result<JarvisResult, AgentError> {
    resume_pipeline(session_id, reason, app, session_manager, config_state).await
}
