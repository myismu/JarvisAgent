//! # window_state.rs — 窗口状态与 UI 偏好持久化
//!
//! 将主窗口/监控窗口的位置尺寸、以及 UI 偏好（字体、紧凑模式、交互行为等）
//! 统一保存到 data/window-state.json，便于项目数据统一管理。
//!
//! ## 关键导出
//! - `get_custom_window_state()`: 读取指定窗口状态
//! - `save_custom_window_state()`: 保存指定窗口状态
//! - `clear_custom_window_states()`: 清空所有窗口状态
//! - `list_custom_window_states()`: 返回所有已保存窗口状态
//! - `get_ui_preferences()`: 读取 UI 偏好设置
//! - `save_ui_preferences()`: 保存 UI 偏好设置并通知所有窗口

use crate::core::data_paths;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::fs;
use tauri::Emitter;

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CustomWindowState {
    pub width: u32,
    pub height: u32,
    pub x: i32,
    pub y: i32,
    pub maximized: bool,
    pub fullscreen: bool,
    pub decorated: bool,
    pub updated_at: u64,
}

/// UI 偏好设置（与前端 usePreferences.ts 对应）
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct UiPreferences {
    #[serde(default = "default_font_size")]
    pub font_size: i32,
    #[serde(default = "default_code_font_size")]
    pub code_font_size: i32,
    #[serde(default = "default_true")]
    pub auto_scroll: bool,
    #[serde(default)]
    pub default_expand_thinking: bool,
    #[serde(default = "default_agent_panel_position")]
    pub agent_panel_position: String,
    #[serde(default)]
    pub compact_mode: bool,
    #[serde(default)]
    pub sidebar_collapsed: bool,
    #[serde(default)]
    pub agent_panel_visible: bool,
    #[serde(default = "default_agent_audience")]
    pub agent_audience: String,
    #[serde(default = "default_agent_work_mode")]
    pub agent_work_mode: String,
    #[serde(default = "default_locale")]
    pub locale: String,
    /// 图片压缩最大宽度（像素）
    #[serde(default = "default_image_max_width")]
    pub image_max_width: u32,
    /// 图片压缩最大高度（像素）
    #[serde(default = "default_image_max_height")]
    pub image_max_height: u32,
    /// 图片压缩质量 (0.0 ~ 1.0)
    #[serde(default = "default_image_quality")]
    pub image_quality: f32,
    /// 向后兼容：读取旧 agent_display_mode 字段
    #[serde(default)]
    agent_display_mode: Option<String>,
}

fn default_font_size() -> i32 { 15 }
fn default_code_font_size() -> i32 { 13 }
fn default_true() -> bool { true }
fn default_agent_panel_position() -> String { "right".to_string() }
fn default_agent_audience() -> String { "developer".to_string() }
fn default_agent_work_mode() -> String { "edit".to_string() }
fn default_locale() -> String { "zh-CN".to_string() }
fn default_image_max_width() -> u32 { 1920 }
fn default_image_max_height() -> u32 { 1080 }
fn default_image_quality() -> f32 { 0.8 }

impl Default for UiPreferences {
    fn default() -> Self {
        Self {
            font_size: 15,
            code_font_size: 13,
            auto_scroll: true,
            default_expand_thinking: false,
            agent_panel_position: "right".to_string(),
            compact_mode: false,
            sidebar_collapsed: false,
            agent_panel_visible: false,
            agent_audience: "developer".to_string(),
            agent_work_mode: "edit".to_string(),
            locale: "zh-CN".to_string(),
            image_max_width: 1920,
            image_max_height: 1080,
            image_quality: 0.8,
            agent_display_mode: None,
        }
    }
}

impl UiPreferences {
    /// 兼容旧版本 agent_display_mode，迁移到双轴
    pub fn migrate_legacy_display_mode(&mut self) {
        if let Some(legacy) = self.agent_display_mode.take() {
            if self.agent_audience == default_agent_audience()
                && self.agent_work_mode == default_agent_work_mode()
            {
                match legacy.as_str() {
                    "user" => {
                        self.agent_audience = "user".to_string();
                        self.agent_work_mode = "chat".to_string();
                    }
                    "developer" => {
                        self.agent_audience = "developer".to_string();
                        self.agent_work_mode = "edit".to_string();
                    }
                    _ => {}
                }
            }
        }
    }
}

/// 顶层文件结构
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct WindowStateFile {
    #[serde(default)]
    windows: HashMap<String, CustomWindowState>,
    #[serde(default = "UiPreferences::default")]
    ui_preferences: UiPreferences,
}

fn window_state_path() -> std::path::PathBuf {
    data_paths::data_root().join("window-state.json")
}

fn read_file() -> WindowStateFile {
    let path = window_state_path();
    let Ok(content) = fs::read_to_string(path) else {
        return WindowStateFile {
            windows: HashMap::new(),
            ui_preferences: UiPreferences::default(),
        };
    };
    serde_json::from_str(&content).unwrap_or(WindowStateFile {
        windows: HashMap::new(),
        ui_preferences: UiPreferences::default(),
    })
}

fn write_file(file: &WindowStateFile) -> Result<(), String> {
    let path = window_state_path();
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).map_err(|err| err.to_string())?;
    }
    let content = serde_json::to_string_pretty(file).map_err(|err| err.to_string())?;
    fs::write(path, content).map_err(|err| err.to_string())
}

// ── Window state commands ──

#[tauri::command]
pub async fn get_custom_window_state(label: String) -> Result<Option<CustomWindowState>, String> {
    Ok(read_file().windows.get(&label).cloned())
}

#[tauri::command]
pub async fn list_custom_window_states() -> Result<HashMap<String, CustomWindowState>, String> {
    Ok(read_file().windows)
}

#[tauri::command]
pub async fn clear_custom_window_states() -> Result<(), String> {
    let mut file = read_file();
    file.windows.clear();
    write_file(&file)
}

#[tauri::command]
pub async fn save_custom_window_state(
    label: String,
    state: CustomWindowState,
) -> Result<(), String> {
    let mut file = read_file();
    file.windows.insert(label, state);
    write_file(&file)
}

// ── UI preferences commands ──

#[tauri::command]
pub async fn get_ui_preferences() -> Result<UiPreferences, String> {
    let mut prefs = read_file().ui_preferences;
    prefs.migrate_legacy_display_mode();
    Ok(prefs)
}

#[tauri::command]
pub async fn save_ui_preferences(
    app: tauri::AppHandle,
    preferences: UiPreferences,
) -> Result<(), String> {
    let mut file = read_file();
    file.ui_preferences = preferences;
    write_file(&file)?;
    // 通知所有窗口（包括监控窗口）偏好已更新
    let _ = app.emit("ui-preferences-changed", ());
    Ok(())
}
