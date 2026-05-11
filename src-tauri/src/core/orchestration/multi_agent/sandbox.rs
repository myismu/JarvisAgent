//! 沙箱管理模块
//!
//! 为每个代理创建独立的工作区，实现：
//! - 隔离的文件操作空间
//! - 沙箱生命周期管理（创建、完成、发布、放弃）
//! - 多沙箱对比（统计变更量）

use crate::core::rollback::{Snapshot, SnapshotTree};
use rusqlite::params;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::fs;
use std::path::PathBuf;
use uuid::Uuid;

/// 代理沙箱实例
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AgentSandbox {
    pub sandbox_id: String,
    pub agent_id: String,
    pub workspace_id: String,
    pub branch_name: String,
    pub base_snapshot_id: String,
    pub workspace_path: PathBuf,
    pub status: SandboxStatus,
    pub created_at: u64,
    pub description: String,
}

/// 沙箱状态
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "lowercase")]
pub enum SandboxStatus {
    Active,
    Completed,
    Published,
    Abandoned,
}

/// 沙箱对比统计
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SandboxComparison {
    pub sandbox_id: String,
    pub agent_id: String,
    pub files_changed: usize,
    pub lines_added: usize,
    pub lines_removed: usize,
    pub snapshot_count: usize,
    pub last_snapshot_id: String,
    pub last_message: Option<String>,
}

