//! 补丁模块
//!
//! 定义文件变更的差异表示，支持创建、删除、更新、重命名四种操作类型。

use serde::{Deserialize, Serialize};
use std::collections::hash_map::DefaultHasher;
use std::hash::{Hash, Hasher};

/// 文件补丁（描述一次文件变更操作）
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum Patch {
    CreateFile {
        path: String,
        content: String,
    },
    DeleteFile {
        path: String,
        /// 原文件内容 hash（snapshot_content 表），None 表示读取失败
        content_hash: Option<String>,
    },
    UpdateFile {
        path: String,
        old_content: String,
        new_content: String,
        diff: Option<TextDiff>,
        /// 大文件内容去重引用：(old_hash, new_hash)
        /// 内容超过阈值时写入 snapshot_content 表，此处仅保留 hash
        content_hash: Option<(String, String)>,
    },
    RenameFile {
        old_path: String,
        new_path: String,
    },
}

/// 文本差异（由多个 hunk 组成）
#[derive(Clone, Debug, Serialize, Deserialize, Default)]
pub struct TextDiff {
    pub hunks: Vec<DiffHunk>,
}

/// 差异块（一个连续的变更区域）
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct DiffHunk {
    pub old_start: u32,
    pub old_lines: u32,
    pub new_start: u32,
    pub new_lines: u32,
    pub lines: Vec<DiffLine>,
}

/// 差异行类型
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(tag = "type")]
pub enum DiffLine {
    Context { content: String },
    Addition { content: String },
    Deletion { content: String },
}

/// 补丁摘要（用于统计展示）
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PatchSummary {
    pub path: String,
    pub operation: String,
    pub lines_added: usize,
    pub lines_removed: usize,
}

/// 补丁操作错误类型
#[derive(Debug, thiserror::Error)]
pub enum PatchError {
    #[error("File not found: {0}")]
    FileNotFound(String),
    #[error("File already exists: {0}")]
    FileAlreadyExists(String),
    #[error("Content hash mismatch: expected {expected}, got {actual}")]
    HashMismatch { expected: String, actual: String },
    #[error("IO error: {0}")]
    IoError(#[from] std::io::Error),
}

impl Patch {
    /// 返回该补丁实际涉及的路径
    pub fn touched_paths(&self) -> Vec<&str> {
        match self {
            Patch::CreateFile { path, .. } => vec![path.as_str()],
            Patch::DeleteFile { path, .. } => vec![path.as_str()],
            Patch::UpdateFile { path, .. } => vec![path.as_str()],
            Patch::RenameFile { old_path, new_path } => vec![old_path.as_str(), new_path.as_str()],
        }
    }

    /// 生成补丁摘要
    pub fn to_summary(&self) -> PatchSummary {
        match self {
            Patch::CreateFile { path, content } => PatchSummary {
                path: path.clone(),
                operation: "create".to_string(),
                lines_added: content.lines().count(),
                lines_removed: 0,
            },
            Patch::DeleteFile { path, .. } => PatchSummary {
                path: path.clone(),
                operation: "delete".to_string(),
                lines_added: 0,
                lines_removed: 0,
            },
            Patch::UpdateFile {
                path,
                old_content,
                new_content,
                ..
            } => PatchSummary {
                path: path.clone(),
                operation: "update".to_string(),
                lines_added: new_content.lines().count(),
                lines_removed: old_content.lines().count(),
            },
            Patch::RenameFile { old_path, .. } => PatchSummary {
                path: old_path.clone(),
                operation: "rename".to_string(),
                lines_added: 0,
                lines_removed: 0,
            },
        }
    }

    /// 计算内容哈希（用于变更检测）
    pub fn content_hash(content: &str) -> String {
        let mut hasher = DefaultHasher::new();
        content.hash(&mut hasher);
        format!("{:016x}", hasher.finish())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_patch_summary_create() {
        let patch = Patch::CreateFile {
            path: "test.rs".to_string(),
            content: "line1\nline2\nline3".to_string(),
        };
        let summary = patch.to_summary();
        assert_eq!(summary.path, "test.rs");
        assert_eq!(summary.operation, "create");
        assert_eq!(summary.lines_added, 3);
        assert_eq!(summary.lines_removed, 0);
    }

    #[test]
    fn test_patch_summary_delete() {
        let patch = Patch::DeleteFile {
            path: "old.rs".to_string(),
            content_hash: None,
        };
        let summary = patch.to_summary();
        assert_eq!(summary.operation, "delete");
        assert_eq!(summary.lines_added, 0);
    }

    #[test]
    fn test_patch_summary_rename() {
        let patch = Patch::RenameFile {
            old_path: "a.rs".to_string(),
            new_path: "b.rs".to_string(),
        };
        let summary = patch.to_summary();
        assert_eq!(summary.path, "a.rs");
        assert_eq!(summary.operation, "rename");
    }

    #[test]
    fn test_content_hash_stable() {
        let h1 = Patch::content_hash("hello world");
        let h2 = Patch::content_hash("hello world");
        assert_eq!(h1, h2);
    }

    #[test]
    fn test_content_hash_differs() {
        let h1 = Patch::content_hash("hello");
        let h2 = Patch::content_hash("world");
        assert_ne!(h1, h2);
    }

    #[test]
    fn test_touched_paths_create() {
        let patch = Patch::CreateFile {
            path: "new.txt".to_string(),
            content: "data".to_string(),
        };
        assert_eq!(patch.touched_paths(), vec!["new.txt"]);
    }

    #[test]
    fn test_touched_paths_rename() {
        let patch = Patch::RenameFile {
            old_path: "old.txt".to_string(),
            new_path: "new.txt".to_string(),
        };
        assert_eq!(patch.touched_paths(), vec!["old.txt", "new.txt"]);
    }
}
