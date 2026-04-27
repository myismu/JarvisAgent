use crate::core::snapshot_engine::{Snapshot, SnapshotTree};
use std::fs;
use std::path::PathBuf;

#[derive(Debug, thiserror::Error)]
pub enum StoreError {
    #[error("IO error: {0}")]
    IoError(#[from] std::io::Error),
    #[error("JSON error: {0}")]
    JsonError(#[from] serde_json::Error),
}

pub struct SnapshotStore {
    base_dir: PathBuf,
}

impl SnapshotStore {
    pub fn new(base_dir: PathBuf) -> Self {
        Self { base_dir }
    }
    
    pub fn save_snapshot(&self, snapshot: &Snapshot) -> Result<(), StoreError> {
        let branch_dir = self.base_dir.join(&snapshot.branch_name);
        fs::create_dir_all(&branch_dir)?;
        
        let snapshot_path = branch_dir.join(format!("{}.json", snapshot.id));
        let json = serde_json::to_string_pretty(snapshot)?;
        fs::write(&snapshot_path, json)?;
        
        Ok(())
    }
    
    pub fn load_snapshot(&self, branch_name: &str, snapshot_id: &str) -> Result<Option<Snapshot>, StoreError> {
        let snapshot_path = self.base_dir.join(branch_name).join(format!("{}.json", snapshot_id));
        
        if !snapshot_path.exists() {
            return Ok(None);
        }
        
        let json = fs::read_to_string(&snapshot_path)?;
        let snapshot: Snapshot = serde_json::from_str(&json)?;
        Ok(Some(snapshot))
    }
    
    pub fn delete_snapshot(&self, branch_name: &str, snapshot_id: &str) -> Result<(), StoreError> {
        let snapshot_path = self.base_dir.join(branch_name).join(format!("{}.json", snapshot_id));
        
        if snapshot_path.exists() {
            fs::remove_file(&snapshot_path)?;
        }
        
        Ok(())
    }
    
    pub fn save_tree(&self, tree: &SnapshotTree) -> Result<(), StoreError> {
        let tree_path = self.base_dir.join("tree.json");
        let json = serde_json::to_string_pretty(tree)?;
        fs::write(&tree_path, json)?;
        
        Ok(())
    }
    
    pub fn load_tree(&self) -> Result<SnapshotTree, StoreError> {
        let tree_path = self.base_dir.join("tree.json");
        
        if !tree_path.exists() {
            return Err(StoreError::IoError(std::io::Error::new(
                std::io::ErrorKind::NotFound,
                "Tree file not found",
            )));
        }
        
        let json = fs::read_to_string(&tree_path)?;
        let tree: SnapshotTree = serde_json::from_str(&json)?;
        Ok(tree)
    }
    
    pub fn list_snapshots(&self, branch_name: &str) -> Result<Vec<Snapshot>, StoreError> {
        let branch_dir = self.base_dir.join(branch_name);
        
        if !branch_dir.exists() {
            return Ok(Vec::new());
        }
        
        let mut snapshots = Vec::new();
        
        for entry in fs::read_dir(&branch_dir)? {
            let entry = entry?;
            let path = entry.path();
            
            if path.extension().map(|e| e == "json").unwrap_or(false) {
                if let Ok(json) = fs::read_to_string(&path) {
                    if let Ok(snapshot) = serde_json::from_str::<Snapshot>(&json) {
                        snapshots.push(snapshot);
                    }
                }
            }
        }
        
        snapshots.sort_by_key(|s| s.created_at);
        Ok(snapshots)
    }
}
