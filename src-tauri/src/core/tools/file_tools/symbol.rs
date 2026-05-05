//! # symbol.rs — 轻量符号定位与读取工具
//!
//! 基于文件扩展名和正则规则提供符号定义查找与代码块读取，作为后续 AST/语言服务实现前的轻量版本。

use std::path::{Path, PathBuf};

use crate::core::tools::framework::permission::ensure_path_permission;

use super::common::{
    is_ignored_entry_name, is_locked_file_error, is_search_skipped_extension,
    read_text_preserve_encoding,
};
use super::workspace::get_workspace;

const FIND_SYMBOL_DEFAULT_LIMIT: usize = 50;
const FIND_SYMBOL_MAX_LIMIT: usize = 200;
const FIND_REFERENCES_DEFAULT_LIMIT: usize = 100;
const FIND_REFERENCES_MAX_LIMIT: usize = 500;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum SymbolKind {
    Any,
    Function,
    Class,
    Type,
    Component,
    Variable,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct SymbolCandidate {
    path: PathBuf,
    line_number: usize,
    signature: String,
    kind: SymbolKind,
    confidence: u8,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ReferenceKind {
    PossibleDefinition,
    PossibleReference,
    ImportExport,
}

fn reference_kind_label(kind: ReferenceKind) -> &'static str {
    match kind {
        ReferenceKind::PossibleDefinition => "possible_definition",
        ReferenceKind::PossibleReference => "possible_reference",
        ReferenceKind::ImportExport => "import_export",
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct ReferenceCandidate {
    path: PathBuf,
    line_number: usize,
    line: String,
    kind: ReferenceKind,
}


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

fn parse_kind(kind: Option<&str>) -> SymbolKind {
    match kind.unwrap_or("any").to_lowercase().as_str() {
        "function" => SymbolKind::Function,
        "class" => SymbolKind::Class,
        "type" => SymbolKind::Type,
        "component" => SymbolKind::Component,
        "variable" => SymbolKind::Variable,
        _ => SymbolKind::Any,
    }
}

fn kind_label(kind: SymbolKind) -> &'static str {
    match kind {
        SymbolKind::Any => "any",
        SymbolKind::Function => "function",
        SymbolKind::Class => "class",
        SymbolKind::Type => "type",
        SymbolKind::Component => "component",
        SymbolKind::Variable => "variable",
    }
}

fn kind_matches(actual: SymbolKind, expected: SymbolKind) -> bool {
    expected == SymbolKind::Any || actual == expected
}

fn display_path(path: &Path) -> String {
    let display = std::env::current_dir()
        .ok()
        .and_then(|cwd| path.strip_prefix(cwd).ok().map(PathBuf::from))
        .unwrap_or_else(|| path.to_path_buf());

    display.to_string_lossy().replace('\\', "/")
}

fn resolve_dir(dir: &str) -> PathBuf {
    let path = Path::new(dir);
    if path.is_absolute() {
        path.to_path_buf()
    } else {
        std::env::current_dir().unwrap_or_default().join(path)
    }
}

fn symbol_regex(symbol: &str, pattern: &str) -> Option<regex::Regex> {
    regex::Regex::new(&pattern.replace("{symbol}", &regex::escape(symbol))).ok()
}

fn match_patterns(
    trimmed: &str,
    symbol: &str,
    patterns: &[(&str, SymbolKind, u8)],
) -> Option<(SymbolKind, u8)> {
    patterns.iter().find_map(|(pattern, kind, confidence)| {
        symbol_regex(symbol, pattern)
            .filter(|re| re.is_match(trimmed))
            .map(|_| (*kind, *confidence))
    })
}

fn vue_component_confidence(path: &Path, symbol: &str) -> Option<(SymbolKind, u8)> {
    let stem = path.file_stem()?.to_string_lossy();
    (stem == symbol).then_some((SymbolKind::Component, 70))
}

fn detect_symbol_in_line(path: &Path, line: &str, symbol: &str) -> Option<(SymbolKind, u8)> {
    let trimmed = line.trim();
    if trimmed.is_empty() || trimmed.starts_with("//") || trimmed.starts_with('#') {
        return None;
    }

    let ext = path
        .extension()
        .and_then(|ext| ext.to_str())
        .unwrap_or("")
        .to_lowercase();

    let ts_patterns = [
        (r"^(?:export\s+)?(?:async\s+)?function\s+{symbol}\b", SymbolKind::Function, 95),
        (r"^(?:export\s+)?(?:const|let|var)\s+{symbol}\b", SymbolKind::Variable, 90),
        (r"^(?:export\s+)?class\s+{symbol}\b", SymbolKind::Class, 95),
        (r"^(?:export\s+)?interface\s+{symbol}\b", SymbolKind::Type, 95),
        (r"^(?:export\s+)?type\s+{symbol}\b", SymbolKind::Type, 95),
        (r"^(?:export\s+)?enum\s+{symbol}\b", SymbolKind::Type, 90),
    ];
    let rust_patterns = [
        (r"^(?:pub(?:\([^)]*\))?\s+)?fn\s+{symbol}\b", SymbolKind::Function, 95),
        (r"^(?:pub(?:\([^)]*\))?\s+)?struct\s+{symbol}\b", SymbolKind::Type, 95),
        (r"^(?:pub(?:\([^)]*\))?\s+)?enum\s+{symbol}\b", SymbolKind::Type, 95),
        (r"^(?:pub(?:\([^)]*\))?\s+)?trait\s+{symbol}\b", SymbolKind::Type, 95),
        (r"^(?:pub(?:\([^)]*\))?\s+)?mod\s+{symbol}\b", SymbolKind::Type, 85),
        (r"^macro_rules!\s+{symbol}\b", SymbolKind::Function, 85),
    ];
    let generic_patterns = [
        (r"^def\s+{symbol}\b", SymbolKind::Function, 90),
        (r"^class\s+{symbol}\b", SymbolKind::Class, 90),
        (r"^func\s+(?:\([^)]*\)\s*)?{symbol}\b", SymbolKind::Function, 90),
        (r"^(?:public|private|protected)?\s*(?:static\s+)?class\s+{symbol}\b", SymbolKind::Class, 85),
        (r"^(?:public|private|protected)?\s*(?:static\s+)?(?:async\s+)?[\w<>\[\], ?]+\s+{symbol}\s*\(", SymbolKind::Function, 75),
    ];

    match ext.as_str() {
        "vue" => match_patterns(trimmed, symbol, &ts_patterns)
            .or_else(|| vue_component_confidence(path, symbol)),
        "ts" | "tsx" | "js" | "jsx" => match_patterns(trimmed, symbol, &ts_patterns),
        "rs" => match_patterns(trimmed, symbol, &rust_patterns),
        _ => match_patterns(trimmed, symbol, &generic_patterns),
    }
}

fn collect_files_with_options(dir: &Path, ignore_dirs: &[String], files: &mut Vec<PathBuf>) {
    let entries = match std::fs::read_dir(dir) {
        Ok(entries) => entries,
        Err(_) => return,
    };

    for entry in entries.flatten() {
        let path = entry.path();
        let file_name = path.file_name().unwrap_or_default().to_string_lossy();
        if is_ignored_entry_name(&file_name) || ignore_dirs.iter().any(|ignored| ignored == file_name.as_ref()) {
            continue;
        }

        if path.is_dir() {
            collect_files_with_options(&path, ignore_dirs, files);
        } else if path.is_file() {
            if let Some(ext) = path.extension().and_then(|ext| ext.to_str()) {
                if is_search_skipped_extension(ext) {
                    continue;
                }
            }
            files.push(path);
        }
    }
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

fn symbol_path_rank(path: &Path) -> (usize, usize, String) {
    (
        if path_contains_component(path, "src") { 0 } else { 1 },
        if is_code_file(path) { 0 } else { 1 },
        display_path(path),
    )
}

struct SymbolSearchOptions<'a> {
    include_patterns: &'a [String],
    exclude_patterns: &'a [String],
    ignore_dirs: &'a [String],
    file_type: Option<&'a str>,
}

fn matches_symbol_filters(path: &Path, base: &Path, options: &SymbolSearchOptions<'_>) -> bool {
    matches_file_type(path, options.file_type)
        && matches_any_glob(path, base, options.include_patterns)
        && !matches_any_glob(path, base, options.exclude_patterns)
}

fn classify_reference(path: &Path, line: &str, symbol: &str) -> Option<ReferenceKind> {
    let symbol_re = symbol_regex(symbol, r"\b{symbol}\b")?;
    if !symbol_re.is_match(line) {
        return None;
    }

    let trimmed = line.trim();
    if trimmed.starts_with("import ")
        || trimmed.starts_with("export ")
        || trimmed.starts_with("use ")
        || trimmed.starts_with("mod ")
        || trimmed.contains(" from ")
    {
        return Some(ReferenceKind::ImportExport);
    }

    if detect_symbol_in_line(path, line, symbol).is_some() {
        return Some(ReferenceKind::PossibleDefinition);
    }

    Some(ReferenceKind::PossibleReference)
}

fn find_reference_candidates(
    dir: &Path,
    symbol: &str,
    limit: usize,
    options: &SymbolSearchOptions<'_>,
) -> Vec<ReferenceCandidate> {
    if limit == 0 {
        return Vec::new();
    }

    let mut files = Vec::new();
    if dir.is_file() {
        files.push(dir.to_path_buf());
    } else {
        collect_files_with_options(dir, options.ignore_dirs, &mut files);
    }
    files.retain(|path| matches_symbol_filters(path, dir, options));
    files.sort_by_key(|path| symbol_path_rank(path));

    let mut candidates = Vec::new();
    for path in files {
        let Ok(decoded) = read_text_preserve_encoding(&path) else {
            continue;
        };
        for (idx, line) in decoded.content.lines().enumerate() {
            let Some(kind) = classify_reference(&path, line, symbol) else {
                continue;
            };
            candidates.push(ReferenceCandidate {
                path: path.clone(),
                line_number: idx + 1,
                line: line.trim().to_string(),
                kind,
            });
            if candidates.len() >= limit {
                return candidates;
            }
        }
    }
    candidates
}

fn find_symbol_candidates(
    dir: &Path,
    symbol: &str,
    expected_kind: SymbolKind,
    limit: usize,
    options: &SymbolSearchOptions<'_>,
) -> Vec<SymbolCandidate> {
    if limit == 0 {
        return Vec::new();
    }

    let mut files = Vec::new();
    if dir.is_file() {
        files.push(dir.to_path_buf());
    } else {
        collect_files_with_options(dir, options.ignore_dirs, &mut files);
    }
    files.retain(|path| matches_symbol_filters(path, dir, options));
    files.sort_by_key(|path| symbol_path_rank(path));

    let mut candidates = Vec::new();
    for path in files {
        let Ok(decoded) = read_text_preserve_encoding(&path) else {
            continue;
        };
        for (idx, line) in decoded.content.lines().enumerate() {
            let Some((kind, confidence)) = detect_symbol_in_line(&path, line, symbol) else {
                continue;
            };
            if !kind_matches(kind, expected_kind) {
                continue;
            }
            candidates.push(SymbolCandidate {
                path: path.clone(),
                line_number: idx + 1,
                signature: line.trim().to_string(),
                kind,
                confidence,
            });
            if candidates.len() >= limit {
                return candidates;
            }
        }
    }
    candidates
}

fn find_symbol_in_file(path: &Path, symbol: &str) -> Option<(usize, SymbolKind, u8)> {
    let decoded = read_text_preserve_encoding(path).ok()?;
    decoded
        .content
        .lines()
        .enumerate()
        .find_map(|(idx, line)| detect_symbol_in_line(path, line, symbol).map(|(kind, confidence)| (idx, kind, confidence)))
}

fn leading_indent(line: &str) -> usize {
    line.chars().take_while(|ch| ch.is_whitespace()).count()
}

fn brace_delta(line: &str) -> isize {
    line.chars().fold(0, |acc, ch| match ch {
        '{' => acc + 1,
        '}' => acc - 1,
        _ => acc,
    })
}

fn symbol_block_range(lines: &[&str], start_idx: usize) -> (usize, usize) {
    let mut balance = 0isize;
    let mut saw_brace = false;
    for (idx, line) in lines.iter().enumerate().skip(start_idx) {
        let delta = brace_delta(line);
        if delta != 0 {
            saw_brace = true;
            balance += delta;
        }
        if saw_brace && balance <= 0 && idx > start_idx {
            return (start_idx, idx);
        }
    }

    let start_indent = leading_indent(lines[start_idx]);
    for (idx, line) in lines.iter().enumerate().skip(start_idx + 1) {
        if line.trim().is_empty() {
            continue;
        }
        if leading_indent(line) <= start_indent {
            return (start_idx, idx.saturating_sub(1));
        }
    }
    (start_idx, lines.len().saturating_sub(1))
}

pub async fn find_references(
    app: &tauri::AppHandle,
    input: &serde_json::Value,
    session_id: &str,
) -> String {
    let symbol = input["symbol"].as_str().unwrap_or("").trim();
    if symbol.is_empty() {
        return "FindReferences 错误: symbol 不能为空。".to_string();
    }

    let dir = input["dir"].as_str().unwrap_or(".");
    let limit = input_usize(input, "limit")
        .unwrap_or(FIND_REFERENCES_DEFAULT_LIMIT)
        .min(FIND_REFERENCES_MAX_LIMIT);
    let ws = get_workspace(app, session_id).await;
    if let Err(e) = ensure_path_permission(app, dir, "查找引用", ws.as_deref()).await {
        return e;
    }

    let search_dir = resolve_dir(dir);
    let include_patterns = input_patterns(input, "include");
    let exclude_patterns = input_patterns(input, "exclude");
    let ignore_dirs = input_string_list(input, "ignore_dirs");
    let file_type = input["type"].as_str().or_else(|| input["file_type"].as_str());
    let options = SymbolSearchOptions {
        include_patterns: &include_patterns,
        exclude_patterns: &exclude_patterns,
        ignore_dirs: &ignore_dirs,
        file_type,
    };
    let candidates = find_reference_candidates(&search_dir, symbol, limit, &options);
    if candidates.is_empty() {
        return format!("未找到符号引用: {}", symbol);
    }

    let mut result = format!("Found {} occurrence(s) for symbol '{}':\n", candidates.len(), symbol);
    for candidate in candidates {
        result.push_str(&format!(
            "{}:{} [{}] {}\n",
            display_path(&candidate.path),
            candidate.line_number,
            reference_kind_label(candidate.kind),
            candidate.line
        ));
    }
    result
}

pub async fn find_symbol(
    app: &tauri::AppHandle,
    input: &serde_json::Value,
    session_id: &str,
) -> String {
    let symbol = input["symbol"].as_str().unwrap_or("").trim();
    if symbol.is_empty() {
        return "FindSymbol 错误: symbol 不能为空。".to_string();
    }

    let dir = input["dir"].as_str().unwrap_or(".");
    let expected_kind = parse_kind(input["kind"].as_str());
    let limit = input_usize(input, "limit")
        .unwrap_or(FIND_SYMBOL_DEFAULT_LIMIT)
        .min(FIND_SYMBOL_MAX_LIMIT);
    let ws = get_workspace(app, session_id).await;
    if let Err(e) = ensure_path_permission(app, dir, "查找符号", ws.as_deref()).await {
        return e;
    }

    let search_dir = resolve_dir(dir);
    let include_patterns = input_patterns(input, "include");
    let exclude_patterns = input_patterns(input, "exclude");
    let ignore_dirs = input_string_list(input, "ignore_dirs");
    let file_type = input["type"].as_str().or_else(|| input["file_type"].as_str());
    let options = SymbolSearchOptions {
        include_patterns: &include_patterns,
        exclude_patterns: &exclude_patterns,
        ignore_dirs: &ignore_dirs,
        file_type,
    };
    let candidates = find_symbol_candidates(&search_dir, symbol, expected_kind, limit, &options);
    if candidates.is_empty() {
        return format!("未找到符号定义: {}", symbol);
    }

    let mut result = format!("Found {} candidate(s) for symbol '{}':\n", candidates.len(), symbol);
    for candidate in candidates {
        result.push_str(&format!(
            "{}:{} [{} confidence={}] {}\n",
            display_path(&candidate.path),
            candidate.line_number,
            kind_label(candidate.kind),
            candidate.confidence,
            candidate.signature
        ));
    }
    result
}

pub async fn code_search(
    app: &tauri::AppHandle,
    input: &serde_json::Value,
    session_id: &str,
) -> String {
    let query = input["query"].as_str().unwrap_or("").trim();
    if query.is_empty() {
        return "CodeSearch 错误: query 不能为空。".to_string();
    }

    let dir = input["dir"].as_str().unwrap_or(".");
    let limit = input_usize(input, "limit").unwrap_or(30).min(100);
    let ws = get_workspace(app, session_id).await;
    if let Err(e) = ensure_path_permission(app, dir, "组合代码搜索", ws.as_deref()).await {
        return e;
    }

    let search_dir = resolve_dir(dir);
    let mut include_patterns = input_patterns(input, "include");
    let exclude_patterns = input_patterns(input, "exclude");
    let ignore_dirs = input_string_list(input, "ignore_dirs");
    let file_type = input["type"].as_str().or_else(|| input["file_type"].as_str());
    if include_patterns.is_empty() {
        include_patterns.push("**/*".to_string());
    }
    let options = SymbolSearchOptions {
        include_patterns: &include_patterns,
        exclude_patterns: &exclude_patterns,
        ignore_dirs: &ignore_dirs,
        file_type,
    };

    let mut files = Vec::new();
    if search_dir.is_file() {
        files.push(search_dir.clone());
    } else {
        collect_files_with_options(&search_dir, options.ignore_dirs, &mut files);
    }
    files.retain(|path| matches_symbol_filters(path, &search_dir, &options));
    files.sort_by_key(|path| symbol_path_rank(path));

    let symbol_candidates = find_symbol_candidates(
        &search_dir,
        query,
        SymbolKind::Any,
        limit,
        &options,
    );

    let query_lower = query.to_lowercase();
    let mut text_matches = Vec::new();
    for path in &files {
        let Ok(decoded) = read_text_preserve_encoding(path) else {
            continue;
        };
        for (idx, line) in decoded.content.lines().enumerate() {
            if line.to_lowercase().contains(&query_lower) {
                text_matches.push((path.clone(), idx + 1, line.trim().to_string()));
                if text_matches.len() >= limit {
                    break;
                }
            }
        }
        if text_matches.len() >= limit {
            break;
        }
    }

    if symbol_candidates.is_empty() && text_matches.is_empty() && files.is_empty() {
        return format!("未找到与 '{}' 相关的代码结果。", query);
    }

    let mut result = format!("CodeSearch results for '{}':\n", query);
    if !symbol_candidates.is_empty() {
        result.push_str("\n[Symbols - 可直接 ReadSymbol]\n");
        for candidate in symbol_candidates {
            result.push_str(&format!(
                "{}:{} [{} confidence={}] {}\n  next: ReadSymbol path=\"{}\" symbol=\"{}\"\n",
                display_path(&candidate.path),
                candidate.line_number,
                kind_label(candidate.kind),
                candidate.confidence,
                candidate.signature,
                display_path(&candidate.path),
                query
            ));
        }
    }

    if !text_matches.is_empty() {
        result.push_str("\n[Text matches - 可直接 ReadFile]\n");
        for (path, line_number, line) in text_matches {
            let start = line_number.saturating_sub(3).max(1);
            let end = line_number + 3;
            result.push_str(&format!(
                "{}:{} {}\n  next: ReadFile path=\"{}\" start_line={} end_line={}\n",
                display_path(&path),
                line_number,
                line,
                display_path(&path),
                start,
                end
            ));
        }
    }

    result
}

pub async fn read_symbol(
    app: &tauri::AppHandle,
    input: &serde_json::Value,
    session_id: &str,
) -> String {
    let path = input["path"].as_str().unwrap_or("");
    let symbol = input["symbol"].as_str().unwrap_or("").trim();
    if path.is_empty() || symbol.is_empty() {
        return "ReadSymbol 错误: path 和 symbol 不能为空。".to_string();
    }

    let ws = get_workspace(app, session_id).await;
    if let Err(e) = ensure_path_permission(app, path, "读取符号", ws.as_deref()).await {
        return e;
    }

    let path = Path::new(path);
    let decoded = match read_text_preserve_encoding(path) {
        Ok(decoded) => decoded,
        Err(e) => {
            let err_msg = e.to_string();
            if is_locked_file_error(&err_msg) {
                return format!(
                    "读取错误: 文件可能被其他智能体或程序锁定，请稍后重试。详细错误: {}",
                    e
                );
            }
            return format!("读取错误: {}", e);
        }
    };
    let lines: Vec<&str> = decoded.content.lines().collect();
    let Some((start_idx, kind, confidence)) = find_symbol_in_file(path, symbol) else {
        return format!("未在文件中找到符号定义: {}", symbol);
    };
    let (start_idx, end_idx) = symbol_block_range(&lines, start_idx);

    let mut result = format!(
        "[Symbol: {}] [Kind: {} confidence={}] [File: {}] (Lines: {}-{})\n",
        symbol,
        kind_label(kind),
        confidence,
        path.display(),
        start_idx + 1,
        end_idx + 1
    );
    for (idx, line) in lines.iter().enumerate().take(end_idx + 1).skip(start_idx) {
        result.push_str(&format!("{:4} | {}\n", idx + 1, line));
    }
    result
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn detects_typescript_function() {
        let path = Path::new("src/store.ts");
        let detected = detect_symbol_in_line(path, "export function loadSession() {}", "loadSession");
        assert_eq!(detected, Some((SymbolKind::Function, 95)));
    }

    #[test]
    fn detects_rust_trait() {
        let path = Path::new("src/lib.rs");
        let detected = detect_symbol_in_line(path, "pub trait ToolRunner {", "ToolRunner");
        assert_eq!(detected, Some((SymbolKind::Type, 95)));
    }

    #[test]
    fn classifies_reference_kinds() {
        let path = Path::new("src/store.ts");
        assert_eq!(
            classify_reference(path, "export function loadSession() {}", "loadSession"),
            Some(ReferenceKind::ImportExport)
        );
        assert_eq!(
            classify_reference(path, "const next = loadSession()", "loadSession"),
            Some(ReferenceKind::PossibleReference)
        );
        assert_eq!(
            classify_reference(path, "import { loadSession } from './session'", "loadSession"),
            Some(ReferenceKind::ImportExport)
        );
    }
}
