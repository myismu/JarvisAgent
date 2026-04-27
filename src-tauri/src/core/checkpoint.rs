// --- Checkpoint Module ---
// 树状检查点系统，支持会话回滚、分支管理、多智能体沙箱隔离。

use crate::get_agent_home;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::fs;
use std::path::PathBuf;

pub const DIR_CHECKPOINTS: &str = ".checkpoints";
pub const FILE_BRANCHES: &str = "branches.json";
pub const DEFAULT_BRANCH: &str = "main";

// === 数据结构 ===

#[derive(Serialize, Deserialize, Debug, Clone)]
#[serde(rename_all = "camelCase")]
pub struct Checkpoint {
    pub id: String,
    pub session_id: String,
    pub parent_id: Option<String>,
    pub branch_name: String,
    pub agent_id: Option<String>,
    pub workspace_id: Option<String>,
    pub created_at: u64,
    pub trigger_message: String,
    pub operations: Vec<FileOperation>,
    pub metadata: HashMap<String, String>,
}

#[derive(Serialize, Deserialize, Debug, Clone)]
#[serde(rename_all = "camelCase")]
pub struct FileOperation {
    pub op_type: OpType,
    pub path: String,
    pub old_content_hash: Option<String>,
    pub backup_path: Option<String>,
    pub new_content_hash: Option<String>,
    pub diff_summary: Option<String>,
}

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq)]
#[serde(rename_all = "lowercase")]
pub enum OpType {
    Edit,
    Write,
    Create,
    Delete,
    Rename,
}

#[derive(Serialize, Deserialize, Debug, Clone)]
#[serde(rename_all = "camelCase")]
pub struct Branch {
    pub name: String,
    pub session_id: String,
    pub head_checkpoint_id: Option<String>,
    pub created_at: u64,
    pub agent_id: Option<String>,
    pub description: String,
    pub is_active: bool,
}

#[derive(Serialize, Deserialize, Debug, Clone, Default)]
pub struct BranchIndex {
    pub branches: HashMap<String, Branch>,
    pub active_branch: String,
}

#[derive(Serialize, Deserialize, Debug, Clone)]
#[serde(rename_all = "camelCase")]
pub struct CheckpointTree {
    pub session_id: String,
    pub branches: Vec<BranchInfo>,
    pub checkpoints: Vec<Checkpoint>,
}

#[derive(Serialize, Deserialize, Debug, Clone)]
#[serde(rename_all = "camelCase")]
pub struct BranchInfo {
    pub name: String,
    pub head_checkpoint_id: Option<String>,
    pub checkpoint_count: usize,
    pub is_active: bool,
}

// === 存储路径 ===

fn checkpoints_dir() -> PathBuf {
    let dir = get_agent_home().join(DIR_CHECKPOINTS);
    if !dir.exists() {
        let _ = fs::create_dir_all(&dir);
    }
    dir
}

fn session_checkpoints_dir(session_id: &str) -> PathBuf {
    let dir = checkpoints_dir().join(session_id);
    if !dir.exists() {
        let _ = fs::create_dir_all(&dir);
    }
    dir
}

fn branch_dir(session_id: &str, branch_name: &str) -> PathBuf {
    let dir = session_checkpoints_dir(session_id).join(branch_name);
    if !dir.exists() {
        let _ = fs::create_dir_all(&dir);
    }
    dir
}

fn backups_dir(session_id: &str, branch_name: &str) -> PathBuf {
    let dir = branch_dir(session_id, branch_name).join("backups");
    if !dir.exists() {
        let _ = fs::create_dir_all(&dir);
    }
    dir
}

fn checkpoint_file(session_id: &str, branch_name: &str, checkpoint_id: &str) -> PathBuf {
    branch_dir(session_id, branch_name).join(format!("{}.json", checkpoint_id))
}

fn branches_file(session_id: &str) -> PathBuf {
    session_checkpoints_dir(session_id).join(FILE_BRANCHES)
}

// === 工具函数 ===

