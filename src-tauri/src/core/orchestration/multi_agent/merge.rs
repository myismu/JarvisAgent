//! 分支合并模块
//!
//! 实现分支间的合并操作，包括冲突检测、自动/手动解决、合并快照生成。

use crate::core::rollback::{Patch, Snapshot, SnapshotTree};
use serde::{Deserialize, Serialize};
use std::collections::{HashMap, HashSet};

/// 合并结果
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct MergeResult {
    pub success: bool,
    pub target_branch: String,
    pub source_branch: String,
    pub merged_snapshot_id: Option<String>,
    pub conflicts: Vec<Conflict>,
    pub auto_resolved: usize,
    pub manual_required: usize,
}

/// 合并冲突详情
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Conflict {
    pub path: String,
    pub conflict_type: ConflictType,
    pub source_content: Option<String>,
    pub target_content: Option<String>,
    pub base_content: Option<String>,
    pub resolution: Option<ConflictResolution>,
}

/// 冲突类型
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "snake_case")]
pub enum ConflictType {
    BothModified,
    SourceDeleted,
    TargetDeleted,
    BothCreated,
    BothRenamed,
}

/// 冲突解决策略
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ConflictResolution {
    KeepSource,
    KeepTarget,
    KeepBoth { new_path: String },
    Manual { resolved_content: String },
    Custom { content: String },
}

/// 合并操作错误类型
#[derive(Debug, thiserror::Error)]
pub enum MergeError {
    #[error("Branch not found: {0}")]
    BranchNotFound(String),
    #[error("No common ancestor")]
    NoCommonAncestor,
    #[error("Merge conflict: {0} conflicts need resolution")]
    UnresolvedConflicts(usize),
    #[error("Cannot merge into same branch")]
    SameBranch,
}

/// 合并引擎
pub struct MergeEngine {
    conflict_threshold: usize,
}

impl MergeEngine {
    pub fn new() -> Self {
        Self {
            conflict_threshold: 10,
        }
    }

    /// 执行分支合并（带冲突解决），返回合并结果和合并后的补丁列表
    pub fn merge_branches(
        &self,
        tree: &SnapshotTree,
        source_branch: &str,
        target_branch: &str,
        resolutions: HashMap<String, ConflictResolution>,
    ) -> Result<(MergeResult, Vec<Patch>), MergeError> {
        if source_branch == target_branch {
            return Err(MergeError::SameBranch);
        }

        let source = tree
            .branches
            .get(source_branch)
            .ok_or_else(|| MergeError::BranchNotFound(source_branch.to_string()))?;
        let target = tree
            .branches
            .get(target_branch)
            .ok_or_else(|| MergeError::BranchNotFound(target_branch.to_string()))?;

        let lca_id =
            self.find_common_ancestor(tree, &source.head_snapshot_id, &target.head_snapshot_id)?;

        let source_patches = self.collect_patches_since(tree, &lca_id, &source.head_snapshot_id);
        let target_patches = self.collect_patches_since(tree, &lca_id, &target.head_snapshot_id);

        let conflicts = self.detect_conflicts(&source_patches, &target_patches);

        let (resolved_conflicts, unresolved) = self.apply_resolutions(conflicts, resolutions);

        if unresolved > self.conflict_threshold {
            return Err(MergeError::UnresolvedConflicts(unresolved));
        }

        let merged_patches =
            self.merge_patches(&source_patches, &target_patches, &resolved_conflicts);

        Ok((MergeResult {
            success: unresolved == 0,
            target_branch: target_branch.to_string(),
            source_branch: source_branch.to_string(),
            merged_snapshot_id: None,
            conflicts: resolved_conflicts.clone(),
            auto_resolved: resolved_conflicts
                .iter()
                .filter(|c| c.resolution.is_some())
                .count(),
            manual_required: unresolved,
        }, merged_patches))
    }

