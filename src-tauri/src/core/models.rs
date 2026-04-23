use serde::{Serialize, Deserialize};

#[derive(Serialize, Deserialize, Debug)]
pub struct JarvisResult {
    pub status: String,
    pub content: String,
    pub input_tokens: u64,
    pub output_tokens: u64,
}

#[derive(Serialize, Clone, Debug)]
pub struct AnthropicRequest {
    pub model: String,
    pub max_tokens: i32,
    pub system: String,
    pub messages: Vec<Message>,
    pub tools: Vec<serde_json::Value>,
    pub stream: bool,
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
}

#[derive(Serialize, Deserialize, Clone, Debug)]
#[serde(tag = "role")]
pub enum OpenAIMessage {
    #[serde(rename = "system")]
    System { content: String },
    #[serde(rename = "user")]
    User { content: String },
    #[serde(rename = "assistant")]
    Assistant {
        #[serde(skip_serializing_if = "Option::is_none")]
        content: Option<String>,
        #[serde(skip_serializing_if = "Option::is_none")]
        tool_calls: Option<Vec<OpenAIToolCall>>,
    },
    #[serde(rename = "tool")]
    Tool {
        content: String,
        tool_call_id: String,
    },
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
}

pub struct Skill {
    pub name: String,
    pub description: String,
    pub body: String,
}

#[derive(Serialize, Deserialize, Debug, Clone, Default)]
pub struct SessionMemory {
    pub messages: Vec<Message>,
    pub context: Vec<String>, // 当前任务状态
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
}
