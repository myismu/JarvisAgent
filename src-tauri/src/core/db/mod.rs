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

use rusqlite::{Connection, Transaction};
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
