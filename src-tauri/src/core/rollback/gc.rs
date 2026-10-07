//! 垃圾回收模块
//!
//! 清理过期快照、孤立分支和孤儿内容，释放存储空间。
//!
//! GC 分三个阶段（**同一套口径：按引用判定，不按时间拍脑袋**）：
//! 1. 清理快照树中脱离分支链路且超期的快照节点
//! 2. 清理 snapshot_content 表中未被任何 workspaceState 引用的孤儿内容
//! 3. 清理回收站中未被任何快照引用的条目（`trash_path` 的引用关系）
//!
//! ## 触发时机
//! 由 `SnapshotRegistry::get_or_create` 在**每个会话本进程首次被打开时**跑一次
//! （见 `session_manager.rs`），会话被删除时则由 `session::delete_session` 整体清空。
//! 之所以不放在"启动时扫全部会话"：会话数量没有上限，而 GC 是按会话的，扫全量不划算。

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
    /// 从 snapshot_content 表中清理的孤儿内容数
    pub removed_orphan_contents: usize,
    /// 清理的孤立分支数
    pub removed_branches: usize,
    /// 从回收站清理的"未被任何快照引用"的条目数
    pub removed_trash_entries: usize,
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
    /// 阶段 2：清理 snapshot_content 表中未被任何 workspaceState 引用的孤儿内容
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

        for (id, _branch_name) in &to_remove {
            tree.nodes.remove(id);
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

        // ═══ 阶段 2：清理 snapshot_content 表中的孤儿内容 ═══
        result.removed_orphan_contents = self.cleanup_orphan_contents(session_id, tree);

        // ═══ 阶段 3：清理回收站中未被任何快照引用的条目 ═══
        result.removed_trash_entries = self.cleanup_orphan_trash(session_id, tree);

        result
    }

    /// 阶段 3：清理回收站里"没有任何快照引用"的条目。
    ///
    /// 回收站存在的唯一理由是给回滚提供本体（`Patch::DeleteFile.trash_path`），
    /// 所以**引用关系就是它的存活判据**：tree 里所有 patch 的 `trash_path` 构成
    /// "被引用集合"，回收站目录里不在该集合中的条目 = 已经不可能被任何回滚用到 → 可清。
    ///
    /// ⚠️ 不要改成"按时间/按大小清"：清早了会让回滚缺本体，而那种缺失事后无法补救
    /// （只能靠回滚时的告警告知用户）。
    fn cleanup_orphan_trash(&self, session_id: &str, tree: &SnapshotTree) -> usize {
        let mut referenced: HashSet<String> = HashSet::new();
        for snapshot in tree.nodes.values() {
            for patch in &snapshot.patches {
                if let super::patch::Patch::DeleteFile {
                    trash_path: Some(path),
                    ..
                } = patch
                {
                    referenced.insert(path.clone());
                }
            }
        }
        super::trash::remove_unreferenced(session_id, &referenced)
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
            // 时间戳口径为**毫秒**（v16 起），86_400_000 毫秒 = 1 天。
            // 若误用秒级换算（÷86400），毫秒差值会算出"数万年"的年龄，把全部快照误判为过期。
            let age_days = (current_timestamp() - snapshot.created_at) / (24 * 60 * 60 * 1000);
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

    /// 阶段 2：清理 snapshot_content 表中未被任何 workspaceState 引用的孤儿内容
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

        crate::infra::db::with_connection(|conn| {
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

/// 阶段 3 的专门测试：回收站里"没被任何快照引用"的条目该被清掉，被引用的必须留下。
///
/// 这条锁的是"删早了会让回滚缺本体"的风险 —— 判据只能是引用，不能是时间/大小。
#[cfg(test)]
mod trash_gc_tests {
    use super::super::patch::Patch;
    use super::super::snapshot::Snapshot;
    use super::*;

    #[test]
    fn gc_removes_only_unreferenced_trash_entries() {
        let _ = crate::AGENT_HOME_DIR.set(std::env::temp_dir().join("jarvisagent-gc-trash-test"));
        let session = "gc-trash-session";
        let root = super::super::trash::trash_root(session);
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(&root).unwrap();

        // 回收站里两条：一条被快照引用（回滚要用），一条没有任何引用
        let kept = root.join("111_kept.bin");
        let stale = root.join("222_stale.bin");
        std::fs::write(&kept, "x").unwrap();
        std::fs::write(&stale, "y").unwrap();

        let mut tree = SnapshotTree::new(session);
        let mut snap = Snapshot {
            id: "s1".to_string(),
            parent_id: None,
            branch_name: "main".to_string(),
            patches: vec![Patch::DeleteFile {
                path: "C:/ws/kept.bin".to_string(),
                content_hash: None,
                trash_path: Some(kept.to_string_lossy().to_string()),
            }],
            message: None,
            is_checkpoint: false,
            workspace_state: None,
            agent_id: None,
            workspace_id: None,
            created_at: current_timestamp(),
            metadata: Default::default(),
        };
        snap.patches.push(Patch::DeleteFile {
            path: "C:/ws/other.txt".to_string(),
            content_hash: Some("deadbeef".to_string()),
            // 有内容（文本文件）→ 不走回收站通道，trash_path 为空
            trash_path: None,
        });
        tree.nodes.insert("s1".to_string(), snap);

        let gc = GarbageCollector::new(GcConfig::default());
        let result = gc.collect(&mut tree, session);

        assert!(
            kept.exists(),
            "被快照引用的回收站条目必须留下，否则回滚会缺本体"
        );
        assert!(!stale.exists(), "没有被引用的回收站条目应被清掉");
        assert_eq!(result.removed_trash_entries, 1);
        let _ = std::fs::remove_dir_all(&root);
    }
}

/// 当前时间戳（**毫秒**，Unix epoch）——全项目 DB 时间戳统一毫秒口径（v16 起）。
fn current_timestamp() -> u64 {
    use std::time::{SystemTime, UNIX_EPOCH};
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as u64
}

#[cfg(test)]
mod tests {
    use super::super::snapshot::{Branch, Snapshot, SnapshotTree};
    use super::*;
    use std::collections::HashMap;

    fn make_snapshot(id: &str, branch: &str, created_at: u64) -> Snapshot {
        Snapshot {
            id: id.to_string(),
            parent_id: None,
            branch_name: branch.to_string(),
            patches: vec![],
            message: None,
            is_checkpoint: false,
            workspace_state: None,
            agent_id: None,
            workspace_id: None,
            created_at,
            metadata: HashMap::new(),
        }
    }

    fn make_branch(name: &str, head_id: &str) -> Branch {
        Branch {
            name: name.to_string(),
            session_id: "test".to_string(),
            head_snapshot_id: head_id.to_string(),
            created_at: current_timestamp(),
            agent_id: None,
            description: String::new(),
            is_active: true,
        }
    }

    fn make_tree() -> SnapshotTree {
        SnapshotTree {
            nodes: HashMap::new(),
            branches: HashMap::new(),
            current_branch: "main".to_string(),
            current_snapshot_id: String::new(),
            session_id: "test".to_string(),
            max_loaded_nodes: 200,
        }
    }

    #[test]
    fn test_gc_should_remove_old_snapshots() {
        let config = GcConfig {
            max_age_days: 30,
            immediate_detach: false,
            ..Default::default()
        };
        let gc = GarbageCollector::new(config);
        // 时间戳口径为毫秒：40 天 = 40 * 24 * 60 * 60 * 1000 毫秒
        let old = make_snapshot(
            "old",
            "main",
            current_timestamp() - 40 * 24 * 60 * 60 * 1000,
        );
        let recent = make_snapshot(
            "recent",
            "main",
            current_timestamp() - 5 * 24 * 60 * 60 * 1000,
        );
        assert!(gc.should_remove(&old));
        assert!(!gc.should_remove(&recent));
    }

    #[test]
    fn test_gc_immediate_detach_removes_all() {
        let config = GcConfig {
            immediate_detach: true,
            ..Default::default()
        };
        let gc = GarbageCollector::new(config);
        let snapshot = make_snapshot("s1", "main", current_timestamp());
        assert!(gc.should_remove(&snapshot));
    }

    #[test]
    fn test_find_orphan_branches() {
        let config = GcConfig::default();
        let gc = GarbageCollector::new(config);
        let mut tree = make_tree();
        let s1 = make_snapshot("s1", "main", current_timestamp());
        tree.nodes.insert("s1".into(), s1);
        tree.branches
            .insert("main".into(), make_branch("main", "s1"));
        tree.branches
            .insert("orphan".into(), make_branch("orphan", "missing"));
        let orphans = gc.find_orphan_branches(&tree);
        assert_eq!(orphans, vec!["orphan"]);
    }
}
