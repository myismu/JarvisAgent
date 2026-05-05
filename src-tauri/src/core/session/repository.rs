//! # repository.rs — 会话 SQLite 仓储
//!
//! 封装会话元数据、完整记忆、消息展开索引和列表筛选的 SQLite 读写。
//!
//! ## Key Exports
//! - `SessionListFilter`: 会话列表筛选条件
//! - `upsert_session()`: 保存会话元数据和记忆
//! - `load_session()`: 读取完整会话记忆
//! - `list_sessions()`: 查询会话列表并支持筛选
//! - `upsert_context_snapshot()`: 保存最近一次上下文 token 快照
//! - `set_last_active_session_id()`: 持久化最后活跃会话
//!
//! ## Dependencies
//! - Internal: `crate::core::db`, `crate::core::models`
//! - External: `rusqlite`, `serde`

use rusqlite::{params, OptionalExtension, Row};
use serde::{Deserialize, Serialize};

use crate::core::models::{Message, SessionContextSnapshot, SessionMemory};
use crate::core::session::SessionMeta;

#[derive(Debug, Clone)]
pub struct StoredSessionMessage {
    pub message_id: String,
    pub seq: usize,
    pub role: String,
    pub content: Message,
    pub created_at: u64,
    pub updated_at: Option<u64>,
    pub recalled_at: Option<u64>,
    pub hidden_at: Option<u64>,
    pub source: String,
    pub turn_id: Option<String>,
}

#[derive(Serialize, Deserialize, Debug, Clone, Default)]
#[serde(rename_all = "camelCase")]
pub struct SessionListFilter {
    #[serde(default)]
    pub keyword: Option<String>,
    #[serde(default)]
    pub from_ts: Option<u64>,
    #[serde(default)]
    pub to_ts: Option<u64>,
    #[serde(default)]
    pub profile_id: Option<String>,
    #[serde(default)]
    pub model: Option<String>,
    #[serde(default)]
    pub tool: Option<String>,
    #[serde(default)]
    pub has_tool_calls: Option<bool>,
    #[serde(default)]
    pub limit: Option<usize>,
    #[serde(default)]
    pub offset: Option<usize>,
}

pub fn upsert_session(meta: &SessionMeta, memory: &SessionMemory) -> Result<(), String> {
    crate::core::db::with_transaction(|tx| {
        tx.execute(
            "INSERT INTO sessions(
                id, title, created_at, updated_at, message_count, is_smart_named,
                profile_id, total_input_tokens, total_output_tokens, title_source, working_directory, deleted_at
            ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, NULL)
            ON CONFLICT(id) DO UPDATE SET
                title = excluded.title,
                created_at = excluded.created_at,
                updated_at = excluded.updated_at,
                message_count = excluded.message_count,
                is_smart_named = excluded.is_smart_named,
                profile_id = excluded.profile_id,
                total_input_tokens = excluded.total_input_tokens,
                total_output_tokens = excluded.total_output_tokens,
                title_source = excluded.title_source,
                working_directory = excluded.working_directory,
                deleted_at = NULL",
            params![
                meta.id,
                meta.title,
                meta.created_at as i64,
                meta.updated_at as i64,
                meta.message_count as i64,
                if meta.is_smart_named { 1 } else { 0 },
                meta.profile_id,
                meta.total_input_tokens as i64,
                meta.total_output_tokens as i64,
                meta.title_source,
                meta.working_directory,
            ],
        )
        .map_err(|e| e.to_string())?;

        let memory_json = serde_json::to_string(memory).map_err(|e| e.to_string())?;
        tx.execute(
            "INSERT INTO session_memory(session_id, memory_json) VALUES(?1, ?2)
             ON CONFLICT(session_id) DO UPDATE SET memory_json = excluded.memory_json",
            params![meta.id, memory_json],
        )
        .map_err(|e| e.to_string())?;

        Ok(())
    })
}