pub fn content_hash(content: &[u8]) -> String {
    use std::collections::hash_map::DefaultHasher;
    use std::hash::{Hash, Hasher};
    let mut hasher = DefaultHasher::new();
    content.hash(&mut hasher);
    format!("{:016x}", hasher.finish())
}

fn current_timestamp() -> u64 {
    use std::time::{SystemTime, UNIX_EPOCH};
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
}

fn generate_id() -> String {
    use uuid::Uuid;
    Uuid::new_v4().to_string()[..8].to_string()
}

// === 分支管理 ===

pub fn load_branch_index(session_id: &str) -> BranchIndex {
    let path = branches_file(session_id);
    if path.exists() {
        fs::read_to_string(&path)
            .ok()
            .and_then(|s| serde_json::from_str(&s).ok())
            .unwrap_or_else(|| create_default_branch_index(session_id))
    } else {
        create_default_branch_index(session_id)
    }
}

fn create_default_branch_index(session_id: &str) -> BranchIndex {
    let mut branches = HashMap::new();
    branches.insert(
        DEFAULT_BRANCH.to_string(),
        Branch {
            name: DEFAULT_BRANCH.to_string(),
            session_id: session_id.to_string(),
            head_checkpoint_id: None,
            created_at: current_timestamp(),
            agent_id: None,
            description: "主分支".to_string(),
            is_active: true,
        },
    );
    BranchIndex {
        branches,
        active_branch: DEFAULT_BRANCH.to_string(),
    }
}

fn save_branch_index(session_id: &str, index: &BranchIndex) {
    let path = branches_file(session_id);
    if let Ok(json) = serde_json::to_string_pretty(index) {
        let _ = fs::write(&path, json);
    }
}

pub fn create_branch(
    session_id: &str,
    branch_name: &str,
    from_checkpoint_id: Option<&str>,
    agent_id: Option<&str>,
    description: Option<&str>,
) -> Branch {
    let mut index = load_branch_index(session_id);
    
    if index.branches.contains_key(branch_name) {
        return index.branches.get(branch_name).unwrap().clone();
    }
    
    let branch = Branch {
        name: branch_name.to_string(),
        session_id: session_id.to_string(),
        head_checkpoint_id: from_checkpoint_id.map(|s| s.to_string()),
        created_at: current_timestamp(),
        agent_id: agent_id.map(|s| s.to_string()),
        description: description.unwrap_or("新分支").to_string(),
        is_active: false,
    };
    
    index.branches.insert(branch_name.to_string(), branch.clone());
    save_branch_index(session_id, &index);
    
    branch
}

pub fn switch_branch(session_id: &str, branch_name: &str) -> Result<Branch, String> {
    let mut index = load_branch_index(session_id);
    
    let branch = index.branches.get(branch_name)
        .ok_or_else(|| format!("分支 '{}' 不存在", branch_name))?
        .clone();
    
    for b in index.branches.values_mut() {
        b.is_active = b.name == branch_name;
    }
    index.active_branch = branch_name.to_string();
    
    save_branch_index(session_id, &index);
    
    Ok(branch)
}

pub fn list_branches(session_id: &str) -> Vec<Branch> {
    let index = load_branch_index(session_id);
    index.branches.values().cloned().collect()
}

pub fn get_active_branch(session_id: &str) -> Branch {
    let index = load_branch_index(session_id);
    index.branches.get(&index.active_branch)
        .cloned()
        .unwrap_or_else(|| create_default_branch_index(session_id)
            .branches.get(DEFAULT_BRANCH)
            .unwrap()
            .clone())
}

pub fn delete_branch(session_id: &str, branch_name: &str) -> Result<(), String> {
    if branch_name == DEFAULT_BRANCH {
        return Err("无法删除主分支".to_string());
    }
    
    let mut index = load_branch_index(session_id);
    
    if index.active_branch == branch_name {
        return Err("无法删除当前活跃分支，请先切换到其他分支".to_string());
    }
    
    if index.branches.remove(branch_name).is_none() {
        return Err(format!("分支 '{}' 不存在", branch_name));
    }
    
    save_branch_index(session_id, &index);
    
    let branch_path = branch_dir(session_id, branch_name);
    if branch_path.exists() {
        let _ = fs::remove_dir_all(&branch_path);
    }
    
    Ok(())
}

