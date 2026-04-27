use crate::get_agent_home;
use crate::core::snapshot_engine::{
    Journal, JournalEntry, Patch, ReplayEngine, Snapshot, SnapshotTree,
    SnapshotTreeView, SnapshotSummary, Workspace, WorkspaceState, FileInfo,
    AgentSandbox, SandboxManager, SandboxComparison, MergeEngine, MergeResult,
    Conflict, ConflictResolution,
};
use crate::core::snapshot_engine::snapshot::Branch;
use super::store::SnapshotStore;
use std::collections::HashMap;
use std::fs;
use std::path::PathBuf;
use std::sync::Arc;
use tokio::sync::RwLock;

const DIR_SNAPSHOTS: &str = ".snapshots";

pub struct SessionManager {
    tree: RwLock<SnapshotTree>,
    journal: RwLock<Journal>,
    store: SnapshotStore,
    replay_engine: ReplayEngine,
    sandbox_manager: RwLock<SandboxManager>,
    merge_engine: MergeEngine,
    session_id: String,
}

impl SessionManager {
    pub async fn new(session_id: &str) -> Result<Self, String> {
        let base_dir = get_agent_home().join(DIR_SNAPSHOTS).join(session_id);
        std::fs::create_dir_all(&base_dir)
            .map_err(|e| format!("创建快照目录失败: {}", e))?;
        
        let journal_path = base_dir.join("journal.log");
        let journal = Journal::open(&journal_path)
            .map_err(|e| format!("打开日志失败: {}", e))?;
        
        let store = SnapshotStore::new(base_dir.clone());
        let tree = store.load_tree()
            .unwrap_or_else(|_| SnapshotTree::new(session_id));
        
        let content_store_path = base_dir.join("content");
        let replay_engine = ReplayEngine::new(content_store_path);
        
        let sandbox_manager = SandboxManager::new(base_dir.clone(), session_id.to_string());
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
    
    pub async fn create_snapshot(
        &self,
        patches: Vec<Patch>,
        message: Option<String>,
        agent_id: Option<String>,
        workspace_id: Option<String>,
        _target_dir: Option<PathBuf>,
    ) -> Result<Snapshot, String> {
        let mut tree = self.tree.write().await;

        let is_checkpoint = tree.should_create_checkpoint();

        let snapshot = tree.create_snapshot(
            patches.clone(), message.clone(), agent_id, workspace_id,
            is_checkpoint, None,
        );

        if is_checkpoint {
            let workspace = self.replay_engine.rebuild_workspace(&tree, &snapshot.id)
                .map_err(|e| format!("为 checkpoint 重建工作区失败: {}", e))?;

            if !workspace.files.is_empty() {
                let content_store = get_agent_home()
                    .join(DIR_SNAPSHOTS).join(&self.session_id).join("content");
                fs::create_dir_all(&content_store)
                    .map_err(|e| format!("创建内容存储目录失败: {}", e))?;

                let mut files: HashMap<String, FileInfo> = HashMap::new();
                for (path, content) in &workspace.files {
                    let hash = Patch::content_hash(content);
                    let content_path = content_store.join(&hash);
                    if !content_path.exists() {
                        fs::write(&content_path, content)
                            .map_err(|e| format!("写入内容文件失败: {}", e))?;
                    }
                    files.insert(path.clone(), FileInfo {
                        hash,
                        size: content.len() as u64,
                    });
                }

                if let Some(node) = tree.nodes.get_mut(&snapshot.id) {
                    node.workspace_state = Some(WorkspaceState { files });
                }
            }
        }

        self.store.save_snapshot(&snapshot)
            .map_err(|e| format!("保存快照失败: {}", e))?;

        self.store.save_tree(&tree)
            .map_err(|e| format!("保存树失败: {}", e))?;

        let mut journal = self.journal.write().await;
        let entry = JournalEntry::CreateSnapshot {
            id: snapshot.id.clone(),
            parent_id: snapshot.parent_id.clone(),
            branch_name: snapshot.branch_name.clone(),
            patches_count: patches.len(),
            message: message.clone(),
            timestamp: snapshot.created_at,
        };
        journal.append(&entry)
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
        tree.create_branch(branch_name.clone(), from_snapshot_id.clone(), agent_id.clone(), description)?;
        
        let mut journal = self.journal.write().await;
        let entry = JournalEntry::CreateBranch {
            name: branch_name,
            from_snapshot_id: from_snapshot_id.unwrap_or_default(),
            agent_id,
        };
        journal.append(&entry)
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
        journal.append(&entry)
            .map_err(|e| format!("写入日志失败: {}", e))?;
        
        Ok(())
    }
    
    pub async fn rollback_to(&self, snapshot_id: &str, target_dir: &PathBuf) -> Result<Workspace, String> {
        let mut tree = self.tree.write().await;
        
        let workspace = self.replay_engine.rebuild_workspace(&tree, snapshot_id)
            .map_err(|e| format!("重建工作区失败: {}", e))?;
        
        self.replay_engine.rollback_to(&mut tree, snapshot_id, target_dir).await
            .map_err(|e| format!("回滚失败: {}", e))?;
        
        self.store.save_tree(&tree)
            .map_err(|e| format!("保存树失败: {}", e))?;
        
        Ok(workspace)
    }
    
    pub async fn list_snapshots(&self, branch_name: Option<&str>) -> Vec<Snapshot> {
        let tree = self.tree.read().await;
        let branch = branch_name.unwrap_or(&tree.current_branch);
        
        tree.nodes.values()
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
    
    pub async fn rebuild_workspace(&self, snapshot_id: &str) -> Result<Workspace, String> {
        let tree = self.tree.read().await;
        self.replay_engine.rebuild_workspace(&tree, snapshot_id)
            .map_err(|e| format!("重建工作区失败: {}", e))
    }
    
    // === P6: 多Agent沙箱方法 ===
    
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
        ).map_err(|e| format!("创建分支失败: {}", e))?;
        
        let sandbox = sandbox_mgr.create_sandbox(agent_id, base_snapshot_id, description)
            .map_err(|e| format!("创建沙箱失败: {}", e))?;
        
        sandbox_mgr.save().map_err(|e| format!("保存沙箱失败: {}", e))?;
        
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
        sandbox_mgr.complete_sandbox(sandbox_id)
            .map_err(|e| format!("完成沙箱失败: {}", e))?;
        sandbox_mgr.save().map_err(|e| format!("保存沙箱失败: {}", e))?;
        Ok(())
    }
    
