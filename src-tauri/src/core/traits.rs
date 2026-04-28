use async_trait::async_trait;
use serde_json::Value;

use crate::core::api_format::ApiFormat;
use crate::core::models::Message;

/// LLM 提供者抽象：统一 Anthropic/OpenAI 的请求构建与格式差异
pub trait LlmProvider: Send + Sync {
    fn api_format(&self) -> ApiFormat;

    /// 构建 LLM API 请求体（JSON）
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
    ) -> Value;

    /// 是否为流式请求
    fn stream(&self) -> bool {
        true
    }
}

/// 工具执行抽象：便于测试时 mock 工具调用
#[async_trait]
pub trait ToolExecutor: Send + Sync {
    async fn execute_tool(
        &self,
        app: &tauri::AppHandle,
        name: &str,
        input: &Value,
        session_id: &str,
    ) -> (String, u64, u64);
}
