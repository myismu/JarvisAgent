//! 操作日志模块
//!
//! 追加写入式日志，记录所有快照操作，支持崩溃恢复和状态重放。

use super::snapshot::SnapshotTree;
use serde::{Deserialize, Serialize};
use std::fs::{File, OpenOptions};
use std::io::{BufRead, BufReader, Write};
use std::path::PathBuf;

/// 日志压缩阈值（超过此数量触发压缩）
const JOURNAL_COMPACT_THRESHOLD: u64 = 1000;

/// 日志条目类型
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

/// 日志操作错误类型
#[derive(Debug, thiserror::Error)]
pub enum JournalError {
    #[error("IO error: {0}")]
    IoError(#[from] std::io::Error),
    #[error("JSON error: {0}")]
    JsonError(#[from] serde_json::Error),
    #[error("Journal is closed")]
    JournalClosed,
}

/// 操作日志（JSONL 格式，每行一个条目）
pub struct Journal {
    path: PathBuf,
    file: Option<File>,
    sequence: u64,
}

impl Journal {
    /// 打开或创建日志文件
    pub fn open(path: &PathBuf) -> Result<Self, JournalError> {
        let file = OpenOptions::new()
            .create(true)
            .append(true)
            .read(true)
            .open(path)?;

        let sequence = Self::count_lines(&file)?;

        Ok(Self {
            path: path.clone(),
            file: Some(file),
            sequence,
        })
    }

    fn count_lines(file: &File) -> Result<u64, JournalError> {
        let reader = BufReader::new(file);
        let count = reader.lines().filter_map(|r| r.ok()).count() as u64;
        Ok(count)
    }

    /// 追加日志条目并同步到磁盘
    pub fn append(&mut self, entry: &JournalEntry) -> Result<(), JournalError> {
        let json = serde_json::to_string(entry)?;
        let file = self.file.as_mut().ok_or(JournalError::JournalClosed)?;
        writeln!(file, "{}", json)?;
        file.sync_all()?;
        self.sequence += 1;
        Ok(())
    }

    /// 重放日志（用于崩溃恢复）
    pub fn replay(&self) -> Result<Vec<JournalEntry>, JournalError> {
        let file = File::open(&self.path)?;
        let reader = BufReader::new(file);
        let mut entries = Vec::new();

        for line in reader.lines() {
            let line = line?;
            if line.trim().is_empty() {
                continue;
            }
            let entry: JournalEntry = serde_json::from_str(&line)?;
            entries.push(entry);
        }

        Ok(entries)
    }

    /// 判断是否需要压缩日志
    pub fn should_compact(&self) -> bool {
        self.sequence >= JOURNAL_COMPACT_THRESHOLD
    }

    /// 压缩日志（保留当前状态，丢弃历史条目）
    pub fn compact(&mut self, tree: &SnapshotTree) -> Result<(), JournalError> {
        let compacted_path = self.path.with_extension("compact");
        let mut compacted = OpenOptions::new()
            .create(true)
            .write(true)
            .truncate(true)
            .open(&compacted_path)?;

        let entry = JournalEntry::Compact {
            snapshot_ids: tree.nodes.keys().cloned().collect(),
            branch_names: tree.branches.keys().cloned().collect(),
        };
        writeln!(compacted, "{}", serde_json::to_string(&entry)?)?;

        for snapshot in tree.nodes.values() {
            let entry = JournalEntry::CreateSnapshot {
                id: snapshot.id.clone(),
                parent_id: snapshot.parent_id.clone(),
                branch_name: snapshot.branch_name.clone(),
                patches_count: snapshot.patches.len(),
                message: snapshot.message.clone(),
                timestamp: snapshot.created_at,
            };
            writeln!(compacted, "{}", serde_json::to_string(&entry)?)?;
        }

        for branch in tree.branches.values() {
            let entry = JournalEntry::CreateBranch {
                name: branch.name.clone(),
                from_snapshot_id: branch.head_snapshot_id.clone(),
                agent_id: branch.agent_id.clone(),
            };
            writeln!(compacted, "{}", serde_json::to_string(&entry)?)?;
        }

        compacted.sync_all()?;

        std::fs::rename(&compacted_path, &self.path)?;

        self.file = Some(OpenOptions::new().append(true).open(&self.path)?);
        self.sequence = tree.nodes.len() as u64 + tree.branches.len() as u64;

        Ok(())
    }

    pub fn sequence(&self) -> u64 {
        self.sequence
    }
}
