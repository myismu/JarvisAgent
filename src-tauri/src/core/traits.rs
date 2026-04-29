//! # traits.rs — 抽象接口定义模块
//!
//! 定义核心抽象接口（trait），用于解耦具体实现。
//! 主要包括 LLM 提供者抽象和工具执行抽象。
//!
//! ## 关键导出
//! - `LlmProvider`: LLM 提供者抽象，统一 Anthropic/OpenAI 的请求构建与格式差异
//! - `ToolExecutor`: 工具执行抽象，便于测试时 mock 工具调用
//!
//! ## 依赖
//! - Internal: `crate::core::llm::api_format::ApiFormat`, `crate::core::models::Message`
//! - External: `async_trait`, `serde_json`
//!
//! ## 约束
//! - 实现 `LlmProvider` 必须处理 API 格式差异
//! - 实现 `ToolExecutor` 必须处理工具执行和权限检查
//! - 所有 trait 方法都是异步的，需要在 Tokio 运行时中调用

use async_trait::async_trait;
use serde_json::Value;

use crate::core::llm::api_format::ApiFormat;
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
