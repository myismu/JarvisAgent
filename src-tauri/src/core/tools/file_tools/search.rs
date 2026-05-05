//! # search.rs — 提供带权限校验的仓库文本搜索工具
//!
//! 实现 agent 可调用的 `search_repo`，支持普通字符串/正则、大小写不敏感搜索，并递归遍历目录时跳过构建产物和静态资源。
//!
//! ## Key Exports
//! - `search_repo()`: 在指定目录下搜索文本或正则匹配
//! - `search_in_dir()`: 递归搜索目录并返回带文件路径和行号的结果
//!
//! ## Dependencies
//! - Internal: `crate::core::tools::framework::permission`, `super::workspace`, `super::common`
//! - External: `regex`

use std::path::{Path, PathBuf};

use crate::core::tools::framework::permission::ensure_path_permission;

use super::common::{
    is_ignored_entry_name, is_search_skipped_extension, read_text_preserve_encoding,
};
use super::workspace::get_workspace;

const SEARCH_DEFAULT_LIMIT: usize = 50;
const SEARCH_MAX_LIMIT: usize = 500;
const SEARCH_MAX_CONTEXT_LINES: usize = 10;

fn input_usize(input: &serde_json::Value, key: &str) -> Option<usize> {
    let value = input.get(key)?;
    if let Some(value) = value.as_u64() {
        return Some(value as usize);
    }
    value.as_str().and_then(|value| value.trim().parse().ok())
}

fn input_string_list(input: &serde_json::Value, key: &str) -> Vec<String> {
    if let Some(raw) = input[key].as_str() {
        return raw
            .split(|ch| ch == ',' || ch == ' ')
            .map(str::trim)
            .filter(|part| !part.is_empty())
            .map(str::to_string)
            .collect();
    }

    input[key]
        .as_array()
        .map(|items| {
            items
                .iter()
                .filter_map(|item| item.as_str())
                .map(str::trim)
                .filter(|part| !part.is_empty())
                .map(str::to_string)
                .collect()
        })
        .unwrap_or_default()
}

fn split_glob_patterns(glob: &str) -> Vec<String> {
    glob.split(|ch| ch == ',' || ch == ' ')
        .map(str::trim)
        .filter(|part| !part.is_empty())
        .map(str::to_string)
        .collect()
}

fn input_patterns(input: &serde_json::Value, key: &str) -> Vec<String> {
    input[key]
        .as_str()
        .map(split_glob_patterns)
        .unwrap_or_default()
}

fn display_path(path: &Path) -> String {
    let display = std::env::current_dir()
        .ok()
        .and_then(|cwd| path.strip_prefix(cwd).ok().map(PathBuf::from))
        .unwrap_or_else(|| path.to_path_buf());

    display.to_string_lossy().replace('\\', "/")
}

fn path_contains_component(path: &Path, component: &str) -> bool {
    path.components().any(|part| part.as_os_str() == component)
}

fn is_code_file(path: &Path) -> bool {
    path.extension()
        .and_then(|ext| ext.to_str())
        .map(|ext| {
            matches!(
                ext.to_lowercase().as_str(),
                "rs" | "ts" | "tsx" | "js" | "jsx" | "vue" | "py" | "go" | "java" | "c" | "h"
                    | "cpp" | "hpp" | "cs" | "php" | "rb" | "html" | "css" | "scss"
            )
        })
        .unwrap_or(false)
}

fn code_search_rank(path: &Path) -> (usize, usize, String) {
    (
        if path_contains_component(path, "src") { 0 } else { 1 },
        if is_code_file(path) { 0 } else { 1 },
        display_path(path),
    )
}

fn should_skip_dir(path: &Path, ignore_dirs: &[String]) -> bool {
    path.file_name()
        .and_then(|name| name.to_str())
        .map(|name| is_ignored_entry_name(name) || ignore_dirs.iter().any(|ignored| ignored == name))
        .unwrap_or(false)
}

fn glob_to_regex(pattern: &str) -> Option<regex::Regex> {
    let mut out = String::from("^");
    for ch in pattern.replace('\\', "/").chars() {
        match ch {
            '*' => out.push_str(".*"),
            '?' => out.push('.'),
            '/' => out.push('/'),
            ch if ".+()^$|[]{}\\".contains(ch) => {
                out.push('\\');
                out.push(ch);
            }
            ch => out.push(ch),
        }
    }
    out.push('$');
    regex::Regex::new(&out).ok()
}

