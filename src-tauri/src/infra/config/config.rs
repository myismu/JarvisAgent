//! # config.rs — 配置管理模块
//!
//! 持久化存储 Agent 的 API 密钥、模型选择等配置项。配置文件位于 Agent 家目录下的 config.json。
//! 支持多预设配置管理，允许用户快速切换不同的模型配置。
//!
//! ## 关键导出
//! - `AgentConfig`: 单个模型的连接配置结构体
//! - `ModelProfile`: 模型预设配置（包含名称和配置）
//! - `AppConfig`: 顶级应用配置，管理多个预设
//! - `ConfigState`: Tauri 状态管理器，用于全局配置访问
//! - `load_config()`: 从磁盘加载配置，支持旧版迁移
//! - `save_config()`: 保存配置到磁盘
//!
//! ## 依赖
//! - Internal: `crate::infra::llm::api_format::ApiFormat`, `crate::get_agent_home`
//! - External: `serde`, `tokio`, `std::sync::Arc`
//!
//! ## 约束
//! - 配置文件路径由 `get_agent_home()` 决定，必须在应用初始化后使用
//! - 旧版 `AgentConfig` 会自动迁移到新版 `AppConfig` 格式

use serde::{Deserialize, Serialize};
use std::sync::Arc;
use tokio::sync::Mutex;

use crate::infra::llm::api_format::ApiFormat;

/// Agent 配置结构体（代表单个模型的连接信息）
#[derive(Serialize, Deserialize, Debug, Clone)]
#[serde(rename_all = "camelCase", default)]
pub struct AgentConfig {
    /// API 格式 (anthropic 或 openai)
    pub api_format: String,
    /// API 密钥
    pub api_key: String,
    /// API 基础 URL
    pub base_url: String,
    /// 主对话模型 ID（主代理 + 子代理共用此模型）
    pub main_model: String,
    /// 意图分类器 / 记忆 Agent 使用的模型 ID（可用更便宜的模型）
    pub utility_model: String,
    /// 模型生成的温度参数
    pub temperature: Option<f32>,
    /// 模型生成的 Top P 参数
    pub top_p: Option<f32>,
    /// 模型生成的 Top K 参数
    pub top_k: Option<u32>,
    /// Reflection 模式: "always" | "smart" | "off"
    #[serde(default = "default_reflection_mode")]
    pub reflection_mode: String,
    /// [兼容旧配置] 深度思考默认档位曾挂在**模型预设**上（`profiles[].config.thinkingDefault`）。
    ///
    /// 2026-09-19 重构后该语义已移到全局设置（`UiPreferences.thinking_default`），
    /// 因为"每轮现读 active_config()"会让改任一副预设都倒灌所有会话。
    /// 此字段仅用于**读取老 config.json 时兼容**，不做任何业务读取；
    /// 序列化时跳过，因此新写的配置不会再产生它。
    #[serde(default, skip_serializing)]
    pub thinking_default: Option<String>,
    /// 用户自定义 max_tokens（覆盖模型注册表默认值），None 表示使用注册表值
    pub max_tokens: Option<i32>,
    /// [兼容旧配置] 旧版图片/子模型字段，读取后忽略
    #[serde(default, skip_serializing)]
    pub image_max_width: Option<u32>,
    #[serde(default, skip_serializing)]
    pub image_max_height: Option<u32>,
    #[serde(default, skip_serializing)]
    pub image_quality: Option<f32>,
    #[serde(default, skip_serializing)]
    pub sub_model: Option<String>,
}

impl AgentConfig {
    /// 将 api_format 字符串转换为 ApiFormat 枚举
    pub fn api_format_enum(&self) -> ApiFormat {
        ApiFormat::from_str(&self.api_format)
    }
}

impl Default for AgentConfig {
    fn default() -> Self {
        Self {
            api_format: "openai".to_string(),
            api_key: String::new(),
            base_url: "https://api.xiaomimimo.com/v1/chat/completions".to_string(),
            main_model: "mimo-v2-flash".to_string(),
            utility_model: "mimo-v2-flash".to_string(),
            temperature: None,
            top_p: None,
            top_k: None,
            reflection_mode: default_reflection_mode(),
            // 兼容字段：新配置不再写入（见字段文档）
            thinking_default: None,
            max_tokens: None,
            image_max_width: None,
            image_max_height: None,
            image_quality: None,
            sub_model: None,
        }
    }
}

/// 模型预设
#[derive(Serialize, Deserialize, Debug, Clone)]
#[serde(rename_all = "camelCase")]
pub struct ModelProfile {
    pub id: String,
    pub name: String,
    pub config: AgentConfig,
}

fn default_reflection_mode() -> String {
    "smart".to_string()
}

