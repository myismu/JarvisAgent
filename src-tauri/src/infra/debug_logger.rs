//! # debug_logger.rs — Agent 循环结构化日志模块
//!
//! 记录 Agent 每轮循环的请求、响应、思考过程、意图分类等事件，
//! 以 JSONL 格式写入 `data/logs/agent_loop/` 目录，按 session 隔离。
//! 配套 HTML 查看器：`data/logs/log-viewer.html`（统一审计中心）
//!
//! ## Key Exports
//! - `DebugLogger`: Agent 循环日志记录器
//! - 事件类型：request_base / request_delta / request_full / response / thinking /
//!   memory / session_summary / sse_summary / sse_event / protocol_violation / api_error
//!
//! ## 请求日志为什么按 base + 增量落盘
//!
//! 请求体里 `system`（约 6KB）+ `tools`（约 12KB）在整轮会话里逐 loop 一字不差重复，
//! 只有 `messages` 在追加；早期实现每 loop 都写一份完整请求，30 loop 的一轮就能写 1MB。
//! 现在拆成三件事：
//! - `request_base`：除 `messages` 之外的全部请求字段，内容寻址（`base_id`），
//!   只在首次/固定块变化时写一次；
//! - `request_delta`：本 loop `messages` 相对上一 loop 的新增尾巴 + 前缀校验信息；
//! - `request_full`：增量无法表达时（历史被压缩/图片折叠/配对修复/换模型）整份落盘，
//!   并作为新基准 —— 保证任何一段日志都能独立重建。
//!
//! 查看器按 `request_base` + `request_delta` 重建完整请求；旧的 `request` 事件仍可读。
//!
//! ## Constraints
//! - append-only 写入，不持有文件句柄（增量状态只在内存里，进程重启后自动重新锚定全量）
//! - 大字段（sse data、api_error 快照）截断到指定长度，避免日志膨胀
//! - session_id 由调用方传入；logger 只按 (session_id, agent_type) 记住上一轮请求用于增量判定
//! - 环境变量 `JARVIS_LOG_SSE_RAW=1` 恢复逐片记录 SSE；`JARVIS_LOG_REQUEST=FULL` 每轮都写全量请求

use std::collections::HashMap;
use std::fs::OpenOptions;
use std::io::Write;
use std::path::PathBuf;
use std::sync::{Mutex, OnceLock};
use std::time::{Duration, Instant};

use serde::Serialize;

// ───────────────────────── 事件结构 ─────────────────────────

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum AgentEventType {
    /// 除 messages 外的请求字段（内容寻址，只写一次）
    RequestBase,
    /// 本 loop messages 的新增尾巴
    RequestDelta,
    /// 整份请求（无法增量表达时）
    RequestFull,
    Response,
    Thinking,
    Memory,
    SessionSummary,
    /// 一个 loop 内 SSE 事件的聚合计数（默认）
    SseSummary,
    /// 原始 SSE 分片（仅 `JARVIS_LOG_SSE_RAW=1`）
    SseEvent,
    ProtocolViolation,
    ApiError,
}

/// 请求固定块：`messages` 之外的所有字段（model / max_tokens / system / tools / thinking …）
#[derive(Debug, Clone, Serialize)]
pub struct RequestBaseEvent {
    #[serde(rename = "type")]
    pub event_type: AgentEventType,
    pub ts: String,
    pub session_id: String,
    pub agent_type: String,
    /// 固定块内容哈希，`request_delta` 靠它找到对应的 base
    pub base_id: String,
    pub base: serde_json::Value,
}

/// 请求增量：本 loop 相对上一 loop 新增的 messages
#[derive(Debug, Clone, Serialize)]
pub struct RequestDeltaEvent {
    #[serde(rename = "type")]
    pub event_type: AgentEventType,
    pub ts: String,
    pub session_id: String,
    pub agent_type: String,
    pub loop_count: usize,
    pub base_id: String,
    /// 本 loop messages 的前 `from_index` 条与上一 loop 完全一致
    pub from_index: usize,
    /// 前缀校验：对 `messages[..from_index]` 取哈希，重建时可据此确认拼接正确
    pub prefix_hash: String,
    /// 新增的消息（追加在 prefix 之后）
    pub added: Vec<serde_json::Value>,
}

