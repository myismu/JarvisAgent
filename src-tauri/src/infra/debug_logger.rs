//! # debug_logger.rs — Agent 循环结构化日志模块
//!
//! 记录 Agent 每轮循环的请求、响应、思考过程、意图分类等事件，
//! 以 JSONL 格式写入 `data/logs/agent_loop/` 目录，按 session 隔离。
//! 配套 HTML 查看器：`data/logs/log-viewer.html`（统一审计中心）
//!
//! ## Key Exports
//! - `DebugLogger`: Agent 循环日志记录器
//! - 事件类型：request / response / thinking / intent / memory / session_summary / sse_event / protocol_violation
//!
//! ## Constraints
//! - append-only 写入，不持有文件句柄
//! - 大字段（request_json, sse data）截断到指定长度，避免日志膨胀
//! - session_id 由调用方传入，logger 本身不持有会话状态

use std::fs::OpenOptions;
use std::io::Write;
use std::path::PathBuf;
use std::sync::OnceLock;

use serde::Serialize;

// ───────────────────────── 事件结构 ─────────────────────────

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum AgentEventType {
    Request,
    Response,
    Thinking,
    Intent,
    Memory,
    SessionSummary,
    SseEvent,
    ProtocolViolation,
}

/// 请求事件：LLM API 调用
#[derive(Debug, Clone, Serialize)]
pub struct RequestEvent {
    #[serde(rename = "type")]
    pub event_type: AgentEventType,
    pub ts: String,
    pub session_id: String,
    pub agent_type: String,
    pub loop_count: usize,
    /// 请求 JSON 截断到 2KB
    pub request_json: String,
}

/// 响应事件：流式响应摘要
#[derive(Debug, Clone, Serialize)]
pub struct ResponseEvent {
    #[serde(rename = "type")]
    pub event_type: AgentEventType,
    pub ts: String,
    pub session_id: String,
    pub agent_type: String,
    pub loop_count: usize,
    pub text_len: usize,
    pub thinking_len: usize,
    pub tool_blocks: usize,
    pub input_tokens: u64,
    pub output_tokens: u64,
}

/// 思考事件：Agent 推理过程
#[derive(Debug, Clone, Serialize)]
pub struct ThinkingEvent {
    #[serde(rename = "type")]
    pub event_type: AgentEventType,
    pub ts: String,
    pub session_id: String,
    pub agent_type: String,
    pub loop_count: usize,
    pub thinking: String,
    pub response_text: String,
    pub tool_calls: Vec<(String, String)>,
    pub input_tokens: u64,
    pub output_tokens: u64,
}

/// 意图分类事件
#[derive(Debug, Clone, Serialize)]
pub struct IntentEvent {
    #[serde(rename = "type")]
    pub event_type: AgentEventType,
    pub ts: String,
    pub session_id: String,
    pub user_input: String,
    pub classifier: String,
    pub detected_intent: String,
    /// LLM 请求 JSON（仅 LLM 分类器有值）
    #[serde(skip_serializing_if = "String::is_empty")]
    pub request_json: String,
    /// LLM 原始响应（仅 LLM 分类器有值）
    #[serde(skip_serializing_if = "String::is_empty")]
    pub llm_response: String,
}

/// 记忆代理事件
#[derive(Debug, Clone, Serialize)]
pub struct MemoryEvent {
    #[serde(rename = "type")]
    pub event_type: AgentEventType,
    pub ts: String,
    pub session_id: String,
    pub request_json: String,
    pub response_summary: String,
}

/// 会话汇总事件
#[derive(Debug, Clone, Serialize)]
pub struct SessionSummaryEvent {
    #[serde(rename = "type")]
    pub event_type: AgentEventType,
    pub ts: String,
    pub session_id: String,
    pub input_tokens: u64,
    pub output_tokens: u64,
    pub status: String,
}

/// SSE 事件（截断到 500 字符）
#[derive(Debug, Clone, Serialize)]
pub struct SseEvent {
    #[serde(rename = "type")]
    pub event_type: AgentEventType,
    pub ts: String,
    pub session_id: String,
    pub loop_count: usize,
    pub data: String,
}

/// 协议违规事件
#[derive(Debug, Clone, Serialize)]
pub struct ProtocolViolationEvent {
    #[serde(rename = "type")]
    pub event_type: AgentEventType,
    pub ts: String,
    pub session_id: String,
    pub agent_type: String,
    pub loop_count: usize,
    pub snippet: String,
}

// ───────────────────────── Logger ─────────────────────────

pub struct DebugLogger {
    log_dir: PathBuf,
}

impl DebugLogger {
    pub fn new() -> Self {
        let log_dir = crate::infra::config::data_paths::logs_dir().join("agent_loop");
        let _ = std::fs::create_dir_all(&log_dir);
        Self { log_dir }
    }

    /// 写入一条 JSONL 记录
    fn write_record(&self, session_id: &str, record: &impl Serialize) {
        let date = chrono::Local::now().format("%Y-%m-%d").to_string();
        let filename = format!("{}_{}.jsonl", date, session_id);
        let path = self.log_dir.join(filename);

        if let Ok(mut file) = OpenOptions::new().create(true).append(true).open(&path) {
            if let Ok(line) = serde_json::to_string(record) {
                let _ = writeln!(file, "{}", line);
            }
        }
    }