fn default_global_profile_id() -> String {
    "default".to_string()
}

/// 顶级应用配置
#[derive(Serialize, Deserialize, Debug, Clone)]
#[serde(rename_all = "camelCase")]
pub struct AppConfig {
    pub active_profile_id: String,
    #[serde(default = "default_global_profile_id")]
    pub global_profile_id: String,
    pub profiles: Vec<ModelProfile>,
}

impl Default for AppConfig {
    fn default() -> Self {
        let default_profile = ModelProfile {
            id: "default".to_string(),
            name: "默认预设".to_string(),
            config: AgentConfig::default(),
        };
        Self {
            active_profile_id: "default".to_string(),
            global_profile_id: "default".to_string(),
            profiles: vec![default_profile],
        }
    }
}

impl AppConfig {
    /// 获取当前激活的配置
    pub fn active_config(&self) -> AgentConfig {
        let mut config = self
            .profiles
            .iter()
            .find(|p| p.id == self.active_profile_id)
            .map(|p| p.config.clone())
            .unwrap_or_else(|| {
                self.profiles
                    .first()
                    .map(|p| p.config.clone())
                    .unwrap_or_default()
            });

        // 规范化 base_url：剥离 query string 后再检查后缀，防误判
        let mut url = config.base_url.trim_end_matches('/').to_string();
        let url_for_check = url.split('?').next().unwrap_or(&url);
        match config.api_format_enum() {
            ApiFormat::OpenAI => {
                if !url_for_check.ends_with("/chat/completions") {
                    if !url_for_check.ends_with("/v1") {
                        url.push_str("/v1");
                    }
                    url.push_str("/chat/completions");
                }
            }
            ApiFormat::Anthropic => {
                if !url_for_check.ends_with("/messages") {
                    if !url_for_check.ends_with("/v1") {
                        url.push_str("/v1");
                    }
                    url.push_str("/messages");
                }
            }
        }
        config.base_url = url;

        config
    }
}

/// 全局配置状态（Tauri State）
pub struct ConfigState(pub Arc<Mutex<AppConfig>>);

/// 获取配置文件路径
fn config_path() -> std::path::PathBuf {
    crate::infra::config::data_paths::config_path()
}

/// 从磁盘加载配置，支持从单配置文件格式迁移
pub fn load_config() -> AppConfig {
    let path = config_path();

    if !path.exists() {
        return AppConfig::default();
    }

    let content = match std::fs::read_to_string(&path) {
        Ok(c) => c,
        Err(_) => return AppConfig::default(),
    };

    // 尝试解析为新版 AppConfig
    if let Ok(app_config) = serde_json::from_str::<AppConfig>(&content) {
        return app_config;
    }

    // 尝试解析为旧版 AgentConfig 并迁移
    if let Ok(old_config) = serde_json::from_str::<AgentConfig>(&content) {
        println!("[Config] 检测到旧版配置，正在迁移...");
        return AppConfig {
            active_profile_id: "default".to_string(),
            global_profile_id: "default".to_string(),
            profiles: vec![ModelProfile {
                id: "default".to_string(),
                name: "我的预设".to_string(),
                config: old_config,
            }],
        };
    }

    AppConfig::default()
}

/// 保存配置到磁盘（原子写入：先写临时文件，再 rename）
pub fn save_config(config: &AppConfig) -> Result<(), String> {
    let path = config_path();
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).map_err(|e| format!("创建配置目录失败: {}", e))?;
    }
    let json =
        serde_json::to_string_pretty(config).map_err(|e| format!("序列化配置失败: {}", e))?;
    let tmp = path.with_extension("tmp");
    std::fs::write(&tmp, &json).map_err(|e| format!("写入配置失败: {}", e))?;
    std::fs::rename(&tmp, &path).map_err(|e| format!("保存配置失败: {}", e))?;
    Ok(())
}

/// 校验配置必填字段
pub fn validate_config(config: &AppConfig) -> Result<(), String> {
    for profile in &config.profiles {
        if profile.name.trim().is_empty() {
            return Err("预设名称不能为空".to_string());
        }
        if profile.config.api_key.trim().is_empty() {
            return Err(format!("预设「{}」的 API Key 不能为空", profile.name));
        }
        if profile.config.base_url.trim().is_empty() {
            return Err(format!("预设「{}」的 Base URL 不能为空", profile.name));
        }
        if profile.config.main_model.trim().is_empty() {
            return Err(format!("预设「{}」的主模型不能为空", profile.name));
        }
        // 注：深度思考默认档位已不在预设上（2026-09-19 移到全局 `UiPreferences`），
        // 其值域校验见 `command::app_config::save_ui_preferences`。
    }
    Ok(())
}
