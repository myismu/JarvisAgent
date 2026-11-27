//! # app_config.rs — 应用配置 Tauri 命令薄封装
//!
//! 对前端暴露窗口状态、UI 偏好与技能激活开关命令；底层并发安全读写、
//! 原子替换与损坏备份统一委托给 `crate::infra::config::app_config`。
//!
//! ## Key Exports
//! - `get_custom_window_state()` / `save_custom_window_state()` / `clear_custom_window_states()` / `list_custom_window_states()`: 窗口状态命令
//! - `get_ui_preferences()` / `save_ui_preferences()`: UI 偏好设置命令
//! - `get_skill_activations()` / `set_skill_active()`: 技能激活状态命令
//!
//! ## Dependencies
//! - Internal: `crate::infra::config::app_config`
//! - External: `tauri`

use crate::infra::config::app_config::{self, CustomWindowState, UiPreferences};
use std::collections::HashMap;
use tauri::Emitter;

// ── Window state commands ──

#[tauri::command]
pub async fn get_custom_window_state(label: String) -> Result<Option<CustomWindowState>, String> {
    Ok(app_config::get_window_state(&label))
}

#[tauri::command]
pub async fn list_custom_window_states() -> Result<HashMap<String, CustomWindowState>, String> {
    Ok(app_config::list_window_states())
}

#[tauri::command]
pub async fn clear_custom_window_states() -> Result<(), String> {
    app_config::clear_window_states()
}

#[tauri::command]
pub async fn save_custom_window_state(
    label: String,
    state: CustomWindowState,
) -> Result<(), String> {
    app_config::save_window_state(label, state)
}

// ── UI preferences commands ──

#[tauri::command]
pub async fn get_ui_preferences() -> Result<UiPreferences, String> {
    Ok(app_config::get_ui_preferences())
}

#[tauri::command]
pub async fn save_ui_preferences(
    app: tauri::AppHandle,
    preferences: UiPreferences,
) -> Result<(), String> {
    app_config::save_ui_preferences(preferences)?;
    // 通知所有窗口（包括监控窗口）偏好已更新
    let _ = app.emit("ui-preferences-changed", ());
    Ok(())
}

// ── Skill activation commands ──

#[tauri::command]
pub async fn get_skill_activations() -> Result<HashMap<String, bool>, String> {
    Ok(app_config::get_all_skill_activations())
}

#[tauri::command]
pub async fn set_skill_active(skill_name: String, active: bool) -> Result<(), String> {
    app_config::set_skill_activation(&skill_name, active)
}
