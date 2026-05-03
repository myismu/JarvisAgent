//! SQLite-backed snapshot persistence.

use super::{Snapshot, SnapshotTree};
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

    pub fn save_snapshot(&self, snapshot: &Snapshot) -> Result<(), StoreError> {
        ensure_session_record(&self.session_id).map_err(StoreError::DbError)?;
        let json = serde_json::to_string(snapshot)?;
        crate::core::db::with_connection(|conn| {
            conn.execute(
                "INSERT INTO snapshots(session_id, snapshot_id, branch_name, snapshot_json, created_at)
                 VALUES(?1, ?2, ?3, ?4, ?5)
                 ON CONFLICT(session_id, snapshot_id) DO UPDATE SET
                    branch_name = excluded.branch_name,
                    snapshot_json = excluded.snapshot_json,
                    created_at = excluded.created_at",
                params![
                    self.session_id.as_str(),
                    snapshot.id.as_str(),
                    snapshot.branch_name.as_str(),
                    json,
                    snapshot.created_at as i64,
                ],
            )
            .map_err(|e| e.to_string())?;
            Ok(())
        })
        .map_err(StoreError::DbError)
    }

    pub fn load_snapshot(
        &self,
        branch_name: &str,
        snapshot_id: &str,
    ) -> Result<Option<Snapshot>, StoreError> {
        let json = crate::core::db::with_connection(|conn| {
            conn.query_row(
                "SELECT snapshot_json FROM snapshots
                 WHERE session_id = ?1 AND branch_name = ?2 AND snapshot_id = ?3",
                params![self.session_id.as_str(), branch_name, snapshot_id],
                |row| row.get::<_, String>(0),
            )
            .optional()
            .map_err(|e| e.to_string())
        })
        .map_err(StoreError::DbError)?;

        json.map(|value| serde_json::from_str(&value).map_err(StoreError::JsonError))
            .transpose()
    }

    pub fn delete_snapshot(&self, branch_name: &str, snapshot_id: &str) -> Result<(), StoreError> {
        crate::core::db::with_connection(|conn| {
            conn.execute(
                "DELETE FROM snapshots WHERE session_id = ?1 AND branch_name = ?2 AND snapshot_id = ?3",
                params![self.session_id.as_str(), branch_name, snapshot_id],
            )
            .map_err(|e| e.to_string())?;
            Ok(())
        })
        .map_err(StoreError::DbError)
    }

    pub fn save_tree(&self, tree: &SnapshotTree) -> Result<(), StoreError> {
        ensure_session_record(&self.session_id).map_err(StoreError::DbError)?;
        let json = serde_json::to_string(tree)?;
        crate::core::db::with_connection(|conn| {
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
        let json = crate::core::db::with_connection(|conn| {
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

    pub fn list_snapshots(&self, branch_name: &str) -> Result<Vec<Snapshot>, StoreError> {
        crate::core::db::with_connection(|conn| {
            let mut stmt = conn
                .prepare(
                    "SELECT snapshot_json FROM snapshots
                     WHERE session_id = ?1 AND branch_name = ?2
                     ORDER BY created_at",
                )
                .map_err(|e| e.to_string())?;
            let rows = stmt
                .query_map(params![self.session_id.as_str(), branch_name], |row| {
                    row.get::<_, String>(0)
                })
                .map_err(|e| e.to_string())?;

            let mut snapshots = Vec::new();
            for row in rows {
                let json = row.map_err(|e| e.to_string())?;
                snapshots.push(serde_json::from_str(&json).map_err(|e| e.to_string())?);
            }
            Ok(snapshots)
        })
        .map_err(StoreError::DbError)
    }
}

pub fn save_content(session_id: &str, hash: &str, content: &str) -> Result<(), String> {
    ensure_session_record(session_id)?;
    crate::core::db::with_connection(|conn| {
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
    crate::core::db::with_connection(|conn| {
        conn.query_row(
            "SELECT content FROM snapshot_content WHERE session_id = ?1 AND content_hash = ?2",
            params![session_id, hash],
            |row| row.get::<_, String>(0),
        )
        .optional()
        .map_err(|e| e.to_string())
    })
}
