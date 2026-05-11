//! 主Agent执行记录模块 - 运行历史与检查点管理
//!
//! 记录主Agent每次执行的完整生命周期：启动、思考、工具调用、完成/失败。
//! 支持检查点保存与恢复，用于断点续传和崩溃恢复。
//! 运行状态、事件和可恢复检查点持久化到 SQLite。

use std::collections::HashSet;
use std::time::{SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};
use tauri::Emitter;

use crate::infra::types::models::Message;
use crate::core::orchestration::agent_run_repository;

/// 运行记录过期阈值（毫秒），超过此时间未更新视为中断
const RUN_STALE_MS: u64 = 120_000;
const INTERRUPTED_SUMMARY: &str = "上次执行在应用关闭或进程结束时中断。";

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
    agent_run_repository::list_runs(session_id).unwrap_or_default()
}

pub fn mark_active_run(app: &tauri::AppHandle, run_id: &str) -> Option<AgentRun> {
    let run = update_run_by_id(run_id, |run| {
        keep_run_active(run);
    })
    .ok()?;
    emit_run(app, &run);
    Some(run)
}

pub fn mark_stale_runs_interrupted(
    app: &tauri::AppHandle,
    session_id: Option<&str>,
    active_run_ids: &HashSet<String>,
    active_session_ids: &HashSet<String>,
) {
    let runs = list_runs(session_id);
    let now = now_millis();
    for run in runs {
        if run.status != AgentRunStatus::Running || active_run_ids.contains(&run.run_id) {
            continue;
        }
        let session_has_active_task = active_session_ids.contains(&run.session_id);
        if session_has_active_task && now.saturating_sub(run.updated_at) <= RUN_STALE_MS {
            continue;
        }
        if let Ok(run) = update_run_by_id(&run.run_id, |run| {
            run.status = AgentRunStatus::Interrupted;
            run.finished_at = Some(run.updated_at);
            run.summary = Some(INTERRUPTED_SUMMARY.to_string());
            run.resumable = agent_run_repository::load_checkpoint(&run.run_id)
                .ok()
                .flatten()
                .is_some();
        }) {
            emit_run(app, &run);
        }
    }
}

pub fn list_events(session_id: Option<&str>, run_id: Option<&str>) -> Vec<AgentRunEvent> {
    agent_run_repository::list_events(session_id, run_id).unwrap_or_default()
}

/// 追加思考内容（流式 delta）
///
/// 只更新 `agent_runs.live_thinking` 字段，不往 `agent_run_events` 插入碎片事件。
/// 思考内容在 turn 结束时通过 `flush_thinking_event()` 一次性写入事件表。
pub fn append_thinking(app: &tauri::AppHandle, run_id: &str, content: &str, loop_count: usize) {
    let _ = update_run_by_id(run_id, |run| {
        keep_run_active(run);
        run.live_thinking.push_str(content);
        run.loop_count = loop_count;
    })
    .map(|run| emit_run(app, &run));
}

/// 追加回复内容（流式 delta）
///
/// 只更新 `agent_runs.live_content` 字段，不往 `agent_run_events` 插入碎片事件。
/// 回复内容在 turn 结束时通过 `flush_content_event()` 一次性写入事件表。
pub fn append_content(app: &tauri::AppHandle, run_id: &str, content: &str, loop_count: usize) {
    let _ = update_run_by_id(run_id, |run| {
        keep_run_active(run);
        run.live_content.push_str(content);
        run.loop_count = loop_count;
    })
    .map(|run| emit_run(app, &run));
}

