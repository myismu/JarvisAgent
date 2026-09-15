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
    /// 后端为用户消息分配的 UUID，前端用于关联撤回按钮
    #[serde(skip_serializing_if = "Option::is_none")]
    pub user_message_id: Option<String>,
    /// break_loop 时的工具执行结果摘要（前端用于 toolBuffer，避免丢失工具执行日志）
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tool_execution_summary: Option<String>,
    /// 本轮的状态标注（如"上游服务已停止响应""用户已取消执行"）。
    ///
    /// 与 `content` 的分工：content 是模型正文（渲染在回复气泡内），
    /// notice 是运行状态说明（渲染在气泡**下方**的小字里）。
    /// 中断类信息走 notice，避免混进正文看起来像模型自己说的话。
    #[serde(skip_serializing_if = "Option::is_none")]
    pub notice: Option<String>,
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
    #[serde(skip_serializing_if = "Vec::is_empty")]
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
    /// 运行时上下文（意图标签 / 工作目录 / 项目结构 / 全局记忆）。
    /// 只在存储和内部表示中存在，出网前由 provider 翻译成普通 text 块。
    #[serde(rename = "context")]
    Context { text: String },
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

#[derive(Serialize, Clone, Debug)]
#[serde(rename_all = "camelCase")]
pub struct Skill {
    pub name: String,
    pub description: String,
    pub body: String,
    pub path: String,
}

/// Skill 列表元数据（不含 body，用于列表展示）
#[derive(Serialize, Clone, Debug)]
#[serde(rename_all = "camelCase")]
pub struct SkillMeta {
    pub name: String,
    pub description: String,
    pub path: String,
    pub body_tokens: usize,
    pub active: bool,
}

/// Skill 完整详情（含 body，用于详情展示）
#[derive(Serialize, Clone, Debug)]
#[serde(rename_all = "camelCase")]
pub struct SkillDetail {
    pub name: String,
    pub description: String,
    pub path: String,
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
    /// 原始 JSON 数据（仅 messages 和 tools section 有值）
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub raw_content: Option<String>,
}

/// 单个 loop 的缓存命中记录（用于渲染回合内趋势）
#[derive(Serialize, Deserialize, Debug, Clone, Default)]
#[serde(rename_all = "camelCase")]
pub struct CacheHitPoint {
    pub loop_count: usize,
    pub hit_tokens: u64,
    pub miss_tokens: u64,
    /// 命中的字段名（服务商未报告时为 None）
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source: Option<String>,
}

impl CacheHitPoint {
    pub fn total(&self) -> u64 {
        self.hit_tokens.saturating_add(self.miss_tokens)
    }

    pub fn hit_rate(&self) -> Option<f32> {
        let total = self.total();
        if total == 0 {
            None
        } else {
            Some(self.hit_tokens as f32 / total as f32)
        }
    }
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
    /// 缓存命中 / 未命中的输入 token。
    /// `None` = 该 provider（或中转链路）未报告该字段，**与 0 命中是两回事**。
    #[serde(default)]
    pub cache_hit_tokens: Option<u64>,
    #[serde(default)]
    pub cache_miss_tokens: Option<u64>,
    /// 命中的字段名（如 `prompt_cache_hit_tokens` / `cached_tokens` / `cache_read_input_tokens`）
    #[serde(default)]
    pub cache_source: Option<String>,
    /// 本会话逐 loop 的缓存命中记录（保留最近 N 条，用于渲染趋势）
    #[serde(default)]
    pub cache_history: Vec<CacheHitPoint>,
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
    /// 每条消息的来源分类（与 messages 平行），从 session_messages 表重建，不序列化存储
    #[serde(default, skip_serializing)]
    pub sources: Vec<String>,
    /// 上下文快照单调序号：每次注入快照前自增，压缩/重启后仍保持单调递增
    #[serde(default)]
    pub snapshot_seq: u64,
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
    /// 用户拒绝方案时填写的修改意见（与 content 分离，不覆盖原始方案）
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub rejection_feedback: Option<String>,
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
