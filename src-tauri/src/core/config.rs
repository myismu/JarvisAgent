// --- 配置管理模块 (Config) ---
// 持久化存储 Agent 的 API 密钥、模型选择等配置项。
// 配置文件位于 Agent 家目录下的 config.json。

use serde::{Deserialize, Serialize};
use std::sync::Arc;
use tokio::sync::Mutex;

use crate::core::api_format::ApiFormat;
use crate::get_agent_home;

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
    /// 是否开启深度思考模式 (DeepSeek / Claude 3.7+)
    pub enable_thinking: Option<bool>,
    /// 模型生成的温度参数
    pub temperature: Option<f32>,
    /// 模型生成的 Top P 参数
    pub top_p: Option<f32>,
    /// 模型生成的 Top K 参数
    pub top_k: Option<u32>,
    /// 图片压缩最大宽度（像素），超过此宽度将等比缩放
    pub image_max_width: Option<u32>,
    /// 图片压缩最大高度（像素），超过此高度将等比缩放
    pub image_max_height: Option<u32>,
    /// 图片压缩质量 (0.0 ~ 1.0)，仅对 JPEG/WebP 有效
    pub image_quality: Option<f32>,
    /// [兼容旧配置] 旧版 sub_model 字段，读取后忽略（合并进 main_model）
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
            api_format: "anthropic".to_string(),
            api_key: String::new(),
            base_url: "https://api.xiaomimimo.com/anthropic/v1/messages".to_string(),
            main_model: "mimo-v2-flash".to_string(),
            utility_model: "mimo-v2-flash".to_string(),
            enable_thinking: Some(false),
            temperature: None,
            top_p: None,
            top_k: None,
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
        let mut config = self.profiles.iter()
            .find(|p| p.id == self.active_profile_id)
            .map(|p| p.config.clone())
            .unwrap_or_else(|| {
                self.profiles.first()
                    .map(|p| p.config.clone())
                    .unwrap_or_default()
            });

        // 规范化 base_url
        let mut url = config.base_url.trim_end_matches('/').to_string();
        match config.api_format_enum() {
            ApiFormat::OpenAI => {
                if !url.ends_with("/chat/completions") {
                    if !url.ends_with("/v1") {
                        url.push_str("/v1");
                    }
                    url.push_str("/chat/completions");
                }
            }
            ApiFormat::Anthropic => {
                if !url.ends_with("/messages") {
                    if !url.ends_with("/v1") {
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
    get_agent_home().join(crate::core::constants::FILE_CONFIG)
}

/// 从磁盘加载配置，支持从旧版 AgentConfig 迁移
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

/// 保存配置到磁盘
pub fn save_config(config: &AppConfig) {
    let path = config_path();
    if let Some(parent) = path.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    let _ = std::fs::write(
        &path,
        serde_json::to_string_pretty(config).unwrap_or_default(),
    );
}
