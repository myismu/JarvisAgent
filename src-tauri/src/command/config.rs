//! # config.rs — 配置管理 Tauri 命令
//!
//! 提供前端读取、保存应用配置以及获取图片压缩参数的 Tauri 命令。
//!
//! ## 关键导出
//! - `get_config()`: 读取当前 `AppConfig`
//! - `save_config_cmd()`: 保存配置并通知前端刷新
//! - `get_image_compress_config()`: 返回当前激活配置的图片压缩参数

use crate::infra::config::config;
use crate::infra::types::error::CommandError;
use serde::Serialize;
use tauri::Emitter;

/// 图片压缩参数快照（从 `UiPreferences` 投影，供前端压缩管线直接消费）
#[derive(Debug, Clone, Serialize, ts_rs::TS)]
#[serde(rename_all = "camelCase")]
#[ts(rename_all = "camelCase")]
pub struct ImageCompressConfig {
    pub max_width: u32,
    pub max_height: u32,
    pub quality: f32,
}

/// 读取当前应用配置
#[tauri::command]
pub async fn get_config(
    config_state: tauri::State<'_, config::ConfigState>,
) -> Result<config::AppConfig, CommandError> {
    Ok(config_state.0.lock().await.clone())
}

/// 保存应用配置，更新内存状态并持久化到磁盘
#[tauri::command]
pub async fn save_config_cmd(
    new_config: config::AppConfig,
    config_state: tauri::State<'_, config::ConfigState>,
    app: tauri::AppHandle,
) -> Result<(), CommandError> {
    // 校验必填字段
    config::validate_config(&new_config).map_err(CommandError::invalid_input)?;
    // 原子写入磁盘
    config::save_config(&new_config).map_err(CommandError::config)?;
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
pub async fn get_image_compress_config() -> Result<ImageCompressConfig, CommandError> {
    let prefs = crate::infra::config::app_config::get_ui_preferences();
    Ok(ImageCompressConfig {
        max_width: prefs.image_max_width,
        max_height: prefs.image_max_height,
        quality: prefs.image_quality,
    })
}
