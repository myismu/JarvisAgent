//! 重放引擎模块
//!
//! 从快照链重建工作区状态，支持：
//! - 全量重放（从头重建）
//! - 增量重放（基于当前状态的 LCA 差异计算）
//! - 原子回滚（带 undo 日志的文件级回滚）

use super::patch::{Patch, PatchSummary};
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
    session_id: String,
    /// 需要从回收站搬回原位的对象（原路径 → 回收站位置）。
    ///
    /// 走的是**并列于 `undo_log` 的第二条恢复通道**：`undo_log` 只能表达"写某段文本内容"，
    /// 而目录 / 二进制 / 大文件（快照里 `content_hash = None`）表达不了，
    /// 只能把本体从回收站搬回来。见 `execute()` 的恢复阶段。
    trash_restores: std::collections::HashMap<String, String>,
}

/// 收集整棵树里**所有补丁涉及过的路径**（归一成比较用的形态）。
///
/// 用途：回滚时清理残留的判据必须是"补丁历史知道这条路径"，而不是"它不在目标状态里"。
/// 后者会把项目里 agent **从未碰过**的文件一并删掉 —— 2026-09-21 实测事故：
/// 一次「撤回会话和代码」把工作区根目录的 `README.md` / `package.json` 全删了
/// （目录因不参与扫盘而幸存）。
pub(crate) fn known_paths_in_tree(tree: &SnapshotTree) -> std::collections::HashSet<String> {
    let mut set = std::collections::HashSet::new();
    for node in tree.nodes.values() {
        for patch in &node.patches {
            for path in patch.touched_paths() {
                set.insert(path_key(path));
            }
        }
    }
    set
}

/// 路径比较用的归一形态（统一分隔符）。
///
/// 补丁里存的是绝对路径（`C:\...\README.md`），`read_dir` 给出的也是绝对路径，
/// 但两者的分隔符与结尾斜杠不保证一致，比较前必须归一。
/// 曾经的写法更糟：拿 `read_dir` 的**裸文件名**去比补丁里的**绝对路径**，恒不匹配。
fn path_key(path: &str) -> String {
    path.replace('\\', "/").trim_end_matches('/').to_string()
}

/// 一次回滚的执行结果。
#[derive(Debug, Default, Clone)]
pub struct RollbackOutcome {
    /// 没能从回收站恢复的对象（快照无内容 **且** 回收站副本缺失）。
    ///
    /// 非空意味着**工作区与目标快照并不完全一致**。这必须一路带到界面上告诉用户 ——
    /// "静默的不一致"比"明确的失败"更糟，这正是本次打通的起因。
    pub restore_failures: Vec<String>,
}

/// 回滚所需的、`prepare` 自己拿不到的信息（都从快照树推导）。
///
/// 抽成结构体而不是继续加位置参数：两个字段来路相同（都由树推导），
/// 而且以后大概率还会长出同类字段。
pub struct RollbackContext {
    /// 补丁历史涉及过的全部路径（归一形态）—— 清理残留的判据，见 [`known_paths_in_tree`]
    pub known_paths: std::collections::HashSet<String>,
    /// 需要从回收站搬回原位的对象（原路径 → 回收站位置），见 [`trash_restores_from_patches`]
    pub trash_restores: std::collections::HashMap<String, String>,
}

