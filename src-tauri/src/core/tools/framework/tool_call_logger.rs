//! # tool_call_logger.rs — 工具调用审计日志模块
//!
//! 记录每次工具调用的成败、错误类型和纠正轨迹，用于分析 LLM 在元工具架构下的错误模式。
//! 日志以 JSONL 格式 append-only 写入 `data/logs/tool_calls/` 目录，按 session 隔离。
//! 配套 HTML 查看器：`data/logs/log-viewer.html`（统一审计中心）
//!
//! ## Key Exports
//! - `ToolCallLogger`: 全局日志写入器（单例，线程安全）
//! - `log_deferred_call()`: 记录 RunDeferredTool 调用（含校验失败和执行结果）
//! - `log_core_call()`: 记录核心工具直接调用
//! - `flush_session_summary()`: 生成 session 级汇总统计
//!
//! ## Dependencies
//! - Internal: `crate::infra::config::data_paths`
//! - External: `serde`, `serde_json`, `chrono`
//!
//! ## Constraints
//! - append-only 写入，不持有文件句柄（每次写入独立 open/write/close）
//! - per-session 纠正追踪在内存中维护，不持久化
//! - args_summary 仅记录关键字段，不序列化完整参数（避免日志膨胀）

use std::collections::HashMap;
use std::fs::OpenOptions;
use std::io::Write;
use std::path::PathBuf;
use std::sync::Mutex;

use serde::Serialize;

