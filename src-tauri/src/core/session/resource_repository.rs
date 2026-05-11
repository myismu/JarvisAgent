//! SQLite-backed storage for session-scoped binary/text resources.

use rusqlite::{params, OptionalExtension};
use std::path::Path;

use crate::infra::types::models::Task;

fn now_ts() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
}

fn basename(value: &str) -> String {
    Path::new(value)
        .file_name()
        .map(|name| name.to_string_lossy().to_string())
        .filter(|name| !name.is_empty())
        .unwrap_or_else(|| value.to_string())
}

fn ensure_session_record(session_id: &str) -> Result<(), String> {
    crate::core::session::repository::ensure_session_exists(
        session_id,
        Some("Session resources"),
        now_ts(),
    )
}

pub fn save_attachment(
    session_id: &str,
    filename: &str,
    media_type: &str,
    data: &[u8],
) -> Result<(), String> {
    ensure_session_record(session_id)?;
    crate::infra::db::with_connection(|conn| {
        conn.execute(
            "INSERT INTO session_attachments(filename, session_id, media_type, data, created_at)
             VALUES(?1, ?2, ?3, ?4, ?5)
             ON CONFLICT(filename) DO UPDATE SET
                session_id = excluded.session_id,
                media_type = excluded.media_type,
                data = excluded.data",
            params![filename, session_id, media_type, data, now_ts() as i64],
        )
        .map_err(|e| e.to_string())?;
        Ok(())
    })
}

pub fn load_attachment(filename: &str) -> Result<Option<(String, Vec<u8>)>, String> {
    let filename = basename(filename);
    crate::infra::db::with_connection(|conn| {
        conn.query_row(
            "SELECT media_type, data FROM session_attachments WHERE filename = ?1",
            [filename],
            |row| Ok((row.get::<_, String>(0)?, row.get::<_, Vec<u8>>(1)?)),
        )
        .optional()
        .map_err(|e| e.to_string())
    })
}

pub fn delete_attachment(filename: &str) -> Result<(), String> {
    let filename = basename(filename);
    crate::infra::db::with_connection(|conn| {
        conn.execute(
            "DELETE FROM session_attachments WHERE filename = ?1",
            [filename],
        )
        .map_err(|e| e.to_string())?;
        Ok(())
    })
}

pub fn save_transcript(
    session_id: &str,
    filename: &str,
    content: &str,
    created_at: u64,
) -> Result<String, String> {
    ensure_session_record(session_id)?;
    let id = format!("transcript_{}_{}", session_id, created_at);
    crate::infra::db::with_connection(|conn| {
        conn.execute(
            "INSERT INTO session_transcripts(id, session_id, filename, content, created_at)
             VALUES(?1, ?2, ?3, ?4, ?5)
             ON CONFLICT(session_id, filename) DO UPDATE SET
                content = excluded.content,
                created_at = excluded.created_at",
            params![id, session_id, filename, content, created_at as i64],
        )
        .map_err(|e| e.to_string())?;
        Ok(format!(
            "sqlite://session/{}/transcripts/{}",
            session_id, filename
        ))
    })
}

pub fn save_task(session_id: &str, task: &Task) -> Result<(), String> {
    ensure_session_record(session_id)?;
    let task_json = serde_json::to_string(task).map_err(|e| e.to_string())?;
    crate::infra::db::with_connection(|conn| {
        conn.execute(
            "INSERT INTO session_tasks(session_id, task_id, task_json, updated_at)
             VALUES(?1, ?2, ?3, ?4)
             ON CONFLICT(session_id, task_id) DO UPDATE SET
                task_json = excluded.task_json,
                updated_at = excluded.updated_at",
            params![session_id, task.id as i64, task_json, now_ts() as i64],
        )
        .map_err(|e| e.to_string())?;
        Ok(())
    })
}

/// 原子分配 ID 并保存任务：在同一 DB 连接内完成 max_id 查询 + insert
pub fn save_task_with_auto_id(session_id: &str, task: &Task) -> Result<Task, String> {
    ensure_session_record(session_id)?;
    let mut task = task.clone();
    crate::infra::db::with_connection(|conn| {
        let next_id: i64 = conn
            .query_row(
                "SELECT COALESCE(MAX(task_id), 0) + 1 FROM session_tasks WHERE session_id = ?1",
                [session_id],
                |row| row.get(0),
            )
            .map_err(|e| e.to_string())?;
        task.id = next_id as i32;
        let task_json = serde_json::to_string(&task).map_err(|e| e.to_string())?;
        conn.execute(
            "INSERT INTO session_tasks(session_id, task_id, task_json, updated_at)
             VALUES(?1, ?2, ?3, ?4)",
            rusqlite::params![session_id, next_id, task_json, now_ts() as i64],
        )
        .map_err(|e| e.to_string())?;
        Ok(())
    })?;
    Ok(task)
}

pub fn load_task(session_id: &str, task_id: i32) -> Result<Option<Task>, String> {
    crate::infra::db::with_connection(|conn| {
        conn.query_row(
            "SELECT task_json FROM session_tasks WHERE session_id = ?1 AND task_id = ?2",
            params![session_id, task_id as i64],
            |row| row.get::<_, String>(0),
        )
        .optional()
        .map_err(|e| e.to_string())?
        .map(|json| serde_json::from_str(&json).map_err(|e| e.to_string()))
        .transpose()
    })
}

pub fn list_tasks(session_id: &str) -> Result<Vec<Task>, String> {
    crate::infra::db::with_connection(|conn| {
        let mut stmt = conn
            .prepare("SELECT task_json FROM session_tasks WHERE session_id = ?1 ORDER BY task_id")
            .map_err(|e| e.to_string())?;
        let rows = stmt
            .query_map([session_id], |row| row.get::<_, String>(0))
            .map_err(|e| e.to_string())?;

        let mut tasks = Vec::new();
        for row in rows {
            let json = row.map_err(|e| e.to_string())?;
            tasks.push(serde_json::from_str(&json).map_err(|e| e.to_string())?);
        }
        Ok(tasks)
    })
}

pub fn delete_task(session_id: &str, task_id: i32) -> Result<(), String> {
    crate::infra::db::with_connection(|conn| {
        conn.execute(
            "DELETE FROM session_tasks WHERE session_id = ?1 AND task_id = ?2",
            params![session_id, task_id as i64],
        )
        .map_err(|e| e.to_string())?;
        Ok(())
    })
}

pub fn max_task_id(session_id: &str) -> Result<i32, String> {
    crate::infra::db::with_connection(|conn| {
        conn.query_row(
            "SELECT COALESCE(MAX(task_id), 0) FROM session_tasks WHERE session_id = ?1",
            [session_id],
            |row| row.get::<_, i64>(0),
        )
        .map(|value| value as i32)
        .map_err(|e| e.to_string())
    })
}
