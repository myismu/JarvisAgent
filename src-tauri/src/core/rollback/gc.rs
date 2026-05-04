//! 垃圾回收模块
//!
//! 清理过期快照、孤立分支和孤儿内容，释放存储空间。
//!
//! GC 分三个阶段：
//! 1. 清理快照树中脱离分支链路且超期的快照节点
//! 2. 清理 snapshots 表中不在快照树 nodes 中的孤儿记录
//! 3. 清理 snapshot_content 表中未被任何 workspaceState 引用的孤儿内容

use super::snapshot::SnapshotTree;
use std::collections::HashSet;

/// GC 配置参数
#[derive(Clone, Debug)]
pub struct GcConfig {
    /// 最多保留多少个检查点（预留，当前未使用）
    pub max_checkpoints: usize,
    /// 快照最大存活天数（默认 30 天）
    pub max_age_days: u64,
    /// 总存储上限 MB（预留，当前未使用）
    pub max_total_size_mb: u64,
    /// 是否保护分支头节点及其祖先链路（默认 true）
    pub keep_branch_heads: bool,
    /// 是否立即清理脱离分支链路的快照（默认 false）
    ///
    /// 当为 true 时，不在任何分支 HEAD→root 链路上的快照将立即被清理，
    /// 不再等待 max_age_days 超期。适用于回滚后希望立即释放空间的场景。
    /// 当为 false 时，脱离链路的快照仍需等待 max_age_days 到期才会被清理。
    pub immediate_detach: bool,
}

impl Default for GcConfig {
    fn default() -> Self {
        Self {
            max_checkpoints: 100,
            max_age_days: 30,
            max_total_size_mb: 500,
            keep_branch_heads: true,
            immediate_detach: false,
        }
    }
}

/// GC 执行结果统计
#[derive(Default, Debug, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct GcResult {
    /// 从快照树中移除的过期快照数
    pub removed_tree_snapshots: usize,
    /// 从 snapshots 表中清理的孤儿记录数
    pub removed_orphan_snapshots: usize,
    /// 从 snapshot_content 表中清理的孤儿内容数
    pub removed_orphan_contents: usize,
    /// 清理的孤立分支数
    pub removed_branches: usize,
}

/// 垃圾回收器
pub struct GarbageCollector {
    config: GcConfig,
}

impl GarbageCollector {
    pub fn new(config: GcConfig) -> Self {
        Self { config }
    }

    /// 执行完整垃圾回收（三阶段）
    ///
    /// 阶段 1：清理快照树中脱离链路且超期的快照节点
    /// 阶段 2：清理 snapshots 表中不在 tree.nodes 中的孤儿记录
    /// 阶段 3：清理 snapshot_content 表中未被任何 workspaceState 引用的孤儿内容
    pub fn collect(&self, tree: &mut SnapshotTree, session_id: &str) -> GcResult {
        let mut result = GcResult::default();

        // ═══ 阶段 1：清理快照树中脱离链路且超期的快照 ═══
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
            // 同步删除 snapshots 表中的对应记录
            if let Err(e) = delete_snapshot_from_db(session_id, branch_name, id) {
                eprintln!("[GC] 删除快照 {} 失败: {}", id, e);
            }
            result.removed_tree_snapshots += 1;
        }

        // 清理孤立分支
        let orphan_branches = self.find_orphan_branches(tree);
        for branch_name in orphan_branches {
            if branch_name != "main" {
                tree.branches.remove(&branch_name);
                result.removed_branches += 1;
            }
        }

        // ═══ 阶段 2：清理 snapshots 表中的孤儿记录 ═══
        result.removed_orphan_snapshots = self.cleanup_orphan_snapshots(session_id, tree);

        // ═══ 阶段 3：清理 snapshot_content 表中的孤儿内容 ═══
        result.removed_orphan_contents = self.cleanup_orphan_contents(session_id, tree);

