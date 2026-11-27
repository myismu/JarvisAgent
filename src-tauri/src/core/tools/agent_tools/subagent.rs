//! # subagent.rs — 子代理工具薄入口
//!
//! 子代理完整执行引擎（独立 Agent Loop、请求构建复用、统一重试与 429 限流）
//! 已下沉至 `crate::core::agent::subagent_runner`，本模块仅保留工具层重导出。
//!
//! ## Key Exports
//! - `run_subagent()`: 重导出自 `crate::core::agent::subagent_runner::run_subagent`
//!
//! ## Dependencies
//! - Internal: `crate::core::agent::subagent_runner`

pub use crate::core::agent::subagent_runner::run_subagent;
