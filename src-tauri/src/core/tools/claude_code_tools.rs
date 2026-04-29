//! # claude_code_tools.rs — Claude Code 风格搜索工具
//!
//! 提供两个只读、并发安全的搜索工具：
//! - `glob`: 按 glob 模式查找文件路径，结果按修改时间倒序排列
//! - `grep`: 按正则表达式搜索文件内容，支持文件过滤、输出模式、上下文和分页

use serde_json::json;
use std::path::{Path, PathBuf};
use std::time::{Instant, UNIX_EPOCH};
use tauri::Manager;

use super::permission::{ensure_path_permission, is_path_safe};
use super::registry::ToolDef;

const GLOB_DEFAULT_LIMIT: usize = 100;
const GREP_DEFAULT_HEAD_LIMIT: usize = 250;
const GREP_MAX_COLUMNS: usize = 500;

const SKIP_DIRS: &[&str] = &[
    ".git",
    ".hg",
    ".svn",
    "node_modules",
    "target",
    "dist",
    "build",
    ".next",
    ".cache",
    "coverage",
];

const BINARY_EXTENSIONS: &[&str] = &[
    "png", "ico", "icns", "jpg", "jpeg", "gif", "webp", "mp3", "mp4", "wav", "woff", "woff2",
    "ttf", "eot", "pdf", "zip", "gz", "tar", "rar", "7z", "exe", "dll", "so", "dylib", "class",
    "jar", "wasm",
];

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum GrepOutputMode {
    Content,
    FilesWithMatches,
    Count,
}

async fn get_workspace(app: &tauri::AppHandle, session_id: &str) -> Option<PathBuf> {
    if let Some(manager) = app.try_state::<crate::core::state::SessionManager>() {
        let ctx = manager.get_or_create(session_id).await;
        return ctx.workspace.lock().await.clone();
    }
    None
}

fn default_base(workspace: Option<&Path>) -> PathBuf {
    workspace
        .map(Path::to_path_buf)
        .unwrap_or_else(|| std::env::current_dir().unwrap_or_default())
}

fn resolve_path(path: Option<&str>, workspace: Option<&Path>) -> Result<PathBuf, String> {
    let raw = path.unwrap_or(".").trim();
    let raw = if raw.is_empty() { "." } else { raw };
    if !is_path_safe(raw) {
        return Err("路径不安全：包含 '..' 遍历".to_string());
    }

    let path = Path::new(raw);
    if path.is_absolute() {
        Ok(path.to_path_buf())
    } else if raw == "." && workspace.is_some() {
        Ok(default_base(workspace))
    } else {
        Ok(default_base(workspace).join(path))
    }
}

async fn ensure_resolved_path_permission(
    app: &tauri::AppHandle,
    raw_path: Option<&str>,
    resolved: &Path,
    action: &str,
    workspace: Option<&Path>,
) -> Result<(), String> {
    let raw = raw_path.unwrap_or(".").trim();
    let raw = if raw.is_empty() { "." } else { raw };
    if !is_path_safe(raw) {
        return Err("路径不安全：包含 '..' 遍历".to_string());
    }
    ensure_path_permission(app, &resolved.to_string_lossy(), action, workspace).await
}

fn should_skip_dir(path: &Path) -> bool {
    path.file_name()
        .and_then(|name| name.to_str())
        .map(|name| SKIP_DIRS.contains(&name))
        .unwrap_or(false)
}

fn is_binary_like(path: &Path) -> bool {
    path.extension()
        .and_then(|ext| ext.to_str())
        .map(|ext| BINARY_EXTENSIONS.contains(&ext.to_lowercase().as_str()))
        .unwrap_or(false)
}

