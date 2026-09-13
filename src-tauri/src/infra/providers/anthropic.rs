//! # anthropic.rs — Anthropic Messages API 提供者
//!
//! 实现 `LlmProvider` trait，构建 Anthropic 原生格式的请求体。
//! 支持扩展思考（extended thinking）模式。
//!
//! ## 关键导出
//! - `AnthropicProvider`: Anthropic 格式的 `LlmProvider` 实现

use serde_json::Value;

use crate::infra::llm::api_format::ApiFormat;
use crate::infra::types::models::*;
use crate::infra::types::traits::LlmProvider;

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
        let mut messages = messages.to_vec();
        // 出网前把内部 Context 块降级为普通 Text（协议不认 "context" 类型）
        crate::infra::llm::adapters::materialize_context_blocks_for_wire(&mut messages);
        // 丢弃无 signature 的 thinking 块（回传给 Anthropic 会被判 400）
        messages = crate::infra::llm::adapters::strip_unsigned_thinking_for_anthropic(&messages);

        let mut body = AnthropicRequest {
            model: model_id.to_string(),
            max_tokens,
            system: system_prompt.to_string(),
            messages,
            tools,
            stream: true,
            thinking: None,
            temperature,
            top_p,
            top_k,
        };

        body.thinking = Some(ThinkingConfig {
            r#type: Some(if should_think { "enabled" } else { "disabled" }.to_string()),
            budget_tokens: if should_think { Some(1024) } else { None },
            enable: None,
        });
        if should_think && body.max_tokens <= 1024 {
            body.max_tokens = 4096;
        }

        serde_json::to_value(body).unwrap()
    }
}
