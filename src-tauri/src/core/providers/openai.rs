use serde_json::Value;

use crate::core::api_format::ApiFormat;
use crate::core::models::*;
use crate::core::traits::LlmProvider;
use crate::core::adapters::{
    should_backfill_deepseek_reasoning_content,
    translate_messages_to_openai_with_reasoning_backfill,
    translate_tools_to_openai,
};

/// OpenAI-compatible API 提供者
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
        let backfill_reasoning =
            should_backfill_deepseek_reasoning_content(model_id, &self.base_url, should_think);
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

        let thinking_param = crate::core::registry::query_capabilities(model_id)
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
