//! 会话快照管理器
//!
//! 管理单个会话的完整快照生命周期：
//! - 快照树维护（创建、查询、回滚）
//! - 分支管理（创建、切换、合并）
//! - 多 Agent 沙箱（隔离工作区、发布合并）
//! - 日志记录与压缩

use super::snapshot::Branch;
use super::store::SnapshotStore;
use super::{
    FileInfo, Journal, JournalEntry, Patch, PatchSummary, ReplayEngine, Snapshot,
    SnapshotSummary, SnapshotTree, SnapshotTreeView, Workspace, WorkspaceState,
};
use crate::core::orchestration::multi_agent::{
    AgentSandbox, Conflict, ConflictResolution, MergeEngine, MergeResult,
    SandboxComparison, SandboxManager,
};
use std::collections::{HashMap, HashSet};
use std::path::PathBuf;
use std::sync::Arc;
use tokio::sync::RwLock;

/// 快照存储根目录名
/// 会话快照管理器
///
/// 持有快照树、日志、沙箱管理器等核心组件，
/// 提供线程安全的异步访问
pub struct SessionSnapshotManager {
    tree: RwLock<SnapshotTree>,
    journal: RwLock<Journal>,
    store: SnapshotStore,
    replay_engine: ReplayEngine,
    sandbox_manager: RwLock<SandboxManager>,
    merge_engine: MergeEngine,
    session_id: String,
}

impl SessionSnapshotManager {
    /// 创建新的会话管理器
    ///
    /// 初始化快照树、日志、沙箱管理器等组件
    pub async fn new(session_id: &str) -> Result<Self, String> {
        let sandbox_base_dir = crate::infra::config::data_paths::tmp_dir()
            .join("sandboxes")
            .join(session_id);

        let journal = Journal::open(session_id).map_err(|e| format!("打开日志失败: {}", e))?;

        let store = SnapshotStore::new(session_id);
        let tree = store
            .load_tree()
            .unwrap_or_else(|_| SnapshotTree::new(session_id));

        let replay_engine = ReplayEngine::new(session_id);

        let mut sandbox_manager = SandboxManager::new(sandbox_base_dir, session_id.to_string());
        let _ = sandbox_manager.load();
        let merge_engine = MergeEngine::new();

        Ok(Self {
            tree: RwLock::new(tree),
            journal: RwLock::new(journal),
            store,
            replay_engine,
            sandbox_manager: RwLock::new(sandbox_manager),
            merge_engine,
            session_id: session_id.to_string(),
        })
    }

    /// 创建新快照
    ///
    /// 普通快照只记录一轮 Agent 累积的补丁，不在这里自动升级为 checkpoint。
    pub async fn create_snapshot(
        &self,
        patches: Vec<Patch>,
        message: Option<String>,
        agent_id: Option<String>,
        workspace_id: Option<String>,
        _target_dir: Option<PathBuf>,
        trigger_user_memory_index: Option<usize>,
    ) -> Result<Snapshot, String> {
        let mut tree = self.tree.write().await;

        let mut snapshot = tree.create_snapshot(
            patches.clone(),
            message.clone(),
            agent_id,
            workspace_id,
            false,
            None,
        );

        if let Some(index) = trigger_user_memory_index {
            snapshot
                .metadata
                .insert("trigger_user_memory_index".to_string(), index.to_string());
        }
        let trigger_user_message_id = trigger_user_memory_index.and_then(|index| {
            crate::core::session::load_session(&self.session_id)
                .ok()
                .and_then(|memory| memory.message_ids.get(index).cloned())
        });
        if let Some(message_id) = trigger_user_message_id.as_ref() {
            snapshot
                .metadata
                .insert("trigger_user_message_id".to_string(), message_id.clone());
        }

        // 将 metadata 同步回 tree.nodes（create_snapshot 返回的是 clone 前的原始对象，
        // tree.nodes 中存的是 metadata 为空的旧副本，必须手动同步）
        if let Some(node) = tree.nodes.get_mut(&snapshot.id) {
            node.metadata = snapshot.metadata.clone();
        }

        self.store
            .save_tree(&tree)
            .map_err(|e| format!("保存树失败: {}", e))?;
        if let Some(index) = trigger_user_memory_index {
            crate::infra::db::upsert_checkpoint_user_message_link_v2(
                &self.session_id,
                trigger_user_message_id.as_deref(),
                index,
                &snapshot.id,
                !patches.is_empty(),
                snapshot.created_at,
            )
            .map_err(|e| format!("写入消息快照关联失败: {}", e))?;
        }
        crate::infra::config::data_paths::refresh_session_manifest(&self.session_id, None, None, None);

        let mut journal = self.journal.write().await;
        let entry = JournalEntry::CreateSnapshot {
            id: snapshot.id.clone(),
            parent_id: snapshot.parent_id.clone(),
            branch_name: snapshot.branch_name.clone(),
            patches_count: patches.len(),
            message: message.clone(),
            timestamp: snapshot.created_at,
        };
        journal
            .append(&entry)
            .map_err(|e| format!("写入日志失败: {}", e))?;

        if journal.should_compact() {
            if let Err(e) = journal.compact(&tree) {
                eprintln!("[Snapshot] Journal compact 失败: {}", e);
            }
        }

        Ok(snapshot)
    }