        result
    }

    /// 判断快照是否应被删除
    ///
    /// 当 `immediate_detach` 为 true 时，脱离链路的快照立即被删除（不受年龄限制）；
    /// 否则，需要超过 max_age_days 才会被删除。
    fn should_remove(&self, snapshot: &super::snapshot::Snapshot) -> bool {
        if self.config.immediate_detach {
            // 立即清理模式：脱离链路即删除，不看年龄
            true
        } else {
            let age_days = (current_timestamp() - snapshot.created_at) / (24 * 60 * 60);
            age_days > self.config.max_age_days
        }
    }

    /// 查找孤立分支（头节点已被删除的分支）
    fn find_orphan_branches(&self, tree: &SnapshotTree) -> Vec<String> {
        tree.branches
            .keys()
            .filter(|name| {
                let branch = &tree.branches[*name];
                !branch.head_snapshot_id.is_empty()
                    && !tree.nodes.contains_key(&branch.head_snapshot_id)
            })
            .cloned()
            .collect()
    }

    /// 阶段 2：清理 snapshots 表中不在 tree.nodes 中的孤儿记录
    ///
    /// 扫描 snapshots 表中的所有 snapshot_id，与 tree.nodes 做差集，
    /// 不在 tree.nodes 中的即为孤儿，直接删除。
    fn cleanup_orphan_snapshots(&self, session_id: &str, tree: &SnapshotTree) -> usize {
        let tree_node_ids: HashSet<String> = tree.nodes.keys().cloned().collect();

        crate::core::db::with_connection(|conn| {
            // 查询该会话的所有 snapshot_id
            let mut stmt = conn
                .prepare("SELECT snapshot_id, branch_name FROM snapshots WHERE session_id = ?1")
                .map_err(|e| e.to_string())?;

            let rows: Vec<(String, String)> = stmt
                .query_map([session_id], |row| {
                    Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
                })
                .map_err(|e| e.to_string())?
                .filter_map(|r| r.ok())
                .collect();

            let mut removed = 0;
            for (snapshot_id, _branch_name) in rows {
                if !tree_node_ids.contains(&snapshot_id) {
                    match conn.execute(
                        "DELETE FROM snapshots WHERE session_id = ?1 AND snapshot_id = ?2",
                        rusqlite::params![session_id, snapshot_id],
                    ) {
                        Ok(_) => removed += 1,
                        Err(e) => eprintln!("[GC] 删除孤儿快照 {} 失败: {}", snapshot_id, e),
                    }
                }
            }

            Ok(removed)
        })
        .unwrap_or(0)
    }

    /// 阶段 3：清理 snapshot_content 表中未被任何 workspaceState 引用的孤儿内容
    ///
    /// 收集 tree.nodes 中所有 workspaceState 引用的 hash，
    /// 然后删除 snapshot_content 表中不在该集合中的记录。
    fn cleanup_orphan_contents(&self, session_id: &str, tree: &SnapshotTree) -> usize {
        // 收集所有被 workspaceState 引用的 hash
        let mut referenced_hashes: HashSet<String> = HashSet::new();
        for snapshot in tree.nodes.values() {
            if let Some(ws) = &snapshot.workspace_state {
                for file_info in ws.files.values() {
                    referenced_hashes.insert(file_info.hash.clone());
                }
            }
        }

        crate::core::db::with_connection(|conn| {
            // 查询该会话的所有 content_hash
            let mut stmt = conn
                .prepare("SELECT content_hash FROM snapshot_content WHERE session_id = ?1")
                .map_err(|e| e.to_string())?;

            let hashes: Vec<String> = stmt
                .query_map([session_id], |row| row.get::<_, String>(0))
                .map_err(|e| e.to_string())?
                .filter_map(|r| r.ok())
                .collect();

            let mut removed = 0;
            for hash in hashes {
                if !referenced_hashes.contains(&hash) {
                    match conn.execute(
                        "DELETE FROM snapshot_content WHERE session_id = ?1 AND content_hash = ?2",
                        rusqlite::params![session_id, hash],
                    ) {
                        Ok(_) => removed += 1,
                        Err(e) => eprintln!("[GC] 删除孤儿内容 {} 失败: {}", hash, e),
                    }
                }
            }

            Ok(removed)
        })
        .unwrap_or(0)
    }
}

/// 从数据库删除单条快照记录
fn delete_snapshot_from_db(
    session_id: &str,
    branch_name: &str,
    snapshot_id: &str,
) -> Result<(), String> {
    crate::core::db::with_connection(|conn| {
        conn.execute(
            "DELETE FROM snapshots WHERE session_id = ?1 AND branch_name = ?2 AND snapshot_id = ?3",
            rusqlite::params![session_id, branch_name, snapshot_id],
        )
        .map_err(|e| e.to_string())?;
        Ok(())
    })
}

fn current_timestamp() -> u64 {
    use std::time::{SystemTime, UNIX_EPOCH};
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
}
