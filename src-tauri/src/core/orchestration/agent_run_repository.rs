//! # repository.rs — Agent Run SQLite 仓储
//!
//! 持久化主 Agent 执行记录、事件流和可恢复检查点，替代 run.json/events.jsonl/checkpoint.json。
//!
//! ## Key Exports
//! - `upsert_run()`: 保存运行记录
//! - `list_runs()`: 查询运行记录
//! - `append_event()`: 追加运行事件
//! - `upsert_checkpoint()`: 保存 resume 检查点
//! - `load_checkpoint()`: 加载 resume 检查点
//!
//! ## Dependencies
//! - Internal: `crate::core::db`, `crate::core::models`
//! - External: `rusqlite`, `serde_json`

use rusqlite::{params, OptionalExtension, Row};

use crate::core::models::Message;
use crate::core::orchestration::agent_runs::{
    AgentRun, AgentRunCheckpoint, AgentRunEvent, AgentRunStatus,
};

pub fn upsert_run(run: &AgentRun) -> Result<(), String> {
    crate::core::db::with_connection(|conn| {
        conn.execute(
            "INSERT INTO agent_runs(
                run_id, session_id, status, user_message_preview, loop_count, input_tokens, output_tokens,
                started_at, updated_at, finished_at, last_safe_point, live_thinking, live_tool_buffer,
                live_content, error, summary, resumable, resumed_from_run_id
            ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15, ?16, ?17, ?18)
            ON CONFLICT(run_id) DO UPDATE SET
                session_id = excluded.session_id,
                status = excluded.status,
                user_message_preview = excluded.user_message_preview,
                loop_count = excluded.loop_count,
                input_tokens = excluded.input_tokens,
                output_tokens = excluded.output_tokens,
                started_at = excluded.started_at,
                updated_at = excluded.updated_at,
                finished_at = excluded.finished_at,
                last_safe_point = excluded.last_safe_point,
                live_thinking = excluded.live_thinking,
                live_tool_buffer = excluded.live_tool_buffer,
                live_content = excluded.live_content,
                error = excluded.error,
                summary = excluded.summary,
                resumable = excluded.resumable,
                resumed_from_run_id = excluded.resumed_from_run_id",
            params![
                run.run_id,
                run.session_id,
                status_to_str(&run.status),
                run.user_message_preview,
                run.loop_count as i64,
                run.input_tokens as i64,
                run.output_tokens as i64,
                run.started_at as i64,
                run.updated_at as i64,
                run.finished_at.map(|v| v as i64),
                run.last_safe_point,
                run.live_thinking,
                run.live_tool_buffer,
                run.live_content,
                run.error,
                run.summary,
                if run.resumable { 1 } else { 0 },
                run.resumed_from_run_id,
            ],
        )
        .map_err(|e| e.to_string())?;
        Ok(())
    })
}

pub fn list_runs(session_id: Option<&str>) -> Result<Vec<AgentRun>, String> {
    crate::core::db::with_connection(|conn| {
        let sql = if session_id.is_some() {
            "SELECT run_id, session_id, status, user_message_preview, loop_count, input_tokens, output_tokens,
                    started_at, updated_at, finished_at, last_safe_point, live_thinking, live_tool_buffer,
                    live_content, error, summary, resumable, resumed_from_run_id
             FROM agent_runs WHERE session_id = ?1 ORDER BY started_at"
        } else {
            "SELECT run_id, session_id, status, user_message_preview, loop_count, input_tokens, output_tokens,
                    started_at, updated_at, finished_at, last_safe_point, live_thinking, live_tool_buffer,
                    live_content, error, summary, resumable, resumed_from_run_id
             FROM agent_runs ORDER BY started_at"
        };
        let mut stmt = conn.prepare(sql).map_err(|e| e.to_string())?;
        let rows = if let Some(session_id) = session_id {
            stmt.query_map([session_id], run_from_row)
                .map_err(|e| e.to_string())?
                .collect::<Result<Vec<_>, _>>()
                .map_err(|e| e.to_string())?
        } else {
            stmt.query_map([], run_from_row)
                .map_err(|e| e.to_string())?
                .collect::<Result<Vec<_>, _>>()
                .map_err(|e| e.to_string())?
        };
        Ok(rows)
    })
}

pub fn load_run(run_id: &str) -> Result<Option<AgentRun>, String> {
    crate::core::db::with_connection(|conn| {
        conn.query_row(
            "SELECT run_id, session_id, status, user_message_preview, loop_count, input_tokens, output_tokens,
                    started_at, updated_at, finished_at, last_safe_point, live_thinking, live_tool_buffer,
                    live_content, error, summary, resumable, resumed_from_run_id
             FROM agent_runs WHERE run_id = ?1",
            [run_id],
            run_from_row,
        )
        .optional()
        .map_err(|e| e.to_string())
    })
}

pub fn append_event(event: &AgentRunEvent) -> Result<(), String> {
    crate::core::db::with_connection(|conn| {
        conn.execute(
            "INSERT INTO agent_run_events(
                event_id, run_id, session_id, event_type, message, tool, input_summary,
                output_summary, error, loop_count, input_tokens, output_tokens, timestamp, model
            ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, NULL)
            ON CONFLICT(event_id) DO NOTHING",
            params![
                event.event_id,
                event.run_id,
                event.session_id,
                event.event_type,
                event.message,
                event.tool,
                event.input_summary,
                event.output_summary,
                event.error,
                event.loop_count as i64,
                event.input_tokens as i64,
                event.output_tokens as i64,
                event.timestamp as i64,
            ],
        )
        .map_err(|e| e.to_string())?;
        Ok(())
    })
}

