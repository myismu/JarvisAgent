//! # config.rs — 配置管理 Tauri 命令
//!
//! 提供前端读取、保存应用配置以及获取图片压缩参数的 Tauri 命令。
//!
//! ## 关键导出
//! - `get_config()`: 读取当前 `AppConfig`
//! - `save_config_cmd()`: 保存配置并通知前端刷新
//! - `get_image_compress_config()`: 返回当前激活配置的图片压缩参数

use crate::core::config;
use tauri::Emitter;

/// 读取当前应用配置
#[tauri::command]
pub async fn get_config(
    config_state: tauri::State<'_, config::ConfigState>,
) -> Result<config::AppConfig, String> {
    Ok(config_state.0.lock().await.clone())
}

/// 保存应用配置，更新内存状态并持久化到磁盘
#[tauri::command]
pub async fn save_config_cmd(
    new_config: config::AppConfig,
    config_state: tauri::State<'_, config::ConfigState>,
    app: tauri::AppHandle,
) -> Result<(), String> {
    // 校验必填字段
    config::validate_config(&new_config)?;
    // 原子写入磁盘
    config::save_config(&new_config)?;
    let mut current = config_state.0.lock().await;
    *current = new_config.clone();
    let active = new_config.active_config();
    println!(
        "[配置] 已保存应用配置，当前激活: {} (main_model={})",
        new_config.active_profile_id, active.main_model
    );
    let _ = app.emit("config-updated", ());
    Ok(())
}

/// 获取图片压缩配置（最大宽高、质量），从 UiPreferences 读取
#[tauri::command]
pub async fn get_image_compress_config() -> Result<serde_json::Value, String> {
    let prefs = crate::core::commands::window_state::get_ui_preferences()
        .await
        .unwrap_or_default();
    Ok(serde_json::json!({
        "maxWidth": prefs.image_max_width,
        "maxHeight": prefs.image_max_height,
        "quality": prefs.image_quality,
    }))
}