/// 将当前累积的回复内容作为一条完整事件写入 agent_run_events
///
/// 在每轮 turn 结束时调用（而非每个 delta 都调用），大幅减少事件数量。
pub fn flush_content_event(app: &tauri::AppHandle, run_id: &str, loop_count: usize) {
    if let Some(run) = load_run_by_id(run_id) {
        if !run.live_content.is_empty() {
            push_event(
                app,
                run_id,
                &run.session_id,
                "content",
                run.live_content.clone(),
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
}

/// 将当前累积的思考内容作为一条完整事件写入 agent_run_events
///
/// 在每轮 turn 结束时调用（而非每个 delta 都调用），大幅减少事件数量。
pub fn flush_thinking_event(app: &tauri::AppHandle, run_id: &str, loop_count: usize) {
    if let Some(run) = load_run_by_id(run_id) {
        if !run.live_thinking.is_empty() {
            push_event(
                app,
                run_id,
                &run.session_id,
                "thinking",
                run.live_thinking.clone(),
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
}

pub fn append_tool_log(app: &tauri::AppHandle, run_id: &str, content: &str, loop_count: usize) {
    let _ = update_run_by_id(run_id, |run| {
        keep_run_active(run);
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
            if error.is_some() {
                "tool_error"
            } else {
                "tool_result"
            },
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
    let _ = agent_run_repository::upsert_checkpoint(&checkpoint);
    let _ = update_run_by_id(run_id, |run| {
        keep_run_active(run);
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
    finish_run(
        app,
        run_id,
        AgentRunStatus::Completed,
        input_tokens,
        output_tokens,
        summary,
        None,
    );
}

pub fn cancel_run(
    app: &tauri::AppHandle,
    run_id: &str,
    input_tokens: u64,
    output_tokens: u64,
    summary: Option<String>,
) {
    finish_run(
        app,
        run_id,
        AgentRunStatus::Cancelled,
        input_tokens,
        output_tokens,
        summary,
        None,
    );
}

pub fn fail_run(app: &tauri::AppHandle, run_id: &str, error: String) {
    finish_run(app, run_id, AgentRunStatus::Failed, 0, 0, None, Some(error));
}

/// 准备恢复执行 - 加载检查点并生成恢复提示词
///
/// 返回检查点数据和恢复计划，用于断点续传
pub fn prepare_resume(run_id: &str) -> Result<(AgentRunCheckpoint, ResumeAgentRunPlan), String> {
    load_run_by_id(run_id).ok_or_else(|| format!("执行记录不存在: {}", run_id))?;
    let checkpoint = agent_run_repository::load_checkpoint(run_id)?
        .ok_or_else(|| "没有可恢复的安全点".to_string())?;
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

/// 查找指定会话最近一个中断的 run
///
/// 同时查找 Interrupted 和 Running 状态的 run，
/// 因为程序崩溃时 run 的状态仍然是 Running，还没来得及标记为 Interrupted
pub fn find_interrupted_run(session_id: &str) -> Option<AgentRun> {
    let runs = agent_run_repository::list_runs(Some(session_id)).ok()?;
    runs.into_iter()
        .filter(|r| r.status == AgentRunStatus::Interrupted || r.status == AgentRunStatus::Running)
        .max_by_key(|r| r.updated_at)
}

/// 从中断的 run 中恢复消息，补回 session_memory 缺失的部分
///
/// 核心逻辑：
/// 1. 从 checkpoint 加载中断时的完整消息列表
/// 2. 与当前 session_memory 中的消息做对比，找出 checkpoint 中多出的部分
/// 3. 返回需要追加的消息 + 半截助手回复（live_content/live_thinking）
///
/// 返回 (需要追加的消息, 半截助手文本, 半截思考文本)
pub fn recover_interrupted_messages(
    session_id: &str,
    current_messages: &[Message],
) -> Option<(Vec<Message>, String, String)> {
    let run = find_interrupted_run(session_id)?;

    // 如果 run 状态是 Running，需要判断它是否真的已经中断
    // 条件：updated_at 超过 STALE 阈值（2分钟），才认为是崩溃导致的
    if run.status == AgentRunStatus::Running {
        let now = now_millis();
        if now.saturating_sub(run.updated_at) <= RUN_STALE_MS {
            // 还在活跃期内，可能是正在执行的 run，不要恢复
            return None;
        }
    }

    // 加载检查点的消息
    let checkpoint = agent_run_repository::load_checkpoint(&run.run_id)
        .ok()
        .flatten()?;

    // 如果 checkpoint 的消息数 <= 当前 session_memory 的消息数，
    // 说明 session_memory 已经是最新的，不需要恢复
    if checkpoint.messages.len() <= current_messages.len() {
        // 但可能仍有半截助手回复（live_content 比 checkpoint 更新）
        if run.live_content.trim().is_empty() && run.live_thinking.trim().is_empty() {
            return None;
        }
        // checkpoint 和 session_memory 消息一致，但 live_content 有半截回复
        return Some((
            vec![], // 不需要追加消息
            run.live_content.clone(),
            run.live_thinking.clone(),
        ));
    }

    // 取出 checkpoint 中多出的消息（从 current_messages.len() 开始）
    let extra_messages: Vec<Message> = checkpoint
        .messages
        .into_iter()
        .skip(current_messages.len())
        .collect();

    if extra_messages.is_empty() && run.live_content.is_empty() && run.live_thinking.is_empty() {
        return None;
    }

    Some((
        extra_messages,
        run.live_content.clone(),
        run.live_thinking.clone(),
    ))
}

/// 将中断 run 标记为已恢复，避免下次加载时重复恢复
pub fn mark_run_recovered(run_id: &str) -> Result<(), String> {
    update_run_by_id(run_id, |run| {
        run.status = AgentRunStatus::Completed;
        run.finished_at = Some(now_millis());
        if run.summary.is_none() || run.summary.as_deref() == Some(INTERRUPTED_SUMMARY) {
            run.summary = Some("已从中断恢复".to_string());
        }
    })?;
    Ok(())
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
    let _ = agent_run_repository::append_event(&event);
    let _ = app.emit("agent-run-event", event);
}

fn emit_run(app: &tauri::AppHandle, run: &AgentRun) {
    let _ = app.emit("agent-run-updated", run);
}

fn load_run_by_id(run_id: &str) -> Option<AgentRun> {
    agent_run_repository::load_run(run_id).ok().flatten()
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

fn keep_run_active(run: &mut AgentRun) {
    if matches!(
        run.status,
        AgentRunStatus::Completed | AgentRunStatus::Failed | AgentRunStatus::Cancelled
    ) {
        return;
    }
    run.status = AgentRunStatus::Running;
    run.finished_at = None;
    if run.summary.as_deref() == Some(INTERRUPTED_SUMMARY) {
        run.summary = None;
    }
}

fn write_run(run: &AgentRun) -> Result<(), String> {
    agent_run_repository::upsert_run(run)
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