fn glob_matches(pattern: &str, path: &Path) -> bool {
    let normalized = path.to_string_lossy().replace('\\', "/");
    let file_name = path
        .file_name()
        .map(|name| name.to_string_lossy().replace('\\', "/"))
        .unwrap_or_default();
    glob_to_regex(pattern)
        .map(|re| re.is_match(&normalized) || (!pattern.contains('/') && re.is_match(&file_name)))
        .unwrap_or(false)
}

fn matches_any_glob(path: &Path, base: &Path, patterns: &[String]) -> bool {
    if patterns.is_empty() {
        return true;
    }
    let relative = path.strip_prefix(base).unwrap_or(path);
    patterns.iter().any(|pattern| glob_matches(pattern, relative))
}

fn type_extensions(file_type: &str) -> Vec<&'static str> {
    match file_type.to_lowercase().as_str() {
        "ts" | "typescript" => vec!["ts", "tsx"],
        "js" | "javascript" => vec!["js", "jsx", "mjs", "cjs"],
        "rs" | "rust" => vec!["rs"],
        "vue" => vec!["vue"],
        "py" | "python" => vec!["py"],
        "md" | "markdown" => vec!["md", "mdx"],
        "json" => vec!["json"],
        _ => Vec::new(),
    }
}

fn matches_file_type(path: &Path, file_type: Option<&str>) -> bool {
    let Some(file_type) = file_type else {
        return true;
    };
    let extensions = type_extensions(file_type);
    path.extension()
        .and_then(|ext| ext.to_str())
        .map(|ext| {
            let ext = ext.to_lowercase();
            if extensions.is_empty() {
                ext == file_type.to_lowercase()
            } else {
                extensions.iter().any(|candidate| *candidate == ext)
            }
        })
        .unwrap_or(false)
}

fn matches_filters(
    path: &Path,
    base: &Path,
    include_patterns: &[String],
    exclude_patterns: &[String],
    file_type: Option<&str>,
) -> bool {
    matches_file_type(path, file_type)
        && matches_any_glob(path, base, include_patterns)
        && !matches_any_glob(path, base, exclude_patterns)
}

fn line_matches(
    line: &str,
    pattern: &str,
    re: Option<&regex::Regex>,
    case_insensitive: bool,
    pattern_lower: &str,
) -> bool {
    if let Some(re) = re {
        re.is_match(line)
    } else if case_insensitive {
        line.to_lowercase().contains(pattern_lower)
    } else {
        line.contains(pattern)
    }
}

fn append_match_context(
    result: &mut String,
    path: &Path,
    lines: &[&str],
    match_idx: usize,
    context_lines: usize,
) {
    if context_lines == 0 {
        result.push_str(&format!(
            "{}:{}: {}\n",
            display_path(path),
            match_idx + 1,
            lines[match_idx].trim()
        ));
        return;
    }

    result.push_str(&format!("{}:{}\n", display_path(path), match_idx + 1));
    let start = match_idx.saturating_sub(context_lines);
    let end = (match_idx + context_lines + 1).min(lines.len());
    for (idx, line) in lines.iter().enumerate().take(end).skip(start) {
        let marker = if idx == match_idx { '>' } else { ' ' };
        result.push_str(&format!("{}{:4} | {}\n", marker, idx + 1, line));
    }
}


