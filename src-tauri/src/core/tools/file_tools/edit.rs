//! # edit.rs — 执行安全的文本搜索替换文件编辑
//!
//! 实现 agent 可调用的 `edit_file` 工具，在权限校验后进行唯一匹配替换，并记录快照；
//! 同时拒绝 Notebook 文本编辑、归一化行尾并执行 TOCTOU 防护。
//! 支持 `edits` 数组批量编辑（单次调用内多段替换），向后兼容单条 `old_text`/`new_text`。
//!
//! ## Key Exports
//! - `edit_file()`: 基于唯一 old_text 匹配编辑普通文本文件（支持批量）
//!
//! ## Dependencies
//! - Internal: `crate::core::rollback`, `crate::core::tools::framework::permission`, `super::notebook_guard`
//!
//! ## Constraints
//! - 不得用于 `.ipynb` 或 notebook-shaped JSON，Notebook 必须通过 cell 级工具修改

use crate::core::rollback::Patch;
use crate::core::tools::framework::permission::ensure_path_permission;

use super::common::{
    encode_text_preserve_encoding, is_locked_file_error, is_unc_path, normalize_line_endings,
    normalize_quotes, read_text_preserve_encoding, resolve_path, unc_path_rejection,
    MAX_FILE_SIZE_BYTES,
};
use super::diff::compute_diff;
use crate::core::tools::notebook_tools::notebook_guard::{
    is_notebook_path, looks_like_notebook_json, notebook_text_edit_rejection,
};
use super::workspace::{get_workspace, record_patch_to_snapshot};

struct SingleEdit {
    old_text: String,
    new_text: String,
    replace_all: bool,
}

/// 解析 old_text 参数（别名兼容）
fn resolve_old_text(input: &serde_json::Value) -> String {
    input["old_text"].as_str()
        .or_else(|| input["old_str"].as_str())
        .or_else(|| input["old_string"].as_str())
        .or_else(|| input["old_content"].as_str())
        .or_else(|| input["search"].as_str())
        .unwrap_or("")
        .to_string()
}

/// 解析 new_text 参数（别名兼容）
fn resolve_new_text(input: &serde_json::Value) -> String {
    normalize_line_endings(
        input["new_text"].as_str()
            .or_else(|| input["new_str"].as_str())
            .or_else(|| input["new_string"].as_str())
            .or_else(|| input["new_content"].as_str())
            .or_else(|| input["replace"].as_str())
            .unwrap_or(""),
    )
}

/// 解析编辑列表：优先 `edits` 数组，否则退到单条 `old_text`/`new_text`
fn resolve_edits(input: &serde_json::Value) -> Vec<SingleEdit> {
    if let Some(arr) = input["edits"].as_array() {
        arr.iter().map(|item| SingleEdit {
            old_text: resolve_old_text(item),
            new_text: resolve_new_text(item),
            replace_all: item["replace_all"].as_bool().unwrap_or(false),
        }).collect()
    } else {
        vec![SingleEdit {
            old_text: resolve_old_text(input),
            new_text: resolve_new_text(input),
            replace_all: input["replace_all"].as_bool().unwrap_or(false),
        }]
    }
}

