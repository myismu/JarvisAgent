use serde::Serialize;

#[derive(Serialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct AgentTurnSnapshot {
    pub version: u32,
    pub status: String,
    pub text_blocks: Vec<AgentTextBlock>,
    pub thinking_blocks: Vec<AgentThinkingBlock>,
    pub tool_calls: Vec<AgentToolCallView>,
    pub logs: Vec<AgentExecutionLog>,
    pub created_at: u64,
}

#[derive(Serialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct AgentTextBlock {
    pub id: String,
    pub loop_: u32,
    pub kind: String,
    pub content: String,
    pub status: String,
    pub timestamp: u64,
}

#[derive(Serialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct AgentThinkingBlock {
    pub id: String,
    pub loop_: u32,
    pub content: String,
    pub status: String,
    pub timestamp: u64,
}

#[derive(Serialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct AgentToolCallView {
    pub id: String,
    pub loop_: u32,
    pub name: String,
    pub status: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub input_summary: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub output_summary: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
    pub logs: Vec<String>,
    pub timestamp: u64,
    pub updated_at: u64,
}

#[derive(Serialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct AgentExecutionLog {
    pub id: String,
    pub loop_: u32,
    pub content: String,
    pub timestamp: u64,
}