    /// 创建显式检查点快照（一轮 Agent 执行结束时的边界快照）
    ///
    /// 与 `create_snapshot` 不同，此方法强制 `is_checkpoint = true`，
    /// 并重建完整工作区状态以加速后续回滚。
    pub async fn create_checkpoint_snapshot(
        &self,
        message: Option<String>,
        agent_id: Option<String>,
        workspace_id: Option<String>,
        trigger_user_memory_index: Option<usize>,
    ) -> Result<Snapshot, String> {
        let mut tree = self.tree.write().await;

        let patch_count = tree.count_patches_since_last_checkpoint();
        let mut snapshot =
            tree.create_snapshot(vec![], message.clone(), agent_id, workspace_id, true, None);
        snapshot
            .metadata
            .insert("patch_count".to_string(), patch_count.to_string());
        if let Some(index) = trigger_user_memory_index {
            snapshot
                .metadata
                .insert("trigger_user_memory_index".to_string(), index.to_string());
        }
        if let Some(message_id) = trigger_user_memory_index.and_then(|index| {
            crate::core::session::load_session(&self.session_id)
                .ok()
                .and_then(|memory| memory.message_ids.get(index).cloned())
        }) {
            snapshot
                .metadata
                .insert("trigger_user_message_id".to_string(), message_id);
        }

        // 将 metadata 同步回 tree.nodes（create_snapshot 返回的是 clone 前的原始对象，
        // tree.nodes 中存的是 metadata 为空的旧副本，必须手动同步）
        if let Some(node) = tree.nodes.get_mut(&snapshot.id) {
            node.metadata = snapshot.metadata.clone();
        }

        // 增量检查点：从前一个检查点的 workspace_state 出发，只应用增量补丁
        let workspace = self
            .replay_engine
            .rebuild_workspace_incremental(&tree, &snapshot.id)
            .map_err(|e| format!("为 checkpoint 重建工作区失败: {}", e))?;

        if !workspace.files.is_empty() {
            // 找到前一个检查点的 workspace_state 用于 diff
            let prev_state = Self::find_prev_checkpoint_state(&tree, &snapshot.id);
            let mut files: HashMap<String, FileInfo> = HashMap::new();
            for (path, content) in &workspace.files {
                // 只对新增或变更的文件重新计算 hash 并写入 snapshot_content
                let hash = if let Some(prev_files) = &prev_state {
                    if let Some(prev_info) = prev_files.get(path) {
                        let new_hash = Patch::content_hash(content);
                        if prev_info.hash == new_hash {
                            // 未变更，复用旧 hash 和内容
                            new_hash
                        } else {
                            super::store::save_content(&self.session_id, &new_hash, content)
                                .map_err(|e| format!("保存快照内容失败: {}", e))?;
                            new_hash
                        }
                    } else {
                        let new_hash = Patch::content_hash(content);
                        super::store::save_content(&self.session_id, &new_hash, content)
                            .map_err(|e| format!("保存快照内容失败: {}", e))?;
                        new_hash
                    }
                } else {
                    let new_hash = Patch::content_hash(content);
                    super::store::save_content(&self.session_id, &new_hash, content)
                        .map_err(|e| format!("保存快照内容失败: {}", e))?;
                    new_hash
                };
                files.insert(
                    path.clone(),
                    FileInfo {
                        hash,
                        size: content.len() as u64,
                    },
                );
            }

            if let Some(node) = tree.nodes.get_mut(&snapshot.id) {
                node.workspace_state = Some(WorkspaceState {
                    files: files.clone(),
                });
            }
            snapshot.workspace_state = Some(WorkspaceState { files });
        }

        self.store
            .save_tree(&tree)
            .map_err(|e| format!("保存树失败: {}", e))?;
        crate::infra::config::data_paths::refresh_session_manifest(&self.session_id, None, None, None);

        let mut journal = self.journal.write().await;
        let entry = JournalEntry::CreateSnapshot {
            id: snapshot.id.clone(),
            parent_id: snapshot.parent_id.clone(),
            branch_name: snapshot.branch_name.clone(),
            patches_count: 0,
            message: message.clone(),
            timestamp: snapshot.created_at,
        };
        journal
            .append(&entry)
            .map_err(|e| format!("写入日志失败: {}", e))?;

        if journal.should_compact() {
            if let Err(e) = journal.compact(&tree) {
                eprintln!("[Snapshot] Journal compact 失败: {}", e);
            }
        }

        Ok(snapshot)
    }

