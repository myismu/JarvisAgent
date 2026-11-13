//! # subagents.rs — 子 Agent 生命周期、实时状态与持久化轨迹
//!
//! 管理子Agent的运行状态、逐轮轨迹存储与取消控制。
//! 通过 Tauri 事件系统向前端推送实时状态更新。
//! 支持心跳检测、批量取消和持久化轨迹查询。
//!
//! ## Key Exports
//! - `SubAgentMonitor`: 管理运行态并提供 SQLite 轨迹查询
//! - `SubAgentRun`: 子 Agent 实时运行状态
//! - `SubAgentTraceRunSummary`: 按会话恢复列表所需的轻量索引
//! - `SubAgentTraceEvent`: 单轮模型请求、回复和工具结果
//!
//! ## Dependencies
//! - Internal: `crate::infra::db`
//! - External: `serde`, `tokio`, `tokio-util`, `tauri`
//!
//! ## Constraints
//! - 详细轨迹写入 SQLite；运行态只在进程内保留并通过事件推送

use std::collections::HashMap;
use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};
use tauri::{Emitter, Manager};
use tokio::sync::Mutex;
use tokio_util::sync::CancellationToken;

/// 子Agent运行状态
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SubAgentStatus {
    Running,   // 运行中
    Completed, // 已完成
    Failed,    // 失败
    Cancelled, // 已取消
}

