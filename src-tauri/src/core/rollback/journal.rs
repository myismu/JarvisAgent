//! SQLite-backed operation journal for snapshot lifecycle events.

use super::snapshot::SnapshotTree;
use rusqlite::{params, OptionalExtension};
use serde::{Deserialize, Serialize};

const JOURNAL_COMPACT_THRESHOLD: u64 = 1000;

#[derive(Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum JournalEntry {
    CreateSnapshot {
        id: String,
        parent_id: Option<String>,
        branch_name: String,
        patches_count: usize,
        message: Option<String>,
        timestamp: u64,
    },
    CreateBranch {
        name: String,
        from_snapshot_id: String,
        agent_id: Option<String>,
    },
    SwitchBranch {
        name: String,
    },
    DeleteBranch {
        name: String,
    },
    Compact {
        snapshot_ids: Vec<String>,
        branch_names: Vec<String>,
    },
}

#[derive(Debug, thiserror::Error)]
pub enum JournalError {
    #[error("Database error: {0}")]
    DbError(String),
    #[error("JSON error: {0}")]
    JsonError(#[from] serde_json::Error),
}

pub struct Journal {
    session_id: String,
    sequence: u64,
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

impl Journal {
    pub fn open(session_id: &str) -> Result<Self, JournalError> {
        let sequence = crate::infra::db::with_connection(|conn| {
            conn.query_row(
                "SELECT COUNT(*) FROM snapshot_journal WHERE session_id = ?1",
                [session_id],
                |row| row.get::<_, i64>(0),
            )
            .optional()
            .map(|value| value.unwrap_or_default() as u64)
            .map_err(|e| e.to_string())
        })
        .map_err(JournalError::DbError)?;

        Ok(Self {
            session_id: session_id.to_string(),
            sequence,
        })
    }

    pub fn append(&mut self, entry: &JournalEntry) -> Result<(), JournalError> {
        ensure_session_record(&self.session_id).map_err(JournalError::DbError)?;
        let json = serde_json::to_string(entry)?;
        crate::infra::db::with_connection(|conn| {
            conn.execute(
                "INSERT INTO snapshot_journal(session_id, event_json, created_at)
                 VALUES(?1, ?2, ?3)",
                params![self.session_id.as_str(), json, now_ts() as i64],
            )
            .map_err(|e| e.to_string())?;
            Ok(())
        })
        .map_err(JournalError::DbError)?;
        self.sequence = self.sequence.saturating_add(1);
        Ok(())
    }

    pub fn replay(&self) -> Result<Vec<JournalEntry>, JournalError> {
        crate::infra::db::with_connection(|conn| {
            let mut stmt = conn
                .prepare(
                    "SELECT event_json FROM snapshot_journal WHERE session_id = ?1 ORDER BY id",
                )
                .map_err(|e| e.to_string())?;
            let rows = stmt
                .query_map([self.session_id.as_str()], |row| row.get::<_, String>(0))
                .map_err(|e| e.to_string())?;

            let mut entries = Vec::new();
            for row in rows {
                let json = row.map_err(|e| e.to_string())?;
                entries.push(serde_json::from_str(&json).map_err(|e| e.to_string())?);
            }
            Ok(entries)
        })
        .map_err(JournalError::DbError)
    }

    pub fn should_compact(&self) -> bool {
        self.sequence >= JOURNAL_COMPACT_THRESHOLD
    }

    pub fn compact(&mut self, tree: &SnapshotTree) -> Result<(), JournalError> {
        ensure_session_record(&self.session_id).map_err(JournalError::DbError)?;
        crate::infra::db::with_transaction(|tx| {
            tx.execute(
                "DELETE FROM snapshot_journal WHERE session_id = ?1",
                [self.session_id.as_str()],
            )
            .map_err(|e| e.to_string())?;

            let mut entries = Vec::new();
            entries.push(JournalEntry::Compact {
                snapshot_ids: tree.nodes.keys().cloned().collect(),
                branch_names: tree.branches.keys().cloned().collect(),
            });
            entries.extend(
                tree.nodes
                    .values()
                    .map(|snapshot| JournalEntry::CreateSnapshot {
                        id: snapshot.id.clone(),
                        parent_id: snapshot.parent_id.clone(),
                        branch_name: snapshot.branch_name.clone(),
                        patches_count: snapshot.patches.len(),
                        message: snapshot.message.clone(),
                        timestamp: snapshot.created_at,
                    }),
            );
            entries.extend(
                tree.branches
                    .values()
                    .map(|branch| JournalEntry::CreateBranch {
                        name: branch.name.clone(),
                        from_snapshot_id: branch.head_snapshot_id.clone(),
                        agent_id: branch.agent_id.clone(),
                    }),
            );

            for entry in entries {
                let json = serde_json::to_string(&entry).map_err(|e| e.to_string())?;
                tx.execute(
                    "INSERT INTO snapshot_journal(session_id, event_json, created_at)
                     VALUES(?1, ?2, ?3)",
                    params![self.session_id.as_str(), json, now_ts() as i64],
                )
                .map_err(|e| e.to_string())?;
            }
            Ok(())
        })
        .map_err(JournalError::DbError)?;

        self.sequence = tree.nodes.len() as u64 + tree.branches.len() as u64 + 1;
        Ok(())
    }

    pub fn sequence(&self) -> u64 {
        self.sequence
    }
}