/// 从一组补丁里挑出"快照装不下本体、只能靠回收站恢复"的删除（原路径 → 回收站位置）。
///
/// 进这个集合必须同时满足：`content_hash` 为空（内容读不出来 = 目录 / 二进制 / 大文件）
/// **且** 有 `trash_path`。文本文件不走这里 —— 它们的内容在快照里，回滚靠写内容恢复。
pub(crate) fn trash_restores_from_patches<'a>(
    patches: impl Iterator<Item = &'a Patch>,
) -> std::collections::HashMap<String, String> {
    let mut map = std::collections::HashMap::new();
    for patch in patches {
        if let Patch::DeleteFile {
            path,
            content_hash,
            trash_path,
        } = patch
        {
            if content_hash.is_none() {
                if let Some(trash) = trash_path {
                    map.insert(path.clone(), trash.clone());
                }
            }
        }
    }
    map
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
                        attempt + 1,
                        max_retries,
                        300 * (attempt + 1)
                    );
                    tokio::time::sleep(std::time::Duration::from_millis(
                        300 * (attempt as u64 + 1),
                    ))
                    .await;
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
            workspace.apply_patches(&snapshot.patches, &tree.session_id)?;
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
            workspace.apply_patch(patch, &tree.session_id)?;
        }

        // 应用 target 自身的补丁（如果 target 有补丁）
        for patch in &target.patches {
            workspace.apply_patch(patch, &tree.session_id)?;
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
            workspace.apply_patches(&snapshot.patches, &self.session_id)?;
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
    ) -> Result<Vec<PatchSummary>, ReplayError> {
        let current_id = tree.current_snapshot_id.clone();
        let (undo_patches, redo_patches) =
            self.collect_transition_patches(tree, &current_id, target_id)?;
        let mut summaries: Vec<PatchSummary> = undo_patches
            .iter()
            .chain(redo_patches.iter())
            .map(|patch| patch.to_summary())
            .collect();
        summaries.sort_by(|a, b| a.path.cmp(&b.path));
        summaries.dedup_by(|a, b| a.path == b.path);
        Ok(summaries)
    }

    /// 收集"从当前位置回退到目标快照"这段里、需要靠回收站恢复本体的对象。
    ///
    /// 为什么不交给 `undo_patch` 去处理：回滚的工作区是**从根重建**的（只应用目标点之前的
    /// 补丁），被撤销的那段补丁根本不参与重建，于是模型里永远不会冒出"这里本该有个目录"
    /// 这样的信号。所以恢复清单必须在这里、拿"被撤销的那段补丁"单独算出来。
    fn trash_restores_between(
        &self,
        tree: &SnapshotTree,
        target_id: &str,
    ) -> Result<std::collections::HashMap<String, String>, ReplayError> {
        let current_id = tree.current_snapshot_id.clone();
        let lca = self.find_lowest_common_ancestor(tree, &current_id, target_id)?;
        let undo_patches = self.collect_undo_patches(tree, &current_id, &lca)?;
        Ok(trash_restores_from_patches(undo_patches.iter()))
    }

    /// 回滚到初始状态（用每个文件的最早内容重建）
    pub async fn rollback_to_initial_state(
        &self,
        workspace: &Workspace,
        target_dir: &PathBuf,
        ctx: &RollbackContext,
    ) -> Result<RollbackOutcome, ReplayError> {
        let atomic_rollback =
            AtomicFileRollback::prepare(workspace, target_dir, &self.session_id, ctx)?;
        atomic_rollback.execute().await
    }

    /// 回滚到指定快照（原子操作）。返回执行结果（含"未能从回收站恢复的对象"，见
    /// [`RollbackOutcome`]）—— 调用方要把它一路带到界面上。
    pub async fn rollback_to(
        &self,
        tree: &mut SnapshotTree,
        target_id: &str,
        target_dir: &PathBuf,
    ) -> Result<RollbackOutcome, ReplayError> {
        let workspace = self.rebuild_workspace(tree, target_id)?;
        let ctx = RollbackContext {
            known_paths: known_paths_in_tree(tree),
            trash_restores: self.trash_restores_between(tree, target_id)?,
        };

        let atomic_rollback =
            AtomicFileRollback::prepare(&workspace, target_dir, &self.session_id, &ctx)?;
        let outcome = atomic_rollback.execute().await?;

        tree.current_snapshot_id = target_id.to_string();

        if let Some(branch) = tree.branches.get_mut(&tree.current_branch) {
            branch.head_snapshot_id = target_id.to_string();
        }

        Ok(outcome)
    }
}

