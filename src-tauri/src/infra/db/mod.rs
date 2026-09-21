//! # db/mod.rs — SQLite 连接与事务入口
//!
//! 负责初始化本地 SQLite 数据库、设置运行期 PRAGMA，并提供同步短事务访问函数。
//!
//! ## Key Exports
//! - `init()`: 初始化数据库连接和 schema
//! - `with_connection()`: 以互斥连接执行一次数据库操作
//! - `with_transaction()`: 以事务执行一次数据库操作
//! - `is_available()`: 判断数据库是否已完成初始化
//!
//! ## Dependencies
//! - Internal: `crate::infra::config::data_paths`, `crate::infra::db::schema`
//! - External: `rusqlite`
//!
//! ## Constraints
//! - 数据库连接通过全局 Mutex 串行化写入，避免 tokio 多任务并发写锁冲突

pub mod schema;

use rusqlite::{params, Connection, OptionalExtension, Transaction};
use std::sync::{Mutex, OnceLock};

static DB: OnceLock<Mutex<Connection>> = OnceLock::new();

pub fn init() -> Result<(), String> {
    if DB.get().is_some() {
        return Ok(());
    }

    let path = crate::infra::config::data_paths::sqlite_db_path();
    let conn = Connection::open(&path).map_err(|e| e.to_string())?;
    configure_connection(&conn)?;
    schema::init_schema(&conn)?;

    let _ = DB.set(Mutex::new(conn));
    Ok(())
}

pub fn is_available() -> bool {
    DB.get().is_some()
}

pub fn with_connection<T, F>(f: F) -> Result<T, String>
where
    F: FnOnce(&Connection) -> Result<T, String>,
{
    let db = DB
        .get()
        .ok_or_else(|| "SQLite 数据库尚未初始化".to_string())?;
    let conn = db.lock().map_err(|_| "SQLite 数据库锁已损坏".to_string())?;
    f(&conn)
}

pub fn with_transaction<T, F>(f: F) -> Result<T, String>
where
    F: FnOnce(&Transaction<'_>) -> Result<T, String>,
{
    let db = DB
        .get()
        .ok_or_else(|| "SQLite 数据库尚未初始化".to_string())?;
    let mut conn = db.lock().map_err(|_| "SQLite 数据库锁已损坏".to_string())?;
    let tx = conn.transaction().map_err(|e| e.to_string())?;
    let result = f(&tx)?;
    tx.commit().map_err(|e| e.to_string())?;
    Ok(result)
}

#[derive(Debug, Clone)]
pub struct CheckpointUserMessageLink {
    pub user_message_index: usize,
    pub message_id: Option<String>,
    pub checkpoint_id: String,
    pub has_file_edits: bool,
    pub created_at: u64,
    pub updated_at: Option<u64>,
}

#[derive(Debug, Clone)]
pub struct PendingSnapshotPatchRecord {
    pub run_id: String,
    pub seq: usize,
    pub patch: crate::core::rollback::Patch,
    pub message: Option<String>,
    pub trigger_user_memory_index: Option<usize>,
    pub trigger_user_message_id: Option<String>,
    pub created_at: u64,
}

/// 当前时间戳（**毫秒**，Unix epoch）——全项目 DB 时间戳统一毫秒口径（v16 起）。
fn now_ts() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as u64
}

pub fn insert_agent_run_patch(
    session_id: &str,
    run_id: &str,
    seq: usize,
    patch: &crate::core::rollback::Patch,
    message: Option<&str>,
    trigger_user_memory_index: Option<usize>,
    trigger_user_message_id: Option<&str>,
) -> Result<(), String> {
    let patch_json = serde_json::to_string(patch).map_err(|e| e.to_string())?;
    with_connection(|conn| {
        conn.execute(
            "INSERT INTO agent_run_patches(
                session_id, run_id, seq, patch_json, message, trigger_user_memory_index,
                trigger_user_message_id, created_at
             ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)
             ON CONFLICT(session_id, run_id, seq) DO UPDATE SET
                patch_json = excluded.patch_json,
                message = excluded.message,
                trigger_user_memory_index = excluded.trigger_user_memory_index,
                trigger_user_message_id = excluded.trigger_user_message_id,
                created_at = excluded.created_at",
            params![
                session_id,
                run_id,
                seq as i64,
                patch_json,
                message,
                trigger_user_memory_index.map(|value| value as i64),
                trigger_user_message_id,
                now_ts() as i64,
            ],
        )
        .map_err(|e| e.to_string())?;
        Ok(())
    })
}

