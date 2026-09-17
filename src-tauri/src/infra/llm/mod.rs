//! LLM 服务抽象层
//!
//! 封装大语言模型调用相关的所有功能：
//! - `api_format`: API 协议格式定义（Anthropic / OpenAI）
//! - `api_client`: HTTP 客户端，含重试机制和流式请求
//! - `adapters`: 消息格式转换适配器（Anthropic ↔ OpenAI）
//! - `registry`: 模型能力注册表，编译时内嵌 model_registry.json
//! - `usage`: 各家 `usage` 字段归一化（含缓存命中字段的候选表探测）
//! - `usage_memory`: 端点级 usage 能力自动记忆（`data/global/model_caps.json`）
//! - `context_budget`: 上下文压缩判据（主 Agent 与子代理共用的唯一口径）

pub mod adapters;
pub mod api_client;
pub mod api_format;
pub mod context_budget;
pub mod registry;
pub mod token_count;
pub mod usage;
pub mod usage_memory;
