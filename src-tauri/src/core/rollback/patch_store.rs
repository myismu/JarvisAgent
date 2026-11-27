//! # patch_store.rs — 运行补丁暂存与撤回检查点关联持久化
//!
//! 承载 `agent_run_patches`（本轮待提交文件补丁的崩溃兜底暂存表）与
//! `checkpoint_user_message_links`（用户消息 UUID 与快照检查点双向锚点表）的 SQLite 读写。
//!
//! ## Key Exports
//! - `PendingSnapshotPatchRecord` / `CheckpointUserMessageLink`: 持久化记录结构
//! - `insert_agent_run_patch()` / `list_agent_run_patches()` / `delete_agent_run_patches()`
//! - `upsert_checkpoint_user_message_link()` / `insert_checkpoint_user_message_link_if_absent()`
//! - `list_checkpoint_user_message_links()` / `find_checkpoint_user_message_link()`
//! - `delete_checkpoint_user_message_links_from_seq()` / `delete_checkpoint_user_message_link_by_checkpoint()`
//!
//! ## Dependencies
//! - Internal: `crate::core::rollback::Patch`, `crate::infra::db::{with_connection, with_read_connection, with_transaction}`, `crate::infra::types::models::MessageSource`
//! - External: `rusqlite`, `serde_json`

use crate::core::rollback::Patch;
use crate::infra::db::{with_connection, with_read_connection, with_transaction};
use crate::infra::types::models::MessageSource;
use rusqlite::{params, OptionalExtension};

#[derive(Debug, Clone)]
pub struct CheckpointUserMessageLink {
    /// 用户消息的稳定 UUID（跨压缩、撤回、分页都不变）——撤回关联的唯一锚点。
    pub message_id: String,
    pub checkpoint_id: String,
    pub has_file_edits: bool,
    pub created_at: u64,
    pub updated_at: Option<u64>,
    /// 触发消息在 `session_messages` 中的全局 seq。
    ///
    /// 消息已被撤回 / 隐藏 / 压缩摘要时为 `None`：这类关联已经失效，不能参与撤回解析。
    pub message_seq: Option<usize>,
}