    pub async fn get_snapshot(&self, id: &str) -> Result<Option<Snapshot>, String> {
        let tree = self.tree.read().await;
        Ok(tree.nodes.get(id).cloned())
    }

    pub async fn get_tree_view(&self) -> SnapshotTreeView {
        let tree = self.tree.read().await;
        tree.to_view()
    }

    pub async fn get_summaries(&self, ids: &[String]) -> Vec<SnapshotSummary> {
        let tree = self.tree.read().await;
        ids.iter()
            .filter_map(|id| tree.nodes.get(id).map(|s| s.to_summary()))
            .collect()
    }

    pub async fn create_branch(
        &self,
        branch_name: String,
        from_snapshot_id: Option<String>,
        agent_id: Option<String>,
        description: Option<String>,
    ) -> Result<(), String> {
        let mut tree = self.tree.write().await;
        tree.create_branch(
            branch_name.clone(),
            from_snapshot_id.clone(),
            agent_id.clone(),
            description,
        )?;

        let mut journal = self.journal.write().await;
        let entry = JournalEntry::CreateBranch {
            name: branch_name,
            from_snapshot_id: from_snapshot_id.unwrap_or_default(),
            agent_id,
        };
        journal
            .append(&entry)
            .map_err(|e| format!("写入日志失败: {}", e))?;

        Ok(())
    }

    pub async fn switch_branch(&self, branch_name: &str) -> Result<(), String> {
        let mut tree = self.tree.write().await;
        tree.switch_branch(branch_name)?;

        let mut journal = self.journal.write().await;
        let entry = JournalEntry::SwitchBranch {
            name: branch_name.to_string(),
        };
        journal
            .append(&entry)
            .map_err(|e| format!("写入日志失败: {}", e))?;

        Ok(())
    }

    pub async fn rollback_to(
        &self,
        snapshot_id: &str,
        target_dir: &PathBuf,
    ) -> Result<Workspace, String> {
        let mut tree = self.tree.write().await;

        let workspace = self
            .replay_engine
            .rebuild_workspace(&tree, snapshot_id)
            .map_err(|e| format!("重建工作区失败: {}", e))?;

        self.replay_engine
            .rollback_to(&mut tree, snapshot_id, target_dir)
            .await
            .map_err(|e| format!("回滚失败: {}", e))?;

        self.store
            .save_tree(&tree)
            .map_err(|e| format!("保存树失败: {}", e))?;
        crate::infra::config::data_paths::refresh_session_manifest(&self.session_id, None, None, None);

        Ok(workspace)
    }

    pub async fn preview_touched_files_to(&self, snapshot_id: &str) -> Result<Vec<PatchSummary>, String> {
        let tree = self.tree.read().await;
        self.replay_engine
            .preview_touched_files(&tree, snapshot_id)
            .map_err(|e| format!("预览回滚文件失败: {}", e))
    }

