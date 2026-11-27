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
    FileInfo, GarbageCollector, GcConfig, Journal, JournalEntry, Patch, PatchSummary, ReplayEngine,
    Snapshot, SnapshotSummary, SnapshotTree, SnapshotTreeView, Workspace, WorkspaceState,
};
use crate::core::orchestration::multi_agent::{
    AgentSandbox, Conflict, ConflictResolution, MergeEngine, MergeResult, SandboxComparison,
    SandboxManager,
};
use std::collections::{HashMap, HashSet};
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex as StdMutex};
use std::time::{SystemTime, UNIX_EPOCH};
use tokio::sync::{Mutex as AsyncMutex, RwLock};

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
    /// 创建新的会话管理器（在阻塞线程池内完成 SQLite/快照树/GC/沙箱磁盘加载，避免阻塞异步工作线程）
    ///
    /// 初始化快照树、日志、沙箱管理器等组件
    pub async fn new(session_id: &str) -> Result<Self, String> {
        let sid = session_id.to_string();
        tokio::task::spawn_blocking(move || Self::new_blocking(&sid))
            .await
            .map_err(|e| format!("初始化会话快照管理器任务失败: {}", e))?
    }

    fn new_blocking(session_id: &str) -> Result<Self, String> {
        let sandbox_base_dir = crate::infra::config::data_paths::tmp_dir()
            .join("sandboxes")
            .join(session_id);

        let journal = Journal::open(session_id).map_err(|e| format!("打开日志失败: {}", e))?;

        let store = SnapshotStore::new(session_id);
        let mut tree = store
            .load_tree()
            .unwrap_or_else(|_| SnapshotTree::new(session_id));

        // ── GC 触发点：本会话在本进程首次被打开时跑一次 ──
        // 放在这里而不是"启动时扫全部会话"：GC 是按会话的，而会话数量没有上限。
        // 三个阶段都是"按引用判定"（见 gc.rs），所以随时跑都安全，不会误删还有用的东西。
        {
            let gc = GarbageCollector::new(GcConfig::default());
            let result = gc.collect(&mut tree, session_id);
            let touched = result.removed_tree_snapshots
                + result.removed_orphan_contents
                + result.removed_branches
                + result.removed_trash_entries;
            if touched > 0 {
                println!(
                    "[GC] 会话 {} 回收：快照 {} / 孤儿内容 {} / 孤立分支 {} / 回收站条目 {}",
                    session_id,
                    result.removed_tree_snapshots,
                    result.removed_orphan_contents,
                    result.removed_branches,
                    result.removed_trash_entries
                );
                if let Err(e) = store.save_tree(&tree) {
                    eprintln!("[GC] 会话 {} 保存清理后的快照树失败: {}", session_id, e);
                }
            }
        }

        // ── v29 回填：旧库的关联表按"内存下标"存，无法可靠还原；
        //    快照树里的 `trigger_user_message_id` 才是真锚点。
        //    这里按 message_id 补齐缺失关联，绝不覆盖已有权威关联（INSERT OR IGNORE）。
        //    普通快照优先；只有整轮快照节点已被裁剪时，才退而使用 checkpoint 节点。 ──
        {
            let mut linked_message_ids: HashSet<String> =
                crate::core::rollback::patch_store::list_checkpoint_user_message_links(session_id)
                    .unwrap_or_default()
                    .into_iter()
                    .map(|link| link.message_id)
                    .collect();
            // 只回填**仍然可见**的消息：已撤回/隐藏消息的快照节点还在树里，
            // 但绝不能因回填把它们的关联"复活"，否则刷新后又会冒出指向幽灵消息的链接。
            let visible_message_ids: HashSet<String> =
                crate::core::session::list_visible_session_messages(session_id)
                    .unwrap_or_default()
                    .into_iter()
                    .map(|message| message.message_id)
                    .collect();
            for pass_checkpoint in [false, true] {
                for snapshot in tree.nodes.values() {
                    if snapshot.is_checkpoint != pass_checkpoint {
                        continue;
                    }
                    let message_id = snapshot
                        .metadata
                        .get("trigger_user_message_id")
                        .map(|value| value.trim().to_string())
                        .filter(|value| !value.is_empty());
                    let Some(message_id) = message_id else {
                        continue;
                    };
                    if linked_message_ids.contains(&message_id)
                        || !visible_message_ids.contains(&message_id)
                    {
                        continue;
                    }
                    if crate::core::rollback::patch_store::insert_checkpoint_user_message_link_if_absent(
                        session_id,
                        &message_id,
                        &snapshot.id,
                        !snapshot.patches.is_empty(),
                        snapshot.created_at,
                    )
                    .is_ok()
                    {
                        linked_message_ids.insert(message_id);
                    }
                }
            }
        }

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
    ///
    /// `trigger_user_message_id` 是本轮用户消息的稳定 UUID（撤回关联的唯一锚点）。
    /// v29 起不再接收/保存内存下标：压缩与撤回都会改写它。
    pub async fn create_snapshot(
        &self,
        patches: Vec<Patch>,
        message: Option<String>,
        agent_id: Option<String>,
        workspace_id: Option<String>,
        _target_dir: Option<PathBuf>,
        trigger_user_message_id: Option<String>,
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

        let trigger_user_message_id = trigger_user_message_id
            .map(|value| value.trim().to_string())
            .filter(|value| !value.is_empty());
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
        if let Some(message_id) = trigger_user_message_id.as_ref() {
            // **只登记本轮第一个文件快照**：同一条用户消息可能因续跑/循环确认产生
            // 多个快照。撤回这条消息时应回到"本轮开始前"，因此目标必须锚定第一个
            // 快照的父节点；若每次覆盖成最新快照，只会撤掉最后一次续跑。
            crate::core::rollback::patch_store::insert_checkpoint_user_message_link_if_absent(
                &self.session_id,
                message_id,
                &snapshot.id,
                !patches.is_empty(),
                snapshot.created_at,
            )
            .map_err(|e| format!("写入消息快照关联失败: {}", e))?;
        }
        crate::infra::config::data_paths::refresh_session_manifest(
            &self.session_id,
            None,
            None,
            None,
        );

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
        trigger_user_message_id: Option<String>,
    ) -> Result<Snapshot, String> {
        let mut tree = self.tree.write().await;

        let patch_count = tree.count_patches_since_last_checkpoint();
        let mut snapshot =
            tree.create_snapshot(vec![], message.clone(), agent_id, workspace_id, true, None);
        snapshot
            .metadata
            .insert("patch_count".to_string(), patch_count.to_string());
        let trigger_user_message_id = trigger_user_message_id
            .map(|value| value.trim().to_string())
            .filter(|value| !value.is_empty());
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
        crate::infra::config::data_paths::refresh_session_manifest(
            &self.session_id,
            None,
            None,
            None,
        );

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

    /// 回滚到指定快照。返回 `(目标工作区, 执行结果)` —— 结果里的 `restore_failures`
    /// 要一路带到 UI（工作区与目标快照不一致时，用户必须知道）。
    pub async fn rollback_to(
        &self,
        snapshot_id: &str,
        target_dir: &PathBuf,
    ) -> Result<(Workspace, super::replay::RollbackOutcome), String> {
        let mut tree = self.tree.write().await;

        let workspace = self
            .replay_engine
            .rebuild_workspace(&tree, snapshot_id)
            .map_err(|e| format!("重建工作区失败: {}", e))?;

        let outcome = self
            .replay_engine
            .rollback_to(&mut tree, snapshot_id, target_dir)
            .await
            .map_err(|e| format!("回滚失败: {}", e))?;

        self.store
            .save_tree(&tree)
            .map_err(|e| format!("保存树失败: {}", e))?;
        crate::infra::config::data_paths::refresh_session_manifest(
            &self.session_id,
            None,
            None,
            None,
        );

        Ok((workspace, outcome))
    }

    pub async fn preview_touched_files_to(
        &self,
        snapshot_id: &str,
    ) -> Result<Vec<PatchSummary>, String> {
        let tree = self.tree.read().await;
        self.replay_engine
            .preview_touched_files(&tree, snapshot_id)
            .map_err(|e| format!("预览回滚文件失败: {}", e))
    }

    /// 回滚到初始状态。返回 `(初始工作区, 执行结果)`，理由同 [`Self::rollback_to`]。
    pub async fn rollback_to_initial_state(
        &self,
        target_dir: &PathBuf,
    ) -> Result<(Workspace, super::replay::RollbackOutcome), String> {
        let tree = self.tree.read().await;
        // 收集每个文件的最早 old_content 作为初始状态
        let mut initial_files: HashMap<String, String> = HashMap::new();
        let mut created_paths: HashSet<String> = HashSet::new();
        for snapshot in tree.nodes.values() {
            for patch in &snapshot.patches {
                match patch {
                    Patch::UpdateFile {
                        path, old_content, ..
                    } => {
                        initial_files
                            .entry(path.clone())
                            .or_insert_with(|| old_content.clone());
                    }
                    Patch::DeleteFile {
                        path,
                        content_hash: Some(hash),
                        ..
                    } => {
                        if let Ok(Some(content)) =
                            crate::core::rollback::store::load_content(&self.session_id, hash)
                        {
                            initial_files.entry(path.clone()).or_insert_with(|| content);
                        }
                    }
                    Patch::CreateFile { path, .. } => {
                        created_paths.insert(path.clone());
                    }
                    _ => {}
                }
            }
        }
        // 回滚上下文（清理判据 + 回收站恢复清单）都要趁 tree 还持有先算出来
        let ctx = super::replay::RollbackContext {
            known_paths: super::replay::known_paths_in_tree(&tree),
            // 回滚到初始状态 = 撤销整棵树 → 树里所有"快照无可用内容"的删除都要从回收站搬回
            trash_restores: super::replay::trash_restores_from_patches(
                &self.session_id,
                tree.nodes.values().flat_map(|node| node.patches.iter()),
            ),
        };
        drop(tree);

        // 用每个文件的最早状态构建初始工作区
        let mut workspace = Workspace::new();
        workspace.files = initial_files;
        workspace.delete_paths = created_paths;

        let outcome = self
            .replay_engine
            .rollback_to_initial_state(&workspace, target_dir, &ctx)
            .await
            .map_err(|e| format!("回滚到初始状态失败: {}", e))?;

        // 重置树
        let mut tree = self.tree.write().await;
        *tree = SnapshotTree::new(&self.session_id);
        self.store
            .delete_all_for_session()
            .map_err(|e| format!("清理快照记录失败: {}", e))?;
        crate::infra::config::data_paths::refresh_session_manifest(
            &self.session_id,
            None,
            None,
            None,
        );

        Ok((workspace, outcome))
    }

    pub async fn clear_snapshots_for_initial_state(&self) -> Result<(), String> {
        let mut tree = self.tree.write().await;
        *tree = SnapshotTree::new(&self.session_id);
        self.store
            .delete_all_for_session()
            .map_err(|e| format!("清理快照记录失败: {}", e))?;
        crate::infra::config::data_paths::refresh_session_manifest(
            &self.session_id,
            None,
            None,
            None,
        );
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

    /// 当前 HEAD 所在链路上的全部快照 ID（含 HEAD 自身）。
    ///
    /// 撤回解析必须只看这条链路：回滚后旧的后继快照仍留在 `nodes` 里（孤儿分支），
    /// 若不筛掉，它们可能被当成"本条消息之后的下一个文件快照"，把撤回目标带偏。
    pub async fn current_chain_ids(&self) -> HashSet<String> {
        let tree = self.tree.read().await;
        let mut ids = HashSet::new();
        let mut current = Some(tree.current_snapshot_id.clone());
        while let Some(id) = current {
            if !ids.insert(id.clone()) {
                break;
            }
            current = tree
                .nodes
                .get(&id)
                .and_then(|snapshot| snapshot.parent_id.clone());
        }
        ids
    }

    /// 查找目标快照之前最近一个检查点的 workspace_state
    fn find_prev_checkpoint_state(
        tree: &SnapshotTree,
        target_id: &str,
    ) -> Option<HashMap<String, crate::core::rollback::snapshot::FileInfo>> {
        let mut current_id = tree.nodes.get(target_id).and_then(|s| s.parent_id.clone());
        while let Some(id) = current_id {
            if let Some(snapshot) = tree.nodes.get(&id) {
                if snapshot.is_checkpoint {
                    return snapshot.workspace_state.as_ref().map(|ws| ws.files.clone());
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
        crate::infra::config::data_paths::refresh_session_manifest(
            &self.session_id,
            None,
            None,
            None,
        );

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
        crate::infra::config::data_paths::refresh_session_manifest(
            &self.session_id,
            None,
            None,
            None,
        );

        Ok(snapshot)
    }
}

/// 会话快照管理器引用类型
pub type SessionSnapshotManagerRef = Arc<SessionSnapshotManager>;

/// 快照管理器注册表默认缓存上限（超出后按最近最少使用回收无外部引用的条目）
pub const MAX_CACHED_SNAPSHOT_MANAGERS: usize = 32;
/// 快照管理器空闲回收阈值（30 分钟未访问且无外部持有者时自动回收）
pub const SNAPSHOT_MANAGER_IDLE_TTL_MS: u64 = 30 * 60 * 1000;

fn now_epoch_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

struct CachedSnapshotManager {
    manager: SessionSnapshotManagerRef,
    /// 最近访问毫秒时间戳（原子更新，热路径持读锁即可刷新，无需升级写锁）
    last_accessed_ms: AtomicU64,
}

impl CachedSnapshotManager {
    fn new(manager: SessionSnapshotManagerRef) -> Self {
        Self {
            manager,
            last_accessed_ms: AtomicU64::new(now_epoch_ms()),
        }
    }

    fn touch(&self) {
        self.last_accessed_ms
            .store(now_epoch_ms(), Ordering::Relaxed);
    }

    fn last_accessed(&self) -> u64 {
        self.last_accessed_ms.load(Ordering::Relaxed)
    }
}

/// 快照管理器注册表
///
/// 管理多个会话的 `SessionSnapshotManager` 实例，具备：
/// - 按 `session_id` 细粒度加锁的双检初始化（Double-Check Lock），彻底杜绝冷会话并发重复初始化与重复 GC；
/// - 基于原子访问时间戳与强引用计数（`Arc::strong_count == 1`）的有界空闲回收，防止内存无限增长。
pub struct SnapshotManagerRegistry {
    managers: RwLock<HashMap<String, CachedSnapshotManager>>,
    init_locks: StdMutex<HashMap<String, Arc<AsyncMutex<()>>>>,
}

impl Default for SnapshotManagerRegistry {
    fn default() -> Self {
        Self::new()
    }
}

impl SnapshotManagerRegistry {
    pub fn new() -> Self {
        Self {
            managers: RwLock::new(HashMap::new()),
            init_locks: StdMutex::new(HashMap::new()),
        }
    }

    /// 获取指定会话的专属初始化锁（不存在则创建）。
    fn acquire_init_lock(&self, session_id: &str) -> Arc<AsyncMutex<()>> {
        let mut locks = self
            .init_locks
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        locks
            .entry(session_id.to_string())
            .or_insert_with(|| Arc::new(AsyncMutex::new(())))
            .clone()
    }

    /// 仅在无其他并发协程持有同一把初始化锁时回收表项，避免 `init_locks` 长期累积。
    fn cleanup_init_lock(&self, session_id: &str, lock: &Arc<AsyncMutex<()>>) {
        let mut locks = self
            .init_locks
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        if locks
            .get(session_id)
            .is_some_and(|existing| Arc::ptr_eq(existing, lock) && Arc::strong_count(existing) == 2)
        {
            locks.remove(session_id);
        }
    }

    /// 在持有 `managers` 写锁时执行空闲超期回收与容量上限 LRU 淘汰。
    ///
    /// 安全不变量：仅驱逐 `Arc::strong_count(&entry.manager) == 1`（仅注册表自身持有，无外部命令/工具在使用）
    /// 且不在 `protect_session_id` 中的条目。
    fn evict_locked(
        managers: &mut HashMap<String, CachedSnapshotManager>,
        now_ms: u64,
        max_idle_ms: u64,
        max_entries: usize,
        protect_session_id: Option<&str>,
    ) -> usize {
        let mut evicted = 0usize;

        // 阶段一：回收超过空闲阈值且无外部引用的条目
        let expired_keys: Vec<String> = managers
            .iter()
            .filter(|(sid, entry)| {
                protect_session_id != Some(sid.as_str())
                    && Arc::strong_count(&entry.manager) == 1
                    && now_ms.saturating_sub(entry.last_accessed()) >= max_idle_ms
            })
            .map(|(sid, _)| sid.clone())
            .collect();

        for sid in expired_keys {
            if managers.remove(&sid).is_some() {
                evicted += 1;
            }
        }

        // 阶段二：若总量仍超过上限，按最近最少使用（LRU）淘汰无外部引用的条目
        if managers.len() > max_entries {
            let need_evict = managers.len() - max_entries;
            let mut candidates: Vec<(u64, String)> = managers
                .iter()
                .filter(|(sid, entry)| {
                    protect_session_id != Some(sid.as_str())
                        && Arc::strong_count(&entry.manager) == 1
                })
                .map(|(sid, entry)| (entry.last_accessed(), sid.clone()))
                .collect();
            candidates.sort_by(|a, b| a.0.cmp(&b.0).then_with(|| a.1.cmp(&b.1)));

            for (_, sid) in candidates.into_iter().take(need_evict) {
                if managers.remove(&sid).is_some() {
                    evicted += 1;
                }
            }
        }

        evicted
    }

    /// 通用按会话细粒度双检锁加载入口（便于单测注入自定义异步 loader 验证并发防击穿）。
    async fn get_or_create_with<F, Fut>(
        &self,
        session_id: &str,
        loader: F,
    ) -> Result<SessionSnapshotManagerRef, String>
    where
        F: FnOnce(String) -> Fut,
        Fut: std::future::Future<Output = Result<SessionSnapshotManagerRef, String>>,
    {
        // 快速路径：热命中直接走读锁并原子刷新访问时间，不碰写锁和初始化锁
        {
            let managers = self.managers.read().await;
            if let Some(entry) = managers.get(session_id) {
                entry.touch();
                return Ok(entry.manager.clone());
            }
        }

        // 慢速路径：按 session_id 粒度加初始化锁（同会话串行，跨会话完全并行）
        let init_lock = self.acquire_init_lock(session_id);
        let init_guard = init_lock.lock().await;

        // 双检（Double-Check）：等待锁期间同会话前序请求可能已完成初始化
        {
            let managers = self.managers.read().await;
            if let Some(entry) = managers.get(session_id) {
                entry.touch();
                let mgr = entry.manager.clone();
                drop(managers);
                drop(init_guard);
                self.cleanup_init_lock(session_id, &init_lock);
                return Ok(mgr);
            }
        }

        // 锁外执行全部磁盘与数据库初始化，避免卡住全局注册表读写锁
        let built = match loader(session_id.to_string()).await {
            Ok(mgr) => mgr,
            Err(err) => {
                drop(init_guard);
                self.cleanup_init_lock(session_id, &init_lock);
                return Err(err);
            }
        };

        // 纳秒级短写锁插入，并在超限时顺手做轻量 LRU 回收
        let manager = {
            let mut managers = self.managers.write().await;
            let entry = managers
                .entry(session_id.to_string())
                .or_insert_with(|| CachedSnapshotManager::new(built));
            entry.touch();
            let mgr = entry.manager.clone();
            if managers.len() > MAX_CACHED_SNAPSHOT_MANAGERS {
                Self::evict_locked(
                    &mut managers,
                    now_epoch_ms(),
                    SNAPSHOT_MANAGER_IDLE_TTL_MS,
                    MAX_CACHED_SNAPSHOT_MANAGERS,
                    Some(session_id),
                );
            }
            mgr
        };

        drop(init_guard);
        self.cleanup_init_lock(session_id, &init_lock);
        Ok(manager)
    }

    /// 获取或创建会话快照管理器
    ///
    /// 优先从缓存获取，不存在则按会话粒度双检初始化新实例
    pub async fn get_or_create(
        &self,
        session_id: &str,
    ) -> Result<SessionSnapshotManagerRef, String> {
        self.get_or_create_with(session_id, |sid| async move {
            SessionSnapshotManager::new(&sid).await.map(Arc::new)
        })
        .await
    }

    /// 回收空闲超时或超出容量上限且无外部引用的快照管理器，返回本次回收条目数。
    pub async fn evict_idle(&self, max_idle_ms: u64, max_entries: usize) -> usize {
        let mut managers = self.managers.write().await;
        Self::evict_locked(
            &mut managers,
            now_epoch_ms(),
            max_idle_ms,
            max_entries,
            None,
        )
    }

    /// 按默认阈值（30 分钟空闲 / 最多 32 个缓存）执行一次回收。
    pub async fn evict_default_idle(&self) -> usize {
        self.evict_idle(SNAPSHOT_MANAGER_IDLE_TTL_MS, MAX_CACHED_SNAPSHOT_MANAGERS)
            .await
    }

    /// 移除指定会话缓存；若该会话正好处于并发冷初始化中，先等待其结束再删除，防止幽灵条目回写。
    pub async fn remove(&self, session_id: &str) {
        let maybe_lock = {
            let locks = self
                .init_locks
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            locks.get(session_id).cloned()
        };
        if let Some(lock) = maybe_lock {
            let guard = lock.lock().await;
            self.managers.write().await.remove(session_id);
            drop(guard);
            self.cleanup_init_lock(session_id, &lock);
        } else {
            self.managers.write().await.remove(session_id);
        }
    }
}

#[cfg(test)]
mod registry_concurrency_tests {
    use super::*;
    use std::sync::atomic::AtomicUsize;
    use std::time::Duration;

    fn make_stub_manager(session_id: &str) -> SessionSnapshotManagerRef {
        Arc::new(SessionSnapshotManager {
            tree: RwLock::new(SnapshotTree::new(session_id)),
            journal: RwLock::new(Journal::new_for_test(session_id)),
            store: SnapshotStore::new(session_id),
            replay_engine: ReplayEngine::new(session_id),
            sandbox_manager: RwLock::new(SandboxManager::new(
                PathBuf::from("test_sandbox"),
                session_id.to_string(),
            )),
            merge_engine: MergeEngine::new(),
            session_id: session_id.to_string(),
        })
    }

    /// 同一冷会话高并发访问快照注册表时，双检锁保证 loader 仅执行一次，且完成后清理 init_locks
    #[tokio::test]
    async fn concurrent_cold_get_or_create_initializes_once_and_cleans_lock() {
        let registry = Arc::new(SnapshotManagerRegistry::new());
        let init_count = Arc::new(AtomicUsize::new(0));

        let mut tasks = Vec::new();
        for _ in 0..8 {
            let reg = Arc::clone(&registry);
            let counter = Arc::clone(&init_count);
            tasks.push(tokio::spawn(async move {
                reg.get_or_create_with("snap_cold_1", |sid| async move {
                    counter.fetch_add(1, Ordering::SeqCst);
                    tokio::time::sleep(Duration::from_millis(50)).await;
                    Ok(make_stub_manager(&sid))
                })
                .await
                .expect("测试初始化不应失败")
            }));
        }

        let mut results = Vec::new();
        for task in tasks {
            results.push(task.await.expect("任务不应 panic"));
        }

        assert_eq!(
            init_count.load(Ordering::SeqCst),
            1,
            "同一冷会话快照管理器只应初始化一次"
        );
        for mgr in &results[1..] {
            assert!(
                Arc::ptr_eq(&results[0], mgr),
                "并发调用应共享同一个 SessionSnapshotManager 实例"
            );
        }
        assert!(
            registry.init_locks.lock().unwrap().is_empty(),
            "初始化完成后应清理无竞争的 init_locks"
        );
    }

    /// 回收逻辑绝不淘汰外部仍在引用的快照管理器，但会按 TTL 与 LRU 回收空闲条目
    #[tokio::test]
    async fn evict_idle_preserves_referenced_and_reclaims_stale_or_overflow() {
        let registry = SnapshotManagerRegistry::new();
        let now = now_epoch_ms();

        // 注入 4 个会话条目并设置确定的访问时间顺序
        let _s1 = registry
            .get_or_create_with("s1", |sid| async move { Ok(make_stub_manager(&sid)) })
            .await
            .unwrap();
        let held_s2 = registry
            .get_or_create_with("s2", |sid| async move { Ok(make_stub_manager(&sid)) })
            .await
            .unwrap();
        let _s3 = registry
            .get_or_create_with("s3", |sid| async move { Ok(make_stub_manager(&sid)) })
            .await
            .unwrap();
        let _s4 = registry
            .get_or_create_with("s4", |sid| async move { Ok(make_stub_manager(&sid)) })
            .await
            .unwrap();
        drop(_s1);
        drop(_s3);
        drop(_s4);

        {
            let guard = registry.managers.read().await;
            guard["s1"]
                .last_accessed_ms
                .store(now.saturating_sub(10_000), Ordering::Relaxed);
            guard["s2"]
                .last_accessed_ms
                .store(now.saturating_sub(9_000), Ordering::Relaxed);
            guard["s3"]
                .last_accessed_ms
                .store(now.saturating_sub(2_000), Ordering::Relaxed);
            guard["s4"].last_accessed_ms.store(now, Ordering::Relaxed);
        }

        // TTL=5s、上限=2：s1 超期应被回收；s2 虽超期但外部仍持有 held_s2 不可回收；
        // 剩余 s2/s3/s4 共 3 个超上限 2，按 LRU 应淘汰最旧的无引用条目 s3，最终保留 s2 与 s4
        let evicted = registry.evict_idle(5_000, 2).await;
        assert_eq!(evicted, 2);
        {
            let guard = registry.managers.read().await;
            assert!(!guard.contains_key("s1"), "超期无引用条目 s1 应被回收");
            assert!(guard.contains_key("s2"), "外部持有引用的 s2 绝不可回收");
            assert!(
                !guard.contains_key("s3"),
                "超容量上限的最旧无引用条目 s3 应被 LRU 淘汰"
            );
            assert!(guard.contains_key("s4"), "最新访问的 s4 应保留");
        }

        // 释放 held_s2 后再次按 TTL=5s 回收，s2 应顺利被清理
        drop(held_s2);
        let evicted_after_drop = registry.evict_idle(5_000, 2).await;
        assert_eq!(evicted_after_drop, 1);
        assert!(!registry.managers.read().await.contains_key("s2"));
    }
}