fn collect_files(root: &Path, include_binary: bool, files: &mut Vec<PathBuf>) {
    if root.is_file() {
        if include_binary || !is_binary_like(root) {
            files.push(root.to_path_buf());
        }
        return;
    }

    let entries = match std::fs::read_dir(root) {
        Ok(entries) => entries,
        Err(_) => return,
    };

    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            if should_skip_dir(&path) {
                continue;
            }
            collect_files(&path, include_binary, files);
        } else if path.is_file() && (include_binary || !is_binary_like(&path)) {
            files.push(path);
        }
    }
}

fn modified_millis(path: &Path) -> u128 {
    std::fs::metadata(path)
        .and_then(|meta| meta.modified())
        .ok()
        .and_then(|time| time.duration_since(UNIX_EPOCH).ok())
        .map(|duration| duration.as_millis())
        .unwrap_or(0)
}

fn sort_by_modified_desc(paths: &mut [PathBuf]) {
    paths.sort_by(|a, b| {
        let mtime_order = modified_millis(b).cmp(&modified_millis(a));
        if mtime_order == std::cmp::Ordering::Equal {
            a.to_string_lossy().cmp(&b.to_string_lossy())
        } else {
            mtime_order
        }
    });
}

fn display_path(path: &Path, workspace: Option<&Path>) -> String {
    let display = workspace
        .and_then(|ws| path.strip_prefix(ws).ok().map(Path::to_path_buf))
        .or_else(|| {
            std::env::current_dir()
                .ok()
                .and_then(|cwd| path.strip_prefix(cwd).ok().map(Path::to_path_buf))
        })
        .unwrap_or_else(|| path.to_path_buf());

    display.to_string_lossy().replace('\\', "/")
}

fn normalize_path_for_match(path: &Path) -> String {
    path.to_string_lossy().replace('\\', "/")
}

fn push_escaped_regex_char(out: &mut String, ch: char) {
    if ".+()^$|[]{}\\".contains(ch) {
        out.push('\\');
    }
    out.push(ch);
}

fn glob_to_regex(pattern: &str) -> Result<regex::Regex, regex::Error> {
    let pattern = pattern.replace('\\', "/");
    let chars: Vec<char> = pattern.chars().collect();
    let mut out = String::from("^");
    let mut i = 0;

    while i < chars.len() {
        match chars[i] {
            '*' => {
                if i + 1 < chars.len() && chars[i + 1] == '*' {
                    if i + 2 < chars.len() && chars[i + 2] == '/' {
                        out.push_str("(?:.*/)?");
                        i += 3;
                    } else {
                        out.push_str(".*");
                        i += 2;
                    }
                } else {
                    out.push_str("[^/]*");
                    i += 1;
                }
            }
            '?' => {
                out.push_str("[^/]");
                i += 1;
            }
            '{' => {
                if let Some(end_offset) = chars[i + 1..].iter().position(|ch| *ch == '}') {
                    let end = i + 1 + end_offset;
                    let inner: String = chars[i + 1..end].iter().collect();
                    let alternatives: Vec<String> = inner
                        .split(',')
                        .filter(|part| !part.is_empty())
                        .map(regex::escape)
                        .collect();
                    if alternatives.is_empty() {
                        out.push_str("\\{\\}");
                    } else {
                        out.push_str("(?:");
                        out.push_str(&alternatives.join("|"));
                        out.push(')');
                    }
                    i = end + 1;
                } else {
                    out.push_str("\\{");
                    i += 1;
                }
            }
            '/' => {
                out.push('/');
                i += 1;
            }
            ch => {
                push_escaped_regex_char(&mut out, ch);
                i += 1;
            }
        }
    }

    out.push('$');
    regex::Regex::new(&out)
}

fn glob_matches(pattern: &str, relative_path: &Path) -> bool {
    let relative = normalize_path_for_match(relative_path);
    let file_name = relative_path
        .file_name()
        .map(|name| name.to_string_lossy().replace('\\', "/"))
        .unwrap_or_default();

    let re = match glob_to_regex(pattern) {
        Ok(re) => re,
        Err(_) => return false,
    };

    re.is_match(&relative) || (!pattern.contains('/') && re.is_match(&file_name))
}