    pub async fn abandon_sandbox(&self, sandbox_id: &str) -> Result<(), String> {
        let mut sandbox_mgr = self.sandbox_manager.write().await;
        sandbox_mgr.abandon_sandbox(sandbox_id)
            .map_err(|e| format!("放弃沙箱失败: {}", e))?;
        sandbox_mgr.save().map_err(|e| format!("保存沙箱失败: {}", e))?;
        Ok(())
    }
    
    pub async fn publish_sandbox(&self, sandbox_id: &str) -> Result<String, String> {
        let mut sandbox_mgr = self.sandbox_manager.write().await;
        let mut tree = self.tree.write().await;
        
        let merge_branch = sandbox_mgr.publish_sandbox(sandbox_id, &mut tree)
            .map_err(|e| format!("发布沙箱失败: {}", e))?;
        
        self.store.save_tree(&tree)
            .map_err(|e| format!("保存树失败: {}", e))?;
        
        sandbox_mgr.save().map_err(|e| format!("保存沙箱失败: {}", e))?;
        
        Ok(merge_branch)
    }
    
    pub async fn compare_sandboxes(&self) -> Vec<SandboxComparison> {
        let sandbox_mgr = self.sandbox_manager.read().await;
        let tree = self.tree.read().await;
        sandbox_mgr.compare_sandboxes(&tree)
    }
    
    // === P7: 分支合并方法 ===
    
    pub async fn preview_merge(
        &self,
        source_branch: &str,
        target_branch: &str,
    ) -> Result<MergeResult, String> {
        let tree = self.tree.read().await;
        self.merge_engine.preview_merge(&tree, source_branch, target_branch)
            .map_err(|e| format!("预览合并失败: {}", e))
    }
    
    pub async fn get_merge_conflicts(
        &self,
        source_branch: &str,
        target_branch: &str,
    ) -> Result<Vec<Conflict>, String> {
        let tree = self.tree.read().await;
        let result = self.merge_engine.preview_merge(&tree, source_branch, target_branch)
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
        
        let result = self.merge_engine.merge_branches(
            &tree,
            source_branch,
            target_branch,
            resolutions,
        ).map_err(|e| format!("合并失败: {}", e))?;
        
        if !result.success {
            return Err(format!("存在 {} 个未解决的冲突", result.manual_required));
        }
        
        let mut journal = self.journal.write().await;
        let entry = crate::core::snapshot_engine::journal::JournalEntry::CreateBranch {
            name: format!("merged-{}", source_branch),
            from_snapshot_id: tree.current_snapshot_id.clone(),
            agent_id: None,
        };
        let _ = journal.append(&entry);
        
        let snapshot = tree.create_snapshot(
            vec![],
            message.or_else(|| Some(format!("Merge {} into {}", source_branch, target_branch))),
            None,
            None,
            false,
            None,
        );
        
        self.store.save_tree(&tree)
            .map_err(|e| format!("保存树失败: {}", e))?;
        
        Ok(snapshot)
    }
}

pub type SessionManagerRef = Arc<SessionManager>;

pub struct SessionManagerRegistry {
    managers: RwLock<HashMap<String, SessionManagerRef>>,
}

impl SessionManagerRegistry {
    pub fn new() -> Self {
        Self {
            managers: RwLock::new(HashMap::new()),
        }
    }
    
    pub async fn get_or_create(&self, session_id: &str) -> Result<SessionManagerRef, String> {
        {
            let managers = self.managers.read().await;
            if let Some(manager) = managers.get(session_id) {
                return Ok(manager.clone());
            }
        }
        
        let manager = Arc::new(SessionManager::new(session_id).await?);
        
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
