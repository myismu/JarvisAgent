//! 垃圾回收模块
//!
//! 清理过期快照和孤立分支，释放存储空间。

use super::snapshot::SnapshotTree;
use std::collections::HashSet;

/// GC 配置参数
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

/// GC 执行结果统计
#[derive(Default, Debug)]
pub struct GcResult {
    pub removed_snapshots: usize,
    pub removed_branches: usize,
    pub space_freed: u64,
}

/// 垃圾回收器
pub struct GarbageCollector {
    config: GcConfig,
}

impl GarbageCollector {
    pub fn new(config: GcConfig) -> Self {
        Self { config }
    }
    
    /// 执行垃圾回收
    ///
    /// 删除过期快照和孤立分支，保护分支头节点不被删除
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
    
    /// 判断快照是否应被删除（基于存活天数）
    fn should_remove(&self, snapshot: &super::snapshot::Snapshot) -> bool {
        let age_days = (current_timestamp() - snapshot.created_at) / (24 * 60 * 60);
        
        if age_days > self.config.max_age_days {
            return true;
        }
        
        false
    }
    
    /// 查找孤立分支（头节点已被删除的分支）
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