// ───────────────────────── 枚举定义 ─────────────────────────

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ToolCallStatus {
    Ok,
    Error,
    Blocked,
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ErrorType {
    MissingParam,
    ToolNotFound,
    NotDeferred,
    IntentBlocked,
    ModeBlocked,
    ExecutionFailed,
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum CorrectionStrategy {
    RetryWithFix,
    SwitchTool,
    StepBack,
    GiveUp,
}

// ───────────────────────── 日志记录结构 ─────────────────────────

#[derive(Debug, Clone, Serialize)]
pub struct CorrectionInfo {
    pub is_correction: bool,
    pub corrects_seq: Option<u64>,
    pub attempt: u32,
    pub strategy: Option<CorrectionStrategy>,
}

impl Default for CorrectionInfo {
    fn default() -> Self {
        Self {
            is_correction: false,
            corrects_seq: None,
            attempt: 0,
            strategy: None,
        }
    }
}

#[derive(Debug, Clone, Serialize)]
pub struct DiagnosisInfo {
    pub followed_protocol: bool,
    pub searched_before: bool,
    pub schema_read: bool,
}

#[derive(Debug, Clone, Serialize)]
pub struct ToolCallRecord {
    pub ts: String,
    pub session_id: String,
    pub seq: u64,
    pub tool: String,
    pub args_summary: serde_json::Value,
    pub intent: String,
    pub work_mode: String,
    pub status: ToolCallStatus,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error_type: Option<ErrorType>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error_message: Option<String>,
    pub correction: CorrectionInfo,
    pub diagnosis: DiagnosisInfo,
}

// ───────────────────────── per-session 纠正追踪状态 ─────────────────────────

struct LastError {
    seq: u64,
    tool_name: String,
    error_type: ErrorType,
    correction_attempts: u32,
}

pub struct SessionToolAudit {
    seq: u64,
    /// 本 session 内是否调用过 SearchTools（用于 diagnosis.followed_protocol）
    searched_tools_names: Vec<String>,
    /// 最近一次未纠正的错误
    last_error: Option<LastError>,
    /// 统计
    total_calls: u64,
    total_errors: u64,
    total_corrections: u64,
    successful_corrections: u64,
    unresolved_errors: u64,
    error_breakdown: HashMap<String, u64>,
}

impl SessionToolAudit {
    fn new() -> Self {
        Self {
            seq: 0,
            searched_tools_names: Vec::new(),
            last_error: None,
            total_calls: 0,
            total_errors: 0,
            total_corrections: 0,
            successful_corrections: 0,
            unresolved_errors: 0,
            error_breakdown: HashMap::new(),
        }
    }

    fn next_seq(&mut self) -> u64 {
        self.seq += 1;
        self.seq
    }

    fn record_search(&mut self, tool_names: Vec<String>) {
        for name in tool_names {
            if !self.searched_tools_names.contains(&name) {
                self.searched_tools_names.push(name);
            }
        }
    }

    fn record_error(&mut self, seq: u64, tool_name: &str, error_type: &ErrorType) {
        self.total_errors += 1;
        *self.error_breakdown.entry(format!("{:?}", error_type)).or_insert(0) += 1;
        self.last_error = Some(LastError {
            seq,
            tool_name: tool_name.to_string(),
            error_type: error_type.clone(),
            correction_attempts: 0,
        });
    }

    fn build_correction(&mut self, current_tool: &str) -> CorrectionInfo {
        if let Some(ref mut last_err) = self.last_error {
            // 本次调用是对上次错误的纠正尝试
            let is_related = current_tool == last_err.tool_name
                || (last_err.error_type == ErrorType::NotDeferred && current_tool == "RunDeferredTool");
            if is_related {
                last_err.correction_attempts += 1;
                self.total_corrections += 1;
                return CorrectionInfo {
                    is_correction: true,
                    corrects_seq: Some(last_err.seq),
                    attempt: last_err.correction_attempts,
                    strategy: Some(infer_strategy(&last_err.error_type, current_tool)),
                };
            }
        }
        CorrectionInfo::default()
    }

    fn clear_error_on_success(&mut self) {
        if self.last_error.is_some() {
            self.successful_corrections += 1;
            self.last_error = None;
        }
    }

    /// 当新调用与上次错误无关时，将上次错误标记为"放弃/未解决"
    fn abandon_unrelated_error(&mut self, current_tool: &str) {
        if let Some(ref last_err) = self.last_error {
            let is_related = current_tool == last_err.tool_name
                || (last_err.error_type == ErrorType::NotDeferred && current_tool == "RunDeferredTool");
            if !is_related {
                self.unresolved_errors += 1;
                self.last_error = None;
            }
        }
    }
}

fn infer_strategy(error_type: &ErrorType, current_tool: &str) -> CorrectionStrategy {
    match error_type {
        ErrorType::ToolNotFound => CorrectionStrategy::SwitchTool,
        ErrorType::NotDeferred => {
            if current_tool == "RunDeferredTool" {
                CorrectionStrategy::RetryWithFix
            } else {
                CorrectionStrategy::SwitchTool
            }
        }
        ErrorType::MissingParam => CorrectionStrategy::RetryWithFix,
        ErrorType::ModeBlocked | ErrorType::IntentBlocked => CorrectionStrategy::StepBack,
        ErrorType::ExecutionFailed => CorrectionStrategy::RetryWithFix,
    }
}

// ───────────────────────── 全局 Logger ─────────────────────────

pub struct ToolCallLogger {
    log_dir: PathBuf,
    sessions: Mutex<HashMap<String, SessionToolAudit>>,
}

impl ToolCallLogger {
    pub fn new() -> Self {
        let log_dir = crate::infra::config::data_paths::logs_dir().join("tool_calls");
        let _ = std::fs::create_dir_all(&log_dir);
        Self {
            log_dir,
            sessions: Mutex::new(HashMap::new()),
        }
    }

    /// 记录 RunDeferredTool 调用
    pub fn log_deferred_call(
        &self,
        session_id: &str,
        tool_name: &str,
        args: &serde_json::Value,
        intent: &str,
        work_mode: &str,
        status: ToolCallStatus,
        error_type: Option<ErrorType>,
        error_message: Option<String>,
        searched_before: bool,
    ) {
        let mut sessions = self.sessions.lock().unwrap();
        let audit = sessions
            .entry(session_id.to_string())
            .or_insert_with(SessionToolAudit::new);

        // 如果新调用与上次错误无关，将上次错误标记为放弃
        audit.abandon_unrelated_error(tool_name);

        let seq = audit.next_seq();
        let correction = audit.build_correction(tool_name);
        let followed_protocol = searched_before;

        if status == ToolCallStatus::Ok {
            audit.clear_error_on_success();
        } else if let Some(ref et) = error_type {
            audit.record_error(seq, tool_name, et);
        }

        audit.total_calls += 1;

        let record = ToolCallRecord {
            ts: chrono::Utc::now().to_rfc3339(),
            session_id: session_id.to_string(),
            seq,
            tool: "RunDeferredTool".to_string(),
            args_summary: extract_deferred_args_summary(tool_name, args),
            intent: intent.to_string(),
            work_mode: work_mode.to_string(),
            status,
            error_type,
            error_message,
            correction,
            diagnosis: DiagnosisInfo {
                followed_protocol,
                searched_before,
                schema_read: searched_before,
            },
        };

        drop(sessions);
        self.write_record(session_id, &record);
    }

    /// 记录核心工具直接调用
    pub fn log_core_call(
        &self,
        session_id: &str,
        tool_name: &str,
        args: &serde_json::Value,
        intent: &str,
        work_mode: &str,
        status: ToolCallStatus,
        error_message: Option<String>,
    ) {
        let mut sessions = self.sessions.lock().unwrap();
        let audit = sessions
            .entry(session_id.to_string())
            .or_insert_with(SessionToolAudit::new);

        // 如果新调用与上次错误无关，将上次错误标记为放弃
        audit.abandon_unrelated_error(tool_name);

        let seq = audit.next_seq();
        audit.total_calls += 1;

        let record = ToolCallRecord {
            ts: chrono::Utc::now().to_rfc3339(),
            session_id: session_id.to_string(),
            seq,
            tool: tool_name.to_string(),
            args_summary: extract_core_args_summary(tool_name, args),
            intent: intent.to_string(),
            work_mode: work_mode.to_string(),
            status,
            error_type: None,
            error_message,
            correction: CorrectionInfo::default(),
            diagnosis: DiagnosisInfo {
                followed_protocol: true,
                searched_before: true,
                schema_read: true,
            },
        };

        drop(sessions);
        self.write_record(session_id, &record);
    }

    /// 检查某工具是否在本 session 内被 SearchTools 查询过
    pub fn has_searched(&self, session_id: &str, tool_name: &str) -> bool {
        let sessions = self.sessions.lock().unwrap();
        sessions
            .get(session_id)
            .map(|a| a.searched_tools_names.iter().any(|n| n == tool_name))
            .unwrap_or(false)
    }

    /// 记录 SearchTools 调用（更新审计状态）
    pub fn record_search_tools(&self, session_id: &str, matched_names: Vec<String>) {
        let mut sessions = self.sessions.lock().unwrap();
        let audit = sessions
            .entry(session_id.to_string())
            .or_insert_with(SessionToolAudit::new);
        audit.record_search(matched_names);
    }

    /// 生成 session 级汇总统计并写入 summary 文件
    pub fn flush_session_summary(&self, session_id: &str) {
        let mut sessions = self.sessions.lock().unwrap();
        let Some(mut audit) = sessions.remove(session_id) else {
            return;
        };

        // session 结束时，如果 last_error 仍存在，计为未解决
        if audit.last_error.is_some() {
            audit.unresolved_errors += 1;
        }

        let correction_rate = if audit.total_corrections > 0 {
            audit.successful_corrections as f64 / audit.total_corrections as f64
        } else {
            0.0
        };

        let summary = serde_json::json!({
            "session_id": session_id,
            "total_calls": audit.total_calls,
            "total_errors": audit.total_errors,
            "total_corrections": audit.total_corrections,
            "correction_success_rate": correction_rate,
            "unresolved_errors": audit.unresolved_errors,
            "error_breakdown": audit.error_breakdown,
        });

        let path = self
            .log_dir
            .join(format!("{}_summary.json", session_id));
        if let Ok(mut file) = OpenOptions::new()
            .create(true)
            .write(true)
            .truncate(true)
            .open(&path)
        {
            let _ = writeln!(
                file,
                "{}",
                serde_json::to_string_pretty(&summary).unwrap_or_default()
            );
        }
    }

    fn write_record(&self, session_id: &str, record: &ToolCallRecord) {
        let date = chrono::Local::now().format("%Y-%m-%d").to_string();
        let filename = format!("{}_{}.jsonl", date, session_id);
        let path = self.log_dir.join(filename);

        if let Ok(mut file) = OpenOptions::new()
            .create(true)
            .append(true)
            .open(&path)
        {
            if let Ok(line) = serde_json::to_string(record) {
                let _ = writeln!(file, "{}", line);
            }
        }
    }
}

impl Default for ToolCallLogger {
    fn default() -> Self {
        Self::new()
    }
}

// ───────────────────────── 辅助函数 ─────────────────────────

/// 从 RunDeferredTool 的 args 中提取关键字段作为日志摘要（避免序列化完整参数）
fn extract_deferred_args_summary(tool_name: &str, args: &serde_json::Value) -> serde_json::Value {
    let mut summary = serde_json::json!({ "name": tool_name });

    // 优先从 args["args"] 提取（正常调用路径），fallback 到顶层（错误路径）
    let source = args.get("args").and_then(|v| v.as_object()).or_else(|| args.as_object());

    if let Some(inner_args) = source {
        for key in &["path", "command", "query", "pattern", "content"] {
            if let Some(val) = inner_args.get(*key) {
                let display = if let Some(s) = val.as_str() {
                    if s.len() > 200 {
                        serde_json::Value::String(format!("{}...(truncated)", &s[..200]))
                    } else {
                        val.clone()
                    }
                } else {
                    val.clone()
                };
                summary[*key] = display;
            }
        }
    }
    summary
}

/// 从核心工具的 args 中提取关键字段
fn extract_core_args_summary(tool_name: &str, args: &serde_json::Value) -> serde_json::Value {
    let mut summary = serde_json::json!({ "name": tool_name });
    for key in &["path", "command", "query", "pattern", "prompt", "skill"] {
        if let Some(val) = args.get(*key) {
            let display = if let Some(s) = val.as_str() {
                if s.len() > 200 {
                    serde_json::Value::String(format!("{}...(truncated)", &s[..200]))
                } else {
                    val.clone()
                }
            } else {
                val.clone()
            };
            summary[*key] = display;
        }
    }
    summary
}

// ───────────────────────── 全局单例 ─────────────────────────

use std::sync::OnceLock;

static LOGGER: OnceLock<ToolCallLogger> = OnceLock::new();

pub fn tool_call_logger() -> &'static ToolCallLogger {
    LOGGER.get_or_init(ToolCallLogger::new)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_correction_tracking() {
        let mut audit = SessionToolAudit::new();
        // 模拟一次错误
        let seq = audit.next_seq();
        audit.record_error(seq, "WriteFile", &ErrorType::ModeBlocked);
        assert_eq!(audit.total_errors, 1);

        // 模拟纠正尝试
        let correction = audit.build_correction("WriteFile");
        assert!(correction.is_correction);
        assert_eq!(correction.corrects_seq, Some(1));
        assert_eq!(correction.attempt, 1);

        // 纠正成功
        audit.clear_error_on_success();
        assert_eq!(audit.successful_corrections, 1);
        assert!(audit.last_error.is_none());
    }

    #[test]
    fn test_deferred_args_summary() {
        let args = serde_json::json!({
            "name": "WriteFile",
            "args": {
                "path": "src/main.rs",
                "content": "fn main() {}",
                "extra_field": "ignored"
            }
        });
        let summary = extract_deferred_args_summary("WriteFile", &args);
        assert_eq!(summary["name"], "WriteFile");
        assert_eq!(summary["path"], "src/main.rs");
        // content 超过 200 字符时会被截断，这里不会
        assert!(summary.get("extra_field").is_none());
    }
}
