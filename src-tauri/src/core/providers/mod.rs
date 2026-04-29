//! # mod.rs — LLM 提供者实现模块
//!
//! 包含 `LlmProvider` trait 的具体实现：Anthropic 原生格式和 OpenAI-compatible 格式。
//!
//! ## 子模块
//! - `anthropic`: Anthropic Messages API 格式构建
//! - `openai`: OpenAI Chat Completions API 格式构建（兼容 DeepSeek、Qwen 等）

pub mod anthropic;
pub mod openai;