pub fn list_agent_run_patches(
    session_id: &str,
    run_id: Option<&str>,
) -> Result<Vec<PendingSnapshotPatchRecord>, String> {
    with_connection(|conn| {
        let mut patches = Vec::new();
        if let Some(run_id) = run_id {
            let mut stmt = conn
                .prepare(&format!(
                    "SELECT {} FROM agent_run_patches
                     WHERE session_id = ?1 AND run_id = ?2
                     ORDER BY seq",
                    PATCH_COLUMNS
                ))
                .map_err(|e| e.to_string())?;
            let rows = stmt
                .query_map(params![session_id, run_id], agent_run_patch_from_row)
                .map_err(|e| e.to_string())?;
            for row in rows {
                patches.push(row.map_err(|e| e.to_string())?);
            }
        } else {
            let mut stmt = conn
                .prepare(&format!(
                    "SELECT {} FROM agent_run_patches
                     WHERE session_id = ?1
                     ORDER BY created_at, seq",
                    PATCH_COLUMNS
                ))
                .map_err(|e| e.to_string())?;
            let rows = stmt
                .query_map([session_id], agent_run_patch_from_row)
                .map_err(|e| e.to_string())?;
            for row in rows {
                patches.push(row.map_err(|e| e.to_string())?);
            }
        }
        Ok(patches)
    })
}

/// `agent_run_patches` 的 SELECT 列清单（两处查询共用一份；与 `agent_run_patch_from_row` 读的列名对应）。
const PATCH_COLUMNS: &str = "run_id, seq, patch_json, message, trigger_user_memory_index, \
     trigger_user_message_id, created_at";

/// 从 `agent_run_patches` 的一行还原变更补丁记录。
///
/// **按列名取值**（2026-09-21 起，与 `session/repository.rs` 的几个映射器同一口径）：
/// 以前是下标硬编码，"某条 SELECT 的列清单没跟上"就是运行期 `Invalid column index`
/// —— 撤回功能 2026-09-21 正是这么挂的（`find_session_message_by_id` 漏列）。
fn agent_run_patch_from_row(
    row: &rusqlite::Row<'_>,
) -> rusqlite::Result<PendingSnapshotPatchRecord> {
    let seq: i64 = row.get("seq")?;
    let patch_json: String = row.get("patch_json")?;
    let trigger_index: Option<i64> = row.get("trigger_user_memory_index")?;
    let trigger_user_message_id: Option<String> = row.get("trigger_user_message_id")?;
    let created_at: i64 = row.get("created_at")?;
    let patch = serde_json::from_str(&patch_json).map_err(|err| {
        // 错误上下文里的列号现查一次；取值本身已不依赖列序
        rusqlite::Error::FromSqlConversionFailure(
            row.as_ref().column_index("patch_json").unwrap_or(0),
            rusqlite::types::Type::Text,
            Box::new(err),
        )
    })?;
    Ok(PendingSnapshotPatchRecord {
        run_id: row.get("run_id")?,
        seq: seq.max(0) as usize,
        patch,
        message: row.get("message")?,
        trigger_user_memory_index: trigger_index.map(|value| value.max(0) as usize),
        trigger_user_message_id,
        created_at: created_at.max(0) as u64,
    })
}

pub fn delete_agent_run_patches(
    session_id: &str,
    run_id: Option<&str>,
) -> Result<(), String> {
    with_connection(|conn| {
        if let Some(run_id) = run_id {
            conn.execute(
                "DELETE FROM agent_run_patches WHERE session_id = ?1 AND run_id = ?2",
                params![session_id, run_id],
            )
        } else {
            conn.execute(
                "DELETE FROM agent_run_patches WHERE session_id = ?1",
                params![session_id],
            )
        }
        .map_err(|e| e.to_string())?;
        Ok(())
    })
}