// === 检查点管理 ===

pub fn create_checkpoint(
    session_id: &str,
    parent_id: Option<&str>,
    trigger_message: &str,
    agent_id: Option<&str>,
    workspace_id: Option<&str>,
    operations: Vec<FileOperation>,
) -> Checkpoint {
    let branch = get_active_branch(session_id);
    let checkpoint_id = format!("cp_{}", generate_id());
    
    let checkpoint = Checkpoint {
        id: checkpoint_id.clone(),
        session_id: session_id.to_string(),
        parent_id: parent_id.map(|s| s.to_string()),
        branch_name: branch.name.clone(),
        agent_id: agent_id.map(|s| s.to_string()),
        workspace_id: workspace_id.map(|s| s.to_string()),
        created_at: current_timestamp(),
        trigger_message: trigger_message.to_string(),
        operations,
        metadata: HashMap::new(),
    };
    
    save_checkpoint(&checkpoint);
    
    let mut index = load_branch_index(session_id);
    if let Some(b) = index.branches.get_mut(&branch.name) {
        b.head_checkpoint_id = Some(checkpoint_id);
    }
    save_branch_index(session_id, &index);
    
    checkpoint
}

fn save_checkpoint(checkpoint: &Checkpoint) {
    let path = checkpoint_file(
        &checkpoint.session_id,
        &checkpoint.branch_name,
        &checkpoint.id,
    );
    if let Ok(json) = serde_json::to_string_pretty(checkpoint) {
        let _ = fs::write(&path, json);
    }
}

pub fn load_checkpoint(session_id: &str, branch_name: &str, checkpoint_id: &str) -> Option<Checkpoint> {
    let path = checkpoint_file(session_id, branch_name, checkpoint_id);
    fs::read_to_string(&path)
        .ok()
        .and_then(|s| serde_json::from_str(&s).ok())
}

pub fn list_checkpoints(session_id: &str, branch_name: Option<&str>) -> Vec<Checkpoint> {
    let branch = branch_name.map(|s| s.to_string())
        .unwrap_or_else(|| get_active_branch(session_id).name);
    
    let branch_path = branch_dir(session_id, &branch);
    let mut checkpoints: Vec<Checkpoint> = Vec::new();
    
    if let Ok(entries) = fs::read_dir(&branch_path) {
        for entry in entries.flatten() {
            let path = entry.path();
            if path.extension().map(|e| e == "json").unwrap_or(false) {
                if let Some(cp) = fs::read_to_string(&path)
                    .ok()
                    .and_then(|s| serde_json::from_str(&s).ok())
                {
                    checkpoints.push(cp);
                }
            }
        }
    }
    
    checkpoints.sort_by_key(|c| c.created_at);
    checkpoints
}

pub fn get_checkpoint_chain(session_id: &str, checkpoint_id: &str) -> Vec<Checkpoint> {
    let mut chain = Vec::new();
    let mut current_id = Some(checkpoint_id.to_string());
    
    while let Some(ref id) = current_id {
        let mut found = false;
        for branch in list_branches(session_id) {
            if let Some(cp) = load_checkpoint(session_id, &branch.name, id) {
                current_id = cp.parent_id.clone();
                chain.push(cp);
                found = true;
                break;
            }
        }
        if !found {
            break;
        }
    }
    
    chain
}

pub fn get_checkpoint_tree(session_id: &str) -> CheckpointTree {
    let branches = list_branches(session_id);
    let mut all_checkpoints = Vec::new();
    let mut branch_infos = Vec::new();
    
    for branch in &branches {
        let checkpoints = list_checkpoints(session_id, Some(&branch.name));
        branch_infos.push(BranchInfo {
            name: branch.name.clone(),
            head_checkpoint_id: branch.head_checkpoint_id.clone(),
            checkpoint_count: checkpoints.len(),
            is_active: branch.is_active,
        });
        all_checkpoints.extend(checkpoints);
    }
    
    all_checkpoints.sort_by_key(|c| c.created_at);
    
    CheckpointTree {
        session_id: session_id.to_string(),
        branches: branch_infos,
        checkpoints: all_checkpoints,
    }
}