fn split_glob_patterns(glob: &str) -> Vec<String> {
    let mut patterns = Vec::new();
    for raw in glob.split_whitespace() {
        if raw.contains('{') && raw.contains('}') {
            patterns.push(raw.to_string());
        } else {
            patterns.extend(
                raw.split(',')
                    .filter(|part| !part.is_empty())
                    .map(str::to_string),
            );
        }
    }
    patterns
}

fn matches_any_glob(path: &Path, base: &Path, patterns: &[String]) -> bool {
    if patterns.is_empty() {
        return true;
    }
    let relative = path.strip_prefix(base).unwrap_or(path);
    patterns
        .iter()
        .any(|pattern| glob_matches(pattern, relative))
}

fn type_extensions(file_type: &str) -> Vec<String> {
    match file_type.to_lowercase().as_str() {
        "js" | "javascript" => ["js", "jsx", "mjs", "cjs"]
            .into_iter()
            .map(str::to_string)
            .collect(),
        "ts" | "typescript" => ["ts", "tsx"].into_iter().map(str::to_string).collect(),
        "py" | "python" => vec!["py".to_string()],
        "rs" | "rust" => vec!["rs".to_string()],
        "go" => vec!["go".to_string()],
        "java" => vec!["java".to_string()],
        "c" => vec!["c".to_string(), "h".to_string()],
        "cpp" | "c++" => ["cpp", "cc", "cxx", "hpp", "hh", "hxx"]
            .into_iter()
            .map(str::to_string)
            .collect(),
        "cs" | "csharp" => vec!["cs".to_string()],
        "json" => vec!["json".to_string()],
        "md" | "markdown" => vec!["md".to_string(), "mdx".to_string()],
        "html" => vec!["html".to_string(), "htm".to_string()],
        "css" => vec!["css".to_string()],
        "vue" => vec!["vue".to_string()],
        "svelte" => vec!["svelte".to_string()],
        "php" => vec!["php".to_string()],
        "rb" | "ruby" => vec!["rb".to_string()],
        "swift" => vec!["swift".to_string()],
        "kt" | "kotlin" => vec!["kt".to_string(), "kts".to_string()],
        other => vec![other.to_string()],
    }
}

fn matches_type(path: &Path, file_type: Option<&str>) -> bool {
    let Some(file_type) = file_type else {
        return true;
    };
    let extensions = type_extensions(file_type);
    path.extension()
        .and_then(|ext| ext.to_str())
        .map(|ext| {
            extensions
                .iter()
                .any(|candidate| candidate == &ext.to_lowercase())
        })
        .unwrap_or(false)
}

fn truncate_columns(text: &str) -> String {
    let mut result: String = text.chars().take(GREP_MAX_COLUMNS).collect();
    if text.chars().count() > GREP_MAX_COLUMNS {
        result.push_str("...");
    }
    result
}

fn value_as_usize(input: &serde_json::Value, key: &str) -> Option<usize> {
    let value = input.get(key)?;
    if let Some(value) = value.as_u64() {
        return Some(value as usize);
    }
    value
        .as_str()
        .and_then(|value| value.trim().parse::<usize>().ok())
}