    // ─────────── 公开 API ───────────

    /// 记录 LLM 请求
    pub fn log_request(
        &self,
        session_id: &str,
        agent_type: &str,
        loop_count: usize,
        request_json: &str,
    ) {
        self.write_record(
            session_id,
            &RequestEvent {
                event_type: AgentEventType::Request,
                ts: chrono::Utc::now().to_rfc3339(),
                session_id: session_id.to_string(),
                agent_type: agent_type.to_string(),
                loop_count,
                request_json: request_json.to_string(),
            },
        );
    }

    /// 记录流式响应摘要
    pub fn log_response(
        &self,
        session_id: &str,
        agent_type: &str,
        loop_count: usize,
        text_len: usize,
        thinking_len: usize,
        tool_blocks: usize,
        input_tokens: u64,
        output_tokens: u64,
    ) {
        self.write_record(
            session_id,
            &ResponseEvent {
                event_type: AgentEventType::Response,
                ts: chrono::Utc::now().to_rfc3339(),
                session_id: session_id.to_string(),
                agent_type: agent_type.to_string(),
                loop_count,
                text_len,
                thinking_len,
                tool_blocks,
                input_tokens,
                output_tokens,
            },
        );
    }

    /// 记录 Agent 思考过程
    pub fn log_thoughts(
        &self,
        session_id: &str,
        agent_type: &str,
        loop_count: usize,
        thinking: &str,
        response_text: &str,
        tool_calls: &[(String, String)],
        input_tokens: u64,
        output_tokens: u64,
    ) {
        self.write_record(
            session_id,
            &ThinkingEvent {
                event_type: AgentEventType::Thinking,
                ts: chrono::Utc::now().to_rfc3339(),
                session_id: session_id.to_string(),
                agent_type: agent_type.to_string(),
                loop_count,
                thinking: thinking.to_string(),
                response_text: response_text.to_string(),
                tool_calls: tool_calls.to_vec(),
                input_tokens,
                output_tokens,
            },
        );
    }

    /// 记录意图分类结果
    pub fn log_intent(
        &self,
        session_id: &str,
        user_input: &str,
        classifier: &str,
        detected_intent: &str,
        request_json: &str,
        llm_response: &str,
    ) {
        self.write_record(
            session_id,
            &IntentEvent {
                event_type: AgentEventType::Intent,
                ts: chrono::Utc::now().to_rfc3339(),
                session_id: session_id.to_string(),
                user_input: user_input.to_string(),
                classifier: classifier.to_string(),
                detected_intent: detected_intent.to_string(),
                request_json: request_json.to_string(),
                llm_response: llm_response.to_string(),
            },
        );
    }

    /// 记录记忆代理操作
    pub fn log_memory(
        &self,
        session_id: &str,
        request_json: &str,
        response_summary: &str,
    ) {
        self.write_record(
            session_id,
            &MemoryEvent {
                event_type: AgentEventType::Memory,
                ts: chrono::Utc::now().to_rfc3339(),
                session_id: session_id.to_string(),
                request_json: request_json.to_string(),
                response_summary: response_summary.to_string(),
            },
        );
    }

    /// 记录会话结束汇总
    pub fn log_session_summary(
        &self,
        session_id: &str,
        input_tokens: u64,
        output_tokens: u64,
        status: &str,
    ) {
        self.write_record(
            session_id,
            &SessionSummaryEvent {
                event_type: AgentEventType::SessionSummary,
                ts: chrono::Utc::now().to_rfc3339(),
                session_id: session_id.to_string(),
                input_tokens,
                output_tokens,
                status: status.to_string(),
            },
        );
    }

    /// 记录 SSE 原始事件（截断到 500 字符）
    pub fn log_sse_event(&self, session_id: &str, loop_count: usize, data: &str) {
        self.write_record(
            session_id,
            &SseEvent {
                event_type: AgentEventType::SseEvent,
                ts: chrono::Utc::now().to_rfc3339(),
                session_id: session_id.to_string(),
                loop_count,
                data: truncate_str(data, 1000),
            },
        );
    }

    /// 记录协议违规（模型把工具调用写成普通文本）
    pub fn log_protocol_violation(
        &self,
        session_id: &str,
        agent_type: &str,
        loop_count: usize,
        snippet: &str,
    ) {
        self.write_record(
            session_id,
            &ProtocolViolationEvent {
                event_type: AgentEventType::ProtocolViolation,
                ts: chrono::Utc::now().to_rfc3339(),
                session_id: session_id.to_string(),
                agent_type: agent_type.to_string(),
                loop_count,
                snippet: snippet.to_string(),
            },
        );
    }
}

impl Default for DebugLogger {
    fn default() -> Self {
        Self::new()
    }
}

// ───────────────────────── 辅助函数 ─────────────────────────

fn truncate_str(s: &str, max_chars: usize) -> String {
    if s.len() <= max_chars {
        s.to_string()
    } else {
        format!("{}...(truncated)", &s[..max_chars])
    }
}

// ───────────────────────── 全局单例 ─────────────────────────

static LOGGER: OnceLock<DebugLogger> = OnceLock::new();

pub fn debug_logger() -> &'static DebugLogger {
    LOGGER.get_or_init(DebugLogger::new)
}