pub fn list_events(
    session_id: Option<&str>,
    run_id: Option<&str>,
) -> Result<Vec<AgentRunEvent>, String> {
    crate::core::db::with_connection(|conn| {
        let mut events = Vec::new();
        let mut stmt = conn
            .prepare(
                "SELECT event_id, run_id, session_id, event_type, message, tool, input_summary,
                        output_summary, error, loop_count, input_tokens, output_tokens, timestamp
                 FROM agent_run_events ORDER BY timestamp",
            )
            .map_err(|e| e.to_string())?;
        let rows = stmt
            .query_map([], event_from_row)
            .map_err(|e| e.to_string())?;
        for row in rows {
            let event = row.map_err(|e| e.to_string())?;
            if session_id.map_or(true, |target| event.session_id == target)
                && run_id.map_or(true, |target| event.run_id == target)
            {
                events.push(event);
            }
        }
        Ok(events)
    })
}

pub fn upsert_checkpoint(checkpoint: &AgentRunCheckpoint) -> Result<(), String> {
    crate::core::db::with_connection(|conn| {
        let messages_json =
            serde_json::to_string(&checkpoint.messages).map_err(|e| e.to_string())?;
        conn.execute(
            "INSERT INTO agent_run_checkpoints(
                run_id, session_id, loop_count, messages_json, input_tokens, output_tokens, last_safe_point, updated_at
            ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)
            ON CONFLICT(run_id) DO UPDATE SET
                session_id = excluded.session_id,
                loop_count = excluded.loop_count,
                messages_json = excluded.messages_json,
                input_tokens = excluded.input_tokens,
                output_tokens = excluded.output_tokens,
                last_safe_point = excluded.last_safe_point,
                updated_at = excluded.updated_at",
            params![
                checkpoint.run_id,
                checkpoint.session_id,
                checkpoint.loop_count as i64,
                messages_json,
                checkpoint.input_tokens as i64,
                checkpoint.output_tokens as i64,
                checkpoint.last_safe_point,
                checkpoint.updated_at as i64,
            ],
        )
        .map_err(|e| e.to_string())?;
        Ok(())
    })
}

pub fn load_checkpoint(run_id: &str) -> Result<Option<AgentRunCheckpoint>, String> {
    crate::core::db::with_connection(|conn| {
        conn.query_row(
            "SELECT run_id, session_id, loop_count, messages_json, input_tokens, output_tokens, last_safe_point, updated_at
             FROM agent_run_checkpoints WHERE run_id = ?1",
            [run_id],
            |row| {
                let messages_json: String = row.get(3)?;
                let messages: Vec<Message> = serde_json::from_str(&messages_json).map_err(|e| {
                    rusqlite::Error::FromSqlConversionFailure(
                        3,
                        rusqlite::types::Type::Text,
                        Box::new(e),
                    )
                })?;
                Ok(AgentRunCheckpoint {
                    run_id: row.get(0)?,
                    session_id: row.get(1)?,
                    loop_count: row.get::<_, i64>(2)? as usize,
                    messages,
                    input_tokens: row.get::<_, i64>(4)? as u64,
                    output_tokens: row.get::<_, i64>(5)? as u64,
                    last_safe_point: row.get(6)?,
                    updated_at: row.get::<_, i64>(7)? as u64,
                })
            },
        )
        .optional()
        .map_err(|e| e.to_string())
    })
}

fn run_from_row(row: &Row<'_>) -> rusqlite::Result<AgentRun> {
    Ok(AgentRun {
        run_id: row.get(0)?,
        session_id: row.get(1)?,
        status: status_from_str(row.get::<_, String>(2)?.as_str()),
        user_message_preview: row.get(3)?,
        loop_count: row.get::<_, i64>(4)? as usize,
        input_tokens: row.get::<_, i64>(5)? as u64,
        output_tokens: row.get::<_, i64>(6)? as u64,
        started_at: row.get::<_, i64>(7)? as u64,
        updated_at: row.get::<_, i64>(8)? as u64,
        finished_at: row.get::<_, Option<i64>>(9)?.map(|value| value as u64),
        last_safe_point: row.get(10)?,
        live_thinking: row.get(11)?,
        live_tool_buffer: row.get(12)?,
        live_content: row.get(13)?,
        error: row.get(14)?,
        summary: row.get(15)?,
        resumable: row.get::<_, i64>(16)? != 0,
        resumed_from_run_id: row.get(17)?,
    })
}

fn event_from_row(row: &Row<'_>) -> rusqlite::Result<AgentRunEvent> {
    Ok(AgentRunEvent {
        event_id: row.get(0)?,
        run_id: row.get(1)?,
        session_id: row.get(2)?,
        event_type: row.get(3)?,
        message: row.get(4)?,
        tool: row.get(5)?,
        input_summary: row.get(6)?,
        output_summary: row.get(7)?,
        error: row.get(8)?,
        loop_count: row.get::<_, i64>(9)? as usize,
        input_tokens: row.get::<_, i64>(10)? as u64,
        output_tokens: row.get::<_, i64>(11)? as u64,
        timestamp: row.get::<_, i64>(12)? as u64,
    })
}

fn status_to_str(status: &AgentRunStatus) -> &'static str {
    match status {
        AgentRunStatus::Running => "running",
        AgentRunStatus::Completed => "completed",
        AgentRunStatus::Failed => "failed",
        AgentRunStatus::Cancelled => "cancelled",
        AgentRunStatus::Interrupted => "interrupted",
    }
}

fn status_from_str(value: &str) -> AgentRunStatus {
    match value {
        "completed" => AgentRunStatus::Completed,
        "failed" => AgentRunStatus::Failed,
        "cancelled" => AgentRunStatus::Cancelled,
        "interrupted" => AgentRunStatus::Interrupted,
        _ => AgentRunStatus::Running,
    }
}