pub fn append_or_upsert_session_messages(
    session_id: &str,
    messages: &[Message],
    message_ids: &[String],
    source: &str,
    now: u64,
) -> Result<(), String> {
    crate::core::db::with_transaction(|tx| {
        let mut next_seq: i64 = tx
            .query_row(
                "SELECT COALESCE(MAX(seq), -1) + 1 FROM session_messages WHERE session_id = ?1",
                [session_id],
                |row| row.get(0),
            )
            .map_err(|e| e.to_string())?;

        let mut existing_by_content = tx
            .prepare(
                "SELECT message_id, seq, role, content_json
                 FROM session_messages
                 WHERE session_id = ?1 AND hidden_at IS NULL AND recalled_at IS NULL",
            )
            .map_err(|e| e.to_string())?
            .query_map([session_id], |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, i64>(1)?,
                    row.get::<_, String>(2)?,
                    row.get::<_, String>(3)?,
                ))
            })
            .map_err(|e| e.to_string())?
            .collect::<Result<Vec<_>, _>>()
            .map_err(|e| e.to_string())?;

        for (idx, message) in messages.iter().enumerate() {
            let Some(message_id) = message_ids.get(idx).filter(|id| !id.trim().is_empty()) else {
                continue;
            };
            let role = match message {
                Message::User { .. } => "user",
                Message::Assistant { .. } => "assistant",
            };
            let content_json = serde_json::to_string(message).map_err(|e| e.to_string())?;
            let existing_seq: Option<i64> = tx
                .query_row(
                    "SELECT seq FROM session_messages WHERE session_id = ?1 AND message_id = ?2",
                    params![session_id, message_id],
                    |row| row.get(0),
                )
                .optional()
                .map_err(|e| e.to_string())?;
            let seq = if let Some(seq) = existing_seq {
                seq
            } else if let Some(position) = existing_by_content.iter().position(
                |(existing_message_id, _, existing_role, existing_content_json)| {
                    existing_message_id != message_id
                        && existing_role == role
                        && existing_content_json == &content_json
                },
            ) {
                let (_, seq, _, _) = existing_by_content.remove(position);
                tx.execute(
                    "UPDATE session_messages
                     SET message_id = ?3, updated_at = ?4, source = ?5
                     WHERE session_id = ?1 AND seq = ?2",
                    params![session_id, seq, message_id, now as i64, source],
                )
                .map_err(|e| e.to_string())?;
                seq
            } else {
                let seq = next_seq;
                next_seq += 1;
                seq
            };

            tx.execute(
                "INSERT INTO session_messages(
                    session_id, message_id, seq, role, content_json, created_at, updated_at, source
                 ) VALUES(?1, ?2, ?3, ?4, ?5, ?6, ?6, ?7)
                 ON CONFLICT(session_id, message_id) DO UPDATE SET
                    role = excluded.role,
                    content_json = excluded.content_json,
                    updated_at = excluded.updated_at,
                    source = excluded.source",
                params![
                    session_id,
                    message_id,
                    seq,
                    role,
                    content_json,
                    now as i64,
                    source,
                ],
            )
            .map_err(|e| e.to_string())?;
        }

        Ok(())
    })
}

pub fn list_visible_session_messages(session_id: &str) -> Result<Vec<StoredSessionMessage>, String> {
    crate::core::db::with_connection(|conn| {
        let mut stmt = conn
            .prepare(
                "SELECT message_id, seq, role, content_json, created_at, updated_at, recalled_at,
                        hidden_at, source, turn_id
                 FROM session_messages
                 WHERE session_id = ?1
                   AND hidden_at IS NULL
                   AND recalled_at IS NULL
                   AND source != 'compact'
                 ORDER BY seq ASC",
            )
            .map_err(|e| e.to_string())?;
        let rows = stmt
            .query_map([session_id], stored_session_message_from_row)
            .map_err(|e| e.to_string())?;
        let mut messages = Vec::new();
        for row in rows {
            messages.push(row.map_err(|e| e.to_string())?);
        }
        Ok(messages)
    })
}

