//! # symbol.rs — 轻量符号定位与读取工具
//!
//! 基于文件扩展名和正则规则提供符号定义查找与代码块读取，作为后续 AST/语言服务实现前的轻量版本。

use std::path::{Path, PathBuf};

use crate::core::tools::framework;
use crate::core::tools::framework::permission::ensure_read_path_permission;

use super::common::{
    display_path, input_patterns, input_string_list, input_usize, is_locked_file_error,
    is_search_skipped_extension, is_skippable_dir_name, passes_file_filters,
    read_text_preserve_encoding, record_file_read, search_path_rank,
};
use super::workspace::{get_workspace, resolve_exec_path, sandbox_missing_hint};

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

fn resolve_dir(dir: &str, workspace: Option<&Path>) -> PathBuf {
    let path = Path::new(dir);
    if path.is_absolute() {
        path.to_path_buf()
    } else {
        workspace
            .map(Path::to_path_buf)
            .unwrap_or_else(|| std::env::current_dir().unwrap_or_default())
            .join(path)
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
        (
            r"^(?:export\s+)?(?:async\s+)?function\s+{symbol}\b",
            SymbolKind::Function,
            95,
        ),
        (
            r"^(?:export\s+)?(?:const|let|var)\s+{symbol}\b",
            SymbolKind::Variable,
            90,
        ),
        (r"^(?:export\s+)?class\s+{symbol}\b", SymbolKind::Class, 95),
        (
            r"^(?:export\s+)?interface\s+{symbol}\b",
            SymbolKind::Type,
            95,
        ),
        (r"^(?:export\s+)?type\s+{symbol}\b", SymbolKind::Type, 95),
        (r"^(?:export\s+)?enum\s+{symbol}\b", SymbolKind::Type, 90),
    ];
    let rust_patterns = [
        (
            r"^(?:pub(?:\([^)]*\))?\s+)?fn\s+{symbol}\b",
            SymbolKind::Function,
            95,
        ),
        (
            r"^(?:pub(?:\([^)]*\))?\s+)?struct\s+{symbol}\b",
            SymbolKind::Type,
            95,
        ),
        (
            r"^(?:pub(?:\([^)]*\))?\s+)?enum\s+{symbol}\b",
            SymbolKind::Type,
            95,
        ),
        (
            r"^(?:pub(?:\([^)]*\))?\s+)?trait\s+{symbol}\b",
            SymbolKind::Type,
            95,
        ),
        (
            r"^(?:pub(?:\([^)]*\))?\s+)?mod\s+{symbol}\b",
            SymbolKind::Type,
            85,
        ),
        (r"^macro_rules!\s+{symbol}\b", SymbolKind::Function, 85),
    ];
    let generic_patterns = [
        (r"^def\s+{symbol}\b", SymbolKind::Function, 90),
        (r"^class\s+{symbol}\b", SymbolKind::Class, 90),
        (
            r"^func\s+(?:\([^)]*\)\s*)?{symbol}\b",
            SymbolKind::Function,
            90,
        ),
        (
            r"^(?:public|private|protected)?\s*(?:static\s+)?class\s+{symbol}\b",
            SymbolKind::Class,
            85,
        ),
        (
            r"^(?:public|private|protected)?\s*(?:static\s+)?(?:async\s+)?[\w<>\[\], ?]+\s+{symbol}\s*\(",
            SymbolKind::Function,
            75,
        ),
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
        if is_skippable_dir_name(&file_name, ignore_dirs) {
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

struct SymbolSearchOptions<'a> {
    include_patterns: &'a [String],
    exclude_patterns: &'a [String],
    ignore_dirs: &'a [String],
    file_type: Option<&'a str>,
}

/// 把工具选项摊平成 common 的过滤入口（include / exclude 的空集合语义见 common.rs）。
fn passes_options(path: &Path, base: &Path, options: &SymbolSearchOptions<'_>) -> bool {
    passes_file_filters(
        path,
        base,
        options.include_patterns,
        options.exclude_patterns,
        options.file_type,
    )
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
    files.retain(|path| passes_options(path, dir, options));
    files.sort_by_key(|path| search_path_rank(path));

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
    files.retain(|path| passes_options(path, dir, options));
    files.sort_by_key(|path| search_path_rank(path));

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
    decoded.content.lines().enumerate().find_map(|(idx, line)| {
        detect_symbol_in_line(path, line, symbol).map(|(kind, confidence)| (idx, kind, confidence))
    })
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

/// SymbolSearch 支持的三种查询类型。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum SymbolOperation {
    Definition,
    References,
    Block,
}

fn parse_operation(raw: &str) -> Option<SymbolOperation> {
    match raw {
        "definition" => Some(SymbolOperation::Definition),
        "references" => Some(SymbolOperation::References),
        "block" => Some(SymbolOperation::Block),
        _ => None,
    }
}

/// SymbolSearch 统一入口：按 `operation` 分发到定义查找 / 引用查找 / 代码块读取。
///
/// 2026-11-20 由 FindSymbol / FindReferences / ReadSymbol 三工具合并而来（CodeSearch 退役）：
/// 三者本就共用同一套符号识别引擎，拆成多个工具只会增加模型的选择负担与描述 token。
pub async fn symbol_search(
    app: &tauri::AppHandle,
    input: &serde_json::Value,
    session_id: &str,
) -> framework::ToolCallResult {
    match parse_operation(input["operation"].as_str().unwrap_or("")) {
        Some(SymbolOperation::Definition) => find_symbol(app, input, session_id).await,
        Some(SymbolOperation::References) => find_references(app, input, session_id).await,
        Some(SymbolOperation::Block) => read_symbol(app, input, session_id).await,
        None => framework::ToolCallResult::error(
            "SymbolSearch 错误: operation 必须是 definition、references 或 block 之一。"
                .to_string(),
        ),
    }
}

pub async fn find_references(
    app: &tauri::AppHandle,
    input: &serde_json::Value,
    session_id: &str,
) -> framework::ToolCallResult {
    let symbol = input["symbol"].as_str().unwrap_or("").trim();
    if symbol.is_empty() {
        return framework::ToolCallResult::error(
            "SymbolSearch(references) 错误: symbol 不能为空。".to_string(),
        );
    }

    let dir = input["dir"].as_str().unwrap_or(".");
    let limit = input_usize(input, "limit")
        .unwrap_or(FIND_REFERENCES_DEFAULT_LIMIT)
        .min(FIND_REFERENCES_MAX_LIMIT);
    let ws = get_workspace(app, session_id).await;
    if let Err(e) = ensure_read_path_permission(dir) {
        return framework::ToolCallResult::error(e);
    }

    let search_dir = resolve_dir(dir, ws.as_deref());
    // 目录不存在就明确报错：静默返回"未找到"会把"路径写错"伪装成"符号不存在"
    if !search_dir.exists() {
        return framework::ToolCallResult::error(format!(
            "SymbolSearch(references) 错误: 搜索目录不存在 —— {}",
            search_dir.display()
        ));
    }
    let include_patterns = input_patterns(input, "include");
    let exclude_patterns = input_patterns(input, "exclude");
    let ignore_dirs = input_string_list(input, "ignore_dirs");
    let file_type = input["type"]
        .as_str()
        .or_else(|| input["file_type"].as_str());
    let options = SymbolSearchOptions {
        include_patterns: &include_patterns,
        exclude_patterns: &exclude_patterns,
        ignore_dirs: &ignore_dirs,
        file_type,
    };
    let candidates = find_reference_candidates(&search_dir, symbol, limit, &options);
    if candidates.is_empty() {
        return framework::ToolCallResult::ok(format!("未找到符号引用: {}", symbol));
    }

    let mut result = format!(
        "Found {} occurrence(s) for symbol '{}':\n",
        candidates.len(),
        symbol
    );
    for candidate in candidates {
        result.push_str(&format!(
            "{}:{} [{}] {}\n",
            display_path(&candidate.path),
            candidate.line_number,
            reference_kind_label(candidate.kind),
            candidate.line
        ));
    }
    framework::ToolCallResult::ok(result)
}

pub async fn find_symbol(
    app: &tauri::AppHandle,
    input: &serde_json::Value,
    session_id: &str,
) -> framework::ToolCallResult {
    let symbol = input["symbol"].as_str().unwrap_or("").trim();
    if symbol.is_empty() {
        return framework::ToolCallResult::error(
            "SymbolSearch(definition) 错误: symbol 不能为空。".to_string(),
        );
    }

    let dir = input["dir"].as_str().unwrap_or(".");
    let expected_kind = parse_kind(input["kind"].as_str());
    let limit = input_usize(input, "limit")
        .unwrap_or(FIND_SYMBOL_DEFAULT_LIMIT)
        .min(FIND_SYMBOL_MAX_LIMIT);
    let ws = get_workspace(app, session_id).await;
    if let Err(e) = ensure_read_path_permission(dir) {
        return framework::ToolCallResult::error(e);
    }

    let search_dir = resolve_dir(dir, ws.as_deref());
    // 同上：目录不存在时显式报错，避免与"符号确实不存在"混淆
    if !search_dir.exists() {
        return framework::ToolCallResult::error(format!(
            "SymbolSearch(definition) 错误: 搜索目录不存在 —— {}",
            search_dir.display()
        ));
    }
    let include_patterns = input_patterns(input, "include");
    let exclude_patterns = input_patterns(input, "exclude");
    let ignore_dirs = input_string_list(input, "ignore_dirs");
    let file_type = input["type"]
        .as_str()
        .or_else(|| input["file_type"].as_str());
    let options = SymbolSearchOptions {
        include_patterns: &include_patterns,
        exclude_patterns: &exclude_patterns,
        ignore_dirs: &ignore_dirs,
        file_type,
    };
    let candidates = find_symbol_candidates(&search_dir, symbol, expected_kind, limit, &options);
    if candidates.is_empty() {
        return framework::ToolCallResult::ok(format!("未找到符号定义: {}", symbol));
    }

    let mut result = format!(
        "Found {} candidate(s) for symbol '{}':\n",
        candidates.len(),
        symbol
    );
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
    framework::ToolCallResult::ok(result)
}

pub async fn read_symbol(
    app: &tauri::AppHandle,
    input: &serde_json::Value,
    session_id: &str,
) -> framework::ToolCallResult {
    let path = input["path"].as_str().unwrap_or("");
    let symbol = input["symbol"].as_str().unwrap_or("").trim();
    if path.is_empty() || symbol.is_empty() {
        return framework::ToolCallResult::error(
            "SymbolSearch(block) 错误: path 和 symbol 不能为空。".to_string(),
        );
    }

    let ws = get_workspace(app, session_id).await;
    if let Err(e) = ensure_read_path_permission(path) {
        return framework::ToolCallResult::error(e);
    }

    // 执行层与校验层对齐：相对路径按沙箱目录解析（而非进程 CWD），否则会越界读取
    let exec_path = resolve_exec_path(path, ws.as_deref());
    let path = Path::new(&exec_path);
    let decoded = match read_text_preserve_encoding(path) {
        Ok(decoded) => decoded,
        Err(e) => {
            let err_msg = e.to_string();
            if is_locked_file_error(&err_msg) {
                return framework::ToolCallResult::error(format!(
                    "读取错误: 文件可能被其他智能体或程序锁定，请稍后重试。详细错误: {}",
                    e
                ));
            }
            return framework::ToolCallResult::error(format!(
                "读取错误: {}{}",
                e,
                sandbox_missing_hint(ws.as_deref())
            ));
        }
    };
    let lines: Vec<&str> = decoded.content.lines().collect();
    let Some((start_idx, kind, confidence)) = find_symbol_in_file(path, symbol) else {
        return framework::ToolCallResult::error(format!("未在文件中找到符号定义: {}", symbol));
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
    // 先读后改：SymbolSearch(block) 返回的是未经修改的原文片段（真实认知），计入已读。
    // 指纹同样算在全量内容上（decoded.content），与 EditFile 校验口径一致。
    // 注意：ReadFileSkeleton 只返回加工后的签名摘要、不算"看过"，刻意不在此列。
    record_file_read(app, session_id, &exec_path, &decoded.content).await;
    framework::ToolCallResult::ok(result)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_only_the_three_documented_operations() {
        assert_eq!(
            parse_operation("definition"),
            Some(SymbolOperation::Definition)
        );
        assert_eq!(
            parse_operation("references"),
            Some(SymbolOperation::References)
        );
        assert_eq!(parse_operation("block"), Some(SymbolOperation::Block));
        assert_eq!(parse_operation("find"), None, "旧工具名不是合法 operation");
        assert_eq!(parse_operation(""), None);
    }

    #[test]
    fn detects_typescript_function() {
        let path = Path::new("src/store.ts");
        let detected =
            detect_symbol_in_line(path, "export function loadSession() {}", "loadSession");
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
            classify_reference(
                path,
                "import { loadSession } from './session'",
                "loadSession"
            ),
            Some(ReferenceKind::ImportExport)
        );
    }

    /// 端到端回归护栏：不打 include / exclude 规则时，必须能在真实目录里找到符号定义。
    ///
    /// 旧实现把"空 exclude"算成"排除所有文件"，文件列表被清空，
    /// 于是这个最普通的调用形态必然返回空——SymbolSearch 等工具就是这样集体失效的。
    #[test]
    fn finds_symbol_without_any_patterns() {
        let dir = std::env::temp_dir().join(format!("jarvis_symbol_it_{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.join("src")).unwrap();
        std::fs::write(
            dir.join("src").join("search.rs"),
            "pub fn search_in_dir(dir: &Path) -> String {\n    String::new()\n}\n",
        )
        .unwrap();

        let options = SymbolSearchOptions {
            include_patterns: &[],
            exclude_patterns: &[],
            ignore_dirs: &[],
            file_type: None,
        };
        let candidates =
            find_symbol_candidates(&dir, "search_in_dir", SymbolKind::Any, 50, &options);

        assert!(
            !candidates.is_empty(),
            "无 include/exclude 时应能找到 search_in_dir，实际返回空（空规则语义回归）"
        );
        assert!(candidates[0].path.ends_with("search.rs"));

        let _ = std::fs::remove_dir_all(&dir);
    }

    /// 同一场景下的引用查找：定义与调用两处都应被找到。
    #[test]
    fn finds_references_without_any_patterns() {
        let dir = std::env::temp_dir().join(format!("jarvis_ref_it_{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.join("src")).unwrap();
        std::fs::write(
            dir.join("src").join("caller.rs"),
            "use crate::search_in_dir;\n\npub fn run() {\n    let _ = search_in_dir();\n}\n",
        )
        .unwrap();

        let options = SymbolSearchOptions {
            include_patterns: &[],
            exclude_patterns: &[],
            ignore_dirs: &[],
            file_type: None,
        };
        let candidates = find_reference_candidates(&dir, "search_in_dir", 50, &options);

        assert!(
            !candidates.is_empty(),
            "无 include/exclude 时应能找到 search_in_dir 的引用，实际返回空"
        );

        let _ = std::fs::remove_dir_all(&dir);
    }
}