fn value_as_bool(input: &serde_json::Value, key: &str, default: bool) -> bool {
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

fn parse_output_mode(input: &serde_json::Value) -> Result<GrepOutputMode, String> {
    match input["output_mode"]
        .as_str()
        .unwrap_or("files_with_matches")
    {
        "content" => Ok(GrepOutputMode::Content),
        "files_with_matches" => Ok(GrepOutputMode::FilesWithMatches),
        "count" => Ok(GrepOutputMode::Count),
        other => Err(format!(
            "无效 output_mode: {}。可选值为 content、files_with_matches、count。",
            other
        )),
    }
}

fn apply_head_limit<T: Clone>(
    items: &[T],
    head_limit: Option<usize>,
    offset: usize,
    default_limit: usize,
) -> (Vec<T>, Option<usize>) {
    let start = offset.min(items.len());
    if head_limit == Some(0) {
        return (items[start..].to_vec(), None);
    }

    let effective_limit = head_limit.unwrap_or(default_limit);
    let end = (start + effective_limit).min(items.len());
    let limited = items[start..end].to_vec();
    let applied_limit =
        (items.len().saturating_sub(start) > effective_limit).then_some(effective_limit);
    (limited, applied_limit)
}

fn format_limit_info(applied_limit: Option<usize>, offset: usize) -> String {
    let mut parts = Vec::new();
    if let Some(limit) = applied_limit {
        parts.push(format!("limit: {}", limit));
    }
    if offset > 0 {
        parts.push(format!("offset: {}", offset));
    }
    parts.join(", ")
}

fn format_content_line(
    path: &Path,
    workspace: Option<&Path>,
    line_number: Option<usize>,
    text: &str,
    show_line_numbers: bool,
) -> String {
    let display = display_path(path, workspace);
    let text = truncate_columns(text.trim_end());
    if show_line_numbers {
        if let Some(line_number) = line_number {
            return format!("{}:{}:{}", display, line_number, text);
        }
    }
    format!("{}:{}", display, text)
}

fn file_has_match(content: &str, re: &regex::Regex, multiline: bool) -> bool {
    if multiline {
        re.is_match(content)
    } else {
        content.lines().any(|line| re.is_match(line))
    }
}

fn count_file_matches(content: &str, re: &regex::Regex, multiline: bool) -> usize {
    if multiline {
        re.find_iter(content).count()
    } else {
        content.lines().filter(|line| re.is_match(line)).count()
    }
}

fn line_number_for_byte(content: &str, byte_index: usize) -> usize {
    content[..byte_index.min(content.len())]
        .bytes()
        .filter(|byte| *byte == b'\n')
        .count()
        + 1
}

fn collect_content_matches(
    path: &Path,
    content: &str,
    re: &regex::Regex,
    multiline: bool,
    before: usize,
    after: usize,
    show_line_numbers: bool,
    workspace: Option<&Path>,
) -> Vec<String> {
    if multiline {
        return re
            .find_iter(content)
            .map(|matched| {
                let line_number = line_number_for_byte(content, matched.start());
                let text = matched.as_str().replace('\n', "\\n");
                format_content_line(path, workspace, Some(line_number), &text, show_line_numbers)
            })
            .collect();
    }

    let lines: Vec<&str> = content.lines().collect();
    let mut include = vec![false; lines.len()];
    for (idx, line) in lines.iter().enumerate() {
        if re.is_match(line) {
            let start = idx.saturating_sub(before);
            let end = (idx + after + 1).min(lines.len());
            for slot in include.iter_mut().take(end).skip(start) {
                *slot = true;
            }
        }
    }

    lines
        .iter()
        .enumerate()
        .filter(|(idx, _)| include[*idx])
        .map(|(idx, line)| {
            format_content_line(path, workspace, Some(idx + 1), line, show_line_numbers)
        })
        .collect()
}

/// Claude Code 风格 Glob：按文件名 glob 模式快速查找文件。
pub async fn glob(app: &tauri::AppHandle, input: &serde_json::Value, session_id: &str) -> String {
    let pattern = input["pattern"].as_str().unwrap_or("").trim();
    if pattern.is_empty() {
        return "Glob 错误: pattern 不能为空。".to_string();
    }

    let raw_path = input["path"].as_str();
    let workspace = get_workspace(app, session_id).await;
    let base = match resolve_path(raw_path, workspace.as_deref()) {
        Ok(path) => path,
        Err(e) => return e,
    };

    if let Err(e) =
        ensure_resolved_path_permission(app, raw_path, &base, "Glob 查找", workspace.as_deref())
            .await
    {
        return e;
    }

    if !base.exists() {
        return format!("Directory does not exist: {}", base.display());
    }
    if !base.is_dir() {
        return format!("Path is not a directory: {}", base.display());
    }

    let start = Instant::now();
    let mut all_files = Vec::new();
    collect_files(&base, true, &mut all_files);

    let mut matches: Vec<PathBuf> = all_files
        .into_iter()
        .filter(|path| {
            let relative = path.strip_prefix(&base).unwrap_or(path);
            glob_matches(pattern, relative)
        })
        .collect();

    sort_by_modified_desc(&mut matches);
    let truncated = matches.len() > GLOB_DEFAULT_LIMIT;
    matches.truncate(GLOB_DEFAULT_LIMIT);

    if matches.is_empty() {
        return "No files found".to_string();
    }

    let mut lines: Vec<String> = matches
        .iter()
        .map(|path| display_path(path, workspace.as_deref()))
        .collect();
    if truncated {
        lines.push(
            "(Results are truncated. Consider using a more specific path or pattern.)".to_string(),
        );
    }
    lines.push(format!(
        "\nFound {} files in {}ms",
        matches.len(),
        start.elapsed().as_millis()
    ));
    lines.join("\n")
}

/// Claude Code 风格 Grep：使用正则表达式搜索文件内容。
pub async fn grep(app: &tauri::AppHandle, input: &serde_json::Value, session_id: &str) -> String {
    let pattern = input["pattern"].as_str().unwrap_or("").trim();
    if pattern.is_empty() {
        return "Grep 错误: pattern 不能为空。".to_string();
    }

    let output_mode = match parse_output_mode(input) {
        Ok(mode) => mode,
        Err(e) => return e,
    };

    let raw_path = input["path"].as_str();
    let workspace = get_workspace(app, session_id).await;
    let base = match resolve_path(raw_path, workspace.as_deref()) {
        Ok(path) => path,
        Err(e) => return e,
    };

    if let Err(e) =
        ensure_resolved_path_permission(app, raw_path, &base, "Grep 搜索", workspace.as_deref())
            .await
    {
        return e;
    }

    if !base.exists() {
        return format!("Path does not exist: {}", base.display());
    }

    let mut builder = regex::RegexBuilder::new(pattern);
    builder.case_insensitive(value_as_bool(input, "-i", false));
    let multiline = value_as_bool(input, "multiline", false);
    if multiline {
        builder.multi_line(true).dot_matches_new_line(true);
    }
    let re = match builder.build() {
        Ok(re) => re,
        Err(e) => return format!("正则表达式无效: {}", e),
    };

    let glob_patterns = input["glob"]
        .as_str()
        .map(split_glob_patterns)
        .unwrap_or_default();
    let file_type = input["type"].as_str();
    let head_limit = value_as_usize(input, "head_limit");
    let offset = value_as_usize(input, "offset").unwrap_or(0);
    let show_line_numbers = value_as_bool(input, "-n", true);

    let context = value_as_usize(input, "context").or_else(|| value_as_usize(input, "-C"));
    let before = context.or_else(|| value_as_usize(input, "-B")).unwrap_or(0);
    let after = context.or_else(|| value_as_usize(input, "-A")).unwrap_or(0);

    let mut files = Vec::new();
    collect_files(&base, false, &mut files);
    let glob_base = if base.is_file() {
        base.parent().unwrap_or(&base)
    } else {
        &base
    };
    files.retain(|path| {
        matches_type(path, file_type) && matches_any_glob(path, glob_base, &glob_patterns)
    });

    match output_mode {
        GrepOutputMode::Content => {
            let mut lines = Vec::new();
            for path in &files {
                if let Ok(content) = std::fs::read_to_string(path) {
                    lines.extend(collect_content_matches(
                        path,
                        &content,
                        &re,
                        multiline,
                        before,
                        after,
                        show_line_numbers,
                        workspace.as_deref(),
                    ));
                }
            }

            let (limited, applied_limit) =
                apply_head_limit(&lines, head_limit, offset, GREP_DEFAULT_HEAD_LIMIT);
            let limit_info = format_limit_info(applied_limit, offset);
            let mut result = if limited.is_empty() {
                "No matches found".to_string()
            } else {
                limited.join("\n")
            };
            if !limit_info.is_empty() {
                result.push_str(&format!(
                    "\n\n[Showing results with pagination = {}]",
                    limit_info
                ));
            }
            result
        }
        GrepOutputMode::Count => {
            let mut count_lines = Vec::new();
            for path in &files {
                if let Ok(content) = std::fs::read_to_string(path) {
                    let count = count_file_matches(&content, &re, multiline);
                    if count > 0 {
                        count_lines.push(format!(
                            "{}:{}",
                            display_path(path, workspace.as_deref()),
                            count
                        ));
                    }
                }
            }
            let (limited, applied_limit) =
                apply_head_limit(&count_lines, head_limit, offset, GREP_DEFAULT_HEAD_LIMIT);
            let total_matches = limited
                .iter()
                .filter_map(|line| line.rsplit_once(':'))
                .filter_map(|(_, count)| count.parse::<usize>().ok())
                .sum::<usize>();
            let limit_info = format_limit_info(applied_limit, offset);
            let mut result = if limited.is_empty() {
                "No matches found".to_string()
            } else {
                limited.join("\n")
            };
            result.push_str(&format!(
                "\n\nFound {} total {} across {} {}.{}",
                total_matches,
                if total_matches == 1 {
                    "occurrence"
                } else {
                    "occurrences"
                },
                limited.len(),
                if limited.len() == 1 { "file" } else { "files" },
                if limit_info.is_empty() {
                    String::new()
                } else {
                    format!(" with pagination = {}", limit_info)
                }
            ));
            result
        }
        GrepOutputMode::FilesWithMatches => {
            let mut matches = Vec::new();
            for path in files {
                if let Ok(content) = std::fs::read_to_string(&path) {
                    if file_has_match(&content, &re, multiline) {
                        matches.push(path);
                    }
                }
            }
            sort_by_modified_desc(&mut matches);
            let (limited, applied_limit) =
                apply_head_limit(&matches, head_limit, offset, GREP_DEFAULT_HEAD_LIMIT);
            let limit_info = format_limit_info(applied_limit, offset);
            if limited.is_empty() {
                return "No files found".to_string();
            }
            let filenames: Vec<String> = limited
                .iter()
                .map(|path| display_path(path, workspace.as_deref()))
                .collect();
            format!(
                "Found {} {}{}\n{}",
                filenames.len(),
                if filenames.len() == 1 {
                    "file"
                } else {
                    "files"
                },
                if limit_info.is_empty() {
                    String::new()
                } else {
                    format!(" {}", limit_info)
                },
                filenames.join("\n")
            )
        }
    }
}

// --- 工具注册 ---
crate::define_tools! {
    pub fn register_tools(registry) {
        ToolDef {
            name: "glob",
            description: "按 glob 模式快速查找文件路径",
            search_hint: "glob wildcard filename file pattern find match",
            schema: json!({
                "name": "glob",
                "description": "Fast file pattern matching tool. Supports glob patterns like \"**/*.js\" or \"src/**/*.ts\". Returns matching file paths sorted by modification time. Use this when you need to find files by name patterns.",
                "input_schema": {
                    "type": "object",
                    "properties": {
                        "pattern": {
                            "type": "string",
                            "description": "The glob pattern to match files against"
                        },
                        "path": {
                            "type": "string",
                            "description": "The directory to search in. If omitted, the current workspace directory is used. Must be a valid directory path if provided."
                        }
                    },
                    "required": ["pattern"]
                }
            }),
            should_defer: true,
            is_read_only: true,
            is_concurrency_safe: true,
            is_enabled: true,
        },
        ToolDef {
            name: "grep",
            description: "使用正则表达式搜索文件内容",
            search_hint: "grep ripgrep regex search file contents text pattern",
            schema: json!({
                "name": "grep",
                "description": "A powerful search tool for file contents, modeled after Claude Code Grep. Supports regex syntax, glob/type filters, output modes, context lines, case-insensitive search, multiline search, and pagination.",
                "input_schema": {
                    "type": "object",
                    "properties": {
                        "pattern": {
                            "type": "string",
                            "description": "The regular expression pattern to search for in file contents"
                        },
                        "path": {
                            "type": "string",
                            "description": "File or directory to search in. Defaults to the current workspace directory."
                        },
                        "glob": {
                            "type": "string",
                            "description": "Glob pattern to filter files (e.g. \"*.js\", \"*.{ts,tsx}\"). Multiple patterns can be separated by spaces or commas."
                        },
                        "output_mode": {
                            "type": "string",
                            "enum": ["content", "files_with_matches", "count"],
                            "description": "Output mode: \"content\" shows matching lines, \"files_with_matches\" shows only file paths (default), \"count\" shows match counts."
                        },
                        "-B": {
                            "type": "integer",
                            "description": "Number of lines to show before each match. Requires output_mode: \"content\"."
                        },
                        "-A": {
                            "type": "integer",
                            "description": "Number of lines to show after each match. Requires output_mode: \"content\"."
                        },
                        "-C": {
                            "type": "integer",
                            "description": "Alias for context."
                        },
                        "context": {
                            "type": "integer",
                            "description": "Number of lines to show before and after each match. Requires output_mode: \"content\"."
                        },
                        "-n": {
                            "type": "boolean",
                            "description": "Show line numbers in content output. Defaults to true."
                        },
                        "-i": {
                            "type": "boolean",
                            "description": "Case insensitive search."
                        },
                        "type": {
                            "type": "string",
                            "description": "File type to search. Common types: js, py, rust, go, java, ts, json, md."
                        },
                        "head_limit": {
                            "type": "integer",
                            "description": "Limit output to first N lines/entries. Defaults to 250. Pass 0 for unlimited."
                        },
                        "offset": {
                            "type": "integer",
                            "description": "Skip first N lines/entries before applying head_limit. Defaults to 0."
                        },
                        "multiline": {
                            "type": "boolean",
                            "description": "Enable multiline mode where . matches newlines and patterns can span lines. Defaults to false."
                        }
                    },
                    "required": ["pattern"]
                }
            }),
            should_defer: true,
            is_read_only: true,
            is_concurrency_safe: true,
            is_enabled: true,
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_glob_matches_recursive_rs() {
        assert!(glob_matches("**/*.rs", Path::new("src/main.rs")));
        assert!(glob_matches("**/*.rs", Path::new("main.rs")));
        assert!(!glob_matches("**/*.rs", Path::new("src/main.ts")));
    }

    #[test]
    fn test_glob_matches_brace_alternatives() {
        assert!(glob_matches("*.{ts,tsx}", Path::new("app.ts")));
        assert!(glob_matches("*.{ts,tsx}", Path::new("app.tsx")));
        assert!(!glob_matches("*.{ts,tsx}", Path::new("app.js")));
    }

    #[test]
    fn test_split_glob_patterns_preserves_braces() {
        assert_eq!(
            split_glob_patterns("*.{ts,tsx} *.rs,*.go"),
            vec!["*.{ts,tsx}", "*.rs", "*.go"]
        );
    }

    #[test]
    fn test_apply_head_limit_default_and_offset() {
        let items = vec![1, 2, 3, 4, 5];
        let (limited, applied) = apply_head_limit(&items, None, 1, 2);
        assert_eq!(limited, vec![2, 3]);
        assert_eq!(applied, Some(2));
    }

    #[test]
    fn test_type_extensions_common_aliases() {
        assert!(type_extensions("rust").contains(&"rs".to_string()));
        assert!(type_extensions("typescript").contains(&"tsx".to_string()));
    }
}
