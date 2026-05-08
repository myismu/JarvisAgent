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
//! - Internal: `crate::core::llm::api_format::ApiFormat`, `crate::get_agent_home`
//! - External: `serde`, `tokio`, `std::sync::Arc`
//!
//! ## 约束
//! - 配置文件路径由 `get_agent_home()` 决定，必须在应用初始化后使用
//! - 旧版 `AgentConfig` 会自动迁移到新版 `AppConfig` 格式

use serde::{Deserialize, Serialize};
use std::sync::Arc;
use tokio::sync::Mutex;

use crate::core::llm::api_format::ApiFormat;

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

/// 运行时可调参数 — 集中管理所有内部阈值、间隔和限制
#[derive(Serialize, Deserialize, Debug, Clone)]
#[serde(rename_all = "camelCase", default)]
pub struct RuntimeSettings {
    /// 最大上下文 token 数
    pub max_tokens_context: i32,
    /// 触发自动压缩的 token 阈值
    pub max_tokens_compact_trigger: usize,
    /// 强制用户确认前的最大循环次数
    pub max_agent_loop_before_confirm: usize,
    /// 绝对循环上限
    pub max_agent_loop_absolute: usize,
    /// 后台任务输出最大长度
    pub max_background_output_len: usize,
    /// 后台通知消息最大长度
    pub max_background_notify_len: usize,
    /// 快照最大保留天数
    pub gc_max_age_days: u64,
    /// 是否保护分支头节点
    pub gc_keep_branch_heads: bool,
    /// 合并冲突阈值
    pub merge_conflict_threshold: usize,
    /// Thinking 预算 token 数
    pub thinking_budget_tokens: u32,
    /// API 重试次数
    pub api_retry_count: u32,
    /// 子 Agent 心跳间隔（秒）
    pub heartbeat_interval_secs: u64,
    /// 子 Agent 事件历史保留上限
    pub subagent_event_history_limit: usize,
    /// 后台任务 TTL（秒）
    pub background_task_ttl_secs: u64,
}

impl Default for RuntimeSettings {
    fn default() -> Self {
        Self {
            max_tokens_context: 8192,
            max_tokens_compact_trigger: 50000,
            max_agent_loop_before_confirm: 30,
            max_agent_loop_absolute: 500,
            max_background_output_len: 50000,
            max_background_notify_len: 500,
            gc_max_age_days: 30,
            gc_keep_branch_heads: true,
            merge_conflict_threshold: 10,
            thinking_budget_tokens: 1024,
            api_retry_count: 3,
            heartbeat_interval_secs: 5,
            subagent_event_history_limit: 200,
            background_task_ttl_secs: 3600,
        }
    }
}

/// 全局配置状态（Tauri State）
pub struct ConfigState(pub Arc<Mutex<AppConfig>>);

/// 运行时配置状态（Tauri State）
pub struct RuntimeConfigState(pub RuntimeSettings);

/// 获取配置文件路径
fn config_path() -> std::path::PathBuf {
    crate::core::data_paths::config_path()
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
    let json = serde_json::to_string_pretty(config).map_err(|e| format!("序列化配置失败: {}", e))?;
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
    }
    Ok(())
}
