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
    #[error("Invalid rollback path: {0}")]
    InvalidPath(String),
    #[error("Rollback restore failed: {0}")]
    RestoreFailed(String),
}

/// 重放引擎（从快照链重建工作区）
pub struct ReplayEngine {
    session_id: String,
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
    Create {
        content: String,
    },
    Delete {
        backup_path: String,
    },
    Update {
        old_content: String,
        new_content: String,
    },
}

/// 原子文件回滚器（带 undo 日志，支持失败恢复）
pub struct AtomicFileRollback {
    undo_log: Vec<UndoEntry>,
    temp_dir: PathBuf,
    target_dir: PathBuf,
}

/// 带重试的文件操作（处理 Windows 文件锁，供回滚时使用）
async fn retry_fs_op<F, T>(op: F, max_retries: u32) -> Result<T, ReplayError>
where
    F: Fn() -> Result<T, std::io::Error>,
{
    let mut last_err = None;
    for attempt in 0..=max_retries {
        match op() {
            Ok(v) => return Ok(v),
            Err(e) => {
                let is_locked = e.kind() == std::io::ErrorKind::PermissionDenied
                    || e.raw_os_error() == Some(32)
                    || e.raw_os_error() == Some(5);
                if is_locked && attempt < max_retries {
                    println!(
                        "[ROLLBACK] File locked (attempt {}/{}), retrying in {}ms...",
                        attempt + 1, max_retries, 300 * (attempt + 1)
                    );
                    tokio::time::sleep(std::time::Duration::from_millis(
                        300 * (attempt as u64 + 1),
                    )).await;
                    last_err = Some(e);
                    continue;
                }
                return Err(ReplayError::IoError(e));
            }
        }
    }
    Err(ReplayError::IoError(last_err.unwrap()))
}

