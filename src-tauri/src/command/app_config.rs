//! # app_config.rs — 应用配置持久化
//!
//! 将窗口状态、UI 偏好、技能激活等用户配置统一保存到 data/app-config.json。
//!
//! ## 关键导出
//! - `get_custom_window_state()`: 读取指定窗口状态
//! - `save_custom_window_state()`: 保存指定窗口状态
//! - `clear_custom_window_states()`: 清空所有窗口状态
//! - `list_custom_window_states()`: 返回所有已保存窗口状态
//! - `get_ui_preferences()`: 读取 UI 偏好设置
//! - `save_ui_preferences()`: 保存 UI 偏好设置并通知所有窗口
//! - `get_skill_activations()`: 读取所有技能激活状态
//! - `set_skill_active()`: 设置指定技能的激活状态
//!
//! ## 依赖
//! - Internal: `infra::config::data_paths`
//! - External: `serde`, `serde_json`, `tauri`

use crate::infra::config::data_paths;
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
    /// 权限档位："request_approval"（请求审批，默认）/ "auto_approve"（帮我批准）
    #[serde(default = "default_approval_mode")]
    pub agent_approval_mode: String,
    #[serde(default = "default_locale")]
    pub locale: String,
    /// 图片压缩档位："eco"（省流）/ "standard"（标准）/ "hd"（高清）。
    ///
    /// 档位是**用户的选择意图**，宽高数值是它的实现细节，两者一起落库：
    /// - 档位决定界面高亮哪一格，也决定了将来调整某档参数时该档用户跟随新参数；
    /// - 数值是压缩算法的直接输入，`TerminalInput.vue` 不需要再查表换算。
    ///
    /// 两者**必须同时写入**（前端 watcher 负责同步），只写其一会造成"显示档位与
    /// 实际压缩参数不一致"。`normalize_image_tier()` 负责在读取时自愈。
    #[serde(default = "default_image_compress_tier")]
    pub image_compress_tier: String,
    /// 图片压缩最大宽度（像素）。前端「省流/标准/高清」三档的数值形态，默认标准档 1568。
    #[serde(default = "default_image_max_width")]
    pub image_max_width: u32,
    /// 图片压缩最大高度（像素）
    #[serde(default = "default_image_max_height")]
    pub image_max_height: u32,
    /// 图片压缩质量 (0.0 ~ 1.0)。
    ///
    /// 刻意不暴露到界面：JPEG 质量只影响体积和肉眼看感，**不影响 token 数**
    /// （token 只由宽高决定），做成档位只会增加用户负担。
    #[serde(default = "default_image_quality")]
    pub image_quality: f32,
    /// 向后兼容：读取旧 agent_display_mode 字段
    #[serde(default)]
    agent_display_mode: Option<String>,
    #[serde(default = "default_agent_opacity")]
    pub agent_message_opacity: i32,
    #[serde(default = "default_opacity")]
    pub user_message_opacity: i32,
}

fn default_font_size() -> i32 { 15 }
fn default_code_font_size() -> i32 { 13 }
fn default_true() -> bool { true }
fn default_agent_panel_position() -> String { "right".to_string() }
fn default_agent_audience() -> String { "developer".to_string() }
fn default_agent_work_mode() -> String { "edit".to_string() }
fn default_approval_mode() -> String { "request_approval".to_string() }
fn default_locale() -> String { "zh-CN".to_string() }
fn default_image_compress_tier() -> String { "standard".to_string() }
/// 图片压缩默认档位 = **标准档**，与前端 `usePreferences.ts` 的 `IMAGE_COMPRESS_TIERS.standard` 同源。
///
/// 为什么是 1568×896 而不是 1920×1080：1568 是 Anthropic 服务端的长边硬上限，
/// 传更大的图既多花上传流量、又多耗 token，模型却仍然只看到 1568（约 1900 token/张 vs 2800）。
fn default_image_max_width() -> u32 { 1568 }
fn default_image_max_height() -> u32 { 896 }
fn default_image_quality() -> f32 { 0.8 }
fn default_opacity() -> i32 { 100 }
fn default_agent_opacity() -> i32 { 0 }

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
            agent_approval_mode: "request_approval".to_string(),
            locale: "zh-CN".to_string(),
            image_compress_tier: "standard".to_string(),
            image_max_width: 1568,
            image_max_height: 896,
            image_quality: 0.8,
            agent_display_mode: None,
            agent_message_opacity: 0,
            user_message_opacity: 100,
        }
    }
}

