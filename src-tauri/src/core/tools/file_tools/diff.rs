//! # diff.rs — 为快照系统生成文本差异结构
//!
//! 将旧文本与新文本转换为 rollback 使用的 `TextDiff`，保留上下文、插入和删除行，供文件编辑/写入后的快照记录复用。
//!
//! ## Key Exports
//! - `compute_diff()`: 计算两个文本之间的结构化 diff
//!
//! ## Dependencies
//! - Internal: `crate::core::rollback::patch`
//! - External: `similar`

/// 计算两个文本之间的 diff（用于快照系统）
pub(super) fn compute_diff(
    old_text: &str,
    new_text: &str,
) -> crate::core::rollback::patch::TextDiff {
    use crate::core::rollback::patch::{DiffHunk, DiffLine, TextDiff};
    use similar::{ChangeTag, TextDiff as SimilarDiff};

    let diff = SimilarDiff::from_lines(old_text, new_text);
    let mut hunks = Vec::new();

    for op in diff.ops() {
        let old_start = op.old_range().start as u32;
        let new_start = op.new_range().start as u32;
        let old_len = op.old_range().len() as u32;
        let new_len = op.new_range().len() as u32;

        let mut lines = Vec::new();
        for change in diff.iter_changes(op) {
            let content = change.to_string();
            match change.tag() {
                ChangeTag::Equal => lines.push(DiffLine::Context { content }),
                ChangeTag::Insert => lines.push(DiffLine::Addition { content }),
                ChangeTag::Delete => lines.push(DiffLine::Deletion { content }),
            }
        }

        if !lines.is_empty() {
            hunks.push(DiffHunk {
                old_start,
                old_lines: old_len,
                new_start,
                new_lines: new_len,
                lines,
            });
        }
    }

    TextDiff { hunks }
}