/// 校验单条编辑，返回 (old_text引用, 错误消息)
fn validate_single_edit<'a>(
    edit: &'a SingleEdit,
    content: &str,
    idx: usize,
    path: &str,
    is_batch: bool,
) -> Result<usize, String> {
    let label = if is_batch { format!("edits[{}]", idx) } else { "old_text".to_string() };

    if edit.old_text.is_empty() {
        return Err(format!(
            "编辑失败: {} old_text 为空。EditFile 是搜索替换工具，old_text 是要被替换的原文片段（至少 3~5 行以确保唯一匹配），new_text 是替换后的新内容。如需替换整个文件请使用 WriteFile。",
            label
        ));
    }
    if edit.old_text.trim().len() < 10 {
        return Err(format!(
            "编辑失败: {} old_text 太短（< 10 个有效字符），请包含至少 3~5 行上下文使匹配唯一。",
            label
        ));
    }

    let match_count = content.matches(&edit.old_text).count();

    if match_count == 0 {
        let normalized_content = normalize_quotes(content);
        let normalized_old = normalize_quotes(&edit.old_text);
        if normalized_content.contains(&normalized_old) {
            return Err(format!(
                "编辑失败: {} 未在 {} 中找到精确匹配的旧文本块，但发现引号风格不一致。\n文件使用弯引号，请使用相同的引号风格重试。",
                label, path
            ));
        }
        return Err(format!(
            "编辑失败: {} 未在 {} 中找到指定的旧文本块。\n请使用 read_file 先查看文件的实际内容",
            label, path
        ));
    }

    if !edit.replace_all && match_count > 1 {
        let mut context_msg = format!(
            "编辑失败: {} 旧文本在 {} 中匹配了 {} 处，请提供更多上下文使其唯一，或设置 replace_all=true 替换所有匹配。\n\n",
            label, path, match_count
        );
        let lines: Vec<&str> = content.lines().collect();
        let old_lines_count = edit.old_text.lines().count();
        let mut search_from = 0;
        for m_idx in 0..match_count {
            if let Some(pos) = content[search_from..].find(&edit.old_text) {
                let absolute_pos = search_from + pos;
                let line_num = content[..absolute_pos].lines().count();
                let ctx_start = line_num.saturating_sub(2);
                let ctx_end = (line_num + old_lines_count + 2).min(lines.len());
                context_msg.push_str(&format!(
                    "--- 匹配 {} (第 {} 行附近) ---\n",
                    m_idx + 1, line_num + 1
                ));
                for (i, line) in lines[ctx_start..ctx_end].iter().enumerate() {
                    let ln = ctx_start + i + 1;
                    let marker = if ln >= line_num + 1 && ln <= line_num + old_lines_count {
                        ">>>" } else { "   " };
                    context_msg.push_str(&format!("{} {:4} | {}\n", marker, ln, line));
                }
                context_msg.push('\n');
                search_from = absolute_pos + edit.old_text.len();
            }
        }
        context_msg.push_str("请提供更多上下文使 old_text 唯一，或设置 replace_all=true 替换所有匹配。");
        return Err(context_msg);
    }

    Ok(match_count)
}

/// 对所有编辑应用替换（所有校验通过后才调用）
fn apply_edits(content: &str, edits: &[SingleEdit]) -> String {
    let mut result = content.to_string();
    for edit in edits {
        if edit.replace_all {
            result = result.replace(&edit.old_text, &edit.new_text);
        } else {
            result = result.replacen(&edit.old_text, &edit.new_text, 1);
        }
    }
    result
}