/// 沙箱操作错误类型
#[derive(Debug, thiserror::Error)]
pub enum SandboxError {
    #[error("Sandbox not found: {0}")]
    NotFound(String),
    #[error("Sandbox already exists: {0}")]
    AlreadyExists(String),
    #[error("Invalid sandbox status: {0}")]
    InvalidStatus(String),
    #[error("IO error: {0}")]
    IoError(#[from] std::io::Error),
    #[error("JSON error: {0}")]
    JsonError(#[from] serde_json::Error),
    #[error("Branch error: {0}")]
    BranchError(String),
}

/// 沙箱管理器（管理所有代理沙箱的生命周期）
pub struct SandboxManager {
    base_dir: PathBuf,
    sandboxes: HashMap<String, AgentSandbox>,
    _session_id: String,
}

impl SandboxManager {
    pub fn new(base_dir: PathBuf, session_id: String) -> Self {
        Self {
            base_dir,
            sandboxes: HashMap::new(),
            _session_id: session_id,
        }
    }

    /// 创建新沙箱（分配独立工作目录和分支）
    pub fn create_sandbox(
        &mut self,
        agent_id: String,
        base_snapshot_id: String,
        description: Option<String>,
    ) -> Result<AgentSandbox, SandboxError> {
        let sandbox_id = format!("sandbox-{}", Uuid::new_v4().to_string()[..8].to_string());
        let workspace_id = format!("ws-{}", Uuid::new_v4().to_string()[..8].to_string());
        let branch_name = format!("agent-{}", agent_id);

        let sandbox_dir = self.base_dir.join("sandboxes").join(&sandbox_id);

        let workspace_path = sandbox_dir.join("workspace");
        fs::create_dir_all(&workspace_path)?;

        let sandbox = AgentSandbox {
            sandbox_id: sandbox_id.clone(),
            agent_id: agent_id.clone(),
            workspace_id: workspace_id.clone(),
            branch_name: branch_name.clone(),
            base_snapshot_id: base_snapshot_id.clone(),
            workspace_path: workspace_path.clone(),
            status: SandboxStatus::Active,
            created_at: current_timestamp(),
            description: description.unwrap_or_else(|| format!("Agent {} 的沙箱", agent_id)),
        };

        self.sandboxes.insert(sandbox_id.clone(), sandbox.clone());

        Ok(sandbox)
    }

    pub fn get_sandbox(&self, sandbox_id: &str) -> Option<&AgentSandbox> {
        self.sandboxes.get(sandbox_id)
    }

    pub fn get_sandbox_by_agent(&self, agent_id: &str) -> Option<&AgentSandbox> {
        self.sandboxes
            .values()
            .find(|s| s.agent_id == agent_id && s.status == SandboxStatus::Active)
    }

    pub fn list_sandboxes(&self) -> Vec<&AgentSandbox> {
        self.sandboxes.values().collect()
    }

    pub fn list_active_sandboxes(&self) -> Vec<&AgentSandbox> {
        self.sandboxes
            .values()
            .filter(|s| s.status == SandboxStatus::Active)
            .collect()
    }

    /// 标记沙箱完成（代理任务执行完毕）
    pub fn complete_sandbox(&mut self, sandbox_id: &str) -> Result<(), SandboxError> {
        let sandbox = self
            .sandboxes
            .get_mut(sandbox_id)
            .ok_or_else(|| SandboxError::NotFound(sandbox_id.to_string()))?;

        if sandbox.status != SandboxStatus::Active {
            return Err(SandboxError::InvalidStatus(format!("{:?}", sandbox.status)));
        }

        sandbox.status = SandboxStatus::Completed;
        Ok(())
    }

    /// 放弃沙箱（清理工作目录）
    pub fn abandon_sandbox(&mut self, sandbox_id: &str) -> Result<(), SandboxError> {
        let sandbox = self
            .sandboxes
            .get_mut(sandbox_id)
            .ok_or_else(|| SandboxError::NotFound(sandbox_id.to_string()))?;

        sandbox.status = SandboxStatus::Abandoned;

        if sandbox.workspace_path.exists() {
            fs::remove_dir_all(&sandbox.workspace_path)?;
        }

        Ok(())
    }

    /// 发布沙箱（准备合并到主分支）
    pub fn publish_sandbox(
        &mut self,
        sandbox_id: &str,
        tree: &mut SnapshotTree,
    ) -> Result<String, SandboxError> {
        let sandbox = self
            .sandboxes
            .get(sandbox_id)
            .ok_or_else(|| SandboxError::NotFound(sandbox_id.to_string()))?
            .clone();

        if sandbox.status != SandboxStatus::Completed {
            return Err(SandboxError::InvalidStatus(format!("{:?}", sandbox.status)));
        }

        let _main_branch = tree
            .branches
            .get("main")
            .ok_or_else(|| SandboxError::BranchError("main branch not found".to_string()))?;

        let merge_branch_name = format!("merged-{}", sandbox.agent_id);

        let sandbox_mut = self.sandboxes.get_mut(sandbox_id).unwrap();
        sandbox_mut.status = SandboxStatus::Published;

        Ok(merge_branch_name)
    }

    /// 对比所有活跃沙箱的变更统计
    pub fn compare_sandboxes(&self, tree: &SnapshotTree) -> Vec<SandboxComparison> {
        self.sandboxes
            .values()
            .filter(|s| s.status == SandboxStatus::Active || s.status == SandboxStatus::Completed)
            .map(|sandbox| {
                let snapshots: Vec<&Snapshot> = tree
                    .nodes
                    .values()
                    .filter(|s| s.branch_name == sandbox.branch_name)
                    .collect();

                let last_snapshot = snapshots.iter().max_by_key(|s| s.created_at);

                let (lines_added, lines_removed, files_changed) = snapshots
                    .iter()
                    .flat_map(|s| s.patches.iter())
                    .fold((0, 0, 0), |(added, removed, changed), patch| {
                        let summary = patch.to_summary();
                        (
                            added + summary.lines_added,
                            removed + summary.lines_removed,
                            changed + 1,
                        )
                    });

                SandboxComparison {
                    sandbox_id: sandbox.sandbox_id.clone(),
                    agent_id: sandbox.agent_id.clone(),
                    files_changed,
                    lines_added,
                    lines_removed,
                    snapshot_count: snapshots.len(),
                    last_snapshot_id: last_snapshot.map(|s| s.id.clone()).unwrap_or_default(),
                    last_message: last_snapshot.and_then(|s| s.message.clone()),
                }
            })
            .collect()
    }

    /// 从磁盘加载沙箱索引
    pub fn load(&mut self) -> Result<(), SandboxError> {
        let rows = crate::infra::db::with_connection(|conn| {
            let mut stmt = conn
                .prepare("SELECT sandbox_json FROM snapshot_sandboxes WHERE session_id = ?1")
                .map_err(|e| e.to_string())?;
            let rows = stmt
                .query_map([self._session_id.as_str()], |row| row.get::<_, String>(0))
                .map_err(|e| e.to_string())?;
            let mut values = Vec::new();
            for row in rows {
                values.push(row.map_err(|e| e.to_string())?);
            }
            Ok(values)
        })
        .map_err(SandboxError::BranchError)?;

        self.sandboxes = rows
            .into_iter()
            .filter_map(|json| serde_json::from_str::<AgentSandbox>(&json).ok())
            .map(|sandbox| (sandbox.sandbox_id.clone(), sandbox))
            .collect();

        Ok(())
    }

    /// 保存沙箱索引到磁盘
    pub fn save(&self) -> Result<(), SandboxError> {
        crate::core::session::repository::ensure_session_exists(
            &self._session_id,
            Some("Session sandboxes"),
            current_timestamp(),
        )
        .map_err(SandboxError::BranchError)?;
        crate::infra::db::with_transaction(|tx| {
            tx.execute(
                "DELETE FROM snapshot_sandboxes WHERE session_id = ?1",
                [self._session_id.as_str()],
            )
            .map_err(|e| e.to_string())?;
            for sandbox in self.sandboxes.values() {
                let json = serde_json::to_string(sandbox).map_err(|e| e.to_string())?;
                tx.execute(
                    "INSERT INTO snapshot_sandboxes(session_id, sandbox_id, sandbox_json, updated_at)
                     VALUES(?1, ?2, ?3, ?4)",
                    params![
                        self._session_id.as_str(),
                        sandbox.sandbox_id.as_str(),
                        json,
                        current_timestamp() as i64,
                    ],
                )
                .map_err(|e| e.to_string())?;
            }
            Ok(())
        })
        .map_err(SandboxError::BranchError)
    }
}

fn current_timestamp() -> u64 {
    use std::time::{SystemTime, UNIX_EPOCH};
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
}
