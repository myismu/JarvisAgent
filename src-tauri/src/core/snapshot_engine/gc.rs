use super::snapshot::SnapshotTree;
use std::collections::HashSet;

#[derive(Clone, Debug)]
pub struct GcConfig {
    pub max_checkpoints: usize,
    pub max_age_days: u64,
    pub max_total_size_mb: u64,
    pub keep_branch_heads: bool,
}

impl Default for GcConfig {
    fn default() -> Self {
        Self {
            max_checkpoints: 100,
            max_age_days: 30,
            max_total_size_mb: 500,
            keep_branch_heads: true,
        }
    }
}

#[derive(Default, Debug)]
pub struct GcResult {
    pub removed_snapshots: usize,
    pub removed_branches: usize,
    pub space_freed: u64,
}

pub struct GarbageCollector {
    config: GcConfig,
}

impl GarbageCollector {
    pub fn new(config: GcConfig) -> Self {
        Self { config }
    }
    
    pub fn collect<F>(&self, tree: &mut SnapshotTree, mut delete_snapshot: F) -> GcResult
    where F: FnMut(&str, &str),
    {
        let mut result = GcResult::default();

        let protected_ids = if self.config.keep_branch_heads {
            tree.get_protected_ids()
        } else {
            HashSet::new()
        };

        let mut to_remove: Vec<(String, String)> = Vec::new();

        for (id, snapshot) in &tree.nodes {
            if protected_ids.contains(id) {
                continue;
            }

            if self.should_remove(snapshot) {
                to_remove.push((id.clone(), snapshot.branch_name.clone()));
            }
        }

        for (id, branch_name) in &to_remove {
            tree.nodes.remove(id);
            delete_snapshot(id, branch_name);
            result.removed_snapshots += 1;
        }

        let orphan_branches = self.find_orphan_branches(tree);
        for branch_name in orphan_branches {
            if branch_name != "main" {
                tree.branches.remove(&branch_name);
                result.removed_branches += 1;
            }
        }

        result
    }
    
    fn should_remove(&self, snapshot: &super::snapshot::Snapshot) -> bool {
        let age_days = (current_timestamp() - snapshot.created_at) / (24 * 60 * 60);
        
        if age_days > self.config.max_age_days {
            return true;
        }
        
        false
    }
    
    fn find_orphan_branches(&self, tree: &SnapshotTree) -> Vec<String> {
        tree.branches.keys()
            .filter(|name| {
                let branch = &tree.branches[*name];
                !branch.head_snapshot_id.is_empty() && !tree.nodes.contains_key(&branch.head_snapshot_id)
            })
            .cloned()
            .collect()
    }
}

fn current_timestamp() -> u64 {
    use std::time::{SystemTime, UNIX_EPOCH};
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
}