pub fn find_session_message_by_id(
    session_id: &str,
    message_id: &str,
) -> Result<Option<StoredSessionMessage>, String> {
    crate::core::db::with_connection(|conn| {
        conn.query_row(
            "SELECT message_id, seq, role, content_json, created_at, updated_at, recalled_at,
                    hidden_at, source, turn_id
             FROM session_messages
             WHERE session_id = ?1 AND message_id = ?2",
            params![session_id, message_id],
            stored_session_message_from_row,
        )
        .optional()
        .map_err(|e| e.to_string())
    })
}

pub fn hide_session_messages_from_seq(
    session_id: &str,
    seq: usize,
    recalled: bool,
) -> Result<(), String> {
    crate::core::db::with_connection(|conn| {
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs();
        if recalled {
            conn.execute(
                "UPDATE session_messages
                 SET hidden_at = COALESCE(hidden_at, ?3), recalled_at = COALESCE(recalled_at, ?3)
                 WHERE session_id = ?1 AND seq >= ?2",
                params![session_id, seq as i64, now as i64],
            )
        } else {
            conn.execute(
                "UPDATE session_messages
                 SET hidden_at = COALESCE(hidden_at, ?3)
                 WHERE session_id = ?1 AND seq >= ?2",
                params![session_id, seq as i64, now as i64],
            )
        }
        .map_err(|e| e.to_string())?;
        Ok(())
    })
}

pub fn delete_session_messages_from_seq(session_id: &str, seq: usize) -> Result<(), String> {
    crate::core::db::with_connection(|conn| {
        conn.execute(
            "DELETE FROM session_messages WHERE session_id = ?1 AND seq >= ?2",
            params![session_id, seq as i64],
        )
        .map_err(|e| e.to_string())?;
        Ok(())
    })
}

pub fn session_messages_count(session_id: &str) -> Result<usize, String> {
    crate::core::db::with_connection(|conn| {
        let count: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM session_messages WHERE session_id = ?1",
                [session_id],
                |row| row.get(0),
            )
            .map_err(|e| e.to_string())?;
        Ok(count.max(0) as usize)
    })
}

fn stored_session_message_from_row(row: &Row<'_>) -> rusqlite::Result<StoredSessionMessage> {
    let seq: i64 = row.get(1)?;
    let content_json: String = row.get(3)?;
    let created_at: i64 = row.get(4)?;
    let updated_at: Option<i64> = row.get(5)?;
    let recalled_at: Option<i64> = row.get(6)?;
    let hidden_at: Option<i64> = row.get(7)?;
    let content = serde_json::from_str(&content_json).map_err(|err| {
        rusqlite::Error::FromSqlConversionFailure(3, rusqlite::types::Type::Text, Box::new(err))
    })?;
    Ok(StoredSessionMessage {
        message_id: row.get(0)?,
        seq: seq.max(0) as usize,
        role: row.get(2)?,
        content,
        created_at: created_at.max(0) as u64,
        updated_at: updated_at.map(|value| value.max(0) as u64),
        recalled_at: recalled_at.map(|value| value.max(0) as u64),
        hidden_at: hidden_at.map(|value| value.max(0) as u64),
        source: row.get(8)?,
        turn_id: row.get(9)?,
    })
}

pub fn load_session(id: &str) -> Result<SessionMemory, String> {
    crate::core::db::with_connection(|conn| {
        conn.query_row(
            "SELECT memory_json FROM session_memory
             JOIN sessions ON sessions.id = session_memory.session_id
             WHERE session_id = ?1 AND sessions.deleted_at IS NULL",
            [id],
            |row| row.get::<_, String>(0),
        )
        .optional()
        .map_err(|e| e.to_string())?
        .ok_or_else(|| format!("会话 {} 不存在", id))
        .and_then(|json| serde_json::from_str(&json).map_err(|e| e.to_string()))
    })
}

