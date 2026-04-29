//! # openai.rs — OpenAI-compatible API 提供者
//!
//! 实现 `LlmProvider` trait，构建 OpenAI Chat Completions 格式的请求体。
//! 通过模型注册表动态适配不同厂商的思考模式参数（reasoning_effort / thinking / thinkingBudget / enable_thinking）。
//!
//! ## 关键导出
//! - `OpenAIProvider`: OpenAI-compatible 格式的 `LlmProvider` 实现（需传入 `base_url`）
//!
//! ## 约束
//! - DeepSeek 模型需要 backfill reasoning_content 到 thinking block
//! - 思考模式参数通过 `registry::query_capabilities()` 动态查询

use serde_json::Value;

use crate::core::llm::api_format::ApiFormat;
use crate::core::models::*;
use crate::core::traits::LlmProvider;
use crate::core::llm::adapters::{
    should_backfill_deepseek_reasoning_content,
    translate_messages_to_openai_with_reasoning_backfill,
    translate_tools_to_openai,
};

/// OpenAI-compatible API 提供者（兼容 DeepSeek、Qwen、Gemini 等）
pub struct OpenAIProvider {
    pub base_url: String,
}

impl OpenAIProvider {
    pub fn new(base_url: String) -> Self {
        Self { base_url }
    }
}

impl LlmProvider for OpenAIProvider {
    fn api_format(&self) -> ApiFormat {
        ApiFormat::OpenAI
    }

    fn build_request_body(
        &self,
        model_id: &str,
        system_prompt: &str,
        messages: &[Message],
        tools: Vec<Value>,
        should_think: bool,
        max_tokens: i32,
        temperature: Option<f32>,
        top_p: Option<f32>,
        _top_k: Option<u32>,
    ) -> Value {
        // DeepSeek 模型需要将 reasoning_content backfill 到 thinking block
        let backfill_reasoning =
            should_backfill_deepseek_reasoning_content(model_id, &self.base_url, should_think);
        // 将 Anthropic 格式消息/工具转换为 OpenAI 格式
        let openai_msgs = translate_messages_to_openai_with_reasoning_backfill(
            system_prompt,
            messages,
            backfill_reasoning,
        );
        let openai_tools = translate_tools_to_openai(&tools);

        let mut openai_req = OpenAIRequest {
            model: model_id.to_string(),
            max_tokens: Some(max_tokens),
            messages: openai_msgs,
            tools: if openai_tools.is_empty() {
                None
            } else {
                Some(openai_tools)
            },
            stream: true,
            stream_options: Some(StreamOptions {
                include_usage: true,
            }),
            reasoning_effort: None,
            thinking: None,
            thinking_budget: None,
            enable_thinking: None,
            temperature,
            top_p,
        };

        // 根据模型注册表的 thinking_param 字段，选择对应的思考模式参数
        let thinking_param = crate::core::llm::registry::query_capabilities(model_id)
            .and_then(|c| c.thinking_param);

        match thinking_param.as_deref() {
            Some("reasoning_effort") => {
                if should_think {
                    openai_req.reasoning_effort = Some("high".to_string());
                }
            }
            Some("thinking") => {
                openai_req.thinking = Some(ThinkingConfig {
                    r#type: if should_think {
                        "enabled".to_string()
                    } else {
                        "disabled".to_string()
                    },
                    budget_tokens: None,
                });
            }
            Some("thinkingBudget") => {
                openai_req.thinking_budget = Some(if should_think { 8192 } else { 0 });
            }
            Some("enable_thinking") => {
                openai_req.enable_thinking = Some(should_think);
            }
            _ => {
                if should_think {
                    openai_req.reasoning_effort = Some("high".to_string());
                }
            }
        }

        serde_json::to_value(openai_req).unwrap()
    }
}
