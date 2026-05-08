//! # patch.rs — 事务式多 hunk 文本补丁应用工具
//!
//! 实现 Agent 可调用的 ApplyPatch，支持多文件 unified patch、dry-run 预览、权限与 Notebook 保护、编码保留、TOCTOU 检查和快照记录。

use std::path::{Path, PathBuf};
use std::time::SystemTime;

use crate::core::rollback::Patch;
use crate::core::tools::framework::permission::ensure_path_permission;

use super::common::{
    encode_text_preserve_encoding, is_locked_file_error, normalize_line_endings,
    read_text_preserve_encoding, TextEncoding,
};
use super::diff::compute_diff;
use crate::core::tools::notebook_tools::notebook_guard::{
    is_notebook_path, looks_like_notebook_json, notebook_text_edit_rejection,
};
use super::workspace::{get_workspace, record_patch_to_snapshot};

#[derive(Debug, Clone, PartialEq, Eq)]
enum PatchLine {
    Context(String),
    Remove(String),
    Add(String),
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct PatchHunk {
    old_start: usize,
    old_len: usize,
    new_start: usize,
    new_len: usize,
    lines: Vec<PatchLine>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct FilePatch {
    path: String,
    hunks: Vec<PatchHunk>,
}

#[derive(Debug)]
struct LoadedFile {
    path: String,
    existed: bool,
    old_content: String,
    encoding: TextEncoding,
    read_mtime: Option<SystemTime>,
}

#[derive(Debug)]
struct PlannedFilePatch {
    loaded: LoadedFile,
    new_content: String,
    added_lines: usize,
    removed_lines: usize,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct ApplyPreview {
    path: String,
    added_lines: usize,
    removed_lines: usize,
    hunks: usize,
}

fn input_bool(input: &serde_json::Value, key: &str, default: bool) -> bool {
    let Some(value) = input.get(key) else {
        return default;
    };
    if let Some(value) = value.as_bool() {
        return value;
    }
    value
        .as_str()
        .map(|value| matches!(value.to_lowercase().as_str(), "true" | "1" | "yes"))
        .unwrap_or(default)
}

fn strip_patch_prefix(path: &str) -> &str {
    path.strip_prefix("a/")
        .or_else(|| path.strip_prefix("b/"))
        .unwrap_or(path)
}

fn parse_file_path(line: &str, prefix: &str) -> Option<String> {
    let rest = line.strip_prefix(prefix)?.trim();
    let path = rest.split_whitespace().next().unwrap_or(rest);
    (path != "/dev/null").then(|| strip_patch_prefix(path).to_string())
}

fn parse_range(raw: &str) -> Result<(usize, usize), String> {
    let raw = raw.trim_start_matches(['-', '+']);
    if let Some((start, len)) = raw.split_once(',') {
        let start = start.parse::<usize>().map_err(|_| format!("无效 hunk range: {}", raw))?;
        let len = len.parse::<usize>().map_err(|_| format!("无效 hunk range: {}", raw))?;
        Ok((start, len))
    } else {
        let start = raw.parse::<usize>().map_err(|_| format!("无效 hunk range: {}", raw))?;
        Ok((start, 1))
    }
}

fn parse_hunk_header(line: &str) -> Result<(usize, usize, usize, usize), String> {
    let mut parts = line.split_whitespace();
    if parts.next() != Some("@@") {
        return Err(format!("无效 hunk header: {}", line));
    }
    let old_range = parts.next().ok_or_else(|| format!("无效 hunk header: {}", line))?;
    let new_range = parts.next().ok_or_else(|| format!("无效 hunk header: {}", line))?;
    let end = parts.next().ok_or_else(|| format!("无效 hunk header: {}", line))?;
    if end != "@@" {
        return Err(format!("无效 hunk header: {}", line));
    }
    let (old_start, old_len) = parse_range(old_range)?;
    let (new_start, new_len) = parse_range(new_range)?;
    Ok((old_start, old_len, new_start, new_len))
}

fn parse_apply_patch(input: &str) -> Result<Vec<FilePatch>, String> {
    let normalized = normalize_line_endings(input);
    let lines: Vec<&str> = normalized.lines().collect();
    let mut patches = Vec::new();
    let mut current_path: Option<String> = None;
    let mut current_hunks: Vec<PatchHunk> = Vec::new();
    let mut i = 0;

    while i < lines.len() {
        let line = lines[i];
        if line == "*** Begin Patch" || line == "*** End Patch" || line.starts_with("diff --git ") {
            i += 1;
            continue;
        }

        if let Some(path) = line.strip_prefix("*** Update File:").map(str::trim).filter(|path| !path.is_empty()) {
            if let Some(previous_path) = current_path.replace(path.to_string()) {
                patches.push(FilePatch { path: previous_path, hunks: std::mem::take(&mut current_hunks) });
            }
            i += 1;
            continue;
        }

        if let Some(path) = parse_file_path(line, "--- ") {
            if current_path.is_none() {
                current_path = Some(path);
            }
            i += 1;
            continue;
        }

        if let Some(path) = parse_file_path(line, "+++ ") {
            if let Some(previous_path) = current_path.replace(path) {
                if !current_hunks.is_empty() {
                    patches.push(FilePatch { path: previous_path, hunks: std::mem::take(&mut current_hunks) });
                }
            }
            i += 1;
            continue;
        }

        if line.starts_with("@@ ") {
            let (old_start, old_len, new_start, new_len) = parse_hunk_header(line)?;
            i += 1;
            let mut hunk_lines = Vec::new();
            while i < lines.len()
                && !lines[i].starts_with("@@ ")
                && !lines[i].starts_with("*** Update File:")
                && !lines[i].starts_with("diff --git ")
                && !lines[i].starts_with("--- ")
                && !lines[i].starts_with("+++ ")
                && lines[i] != "*** End Patch"
            {
                let hunk_line = lines[i];
                if hunk_line == r"\ No newline at end of file" {
                    i += 1;
                    continue;
                }
                let Some(marker) = hunk_line.chars().next() else {
                    hunk_lines.push(PatchLine::Context(String::new()));
                    i += 1;
                    continue;
                };
                let text = hunk_line.get(1..).unwrap_or_default().to_string();
                match marker {
                    ' ' => hunk_lines.push(PatchLine::Context(text)),
                    '-' => hunk_lines.push(PatchLine::Remove(text)),
                    '+' => hunk_lines.push(PatchLine::Add(text)),
                    _ => return Err(format!("无效 patch 行: {}", hunk_line)),
                }
                i += 1;
            }
            current_hunks.push(PatchHunk { old_start, old_len, new_start, new_len, lines: hunk_lines });
            continue;
        }

        i += 1;
    }

    if let Some(path) = current_path {
        patches.push(FilePatch { path, hunks: current_hunks });
    }

    patches.retain(|patch| !patch.hunks.is_empty());
    if patches.is_empty() {
        return Err("未解析到可应用的 patch hunk。请提供 unified diff 或 *** Begin Patch 格式。".to_string());
    }
    Ok(patches)
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct FuzzyHunkMatch {
    start: usize,
    score: usize,
    expected_lines: usize,
}

impl FuzzyHunkMatch {
    fn confidence_percent(&self) -> usize {
        if self.expected_lines == 0 {
            100
        } else {
            self.score * 100 / self.expected_lines
        }
    }
}

fn hunk_expected_lines(hunk: &PatchHunk) -> Vec<&str> {
    hunk.lines.iter().filter_map(|line| match line {
        PatchLine::Context(text) | PatchLine::Remove(text) => Some(text.as_str()),
        PatchLine::Add(_) => None,
    }).collect()
}

fn line_similarity(a: &str, b: &str) -> usize {
    if a == b {
        3
    } else if a.trim() == b.trim() && !a.trim().is_empty() {
        2
    } else if normalize_fuzzy_line(a) == normalize_fuzzy_line(b) && !normalize_fuzzy_line(a).is_empty() {
        1
    } else {
        0
    }
}

fn normalize_fuzzy_line(line: &str) -> String {
    line.split_whitespace().collect::<Vec<_>>().join(" ")
}

fn exact_hunk_start(content_lines: &[String], expected: &[&str], preferred: usize) -> Option<usize> {
    if preferred + expected.len() <= content_lines.len()
        && expected.iter().enumerate().all(|(idx, line)| content_lines[preferred + idx] == *line)
    {
        return Some(preferred);
    }

    content_lines
        .windows(expected.len())
        .position(|window| expected.iter().enumerate().all(|(idx, line)| window[idx] == *line))
}

fn fuzzy_hunk_candidates(content_lines: &[String], expected: &[&str], preferred: usize) -> Vec<FuzzyHunkMatch> {
    if expected.is_empty() {
        return Vec::new();
    }

    let max_start = content_lines.len().saturating_sub(1);
    let mut candidates = Vec::new();
    for start in 0..=max_start {
        let mut score = 0usize;
        let mut matched = 0usize;
        for (idx, expected_line) in expected.iter().enumerate() {
            let Some(actual) = content_lines.get(start + idx) else {
                break;
            };
            let line_score = line_similarity(actual, expected_line);
            if line_score > 0 {
                matched += 1;
                score += line_score;
            }
        }
        if matched > 0 {
            candidates.push(FuzzyHunkMatch {
                start,
                score,
                expected_lines: expected.len() * 3,
            });
        }
    }

    candidates.sort_by(|a, b| {
        b.score
            .cmp(&a.score)
            .then_with(|| a.start.abs_diff(preferred).cmp(&b.start.abs_diff(preferred)))
    });
    candidates
}

fn fuzzy_failure_message(content_lines: &[String], hunk: &PatchHunk, candidates: &[FuzzyHunkMatch]) -> String {
    let near = hunk.old_start.saturating_sub(1).min(content_lines.len());
    if candidates.is_empty() {
        return format!(
            "hunk 无法匹配: -{},{} +{},{}\n附近上下文:\n{}",
            hunk.old_start,
            hunk.old_len,
            hunk.new_start,
            hunk.new_len,
            hunk_context(content_lines, near)
        );
    }

    let mut result = format!(
        "hunk fuzzy 匹配置信度不足或存在歧义: -{},{} +{},{}\n",
        hunk.old_start, hunk.old_len, hunk.new_start, hunk.new_len
    );
    for candidate in candidates.iter().take(3) {
        result.push_str(&format!(
            "候选位置: 第 {} 行，置信度 {}%，score {}/{}\n{}",
            candidate.start + 1,
            candidate.confidence_percent(),
            candidate.score,
            candidate.expected_lines,
            hunk_context(content_lines, candidate.start)
        ));
    }
    result
}

fn find_hunk_start(content_lines: &[String], hunk: &PatchHunk) -> Result<usize, String> {
    let expected = hunk_expected_lines(hunk);

    if expected.is_empty() {
        return Ok(hunk.old_start.saturating_sub(1).min(content_lines.len()));
    }

    let preferred = hunk.old_start.saturating_sub(1);
    if let Some(start) = exact_hunk_start(content_lines, &expected, preferred) {
        return Ok(start);
    }

    let candidates = fuzzy_hunk_candidates(content_lines, &expected, preferred);
    let Some(best) = candidates.first() else {
        return Err(fuzzy_failure_message(content_lines, hunk, &candidates));
    };

    let high_confidence = best.confidence_percent() >= 66;
    let unique = candidates
        .get(1)
        .map(|second| best.score.saturating_sub(second.score) >= 2)
        .unwrap_or(true);

    if high_confidence && unique {
        Ok(best.start)
    } else {
        Err(fuzzy_failure_message(content_lines, hunk, &candidates))
    }
}

fn hunk_context(content_lines: &[String], start: usize) -> String {
    let begin = start.saturating_sub(3);
    let end = (start + 4).min(content_lines.len());
    let mut result = String::new();
    for (idx, line) in content_lines.iter().enumerate().take(end).skip(begin) {
        result.push_str(&format!("{:4} | {}\n", idx + 1, line));
    }
    result
}

fn build_fuzzy_replacement(lines: &[String], start: usize, hunk: &PatchHunk) -> (Vec<String>, usize, usize, usize) {
    let mut replacement = Vec::new();
    let mut consumed = 0usize;
    let mut added = 0usize;
    let mut removed = 0usize;

    for line in &hunk.lines {
        match line {
            PatchLine::Context(_) => {
                if let Some(actual) = lines.get(start + consumed) {
                    replacement.push(actual.clone());
                }
                consumed += 1;
            }
            PatchLine::Remove(_) => {
                consumed += 1;
                removed += 1;
            }
            PatchLine::Add(text) => {
                replacement.push(text.clone());
                added += 1;
            }
        }
    }

    (replacement, consumed, added, removed)
}

fn apply_hunks(content: &str, hunks: &[PatchHunk]) -> Result<(String, usize, usize), String> {
    let mut lines: Vec<String> = content.lines().map(str::to_string).collect();
    let trailing_newline = content.ends_with('\n');
    let mut added_lines = 0;
    let mut removed_lines = 0;

    for hunk in hunks {
        let start = find_hunk_start(&lines, hunk)
            .map_err(|e| format!("{}\n文件 hunk 应用已中止", e))?;
        let exact_match = hunk_expected_lines(hunk)
            .iter()
            .enumerate()
            .all(|(idx, line)| lines.get(start + idx).map(String::as_str) == Some(*line));

        if exact_match {
            let mut replacement = Vec::new();
            let mut consumed = 0usize;
            for line in &hunk.lines {
                match line {
                    PatchLine::Context(text) => {
                        replacement.push(text.clone());
                        consumed += 1;
                    }
                    PatchLine::Remove(_) => {
                        consumed += 1;
                        removed_lines += 1;
                    }
                    PatchLine::Add(text) => {
                        replacement.push(text.clone());
                        added_lines += 1;
                    }
                }
            }
            lines.splice(start..start + consumed, replacement);
            continue;
        }

        let (replacement, consumed, added, removed) = build_fuzzy_replacement(&lines, start, hunk);
        added_lines += added;
        removed_lines += removed;
        lines.splice(start..start + consumed, replacement);
    }

    let mut result = lines.join("\n");
    if trailing_newline || !result.is_empty() {
        result.push('\n');
    }
    Ok((result, added_lines, removed_lines))
}

fn resolve_patch_path(path: &str) -> PathBuf {
    let path = Path::new(path);
    if path.is_absolute() {
        path.to_path_buf()
    } else {
        std::env::current_dir().unwrap_or_default().join(path)
    }
}

fn load_patch_file(path: &str) -> Result<LoadedFile, String> {
    if is_notebook_path(path) {
        return Err(notebook_text_edit_rejection(path));
    }

    let resolved = resolve_patch_path(path);
    let existed = resolved.exists();
    if existed {
        let read_mtime = std::fs::metadata(&resolved).ok().and_then(|meta| meta.modified().ok());
        match read_text_preserve_encoding(&resolved) {
            Ok(decoded) => {
                if looks_like_notebook_json(&decoded.content) {
                    return Err(notebook_text_edit_rejection(path));
                }
                Ok(LoadedFile {
                    path: path.to_string(),
                    existed,
                    old_content: decoded.content,
                    encoding: decoded.encoding,
                    read_mtime,
                })
            }
            Err(e) => {
                let err_msg = e.to_string();
                if is_locked_file_error(&err_msg) {
                    Err(format!("读取失败: 文件可能被锁定，请稍后重试。详细错误: {}", e))
                } else {
                    Err(format!("读取失败: {}", e))
                }
            }
        }
    } else {
        Ok(LoadedFile {
            path: path.to_string(),
            existed,
            old_content: String::new(),
            encoding: TextEncoding::Utf8,
            read_mtime: None,
        })
    }
}

fn plan_patch(file_patch: &FilePatch) -> Result<PlannedFilePatch, String> {
    let loaded = load_patch_file(&file_patch.path)?;
    let (new_content, added_lines, removed_lines) = apply_hunks(&loaded.old_content, &file_patch.hunks)
        .map_err(|e| format!("{}\n文件: {}", e, file_patch.path))?;
    if looks_like_notebook_json(&new_content) {
        return Err(notebook_text_edit_rejection(&file_patch.path));
    }
    Ok(PlannedFilePatch {
        loaded,
        new_content,
        added_lines,
        removed_lines,
    })
}

fn preview_for(file_patch: &FilePatch, planned: &PlannedFilePatch) -> ApplyPreview {
    ApplyPreview {
        path: file_patch.path.clone(),
        added_lines: planned.added_lines,
        removed_lines: planned.removed_lines,
        hunks: file_patch.hunks.len(),
    }
}

fn check_toctou(planned: &PlannedFilePatch) -> Result<(), String> {
    if !planned.loaded.existed {
        if resolve_patch_path(&planned.loaded.path).exists() {
            return Err(format!("应用中止: 文件 {} 在读取后被外部创建。", planned.loaded.path));
        }
        return Ok(());
    }

    let Some(read_mtime) = planned.loaded.read_mtime else {
        return Ok(());
    };
    let current_mtime = std::fs::metadata(resolve_patch_path(&planned.loaded.path))
        .ok()
        .and_then(|meta| meta.modified().ok());
    if current_mtime != Some(read_mtime) {
        return Err(format!(
            "应用中止: 文件 {} 在读取后被外部修改。请重新读取后再应用 patch。",
            planned.loaded.path
        ));
    }
    Ok(())
}

fn write_planned_file(planned: &PlannedFilePatch) -> Result<(), String> {
    let bytes = encode_text_preserve_encoding(&planned.new_content, planned.loaded.encoding)
        .map_err(|e| format!("编码失败: {}", e))?;
    std::fs::write(resolve_patch_path(&planned.loaded.path), bytes).map_err(|e| {
        let err_msg = e.to_string();
        if is_locked_file_error(&err_msg) {
            format!("写入失败: 文件被锁定，请稍后重试。详细错误: {}", e)
        } else {
            format!("写入失败: {}", e)
        }
    })
}

fn format_preview(previews: &[ApplyPreview], dry_run: bool) -> String {
    let mut result = if dry_run {
        "ApplyPatch dry-run 通过，未写入文件。\n".to_string()
    } else {
        "ApplyPatch 应用成功。\n".to_string()
    };
    for preview in previews {
        result.push_str(&format!(
            "{}: {} hunk(s), +{}, -{}\n",
            preview.path, preview.hunks, preview.added_lines, preview.removed_lines
        ));
    }
    result
}

pub async fn apply_patch(
    app: &tauri::AppHandle,
    input: &serde_json::Value,
    session_id: &str,
) -> String {
    let patch = input["patch"].as_str().unwrap_or("");
    if patch.trim().is_empty() {
        return "ApplyPatch 错误: patch 不能为空。".to_string();
    }
    let dry_run = input_bool(input, "dry_run", false);

    let file_patches = match parse_apply_patch(patch) {
        Ok(patches) => patches,
        Err(e) => return format!("ApplyPatch 解析失败: {}", e),
    };

    let ws = get_workspace(app, session_id).await;
    for file_patch in &file_patches {
        if let Err(e) = ensure_path_permission(app, &file_patch.path, "应用 patch", ws.as_deref()).await {
            return e;
        }
    }

    let mut planned = Vec::new();
    let mut previews = Vec::new();
    for file_patch in &file_patches {
        let plan = match plan_patch(file_patch) {
            Ok(plan) => plan,
            Err(e) => return format!("ApplyPatch 预检失败: {}", e),
        };
        previews.push(preview_for(file_patch, &plan));
        planned.push(plan);
    }

    if dry_run {
        return format_preview(&previews, true);
    }

    for plan in &planned {
        if let Err(e) = check_toctou(plan) {
            return e;
        }
    }

    for plan in &planned {
        if let Err(e) = write_planned_file(plan) {
            return format!("ApplyPatch 写入失败，已停止。可能已有部分文件写入，请检查工作区。{}", e);
        }
    }

    for plan in planned {
        let patch = if plan.loaded.existed {
            Patch::UpdateFile {
                path: plan.loaded.path.clone(),
                old_content: plan.loaded.old_content.clone(),
                new_content: plan.new_content.clone(),
                diff: Some(compute_diff(&plan.loaded.old_content, &plan.new_content)),
                content_hash: None,
            }
        } else {
            Patch::CreateFile {
                path: plan.loaded.path.clone(),
                content: plan.new_content.clone(),
            }
        };
        record_patch_to_snapshot(
            app,
            session_id,
            patch,
            Some(format!("应用 patch {}", plan.loaded.path)),
        )
        .await;
    }

    format_preview(&previews, false)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_unified_patch() {
        let patch = "--- a/src/foo.ts\n+++ b/src/foo.ts\n@@ -1,2 +1,2 @@\n const a = 1\n-old\n+new\n";
        let parsed = parse_apply_patch(patch).unwrap();
        assert_eq!(parsed.len(), 1);
        assert_eq!(parsed[0].path, "src/foo.ts");
        assert_eq!(parsed[0].hunks.len(), 1);
    }

    #[test]
    fn applies_hunk() {
        let patch = parse_apply_patch("--- a/foo.txt\n+++ b/foo.txt\n@@ -1,3 +1,3 @@\n a\n-b\n+B\n c\n").unwrap();
        let (content, added, removed) = apply_hunks("a\nb\nc\n", &patch[0].hunks).unwrap();
        assert_eq!(content, "a\nB\nc\n");
        assert_eq!(added, 1);
        assert_eq!(removed, 1);
    }

    #[test]
    fn applies_fuzzy_hunk_when_whitespace_changed() {
        let patch = parse_apply_patch("--- a/foo.txt\n+++ b/foo.txt\n@@ -1,3 +1,3 @@\n a\n-b\n+B\n c\n").unwrap();
        let (content, added, removed) = apply_hunks(" a\n b\n c\n", &patch[0].hunks).unwrap();
        assert_eq!(content, " a\nB\n c\n");
        assert_eq!(added, 1);
        assert_eq!(removed, 1);
    }

    #[test]
    fn rejects_ambiguous_fuzzy_hunk() {
        let patch = parse_apply_patch("--- a/foo.txt\n+++ b/foo.txt\n@@ -1,2 +1,2 @@\n same\n-old\n+new\n").unwrap();
        let result = apply_hunks(" same\n old\n same\n old\n", &patch[0].hunks);
        assert!(result.unwrap_err().contains("fuzzy 匹配置信度不足或存在歧义"));
    }
}