pub fn delete_checkpoint(session_id: &str, branch_name: &str, checkpoint_id: &str) -> Result<(), String> {
    let path = checkpoint_file(session_id, branch_name, checkpoint_id);
    if path.exists() {
        let _ = fs::remove_file(&path);
    }
    Ok(())
}

// === 文件备份 ===

pub fn backup_file(
    session_id: &str,
    branch_name: &str,
    file_path: &str,
    content: &[u8],
) -> Option<String> {
    let hash = content_hash(content);
    let backup_dir = backups_dir(session_id, branch_name);
    
    let filename = format!("{}_{}", 
        hash,
        file_path.replace(['/', '\\', ':'], "_")
    );
    let backup_path = backup_dir.join(&filename);
    
    if !backup_path.exists() {
        if fs::write(&backup_path, content).is_ok() {
            return Some(backup_path.to_string_lossy().to_string());
        }
    } else {
        return Some(backup_path.to_string_lossy().to_string());
    }
    
    None
}

pub fn restore_file(backup_path: &str, target_path: &str) -> Result<(), String> {
    let backup = PathBuf::from(backup_path);
    if !backup.exists() {
        return Err(format!("备份文件不存在: {}", backup_path));
    }
    
    fs::copy(&backup, target_path)
        .map(|_| ())
        .map_err(|e| format!("恢复文件失败: {}", e))
}

// === 回滚 ===

pub fn rollback_to_checkpoint(session_id: &str, checkpoint_id: &str) -> Result<Vec<String>, String> {
    // 获取所有 checkpoints
    let all_checkpoints = list_checkpoints(session_id, None);

    // 找到目标 checkpoint 的索引
    let target_idx = all_checkpoints
        .iter()
        .position(|cp| cp.id == checkpoint_id)
        .ok_or_else(|| format!("检查点 '{}' 不存在", checkpoint_id))?;

    // 获取该 checkpoint 及之后的所有 checkpoints（需要回滚这些操作）
    let checkpoints_to_rollback: Vec<&Checkpoint> = all_checkpoints[target_idx..].iter().collect();

    let mut restored_files = Vec::new();

    // 从最新的 checkpoint 开始，逆序回滚到目标 checkpoint
    for checkpoint in checkpoints_to_rollback.iter().rev() {
        for op in checkpoint.operations.iter().rev() {
            match op.op_type {
                OpType::Edit | OpType::Write => {
                    if let Some(backup_path) = &op.backup_path {
                        restore_file(backup_path, &op.path)?;
                        restored_files.push(op.path.clone());
                    }
                }
                OpType::Create => {
                    if std::path::Path::new(&op.path).exists() {
                        let _ = fs::remove_file(&op.path);
                        restored_files.push(format!("{} (已删除)", op.path));
                    }
                }
                OpType::Delete => {
                    if let Some(backup_path) = &op.backup_path {
                        restore_file(backup_path, &op.path)?;
                        restored_files.push(format!("{} (已恢复)", op.path));
                    }
                }
                OpType::Rename => {
                    // TODO: 处理重命名回滚
                }
            }
        }
    }

    Ok(restored_files)
}

// === 辅助函数：获取最新检查点 ===

pub fn get_latest_checkpoint(session_id: &str) -> Option<Checkpoint> {
    let branch = get_active_branch(session_id);
    branch.head_checkpoint_id
        .and_then(|id| load_checkpoint(session_id, &branch.name, &id))
}

pub fn get_head_checkpoint_id(session_id: &str) -> Option<String> {
    let branch = get_active_branch(session_id);
    branch.head_checkpoint_id
}