pub fn upsert_context_snapshot(snapshot: &SessionContextSnapshot) -> Result<(), String> {
    crate::core::db::with_connection(|conn| {
        let snapshot_json = serde_json::to_string(snapshot).map_err(|e| e.to_string())?;
        conn.execute(
            "INSERT INTO session_context_snapshots(session_id, snapshot_json, updated_at)
             VALUES(?1, ?2, ?3)
             ON CONFLICT(session_id) DO UPDATE SET
                snapshot_json = excluded.snapshot_json,
                updated_at = excluded.updated_at",
            params![
                snapshot.session_id,
                snapshot_json,
                snapshot.created_at as i64
            ],
        )
        .map_err(|e| e.to_string())?;
        Ok(())
    })
}

pub fn update_context_snapshot_usage(
    session_id: &str,
    provider_input_tokens: u64,
    provider_output_tokens: u64,
    provider_total_tokens: u64,
    drift_percent: Option<f32>,
) -> Result<Option<SessionContextSnapshot>, String> {
    crate::core::db::with_connection(|conn| {
        let snapshot_json = conn
            .query_row(
                "SELECT snapshot_json FROM session_context_snapshots
                 JOIN sessions ON sessions.id = session_context_snapshots.session_id
                 WHERE session_context_snapshots.session_id = ?1 AND sessions.deleted_at IS NULL",
                [session_id],
                |row| row.get::<_, String>(0),
            )
            .optional()
            .map_err(|e| e.to_string())?;

        let Some(snapshot_json) = snapshot_json else {
            return Ok(None);
        };

        let mut snapshot: SessionContextSnapshot =
            serde_json::from_str(&snapshot_json).map_err(|e| e.to_string())?;
        snapshot.provider_input_tokens = Some(provider_input_tokens);
        snapshot.provider_output_tokens = Some(provider_output_tokens);
        snapshot.provider_total_tokens = Some(provider_total_tokens);
        snapshot.drift_percent = drift_percent;

        let snapshot_json = serde_json::to_string(&snapshot).map_err(|e| e.to_string())?;
        conn.execute(
            "UPDATE session_context_snapshots
             SET snapshot_json = ?2, updated_at = ?3
             WHERE session_id = ?1",
            params![session_id, snapshot_json, snapshot.created_at as i64],
        )
        .map_err(|e| e.to_string())?;

        Ok(Some(snapshot))
    })
}

pub fn get_context_snapshot(session_id: &str) -> Result<Option<SessionContextSnapshot>, String> {
    crate::core::db::with_connection(|conn| {
        conn.query_row(
            "SELECT snapshot_json FROM session_context_snapshots
             JOIN sessions ON sessions.id = session_context_snapshots.session_id
             WHERE session_context_snapshots.session_id = ?1 AND sessions.deleted_at IS NULL",
            [session_id],
            |row| row.get::<_, String>(0),
        )
        .optional()
        .map_err(|e| e.to_string())?
        .map(|json| serde_json::from_str(&json).map_err(|e| e.to_string()))
        .transpose()
    })
}

pub fn get_session_meta(id: &str) -> Result<SessionMeta, String> {
    crate::core::db::with_connection(|conn| {
        conn.query_row(
            "SELECT id, title, created_at, updated_at, message_count, is_smart_named,
                    profile_id, total_input_tokens, total_output_tokens, title_source, working_directory
             FROM sessions WHERE id = ?1 AND deleted_at IS NULL",
            [id],
            session_meta_from_row,
        )
        .optional()
        .map_err(|e| e.to_string())?
        .ok_or_else(|| format!("会话 {} 不存在", id))
    })
}