pub fn upsert_checkpoint_user_message_link(
    session_id: &str,
    user_message_index: usize,
    checkpoint_id: &str,
    has_file_edits: bool,
    created_at: u64,
) -> Result<(), String> {
    upsert_checkpoint_user_message_link_v2(
        session_id,
        None,
        user_message_index,
        checkpoint_id,
        has_file_edits,
        created_at,
    )
}

pub fn upsert_checkpoint_user_message_link_v2(
    session_id: &str,
    message_id: Option<&str>,
    user_message_index: usize,
    checkpoint_id: &str,
    has_file_edits: bool,
    created_at: u64,
) -> Result<(), String> {
    with_connection(|conn| {
        conn.execute(
            "INSERT INTO checkpoint_user_message_links(
                session_id, user_message_index, checkpoint_id, has_file_edits, created_at, message_id, updated_at
             ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?5)
             ON CONFLICT(session_id, user_message_index) DO UPDATE SET
                checkpoint_id = excluded.checkpoint_id,
                has_file_edits = excluded.has_file_edits,
                created_at = excluded.created_at,
                message_id = excluded.message_id,
                updated_at = excluded.updated_at",
            params![
                session_id,
                user_message_index as i64,
                checkpoint_id,
                if has_file_edits { 1 } else { 0 },
                created_at as i64,
                message_id,
            ],
        )
        .map_err(|e| e.to_string())?;
        Ok(())
    })
}

pub fn list_checkpoint_user_message_links(
    session_id: &str,
) -> Result<Vec<CheckpointUserMessageLink>, String> {
    with_connection(|conn| {
        let mut stmt = conn
            .prepare(
                "SELECT user_message_index, message_id, checkpoint_id, has_file_edits, created_at, updated_at
                 FROM checkpoint_user_message_links
                 WHERE session_id = ?1",
            )
            .map_err(|e| e.to_string())?;
        let rows = stmt
            .query_map([session_id], |row| {
                let index: i64 = row.get(0)?;
                let has_file_edits: i64 = row.get(3)?;
                let created_at: i64 = row.get(4)?;
                let updated_at: Option<i64> = row.get(5)?;
                Ok(CheckpointUserMessageLink {
                    user_message_index: index.max(0) as usize,
                    message_id: row.get(1)?,
                    checkpoint_id: row.get(2)?,
                    has_file_edits: has_file_edits != 0,
                    created_at: created_at.max(0) as u64,
                    updated_at: updated_at.map(|value| value.max(0) as u64),
                })
            })
            .map_err(|e| e.to_string())?;

        let mut links = Vec::new();
        for row in rows {
            links.push(row.map_err(|e| e.to_string())?);
        }
        Ok(links)
    })
}

pub fn find_checkpoint_user_message_link(
    session_id: &str,
    user_message_index: usize,
) -> Result<Option<CheckpointUserMessageLink>, String> {
    with_connection(|conn| {
        let mut stmt = conn
            .prepare(
                "SELECT user_message_index, message_id, checkpoint_id, has_file_edits, created_at, updated_at
                 FROM checkpoint_user_message_links
                 WHERE session_id = ?1 AND user_message_index = ?2",
            )
            .map_err(|e| e.to_string())?;
        let mut rows = stmt
            .query(params![session_id, user_message_index as i64])
            .map_err(|e| e.to_string())?;

        if let Some(row) = rows.next().map_err(|e| e.to_string())? {
            let index: i64 = row.get(0).map_err(|e| e.to_string())?;
            let has_file_edits: i64 = row.get(3).map_err(|e| e.to_string())?;
            let created_at: i64 = row.get(4).map_err(|e| e.to_string())?;
            let updated_at: Option<i64> = row.get(5).map_err(|e| e.to_string())?;
            return Ok(Some(CheckpointUserMessageLink {
                user_message_index: index.max(0) as usize,
                message_id: row.get(1).map_err(|e| e.to_string())?,
                checkpoint_id: row.get(2).map_err(|e| e.to_string())?,
                has_file_edits: has_file_edits != 0,
                created_at: created_at.max(0) as u64,
                updated_at: updated_at.map(|value| value.max(0) as u64),
            }));
        }
        Ok(None)
    })
}

