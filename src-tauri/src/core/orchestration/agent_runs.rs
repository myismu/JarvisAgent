//! 主Agent执行记录模块 - 运行历史与检查点管理
//!
//! 记录主Agent每次执行的完整生命周期：启动、思考、工具调用、完成/失败。
//! 支持检查点保存与恢复，用于断点续传和崩溃恢复。
//! 事件以 JSONL 格式追加存储，运行状态以 JSON 文件持久化。

use std::fs::{self, OpenOptions};
use std::io::Write;
use std::path::PathBuf;
use std::time::{SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};
use serde_json::json;
use tauri::Emitter;

use crate::core::models::Message;
use crate::get_agent_home;

/// 运行记录过期阈值（毫秒），超过此时间未更新视为中断
const RUN_STALE_MS: u64 = 15_000;

/// 主Agent运行状态
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AgentRunStatus {
    Running,     // 运行中
    Completed,   // 已完成
    Failed,      // 失败
    Cancelled,   // 已取消
    Interrupted, // 已中断（应用关闭等）
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AgentRun {
    pub run_id: String,
    pub session_id: String,
    pub status: AgentRunStatus,
    pub user_message_preview: String,
    pub loop_count: usize,
    pub input_tokens: u64,
    pub output_tokens: u64,
    pub started_at: u64,
    pub updated_at: u64,
    pub finished_at: Option<u64>,
    pub last_safe_point: Option<String>,
    pub live_thinking: String,
    pub live_tool_buffer: String,
    pub live_content: String,
    pub error: Option<String>,
    pub summary: Option<String>,
    pub resumable: bool,
    pub resumed_from_run_id: Option<String>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AgentRunEvent {
    pub event_id: String,
    pub run_id: String,
    pub session_id: String,
    pub event_type: String,
    pub message: String,
    pub tool: Option<String>,
    pub input_summary: Option<String>,
    pub output_summary: Option<String>,
    pub error: Option<String>,
    pub loop_count: usize,
    pub input_tokens: u64,
    pub output_tokens: u64,
    pub timestamp: u64,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AgentRunCheckpoint {
    pub run_id: String,
    pub session_id: String,
    pub loop_count: usize,
    pub messages: Vec<Message>,
    pub input_tokens: u64,
    pub output_tokens: u64,
    pub last_safe_point: String,
    pub updated_at: u64,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ResumeAgentRunPlan {
    pub session_id: String,
    pub prompt: String,
}

/// 开始一次新的Agent运行
///
/// 创建运行记录并向前端发送启动事件。
/// 返回生成的 run_id，用于后续的状态更新。
pub fn start_run(
    app: &tauri::AppHandle,
    session_id: &str,
    user_message: &str,
    resumed_from_run_id: Option<String>,
) -> String {
    let run_id = format!("ar_{}", &uuid::Uuid::new_v4().to_string()[..8]);
    let now = now_millis();
    let run = AgentRun {
        run_id: run_id.clone(),
        session_id: session_id.to_string(),
        status: AgentRunStatus::Running,
        user_message_preview: preview(user_message, 120),
        loop_count: 0,
        input_tokens: 0,
        output_tokens: 0,
        started_at: now,
        updated_at: now,
        finished_at: None,
        last_safe_point: None,
        live_thinking: String::new(),
        live_tool_buffer: String::new(),
        live_content: String::new(),
        error: None,
        summary: None,
        resumable: false,
        resumed_from_run_id,
    };
    let _ = write_run(&run);
    emit_run(app, &run);
    push_event(
        app,
        &run_id,
        session_id,
        "start",
        "主 Agent 开始执行".to_string(),
        None,
        Some(run.user_message_preview.clone()),
        None,
        None,
        0,
        0,
        0,
    );
    run_id
}

/// 列出所有运行记录，可按 session_id 过滤
///
/// 自动将超时未更新的 Running 状态标记为 Interrupted
pub fn list_runs(session_id: Option<&str>) -> Vec<AgentRun> {
    let mut runs = Vec::new();
    let root = runs_dir();
    if let Ok(session_dirs) = fs::read_dir(root) {
        for session_dir in session_dirs.flatten() {
            let path = session_dir.path();
            if !path.is_dir() {
                continue;
            }
            if let Some(target_session_id) = session_id {
                if path.file_name().and_then(|name| name.to_str()) != Some(target_session_id) {
                    continue;
                }
            }
            if let Ok(run_dirs) = fs::read_dir(path) {
                for run_dir in run_dirs.flatten() {
                    let run_path = run_dir.path().join("run.json");
                    if let Ok(content) = fs::read_to_string(&run_path) {
                        if let Ok(mut run) = serde_json::from_str::<AgentRun>(&content) {
                            if run.status == AgentRunStatus::Running
                                && now_millis().saturating_sub(run.updated_at) > RUN_STALE_MS
                            {
                                run.status = AgentRunStatus::Interrupted;
                                run.finished_at = Some(run.updated_at);
                                run.summary = Some("上次执行在应用关闭或进程结束时中断。".to_string());
                                run.resumable = checkpoint_path(&run.session_id, &run.run_id).exists();
                                let _ = write_run(&run);
                            }
                            runs.push(run);
                        }
                    }
                }
            }
        }
    }
    runs.sort_by_key(|run| run.started_at);
    runs
}

pub fn list_events(session_id: Option<&str>, run_id: Option<&str>) -> Vec<AgentRunEvent> {
    let mut events = Vec::new();
    for run in list_runs(session_id) {
        if run_id.map_or(false, |target| target != run.run_id) {
            continue;
        }
        let path = events_path(&run.session_id, &run.run_id);
        if let Ok(content) = fs::read_to_string(path) {
            for line in content.lines() {
                if let Ok(event) = serde_json::from_str::<AgentRunEvent>(line) {
                    events.push(event);
                }
            }
        }
    }
    events.sort_by_key(|event| event.timestamp);
    events
}

pub fn append_thinking(app: &tauri::AppHandle, run_id: &str, content: &str, loop_count: usize) {
    let _ = update_run_by_id(run_id, |run| {
        run.live_thinking.push_str(content);
        run.loop_count = loop_count;
    })
    .map(|run| emit_run(app, &run));
    if let Some(run) = load_run_by_id(run_id) {
        push_event(
            app,
            run_id,
            &run.session_id,
            "thinking",
            content.to_string(),
            None,
            None,
            None,
            None,
            loop_count,
            run.input_tokens,
            run.output_tokens,
        );
    }
}

pub fn append_content(app: &tauri::AppHandle, run_id: &str, content: &str, loop_count: usize) {
    let _ = update_run_by_id(run_id, |run| {
        run.live_content.push_str(content);
        run.loop_count = loop_count;
    })
    .map(|run| emit_run(app, &run));
    if let Some(run) = load_run_by_id(run_id) {
        push_event(
            app,
            run_id,
            &run.session_id,
            "content",
            content.to_string(),
            None,
            None,
            None,
            None,
            loop_count,
            run.input_tokens,
            run.output_tokens,
        );
    }
}

pub fn append_tool_log(app: &tauri::AppHandle, run_id: &str, content: &str, loop_count: usize) {
    let _ = update_run_by_id(run_id, |run| {
        run.live_tool_buffer.push_str(content);
        run.loop_count = loop_count;
    })
    .map(|run| emit_run(app, &run));
}

pub fn record_tool_call(
    app: &tauri::AppHandle,
    run_id: &str,
    tool: &str,
    input_summary: Option<String>,
    loop_count: usize,
) {
    if let Some(run) = load_run_by_id(run_id) {
        push_event(
            app,
            run_id,
            &run.session_id,
            "tool_call",
            format!("调用工具 {}", tool),
            Some(tool.to_string()),
            input_summary,
            None,
            None,
            loop_count,
            run.input_tokens,
            run.output_tokens,
        );
    }
}

pub fn record_tool_result(
    app: &tauri::AppHandle,
    run_id: &str,
    tool: &str,
    output_summary: Option<String>,
    error: Option<String>,
    loop_count: usize,
) {
    if let Some(run) = load_run_by_id(run_id) {
        push_event(
            app,
            run_id,
            &run.session_id,
            if error.is_some() { "tool_error" } else { "tool_result" },
            if error.is_some() {
                format!("工具 {} 执行失败", tool)
            } else {
                format!("工具 {} 执行完成", tool)
            },
            Some(tool.to_string()),
            None,
            output_summary,
            error,
            loop_count,
            run.input_tokens,
            run.output_tokens,
        );
    }
}

pub fn save_checkpoint(
    app: &tauri::AppHandle,
    run_id: &str,
    session_id: &str,
    loop_count: usize,
    messages: Vec<Message>,
    input_tokens: u64,
    output_tokens: u64,
    last_safe_point: &str,
) {
    let checkpoint = AgentRunCheckpoint {
        run_id: run_id.to_string(),
        session_id: session_id.to_string(),
        loop_count,
        messages,
        input_tokens,
        output_tokens,
        last_safe_point: last_safe_point.to_string(),
        updated_at: now_millis(),
    };
    let _ = fs::write(
        checkpoint_path(session_id, run_id),
        serde_json::to_string_pretty(&checkpoint).unwrap_or_default(),
    );
    let _ = update_run_by_id(run_id, |run| {
        run.loop_count = loop_count;
        run.input_tokens = input_tokens;
        run.output_tokens = output_tokens;
        run.last_safe_point = Some(last_safe_point.to_string());
        run.resumable = true;
    })
    .map(|run| emit_run(app, &run));
    push_event(
        app,
        run_id,
        session_id,
        "checkpoint",
        format!("已保存安全点：{}", last_safe_point),
        None,
        None,
        None,
        None,
        loop_count,
        input_tokens,
        output_tokens,
    );
}

pub fn complete_run(
    app: &tauri::AppHandle,
    run_id: &str,
    input_tokens: u64,
    output_tokens: u64,
    summary: Option<String>,
) {
    finish_run(app, run_id, AgentRunStatus::Completed, input_tokens, output_tokens, summary, None);
}

pub fn cancel_run(
    app: &tauri::AppHandle,
    run_id: &str,
    input_tokens: u64,
    output_tokens: u64,
    summary: Option<String>,
) {
    finish_run(app, run_id, AgentRunStatus::Cancelled, input_tokens, output_tokens, summary, None);
}

pub fn fail_run(app: &tauri::AppHandle, run_id: &str, error: String) {
    finish_run(app, run_id, AgentRunStatus::Failed, 0, 0, None, Some(error));
}

/// 准备恢复执行 - 加载检查点并生成恢复提示词
///
/// 返回检查点数据和恢复计划，用于断点续传
pub fn prepare_resume(
    run_id: &str,
) -> Result<(AgentRunCheckpoint, ResumeAgentRunPlan), String> {
    let run = load_run_by_id(run_id).ok_or_else(|| format!("执行记录不存在: {}", run_id))?;
    let content = fs::read_to_string(checkpoint_path(&run.session_id, run_id))
        .map_err(|_| "没有可恢复的安全点".to_string())?;
    let checkpoint: AgentRunCheckpoint = serde_json::from_str(&content).map_err(|e| e.to_string())?;
    let prompt = format!(
        "继续上次中断的执行。上次执行记录为 {}，最后安全点为「{}」，已完成 {} 轮。请基于已有上下文继续，不要重复已经完成且已有工具结果的操作；如果某个操作可能有副作用，请先说明并等待确认。",
        run_id, checkpoint.last_safe_point, checkpoint.loop_count
    );
    Ok((
        checkpoint.clone(),
        ResumeAgentRunPlan {
            session_id: checkpoint.session_id.clone(),
            prompt,
        },
    ))
}

fn finish_run(
    app: &tauri::AppHandle,
    run_id: &str,
    status: AgentRunStatus,
    input_tokens: u64,
    output_tokens: u64,
    summary: Option<String>,
    error: Option<String>,
) {
    if let Ok(run) = update_run_by_id(run_id, |run| {
        run.status = status.clone();
        run.input_tokens = input_tokens;
        run.output_tokens = output_tokens;
        run.finished_at = Some(now_millis());
        run.summary = summary.clone();
        run.error = error.clone();
        run.resumable = status == AgentRunStatus::Interrupted;
    }) {
        emit_run(app, &run);
        push_event(
            app,
            run_id,
            &run.session_id,
            match run.status {
                AgentRunStatus::Completed => "complete",
                AgentRunStatus::Failed => "error",
                AgentRunStatus::Cancelled => "cancel",
                AgentRunStatus::Interrupted => "interrupted",
                AgentRunStatus::Running => "phase",
            },
            run.summary
                .clone()
                .or_else(|| run.error.clone())
                .unwrap_or_else(|| "执行结束".to_string()),
            None,
            None,
            None,
            run.error.clone(),
            run.loop_count,
            run.input_tokens,
            run.output_tokens,
        );
    }
}

fn push_event(
    app: &tauri::AppHandle,
    run_id: &str,
    session_id: &str,
    event_type: &str,
    message: String,
    tool: Option<String>,
    input_summary: Option<String>,
    output_summary: Option<String>,
    error: Option<String>,
    loop_count: usize,
    input_tokens: u64,
    output_tokens: u64,
) {
    let event = AgentRunEvent {
        event_id: format!("are_{}", &uuid::Uuid::new_v4().to_string()[..8]),
        run_id: run_id.to_string(),
        session_id: session_id.to_string(),
        event_type: event_type.to_string(),
        message,
        tool,
        input_summary,
        output_summary,
        error,
        loop_count,
        input_tokens,
        output_tokens,
        timestamp: now_millis(),
    };
    let path = events_path(session_id, run_id);
    if let Ok(mut file) = OpenOptions::new().create(true).append(true).open(path) {
        let _ = writeln!(file, "{}", serde_json::to_string(&event).unwrap_or_default());
    }
    let _ = app.emit("agent-run-event", event);
}

fn emit_run(app: &tauri::AppHandle, run: &AgentRun) {
    let _ = app.emit("agent-run-updated", run);
}

fn load_run_by_id(run_id: &str) -> Option<AgentRun> {
    let root = runs_dir();
    let session_dirs = fs::read_dir(root).ok()?;
    for session_dir in session_dirs.flatten() {
        let run_path = session_dir.path().join(run_id).join("run.json");
        if let Ok(content) = fs::read_to_string(run_path) {
            if let Ok(run) = serde_json::from_str::<AgentRun>(&content) {
                return Some(run);
            }
        }
    }
    None
}

fn update_run_by_id<F>(run_id: &str, update: F) -> Result<AgentRun, String>
where
    F: FnOnce(&mut AgentRun),
{
    let mut run = load_run_by_id(run_id).ok_or_else(|| format!("执行记录不存在: {}", run_id))?;
    update(&mut run);
    run.updated_at = now_millis();
    write_run(&run)?;
    Ok(run)
}

fn write_run(run: &AgentRun) -> Result<(), String> {
    let dir = run_dir(&run.session_id, &run.run_id);
    fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
    fs::write(
        dir.join("run.json"),
        serde_json::to_string_pretty(run).unwrap_or_else(|_| json!({}).to_string()),
    )
    .map_err(|e| e.to_string())
}

fn runs_dir() -> PathBuf {
    let dir = get_agent_home().join(crate::core::constants::DIR_AGENT_RUNS);
    let _ = fs::create_dir_all(&dir);
    dir
}

fn run_dir(session_id: &str, run_id: &str) -> PathBuf {
    let dir = runs_dir().join(session_id).join(run_id);
    let _ = fs::create_dir_all(&dir);
    dir
}

fn events_path(session_id: &str, run_id: &str) -> PathBuf {
    run_dir(session_id, run_id).join("events.jsonl")
}

fn checkpoint_path(session_id: &str, run_id: &str) -> PathBuf {
    run_dir(session_id, run_id).join("checkpoint.json")
}

fn now_millis() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as u64
}

fn preview(value: &str, max_chars: usize) -> String {
    let mut chars = value.chars();
    let preview: String = chars.by_ref().take(max_chars).collect();
    if chars.next().is_some() {
        format!("{}...", preview)
    } else {
        preview
    }
}