pub fn list_sessions(filter: Option<&SessionListFilter>) -> Result<Vec<SessionMeta>, String> {
    crate::core::db::with_connection(|conn| {
        let mut sessions = Vec::new();
        let mut stmt = conn
            .prepare(
                "SELECT id, title, created_at, updated_at, message_count, is_smart_named,
                        profile_id, total_input_tokens, total_output_tokens, title_source, working_directory
                 FROM sessions
                 WHERE deleted_at IS NULL
                 ORDER BY updated_at DESC",
            )
            .map_err(|e| e.to_string())?;
        let rows = stmt
            .query_map([], session_meta_from_row)
            .map_err(|e| e.to_string())?;
        for row in rows {
            let meta = row.map_err(|e| e.to_string())?;
            if matches_filter(conn, &meta, filter)? {
                sessions.push(meta);
            }
        }

        if let Some(filter) = filter {
            let offset = filter.offset.unwrap_or(0);
            let limit = filter.limit.unwrap_or(sessions.len());
            sessions = sessions.into_iter().skip(offset).take(limit).collect();
        }
        Ok(sessions)
    })
}

pub fn ensure_session_exists(id: &str, title: Option<&str>, created_at: u64) -> Result<(), String> {
    crate::core::db::with_connection(|conn| {
        let existing: Option<String> = conn
            .query_row(
                "SELECT id FROM sessions WHERE id = ?1 AND deleted_at IS NULL",
                [id],
                |row| row.get(0),
            )
            .optional()
            .map_err(|e| e.to_string())?;
        if existing.is_some() {
            return Ok(());
        }

        let title = title.unwrap_or("导入会话");
        conn.execute(
            "INSERT INTO sessions(
                id, title, created_at, updated_at, message_count, is_smart_named,
                profile_id, total_input_tokens, total_output_tokens, title_source, working_directory, deleted_at
            ) VALUES(?1, ?2, ?3, ?3, 0, 0, NULL, 0, 0, 'default', NULL, NULL)",
            params![id, title, created_at as i64],
        )
        .map_err(|e| e.to_string())?;

        let memory_json =
            serde_json::to_string(&SessionMemory::default()).map_err(|e| e.to_string())?;
        conn.execute(
            "INSERT INTO session_memory(session_id, memory_json) VALUES(?1, ?2)
             ON CONFLICT(session_id) DO NOTHING",
            params![id, memory_json],
        )
        .map_err(|e| e.to_string())?;

        Ok(())
    })
}

pub fn delete_session(id: &str) -> Result<(), String> {
    crate::core::db::with_connection(|conn| {
        let changed = conn
            .execute("DELETE FROM sessions WHERE id = ?1", [id])
            .map_err(|e| e.to_string())?;
        if changed == 0 {
            return Err(format!("会话 {} 不存在", id));
        }
        Ok(())
    })
}

pub fn rename_session(
    id: &str,
    title: &str,
    is_smart_named: bool,
    title_source: &str,
) -> Result<SessionMeta, String> {
    crate::core::db::with_connection(|conn| {
        let changed = conn
            .execute(
                "UPDATE sessions SET title = ?2, is_smart_named = ?3, title_source = ?4 WHERE id = ?1 AND deleted_at IS NULL",
                params![id, title, if is_smart_named { 1 } else { 0 }, title_source],
            )
            .map_err(|e| e.to_string())?;
        if changed == 0 {
            return Err(format!("会话 {} 不存在", id));
        }
        conn.query_row(
            "SELECT id, title, created_at, updated_at, message_count, is_smart_named,
                    profile_id, total_input_tokens, total_output_tokens, title_source, working_directory
             FROM sessions WHERE id = ?1 AND deleted_at IS NULL",
            [id],
            session_meta_from_row,
        )
        .map_err(|e| e.to_string())
    })
}

pub fn update_session_profile(id: &str, profile_id: &str) -> Result<(), String> {
    crate::core::db::with_connection(|conn| {
        let changed = conn
            .execute(
                "UPDATE sessions SET profile_id = ?2 WHERE id = ?1 AND deleted_at IS NULL",
                params![id, profile_id],
            )
            .map_err(|e| e.to_string())?;
        if changed == 0 {
            return Err(format!("会话 {} 不存在", id));
        }
        Ok(())
    })
}

