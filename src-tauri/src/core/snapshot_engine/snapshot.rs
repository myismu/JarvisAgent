//! 快照与快照树模块
//!
//! 定义快照数据结构和版本树管理逻辑，支持：
//! - 线性快照链（parent_id 指针）
//! - 多分支管理（main 分支 + 代理分支）
//! - 检查点机制（定期保存完整工作区状态）
//! - 树形视图（前端展示用）

use super::patch::{Patch, PatchSummary};
use serde::{Deserialize, Serialize};
use std::collections::{HashMap, HashSet};
use std::time::{SystemTime, UNIX_EPOCH};

/// 单个快照，记录一次文件变更操作
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Snapshot {
    pub id: String,
    pub parent_id: Option<String>,
    pub branch_name: String,
    pub patches: Vec<Patch>,
    pub message: Option<String>,
    pub is_checkpoint: bool,
    pub workspace_state: Option<WorkspaceState>,
    pub agent_id: Option<String>,
    pub workspace_id: Option<String>,
    pub created_at: u64,
    pub metadata: HashMap<String, String>,
}

/// 工作区状态快照（用于检查点，记录文件哈希和大小）
#[derive(Clone, Debug, Serialize, Deserialize, Default)]
pub struct WorkspaceState {
    pub files: HashMap<String, FileInfo>,
}

/// 文件元信息（哈希 + 大小）
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct FileInfo {
    pub hash: String,
    pub size: u64,
}

/// 内存中的工作区（文件路径 -> 内容映射）
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct Workspace {
    pub files: HashMap<String, String>,
}

/// 快照版本树，管理所有快照和分支
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SnapshotTree {
    pub nodes: HashMap<String, Snapshot>,
    pub branches: HashMap<String, Branch>,
    pub current_branch: String,
    pub current_snapshot_id: String,
    pub session_id: String,
}

/// 分支信息
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Branch {
    pub name: String,
    pub session_id: String,
    pub head_snapshot_id: String,
    pub created_at: u64,
    pub agent_id: Option<String>,
    pub description: String,
    pub is_active: bool,
}