#[derive(Debug, Clone)]
pub struct PendingSnapshotPatchRecord {
    pub run_id: String,
    pub seq: usize,
    pub patch: Patch,
    pub message: Option<String>,
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
    patch: &Patch,
    message: Option<&str>,
    trigger_user_message_id: Option<&str>,
) -> Result<(), String> {
    let patch_json = serde_json::to_string(patch).map_err(|e| e.to_string())?;
    with_connection(|conn| {
        conn.execute(
            // `trigger_user_memory_index` 是 v29 之前的旧锚点，已无任何读取方。
            // 新写入一律置 NULL，避免它继续以"看起来还有用"的形态留在库里。
            "INSERT INTO agent_run_patches(
                session_id, run_id, seq, patch_json, message, trigger_user_memory_index,
                trigger_user_message_id, created_at
             ) VALUES (?1, ?2, ?3, ?4, ?5, NULL, ?6, ?7)
             ON CONFLICT(session_id, run_id, seq) DO UPDATE SET
                patch_json = excluded.patch_json,
                message = excluded.message,
                trigger_user_memory_index = NULL,
                trigger_user_message_id = excluded.trigger_user_message_id,
                created_at = excluded.created_at",
            params![
                session_id,
                run_id,
                seq as i64,
                patch_json,
                message,
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
    with_read_connection(|conn| {
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
///
/// v29 起不再读取 `trigger_user_memory_index`：它是易变的内存下标，已无任何解析用途。
const PATCH_COLUMNS: &str = "run_id, seq, patch_json, message, trigger_user_message_id, created_at";

/// 从 `agent_run_patches` 的一行还原变更补丁记录（按列名取值，避免列序偏移）。
fn agent_run_patch_from_row(
    row: &rusqlite::Row<'_>,
) -> rusqlite::Result<PendingSnapshotPatchRecord> {
    let seq: i64 = row.get("seq")?;
    let patch_json: String = row.get("patch_json")?;
    let trigger_user_message_id: Option<String> = row.get("trigger_user_message_id")?;
    let created_at: i64 = row.get("created_at")?;
    let patch = serde_json::from_str(&patch_json).map_err(|err| {
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
        trigger_user_message_id,
        created_at: created_at.max(0) as u64,
    })
}

pub fn delete_agent_run_patches(session_id: &str, run_id: Option<&str>) -> Result<(), String> {
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

/// `checkpoint_user_message_links` 的 SELECT 清单（两处查询共用一份）。
///
/// `message_seq` 通过 JOIN `session_messages` 现取：它才是跨压缩/撤回稳定的顺序键；
/// 表里不再保存任何"内存下标"。
const CHECKPOINT_LINK_COLUMNS: &str =
    "l.message_id, l.checkpoint_id, l.has_file_edits, l.created_at, l.updated_at, \
     m.seq AS message_seq";

fn checkpoint_link_from_row(
    row: &rusqlite::Row<'_>,
) -> rusqlite::Result<CheckpointUserMessageLink> {
    let has_file_edits: i64 = row.get("has_file_edits")?;
    let created_at: i64 = row.get("created_at")?;
    let updated_at: Option<i64> = row.get("updated_at")?;
    let message_seq: Option<i64> = row.get("message_seq")?;
    Ok(CheckpointUserMessageLink {
        message_id: row.get("message_id")?,
        checkpoint_id: row.get("checkpoint_id")?,
        has_file_edits: has_file_edits != 0,
        created_at: created_at.max(0) as u64,
        updated_at: updated_at.map(|value| value.max(0) as u64),
        message_seq: message_seq.map(|value| value.max(0) as usize),
    })
}

/// 写入/更新"用户消息 → 本轮快照"的关联（唯一锚点是 `message_id`）。
///
/// 同一个 `checkpoint_id` 只允许指向一条消息，因此在插入前先清掉指向该快照的旧行，
/// 避免历史脏数据触发 `UNIQUE(session_id, checkpoint_id)`。
pub fn upsert_checkpoint_user_message_link(
    session_id: &str,
    message_id: &str,
    checkpoint_id: &str,
    has_file_edits: bool,
    created_at: u64,
) -> Result<(), String> {
    if message_id.trim().is_empty() || checkpoint_id.trim().is_empty() {
        return Err("写入撤回关联失败：message_id / checkpoint_id 不能为空".to_string());
    }
    with_transaction(|tx| {
        tx.execute(
            "DELETE FROM checkpoint_user_message_links
             WHERE session_id = ?1 AND checkpoint_id = ?2 AND message_id <> ?3",
            params![session_id, checkpoint_id, message_id],
        )
        .map_err(|e| e.to_string())?;
        tx.execute(
            "INSERT INTO checkpoint_user_message_links(
                session_id, message_id, checkpoint_id, has_file_edits, created_at, updated_at
             ) VALUES (?1, ?2, ?3, ?4, ?5, ?5)
             ON CONFLICT(session_id, message_id) DO UPDATE SET
                checkpoint_id = excluded.checkpoint_id,
                has_file_edits = excluded.has_file_edits,
                created_at = excluded.created_at,
                updated_at = excluded.updated_at",
            params![
                session_id,
                message_id,
                checkpoint_id,
                if has_file_edits { 1 } else { 0 },
                created_at as i64,
            ],
        )
        .map_err(|e| e.to_string())?;
        Ok(())
    })
}

/// 仅在关联不存在时写入（用于从快照树回填旧会话，绝不覆盖已有权威关联）。
pub fn insert_checkpoint_user_message_link_if_absent(
    session_id: &str,
    message_id: &str,
    checkpoint_id: &str,
    has_file_edits: bool,
    created_at: u64,
) -> Result<(), String> {
    if message_id.trim().is_empty() || checkpoint_id.trim().is_empty() {
        return Ok(());
    }
    with_connection(|conn| {
        conn.execute(
            "INSERT OR IGNORE INTO checkpoint_user_message_links(
                session_id, message_id, checkpoint_id, has_file_edits, created_at, updated_at
             ) VALUES (?1, ?2, ?3, ?4, ?5, ?5)",
            params![
                session_id,
                message_id,
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
    with_read_connection(|conn| {
        let mut stmt = conn
            .prepare(&format!(
                "SELECT {} FROM checkpoint_user_message_links l
                 LEFT JOIN session_messages m
                   ON m.session_id = l.session_id
                  AND m.message_id = l.message_id
                  AND m.hidden_at IS NULL
                  AND m.recalled_at IS NULL
                  AND m.source <> ?2
                 WHERE l.session_id = ?1
                 ORDER BY m.seq ASC, l.created_at ASC",
                CHECKPOINT_LINK_COLUMNS
            ))
            .map_err(|e| e.to_string())?;
        let rows = stmt
            .query_map(
                params![session_id, MessageSource::Compact.as_db()],
                checkpoint_link_from_row,
            )
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
    message_id: &str,
) -> Result<Option<CheckpointUserMessageLink>, String> {
    with_read_connection(|conn| {
        conn.query_row(
            &format!(
                "SELECT {} FROM checkpoint_user_message_links l
                 LEFT JOIN session_messages m
                   ON m.session_id = l.session_id
                  AND m.message_id = l.message_id
                  AND m.hidden_at IS NULL
                  AND m.recalled_at IS NULL
                  AND m.source <> ?3
                 WHERE l.session_id = ?1 AND l.message_id = ?2",
                CHECKPOINT_LINK_COLUMNS
            ),
            params![session_id, message_id, MessageSource::Compact.as_db()],
            checkpoint_link_from_row,
        )
        .optional()
        .map_err(|e| e.to_string())
    })
}

/// 撤回消息时清理"被撤回区间"内的关联。
///
/// 必须在物理删除 `session_messages` **之前**调用：子查询要按 seq 找到这些 message_id。
pub fn delete_checkpoint_user_message_links_from_seq(
    session_id: &str,
    seq: usize,
) -> Result<(), String> {
    with_connection(|conn| {
        conn.execute(
            "DELETE FROM checkpoint_user_message_links
             WHERE session_id = ?1
               AND message_id IN (
                   SELECT message_id FROM session_messages
                   WHERE session_id = ?1 AND seq >= ?2
               )",
            params![session_id, seq as i64],
        )
        .map_err(|e| e.to_string())?;
        Ok(())
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

#[cfg(test)]
mod patch_columns_tests {
    //! `agent_run_patches` 的列一致性护栏（与 `session/repository.rs` 的两套同口径）。

    use super::*;
    use rusqlite::Connection;

    fn column_names(list: &str) -> Vec<&str> {
        list.split(',')
            .map(|c| c.trim())
            .filter(|c| !c.is_empty())
            .collect()
    }

    #[test]
    fn patch_columns_cover_every_name_the_mapper_reads() {
        let columns = column_names(PATCH_COLUMNS);
        for name in [
            "run_id",
            "seq",
            "patch_json",
            "message",
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
            6,
            "列数应为 6，实际 {}：{columns:?}",
            columns.len()
        );
    }

    /// 清单里的每一列都必须在真实表结构上存在（迁移删列没跟上时在这里暴露）。
    #[test]
    fn patch_columns_prepare_against_the_real_schema() {
        let conn = Connection::open_in_memory().unwrap();
        crate::infra::db::schema::init_schema(&conn).expect("建表");
        conn.prepare(&format!("SELECT {} FROM agent_run_patches", PATCH_COLUMNS))
            .unwrap_or_else(|e| panic!("PATCH_COLUMNS 无法在真实表结构上准备：{e}"));
    }
}