/// 子Agent执行阶段 - 用于实时状态展示
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SubAgentPhase {
    Starting,             // 启动中
    WaitingModel,         // 等待模型响应
    Streaming,            // 流式输出中
    Thinking,             // 思考中
    CallingTool,          // 调用工具中
    ProcessingToolResult, // 处理工具结果
    Finalizing,           // 收尾中
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SubAgentRun {
    pub run_id: String,
    pub session_id: String,
    pub task_id: Option<i32>,
    pub label: String,
    pub agent_role: String,
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
pub struct SubAgentTraceEvent {
    pub event_id: String,
    pub run_id: String,
    pub session_id: String,
    pub loop_index: usize,
    pub event_type: String,
    pub payload: serde_json::Value,
    pub model: Option<String>,
    pub input_tokens: Option<u64>,
    pub output_tokens: Option<u64>,
    pub timestamp: u64,
}

/// 从 SQLite 恢复会话级子 Agent 列表所需的轻量索引信息。
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SubAgentTraceRunSummary {
    pub run_id: String,
    pub session_id: String,
    pub parent_run_id: Option<String>,
    pub label: String,
    pub started_at: u64,
    pub loop_count: usize,
}

pub struct SubAgentMonitor {
    pub runs: HashMap<String, SubAgentRun>,
    pub cancel_tokens: HashMap<String, CancellationToken>,
}

impl SubAgentMonitor {
    pub fn new() -> Self {
        Self {
            runs: HashMap::new(),
            cancel_tokens: HashMap::new(),
        }
    }

    pub fn list(&mut self, session_id: Option<&str>) -> Vec<SubAgentRun> {
        self.cleanup_expired();
        let mut runs: Vec<SubAgentRun> = self
            .runs
            .values()
            .filter(|run| session_id.map_or(true, |sid| run.session_id == sid))
            .cloned()
            .collect();
        runs.sort_by_key(|run| run.started_at);
        runs
    }

    /// 清理已完成超过 1 小时的运行记录，防止内存无限增长
    fn cleanup_expired(&mut self) {
        let now = now_millis();
        let ttl_ms: u64 = 3600 * 1000; // 1 小时
        let expired: Vec<String> = self
            .runs
            .iter()
            .filter(|(_, run)| {
                run.status != SubAgentStatus::Running
                    && run
                        .finished_at
                        .map_or(false, |finished| now.saturating_sub(finished) > ttl_ms)
            })
            .map(|(id, _)| id.clone())
            .collect();
        for id in &expired {
            self.runs.remove(id);
            self.cancel_tokens.remove(id);
        }
    }

    pub fn create_trace_run(session_id: &str, run_id: &str, parent_run_id: Option<&str>, label: &str, started_at: u64) -> Result<(), String> {
        crate::infra::db::with_connection(|conn| {
            conn.execute(
                "INSERT OR IGNORE INTO subagent_trace_runs(run_id, session_id, parent_run_id, label, started_at) VALUES (?1, ?2, ?3, ?4, ?5)",
                rusqlite::params![run_id, session_id, parent_run_id, label, started_at as i64],
            ).map_err(|e| e.to_string())?;
            Ok(())
        })
    }

    pub fn list_trace_runs(session_id: &str) -> Result<Vec<SubAgentTraceRunSummary>, String> {
        crate::infra::db::with_connection(|conn| {
            let mut stmt = conn.prepare(
                "SELECT r.run_id, r.session_id, r.parent_run_id, r.label, r.started_at,
                        COALESCE(MAX(e.loop_index), 0),
                        (SELECT request.payload_json
                         FROM subagent_trace_events request
                         WHERE request.run_id = r.run_id AND request.event_type = 'request'
                         ORDER BY request.loop_index, request.timestamp
                         LIMIT 1)
                 FROM subagent_trace_runs r
                 LEFT JOIN subagent_trace_events e ON e.run_id = r.run_id
                 WHERE r.session_id = ?1
                 GROUP BY r.run_id
                 ORDER BY r.started_at DESC, r.run_id DESC",
            ).map_err(|e| e.to_string())?;
            let rows = stmt.query_map([session_id], |row| {
                let run_id: String = row.get(0)?;
                let stored_label: String = row.get(3)?;
                let first_request: Option<String> = row.get(6)?;
                let inferred_label = first_request
                    .as_deref()
                    .and_then(infer_trace_label);
                Ok(SubAgentTraceRunSummary {
                    run_id,
                    session_id: row.get(1)?,
                    parent_run_id: row.get(2)?,
                    label: if stored_label.trim().is_empty() {
                        inferred_label.unwrap_or_else(|| "子 Agent".to_string())
                    } else {
                        stored_label
                    },
                    started_at: row.get::<_, i64>(4)?.max(0) as u64,
                    loop_count: row.get::<_, i64>(5)?.max(0) as usize,
                })
            }).map_err(|e| e.to_string())?;
            rows.collect::<Result<Vec<_>, _>>().map_err(|e| e.to_string())
        })
    }

    pub async fn append_trace_event(app: &tauri::AppHandle, run_id: &str, loop_index: usize, event_type: &str,
        payload: &serde_json::Value, model: Option<&str>, input_tokens: Option<u64>, output_tokens: Option<u64>) -> Result<(), String> {
        let state = app.try_state::<SubAgentMonitorState>().ok_or_else(|| "Subagent monitor is not initialized".to_string())?;
        let session_id = state.0.lock().await.runs.get(run_id).map(|r| r.session_id.clone())
            .ok_or_else(|| format!("Subagent run {} not found", run_id))?;
        let payload_json = serde_json::to_string(payload).map_err(|e| e.to_string())?;
        let timestamp = now_millis();
        crate::infra::db::with_connection(|conn| {
            conn.execute("INSERT INTO subagent_trace_events(event_id, run_id, session_id, loop_index, event_type, payload_json, model, input_tokens, output_tokens, timestamp) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10)",
                rusqlite::params![format!("sat_{}", uuid::Uuid::new_v4().simple()), run_id, session_id, loop_index as i64, event_type, payload_json, model, input_tokens.map(|v| v as i64), output_tokens.map(|v| v as i64), timestamp as i64]).map_err(|e| e.to_string())?;
            Ok(())
        })?;
        let _ = app.emit("subagent-trace-updated", serde_json::json!({"runId":run_id,"loopIndex":loop_index}));
        Ok(())
    }

    pub fn list_trace_events(run_id: &str) -> Result<Vec<SubAgentTraceEvent>, String> {
        crate::infra::db::with_connection(|conn| {
            let mut stmt = conn.prepare("SELECT event_id, run_id, session_id, loop_index, event_type, payload_json, model, input_tokens, output_tokens, timestamp FROM subagent_trace_events WHERE run_id=?1 ORDER BY loop_index, timestamp").map_err(|e| e.to_string())?;
            let rows = stmt.query_map([run_id], |row| {
                let json: String = row.get(5)?;
                Ok(SubAgentTraceEvent { event_id: row.get(0)?, run_id: row.get(1)?, session_id: row.get(2)?, loop_index: row.get::<_, i64>(3)? as usize, event_type: row.get(4)?, payload: serde_json::from_str(&json).unwrap_or(serde_json::Value::Null), model: row.get(6)?, input_tokens: row.get::<_, Option<i64>>(7)?.map(|v| v as u64), output_tokens: row.get::<_, Option<i64>>(8)?.map(|v| v as u64), timestamp: row.get::<_, i64>(9)? as u64 })
            }).map_err(|e| e.to_string())?;
            rows.collect::<Result<Vec<_>, _>>().map_err(|e| e.to_string())
        })
    }

    /// 启动子Agent运行
    ///
    /// 创建运行记录、注册取消令牌、启动心跳检测。
    /// 返回生成的 run_id。
    pub async fn start_run(
        app: &tauri::AppHandle,
        session_id: &str,
        prompt: &str,
        read_only: bool,
        task_id: Option<i32>,
        label: Option<String>,
        agent_role: String,
        max_loops: usize,
        parent_run_id: Option<String>,
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
            agent_role,
            prompt_preview,
            read_only,
            status: SubAgentStatus::Running,
            phase: SubAgentPhase::Starting,
            loop_count: 0,
            max_loops,
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

        if let Err(err) = Self::create_trace_run(session_id, &run_id, parent_run_id.as_deref(), &run.label, now) {
            crate::jarvis_warn!("SUBAGENT", "failed to create trace run: {}", err);
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

    /// 取消指定会话的所有运行中子Agent
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
            run.error
                .get_or_insert_with(|| "Cancelled by user".to_string());
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
        let tool_count = 0usize;
        let last_tool: Option<String> = None;
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
            answer_summary, run.loop_count, tool_count, tool_part, total_tokens
        )
    }

    async fn push_event(
        _app: &tauri::AppHandle,
        _run_id: &str,
        _event_type: &str,
        _message: String,
        _tool: Option<String>,
        _input_summary: Option<String>,
        _output_summary: Option<String>,
        _error: Option<String>,
        _loop_count: usize,
        _input_tokens: u64,
        _output_tokens: u64,
    ) {
        // Legacy call sites are removed as the UI migrates to persisted loop traces.
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

fn infer_trace_label(payload_json: &str) -> Option<String> {
    let payload: serde_json::Value = serde_json::from_str(payload_json).ok()?;
    let messages = payload.get("messages")?.as_array()?;
    for message in messages {
        if message.get("role").and_then(serde_json::Value::as_str) != Some("user") {
            continue;
        }
        let content = message.get("content")?;
        let text = if let Some(text) = content.as_str() {
            text.to_string()
        } else if let Some(blocks) = content.as_array() {
            blocks
                .iter()
                .filter_map(|block| {
                    let kind = block.get("type").and_then(serde_json::Value::as_str);
                    if matches!(kind, Some("text") | Some("context")) {
                        block.get("text").and_then(serde_json::Value::as_str)
                    } else {
                        None
                    }
                })
                .collect::<Vec<_>>()
                .join(" ")
        } else {
            String::new()
        };
        let text = text.split_whitespace().collect::<Vec<_>>().join(" ");
        if !text.is_empty() {
            return Some(preview(&text, 48));
        }
    }
    None
}