    pub async fn rollback_to_initial_state(&self, target_dir: &PathBuf) -> Result<Workspace, String> {
        let tree = self.tree.read().await;
        // 收集每个文件的最早 old_content 作为初始状态
        let mut initial_files: HashMap<String, String> = HashMap::new();
        let mut created_paths: HashSet<String> = HashSet::new();
        for snapshot in tree.nodes.values() {
            for patch in &snapshot.patches {
                match patch {
                    Patch::UpdateFile { path, old_content, .. } => {
                        initial_files.entry(path.clone()).or_insert_with(|| old_content.clone());
                    }
                    Patch::DeleteFile { path, content_hash } => {
                        if let Some(hash) = content_hash {
                            if let Ok(Some(content)) = crate::core::rollback::store::load_content(&self.session_id, hash) {
                                initial_files.entry(path.clone()).or_insert_with(|| content);
                            }
                        }
                    }
                    Patch::CreateFile { path, .. } => {
                        created_paths.insert(path.clone());
                    }
                    _ => {}
                }
            }
        }
        drop(tree);

        // 用每个文件的最早状态构建初始工作区
        let mut workspace = Workspace::new();
        workspace.files = initial_files;
        workspace.delete_paths = created_paths;

        self.replay_engine
            .rollback_to_initial_state(&workspace, target_dir)
            .await
            .map_err(|e| format!("回滚到初始状态失败: {}", e))?;

        // 重置树
        let mut tree = self.tree.write().await;
        *tree = SnapshotTree::new(&self.session_id);
        self.store
            .delete_all_for_session()
            .map_err(|e| format!("清理快照记录失败: {}", e))?;
        crate::infra::config::data_paths::refresh_session_manifest(&self.session_id, None, None, None);

        Ok(workspace)
    }

    pub async fn clear_snapshots_for_initial_state(&self) -> Result<(), String> {
        let mut tree = self.tree.write().await;
        *tree = SnapshotTree::new(&self.session_id);
        self.store
            .delete_all_for_session()
            .map_err(|e| format!("清理快照记录失败: {}", e))?;
        crate::infra::config::data_paths::refresh_session_manifest(&self.session_id, None, None, None);
        Ok(())
    }

    pub async fn list_snapshots(&self, branch_name: Option<&str>) -> Vec<Snapshot> {
        let tree = self.tree.read().await;
        let branch = branch_name.unwrap_or(&tree.current_branch);

        tree.nodes
            .values()
            .filter(|s| s.branch_name == branch)
            .cloned()
            .collect()
    }

    pub async fn list_branches(&self) -> Vec<Branch> {
        let tree = self.tree.read().await;
        tree.branches.values().cloned().collect()
    }

    pub async fn get_current_branch(&self) -> String {
        let tree = self.tree.read().await;
        tree.current_branch.clone()
    }

    pub async fn get_current_snapshot_id(&self) -> String {
        let tree = self.tree.read().await;
        tree.current_snapshot_id.clone()
    }

    /// 查找目标快照之前最近一个检查点的 workspace_state
    fn find_prev_checkpoint_state(
        tree: &SnapshotTree,
        target_id: &str,
    ) -> Option<HashMap<String, crate::core::rollback::snapshot::FileInfo>> {
        let mut current_id = tree
            .nodes
            .get(target_id)
            .and_then(|s| s.parent_id.clone());
        while let Some(id) = current_id {
            if let Some(snapshot) = tree.nodes.get(&id) {
                if snapshot.is_checkpoint {
                    return snapshot
                        .workspace_state
                        .as_ref()
                        .map(|ws| ws.files.clone());
                }
                current_id = snapshot.parent_id.clone();
            } else {
                break;
            }
        }
        None
    }

    /// 获取自上次 checkpoint 以来累积的补丁数量
    pub async fn count_patches_since_last_checkpoint(&self) -> usize {
        let tree = self.tree.read().await;
        tree.count_patches_since_last_checkpoint()
    }

    pub async fn should_create_checkpoint(&self) -> bool {
        let tree = self.tree.read().await;
        tree.should_create_checkpoint()
    }

    /// 从当前快照位置向前追溯，找到最近的 checkpoint 快照 ID
    ///
    /// 用于回滚时：纯聊天轮次没有自己的快照，
    /// 需要向前找到最近的"实快照"来恢复文件状态。
    /// 返回 None 表示从未有过文件编辑（可恢复到初始空状态）。
    pub async fn find_nearest_checkpoint_before(&self) -> Option<String> {
        let tree = self.tree.read().await;
        let mut current_id = Some(tree.current_snapshot_id.clone());

        while let Some(id) = current_id {
            if let Some(snapshot) = tree.nodes.get(&id) {
                if snapshot.is_checkpoint {
                    return Some(snapshot.id.clone());
                }
                current_id = snapshot.parent_id.clone();
            } else {
                break;
            }
        }

        None
    }

