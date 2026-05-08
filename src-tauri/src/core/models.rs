//! # models.rs — 数据模型定义模块
//!
//! 定义应用中所有核心数据结构，包括消息格式、API 请求/响应格式、会话记忆等。
//! 支持 Anthropic 和 OpenAI 两种 API 格式。
//!
//! ## 关键导出
//! - 消息格式: `Message`, `Content`, `ContentBlock`
//! - Anthropic 格式: `AnthropicRequest`, `ThinkingConfig`, `ImageSource`
//! - OpenAI 格式: `OpenAIRequest`, `OpenAIMessage`, `OpenAITool`, `OpenAIToolCall`
//! - 会话数据: `SessionMemory`, `SessionContextSnapshot`, `AgentStep`, `PlanDocument`
//! - 任务管理: `Task`, `TaskStatus`
//! - 工具定义: `Skill`
//!
//! ## 依赖
//! - Internal: 无
//! - External: `serde`, `serde_json`
//!
//! ## 约束
//! - 所有结构体必须实现 `Serialize` 和 `Deserialize`
//! - OpenAI 格式使用 `#[serde(tag = "role")]` 进行多态序列化
//! - 字段命名使用 camelCase 以匹配 JSON 格式

use serde::{Deserialize, Serialize};

#[derive(Serialize, Deserialize, Debug)]
pub struct JarvisResult {
    pub status: String,
    pub content: String,
    pub input_tokens: u64,
    pub output_tokens: u64,
    pub session_input_tokens: u64,
    pub session_output_tokens: u64,
}

#[derive(Serialize, Clone, Debug)]
pub struct ThinkingConfig {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub r#type: Option<String>, // "enabled" / "disabled" (Doubao/DeepSeek)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub budget_tokens: Option<i32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub enable: Option<bool>, // Hunyuan 格式
}

#[derive(Serialize, Clone, Debug)]
pub struct AnthropicRequest {
    pub model: String,
    pub max_tokens: i32,
    pub system: String,
    pub messages: Vec<Message>,
    pub tools: Vec<serde_json::Value>,
    pub stream: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub thinking: Option<ThinkingConfig>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub temperature: Option<f32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub top_p: Option<f32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub top_k: Option<u32>,
}

// --- OpenAI Format Structs ---

#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct StreamOptions {
    pub include_usage: bool,
}

#[derive(Serialize, Clone, Debug)]
pub struct OpenAIRequest {
    pub model: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub max_tokens: Option<i32>,
    pub messages: Vec<OpenAIMessage>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tools: Option<Vec<OpenAITool>>,
    pub stream: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub stream_options: Option<StreamOptions>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reasoning_effort: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub thinking: Option<ThinkingConfig>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub thinking_budget: Option<i32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub enable_thinking: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub extra_body: Option<serde_json::Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub parameters: Option<serde_json::Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub temperature: Option<f32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub top_p: Option<f32>,
}

#[derive(Serialize, Deserialize, Clone, Debug)]
#[serde(untagged)]
pub enum OpenAIUserContent {
    Text(String),
    Parts(Vec<OpenAIContentPart>),
}

#[derive(Serialize, Deserialize, Clone, Debug)]
#[serde(tag = "role")]
pub enum OpenAIMessage {
    #[serde(rename = "system")]
    System { content: String },
    #[serde(rename = "user")]
    User { content: OpenAIUserContent },
    #[serde(rename = "assistant")]
    Assistant {
        #[serde(skip_serializing_if = "Option::is_none")]
        content: Option<String>,
        #[serde(skip_serializing_if = "Option::is_none")]
        tool_calls: Option<Vec<OpenAIToolCall>>,
        #[serde(skip_serializing_if = "Option::is_none")]
        reasoning_content: Option<serde_json::Value>,
    },
    #[serde(rename = "tool")]
    Tool {
        content: String,
        tool_call_id: String,
    },
}

#[derive(Serialize, Deserialize, Clone, Debug)]
#[serde(tag = "type")]
pub enum OpenAIContentPart {
    #[serde(rename = "text")]
    Text { text: String },
    #[serde(rename = "image_url")]
    ImageUrl { image_url: OpenAIImageUrl },
}

#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct OpenAIImageUrl {
    pub url: String,
}

#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct OpenAIToolCall {
    pub id: String,
    pub r#type: String, // always "function"
    pub function: OpenAIFunctionCall,
}

#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct OpenAIFunctionCall {
    pub name: String,
    pub arguments: String, // stringified JSON
}

#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct OpenAITool {
    pub r#type: String, // always "function"
    pub function: OpenAIFunctionDefinition,
}

#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct OpenAIFunctionDefinition {
    pub name: String,
    pub description: String,
    pub parameters: serde_json::Value,
}

#[derive(Serialize, Deserialize, Clone, Debug)]
#[serde(tag = "role")]
pub enum Message {
    #[serde(rename = "user")]
    User { content: Content },
    #[serde(rename = "assistant")]
    Assistant { content: Content },
}