pub fn get_last_active_session_id() -> Option<String> {
    crate::core::db::with_connection(|conn| {
        conn.query_row(
            "SELECT value FROM app_state WHERE key = 'last_active_session_id'",
            [],
            |row| row.get::<_, String>(0),
        )
        .optional()
        .map_err(|e| e.to_string())
    })
    .ok()
    .flatten()
}

pub fn set_last_active_session_id(id: &str) -> Result<(), String> {
    crate::core::db::with_connection(|conn| {
        conn.execute(
            "INSERT INTO app_state(key, value) VALUES('last_active_session_id', ?1)
             ON CONFLICT(key) DO UPDATE SET value = excluded.value",
            [id],
        )
        .map_err(|e| e.to_string())?;
        Ok(())
    })
}

fn session_meta_from_row(row: &Row<'_>) -> rusqlite::Result<SessionMeta> {
    Ok(SessionMeta {
        id: row.get(0)?,
        title: row.get(1)?,
        created_at: row.get::<_, i64>(2)? as u64,
        updated_at: row.get::<_, i64>(3)? as u64,
        message_count: row.get::<_, i64>(4)? as usize,
        is_smart_named: row.get::<_, i64>(5)? != 0,
        profile_id: row.get(6)?,
        total_input_tokens: row.get::<_, i64>(7)? as u64,
        total_output_tokens: row.get::<_, i64>(8)? as u64,
        title_source: row.get(9)?,
        working_directory: row.get(10)?,
    })
}

fn matches_filter(
    conn: &rusqlite::Connection,
    meta: &SessionMeta,
    filter: Option<&SessionListFilter>,
) -> Result<bool, String> {
    let Some(filter) = filter else {
        return Ok(true);
    };

    if let Some(keyword) = filter
        .keyword
        .as_ref()
        .map(|value| value.trim())
        .filter(|value| !value.is_empty())
    {
        let keyword = keyword.to_lowercase();
        if !meta.title.to_lowercase().contains(&keyword)
            && !meta.id.to_lowercase().contains(&keyword)
        {
            return Ok(false);
        }
    }
    if let Some(from_ts) = filter.from_ts {
        if meta.updated_at < from_ts {
            return Ok(false);
        }
    }
    if let Some(to_ts) = filter.to_ts {
        if meta.updated_at > to_ts {
            return Ok(false);
        }
    }
    if let Some(profile_id) = filter.profile_id.as_ref().filter(|value| !value.is_empty()) {
        if meta.profile_id.as_deref() != Some(profile_id.as_str()) {
            return Ok(false);
        }
    }
    if let Some(tool) = filter.tool.as_ref().filter(|value| !value.is_empty()) {
        let count: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM agent_run_events WHERE session_id = ?1 AND tool = ?2",
                params![meta.id, tool],
                |row| row.get(0),
            )
            .map_err(|e| e.to_string())?;
        if count == 0 {
            return Ok(false);
        }
    }
    if let Some(has_tool_calls) = filter.has_tool_calls {
        let count: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM agent_run_events WHERE session_id = ?1 AND tool IS NOT NULL",
                [meta.id.as_str()],
                |row| row.get(0),
            )
            .map_err(|e| e.to_string())?;
        if has_tool_calls != (count > 0) {
            return Ok(false);
        }
    }
    if let Some(model) = filter.model.as_ref().filter(|value| !value.is_empty()) {
        let count: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM agent_run_events WHERE session_id = ?1 AND model = ?2",
                params![meta.id, model],
                |row| row.get(0),
            )
            .map_err(|e| e.to_string())?;
        if count == 0 {
            return Ok(false);
        }
    }

    Ok(true)
}
