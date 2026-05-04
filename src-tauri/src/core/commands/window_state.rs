//! # window_state.rs — 自定义窗口状态持久化命令
//!
//! 将主窗口和监控窗口的位置、尺寸等 UI 状态保存到 data/window-state.json，
//! 避免依赖系统 AppData 目录，便于项目数据统一迁移和调试。
//!
//! ## 关键导出
//! - `get_custom_window_state()`: 读取指定窗口状态
//! - `save_custom_window_state()`: 保存指定窗口状态
//! - `clear_custom_window_states()`: 清空所有窗口状态
//! - `list_custom_window_states()`: 返回所有已保存窗口状态

use crate::core::data_paths;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::fs;

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

fn window_state_path() -> std::path::PathBuf {
    data_paths::data_root().join("window-state.json")
}

fn read_states() -> HashMap<String, CustomWindowState> {
    let path = window_state_path();
    let Ok(content) = fs::read_to_string(path) else {
        return HashMap::new();
    };
    serde_json::from_str(&content).unwrap_or_default()
}

fn write_states(states: &HashMap<String, CustomWindowState>) -> Result<(), String> {
    let path = window_state_path();
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).map_err(|err| err.to_string())?;
    }
    let content = serde_json::to_string_pretty(states).map_err(|err| err.to_string())?;
    fs::write(path, content).map_err(|err| err.to_string())
}

#[tauri::command]
pub async fn get_custom_window_state(label: String) -> Result<Option<CustomWindowState>, String> {
    Ok(read_states().remove(&label))
}

#[tauri::command]
pub async fn list_custom_window_states() -> Result<HashMap<String, CustomWindowState>, String> {
    Ok(read_states())
}

#[tauri::command]
pub async fn clear_custom_window_states() -> Result<(), String> {
    let path = window_state_path();
    if path.exists() {
        fs::remove_file(path).map_err(|err| err.to_string())?;
    }
    Ok(())
}

#[tauri::command]
pub async fn save_custom_window_state(
    label: String,
    state: CustomWindowState,
) -> Result<(), String> {
    let mut states = read_states();
    states.insert(label, state);
    write_states(&states)
}