#[derive(Serialize, Deserialize, Clone, Debug)]
#[serde(untagged)]
pub enum Content {
    Single(String),
    Multiple(Vec<ContentBlock>),
}

#[derive(Serialize, Deserialize, Clone, Debug)]
#[serde(tag = "type")]
pub enum ContentBlock {
    #[serde(rename = "text")]
    Text { text: String },
    #[serde(rename = "thinking")]
    Thinking { thinking: String, signature: String },
    #[serde(rename = "tool_use")]
    ToolUse {
        id: String,
        name: String,
        input: serde_json::Value,
    },
    #[serde(rename = "tool_result")]
    ToolResult {
        tool_use_id: String,
        content: String,
    },
    #[serde(rename = "image")]
    Image { source: ImageSource },
}

#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct ImageSource {
    pub r#type: String,
    pub media_type: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub data: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub file_path: Option<String>,
}

pub struct Skill {
    pub name: String,
    pub description: String,
    pub body: String,
}

#[derive(Serialize, Deserialize, Debug, Clone)]
#[serde(rename_all = "camelCase")]
pub struct ContextSectionSnapshot {
    pub key: String,
    pub label: String,
    pub chars: usize,
    pub estimated_tokens: usize,
    pub token_count_method: String,
    pub item_count: usize,
    pub content: String,
    pub truncated: bool,
}

#[derive(Serialize, Deserialize, Debug, Clone)]
#[serde(rename_all = "camelCase")]
pub struct SessionContextSnapshot {
    pub session_id: String,
    pub run_id: Option<String>,
    pub loop_count: usize,
    pub model: String,
    pub intent: String,
    pub api_format: String,
    pub created_at: u64,
    pub total_chars: usize,
    pub estimated_tokens: usize,
    pub provider_input_tokens: Option<u64>,
    pub provider_output_tokens: Option<u64>,
    pub provider_total_tokens: Option<u64>,
    pub drift_percent: Option<f32>,
    pub max_context_tokens: Option<u32>,
    pub max_output_tokens: i32,
    pub message_count: usize,
    pub tool_schema_count: usize,
    pub tool_call_count: usize,
    pub tool_result_count: usize,
    pub sections: Vec<ContextSectionSnapshot>,
}

#[derive(Serialize, Deserialize, Debug, Clone, Default)]
pub struct SessionMemory {
    /// 运行时消息（从 session_messages 表按 active_message_ids 重建，不序列化存储）
    #[serde(default, skip_serializing)]
    pub messages: Vec<Message>,
    /// LLM 活动视图索引 —— 指向 session_messages 表中 LLM 当前应看到的消息 ID
    #[serde(default)]
    pub message_ids: Vec<String>,
    #[serde(default)]
    pub activated_tools: Vec<String>,
    #[serde(default)]
    pub plan_documents: Vec<PlanDocument>,
}

#[derive(Serialize, Deserialize, Debug, Clone)]
#[serde(rename_all = "camelCase")]
pub struct AgentStep {
    #[serde(rename = "type")]
    pub step_type: String,
    pub tool: Option<String>,
    pub input_summary: Option<String>,
    pub output_summary: Option<String>,
    pub error: Option<String>,
    pub task: Option<String>,
    pub attempt: Option<i32>,
    pub max: Option<i32>,
    pub content: Option<String>,
    pub timestamp: u64,
}

#[derive(Serialize, Deserialize, Debug, Clone)]
#[serde(rename_all = "camelCase")]
pub struct PlanDocument {
    pub id: String,
    #[serde(default)]
    pub session_id: String,
    pub title: String,
    pub content: String,
    pub status: String,
    pub path: Option<String>,
    pub created_at: u64,
    pub updated_at: u64,
    pub decided_at: Option<u64>,
}

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq)]
#[serde(rename_all = "snake_case")]
pub enum TodoStatus {
    Pending,
    InProgress,
    Completed,
}

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct TodoItem {
    pub id: String,
    /// Imperative form describing what needs to be done.
    pub content: String,
    /// Present continuous form shown while the item is in progress.
    pub active_form: String,
    pub status: TodoStatus,
}

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq)]
#[serde(rename_all = "snake_case")]
pub enum TaskStatus {
    Pending,
    InProgress,
    Completed,
}

#[derive(Serialize, Deserialize, Debug, Clone)]
#[serde(rename_all = "camelCase")]
pub struct Task {
    pub id: i32,
    pub subject: String,
    pub description: String,
    pub status: TaskStatus,
    pub blocked_by: Vec<i32>,
    pub blocks: Vec<i32>,
    pub owner: String,
    /// 进行中时显示的动态文本（如 "Fixing authentication bug"）
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub active_form: Option<String>,
    /// 任意附加元数据
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub metadata: Option<serde_json::Value>,
}