    /// 预览合并结果（不实际执行）
    pub fn preview_merge(
        &self,
        tree: &SnapshotTree,
        source_branch: &str,
        target_branch: &str,
    ) -> Result<MergeResult, MergeError> {
        if source_branch == target_branch {
            return Err(MergeError::SameBranch);
        }

        let source = tree
            .branches
            .get(source_branch)
            .ok_or_else(|| MergeError::BranchNotFound(source_branch.to_string()))?;
        let target = tree
            .branches
            .get(target_branch)
            .ok_or_else(|| MergeError::BranchNotFound(target_branch.to_string()))?;

        let lca_id =
            self.find_common_ancestor(tree, &source.head_snapshot_id, &target.head_snapshot_id)?;

        let source_patches = self.collect_patches_since(tree, &lca_id, &source.head_snapshot_id);
        let target_patches = self.collect_patches_since(tree, &lca_id, &target.head_snapshot_id);

        let conflicts = self.detect_conflicts(&source_patches, &target_patches);
        let auto_resolvable = conflicts
            .iter()
            .filter(|c| self.can_auto_resolve(c))
            .count();

        Ok(MergeResult {
            success: conflicts.is_empty(),
            target_branch: target_branch.to_string(),
            source_branch: source_branch.to_string(),
            merged_snapshot_id: None,
            conflicts,
            auto_resolved: auto_resolvable,
            manual_required: 0,
        })
    }

    /// 查找两个分支的最近公共祖先
    fn find_common_ancestor(
        &self,
        tree: &SnapshotTree,
        id1: &str,
        id2: &str,
    ) -> Result<String, MergeError> {
        let mut ancestors: HashSet<String> = HashSet::new();
        let mut current = Some(id1.to_string());

        while let Some(id) = current {
            ancestors.insert(id.clone());
            current = tree.nodes.get(&id).and_then(|s| s.parent_id.clone());
        }

        current = Some(id2.to_string());
        while let Some(id) = current {
            if ancestors.contains(&id) {
                return Ok(id);
            }
            current = tree.nodes.get(&id).and_then(|s| s.parent_id.clone());
        }

        Err(MergeError::NoCommonAncestor)
    }

    fn collect_patches_since(
        &self,
        tree: &SnapshotTree,
        since_id: &str,
        to_id: &str,
    ) -> Vec<Patch> {
        let mut patches = Vec::new();
        let mut current = Some(to_id.to_string());

        while let Some(id) = current {
            if id == since_id {
                break;
            }

            if let Some(snapshot) = tree.nodes.get(&id) {
                patches.splice(0..0, snapshot.patches.clone());
                current = snapshot.parent_id.clone();
            } else {
                break;
            }
        }

        patches
    }

    /// 检测两个补丁集之间的冲突
    fn detect_conflicts(
        &self,
        source_patches: &[Patch],
        target_patches: &[Patch],
    ) -> Vec<Conflict> {
        let mut conflicts = Vec::new();

        let source_paths: HashMap<String, &Patch> = source_patches
            .iter()
            .map(|p| (self.get_patch_path(p), p))
            .collect();

        let target_paths: HashMap<String, &Patch> = target_patches
            .iter()
            .map(|p| (self.get_patch_path(p), p))
            .collect();

        for (path, source_patch) in &source_paths {
            if let Some(target_patch) = target_paths.get(path) {
                if self.patches_conflict(source_patch, target_patch) {
                    conflicts.push(Conflict {
                        path: path.clone(),
                        conflict_type: ConflictType::BothModified,
                        source_content: self.get_patch_content(source_patch),
                        target_content: self.get_patch_content(target_patch),
                        base_content: None,
                        resolution: None,
                    });
                }
            }
        }

        for path in source_paths.keys() {
            if !target_paths.contains_key(path) {
                // Source only
            }
        }

        for path in target_paths.keys() {
            if !source_paths.contains_key(path) {
                // Target only
            }
        }

        conflicts
    }

    fn get_patch_path(&self, patch: &Patch) -> String {
        match patch {
            Patch::CreateFile { path, .. } => path.clone(),
            Patch::DeleteFile { path, .. } => path.clone(),
            Patch::UpdateFile { path, .. } => path.clone(),
            Patch::RenameFile { old_path, .. } => old_path.clone(),
        }
    }

    fn get_patch_content(&self, patch: &Patch) -> Option<String> {
        match patch {
            Patch::CreateFile { content, .. } => Some(content.clone()),
            Patch::UpdateFile { new_content, .. } => Some(new_content.clone()),
            _ => None,
        }
    }