/// 整份请求：增量无法表达（历史被重写）或进程首次锚定时使用
#[derive(Debug, Clone, Serialize)]
pub struct RequestFullEvent {
    #[serde(rename = "type")]
    pub event_type: AgentEventType,
    pub ts: String,
    pub session_id: String,
    pub agent_type: String,
    pub loop_count: usize,
    pub base_id: String,
    /// first / base_changed / history_rewritten
    pub reason: String,
    pub messages: Vec<serde_json::Value>,
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
    /// 缓存命中 / 未命中的输入 token（None = 该 provider 未报告，**不是 0**）
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cache_hit_tokens: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cache_miss_tokens: Option<u64>,
    /// 命中的字段名（如 prompt_cache_hit_tokens / cached_tokens / cache_read_input_tokens）
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cache_source: Option<String>,
    /// true = 这次的 0 命中**不是** provider 明确上报的，而是依据"端点记忆"推断出来的（预热期）。
    /// 留着这个标记是为了让日志说实话：推断值不能和厂商亲口说的 0 混为一谈。
    #[serde(default, skip_serializing_if = "is_false")]
    pub cache_inferred: bool,
    /// 原始 usage 原文（截断）：遇到未知写法时可直接从日志自诊断
    #[serde(skip_serializing_if = "Option::is_none")]
    pub usage_raw: Option<String>,
}

