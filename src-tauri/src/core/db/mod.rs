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
//! - Internal: `crate::core::data_paths`, `crate::core::db::schema`
//! - External: `rusqlite`
//!
//! ## Constraints
//! - 数据库连接通过全局 Mutex 串行化写入，避免 tokio 多任务并发写锁冲突

pub mod schema;

use rusqlite::{params, Connection, Transaction};
use std::sync::{Mutex, OnceLock};

static DB: OnceLock<Mutex<Connection>> = OnceLock::new();

pub fn init() -> Result<(), String> {
    if DB.get().is_some() {
        return Ok(());
    }

    let path = crate::core::data_paths::sqlite_db_path();
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
    pub checkpoint_id: String,
    pub has_file_edits: bool,
    pub created_at: u64,
}

pub fn upsert_checkpoint_user_message_link(
    session_id: &str,
    user_message_index: usize,
    checkpoint_id: &str,
    has_file_edits: bool,
    created_at: u64,
) -> Result<(), String> {
    with_connection(|conn| {
        conn.execute(
            "INSERT INTO checkpoint_user_message_links(
                session_id, user_message_index, checkpoint_id, has_file_edits, created_at
             ) VALUES (?1, ?2, ?3, ?4, ?5)
             ON CONFLICT(session_id, user_message_index) DO UPDATE SET
                checkpoint_id = excluded.checkpoint_id,
                has_file_edits = excluded.has_file_edits,
                created_at = excluded.created_at",
            params![
                session_id,
                user_message_index as i64,
                checkpoint_id,
                if has_file_edits { 1 } else { 0 },
                created_at as i64,
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
                "SELECT user_message_index, checkpoint_id, has_file_edits, created_at
                 FROM checkpoint_user_message_links
                 WHERE session_id = ?1",
            )
            .map_err(|e| e.to_string())?;
        let rows = stmt
            .query_map([session_id], |row| {
                let index: i64 = row.get(0)?;
                let has_file_edits: i64 = row.get(2)?;
                let created_at: i64 = row.get(3)?;
                Ok(CheckpointUserMessageLink {
                    user_message_index: index.max(0) as usize,
                    checkpoint_id: row.get(1)?,
                    has_file_edits: has_file_edits != 0,
                    created_at: created_at.max(0) as u64,
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
                "SELECT user_message_index, checkpoint_id, has_file_edits, created_at
                 FROM checkpoint_user_message_links
                 WHERE session_id = ?1 AND user_message_index = ?2",
            )
            .map_err(|e| e.to_string())?;
        let mut rows = stmt
            .query(params![session_id, user_message_index as i64])
            .map_err(|e| e.to_string())?;

        if let Some(row) = rows.next().map_err(|e| e.to_string())? {
            let index: i64 = row.get(0).map_err(|e| e.to_string())?;
            let has_file_edits: i64 = row.get(2).map_err(|e| e.to_string())?;
            let created_at: i64 = row.get(3).map_err(|e| e.to_string())?;
            return Ok(Some(CheckpointUserMessageLink {
                user_message_index: index.max(0) as usize,
                checkpoint_id: row.get(1).map_err(|e| e.to_string())?,
                has_file_edits: has_file_edits != 0,
                created_at: created_at.max(0) as u64,
            }));
        }
        Ok(None)
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
