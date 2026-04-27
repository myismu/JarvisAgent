use std::collections::HashMap;
use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};
use tauri::{Emitter, Manager};
use tokio::sync::Mutex;
use tokio_util::sync::CancellationToken;

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SubAgentStatus {
    Running,
    Completed,
    Failed,
    Cancelled,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SubAgentPhase {
    Starting,
    WaitingModel,
    Streaming,
    Thinking,
    CallingTool,
    ProcessingToolResult,
    Finalizing,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SubAgentRun {
    pub run_id: String,
    pub session_id: String,
    pub task_id: Option<i32>,
    pub label: String,
    pub prompt_preview: String,
    pub read_only: bool,
    pub status: SubAgentStatus,
    pub phase: SubAgentPhase,
    pub loop_count: usize,
    pub max_loops: usize,
    pub current_tool: Option<String>,
    pub current_tool_input: Option<String>,
    pub input_tokens: u64,
    pub output_tokens: u64,
    pub started_at: u64,
    pub updated_at: u64,
    pub finished_at: Option<u64>,
    pub error: Option<String>,
    pub summary: Option<String>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SubAgentEvent {
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

pub struct SubAgentMonitor {
    pub runs: HashMap<String, SubAgentRun>,
    pub events: HashMap<String, Vec<SubAgentEvent>>,
    pub cancel_tokens: HashMap<String, CancellationToken>,
}

impl SubAgentMonitor {
    pub fn new() -> Self {
        Self {
            runs: HashMap::new(),
            events: HashMap::new(),
            cancel_tokens: HashMap::new(),
        }
    }

    pub fn list(&self, session_id: Option<&str>) -> Vec<SubAgentRun> {
        let mut runs: Vec<SubAgentRun> = self
            .runs
            .values()
            .filter(|run| session_id.map_or(true, |sid| run.session_id == sid))
            .cloned()
            .collect();
        runs.sort_by_key(|run| run.started_at);
        runs
    }

    pub fn list_events(
        &self,
        session_id: Option<&str>,
        run_id: Option<&str>,
    ) -> Vec<SubAgentEvent> {
        let mut events: Vec<SubAgentEvent> = self
            .events
            .iter()
            .filter(|(rid, _)| run_id.map_or(true, |target| target == rid.as_str()))
            .flat_map(|(_, events)| events.iter())
            .filter(|event| session_id.map_or(true, |sid| event.session_id == sid))
            .cloned()
            .collect();
        events.sort_by_key(|event| event.timestamp);
        events
    }

    pub async fn start_run(
        app: &tauri::AppHandle,
        session_id: &str,
        prompt: &str,
        read_only: bool,
        task_id: Option<i32>,
        label: Option<String>,
    ) -> String {
        let run_id = format!("sa_{}", uuid::Uuid::new_v4().to_string()[..8].to_string());
        let now = now_millis();
        let cancel_token = CancellationToken::new();
        let prompt_preview = preview(prompt, 160);
        let label = label
            .filter(|value| !value.trim().is_empty())
            .unwrap_or_else(|| preview(prompt, 48));

        let run = SubAgentRun {
            run_id: run_id.clone(),
            session_id: session_id.to_string(),
            task_id,
            label,
            prompt_preview,
            read_only,
            status: SubAgentStatus::Running,
            phase: SubAgentPhase::Starting,
            loop_count: 0,
            max_loops: crate::core::constants::MAX_AGENT_LOOP_BEFORE_CONFIRM,
            current_tool: None,
            current_tool_input: None,
            input_tokens: 0,
            output_tokens: 0,
            started_at: now,
            updated_at: now,
            finished_at: None,
            error: None,
            summary: None,
        };

        if let Some(state) = app.try_state::<SubAgentMonitorState>() {
            let mut monitor = state.0.lock().await;
            monitor.runs.insert(run_id.clone(), run.clone());
            monitor.cancel_tokens.insert(run_id.clone(), cancel_token);
        }

        emit_update(app, &run);
        Self::push_event(
            app,
            &run_id,
            "start",
            format!("Started subagent: {}", run.label),
            None,
            Some(run.prompt_preview.clone()),
            None,
            None,
            0,
            0,
            0,
        )
        .await;
        spawn_heartbeat(app.clone(), run_id.clone());
        run_id
    }

    pub async fn update_run<F>(
        app: &tauri::AppHandle,
        run_id: &str,
        update: F,
    ) -> Option<SubAgentRun>
    where
        F: FnOnce(&mut SubAgentRun),
    {
        let state = app.try_state::<SubAgentMonitorState>()?;
        let payload = {
            let mut monitor = state.0.lock().await;
            let run = monitor.runs.get_mut(run_id)?;
            update(run);
            run.updated_at = now_millis();
            run.clone()
        };

        emit_update(app, &payload);
        Some(payload)
    }

    pub async fn update_phase(
        app: &tauri::AppHandle,
        run_id: &str,
        phase: SubAgentPhase,
        loop_count: usize,
        input_tokens: u64,
        output_tokens: u64,
    ) {
        let phase_message = format!("Phase changed to {:?}", phase);
        let _ = Self::update_run(app, run_id, |run| {
            run.phase = phase;
            run.loop_count = loop_count;
            run.current_tool = None;
            run.current_tool_input = None;
            run.input_tokens = input_tokens;
            run.output_tokens = output_tokens;
        })
        .await;
        Self::push_event(
            app,
            run_id,
            "phase",
            phase_message,
            None,
            None,
            None,
            None,
            loop_count,
            input_tokens,
            output_tokens,
        )
        .await;
    }

    pub async fn update_tool(
        app: &tauri::AppHandle,
        run_id: &str,
        tool: &str,
        input_summary: Option<String>,
        loop_count: usize,
        input_tokens: u64,
        output_tokens: u64,
    ) {
        let event_input = input_summary.clone();
        let _ = Self::update_run(app, run_id, |run| {
            run.phase = SubAgentPhase::CallingTool;
            run.loop_count = loop_count;
            run.current_tool = Some(tool.to_string());
            run.current_tool_input = input_summary;
            run.input_tokens = input_tokens;
            run.output_tokens = output_tokens;
        })
        .await;
        Self::push_event(
            app,
            run_id,
            "tool_call",
            format!("Calling tool {}", tool),
            Some(tool.to_string()),
            event_input,
            None,
            None,
            loop_count,
            input_tokens,
            output_tokens,
        )
        .await;
    }

    pub async fn record_tool_result(
        app: &tauri::AppHandle,
        run_id: &str,
        tool: &str,
        output_summary: Option<String>,
        loop_count: usize,
        input_tokens: u64,
        output_tokens: u64,
    ) {
        let event_output = output_summary.clone();
        let _ = Self::update_run(app, run_id, |run| {
            run.phase = SubAgentPhase::ProcessingToolResult;
            run.loop_count = loop_count;
            run.current_tool = Some(tool.to_string());
            run.current_tool_input = None;
            run.input_tokens = input_tokens;
            run.output_tokens = output_tokens;
        })
        .await;
        Self::push_event(
            app,
            run_id,
            "tool_result",
            format!("Tool {} completed", tool),
            Some(tool.to_string()),
            None,
            event_output,
            None,
            loop_count,
            input_tokens,
            output_tokens,
        )
        .await;
    }

    pub async fn complete_run(
        app: &tauri::AppHandle,
        run_id: &str,
        input_tokens: u64,
        output_tokens: u64,
        final_answer: Option<String>,
    ) {
        let summary = Self::build_summary(app, run_id, final_answer.as_deref(), None).await;
        let _ = Self::update_run(app, run_id, |run| {
            run.status = SubAgentStatus::Completed;
            run.phase = SubAgentPhase::Finalizing;
            run.current_tool = None;
            run.current_tool_input = None;
            run.input_tokens = input_tokens;
            run.output_tokens = output_tokens;
            run.finished_at = Some(now_millis());
            run.error = None;
            run.summary = Some(summary.clone());
        })
        .await;
        Self::remove_cancel_token(app, run_id).await;
        Self::push_event(
            app,
            run_id,
            "complete",
            summary,
            None,
            None,
            None,
            None,
            0,
            input_tokens,
            output_tokens,
        )
        .await;
    }

    pub async fn fail_run(
        app: &tauri::AppHandle,
        run_id: &str,
        error: String,
        input_tokens: u64,
        output_tokens: u64,
    ) {
        let event_error = error.clone();
        let summary = Self::build_summary(app, run_id, None, Some(error.as_str())).await;
        let _ = Self::update_run(app, run_id, |run| {
            run.status = SubAgentStatus::Failed;
            run.phase = SubAgentPhase::Finalizing;
            run.current_tool = None;
            run.current_tool_input = None;
            run.input_tokens = input_tokens;
            run.output_tokens = output_tokens;
            run.finished_at = Some(now_millis());
            run.error = Some(error);
            run.summary = Some(summary.clone());
        })
        .await;
        Self::remove_cancel_token(app, run_id).await;
        Self::push_event(
            app,
            run_id,
            "error",
            "Subagent failed".to_string(),
            None,
            None,
            None,
            Some(event_error),
            0,
            input_tokens,
            output_tokens,
        )
        .await;
    }

    pub async fn cancel_run(app: &tauri::AppHandle, run_id: &str) -> Result<SubAgentRun, String> {
        let state = app
            .try_state::<SubAgentMonitorState>()
            .ok_or_else(|| "Subagent monitor is not initialized".to_string())?;

        let payload = {
            let mut monitor = state.0.lock().await;
            let token = monitor
                .cancel_tokens
                .get(run_id)
                .cloned()
                .ok_or_else(|| format!("Subagent run {} is not cancellable", run_id))?;
            token.cancel();

            let run = monitor
                .runs
                .get_mut(run_id)
                .ok_or_else(|| format!("Subagent run {} not found", run_id))?;
            if run.status != SubAgentStatus::Running {
                return Ok(run.clone());
            }
            run.status = SubAgentStatus::Cancelled;
            run.phase = SubAgentPhase::Finalizing;
            run.current_tool = None;
            run.current_tool_input = None;
            run.finished_at = Some(now_millis());
            run.error = Some("Cancelled by user".to_string());
            run.summary = Some(format!(
                "已取消：{}。已运行 {} 轮，累计 {} tokens。",
                run.label,
                run.loop_count,
                run.input_tokens + run.output_tokens
            ));
            run.updated_at = now_millis();
            run.clone()
        };

        emit_update(app, &payload);
        Self::push_event(
            app,
            run_id,
            "cancel",
            "Subagent cancellation requested".to_string(),
            None,
            None,
            None,
            Some("Cancelled by user".to_string()),
            payload.loop_count,
            payload.input_tokens,
            payload.output_tokens,
        )
        .await;
        Ok(payload)
    }

    pub async fn cancel_session(app: &tauri::AppHandle, session_id: &str) -> Vec<SubAgentRun> {
        let Some(state) = app.try_state::<SubAgentMonitorState>() else {
            return Vec::new();
        };

        let run_ids = {
            let monitor = state.0.lock().await;
            monitor
                .runs
                .values()
                .filter(|run| {
                    run.session_id == session_id
                        && run.status == SubAgentStatus::Running
                        && monitor.cancel_tokens.contains_key(&run.run_id)
                })
                .map(|run| run.run_id.clone())
                .collect::<Vec<_>>()
        };

        let mut cancelled = Vec::new();
        for run_id in run_ids {
            if let Ok(run) = Self::cancel_run(app, &run_id).await {
                cancelled.push(run);
            }
        }
        cancelled
    }

    pub async fn is_cancelled(app: &tauri::AppHandle, run_id: &str) -> bool {
        let Some(state) = app.try_state::<SubAgentMonitorState>() else {
            return false;
        };
        let monitor = state.0.lock().await;
        monitor
            .cancel_tokens
            .get(run_id)
            .map(|token| token.is_cancelled())
            .unwrap_or(false)
    }

    pub async fn cancel_token(app: &tauri::AppHandle, run_id: &str) -> Option<CancellationToken> {
        let state = app.try_state::<SubAgentMonitorState>()?;
        let monitor = state.0.lock().await;
        monitor.cancel_tokens.get(run_id).cloned()
    }

    pub async fn acknowledge_cancelled(app: &tauri::AppHandle, run_id: &str) {
        let _ = Self::update_run(app, run_id, |run| {
            run.status = SubAgentStatus::Cancelled;
            run.phase = SubAgentPhase::Finalizing;
            run.current_tool = None;
            run.current_tool_input = None;
            run.finished_at.get_or_insert_with(now_millis);
            run.error.get_or_insert_with(|| "Cancelled by user".to_string());
            run.summary.get_or_insert_with(|| {
                format!(
                    "已取消：{}。已运行 {} 轮，累计 {} tokens。",
                    run.label,
                    run.loop_count,
                    run.input_tokens + run.output_tokens
                )
            });
        })
        .await;
        Self::remove_cancel_token(app, run_id).await;
    }

    async fn remove_cancel_token(app: &tauri::AppHandle, run_id: &str) {
        if let Some(state) = app.try_state::<SubAgentMonitorState>() {
            let mut monitor = state.0.lock().await;
            monitor.cancel_tokens.remove(run_id);
        }
    }

    async fn build_summary(
        app: &tauri::AppHandle,
        run_id: &str,
        final_answer: Option<&str>,
        error: Option<&str>,
    ) -> String {
        let Some(state) = app.try_state::<SubAgentMonitorState>() else {
            return final_answer
                .map(|answer| preview(answer, 140))
                .or_else(|| error.map(|err| format!("失败：{}", preview(err, 120))))
                .unwrap_or_else(|| "子 Agent 已结束。".to_string());
        };

        let monitor = state.0.lock().await;
        let Some(run) = monitor.runs.get(run_id) else {
            return "子 Agent 已结束。".to_string();
        };
        let events = monitor.events.get(run_id).cloned().unwrap_or_default();
        let tool_count = events
            .iter()
            .filter(|event| event.event_type == "tool_call")
            .count();
        let last_tool = events
            .iter()
            .rev()
            .find_map(|event| event.tool.as_ref())
            .cloned();
        let total_tokens = run.input_tokens + run.output_tokens;

        if let Some(err) = error {
            return format!(
                "失败：{}。已运行 {} 轮，调用工具 {} 次，累计 {} tokens。",
                preview(err, 90),
                run.loop_count,
                tool_count,
                total_tokens
            );
        }

        let answer_summary = final_answer
            .filter(|answer| !answer.trim().is_empty())
            .map(|answer| preview(answer.trim(), 110))
            .unwrap_or_else(|| "未返回正文结果".to_string());
        let tool_part = last_tool
            .map(|tool| format!("，最后工具 {}", tool))
            .unwrap_or_default();

        format!(
            "完成：{}。运行 {} 轮，调用工具 {} 次{}，累计 {} tokens。",
            answer_summary,
            run.loop_count,
            tool_count,
            tool_part,
            total_tokens
        )
    }

    async fn push_event(
        app: &tauri::AppHandle,
        run_id: &str,
        event_type: &str,
        message: String,
        tool: Option<String>,
        input_summary: Option<String>,
        output_summary: Option<String>,
        error: Option<String>,
        loop_count: usize,
        input_tokens: u64,
        output_tokens: u64,
    ) -> Option<SubAgentEvent> {
        let state = app.try_state::<SubAgentMonitorState>()?;
        let event = {
            let mut monitor = state.0.lock().await;
            let run = monitor.runs.get(run_id)?;
            let event = SubAgentEvent {
                event_id: format!("sae_{}", uuid::Uuid::new_v4().to_string()[..8].to_string()),
                run_id: run_id.to_string(),
                session_id: run.session_id.clone(),
                event_type: event_type.to_string(),
                message,
                tool,
                input_summary,
                output_summary,
                error,
                loop_count: if loop_count == 0 {
                    run.loop_count
                } else {
                    loop_count
                },
                input_tokens,
                output_tokens,
                timestamp: now_millis(),
            };
            let events = monitor.events.entry(run_id.to_string()).or_default();
            events.push(event.clone());
            if events.len() > 300 {
                let overflow = events.len() - 300;
                events.drain(0..overflow);
            }
            event
        };

        let _ = app.emit("subagent-event", &event);
        Some(event)
    }
}

pub struct SubAgentMonitorState(pub Arc<Mutex<SubAgentMonitor>>);

impl Default for SubAgentMonitorState {
    fn default() -> Self {
        Self(Arc::new(Mutex::new(SubAgentMonitor::new())))
    }
}

fn emit_update(app: &tauri::AppHandle, run: &SubAgentRun) {
    let _ = app.emit("subagent-updated", run);
}

fn spawn_heartbeat(app: tauri::AppHandle, run_id: String) {
    tokio::spawn(async move {
        loop {
            tokio::time::sleep(std::time::Duration::from_secs(5)).await;
            let Some(state) = app.try_state::<SubAgentMonitorState>() else {
                break;
            };

            let payload = {
                let mut monitor = state.0.lock().await;
                let Some(run) = monitor.runs.get_mut(&run_id) else {
                    break;
                };
                if run.status != SubAgentStatus::Running {
                    break;
                }
                run.updated_at = now_millis();
                run.clone()
            };

            emit_update(&app, &payload);
        }
    });
}

fn now_millis() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_millis() as u64)
        .unwrap_or(0)
}

fn preview(value: &str, max_chars: usize) -> String {
    let mut preview: String = value.chars().take(max_chars).collect();
    if value.chars().count() > max_chars {
        preview.push_str("...");
    }
    preview
}