    /// 根据快照 message 模糊匹配，向前追溯找到最近的有 checkpoint 的快照 ID
    ///
    /// 在回滚场景中，用户可能回滚到一个没有自己快照的纯聊天轮次。
    /// 此方法从该轮次对应的快照位置（或当前头部）向前追溯，
    /// 找到最近的 checkpoint 快照以恢复文件状态。
    pub async fn find_nearest_checkpoint_for_message(&self, message: &str) -> Option<String> {
        let tree = self.tree.read().await;

        // 先尝试找到 message 对应的快照
        let start_id = tree
            .nodes
            .values()
            .filter(|s| s.is_checkpoint && s.message.as_deref() == Some(message))
            .map(|s| s.id.clone())
            .next()
            .unwrap_or_else(|| tree.current_snapshot_id.clone());

        let mut current_id = Some(start_id);

        while let Some(id) = current_id {
            if let Some(snapshot) = tree.nodes.get(&id) {
                if snapshot.is_checkpoint {
                    return Some(snapshot.id.clone());
                }
                current_id = snapshot.parent_id.clone();
            } else {
                break;
            }
        }

        None
    }

    pub async fn rebuild_workspace(&self, snapshot_id: &str) -> Result<Workspace, String> {
        let tree = self.tree.read().await;
        self.replay_engine
            .rebuild_workspace(&tree, snapshot_id)
            .map_err(|e| format!("重建工作区失败: {}", e))
    }

    // === 多 Agent 沙箱方法 ===

    /// 为指定 Agent 创建隔离沙箱
    pub async fn create_sandbox(
        &self,
        agent_id: String,
        base_snapshot_id: String,
        description: Option<String>,
    ) -> Result<AgentSandbox, String> {
        let mut sandbox_mgr = self.sandbox_manager.write().await;

        let mut tree = self.tree.write().await;
        let branch_name = format!("agent-{}", agent_id);
        tree.create_branch(
            branch_name.clone(),
            Some(base_snapshot_id.clone()),
            Some(agent_id.clone()),
            description.clone(),
        )
        .map_err(|e| format!("创建分支失败: {}", e))?;

        let sandbox = sandbox_mgr
            .create_sandbox(agent_id, base_snapshot_id, description)
            .map_err(|e| format!("创建沙箱失败: {}", e))?;

        sandbox_mgr
            .save()
            .map_err(|e| format!("保存沙箱失败: {}", e))?;

        Ok(sandbox)
    }

    pub async fn get_sandbox(&self, sandbox_id: &str) -> Result<Option<AgentSandbox>, String> {
        let sandbox_mgr = self.sandbox_manager.read().await;
        Ok(sandbox_mgr.get_sandbox(sandbox_id).cloned())
    }

    pub async fn list_sandboxes(&self) -> Vec<AgentSandbox> {
        let sandbox_mgr = self.sandbox_manager.read().await;
        sandbox_mgr.list_sandboxes().into_iter().cloned().collect()
    }

    pub async fn complete_sandbox(&self, sandbox_id: &str) -> Result<(), String> {
        let mut sandbox_mgr = self.sandbox_manager.write().await;
        sandbox_mgr
            .complete_sandbox(sandbox_id)
            .map_err(|e| format!("完成沙箱失败: {}", e))?;
        sandbox_mgr
            .save()
            .map_err(|e| format!("保存沙箱失败: {}", e))?;
        Ok(())
    }

    pub async fn abandon_sandbox(&self, sandbox_id: &str) -> Result<(), String> {
        let mut sandbox_mgr = self.sandbox_manager.write().await;
        sandbox_mgr
            .abandon_sandbox(sandbox_id)
            .map_err(|e| format!("放弃沙箱失败: {}", e))?;
        sandbox_mgr
            .save()
            .map_err(|e| format!("保存沙箱失败: {}", e))?;
        Ok(())
    }