/// 基于搜索替换编辑文件（唯一性检查 + 引号归一化 + TOCTOU 防护 + 自动快照）
/// 支持 `edits` 数组批量编辑，向后兼容单条 `old_text`/`new_text`
pub async fn edit_file(
    app: &tauri::AppHandle,
    input: &serde_json::Value,
    session_id: &str,
) -> String {
    let path = resolve_path(input);
    let edits = resolve_edits(input);
    let is_batch = edits.len() > 1;

    if edits.is_empty() {
        return "编辑失败: 缺少 old_text 或 edits 参数".to_string();
    }

    if is_unc_path(path) {
        return unc_path_rejection("编辑", path);
    }
    let ws = get_workspace(app, session_id).await;
    if let Err(e) = ensure_path_permission(app, path, "编辑", ws.as_deref()).await {
        return e;
    }
    if is_notebook_path(path) {
        return notebook_text_edit_rejection(path);
    }

    let read_mtime = std::fs::metadata(path).ok().and_then(|m| m.modified().ok());

    match read_text_preserve_encoding(path) {
        Ok(decoded) => {
            let content = decoded.content;
            let encoding = decoded.encoding;

            let file_size = std::fs::metadata(path).map(|m| m.len()).unwrap_or(0);
            if file_size > MAX_FILE_SIZE_BYTES {
                return format!(
                    "编辑失败: 文件 {} 过大 ({} bytes)，超过限制 {} bytes。",
                    path, file_size, MAX_FILE_SIZE_BYTES
                );
            }

            if looks_like_notebook_json(&content) {
                return notebook_text_edit_rejection(path);
            }

            // 逐条校验
            let mut total_replacements = 0usize;
            for (i, edit) in edits.iter().enumerate() {
                let count = match validate_single_edit(edit, &content, i, path, is_batch) {
                    Ok(c) => c,
                    Err(e) => return e,
                };
                if edit.replace_all {
                    total_replacements += count;
                } else {
                    total_replacements += 1;
                }
            }

            // 所有校验通过 → 原子替换
            let updated_content = apply_edits(&content, &edits);

            // TOCTOU 防护
            if let (Some(orig_mtime), Ok(current_meta)) = (read_mtime, std::fs::metadata(path)) {
                if let Ok(current_mtime) = current_meta.modified() {
                    if current_mtime != orig_mtime {
                        return format!(
                            "编辑中止: 文件 {} 在读取后被外部修改。请重新读取后再编辑。",
                            path
                        );
                    }
                }
            }

            let bytes = match encode_text_preserve_encoding(&updated_content, encoding) {
                Ok(bytes) => bytes,
                Err(e) => return format!("编辑并保存失败: {}", e),
            };

            match std::fs::write(path, bytes) {
                Ok(_) => {
                    let patch = Patch::update_file_patch(
                        session_id,
                        path.to_string(),
                        content.clone(),
                        updated_content.clone(),
                        Some(compute_diff(&content, &updated_content)),
                    );
                    let msg = if is_batch {
                        Some(format!("批量编辑 {} 处 → {} ({} 条)", total_replacements, path, edits.len()))
                    } else if total_replacements > 1 {
                        Some(format!("全局替换 {} 处 → {}", total_replacements, path))
                    } else {
                        Some(format!("编辑 {}", path))
                    };
                    record_patch_to_snapshot(app, session_id, patch, msg).await;

                    if is_batch {
                        format!("成功: 在 {} 中批量替换了 {} 处（{} 条编辑）", path, total_replacements, edits.len())
                    } else if total_replacements > 1 {
                        format!("成功: 在 {} 中替换了 {} 处匹配", path, total_replacements)
                    } else {
                        format!("成功编辑 {}", path)
                    }
                }
                Err(e) => {
                    let err_msg = e.to_string();
                    if is_locked_file_error(&err_msg) {
                        format!(
                            "编辑并保存失败: 文件被其他智能体或程序锁定，请稍后重试。详细错误: {}",
                            e
                        )
                    } else {
                        format!("编辑并保存失败: {}", e)
                    }
                }
            }
        }
        Err(e) => {
            let err_msg = e.to_string();
            if is_locked_file_error(&err_msg) {
                format!(
                    "编辑失败: 文件可能被其他智能体或程序锁定，请稍后重试。详细错误: {}",
                    e
                )
            } else {
                format!("编辑失败，无法读取文件: {}", e)
            }
        }
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn replace_replaces_all_occurrences() {
        let content = "line1\nfoo\nline3\nfoo\nline5";
        let result = content.replace("foo", "bar");
        assert_eq!(result, "line1\nbar\nline3\nbar\nline5");
    }

    #[test]
    fn replacen_replaces_only_first() {
        let content = "line1\nfoo\nline3\nfoo\nline5";
        let result = content.replacen("foo", "bar", 1);
        assert_eq!(result, "line1\nbar\nline3\nfoo\nline5");
    }

    #[test]
    fn matches_counts_correctly() {
        let content = "foo bar foo baz foo";
        assert_eq!(content.matches("foo").count(), 3);
    }

    #[test]
    fn replacement_with_zero_matches_is_noop() {
        let content = "hello world";
        let result = content.replace("xyz", "abc");
        assert_eq!(result, "hello world");
        assert_eq!(content.matches("xyz").count(), 0);
    }

    #[test]
    fn batch_edit_replaces_all_in_order() {
        let content = "header\nbody\nfooter";
        let edits = vec![
            super::SingleEdit { old_text: "header".into(), new_text: "HEAD".into(), replace_all: false },
            super::SingleEdit { old_text: "footer".into(), new_text: "FOOT".into(), replace_all: false },
        ];
        let result = super::apply_edits(content, &edits);
        assert_eq!(result, "HEAD\nbody\nFOOT");
    }

    #[test]
    fn batch_edit_with_replace_all() {
        let content = "a a a";
        let edits = vec![
            super::SingleEdit { old_text: "a".into(), new_text: "b".into(), replace_all: true },
        ];
        let result = super::apply_edits(content, &edits);
        assert_eq!(result, "b b b");
    }
}
