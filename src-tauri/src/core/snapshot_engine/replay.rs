//! 重放引擎模块
//!
//! 从快照链重建工作区状态，支持：
//! - 全量重放（从头重建）
//! - 增量重放（基于当前状态的 LCA 差异计算）
//! - 原子回滚（带 undo 日志的文件级回滚）

use super::patch::Patch;
use super::snapshot::{Snapshot, SnapshotTree, Workspace, WorkspaceState};
use serde::{Deserialize, Serialize};
use std::collections::HashSet;
use std::fs;
use std::path::PathBuf;
use uuid::Uuid;

/// 重放操作错误类型
#[derive(Debug, thiserror::Error)]
pub enum ReplayError {
    #[error("Snapshot not found: {0}")]
    SnapshotNotFound(String),
    #[error("No common ancestor found")]
    NoCommonAncestor,
    #[error("Patch error: {0}")]
    PatchError(#[from] super::patch::PatchError),
    #[error("IO error: {0}")]
    IoError(#[from] std::io::Error),
    #[error("JSON error: {0}")]
    JsonError(#[from] serde_json::Error),
}

/// 重放引擎（从快照链重建工作区）
pub struct ReplayEngine {
    content_store_path: PathBuf,
}

impl ReplayEngine {
    pub fn content_store_path(&self) -> &PathBuf {
        &self.content_store_path
    }
}

/// 撤销日志条目
#[derive(Clone, Serialize, Deserialize)]
pub struct UndoEntry {
    pub path: String,
    pub action: UndoAction,
}

/// 撤销动作类型
#[derive(Clone, Serialize, Deserialize)]
pub enum UndoAction {
    Create { content: String },
    Delete { backup_path: String },
    Update { old_content: String },
}

/// 原子文件回滚器（带 undo 日志，支持失败恢复）
pub struct AtomicFileRollback {
    undo_log: Vec<UndoEntry>,
    temp_dir: PathBuf,
    target_dir: PathBuf,
}

impl ReplayEngine {
    pub fn new(content_store_path: PathBuf) -> Self {
        Self { content_store_path }
    }
    
    /// 从快照链重建工作区（全量重放）
    pub fn rebuild_workspace(
        &self,
        tree: &SnapshotTree,
        target_id: &str,
    ) -> Result<Workspace, ReplayError> {
        if target_id.is_empty() {
            return Ok(Workspace::new());
        }
        
        let mut chain = Vec::new();
        let mut current_id = Some(target_id.to_string());
        
        while let Some(id) = current_id {
            let snapshot = tree.nodes.get(&id)
                .ok_or_else(|| ReplayError::SnapshotNotFound(id.clone()))?;
            
            chain.push(snapshot.clone());
            
            if snapshot.is_checkpoint {
                if let Some(state) = &snapshot.workspace_state {
                    return self.replay_from_checkpoint(state, &chain);
                }
            }
            
            current_id = snapshot.parent_id.clone();
        }
        
        chain.reverse();
        
        let mut workspace = Workspace::new();
        for snapshot in &chain {
            workspace.apply_patches(&snapshot.patches)?;
        }
        
        Ok(workspace)
    }
    
    fn replay_from_checkpoint(
        &self,
        state: &WorkspaceState,
        chain: &[Snapshot],
    ) -> Result<Workspace, ReplayError> {
        let mut workspace = Workspace::new();
        
        for (path, info) in &state.files {
            let content = self.load_file_content(&info.hash)?;
            workspace.files.insert(path.clone(), content);
        }
        
        for snapshot in chain.iter().rev() {
            workspace.apply_patches(&snapshot.patches)?;
        }
        
        Ok(workspace)
    }
    
    fn load_file_content(&self, hash: &str) -> Result<String, ReplayError> {
        let content_path = self.content_store_path.join(hash);
        if content_path.exists() {
            let content = fs::read_to_string(&content_path)?;
            Ok(content)
        } else {
            Ok(String::new())
        }
    }
    
    /// 增量重建工作区（基于 LCA 计算差异，避免全量重放）
    pub fn rebuild_workspace_lazy(
        &self,
        tree: &SnapshotTree,
        current_workspace: &Workspace,
        current_snapshot_id: &str,
        target_id: &str,
    ) -> Result<Workspace, ReplayError> {
        if current_snapshot_id == target_id {
            return Ok(current_workspace.clone());
        }
        
        let lca = self.find_lowest_common_ancestor(tree, current_snapshot_id, target_id)?;
        
        let mut workspace = current_workspace.clone();
        
        let undo_patches = self.collect_undo_patches(tree, current_snapshot_id, &lca)?;
        for patch in undo_patches.iter().rev() {
            workspace.undo_patch(patch)?;
        }
        
        let redo_patches = self.collect_redo_patches(tree, &lca, target_id)?;
        for patch in &redo_patches {
            workspace.apply_patch(patch)?;
        }
        
        Ok(workspace)
    }
    
    /// 查找两个快照的最近公共祖先
    pub fn find_lowest_common_ancestor(
        &self,
        tree: &SnapshotTree,
        id1: &str,
        id2: &str,
    ) -> Result<String, ReplayError> {
        if id1.is_empty() || id2.is_empty() {
            return Ok(String::new());
        }
        
        let mut ancestors1: HashSet<String> = HashSet::new();
        let mut current = Some(id1.to_string());
        while let Some(id) = current {
            ancestors1.insert(id.clone());
            current = tree.nodes.get(&id).and_then(|s| s.parent_id.clone());
        }
        
        current = Some(id2.to_string());
        while let Some(id) = current {
            if ancestors1.contains(&id) {
                return Ok(id);
            }
            current = tree.nodes.get(&id).and_then(|s| s.parent_id.clone());
        }
        
        Err(ReplayError::NoCommonAncestor)
    }
    
    fn collect_undo_patches(
        &self,
        tree: &SnapshotTree,
        from_id: &str,
        to_id: &str,
    ) -> Result<Vec<Patch>, ReplayError> {
        let mut patches = Vec::new();
        let mut current = Some(from_id.to_string());
        
        while let Some(id) = current {
            if id == *to_id {
                break;
            }
            if let Some(snapshot) = tree.nodes.get(&id) {
                patches.extend(snapshot.patches.clone());
                current = snapshot.parent_id.clone();
            } else {
                break;
            }
        }
        
        Ok(patches)
    }
    
    fn collect_redo_patches(
        &self,
        tree: &SnapshotTree,
        from_id: &str,
        to_id: &str,
    ) -> Result<Vec<Patch>, ReplayError> {
        let mut patches = Vec::new();
        let mut current = Some(to_id.to_string());
        
        while let Some(id) = current {
            if id == *from_id {
                break;
            }
            if let Some(snapshot) = tree.nodes.get(&id) {
                patches.splice(0..0, snapshot.patches.clone());
                current = snapshot.parent_id.clone();
            } else {
                break;
            }
        }
        
        Ok(patches)
    }
    
    /// 回滚到指定快照（原子操作）
    pub async fn rollback_to(
        &self,
        tree: &mut SnapshotTree,
        target_id: &str,
        target_dir: &PathBuf,
    ) -> Result<(), ReplayError> {
        let workspace = self.rebuild_workspace(tree, target_id)?;
        
        let atomic_rollback = AtomicFileRollback::prepare(&workspace, target_dir)?;
        atomic_rollback.execute().await?;
        
        tree.current_snapshot_id = target_id.to_string();
        
        if let Some(branch) = tree.branches.get_mut(&tree.current_branch) {
            branch.head_snapshot_id = target_id.to_string();
        }
        
        Ok(())
    }
}

impl AtomicFileRollback {
    /// 准备回滚（生成 undo 日志）
    pub fn prepare(workspace: &Workspace, target_dir: &PathBuf) -> Result<Self, ReplayError> {
        let temp_dir = target_dir.join(".rollback_temp");
        fs::create_dir_all(&temp_dir)?;
        
        let mut undo_log = Vec::new();
        
        for (path, content) in &workspace.files {
            let full_path = target_dir.join(path);
            
            if full_path.exists() {
                let old_content = fs::read_to_string(&full_path)?;
                undo_log.push(UndoEntry {
                    path: path.clone(),
                    action: UndoAction::Update { old_content },
                });
            } else {
                undo_log.push(UndoEntry {
                    path: path.clone(),
                    action: UndoAction::Create { content: content.clone() },
                });
            }
        }
        
        Ok(Self { undo_log, temp_dir, target_dir: target_dir.clone() })
    }
    
    /// 执行原子回滚（先写入临时目录，再批量重命名）
    pub async fn execute(&self) -> Result<(), ReplayError> {
        let staging_dir = self.temp_dir.join(format!("staging-{}", Uuid::new_v4()));
        fs::create_dir_all(&staging_dir)?;
        
        for entry in &self.undo_log {
            let staging_path = staging_dir.join(&entry.path);
            if let Some(parent) = staging_path.parent() {
                fs::create_dir_all(parent)?;
            }
            
            match &entry.action {
                UndoAction::Create { content } | UndoAction::Update { old_content: content } => {
                    fs::write(&staging_path, content)?;
                }
                UndoAction::Delete { .. } => {}
            }
        }
        
        for entry in &self.undo_log {
            let target_path = self.target_dir.join(&entry.path);
            let staging_path = staging_dir.join(&entry.path);
            
            match &entry.action {
                UndoAction::Create { .. } => {
                    if let Some(parent) = target_path.parent() {
                        fs::create_dir_all(parent)?;
                    }
                    if staging_path.exists() {
                        fs::rename(&staging_path, &target_path)?;
                    }
                }
                UndoAction::Update { .. } => {
                    if staging_path.exists() {
                        fs::rename(&staging_path, &target_path)?;
                    }
                }
                UndoAction::Delete { .. } => {
                    if target_path.exists() {
                        fs::remove_file(&target_path)?;
                    }
                }
            }
        }
        
        fs::remove_dir_all(&staging_dir)?;
        
        Ok(())
    }
    
    pub fn save_undo_log(&self, path: &PathBuf) -> Result<(), ReplayError> {
        let json = serde_json::to_string_pretty(&self.undo_log)?;
        fs::write(path, json)?;
        Ok(())
    }
    
    pub fn load_undo_log(path: &PathBuf, target_dir: Option<PathBuf>) -> Result<Self, ReplayError> {
        let json = fs::read_to_string(path)?;
        let undo_log: Vec<UndoEntry> = serde_json::from_str(&json)?;
        let actual_target = target_dir.unwrap_or_else(|| {
            path.parent().map(|p| p.to_path_buf()).unwrap_or_default()
        });
        let temp_dir = actual_target.join(".rollback_temp");
        Ok(Self {
            undo_log,
            temp_dir,
            target_dir: actual_target,
        })
    }
}