fn is_false(value: &bool) -> bool {
    !*value
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

/// SSE 事件（截断到 1000 字符，仅 `JARVIS_LOG_SSE_RAW=1` 时逐片记录）
#[derive(Debug, Clone, Serialize)]
pub struct SseEvent {
    #[serde(rename = "type")]
    pub event_type: AgentEventType,
    pub ts: String,
    pub session_id: String,
    pub loop_count: usize,
    pub data: String,
}

/// 一个 loop 内 SSE 事件的聚合：默认只把"有多少片、多少字节、首尾样本"落盘
#[derive(Debug, Clone, Serialize)]
pub struct SseSummaryEvent {
    #[serde(rename = "type")]
    pub event_type: AgentEventType,
    pub ts: String,
    pub session_id: String,
    pub agent_type: String,
    pub loop_count: usize,
    pub count: usize,
    pub bytes: usize,
    pub duration_ms: u64,
    /// 首片样本（截断）
    pub first: String,
    /// 末片样本（截断）
    pub last: String,
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

/// API 错误事件：记录导致失败的错误详情和消息序列快照
#[derive(Debug, Clone, Serialize)]
pub struct ApiErrorEvent {
    #[serde(rename = "type")]
    pub event_type: AgentEventType,
    pub ts: String,
    pub session_id: String,
    pub agent_type: String,
    pub loop_count: usize,
    pub error_message: String,
    /// 最后一轮请求的 messages 数组摘要（截断到 8KB）
    pub messages_snapshot: String,
}

// ───────────────────────── Logger ─────────────────────────

/// 上一次请求的内存快照：只用于判定"这轮能否写增量"，不落盘。
struct LastRequest {
    base_id: String,
    /// 这个 base 写进了哪个日志文件（文件名，含分片序号）。
    /// 跨天换文件、或文件超过分片阈值换片后必须重新锚定 base，
    /// 否则新文件里出现引用不到 base 的 delta（查看器会标"缺少 base"）。
    base_written_file: String,
    messages_len: usize,
    /// 对上一轮完整 messages 数组取的哈希，等价于下一轮的"前缀哈希"
    messages_hash: String,
    updated_at: Instant,
}

/// 一个 loop 内 SSE 事件的聚合状态
struct SseAgg {
    count: usize,
    bytes: usize,
    first: String,
    last: String,
    started: Instant,
}

#[derive(Default)]
struct LoggerState {
    /// key = "{session_id}\u{1}{agent_type}"
    last_request: HashMap<String, LastRequest>,
    /// key = (session_id, agent_type, loop_count)
    sse: HashMap<(String, String, usize), SseAgg>,
}

/// 增量状态保留时长：超过这个时间没再来的会话快照会被清掉（避免长期占内存）
const REQUEST_STATE_TTL: Duration = Duration::from_secs(6 * 3600);
/// SSE 聚合残留清理阈值（loop 异常结束、没等到 response 时兜底）
const SSE_AGG_TTL: Duration = Duration::from_secs(600);
/// SSE 样本截断长度
const SSE_SAMPLE_CHARS: usize = 160;

/// 这一轮请求该怎么写（纯判定，便于单测）
#[derive(Debug, PartialEq, Eq)]
enum Plan {
    /// 与前一轮前缀一致：只写新增尾巴
    Delta { from: usize },
    /// 固定块变了：补写 base + 全量
    BaseChanged,
    /// 历史被重写（压缩 / 图片折叠 / 配对修复）：只补全量
    Rewritten,
    /// 本进程首次、或跨天换了日志文件：base + 全量做锚点
    Anchor,
}

/// 判定本轮该写增量还是全量。
///
/// 只有"固定块没变 + 本文件内已写过 base + 上一轮 messages 是本轮的前缀"才敢写增量；
/// 其余情况（换模型、跨天/换分片文件、历史被压缩/折叠/修复、显式 FULL 模式）一律全量，
/// 保证日志任何一段都能独立重建。
fn plan_request(
    prev: Option<&LastRequest>,
    base_id: &str,
    current_file: &str,
    messages: &[serde_json::Value],
    force_full: bool,
) -> Plan {
    match prev {
        None => Plan::Anchor,
        Some(_) if force_full => Plan::Rewritten,
        Some(prev) => {
            if prev.base_id != base_id {
                Plan::BaseChanged
            } else if prev.base_written_file != current_file {
                Plan::Anchor
            } else if prev.messages_len <= messages.len()
                && hash_values(&messages[..prev.messages_len]) == prev.messages_hash
            {
                Plan::Delta {
                    from: prev.messages_len,
                }
            } else {
                Plan::Rewritten
            }
        }
    }
}

pub struct DebugLogger {
    log_dir: PathBuf,
    policy: crate::infra::log_maintenance::LogPolicy,
    state: Mutex<LoggerState>,
}

impl DebugLogger {
    pub fn new() -> Self {
        let log_dir = crate::infra::config::data_paths::logs_dir().join("agent_loop");
        let _ = std::fs::create_dir_all(&log_dir);
        Self {
            log_dir,
            policy: crate::infra::log_maintenance::LogPolicy::from_env(),
            state: Mutex::new(LoggerState::default()),
        }
    }

    /// 测试用：指定日志目录，避免依赖全局数据目录（AGENT_HOME 在单测里可能未初始化）
    #[cfg(test)]
    fn with_dir(log_dir: PathBuf) -> Self {
        let _ = std::fs::create_dir_all(&log_dir);
        Self {
            log_dir,
            policy: crate::infra::log_maintenance::LogPolicy::default(),
            state: Mutex::new(LoggerState::default()),
        }
    }

    /// 本次写入应落在哪个分片文件（文件名，含分片序号）
    fn current_part_file(&self, session_id: &str) -> std::ffi::OsString {
        let date = chrono::Local::now().format("%Y-%m-%d").to_string();
        crate::infra::log_maintenance::resolve_part_file(
            &self.log_dir,
            &date,
            session_id,
            &self.policy,
        )
    }

    /// 写入一条 JSONL 记录（自动选择当前分片）
    fn write_record(&self, session_id: &str, record: &impl Serialize) {
        let filename = self.current_part_file(session_id);
        self.write_record_to(&filename, record);
    }

    /// 写入指定分片文件（同一轮请求的多个事件必须落在同一个分片里）
    fn write_record_to(&self, filename: &std::ffi::OsStr, record: &impl Serialize) {
        let path = self.log_dir.join(filename);

        if let Ok(mut file) = OpenOptions::new().create(true).append(true).open(&path) {
            if let Ok(line) = serde_json::to_string(record) {
                let _ = writeln!(file, "{}", line);
            }
        }
    }

    // ─────────── 公开 API ───────────

    /// 记录一次 LLM 请求。
    ///
    /// 按 `(session_id, agent_type)` 记住上一轮请求：能增量就只写 `messages` 的新增尾巴，
    /// 否则退化成整份落盘（并补齐 base），保证任意一段日志都能独立重建完整请求。
    pub fn log_request(
        &self,
        session_id: &str,
        agent_type: &str,
        loop_count: usize,
        request: &serde_json::Value,
    ) {
        let ts = chrono::Utc::now().to_rfc3339();
        // 本轮请求的所有事件都必须落在同一个分片文件里（base 与它的 delta 同文件才能重建）；
        // 换片/跨天时下面的 plan_request 会判定为 Anchor，重新写 base + 全量
        let part_file = self.current_part_file(session_id);
        let part_file_name = part_file.to_string_lossy().to_string();

        // 固定块 = 除 messages 外的全部字段；messages 单独走增量
        let base = {
            let mut b = request.clone();
            if let Some(obj) = b.as_object_mut() {
                obj.remove("messages");
            }
            b
        };
        let base_id = hash_json(&base);
        let messages: Vec<serde_json::Value> = request
            .get("messages")
            .and_then(|m| m.as_array())
            .cloned()
            .unwrap_or_default();
        let messages_hash = hash_values(&messages);
        let key = format!("{session_id}\u{1}{agent_type}");

        let force_full = env_flag("JARVIS_LOG_REQUEST", "FULL");
        let plan = {
            let st = self.state.lock().unwrap_or_else(|e| e.into_inner());
            plan_request(
                st.last_request.get(&key),
                &base_id,
                &part_file_name,
                &messages,
                force_full,
            )
        };

        let write_base = || {
            self.write_record_to(
                &part_file,
                &RequestBaseEvent {
                    event_type: AgentEventType::RequestBase,
                    ts: ts.clone(),
                    session_id: session_id.to_string(),
                    agent_type: agent_type.to_string(),
                    base_id: base_id.clone(),
                    base: base.clone(),
                },
            );
        };
        let write_full = |reason: &str| {
            self.write_record_to(
                &part_file,
                &RequestFullEvent {
                    event_type: AgentEventType::RequestFull,
                    ts: ts.clone(),
                    session_id: session_id.to_string(),
                    agent_type: agent_type.to_string(),
                    loop_count,
                    base_id: base_id.clone(),
                    reason: reason.to_string(),
                    messages: messages.clone(),
                },
            );
        };

        match plan {
            Plan::Delta { from } => {
                let prefix_hash = hash_values(&messages[..from]);
                self.write_record_to(
                    &part_file,
                    &RequestDeltaEvent {
                        event_type: AgentEventType::RequestDelta,
                        ts: ts.clone(),
                        session_id: session_id.to_string(),
                        agent_type: agent_type.to_string(),
                        loop_count,
                        base_id: base_id.clone(),
                        from_index: from,
                        prefix_hash,
                        added: messages[from..].to_vec(),
                    },
                );
                println!(
                    "[JARVIS] 日志: loop {} 请求增量 from={} +{} 条 messages（base {}）",
                    loop_count,
                    from,
                    messages.len() - from,
                    short_id(&base_id)
                );
            }
            Plan::BaseChanged => {
                write_base();
                write_full("base_changed");
                println!("[JARVIS] 日志: loop {} 固定块变化，重写 base + 全量请求", loop_count);
            }
            Plan::Anchor => {
                write_base();
                write_full("first");
                println!("[JARVIS] 日志: loop {} 首次锚定，写 base + 全量请求", loop_count);
            }
            Plan::Rewritten => {
                let reason = if force_full { "forced" } else { "history_rewritten" };
                write_full(reason);
                println!("[JARVIS] 日志: loop {} 历史非追加（{}），写全量请求", loop_count, reason);
            }
        }

        // 更新增量状态（写盘之后再更新，失败也不会产生"孤儿 delta"）
        {
            let mut st = self.state.lock().unwrap_or_else(|e| e.into_inner());
            st.last_request.insert(
                key,
                LastRequest {
                    base_id,
                    base_written_file: part_file_name,
                    messages_len: messages.len(),
                    messages_hash,
                    updated_at: Instant::now(),
                },
            );
            st.last_request
                .retain(|_, v| v.updated_at.elapsed() < REQUEST_STATE_TTL);
        }
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
        cache_hit_tokens: Option<u64>,
        cache_miss_tokens: Option<u64>,
        cache_source: Option<&str>,
        cache_inferred: bool,
        usage_raw: Option<&str>,
    ) {
        // 流式收尾：把本 loop 的 SSE 聚合成一条 sse_summary（默认不逐片落盘）
        self.flush_sse_summary(session_id, agent_type, loop_count);
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
                cache_hit_tokens,
                cache_miss_tokens,
                cache_source: cache_source.map(|s| s.to_string()),
                cache_inferred,
                usage_raw: usage_raw.map(|s| s.to_string()),
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
        // 取消/异常结束的 loop 可能没走到 response，这里兜底冲刷 SSE 聚合
        self.flush_sse_for_session(session_id);
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

    /// 记录 API 错误（含消息序列快照，用于诊断 400 等错误）
    pub fn log_api_error(
        &self,
        session_id: &str,
        agent_type: &str,
        loop_count: usize,
        error_message: &str,
        messages_snapshot: &str,
    ) {
        // 截断消息快照到 8KB，避免日志膨胀
        let truncated = if messages_snapshot.len() > 8192 {
            let safe: String = messages_snapshot.chars().take(8192).collect();
            format!("{}...[截断，原始长度 {} 字符]", safe, messages_snapshot.len())
        } else {
            messages_snapshot.to_string()
        };
        self.write_record(
            session_id,
            &ApiErrorEvent {
                event_type: AgentEventType::ApiError,
                ts: chrono::Utc::now().to_rfc3339(),
                session_id: session_id.to_string(),
                agent_type: agent_type.to_string(),
                loop_count,
                error_message: error_message.to_string(),
                messages_snapshot: truncated,
            },
        );
    }

    /// 记录原始 SSE 事件。
    ///
    /// 默认不逐片落盘（一次 110 字回复就有 30+ 片，占整个日志 1/3），
    /// 只在内存里累加计数/字节/首尾样本，等 `log_response` 时落一条 `sse_summary`。
    /// 需要逐片排查协议/流式解析时设 `JARVIS_LOG_SSE_RAW=1` 恢复旧行为。
    pub fn log_sse_event(&self, session_id: &str, agent_type: &str, loop_count: usize, data: &str) {
        if env_flag("JARVIS_LOG_SSE_RAW", "1") {
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
            return;
        }
        let sample = truncate_str(data, SSE_SAMPLE_CHARS);
        let mut st = self.state.lock().unwrap_or_else(|e| e.into_inner());
        let agg = st
            .sse
            .entry((session_id.to_string(), agent_type.to_string(), loop_count))
            .or_insert_with(|| SseAgg {
                count: 0,
                bytes: 0,
                first: sample.clone(),
                last: String::new(),
                started: Instant::now(),
            });
        agg.count += 1;
        agg.bytes += data.len();
        agg.last = sample;
    }

    /// 把某个 loop 的 SSE 聚合落成一条 `sse_summary`（幂等：已冲刷过就什么也不做）
    pub fn flush_sse_summary(&self, session_id: &str, agent_type: &str, loop_count: usize) {
        let agg = {
            let mut st = self.state.lock().unwrap_or_else(|e| e.into_inner());
            let agg = st
                .sse
                .remove(&(session_id.to_string(), agent_type.to_string(), loop_count));
            // 兜底清理：loop 异常中断（没走到 response）时残留的聚合
            st.sse.retain(|_, v| v.started.elapsed() < SSE_AGG_TTL);
            agg
        };
        if let Some(agg) = agg {
            if agg.count == 0 {
                return;
            }
            self.write_record(
                session_id,
                &SseSummaryEvent {
                    event_type: AgentEventType::SseSummary,
                    ts: chrono::Utc::now().to_rfc3339(),
                    session_id: session_id.to_string(),
                    agent_type: agent_type.to_string(),
                    loop_count,
                    count: agg.count,
                    bytes: agg.bytes,
                    duration_ms: agg.started.elapsed().as_millis() as u64,
                    first: agg.first,
                    last: agg.last,
                },
            );
        }
    }

    /// 会话收尾：把该 session 还没落盘的 SSE 聚合全部冲刷掉（取消/异常结束也不丢计数）
    fn flush_sse_for_session(&self, session_id: &str) {
        let pending: Vec<(String, usize)> = {
            let st = self.state.lock().unwrap_or_else(|e| e.into_inner());
            st.sse
                .keys()
                .filter(|(sid, _, _)| sid == session_id)
                .map(|(_, agent, lp)| (agent.clone(), *lp))
                .collect()
        };
        for (agent_type, loop_count) in pending {
            self.flush_sse_summary(session_id, &agent_type, loop_count);
        }
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

/// 按"字符"上限安全截断（不会切在多字节字符中间导致 panic）
fn truncate_str(s: &str, max_chars: usize) -> String {
    if s.chars().count() <= max_chars {
        return s.to_string();
    }
    let head: String = s.chars().take(max_chars).collect();
    format!("{}...(truncated)", head)
}

/// FNV-1a 64：日志去重只需要稳定的短标识，不需要密码学强度
fn fnv1a64(bytes: &[u8]) -> u64 {
    let mut hash: u64 = 0xcbf2_9ce4_8422_2325;
    for byte in bytes {
        hash ^= *byte as u64;
        hash = hash.wrapping_mul(0x0000_0100_0000_01b3);
    }
    hash
}

fn hash_bytes(bytes: &[u8]) -> String {
    format!("fnv1a:{:016x}", fnv1a64(bytes))
}

/// 固定块哈希（键序由 serde_json 的映射类型稳定保证）
fn hash_json(value: &serde_json::Value) -> String {
    hash_bytes(serde_json::to_string(value).unwrap_or_default().as_bytes())
}

/// 消息数组 / 前缀哈希
fn hash_values(values: &[serde_json::Value]) -> String {
    hash_bytes(serde_json::to_string(values).unwrap_or_default().as_bytes())
}

/// 日志里打印用的短 id
fn short_id(id: &str) -> &str {
    let tail = id.rsplit(':').next().unwrap_or(id);
    &tail[..tail.len().min(8)]
}

fn env_flag(name: &str, expected: &str) -> bool {
    std::env::var(name)
        .map(|v| v.eq_ignore_ascii_case(expected))
        .unwrap_or(false)
}

// ───────────────────────── 全局单例 ─────────────────────────

static LOGGER: OnceLock<DebugLogger> = OnceLock::new();

pub fn debug_logger() -> &'static DebugLogger {
    LOGGER.get_or_init(DebugLogger::new)
}

// ───────────────────────── 测试 ─────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    /// 构造"上一轮请求"的内存快照；`part_file` 是它写进的分片文件名
    fn last_request(base_id: &str, part_file: &str, messages: &[serde_json::Value]) -> LastRequest {
        LastRequest {
            base_id: base_id.to_string(),
            base_written_file: part_file.to_string(),
            messages_len: messages.len(),
            messages_hash: hash_values(messages),
            updated_at: Instant::now(),
        }
    }

    /// 回归：同一会话换了分片文件（按大小滚动 / 跨天）后必须重新锚定 base，
    /// 否则新文件里会出现引用不到 base 的 delta，查看器只能标"缺少 base"。
    #[test]
    fn new_part_file_reanchors_base() {
        let prev_msgs = vec![json!({"role": "user", "content": "hi"})];
        let prev = last_request("b1", "2026-09-15_s1.jsonl", &prev_msgs);
        let mut now = prev_msgs.clone();
        now.push(json!({"role": "assistant", "content": "hello"}));
        // 同一分片内、messages 是前缀 → 增量
        assert_eq!(
            plan_request(Some(&prev), "b1", "2026-09-15_s1.jsonl", &now, false),
            Plan::Delta { from: 1 }
        );
        // 滚到下一个分片 → 重新锚定（哪怕 base 与 messages 都没变）
        assert_eq!(
            plan_request(Some(&prev), "b1", "2026-09-15_s1.2.jsonl", &now, false),
            Plan::Anchor
        );
    }

    #[test]
    fn first_request_anchors_with_base_and_full() {
        let msgs = vec![json!({"role": "user", "content": "hi"})];
        assert_eq!(
            plan_request(None, "b1", "F1", &msgs, false),
            Plan::Anchor
        );
    }

    #[test]
    fn appended_messages_produce_delta() {
        let prev_msgs = vec![json!({"role": "user", "content": "hi"})];
        let prev = last_request("b1", "F1", &prev_msgs);
        let mut now = prev_msgs.clone();
        now.push(json!({"role": "assistant", "content": "hello"}));
        assert_eq!(
            plan_request(Some(&prev), "b1", "F1", &now, false),
            Plan::Delta { from: 1 }
        );
    }

    #[test]
    fn identical_messages_produce_empty_delta() {
        let prev_msgs = vec![json!({"role": "user", "content": "hi"})];
        let prev = last_request("b1", "F1", &prev_msgs);
        assert_eq!(
            plan_request(Some(&prev), "b1", "F1", &prev_msgs, false),
            Plan::Delta { from: 1 }
        );
    }

    #[test]
    fn changed_base_writes_base_then_full() {
        let prev_msgs = vec![json!({"role": "user"})];
        let prev = last_request("b1", "F1", &prev_msgs);
        assert_eq!(
            plan_request(Some(&prev), "b2", "F1", &prev_msgs, false),
            Plan::BaseChanged
        );
    }

    #[test]
    fn rewritten_history_falls_back_to_full() {
        // 压缩把往轮消息换成摘要：不再是前缀
        let prev_msgs = vec![json!({"role": "user", "content": "很长的原始历史"})];
        let prev = last_request("b1", "F1", &prev_msgs);
        let now = vec![
            json!({"role": "user", "content": "[上下文压缩摘要]"}),
            json!({"role": "assistant", "content": "ok"}),
        ];
        assert_eq!(
            plan_request(Some(&prev), "b1", "F1", &now, false),
            Plan::Rewritten
        );
    }

    #[test]
    fn shrunk_history_falls_back_to_full() {
        let prev_msgs = vec![json!({"role": "user"}), json!({"role": "assistant"})];
        let prev = last_request("b1", "F1", &prev_msgs);
        let now = vec![json!({"role": "user"})];
        assert_eq!(
            plan_request(Some(&prev), "b1", "F1", &now, false),
            Plan::Rewritten
        );
    }

    #[test]
    fn new_day_reanchors_so_base_exists_in_the_new_file() {
        let prev_msgs = vec![json!({"role": "user"})];
        let prev = last_request("b1", "F0", &prev_msgs);
        assert_eq!(
            plan_request(Some(&prev), "b1", "F1", &prev_msgs, false),
            Plan::Anchor
        );
    }

    #[test]
    fn forced_full_mode_always_writes_full() {
        let prev_msgs = vec![json!({"role": "user"})];
        let prev = last_request("b1", "F1", &prev_msgs);
        assert_eq!(
            plan_request(Some(&prev), "b1", "F1", &prev_msgs, true),
            Plan::Rewritten
        );
    }

    #[test]
    fn prefix_hash_matches_whole_array_hash() {
        let all = vec![json!({"role": "user"}), json!({"role": "assistant"})];
        assert_eq!(hash_values(&all[..1]), hash_values(&[json!({"role": "user"})]));
        assert_ne!(hash_values(&all[..1]), hash_values(&all));
        assert_eq!(hash_values(&[]), hash_values(&[]));
    }

    #[test]
    fn truncate_str_is_char_boundary_safe() {
        // 按字符截断：不会切在多字节字符中间，也不会因为 CJK 占 3 字节而只留下 1/3 的内容
        assert_eq!(truncate_str("中文中文中文中文", 3), "中文中...(truncated)");
        assert_eq!(truncate_str("short", 100), "short");
        assert_eq!(truncate_str("abc", 3), "abc");
    }

    // ── 落盘端到端：写一轮会话日志，再按 base + delta 重建，验证格式自洽 ──

    fn read_events(dir: &std::path::Path) -> Vec<serde_json::Value> {
        let mut out = Vec::new();
        for entry in std::fs::read_dir(dir).expect("read log dir") {
            let path = entry.expect("dir entry").path();
            if path.extension().and_then(|e| e.to_str()) != Some("jsonl") {
                continue;
            }
            let text = std::fs::read_to_string(&path).expect("read jsonl");
            for line in text.lines().filter(|l| !l.trim().is_empty()) {
                out.push(serde_json::from_str(line).expect("parse jsonl line"));
            }
        }
        out
    }

    /// 复刻查看器的重建逻辑：request_base + request_delta/request_full → loop -> 完整请求
    fn rebuild_requests(events: &[serde_json::Value]) -> std::collections::HashMap<usize, serde_json::Value> {
        let mut bases: std::collections::HashMap<String, serde_json::Value> = std::collections::HashMap::new();
        let mut messages: Vec<serde_json::Value> = Vec::new();
        let mut current_base: Option<String> = None;
        let mut out = std::collections::HashMap::new();
        for ev in events {
            let kind = ev.get("type").and_then(|t| t.as_str()).unwrap_or("");
            match kind {
                "request_base" => {
                    let id = ev["base_id"].as_str().unwrap_or_default().to_string();
                    bases.insert(id, ev["base"].clone());
                }
                "request_full" => {
                    let id = ev["base_id"].as_str().unwrap_or_default().to_string();
                    messages = ev["messages"].as_array().cloned().unwrap_or_default();
                    current_base = Some(id);
                    let mut body = bases
                        .get(current_base.as_deref().unwrap_or_default())
                        .cloned()
                        .unwrap_or_else(|| json!({}));
                    body["messages"] = serde_json::Value::Array(messages.clone());
                    out.insert(ev["loop_count"].as_u64().unwrap_or(0) as usize, body);
                }
                "request_delta" => {
                    let id = ev["base_id"].as_str().unwrap_or_default().to_string();
                    assert_eq!(current_base.as_deref(), Some(id.as_str()), "delta 的 base 必须已写过");
                    // 前缀校验：重建出来的前缀哈希必须和落盘的一致
                    let prefix = &messages[..ev["from_index"].as_u64().unwrap_or(0) as usize];
                    assert_eq!(hash_values(prefix), ev["prefix_hash"].as_str().unwrap_or_default());
                    messages.extend(ev["added"].as_array().cloned().unwrap_or_default());
                    let mut body = bases.get(&id).cloned().unwrap_or_else(|| json!({}));
                    body["messages"] = serde_json::Value::Array(messages.clone());
                    out.insert(ev["loop_count"].as_u64().unwrap_or(0) as usize, body);
                }
                _ => {}
            }
        }
        out
    }

    #[test]
    fn jsonl_round_trip_base_plus_delta() {
        let dir = std::env::temp_dir().join(format!(
            "jarvis-debug-logger-test-{}",
            uuid::Uuid::new_v4()
        ));
        let logger = DebugLogger::with_dir(dir.clone());
        let sid = "testsession";

        let req1 = json!({
            "model": "deepseek-flash",
            "max_tokens": 8192,
            "stream": true,
            "system": "SYSTEM-PROMPT-".repeat(200),
            "tools": [{"name": "ReadFile"}, {"name": "SwitchWorkMode"}],
            "messages": [{"role": "user", "content": "你好"}],
        });
        logger.log_request(sid, "MAIN", 1, &req1);

        // 第二轮：messages 追加（应当走增量）
        let mut req2 = req1.clone();
        req2["messages"] = json!([
            {"role": "user", "content": "你好"},
            {"role": "assistant", "content": [{"type": "tool_use", "name": "SwitchWorkMode"}]},
        ]);
        logger.log_request(sid, "MAIN", 2, &req2);

        // 流式：3 片 SSE + response（应当聚合成一条 sse_summary）
        for chunk in ["{\"a\":1}", "{\"b\":2}", "[DONE]"] {
            logger.log_sse_event(sid, "MAIN", 2, chunk);
        }
        logger.log_response(
            sid,
            "MAIN",
            2,
            110,
            0,
            1,
            440,
            24,
            Some(1536),
            Some(190),
            Some("prompt_cache_hit_tokens"),
            false,
            None,
        );

        // 第三轮：历史被压缩重写（应当退化成全量）
        let mut req3 = req1.clone();
        req3["messages"] = json!([
            {"role": "user", "content": "[上下文压缩摘要]"},
            {"role": "assistant", "content": "ok"},
        ]);
        logger.log_request(sid, "MAIN", 3, &req3);

        logger.log_session_summary(sid, 440, 24, "FINISH");

        let events = read_events(&dir);
        let kinds: Vec<&str> = events
            .iter()
            .map(|e| e["type"].as_str().unwrap_or(""))
            .collect();
        assert_eq!(
            kinds,
            vec![
                "request_base",
                "request_full",
                "request_delta",
                "sse_summary",
                "response",
                "request_full",
                "session_summary",
            ],
            "落盘事件序列（base 只写一次，压缩后退化全量）"
        );

        // 增量事件只带新增的 1 条 message
        let delta = &events[2];
        assert_eq!(delta["from_index"], json!(1));
        assert_eq!(delta["added"].as_array().unwrap().len(), 1);

        // SSE 聚合：3 片 / 计数与字节正确，且不再逐片落盘
        let sse = &events[3];
        assert_eq!(sse["count"], json!(3));
        assert_eq!(sse["bytes"], json!(7 + 7 + 6));
        assert_eq!(sse["agent_type"], json!("MAIN"));
        assert!(!kinds.contains(&"sse_event"), "默认不逐片写 SSE 原文");

        // 重建：每个 loop 都能还原出与原始请求一致的 messages
        let rebuilt = rebuild_requests(&events);
        assert_eq!(rebuilt[&1]["messages"], req1["messages"]);
        assert_eq!(rebuilt[&2]["messages"], req2["messages"]);
        assert_eq!(rebuilt[&3]["messages"], req3["messages"]);
        // 固定块（system/tools/model）也能从 base 带回来
        assert_eq!(rebuilt[&2]["system"], req1["system"]);
        assert_eq!(rebuilt[&2]["tools"], req1["tools"]);

        // 体积：增量格式明显小于"每轮一份完整请求"
        let new_bytes: usize = std::fs::read_dir(&dir)
            .unwrap()
            .filter_map(|e| e.ok())
            .map(|e| std::fs::metadata(e.path()).map(|m| m.len() as usize).unwrap_or(0))
            .sum();
        let old_bytes: usize = [&req1, &req2, &req3]
            .iter()
            .map(|r| serde_json::to_string(r).unwrap().len() + 64)
            .sum();
        assert!(
            new_bytes < old_bytes / 2,
            "增量格式应显著小于全量：new={new_bytes} old={old_bytes}"
        );

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn sse_raw_mode_switch_is_off_by_default() {
        assert!(!env_flag("JARVIS_LOG_SSE_RAW", "1"), "默认不逐片记录 SSE");
        assert!(!env_flag("JARVIS_LOG_REQUEST", "FULL"), "默认按增量记录请求");
    }
}