    fn patches_conflict(&self, patch1: &Patch, patch2: &Patch) -> bool {
        match (patch1, patch2) {
            (
                Patch::UpdateFile {
                    path: p1,
                    new_content: c1,
                    ..
                },
                Patch::UpdateFile {
                    path: p2,
                    new_content: c2,
                    ..
                },
            ) => p1 == p2 && c1 != c2,
            (Patch::DeleteFile { path: p1, .. }, Patch::UpdateFile { path: p2, .. })
            | (Patch::UpdateFile { path: p1, .. }, Patch::DeleteFile { path: p2, .. }) => p1 == p2,
            (Patch::DeleteFile { path: p1, .. }, Patch::DeleteFile { path: p2, .. }) => p1 == p2,
            (Patch::CreateFile { path: p1, .. }, Patch::CreateFile { path: p2, .. }) => p1 == p2,
            _ => false,
        }
    }

    fn can_auto_resolve(&self, conflict: &Conflict) -> bool {
        matches!(conflict.conflict_type, ConflictType::BothCreated)
    }

    fn apply_resolutions(
        &self,
        conflicts: Vec<Conflict>,
        resolutions: HashMap<String, ConflictResolution>,
    ) -> (Vec<Conflict>, usize) {
        let resolved: Vec<Conflict> = conflicts
            .into_iter()
            .map(|mut c| {
                if let Some(resolution) = resolutions.get(&c.path) {
                    c.resolution = Some(resolution.clone());
                }
                c
            })
            .collect();

        let unresolved = resolved.iter().filter(|c| c.resolution.is_none()).count();

        (resolved, unresolved)
    }

    /// 根据冲突解决结果构建合并后的补丁列表（公开方法）
    pub fn build_merged_patches(
        &self,
        source_patches: &[Patch],
        target_patches: &[Patch],
        conflicts: &[Conflict],
    ) -> Vec<Patch> {
        self.merge_patches(source_patches, target_patches, conflicts)
    }

    fn merge_patches(
        &self,
        source_patches: &[Patch],
        target_patches: &[Patch],
        conflicts: &[Conflict],
    ) -> Vec<Patch> {
        let mut merged = Vec::new();

        let conflict_paths: HashSet<&str> = conflicts.iter().map(|c| c.path.as_str()).collect();

        for patch in target_patches {
            let path = self.get_patch_path(patch);
            if !conflict_paths.contains(path.as_str()) {
                merged.push(patch.clone());
            }
        }

        for patch in source_patches {
            let path = self.get_patch_path(patch);
            if !conflict_paths.contains(path.as_str()) {
                merged.push(patch.clone());
            }
        }

        for conflict in conflicts {
            if let Some(resolution) = &conflict.resolution {
                match resolution {
                    ConflictResolution::KeepSource => {
                        if let Some(content) = &conflict.source_content {
                            merged.push(Patch::UpdateFile {
                                path: conflict.path.clone(),
                                old_content: conflict.target_content.clone().unwrap_or_default(),
                                new_content: content.clone(),
                                diff: None,
                                content_hash: None,
                            });
                        }
                    }
                    ConflictResolution::KeepTarget => {}
                    ConflictResolution::KeepBoth { new_path } => {
                        if let Some(content) = &conflict.source_content {
                            merged.push(Patch::CreateFile {
                                path: new_path.clone(),
                                content: content.clone(),
                            });
                        }
                    }
                    ConflictResolution::Manual { resolved_content }
                    | ConflictResolution::Custom {
                        content: resolved_content,
                    } => {
                        merged.push(Patch::UpdateFile {
                            path: conflict.path.clone(),
                            old_content: conflict.target_content.clone().unwrap_or_default(),
                            new_content: resolved_content.clone(),
                            diff: None,
                            content_hash: None,
                        });
                    }
                }
            }
        }

        merged
    }

    /// 创建合并快照
    pub fn create_merge_snapshot(
        &self,
        tree: &mut SnapshotTree,
        merge_result: &MergeResult,
        merged_patches: Vec<Patch>,
        message: Option<String>,
    ) -> Result<Snapshot, MergeError> {
        let snapshot = tree.create_snapshot(
            merged_patches,
            message.or_else(|| {
                Some(format!(
                    "Merge {} into {}",
                    merge_result.source_branch, merge_result.target_branch
                ))
            }),
            None,
            None,
            false,
            None,
        );

        Ok(snapshot)
    }
}

impl Default for MergeEngine {
    fn default() -> Self {
        Self::new()
    }
}
