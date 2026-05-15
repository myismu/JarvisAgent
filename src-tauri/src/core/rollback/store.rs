//! SQLite-backed snapshot persistence.

use super::SnapshotTree;
use rusqlite::{params, OptionalExtension};

#[derive(Debug, thiserror::Error)]
pub enum StoreError {
    #[error("JSON error: {0}")]
    JsonError(#[from] serde_json::Error),
    #[error("Database error: {0}")]
    DbError(String),
    #[error("Not found: {0}")]
    NotFound(String),
}

pub struct SnapshotStore {
    session_id: String,
}

fn now_ts() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
}

fn ensure_session_record(session_id: &str) -> Result<(), String> {
    crate::core::session::repository::ensure_session_exists(
        session_id,
        Some("Session snapshots"),
        now_ts(),
    )
}

impl SnapshotStore {
    pub fn new(session_id: &str) -> Self {
        Self {
            session_id: session_id.to_string(),
        }
    }

    pub fn delete_all_for_session(&self) -> Result<(), StoreError> {
        crate::infra::db::with_transaction(|tx| {
            tx.execute(
                "DELETE FROM checkpoint_user_message_links WHERE session_id = ?1",
                [self.session_id.as_str()],
            )
            .map_err(|e| e.to_string())?;
            tx.execute(
                "DELETE FROM snapshot_journal WHERE session_id = ?1",
                [self.session_id.as_str()],
            )
            .map_err(|e| e.to_string())?;
            tx.execute(
                "DELETE FROM snapshot_content WHERE session_id = ?1",
                [self.session_id.as_str()],
            )
            .map_err(|e| e.to_string())?;
            tx.execute(
                "DELETE FROM agent_run_patches WHERE session_id = ?1",
                [self.session_id.as_str()],
            )
            .map_err(|e| e.to_string())?;
            tx.execute(
                "DELETE FROM snapshot_trees WHERE session_id = ?1",
                [self.session_id.as_str()],
            )
            .map_err(|e| e.to_string())?;
            Ok(())
        })
        .map_err(StoreError::DbError)
    }

    pub fn save_tree(&self, tree: &SnapshotTree) -> Result<(), StoreError> {
        ensure_session_record(&self.session_id).map_err(StoreError::DbError)?;
        let json = serde_json::to_string(tree)?;
        crate::infra::db::with_connection(|conn| {
            conn.execute(
                "INSERT INTO snapshot_trees(session_id, tree_json, updated_at)
                 VALUES(?1, ?2, ?3)
                 ON CONFLICT(session_id) DO UPDATE SET
                    tree_json = excluded.tree_json,
                    updated_at = excluded.updated_at",
                params![self.session_id.as_str(), json, now_ts() as i64],
            )
            .map_err(|e| e.to_string())?;
            Ok(())
        })
        .map_err(StoreError::DbError)
    }

    pub fn load_tree(&self) -> Result<SnapshotTree, StoreError> {
        let json = crate::infra::db::with_connection(|conn| {
            conn.query_row(
                "SELECT tree_json FROM snapshot_trees WHERE session_id = ?1",
                [self.session_id.as_str()],
                |row| row.get::<_, String>(0),
            )
            .optional()
            .map_err(|e| e.to_string())
        })
        .map_err(StoreError::DbError)?;

        match json {
            Some(value) => serde_json::from_str(&value).map_err(StoreError::JsonError),
            None => Err(StoreError::NotFound("snapshot tree".to_string())),
        }
    }

}

pub fn save_content(session_id: &str, hash: &str, content: &str) -> Result<(), String> {
    ensure_session_record(session_id)?;
    crate::infra::db::with_connection(|conn| {
        conn.execute(
            "INSERT INTO snapshot_content(session_id, content_hash, content, created_at)
             VALUES(?1, ?2, ?3, ?4)
             ON CONFLICT(session_id, content_hash) DO UPDATE SET
                content = excluded.content",
            params![session_id, hash, content, now_ts() as i64],
        )
        .map_err(|e| e.to_string())?;
        Ok(())
    })
}

pub fn load_content(session_id: &str, hash: &str) -> Result<Option<String>, String> {
    crate::infra::db::with_connection(|conn| {
        conn.query_row(
            "SELECT content FROM snapshot_content WHERE session_id = ?1 AND content_hash = ?2",
            params![session_id, hash],
            |row| row.get::<_, String>(0),
        )
        .optional()
        .map_err(|e| e.to_string())
    })
}