impl ReplayEngine {
    pub fn new(session_id: &str) -> Self {
        Self {
            session_id: session_id.to_string(),
        }
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
            let snapshot = tree
                .nodes
                .get(&id)
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

    /// 增量重建工作区：从前一个检查点的 workspace_state 出发，仅应用增量补丁
    /// 这避免了从根节点全量重放的 O(n) 开销
    pub fn rebuild_workspace_incremental(
        &self,
        tree: &SnapshotTree,
        target_id: &str,
    ) -> Result<Workspace, ReplayError> {
        if target_id.is_empty() {
            return Ok(Workspace::new());
        }

        let target = tree
            .nodes
            .get(target_id)
            .ok_or_else(|| ReplayError::SnapshotNotFound(target_id.to_string()))?;

        // 收集从 target 到上一个检查点的补丁链
        let mut patches_since_checkpoint = Vec::new();
        let mut current_id = target.parent_id.clone();
        let mut checkpoint_state: Option<&WorkspaceState> = None;

        while let Some(id) = current_id {
            let snapshot = tree
                .nodes
                .get(&id)
                .ok_or_else(|| ReplayError::SnapshotNotFound(id.clone()))?;

            if snapshot.is_checkpoint {
                checkpoint_state = snapshot.workspace_state.as_ref();
                break;
            }

            patches_since_checkpoint.splice(0..0, snapshot.patches.clone());
            current_id = snapshot.parent_id.clone();
        }

        let mut workspace = if let Some(state) = checkpoint_state {
            let mut ws = Workspace::new();
            for (path, info) in &state.files {
                let content = self.load_file_content(&info.hash)?;
                ws.files.insert(path.clone(), content);
            }
            ws
        } else {
            Workspace::new()
        };

        for patch in &patches_since_checkpoint {
            workspace.apply_patch(patch)?;
        }

        // 应用 target 自身的补丁（如果 target 有补丁）
        for patch in &target.patches {
            workspace.apply_patch(patch)?;
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
        super::store::load_content(&self.session_id, hash)
            .map_err(|err| {
                ReplayError::IoError(std::io::Error::new(std::io::ErrorKind::Other, err))
            })
            .map(|content| content.unwrap_or_default())
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

    fn collect_transition_patches(
        &self,
        tree: &SnapshotTree,
        current_id: &str,
        target_id: &str,
    ) -> Result<(Vec<Patch>, Vec<Patch>), ReplayError> {
        if current_id == target_id {
            return Ok((Vec::new(), Vec::new()));
        }

        let lca = self.find_lowest_common_ancestor(tree, current_id, target_id)?;
        let undo_patches = self.collect_undo_patches(tree, current_id, &lca)?;
        let redo_patches = self.collect_redo_patches(tree, &lca, target_id)?;

        Ok((undo_patches, redo_patches))
    }

    pub fn preview_touched_files(
        &self,
        tree: &SnapshotTree,
        target_id: &str,
    ) -> Result<Vec<String>, ReplayError> {
        let current_id = tree.current_snapshot_id.clone();
        let (undo_patches, redo_patches) =
            self.collect_transition_patches(tree, &current_id, target_id)?;
        let mut paths: Vec<String> = undo_patches
            .iter()
            .chain(redo_patches.iter())
            .flat_map(|patch| patch.touched_paths().into_iter().map(str::to_string))
            .collect();
        paths.sort();
        paths.dedup();
        Ok(paths)
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
    /// 准备回滚（生成 undo 日志），支持相对路径（工作区）和绝对路径（默认会话）
    pub fn prepare(workspace: &Workspace, target_dir: &PathBuf) -> Result<Self, ReplayError> {
        let temp_dir = if target_dir.as_os_str().is_empty() {
            std::env::temp_dir().join(format!("jarvis_rollback_{}", Uuid::new_v4()))
        } else {
            target_dir.join(".rollback_temp")
        };
        fs::create_dir_all(&temp_dir)?;

        use std::collections::HashSet;

        let mut undo_log = Vec::new();
        let mut target_files = HashSet::new();
        let has_target_dir = !target_dir.as_os_str().is_empty();

        for (path, content) in &workspace.files {
            target_files.insert(path.clone());
            let full_path = if PathBuf::from(path).is_absolute() {
                PathBuf::from(path)
            } else {
                target_dir.join(path)
            };

            if full_path.exists() {
                let old_content = fs::read_to_string(&full_path)?;
                undo_log.push(UndoEntry {
                    path: path.clone(),
                    action: UndoAction::Update {
                        old_content,
                        new_content: content.clone(),
                    },
                });
            } else {
                undo_log.push(UndoEntry {
                    path: path.clone(),
                    action: UndoAction::Create {
                        content: content.clone(),
                    },
                });
            }
        }

        // 只在相对路径模式下扫描目标目录中需要删除的文件
        if has_target_dir {
            for entry in fs::read_dir(target_dir)? {
                let entry = entry?;
                if !entry.file_type()?.is_file() {
                    continue;
                }
                let path = entry.file_name().to_string_lossy().to_string();
                if !target_files.contains(&path) && path != ".rollback_temp" {
                    undo_log.push(UndoEntry {
                        path,
                        action: UndoAction::Delete {
                            backup_path: String::new(),
                        },
                    });
                }
            }
        }

        Ok(Self {
            undo_log,
            temp_dir,
            target_dir: target_dir.clone(),
        })
    }

    fn resolve_target_path(&self, path: &str) -> PathBuf {
        let p = PathBuf::from(path);
        if p.is_absolute() {
            p
        } else {
            self.target_dir.join(path)
        }
    }

    /// 执行原子回滚（先写入临时目录，再批量重命名，遇文件锁自动重试）
    pub async fn execute(&self) -> Result<(), ReplayError> {
        let staging_dir = self.temp_dir.join(format!("staging-{}", Uuid::new_v4()));
        fs::create_dir_all(&staging_dir)?;

        for entry in &self.undo_log {
            let file_name = PathBuf::from(&entry.path)
                .file_name()
                .map(|n| n.to_string_lossy().to_string())
                .unwrap_or_else(|| entry.path.clone());
            let staging_path = staging_dir.join(&file_name);
            if let Some(parent) = staging_path.parent() {
                fs::create_dir_all(parent)?;
            }

            match &entry.action {
                UndoAction::Create { content } => {
                    let content_clone = content.clone();
                    let p = staging_path.clone();
                    retry_fs_op(
                        || std::fs::write(&p, &content_clone),
                        3,
                    )
                    .await?;
                }
                UndoAction::Update { new_content, .. } => {
                    let content_clone = new_content.clone();
                    let p = staging_path.clone();
                    retry_fs_op(
                        || std::fs::write(&p, &content_clone),
                        3,
                    )
                    .await?;
                }
                UndoAction::Delete { .. } => {}
            }
        }

        for entry in &self.undo_log {
            let file_name = PathBuf::from(&entry.path)
                .file_name()
                .map(|n| n.to_string_lossy().to_string())
                .unwrap_or_else(|| entry.path.clone());
            let staging_path = staging_dir.join(&file_name);
            let target_path = self.resolve_target_path(&entry.path);

            match &entry.action {
                UndoAction::Create { .. } => {
                    if let Some(parent) = target_path.parent() {
                        fs::create_dir_all(parent)?;
                    }
                    if staging_path.exists() {
                        let sp = staging_path.clone();
                        let tp = target_path.clone();
                        retry_fs_op(
                            || std::fs::rename(&sp, &tp),
                            5,
                        )
                        .await?;
                    }
                }
                UndoAction::Update { .. } => {
                    if staging_path.exists() {
                        let sp = staging_path.clone();
                        let tp = target_path.clone();
                        retry_fs_op(
                            || std::fs::rename(&sp, &tp),
                            5,
                        )
                        .await?;
                    }
                }
                UndoAction::Delete { .. } => {
                    if target_path.exists() {
                        let tp = target_path.clone();
                        retry_fs_op(
                            || std::fs::remove_file(&tp),
                            5,
                        )
                        .await?;
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
        let actual_target = target_dir
            .unwrap_or_else(|| path.parent().map(|p| p.to_path_buf()).unwrap_or_default());
        let temp_dir = actual_target.join(".rollback_temp");
        Ok(Self {
            undo_log,
            temp_dir,
            target_dir: actual_target,
        })
    }
}