/// 快照树视图（前端展示用）
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SnapshotTreeView {
    pub branches: Vec<BranchView>,
    pub current_branch: String,
    pub current_snapshot_id: String,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct BranchView {
    pub name: String,
    pub description: String,
    pub agent_id: Option<String>,
    pub is_active: bool,
    pub root: SnapshotNode,
}

/// 快照树节点（递归结构）
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SnapshotNode {
    pub id: String,
    pub message: Option<String>,
    pub timestamp: u64,
    pub is_checkpoint: bool,
    pub agent_id: Option<String>,
    pub children: Vec<SnapshotNode>,
}

/// 快照摘要（精简信息，用于列表展示）
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SnapshotSummary {
    pub id: String,
    pub message: Option<String>,
    pub timestamp: u64,
    pub is_checkpoint: bool,
    pub agent_id: Option<String>,
    pub patch_count: usize,
    pub patch_summary: Vec<PatchSummary>,
}

impl Workspace {
    pub fn new() -> Self {
        Self {
            files: HashMap::new(),
        }
    }
    
    /// 应用单个补丁到工作区
    pub fn apply_patch(&mut self, patch: &Patch) -> Result<(), super::patch::PatchError> {
        use super::patch::PatchError;
        
        match patch {
            Patch::CreateFile { path, content } => {
                if self.files.contains_key(path) {
                    return Err(PatchError::FileAlreadyExists(path.clone()));
                }
                self.files.insert(path.clone(), content.clone());
                Ok(())
            }
            Patch::DeleteFile { path } => {
                if self.files.remove(path).is_none() {
                    return Err(PatchError::FileNotFound(path.clone()));
                }
                Ok(())
            }
            Patch::UpdateFile { path, old_content, new_content, .. } => {
                let current = self.files.get(path)
                    .ok_or_else(|| PatchError::FileNotFound(path.clone()))?;
                if current != old_content {
                    return Err(PatchError::HashMismatch {
                        expected: Patch::content_hash(old_content),
                        actual: Patch::content_hash(current),
                    });
                }
                self.files.insert(path.clone(), new_content.clone());
                Ok(())
            }
            Patch::RenameFile { old_path, new_path } => {
                let content = self.files.remove(old_path)
                    .ok_or_else(|| PatchError::FileNotFound(old_path.clone()))?;
                if self.files.contains_key(new_path) {
                    self.files.insert(old_path.clone(), content);
                    return Err(PatchError::FileAlreadyExists(new_path.clone()));
                }
                self.files.insert(new_path.clone(), content);
                Ok(())
            }
        }
    }
    
    /// 批量应用补丁
    pub fn apply_patches(&mut self, patches: &[Patch]) -> Result<(), super::patch::PatchError> {
        for patch in patches {
            self.apply_patch(patch)?;
        }
        Ok(())
    }
    
    /// 撤销单个补丁（用于回滚）
    pub fn undo_patch(&mut self, patch: &Patch) -> Result<(), super::patch::PatchError> {
        match patch {
            Patch::CreateFile { path, .. } => {
                self.files.remove(path);
                Ok(())
            }
            Patch::DeleteFile { path: _ } => {
                Ok(())
            }
            Patch::UpdateFile { path, old_content, .. } => {
                self.files.insert(path.clone(), old_content.clone());
                Ok(())
            }
            Patch::RenameFile { old_path, new_path } => {
                if let Some(content) = self.files.remove(new_path) {
                    self.files.insert(old_path.clone(), content);
                }
                Ok(())
            }
        }
    }
}

impl Snapshot {
    pub fn to_summary(&self) -> SnapshotSummary {
        SnapshotSummary {
            id: self.id.clone(),
            message: self.message.clone(),
            timestamp: self.created_at,
            is_checkpoint: self.is_checkpoint,
            agent_id: self.agent_id.clone(),
            patch_count: self.patches.len(),
            patch_summary: self.patches.iter().map(|p| p.to_summary()).collect(),
        }
    }
}

impl SnapshotTree {
    /// 创建新的快照树（自动初始化 main 分支）
    pub fn new(session_id: &str) -> Self {
        let main_branch = Branch {
            name: "main".to_string(),
            session_id: session_id.to_string(),
            head_snapshot_id: String::new(),
            created_at: current_timestamp(),
            agent_id: None,
            description: "主分支".to_string(),
            is_active: true,
        };
        
        let mut branches = HashMap::new();
        branches.insert("main".to_string(), main_branch);
        
        Self {
            nodes: HashMap::new(),
            branches,
            current_branch: "main".to_string(),
            current_snapshot_id: String::new(),
            session_id: session_id.to_string(),
        }
    }
    
    /// 创建新快照并追加到当前分支
    pub fn create_snapshot(
        &mut self,
        patches: Vec<Patch>,
        message: Option<String>,
        agent_id: Option<String>,
        workspace_id: Option<String>,
        is_checkpoint: bool,
        workspace_state: Option<WorkspaceState>,
    ) -> Snapshot {
        let id = generate_id();
        let parent_id = if self.current_snapshot_id.is_empty() {
            None
        } else {
            Some(self.current_snapshot_id.clone())
        };

        let snapshot = Snapshot {
            id: id.clone(),
            parent_id,
            branch_name: self.current_branch.clone(),
            patches,
            message,
            is_checkpoint,
            workspace_state,
            agent_id,
            workspace_id,
            created_at: current_timestamp(),
            metadata: HashMap::new(),
        };

        self.nodes.insert(id.clone(), snapshot.clone());
        self.current_snapshot_id = id.clone();

        if let Some(branch) = self.branches.get_mut(&self.current_branch) {
            branch.head_snapshot_id = id;
        }

        snapshot
    }
    
    /// 判断是否需要创建检查点（基于补丁数量阈值）
    pub fn should_create_checkpoint(&self) -> bool {
        self.count_patches_since_last_checkpoint() >= CHECKPOINT_INTERVAL
    }

    pub fn count_patches_since_last_checkpoint(&self) -> usize {
        let mut count = 0;
        let mut current_id = Some(self.current_snapshot_id.clone());
        
        while let Some(id) = current_id {
            if let Some(snapshot) = self.nodes.get(&id) {
                if snapshot.is_checkpoint {
                    break;
                }
                count += snapshot.patches.len();
                current_id = snapshot.parent_id.clone();
            } else {
                break;
            }
        }
        
        count
    }
    
    /// 从指定快照创建新分支
    pub fn create_branch(
        &mut self,
        branch_name: String,
        from_snapshot_id: Option<String>,
        agent_id: Option<String>,
        description: Option<String>,
    ) -> Result<Branch, String> {
        if self.branches.contains_key(&branch_name) {
            return Err(format!("分支 '{}' 已存在", branch_name));
        }
        
        let head_id = from_snapshot_id.unwrap_or_else(|| self.current_snapshot_id.clone());
        
        let branch = Branch {
            name: branch_name.clone(),
            session_id: self.session_id.clone(),
            head_snapshot_id: head_id,
            created_at: current_timestamp(),
            agent_id,
            description: description.unwrap_or_else(|| "新分支".to_string()),
            is_active: false,
        };
        
        self.branches.insert(branch_name, branch.clone());
        Ok(branch)
    }
    
    /// 切换到指定分支
    pub fn switch_branch(&mut self, branch_name: &str) -> Result<(), String> {
        let _branch = self.branches.get(branch_name)
            .ok_or_else(|| format!("分支 '{}' 不存在", branch_name))?
            .clone();
        
        for b in self.branches.values_mut() {
            b.is_active = b.name == branch_name;
        }
        
        self.current_branch = branch_name.to_string();
        self.current_snapshot_id = _branch.head_snapshot_id.clone();
        
        Ok(())
    }
    
    /// 生成前端展示用的树形视图
    pub fn to_view(&self) -> SnapshotTreeView {
        let branches: Vec<BranchView> = self.branches.values()
            .filter_map(|branch| {
                if branch.head_snapshot_id.is_empty() {
                    return Some(BranchView {
                        name: branch.name.clone(),
                        description: branch.description.clone(),
                        agent_id: branch.agent_id.clone(),
                        is_active: branch.is_active,
                        root: SnapshotNode {
                            id: String::new(),
                            message: Some("空分支".to_string()),
                            timestamp: branch.created_at,
                            is_checkpoint: false,
                            agent_id: None,
                            children: vec![],
                        },
                    });
                }
                
                let root = self.build_tree_from(&branch.head_snapshot_id);
                Some(BranchView {
                    name: branch.name.clone(),
                    description: branch.description.clone(),
                    agent_id: branch.agent_id.clone(),
                    is_active: branch.is_active,
                    root,
                })
            })
            .collect();
        
        SnapshotTreeView {
            branches,
            current_branch: self.current_branch.clone(),
            current_snapshot_id: self.current_snapshot_id.clone(),
        }
    }
    
    fn build_tree_from(&self, snapshot_id: &str) -> SnapshotNode {
        let snapshot = match self.nodes.get(snapshot_id) {
            Some(s) => s,
            None => return SnapshotNode {
                id: snapshot_id.to_string(),
                message: None,
                timestamp: 0,
                is_checkpoint: false,
                agent_id: None,
                children: vec![],
            },
        };
        
        let children: Vec<SnapshotNode> = self.nodes
            .values()
            .filter(|s| s.parent_id.as_deref() == Some(snapshot_id))
            .map(|s| self.build_tree_from(&s.id))
            .collect();
        
        SnapshotNode {
            id: snapshot.id.clone(),
            message: snapshot.message.clone(),
            timestamp: snapshot.created_at,
            is_checkpoint: snapshot.is_checkpoint,
            agent_id: snapshot.agent_id.clone(),
            children,
        }
    }
    
    /// 获取所有受保护的快照 ID（分支头节点及其祖先，GC 不可删除）
    pub fn get_protected_ids(&self) -> HashSet<String> {
        let mut protected = HashSet::new();
        
        for branch in self.branches.values() {
            if !branch.head_snapshot_id.is_empty() {
                protected.insert(branch.head_snapshot_id.clone());
                
                let mut current = Some(branch.head_snapshot_id.clone());
                while let Some(id) = current {
                    if protected.contains(&id) {
                        break;
                    }
                    protected.insert(id.clone());
                    current = self.nodes.get(&id).and_then(|s| s.parent_id.clone());
                }
            }
        }
        
        protected
    }
}

/// 检查点创建间隔（每 10 个补丁自动创建一次）
const CHECKPOINT_INTERVAL: usize = 10;

fn current_timestamp() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
}

fn generate_id() -> String {
    use uuid::Uuid;
    format!("snap_{}", Uuid::new_v4().to_string()[..8].to_string())
}
