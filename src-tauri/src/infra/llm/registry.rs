// --- 模型能力注册表 ---
// 从 model_registry.json 编译时内嵌，提供模型能力查询接口。
// 使用 include_str!() 宏确保数据随二进制一起打包，无需运行时外部文件。

use serde::{Deserialize, Serialize};

/// 单个模型的能力描述
#[derive(Serialize, Deserialize, Debug, Clone)]
#[serde(rename_all = "camelCase")]
pub struct ModelCapabilities {
    /// 是否支持流式输出
    pub streaming: bool,
    /// 是否支持深度思考模式（可通过参数控制）
    pub thinking: bool,
    /// 控制思考的参数名（如 "thinking", "reasoning_effort", "enable_thinking"）
    #[serde(skip_serializing_if = "Option::is_none")]
    pub thinking_param: Option<String>,
    /// 是否支持温度参数（部分推理模型开启思考后不可调）
    pub temperature: bool,
    /// 是否支持视觉/多模态
    #[serde(default)]
    pub vision: bool,
    /// 最大输出 token 数
    #[serde(default)]
    pub max_tokens: u32,
    /// 最大上下文窗口 token 数
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_context_tokens: Option<u32>,
    /// 备注说明
    #[serde(default)]
    pub notes: String,
}

/// 注册表中的单条模型记录
#[derive(Serialize, Deserialize, Debug, Clone)]
#[serde(rename_all = "camelCase")]
pub struct ModelRegistryEntry {
    /// 模型 ID（用于精确或模糊匹配）
    pub id: String,
    /// 服务商名称
    pub provider: String,
    /// 用户友好的显示名称
    pub display_name: String,
    /// 推荐的 API 格式
    pub api_format: String,
    /// 能力描述
    pub capabilities: ModelCapabilities,
}

/// 注册表根结构
#[derive(Deserialize, Debug)]
struct RegistryRoot {
    models: Vec<ModelRegistryEntry>,
}

/// 编译时内嵌注册表 JSON，确保打包后无需外部文件
const REGISTRY_JSON: &str = include_str!("../../../model_registry.json");

/// 加载全量注册表列表
pub fn load_registry() -> Vec<ModelRegistryEntry> {
    match serde_json::from_str::<RegistryRoot>(REGISTRY_JSON) {
        Ok(root) => root.models,
        Err(e) => {
            // 编译时内嵌，理论上不会失败；若失败则说明 JSON 格式错误
            eprintln!("[Registry] 解析 model_registry.json 失败: {}", e);
            vec![]
        }
    }
}

/// 按模型 ID 查询能力（精确匹配优先，后降级为前缀模糊匹配）
///
/// 匹配策略：
/// 1. 精确匹配 id
/// 2. 注册表中的 id 是用户输入的前缀（如 "deepseek-v4" 匹配 "deepseek-v4-pro"）
/// 3. 用户输入是注册表 id 的前缀
pub fn query_capabilities(model_id: &str) -> Option<ModelCapabilities> {
    let models = load_registry();
    let lower = model_id.to_lowercase();

    // 1. 精确匹配
    if let Some(entry) = models.iter().find(|m| m.id.to_lowercase() == lower) {
        return Some(entry.capabilities.clone());
    }

    // 2. 模糊匹配：注册表 id 包含用户输入，或用户输入包含注册表 id
    let fuzzy = models.iter().find(|m| {
        let mid = m.id.to_lowercase();
        lower.contains(&mid) || mid.contains(&lower)
    });
    fuzzy.map(|e| e.capabilities.clone())
}

/// Tauri 命令：前端查询指定模型的能力
/// 返回 Option<ModelCapabilities>，None 表示注册表中无此模型
#[tauri::command]
pub fn get_model_capabilities(model_id: String) -> Option<ModelCapabilities> {
    query_capabilities(&model_id)
}

/// 统一入口：根据模型注册表的 thinkingParam，向 OpenAIRequest 写入思考参数
/// should_think=true 开启，false 显式关闭。不匹配或无能力时不做任何操作。
pub fn apply_thinking_for_model(
    req: &mut crate::infra::types::models::OpenAIRequest,
    model_id: &str,
    should_think: bool,
) {
    let Some(caps) = query_capabilities(model_id) else { return };
    if !caps.thinking { return; }
    match caps.thinking_param.as_deref() {
        Some("reasoning_effort") => {
            if should_think {
                req.reasoning_effort = Some("high".to_string());
            }
        }
        Some("thinking") => {
            req.thinking = Some(crate::infra::types::models::ThinkingConfig {
                r#type: Some(if should_think { "enabled" } else { "disabled" }.to_string()),
                budget_tokens: None,
                enable: None,
            });
        }
        Some("thinkingBudget") => {
            req.thinking_budget = Some(if should_think { 8192 } else { 0 });
        }
        Some("enable_thinking") => {
            req.enable_thinking = Some(should_think);
        }
        Some("extra_thinking") | Some("extra_enable_thinking") => {
            let key = if caps.thinking_param.as_deref() == Some("extra_enable_thinking") {
                "enable_thinking"
            } else {
                "thinking"
            };
            req.extra_body = Some(serde_json::json!({ key: should_think }));
        }
        Some("extra_chain_of_thought") => {
            req.extra_body = Some(serde_json::json!({ "chain_of_thought": should_think }));
        }
        Some("thinking_enable") => {
            req.thinking = Some(crate::infra::types::models::ThinkingConfig {
                r#type: None,
                budget_tokens: None,
                enable: Some(should_think),
            });
        }
        Some("enable_thought") => {
            req.parameters = Some(serde_json::json!({ "enable_thought": should_think }));
        }
        _ => {
            if should_think {
                req.reasoning_effort = Some("high".to_string());
            }
        }
    }
}

/// Tauri 命令：返回完整的注册表列表（用于前端下拉选择）
#[tauri::command]
pub fn list_model_registry() -> Vec<ModelRegistryEntry> {
    load_registry()
}