pub fn find_checkpoint_user_message_link_by_message_id(
    session_id: &str,
    message_id: &str,
) -> Result<Option<CheckpointUserMessageLink>, String> {
    with_connection(|conn| {
        conn.query_row(
            "SELECT user_message_index, message_id, checkpoint_id, has_file_edits, created_at, updated_at
             FROM checkpoint_user_message_links
             WHERE session_id = ?1 AND message_id = ?2",
            params![session_id, message_id],
            |row| {
                let index: i64 = row.get(0)?;
                let has_file_edits: i64 = row.get(3)?;
                let created_at: i64 = row.get(4)?;
                let updated_at: Option<i64> = row.get(5)?;
                Ok(CheckpointUserMessageLink {
                    user_message_index: index.max(0) as usize,
                    message_id: row.get(1)?,
                    checkpoint_id: row.get(2)?,
                    has_file_edits: has_file_edits != 0,
                    created_at: created_at.max(0) as u64,
                    updated_at: updated_at.map(|value| value.max(0) as u64),
                })
            },
        )
        .optional()
        .map_err(|e| e.to_string())
    })
}

pub fn delete_checkpoint_user_message_link_by_checkpoint(
    session_id: &str,
    checkpoint_id: &str,
) -> Result<(), String> {
    with_connection(|conn| {
        conn.execute(
            "DELETE FROM checkpoint_user_message_links WHERE session_id = ?1 AND checkpoint_id = ?2",
            params![session_id, checkpoint_id],
        )
        .map_err(|e| e.to_string())?;
        Ok(())
    })
}

fn configure_connection(conn: &Connection) -> Result<(), String> {
    conn.pragma_update(None, "journal_mode", "WAL")
        .map_err(|e| e.to_string())?;
    conn.pragma_update(None, "synchronous", "NORMAL")
        .map_err(|e| e.to_string())?;
    conn.pragma_update(None, "foreign_keys", "ON")
        .map_err(|e| e.to_string())?;
    conn.busy_timeout(std::time::Duration::from_millis(5000))
        .map_err(|e| e.to_string())?;
    Ok(())
}

#[cfg(test)]
mod patch_columns_tests {
    //! `agent_run_patches` 的列一致性护栏（与 `session/repository.rs` 的两套同口径）。

    use super::*;

    fn column_names(list: &str) -> Vec<&str> {
        list.split(',').map(|c| c.trim()).filter(|c| !c.is_empty()).collect()
    }

    #[test]
    fn patch_columns_cover_every_name_the_mapper_reads() {
        let columns = column_names(PATCH_COLUMNS);
        for name in [
            "run_id",
            "seq",
            "patch_json",
            "message",
            "trigger_user_memory_index",
            "trigger_user_message_id",
            "created_at",
        ] {
            assert!(
                columns.contains(&name),
                "PATCH_COLUMNS 缺少 {name}：{columns:?}"
            );
        }
        assert_eq!(
            columns.len(),
            7,
            "列数应为 7，实际 {}：{columns:?}",
            columns.len()
        );
    }

    /// 清单里的每一列都必须在真实表结构上存在（迁移删列没跟上时在这里暴露）。
    #[test]
    fn patch_columns_prepare_against_the_real_schema() {
        let conn = Connection::open_in_memory().unwrap();
        crate::infra::db::schema::init_schema(&conn).expect("建表");
        conn.prepare(&format!(
            "SELECT {} FROM agent_run_patches",
            PATCH_COLUMNS
        ))
        .unwrap_or_else(|e| panic!("PATCH_COLUMNS 无法在真实表结构上准备：{e}"));
    }
}
