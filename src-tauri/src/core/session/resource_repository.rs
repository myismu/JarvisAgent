//! SQLite-backed storage for session-scoped binary/text resources.

use rusqlite::{params, OptionalExtension};
use std::path::Path;

use crate::infra::types::models::Task;

/// 当前时间戳（**毫秒**，Unix epoch）——全项目 DB 时间戳统一毫秒口径（v16 起）。
fn now_ts() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as u64
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

/// 校验自定义协议 / 工具传入的附件名是否安全：非空、不含路径分隔符与 `..`。
///
/// `jarvis-attachment` 协议把 URL 路径直接当附件名查库，这里是唯一的入口闸门；
/// 抽成纯函数以便单测锁住（路径穿越类问题一旦回归就是任意读取）。
pub fn is_safe_attachment_name(name: &str) -> bool {
    !name.is_empty() && !name.contains("..") && !name.contains('/') && !name.contains('\\')
}

/// 按当前会话 ID 严格校验并读取附件元信息（防止跨会话越权访问）
pub fn load_session_attachment_meta(
    session_id: &str,
    filename: &str,
) -> Result<Option<(String, String)>, String> {
    let filename = basename(filename);
    crate::infra::db::with_connection(|conn| query_session_attachment_meta(conn, session_id, &filename))
}

/// `load_session_attachment_meta` 的纯 SQL 部分（接收连接，便于内存库单测）
fn query_session_attachment_meta(
    conn: &rusqlite::Connection,
    session_id: &str,
    filename: &str,
) -> Result<Option<(String, String)>, String> {
    conn.query_row(
        "SELECT filename, media_type FROM session_attachments WHERE session_id = ?1 AND filename = ?2",
        params![session_id, filename],
        |row| Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?)),
    )
    .optional()
    .map_err(|e| e.to_string())
}

/// 列出指定会话内的全部附件 `(filename, media_type)`（按创建时间升序）
pub fn list_session_attachments(session_id: &str) -> Result<Vec<(String, String)>, String> {
    crate::infra::db::with_connection(|conn| query_session_attachments(conn, session_id))
}

/// `list_session_attachments` 的纯 SQL 部分（接收连接，便于内存库单测）
fn query_session_attachments(
    conn: &rusqlite::Connection,
    session_id: &str,
) -> Result<Vec<(String, String)>, String> {
    let mut stmt = conn
        .prepare(
            "SELECT filename, media_type FROM session_attachments WHERE session_id = ?1 ORDER BY created_at ASC",
        )
        .map_err(|e| e.to_string())?;
    let rows = stmt
        .query_map([session_id], |row| {
            Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
        })
        .map_err(|e| e.to_string())?;
    let mut out = Vec::new();
    for row in rows {
        out.push(row.map_err(|e| e.to_string())?);
    }
    Ok(out)
}

#[cfg(test)]
mod attachment_isolation_tests {
    //! 会话附件隔离：`ViewSessionImage` 只能看到**当前会话**的图片，跨会话必须查不到。

    use super::*;
    use rusqlite::Connection;

    fn conn_with_two_sessions() -> Connection {
        let conn = Connection::open_in_memory().unwrap();
        crate::infra::db::schema::init_schema(&conn).expect("建表");
        for sid in ["sess_a", "sess_b"] {
            conn.execute(
                "INSERT INTO sessions(id, title, created_at, updated_at, message_count) \
                 VALUES(?1, 't', 0, 0, 0)",
                params![sid],
            )
            .unwrap();
        }
        let insert = |filename: &str, session: &str, media: &str, ts: i64| {
            conn.execute(
                "INSERT INTO session_attachments(filename, session_id, media_type, data, created_at) \
                 VALUES(?1, ?2, ?3, x'00', ?4)",
                params![filename, session, media, ts],
            )
            .unwrap();
        };
        insert("a_1.png", "sess_a", "image/png", 1);
        insert("a_2.jpg", "sess_a", "image/jpeg", 2);
        insert("b_1.png", "sess_b", "image/png", 3);
        conn
    }

    #[test]
    fn meta_lookup_is_scoped_to_the_given_session() {
        let conn = conn_with_two_sessions();
        // 本会话的图查得到
        let own = query_session_attachment_meta(&conn, "sess_a", "a_1.png").unwrap();
        assert_eq!(own, Some(("a_1.png".to_string(), "image/png".to_string())));
        // 别的会话的图：文件名真实存在，但会话不匹配 → 必须查不到
        let foreign = query_session_attachment_meta(&conn, "sess_a", "b_1.png").unwrap();
        assert_eq!(foreign, None, "不得跨会话读取附件");
        // 不存在的文件名同样为 None
        assert_eq!(
            query_session_attachment_meta(&conn, "sess_a", "nope.png").unwrap(),
            None
        );
    }

    #[test]
    fn listing_only_returns_the_given_sessions_attachments_in_order() {
        let conn = conn_with_two_sessions();
        let a = query_session_attachments(&conn, "sess_a").unwrap();
        assert_eq!(
            a,
            vec![
                ("a_1.png".to_string(), "image/png".to_string()),
                ("a_2.jpg".to_string(), "image/jpeg".to_string()),
            ]
        );
        let b = query_session_attachments(&conn, "sess_b").unwrap();
        assert_eq!(b.len(), 1);
        assert!(query_session_attachments(&conn, "sess_none").unwrap().is_empty());
    }

    #[test]
    fn attachment_name_validation_rejects_path_traversal() {
        assert!(is_safe_attachment_name("45dc870d_148726fc.jpg"));
        assert!(!is_safe_attachment_name(""));
        assert!(!is_safe_attachment_name("../secret.db"));
        assert!(!is_safe_attachment_name("a/b.png"));
        assert!(!is_safe_attachment_name("a\\b.png"));
        assert!(!is_safe_attachment_name(".."));
    }
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