impl AtomicFileRollback {
    /// 准备回滚（生成 undo 日志），支持相对路径（工作区）和绝对路径（默认会话）。
    ///
    /// `ctx` 携带 `prepare` 自己拿不到、需要从快照树推导的两样东西：
    /// 清理残留的判据（[`RollbackContext::known_paths`]）与回收站恢复清单
    /// （[`RollbackContext::trash_restores`]）。
    pub fn prepare(
        workspace: &Workspace,
        target_dir: &PathBuf,
        session_id: &str,
        ctx: &RollbackContext,
    ) -> Result<Self, ReplayError> {
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
            let full_path = if PathBuf::from(path).is_absolute() {
                PathBuf::from(path)
            } else {
                target_dir.join(path)
            };
            // 目标集合登记**绝对路径的归一形态**：扫盘拿到的是绝对路径，两边必须同形态才能比
            // （见 `path_key` 的说明）。
            target_files.insert(path_key(&full_path.to_string_lossy()));

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

        // 清理工作区根目录的残留：**只清理"补丁历史里出现过、且目标状态里不该有"的普通文件**
        // （典型场景：回滚点之后新建的文件）。
        //
        // 判据为什么必须带上 `known_paths`（2026-09-21 实测事故）：只判"不在目标状态里就删"
        // 会把项目里 agent **从未碰过**的文件一起删掉 —— 工作区模型是从补丁链重建的，
        // `target_files` 天然只含"被工具改过"的路径，于是 `README.md` / `package.json`
        // 这类项目原有文件会被当成"该删的"；再叠加当时用裸文件名去比绝对路径（恒不匹配），
        // 一次「撤回会话和代码」就把根目录普通文件清空了。
        //
        // 只处理根目录一层的普通文件：目录不递归（`is_file()` 已挡掉），
        // `.rollback_temp` 是目录，同样不受影响。
        if has_target_dir {
            for entry in fs::read_dir(target_dir)? {
                let entry = entry?;
                if !entry.file_type()?.is_file() {
                    continue;
                }
                let full = entry.path();
                let key = path_key(&full.to_string_lossy());
                if ctx.known_paths.contains(&key) && !target_files.contains(&key) {
                    let full = full.to_string_lossy().to_string();
                    undo_log.push(UndoEntry {
                        path: full.clone(),
                        action: UndoAction::Delete { backup_path: full },
                    });
                }
            }
        }

        // 精准删除孤儿文件（如 RenameFile 留下的旧/新路径）
        for path in &workspace.delete_paths {
            let exist_path = if PathBuf::from(path).is_absolute() {
                PathBuf::from(path)
            } else {
                target_dir.join(path)
            };
            if exist_path.exists() {
                undo_log.push(UndoEntry {
                    path: if has_target_dir {
                        path.clone()
                    } else {
                        exist_path.to_string_lossy().to_string()
                    },
                    action: UndoAction::Delete {
                        backup_path: exist_path.to_string_lossy().to_string(),
                    },
                });
            }
        }

        Ok(Self {
            undo_log,
            temp_dir,
            target_dir: target_dir.clone(),
            session_id: session_id.to_string(),
            trash_restores: ctx.trash_restores.clone(),
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
    pub async fn execute(&self) -> Result<RollbackOutcome, ReplayError> {
        use super::rollback_logger::{
            self, FileAction, FileOpRecord, FileOpStatus, RollbackPhase, RollbackSummary,
        };

        let logger = rollback_logger::rollback_logger();
        let staging_dir = self.temp_dir.join(format!("staging-{}", Uuid::new_v4()));
        fs::create_dir_all(&staging_dir)?;

        let total = self.undo_log.len() as u32;
        let rollback_start = std::time::Instant::now();
        let mut seq: u32 = 0;
        let mut success_count: u32 = 0;
        let mut skip_count: u32 = 0;
        let mut failed_files: Vec<String> = Vec::new();
        // 只收"回收站恢复失败"这一类：它们要一路带到 UI（见 `RollbackOutcome`）
        let mut restore_failures: Vec<String> = Vec::new();

        // Phase 1: 写入 staging 目录
        for entry in &self.undo_log {
            seq += 1;

            let (action, size) = match &entry.action {
                UndoAction::Create { content } => (FileAction::Create, Some(content.len() as u64)),
                UndoAction::Update { new_content, .. } => {
                    (FileAction::Update, Some(new_content.len() as u64))
                }
                UndoAction::Delete { .. } => (FileAction::Delete, None),
            };

            // Delete 不需要 staging，且必须**在算 staging 路径之前**返回：
            // 待删条目的 path 现在是绝对路径，而 `PathBuf::join(绝对路径)` 会直接替换掉前缀
            // —— 那句 create_dir_all 就会跑去 staging 目录之外建父目录。
            if matches!(entry.action, UndoAction::Delete { .. }) {
                logger.log_file_op(&FileOpRecord {
                    ts: chrono::Utc::now().to_rfc3339(),
                    session_id: self.session_id.clone(),
                    rollback_seq: seq,
                    phase: RollbackPhase::Staging,
                    path: entry.path.clone(),
                    action,
                    status: FileOpStatus::Skip,
                    size_bytes: None,
                    error: None,
                    duration_ms: 0,
                });
                continue;
            }

            // 写 staging（仅 Create / Update 会走到这里）
            let staging_path = staging_dir.join(&entry.path);
            if let Some(parent) = staging_path.parent() {
                fs::create_dir_all(parent)?;
            }

            let op_start = std::time::Instant::now();
            let content = match &entry.action {
                UndoAction::Create { content } => content.clone(),
                UndoAction::Update { new_content, .. } => new_content.clone(),
                UndoAction::Delete { .. } => unreachable!(),
            };
            let p = staging_path.clone();
            let result = retry_fs_op(|| std::fs::write(&p, &content), 3).await;
            let duration_ms = op_start.elapsed().as_millis() as u64;

            match result {
                Ok(_) => {
                    logger.log_file_op(&FileOpRecord {
                        ts: chrono::Utc::now().to_rfc3339(),
                        session_id: self.session_id.clone(),
                        rollback_seq: seq,
                        phase: RollbackPhase::Staging,
                        path: entry.path.clone(),
                        action,
                        status: FileOpStatus::Ok,
                        size_bytes: size,
                        error: None,
                        duration_ms,
                    });
                }
                Err(e) => {
                    let err_msg = e.to_string();
                    logger.log_file_op(&FileOpRecord {
                        ts: chrono::Utc::now().to_rfc3339(),
                        session_id: self.session_id.clone(),
                        rollback_seq: seq,
                        phase: RollbackPhase::Staging,
                        path: entry.path.clone(),
                        action,
                        status: FileOpStatus::Error,
                        size_bytes: size,
                        error: Some(err_msg),
                        duration_ms,
                    });
                    return Err(e);
                }
            }
        }

        // Phase 2: 从 staging 应用到目标
        for entry in &self.undo_log {
            seq += 1;
            let staging_path = staging_dir.join(&entry.path);
            let target_path = self.resolve_target_path(&entry.path);

            let action = match &entry.action {
                UndoAction::Create { .. } => FileAction::Create,
                UndoAction::Update { .. } => FileAction::Update,
                UndoAction::Delete { .. } => FileAction::Delete,
            };

            let op_start = std::time::Instant::now();

            match &entry.action {
                UndoAction::Create { .. } | UndoAction::Update { .. } => {
                    if !staging_path.exists() {
                        skip_count += 1;
                        logger.log_file_op(&FileOpRecord {
                            ts: chrono::Utc::now().to_rfc3339(),
                            session_id: self.session_id.clone(),
                            rollback_seq: seq,
                            phase: RollbackPhase::Apply,
                            path: entry.path.clone(),
                            action,
                            status: FileOpStatus::Skip,
                            size_bytes: None,
                            error: Some("staging file not found".to_string()),
                            duration_ms: 0,
                        });
                        continue;
                    }
                    if matches!(entry.action, UndoAction::Create { .. }) {
                        if let Some(parent) = target_path.parent() {
                            fs::create_dir_all(parent)?;
                        }
                    }
                    let sp = staging_path.clone();
                    let tp = target_path.clone();
                    match retry_fs_op(|| std::fs::rename(&sp, &tp), 5).await {
                        Ok(_) => {
                            success_count += 1;
                            logger.log_file_op(&FileOpRecord {
                                ts: chrono::Utc::now().to_rfc3339(),
                                session_id: self.session_id.clone(),
                                rollback_seq: seq,
                                phase: RollbackPhase::Apply,
                                path: entry.path.clone(),
                                action,
                                status: FileOpStatus::Ok,
                                size_bytes: None,
                                error: None,
                                duration_ms: op_start.elapsed().as_millis() as u64,
                            });
                        }
                        Err(e) => {
                            let err_msg = e.to_string();
                            failed_files.push(entry.path.clone());
                            logger.log_file_op(&FileOpRecord {
                                ts: chrono::Utc::now().to_rfc3339(),
                                session_id: self.session_id.clone(),
                                rollback_seq: seq,
                                phase: RollbackPhase::Apply,
                                path: entry.path.clone(),
                                action,
                                status: FileOpStatus::Error,
                                size_bytes: None,
                                error: Some(err_msg),
                                duration_ms: op_start.elapsed().as_millis() as u64,
                            });
                            return Err(e);
                        }
                    }
                }
                UndoAction::Delete { .. } => {
                    if target_path.exists() {
                        let tp = target_path.clone();
                        match retry_fs_op(|| std::fs::remove_file(&tp), 5).await {
                            Ok(_) => {
                                success_count += 1;
                                logger.log_file_op(&FileOpRecord {
                                    ts: chrono::Utc::now().to_rfc3339(),
                                    session_id: self.session_id.clone(),
                                    rollback_seq: seq,
                                    phase: RollbackPhase::Apply,
                                    path: entry.path.clone(),
                                    action,
                                    status: FileOpStatus::Ok,
                                    size_bytes: None,
                                    error: None,
                                    duration_ms: op_start.elapsed().as_millis() as u64,
                                });
                            }
                            Err(e) => {
                                let err_msg = e.to_string();
                                failed_files.push(entry.path.clone());
                                logger.log_file_op(&FileOpRecord {
                                    ts: chrono::Utc::now().to_rfc3339(),
                                    session_id: self.session_id.clone(),
                                    rollback_seq: seq,
                                    phase: RollbackPhase::Apply,
                                    path: entry.path.clone(),
                                    action,
                                    status: FileOpStatus::Error,
                                    size_bytes: None,
                                    error: Some(err_msg),
                                    duration_ms: op_start.elapsed().as_millis() as u64,
                                });
                                return Err(e);
                            }
                        }
                    } else {
                        skip_count += 1;
                        logger.log_file_op(&FileOpRecord {
                            ts: chrono::Utc::now().to_rfc3339(),
                            session_id: self.session_id.clone(),
                            rollback_seq: seq,
                            phase: RollbackPhase::Apply,
                            path: entry.path.clone(),
                            action,
                            status: FileOpStatus::Skip,
                            size_bytes: None,
                            error: Some("target file not found".to_string()),
                            duration_ms: 0,
                        });
                    }
                }
            }
        }

        // Phase 3: 从回收站搬回"快照装不下本体"的对象（目录 / 二进制 / 大文件）
        //
        // 这些对象在目标状态里本就该存在，只是 `Workspace`（路径 → 文本内容）表达不出来，
        // 所以恢复动作是**文件系统层搬运**，而不是写内容 —— 这是与 `undo_log` 并列的
        // 第二条恢复通道（2026-09-21 打通；此前这类删除在回滚时被静默忽略）。
        //
        // 失败不中止整次回滚：回收站副本可能已被人工清掉，那属于"本来就恢复不了"，
        // 如实记进 `failed_files`（汇总会带出去）比让整次回滚失败更有用。
        for (path, trash) in &self.trash_restores {
            let target_path = self.resolve_target_path(path);
            if target_path.exists() {
                continue; // 已在位（例如同名文件已由快照通道写回）→ 不重复搬
            }
            seq += 1;
            let op_start = std::time::Instant::now();
            let trash_path = PathBuf::from(trash);
            let (ok, error): (bool, Option<String>) = if !trash_path.exists() {
                (
                    false,
                    Some(format!(
                        "快照无内容且回收站副本不存在（{}）——该对象无法恢复",
                        trash_path.display()
                    )),
                )
            } else {
                match super::trash::move_to(&trash_path, &target_path) {
                    Ok(()) => (true, None),
                    Err(e) => (false, Some(e.to_string())),
                }
            };
            if ok {
                success_count += 1;
            } else {
                failed_files.push(path.clone());
                restore_failures.push(path.clone());
                eprintln!(
                    "[ROLLBACK] 从回收站恢复失败: {} ← {}（{}）",
                    path,
                    trash_path.display(),
                    error.clone().unwrap_or_default()
                );
            }
            logger.log_file_op(&FileOpRecord {
                ts: chrono::Utc::now().to_rfc3339(),
                session_id: self.session_id.clone(),
                rollback_seq: seq,
                phase: RollbackPhase::Apply,
                path: path.clone(),
                action: FileAction::Create,
                status: if ok {
                    FileOpStatus::Ok
                } else {
                    FileOpStatus::Error
                },
                size_bytes: None,
                error,
                duration_ms: op_start.elapsed().as_millis() as u64,
            });
        }

        // 写入汇总
        logger.flush_summary(&RollbackSummary {
            ts: chrono::Utc::now().to_rfc3339(),
            session_id: self.session_id.clone(),
            total,
            success: success_count,
            skipped: skip_count,
            failed: failed_files.len() as u32,
            total_duration_ms: rollback_start.elapsed().as_millis() as u64,
            failed_files,
        });

        fs::remove_dir_all(&staging_dir)?;
        let _ = fs::remove_dir_all(&self.temp_dir);

        Ok(RollbackOutcome { restore_failures })
    }

    pub fn save_undo_log(&self, path: &PathBuf) -> Result<(), ReplayError> {
        let json = serde_json::to_string_pretty(&self.undo_log)?;
        fs::write(path, json)?;
        Ok(())
    }

    pub fn load_undo_log(
        path: &PathBuf,
        target_dir: Option<PathBuf>,
        session_id: &str,
    ) -> Result<Self, ReplayError> {
        let json = fs::read_to_string(path)?;
        let undo_log: Vec<UndoEntry> = serde_json::from_str(&json)?;
        let actual_target = target_dir
            .unwrap_or_else(|| path.parent().map(|p| p.to_path_buf()).unwrap_or_default());
        let temp_dir = actual_target.join(".rollback_temp");
        Ok(Self {
            undo_log,
            temp_dir,
            target_dir: actual_target,
            session_id: session_id.to_string(),
            // 从序列化的 undo 日志恢复时没有回收站清单 —— 那条通道只存在于"当场算出"的
            // 回滚上下文里（见 `RollbackContext`）。
            trash_restores: std::collections::HashMap::new(),
        })
    }
}

#[cfg(test)]
mod sweep_tests {
    //! 回滚"扫盘清理"的护栏。
    //!
    //! 背景（2026-09-21 实测事故）：一次「撤回会话和代码」把工作区根目录的普通文件全删了。
    //! 两个原因叠加：① 判据是"不在目标状态里就删"，而目标状态（从补丁链重建）天然只含
    //! "被工具改过"的路径 → 项目原有文件被当成该删的；② 比较用**裸文件名**去比补丁里的
    //! **绝对路径** → 恒不匹配。
    //!
    //! 现在的判据是"补丁历史里出现过 **且** 目标状态里没有"，下面两条测试各锁一头。

    use super::*;

    fn init_agent_home() {
        let dir = std::env::temp_dir().join("jarvisagent-rollback-test-data");
        let _ = std::fs::create_dir_all(&dir);
        let _ = crate::AGENT_HOME_DIR.set(dir);
    }

    fn fresh_root(tag: &str) -> PathBuf {
        let root = std::env::temp_dir().join(format!("jarvis-rollback-{tag}"));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(root.join("src")).unwrap();
        root
    }

    fn known(paths: &[&std::path::Path]) -> std::collections::HashSet<String> {
        paths
            .iter()
            .map(|p| path_key(&p.to_string_lossy()))
            .collect()
    }

    /// 只带"清理判据"的回滚上下文（回收站恢复清单为空）
    fn ctx_known_only(paths: &[&std::path::Path]) -> RollbackContext {
        RollbackContext {
            known_paths: known(paths),
            trash_restores: std::collections::HashMap::new(),
        }
    }

    /// agent 从未碰过的文件必须原样保留（事故的正面锁）。
    #[test]
    fn rollback_keeps_untouched_files() {
        init_agent_home();
        let root = fresh_root("keeps");
        std::fs::write(root.join("README.md"), "项目自带").unwrap();
        std::fs::write(root.join("package.json"), "{}").unwrap();
        let edited = root.join("src").join("app.js");
        std::fs::write(&edited, "old").unwrap();

        // 目标状态只有被改过的那个文件；补丁历史也只涉及它
        let mut ws = Workspace::new();
        ws.files
            .insert(edited.to_string_lossy().to_string(), "new".to_string());
        let ctx = ctx_known_only(&[&edited]);

        let rb = AtomicFileRollback::prepare(&ws, &root, "s", &ctx).expect("prepare");
        tauri::async_runtime::block_on(rb.execute()).expect("execute");

        assert!(
            root.join("README.md").exists(),
            "从未被工具碰过的文件被删了"
        );
        assert!(
            root.join("package.json").exists(),
            "从未被工具碰过的文件被删了"
        );
        assert_eq!(
            std::fs::read_to_string(&edited).unwrap(),
            "new",
            "目标文件应被写回"
        );
    }

    /// 反向锁：补丁历史里出现过、但目标状态里没有的根文件（回滚点之后新建的）应被清掉
    /// —— 原意必须保留，别把这条判据一起废掉。
    #[test]
    fn rollback_removes_files_created_after_target() {
        init_agent_home();
        let root = fresh_root("removes");
        let created = root.join("later.tmp.js");
        std::fs::write(&created, "agent 后来建的").unwrap();

        let ws = Workspace::new(); // 目标状态：空
        let ctx = ctx_known_only(&[&created]); // 但它出现在补丁历史里

        let rb = AtomicFileRollback::prepare(&ws, &root, "s", &ctx).expect("prepare");
        tauri::async_runtime::block_on(rb.execute()).expect("execute");

        assert!(!created.exists(), "回滚点之后新建的文件应被清掉");
    }

    /// 目录 / 二进制 / 大文件：快照里没有内容（`content_hash = None`），
    /// 回滚时必须**从回收站把本体搬回原位** —— 这是 2026-09-21 打通的第二条恢复通道。
    #[test]
    fn rollback_restores_objects_from_trash_when_snapshot_has_no_content() {
        init_agent_home();
        let root = fresh_root("restore");
        let deleted_dir = root.join("tmp-e2e");
        assert!(!deleted_dir.exists(), "前置：该目录当前处于已删除状态");

        // 回收站里躺着它的本体（含嵌套内容，验证搬回的是"整棵"）
        let trash_dir = std::env::temp_dir().join("jarvis-rollback-restore-trash");
        let _ = std::fs::remove_dir_all(&trash_dir);
        let trash_copy = trash_dir.join("1789944415_tmp-e2e");
        std::fs::create_dir_all(trash_copy.join("nested")).unwrap();
        std::fs::write(trash_copy.join("nested").join("data.json"), "{}").unwrap();

        // 目标状态：目录进不了 `files`（那是"路径 → 文本内容"），只能靠回收站清单
        let ws = Workspace::new();
        let ctx = RollbackContext {
            known_paths: known(&[&deleted_dir]),
            trash_restores: std::collections::HashMap::from([(
                deleted_dir.to_string_lossy().to_string(),
                trash_copy.to_string_lossy().to_string(),
            )]),
        };

        let rb = AtomicFileRollback::prepare(&ws, &root, "s", &ctx).expect("prepare");
        tauri::async_runtime::block_on(rb.execute()).expect("execute");

        assert!(deleted_dir.exists(), "目录应从回收站被搬回原位");
        assert_eq!(
            std::fs::read_to_string(deleted_dir.join("nested").join("data.json")).unwrap(),
            "{}",
            "搬回的应是整棵目录"
        );
    }

    /// 回收站副本已不在（被人工清掉）时：不中止回滚，但要如实记进失败清单，不静默。
    #[test]
    fn missing_trash_copy_is_reported_not_silently_ignored() {
        init_agent_home();
        let root = fresh_root("missing-trash");
        let ws = Workspace::new();
        let missing = root.join("gone");
        let ctx = RollbackContext {
            known_paths: known(&[&missing]),
            trash_restores: std::collections::HashMap::from([(
                missing.to_string_lossy().to_string(),
                root.join("no-such-trash-entry")
                    .to_string_lossy()
                    .to_string(),
            )]),
        };

        let rb = AtomicFileRollback::prepare(&ws, &root, "s", &ctx).expect("prepare");
        // 关键 1：整次回滚**不该**因为一个恢复不了的旧对象而失败
        let outcome = tauri::async_runtime::block_on(rb.execute()).expect("execute 不应报错");
        assert!(!missing.exists());
        // 关键 2：失败必须被带出去（一路到 UI），不能只写日志
        assert_eq!(
            outcome.restore_failures,
            vec![missing.to_string_lossy().to_string()],
            "未能恢复的对象必须出现在执行结果里"
        );
    }

    /// 归一化：反斜杠与正斜杠、结尾斜杠都要能对上（否则又是"恒不匹配"）。
    #[test]
    fn path_key_normalizes_separators_and_trailing_slash() {
        assert_eq!(path_key(r"C:\ws\README.md"), path_key("C:/ws/README.md"));
        assert_eq!(path_key("C:/ws/"), "C:/ws");
    }

    /// 历史路径集合必须覆盖四种补丁（改名的**新旧两个路径**都要在）。
    #[test]
    fn known_paths_covers_all_patch_kinds() {
        let mut tree = SnapshotTree::new("s");
        let _ = tree.create_snapshot(
            vec![
                Patch::CreateFile {
                    path: "C:/ws/a.js".into(),
                    content: "a".into(),
                },
                Patch::DeleteFile {
                    path: "C:/ws/b.js".into(),
                    content_hash: None,
                    trash_path: Some("C:/data/trash/s/1_b.js".into()),
                },
                Patch::UpdateFile {
                    path: "C:/ws/c.js".into(),
                    old_content: "o".into(),
                    new_content: "n".into(),
                    diff: None,
                    content_hash: None,
                },
                Patch::RenameFile {
                    old_path: "C:/ws/d.js".into(),
                    new_path: "C:/ws/e.js".into(),
                },
            ],
            None,
            None,
            None,
            false,
            None,
        );

        let set = known_paths_in_tree(&tree);
        for p in [
            "C:/ws/a.js",
            "C:/ws/b.js",
            "C:/ws/c.js",
            "C:/ws/d.js",
            "C:/ws/e.js",
        ] {
            assert!(set.contains(p), "已知路径集合缺少 {p}：{set:?}");
        }
    }
}
