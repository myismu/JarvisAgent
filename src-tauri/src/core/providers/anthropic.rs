//! # anthropic.rs — Anthropic Messages API 提供者
//!
//! 实现 `LlmProvider` trait，构建 Anthropic 原生格式的请求体。
//! 支持扩展思考（extended thinking）模式。
//!
//! ## 关键导出
//! - `AnthropicProvider`: Anthropic 格式的 `LlmProvider` 实现

use serde_json::Value;

use crate::core::llm::api_format::ApiFormat;
use crate::core::models::*;
use crate::core::traits::LlmProvider;

/// Anthropic Messages API 提供者
pub struct AnthropicProvider;

impl LlmProvider for AnthropicProvider {
    fn api_format(&self) -> ApiFormat {
        ApiFormat::Anthropic
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
        top_k: Option<u32>,
    ) -> Value {
        let mut body = AnthropicRequest {
            model: model_id.to_string(),
            max_tokens,
            system: system_prompt.to_string(),
            messages: messages.to_vec(),
            tools,
            stream: true,
            thinking: None,
            temperature,
            top_p,
            top_k,
        };

        // 启用扩展思考模式，预算 1024 tokens
        if should_think {
            body.thinking = Some(ThinkingConfig {
                r#type: "enabled".to_string(),
                budget_tokens: Some(1024),
            });
            // thinking 模式需要更大的 max_tokens 空间
            if body.max_tokens <= 1024 {
                body.max_tokens = 4096;
            }
        }

        serde_json::to_value(body).unwrap()
    }
}
