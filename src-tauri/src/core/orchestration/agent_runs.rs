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
    pub message_id: Option<String>,
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
    message_id: Option<String>,
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
        message_id,
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

/// 根据 message_id 查找对应的 agent_run
pub fn find_run_by_message_id(message_id: &str) -> Option<AgentRun> {
    agent_run_repository::find_by_message_id(message_id).ok().flatten()
}

/// 根据 message_id 清理对应的 agent_run、events、checkpoints
pub fn cleanup_by_message_id(message_id: &str) {
    if let Some(run) = find_run_by_message_id(message_id) {
        let _ = agent_run_repository::delete_events_by_run(&run.run_id);
        let _ = agent_run_repository::delete_checkpoints_by_run(&run.run_id);
        let _ = agent_run_repository::delete_run(&run.run_id);
    }
}

/// 删除 agent_run 及其关联的 events 和 checkpoints
pub fn delete_run(run_id: &str) -> Result<(), String> {
    agent_run_repository::delete_run(run_id)
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

/// 应用退出（ExitRequested）收尾：把所有仍为 Running 的 run 标记为 Interrupted。
///
/// 与 `mark_stale_runs_interrupted` 的区别：无活跃排除集、不判 STALE ——
/// 退出路径上所有 Running 都是"被进程结束打断的"，全部留痕，
/// 避免永久 Running 僵尸（此前关闭窗口后 run 状态永远停在 Running，
/// 崩溃恢复也因找不到 Interrupted 状态而无法收口）。
pub fn mark_running_interrupted_on_exit() {
    let runs = agent_run_repository::list_runs(None).unwrap_or_default();
    for run in runs {
        if run.status != AgentRunStatus::Running {
            continue;
        }
        let _ = update_run_by_id(&run.run_id, |run| {
            run.status = AgentRunStatus::Interrupted;
            run.finished_at = Some(now_millis());
            run.summary = Some(INTERRUPTED_SUMMARY.to_string());
            run.resumable = agent_run_repository::load_checkpoint(&run.run_id)
                .ok()
                .flatten()
                .is_some();
        });
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

/// 标记 run 失败。
///
/// `input_tokens` / `output_tokens` 为本次运行真实累计值——旧实现硬编码传 0，
/// 导致失败轮次的消耗在成本统计中完全缺失。
pub fn fail_run(
    app: &tauri::AppHandle,
    run_id: &str,
    error: String,
    input_tokens: u64,
    output_tokens: u64,
) {
    finish_run(
        app,
        run_id,
        AgentRunStatus::Failed,
        input_tokens,
        output_tokens,
        None,
        Some(error),
    );
}

/// 标记 run 为已中断（运行被打断，非用户主动撤销）。
///
/// 与 [`fail_run`] 的区别：状态为 `Interrupted`（`resumable = true`，保留续跑语义），
/// 且 `error` 字段承载中断原因，同时会通过 `finish_run` 落一条结构化事件，
/// 供界面「执行详情」展示——错误信息不进对话消息内容，避免污染模型上下文。
pub fn interrupt_run(
    app: &tauri::AppHandle,
    run_id: &str,
    reason: String,
    input_tokens: u64,
    output_tokens: u64,
) {
    finish_run(
        app,
        run_id,
        AgentRunStatus::Interrupted,
        input_tokens,
        output_tokens,
        None,
        Some(reason),
    );
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

/// 中断恢复的结果语义分层。
///
/// `NeedsClosure` 是对称性保证的兜底：崩溃 run 无任何可补内容
/// （checkpoint 不领先、live 增量为空 —— 典型如"模式快照落库后、
/// 下一轮 LLM 输出前被杀"），但会话尾部悬尾（最后一条是 user 且
/// 无人回应）。此时补一条 assistant 中断占位，维持 session_messages
/// 消息级 user/assistant 严格交替，并把 run 从永久 Running 中捞出。
pub enum RecoveryOutcome {
    /// 无可恢复（活跃期内 / 没有中断 run —— 绝大多数加载的正常路径）
    None,
    /// 有内容可补：checkpoint 领先的消息 + 半截正文/思考
    Content {
        messages: Vec<Message>,
        live_content: String,
        live_thinking: String,
    },
    /// 无内容可补，但尾部悬尾，需补中断占位收口
    NeedsClosure,
}

/// 崩溃恢复的"未产生回复"占位文案（**会发给 LLM**，也会渲染为气泡下方小字）。
///
/// 与 pipeline 的 INTERRUPT_MARKER 系同风格：`⚠️` 开头 + Markdown 引用，
/// 前端 `splitInterruptMarker` 依此把整行剥离成 notice 小字。
/// 语义区别于 INTERRUPT_MARKER_RESUMABLE（"请基于上下文继续完成"）——
/// 这里没有任何半截内容可续，模型不该自动续写，等待用户下一条消息
/// 表达新意图。只在恢复路径出现一次，之后固定为历史前缀的一部分，
/// 不产生措辞变体（prompt cache 安全）。
pub const INTERRUPT_PLACEHOLDER_NO_REPLY: &str =
    "> ⚠️ **[回复被中断]** 本次执行因应用关闭而中断，未产生回复内容。";

/// 从中断的 run 中恢复消息，补回 session_memory 缺失的部分
///
/// 核心逻辑：
/// 1. 从 checkpoint 加载中断时的完整消息列表
/// 2. 与当前 session_memory 中的消息做对比，找出 checkpoint 中多出的部分
/// 3. 有内容 → 返回需要追加的消息 + 半截助手回复（live_content/live_thinking）
/// 4. 无内容但尾部悬尾 → 返回 `NeedsClosure`（补中断占位，维持消息级交替）
pub fn recover_interrupted_messages(
    session_id: &str,
    current_messages: &[Message],
) -> RecoveryOutcome {
    let Some(run) = find_interrupted_run(session_id) else {
        return RecoveryOutcome::None;
    };

    // 如果 run 状态是 Running，需要判断它是否真的已经中断
    // 条件：updated_at 超过 STALE 阈值（2分钟），才认为是崩溃导致的
    if run.status == AgentRunStatus::Running {
        let now = now_millis();
        if now.saturating_sub(run.updated_at) <= RUN_STALE_MS {
            // 还在活跃期内，可能是正在执行的 run，不要恢复
            return RecoveryOutcome::None;
        }
    }

    // 加载检查点的消息；无 checkpoint 的死 run 也走收口判定（留痕优先）
    let checkpoint = match agent_run_repository::load_checkpoint(&run.run_id)
        .ok()
        .flatten()
    {
        Some(cp) => cp,
        None => return closure_outcome(current_messages),
    };

    // 如果 checkpoint 的消息数 <= 当前 session_memory 的消息数，
    // 说明 session_memory 已经是最新的，不需要恢复
    if checkpoint.messages.len() <= current_messages.len() {
        // 但可能仍有半截助手回复（live_content 比 checkpoint 更新）
        if run.live_content.trim().is_empty() && run.live_thinking.trim().is_empty() {
            // 无任何内容可补：仍需收口判定 —— 崩溃点落在"检查点刚落库、
            // 下一轮 LLM 输出前"的空隙时（典型：模式快照落库后被杀），
            // 会话尾部悬尾（最后一条是 user 且无人回应），补一条 assistant
            // 中断占位维持 user/assistant 严格交替
            return closure_outcome(current_messages);
        }
        // checkpoint 和 session_memory 消息一致，但 live_content 有半截回复
        return RecoveryOutcome::Content {
            messages: vec![], // 不需要追加消息
            live_content: run.live_content.clone(),
            live_thinking: run.live_thinking.clone(),
        };
    }

    // 取出 checkpoint 中多出的消息（从 current_messages.len() 开始）
    let extra_messages: Vec<Message> = checkpoint
        .messages
        .into_iter()
        .skip(current_messages.len())
        .collect();

    if extra_messages.is_empty() && run.live_content.is_empty() && run.live_thinking.is_empty() {
        return RecoveryOutcome::None;
    }

    RecoveryOutcome::Content {
        messages: extra_messages,
        live_content: run.live_content.clone(),
        live_thinking: run.live_thinking.clone(),
    }
}

/// 悬尾收口判定：会话尾部是 user（含 tool_result 信封形态）且无人回应 → 需补占位；
/// 尾部已是 assistant / 会话为空 → 消息序列已对称，不动消息
/// （run 状态由 mark_stale_runs_interrupted / 退出收尾兜底）。
fn closure_outcome(current_messages: &[Message]) -> RecoveryOutcome {
    match current_messages.last() {
        Some(Message::User { .. }) => RecoveryOutcome::NeedsClosure,
        _ => RecoveryOutcome::None,
    }
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

#[cfg(test)]
mod recovery_closure_tests {
    //! 悬尾收口判定与占位文案的防回归。
    //!
    //! 背景：崩溃点落在"检查点刚落库、下一轮 LLM 输出前"的空隙时
    //! （典型：模式快照落库后进程被杀），session_messages 尾部悬尾
    //! （最后一条是 user 且无人回应），旧恢复逻辑三门槛不满足直接
    //! 返回 None，既不补内容也不留痕 —— 连续 user 相邻由此产生。
    use super::{closure_outcome, RecoveryOutcome, INTERRUPT_PLACEHOLDER_NO_REPLY};
    use crate::infra::types::models::{Content, ContentBlock, Message};

    /// 尾部悬尾（tool_result 信封形态也是 user）→ 需要补占位收口
    #[test]
    fn tail_user_tool_result_envelope_needs_closure() {
        let messages = vec![
            Message::Assistant {
                content: Content::Single("计划如下".to_string()),
            },
            Message::User {
                content: Content::Multiple(vec![ContentBlock::ToolResult {
                    tool_use_id: "call_00_x".to_string(),
                    content: "ok".to_string(),
                }]),
            },
        ];
        assert!(matches!(
            closure_outcome(&messages),
            RecoveryOutcome::NeedsClosure
        ));
    }

    /// 尾部是普通 user 文本（LLM 回复产生前被杀）→ 同样需要收口
    #[test]
    fn tail_plain_user_message_needs_closure() {
        let messages = vec![Message::User {
            content: Content::Single("开始吧".to_string()),
        }];
        assert!(matches!(
            closure_outcome(&messages),
            RecoveryOutcome::NeedsClosure
        ));
    }

    /// 尾部已是 assistant → 消息序列已对称，不动消息
    #[test]
    fn tail_assistant_stays_untouched() {
        let messages = vec![
            Message::User {
                content: Content::Single("开始吧".to_string()),
            },
            Message::Assistant {
                content: Content::Single("已完成".to_string()),
            },
        ];
        assert!(matches!(closure_outcome(&messages), RecoveryOutcome::None));
    }

    /// 空会话不动
    #[test]
    fn empty_messages_stay_untouched() {
        assert!(matches!(closure_outcome(&[]), RecoveryOutcome::None));
    }

    /// 占位文案与前端 splitInterruptMarker 的剥离约定兼容：
    /// ⚠ 符号开头（正则 `[⚠✕]` 识别整行剥成 notice 小字）+ Markdown 引用开头
    #[test]
    fn placeholder_matches_frontend_strip_convention() {
        assert!(
            INTERRUPT_PLACEHOLDER_NO_REPLY.trim_start().starts_with('>'),
            "应以 Markdown 引用开头，便于统一剥离：{INTERRUPT_PLACEHOLDER_NO_REPLY}"
        );
        assert!(
            INTERRUPT_PLACEHOLDER_NO_REPLY.contains('⚠'),
            "前端 splitInterruptMarker 依赖 ⚠/✕ 符号识别：{INTERRUPT_PLACEHOLDER_NO_REPLY}"
        );
        assert!(
            INTERRUPT_PLACEHOLDER_NO_REPLY.contains("[回复被中断]"),
            "保持与 INTERRUPT_MARKER 系统一的可识别前缀：{INTERRUPT_PLACEHOLDER_NO_REPLY}"
        );
    }
}