impl UiPreferences {
    /// 兼容旧版本 agent_display_mode，迁移到双轴。
    ///
    /// 旧「普通用户」= 只读，因此迁移为 audience=user + 只读保护开启 + 编辑模式。
    pub fn migrate_legacy_display_mode(&mut self) {
        if let Some(legacy) = self.agent_display_mode.take() {
            if self.agent_audience == default_agent_audience()
                && self.agent_work_mode == default_agent_work_mode()
            {
                match legacy.as_str() {
                    "user" => {
                        self.agent_audience = "user".to_string();
                        self.agent_work_mode = "edit".to_string();
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

    /// 兼容旧版本：`chat` 曾是用户可选的工作模式（只读保护），第二步起该模式取消，
    /// 迁移为"编辑模式 + 请求审批档"——安全等价：改动都会先问用户。
    pub fn migrate_legacy_work_mode(&mut self) {
        if self.agent_work_mode == "chat" {
            self.agent_approval_mode = default_approval_mode();
            self.agent_work_mode = "edit".to_string();
        }
        // 工作模式收敛为 edit / plan
        if !matches!(self.agent_work_mode.as_str(), "edit" | "plan") {
            self.agent_work_mode = "edit".to_string();
        }
        // 档位收敛
        if !matches!(
            self.agent_approval_mode.as_str(),
            "request_approval" | "auto_approve"
        ) {
            self.agent_approval_mode = default_approval_mode();
        }
    }

    /// 自愈图片压缩档位，保持"档位 ↔ 宽高数值"一致。
    ///
    /// 为什么要它：这两个字段来自前端两次独立的赋值，任何一种历史遗留都可能让它们对不上 ——
    /// 例如旧版本只存了数值（那时还没有档位字段）、或将来某档参数被调整。
    /// 处理原则是**以数值为准反推档位**，因为数值才是压缩算法实际用的东西，
    /// 反推出来的档位才是界面该显示的事实；数值对不上任何档时收敛到标准档并校正数值。
    pub fn normalize_image_tier(&mut self) {
        // (档位, 宽, 高)。与前端 `usePreferences.ts` 的 `IMAGE_COMPRESS_TIERS` 必须一致。
        const TIERS: [(&str, u32, u32); 3] = [
            ("eco", 1280, 720),
            ("standard", 1568, 896),
            ("hd", 1920, 1080),
        ];
        if let Some((tier, _, _)) = TIERS
            .iter()
            .find(|(_, w, h)| *w == self.image_max_width && *h == self.image_max_height)
        {
            self.image_compress_tier = (*tier).to_string();
            return;
        }
        // 数值不在任何档上（旧自定义值）：收敛到标准档，顺带把数值也校正
        self.image_compress_tier = default_image_compress_tier();
        self.image_max_width = default_image_max_width();
        self.image_max_height = default_image_max_height();
        self.image_quality = default_image_quality();
    }
}

/// 顶层配置文件结构
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct AppConfigFile {
    #[serde(default)]
    windows: HashMap<String, CustomWindowState>,
    #[serde(default = "UiPreferences::default")]
    ui_preferences: UiPreferences,
    /// 技能激活状态：skill_name → active。未记录的技能默认激活。
    #[serde(default)]
    skills: HashMap<String, bool>,
}

fn app_config_path() -> std::path::PathBuf {
    data_paths::data_root().join("app-config.json")
}

fn old_config_path() -> std::path::PathBuf {
    data_paths::data_root().join("window-state.json")
}

fn read_file() -> AppConfigFile {
    let path = app_config_path();

    // 旧文件迁移：如果新文件不存在但旧文件存在，重命名
    if !path.exists() {
        let old = old_config_path();
        if old.exists() {
            let _ = fs::rename(&old, &path);
        }
    }

    let Ok(content) = fs::read_to_string(&path) else {
        return AppConfigFile {
            windows: HashMap::new(),
            ui_preferences: UiPreferences::default(),
            skills: HashMap::new(),
        };
    };
    serde_json::from_str(&content).unwrap_or(AppConfigFile {
        windows: HashMap::new(),
        ui_preferences: UiPreferences::default(),
        skills: HashMap::new(),
    })
}

fn write_file(file: &AppConfigFile) -> Result<(), String> {
    let path = app_config_path();
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).map_err(|err| err.to_string())?;
    }
    let content = serde_json::to_string_pretty(file).map_err(|err| err.to_string())?;
    fs::write(path, content).map_err(|err| err.to_string())
}

/// 读取技能激活状态，未记录的技能默认激活
pub fn get_skill_activation(skill_name: &str) -> bool {
    let file = read_file();
    file.skills.get(skill_name).copied().unwrap_or(true)
}

/// 读取所有技能激活状态
pub fn get_all_skill_activations() -> HashMap<String, bool> {
    read_file().skills
}

/// 设置技能激活状态
pub fn set_skill_activation(skill_name: &str, active: bool) -> Result<(), String> {
    let mut file = read_file();
    file.skills.insert(skill_name.to_string(), active);
    write_file(&file)
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
    prefs.migrate_legacy_work_mode();
    prefs.normalize_image_tier();
    Ok(prefs)
}

#[tauri::command]
pub async fn save_ui_preferences(
    app: tauri::AppHandle,
    preferences: UiPreferences,
) -> Result<(), String> {
    let mut file = read_file();
    let mut incoming = preferences;
    // 前端只会送 edit / plan；出现 chat 说明是旧版界面，按只读保护落库
    incoming.migrate_legacy_work_mode();
    // 落库前先自愈档位，防止前端漏送某一半字段时把不一致的状态写进磁盘
    incoming.normalize_image_tier();
    file.ui_preferences = incoming;
    write_file(&file)?;
    // 通知所有窗口（包括监控窗口）偏好已更新
    let _ = app.emit("ui-preferences-changed", ());
    Ok(())
}

// ── Skill activation commands ──

#[tauri::command]
pub async fn get_skill_activations() -> Result<HashMap<String, bool>, String> {
    Ok(get_all_skill_activations())
}

#[tauri::command]
pub async fn set_skill_active(skill_name: String, active: bool) -> Result<(), String> {
    set_skill_activation(&skill_name, active)
}

#[cfg(test)]
mod image_tier_tests {
    use super::*;

    fn prefs_with(tier: &str, w: u32, h: u32) -> UiPreferences {
        UiPreferences {
            image_compress_tier: tier.to_string(),
            image_max_width: w,
            image_max_height: h,
            ..UiPreferences::default()
        }
    }

    /// 数值落在某一档上 → 档位被反推成该档（数值是事实来源）
    #[test]
    fn tier_derived_from_dimensions() {
        let mut p = prefs_with("standard", 1920, 1080);
        p.normalize_image_tier();
        assert_eq!(p.image_compress_tier, "hd");

        let mut p = prefs_with("standard", 1280, 720);
        p.normalize_image_tier();
        assert_eq!(p.image_compress_tier, "eco");
    }

    /// 数值不在任何档上 → 收敛到标准档，且**数值被一起校正**（关键回归：
    /// 曾经用 Object.assign 写错了字段名，导致数值没改、还多出废字段）
    #[test]
    fn unknown_dimensions_fall_back_to_standard() {
        let mut p = prefs_with("hd", 1000, 600);
        p.normalize_image_tier();
        assert_eq!(p.image_compress_tier, "standard");
        assert_eq!(p.image_max_width, 1568);
        assert_eq!(p.image_max_height, 896);
        assert_eq!(p.image_quality, 0.8);
    }

    /// 前端漏送档位字段时（serde 用默认值补齐），不会把已存的数值带偏
    #[test]
    fn missing_tier_field_does_not_disturb_dimensions() {
        let mut p = prefs_with(&default_image_compress_tier(), 1280, 720);
        p.normalize_image_tier();
        assert_eq!(p.image_compress_tier, "eco");
        assert_eq!(p.image_max_width, 1280);
        assert_eq!(p.image_max_height, 720);
    }
}