pub async fn search_repo(
    app: &tauri::AppHandle,
    input: &serde_json::Value,
    session_id: &str,
) -> String {
    let pattern = input["pattern"].as_str().unwrap_or("");
    let dir_str = input["dir"].as_str().unwrap_or(".");
    let use_regex = input["regex"].as_bool().unwrap_or(false);
    let case_insensitive = input["case_insensitive"].as_bool().unwrap_or(false);
    let ws = get_workspace(app, session_id).await;
    if let Err(e) = ensure_path_permission(app, dir_str, "搜索", ws.as_deref()).await {
        return e;
    }

    // 如果启用了正则，先验证正则表达式是否有效
    let compiled_regex = if use_regex {
        let re = regex::RegexBuilder::new(pattern)
            .case_insensitive(case_insensitive)
            .build();
        match re {
            Ok(re) => Some(re),
            Err(e) => return format!("正则表达式无效: {}", e),
        }
    } else {
        None
    };

    let path = Path::new(dir_str);
    let search_dir = if path.is_absolute() {
        path.to_path_buf()
    } else {
        std::env::current_dir().unwrap_or_default().join(path)
    };

    let limit = input_usize(input, "limit")
        .unwrap_or(SEARCH_DEFAULT_LIMIT)
        .min(SEARCH_MAX_LIMIT);
    let context_lines = input_usize(input, "context_lines")
        .unwrap_or(0)
        .min(SEARCH_MAX_CONTEXT_LINES);
    let include_patterns = input_patterns(input, "include");
    let exclude_patterns = input_patterns(input, "exclude");
    let ignore_dirs = input_string_list(input, "ignore_dirs");
    let file_type = input["type"].as_str().or_else(|| input["file_type"].as_str());
    let mut remaining = limit;
    let options = SearchOptions {
        re: compiled_regex.as_ref(),
        case_insensitive,
        context_lines,
        include_patterns: &include_patterns,
        exclude_patterns: &exclude_patterns,
        ignore_dirs: &ignore_dirs,
        file_type,
    };
    let result = search_in_dir(
        &search_dir,
        pattern,
        &mut remaining,
        &options,
    );
    if result.is_empty() {
        format!("未找到包含 '{}' 的内容。", pattern)
    } else {
        result
    }
}

pub struct SearchOptions<'a> {
    re: Option<&'a regex::Regex>,
    case_insensitive: bool,
    context_lines: usize,
    include_patterns: &'a [String],
    exclude_patterns: &'a [String],
    ignore_dirs: &'a [String],
    file_type: Option<&'a str>,
}

/// 在目录中递归搜索关键词（支持正则表达式）
pub fn search_in_dir(
    dir: &Path,
    pattern: &str,
    limit: &mut usize,
    options: &SearchOptions<'_>,
) -> String {
    let mut result = String::new();
    if *limit == 0 {
        return result;
    }

    let entries = match std::fs::read_dir(dir) {
        Ok(e) => e,
        Err(_) => return result,
    };

    // 预编译大小写不敏感的匹配（非正则模式）
    let pattern_lower = if options.case_insensitive && options.re.is_none() {
        pattern.to_lowercase()
    } else {
        String::new()
    };

    let mut paths: Vec<PathBuf> = entries.flatten().map(|entry| entry.path()).collect();
    paths.sort_by_key(|path| code_search_rank(path));

    for path in paths {
        if path.is_dir() {
            if should_skip_dir(&path, options.ignore_dirs) {
                continue;
            }
            result.push_str(&search_in_dir(&path, pattern, limit, options));
        } else if path.is_file() {
            if let Some(ext) = path.extension().and_then(|s| s.to_str()) {
                if is_search_skipped_extension(ext) {
                    continue;
                }
            }
            let base = dir;
            if !matches_filters(
                &path,
                base,
                options.include_patterns,
                options.exclude_patterns,
                options.file_type,
            ) {
                continue;
            }
            if let Ok(decoded) = read_text_preserve_encoding(&path) {
                let content = decoded.content;
                let lines: Vec<&str> = content.lines().collect();
                let mut normal_matches = Vec::new();
                let mut definition_matches = Vec::new();
                for (i, line) in lines.iter().enumerate() {
                    if line_matches(
                        line,
                        pattern,
                        options.re,
                        options.case_insensitive,
                        &pattern_lower,
                    ) {
                        if crate::core::tools::search_tools::looks_like_definition_line(line) {
                            definition_matches.push(i);
                        } else {
                            normal_matches.push(i);
                        }
                    }
                }
                definition_matches.extend(normal_matches);
                for i in definition_matches {
                    append_match_context(&mut result, &path, &lines, i, options.context_lines);
                    *limit = limit.saturating_sub(1);
                    if *limit == 0 {
                        result.push_str("... (搜索结果过多，已被截断)\n");
                        return result;
                    }
                }
            }
        }
    }
    result
}