    pub async fn publish_sandbox(&self, sandbox_id: &str) -> Result<String, String> {
        let mut sandbox_mgr = self.sandbox_manager.write().await;
        let mut tree = self.tree.write().await;

        let merge_branch = sandbox_mgr
            .publish_sandbox(sandbox_id, &mut tree)
            .map_err(|e| format!("发布沙箱失败: {}", e))?;

        self.store
            .save_tree(&tree)
            .map_err(|e| format!("保存树失败: {}", e))?;
        crate::infra::config::data_paths::refresh_session_manifest(&self.session_id, None, None, None);

        sandbox_mgr
            .save()
            .map_err(|e| format!("保存沙箱失败: {}", e))?;

        Ok(merge_branch)
    }

    pub async fn compare_sandboxes(&self) -> Vec<SandboxComparison> {
        let sandbox_mgr = self.sandbox_manager.read().await;
        let tree = self.tree.read().await;
        sandbox_mgr.compare_sandboxes(&tree)
    }

    // === 分支合并方法 ===

    /// 预览分支合并结果
    pub async fn preview_merge(
        &self,
        source_branch: &str,
        target_branch: &str,
    ) -> Result<MergeResult, String> {
        let tree = self.tree.read().await;
        self.merge_engine
            .preview_merge(&tree, source_branch, target_branch)
            .map_err(|e| format!("预览合并失败: {}", e))
    }

    pub async fn get_merge_conflicts(
        &self,
        source_branch: &str,
        target_branch: &str,
    ) -> Result<Vec<Conflict>, String> {
        let tree = self.tree.read().await;
        let result = self
            .merge_engine
            .preview_merge(&tree, source_branch, target_branch)
            .map_err(|e| format!("获取冲突失败: {}", e))?;
        Ok(result.conflicts)
    }

    pub async fn execute_merge(
        &self,
        source_branch: &str,
        target_branch: &str,
        resolutions: HashMap<String, ConflictResolution>,
        message: Option<String>,
    ) -> Result<Snapshot, String> {
        let mut tree = self.tree.write().await;

        let (mut result, merged_patches) = self
            .merge_engine
            .merge_branches(&tree, source_branch, target_branch, resolutions)
            .map_err(|e| format!("合并失败: {}", e))?;

        if !result.success {
            return Err(format!("存在 {} 个未解决的冲突", result.manual_required));
        }

        if merged_patches.is_empty() {
            return Err("合并后没有需要应用的补丁".to_string());
        }

        let snapshot = tree.create_snapshot(
            merged_patches,
            message.or_else(|| Some(format!("Merge {} into {}", source_branch, target_branch))),
            None,
            None,
            false,
            None,
        );

        result.merged_snapshot_id = Some(snapshot.id.clone());

        let mut journal = self.journal.write().await;
        let entry = super::journal::JournalEntry::CreateBranch {
            name: format!("merged-{}", source_branch),
            from_snapshot_id: tree.current_snapshot_id.clone(),
            agent_id: None,
        };
        let _ = journal.append(&entry);

        self.store
            .save_tree(&tree)
            .map_err(|e| format!("保存树失败: {}", e))?;
        crate::infra::config::data_paths::refresh_session_manifest(&self.session_id, None, None, None);

        Ok(snapshot)
    }
}

/// 会话快照管理器引用类型
pub type SessionSnapshotManagerRef = Arc<SessionSnapshotManager>;

/// 快照管理器注册表
///
/// 管理多个会话的 SessionSnapshotManager 实例，支持按需创建和缓存
pub struct SnapshotManagerRegistry {
    managers: RwLock<HashMap<String, SessionSnapshotManagerRef>>,
}

impl SnapshotManagerRegistry {
    pub fn new() -> Self {
        Self {
            managers: RwLock::new(HashMap::new()),
        }
    }

    /// 获取或创建会话快照管理器
    ///
    /// 优先从缓存获取，不存在则创建新实例
    pub async fn get_or_create(
        &self,
        session_id: &str,
    ) -> Result<SessionSnapshotManagerRef, String> {
        // 先尝试读取缓存
        {
            let managers = self.managers.read().await;
            if let Some(manager) = managers.get(session_id) {
                return Ok(manager.clone());
            }
        }

        // 缓存未命中，创建新实例
        let manager = Arc::new(SessionSnapshotManager::new(session_id).await?);

        // 写入缓存
        {
            let mut managers = self.managers.write().await;
            managers.insert(session_id.to_string(), manager.clone());
        }

        Ok(manager)
    }

    pub async fn remove(&self, session_id: &str) {
        let mut managers = self.managers.write().await;
        managers.remove(session_id);
    }
}
