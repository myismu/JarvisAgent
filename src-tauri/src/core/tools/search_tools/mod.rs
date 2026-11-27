//! # search_tools/mod.rs — 文件搜索工具
//!
//! 提供只读、并发安全的文件搜索工具：
//! - `glob`: 按 glob 模式查找文件路径，结果按修改时间倒序排列
//!
//! （原 Claude Code Grep 风格的 SearchText 已于 2026-11-20 退役：实测其高阶参数从未被使用，
//! 内容搜索统一走 SearchRepo，保留两份实现只会造成模型的选择摇摆。）

use serde_json::json;
use std::path::{Path, PathBuf};
use std::time::Instant;
use tauri::Manager;

use super::framework::permission::{ensure_read_path_permission, is_path_safe};
use super::framework::registry::ToolDef;
use crate::core::tools::framework;

const GLOB_DEFAULT_LIMIT: usize = 100;

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
    // Windows 系统 / 用户数据目录（文件量巨大）
    "AppData",
    "Application Data",
    "Local Settings",
    "Temp",
    "tmp",
    "ProgramData",
    "All Users",
    "Default",
    "Default User",
    "Windows",
    "System32",
    "SysWOW64",
    "WinSxS",
    "Microsoft",
    "MicrosoftEdgeBackups",
    "Recovery",
    "System Volume Information",
    "$Recycle.Bin",
    // 包管理器缓存
    ".npm",
    ".cargo",
    ".rustup",
    ".nuget",
    ".gem",
    ".pip",
    ".ivy2",
    ".m2",
    ".gradle",
    ".yarn",
    ".pnpm-store",
    ".pnp",
    ".nvm",
    // IDE / 工具缓存
    ".vscode",
    ".idea",
    ".settings",
    ".metadata",
    ".dart_tool",
    ".pub-cache",
    ".stack-work",
    // 跨平台
    "Library",
    "Caches",
];

async fn get_workspace(app: &tauri::AppHandle, session_id: &str) -> Option<PathBuf> {
    if let Some(manager) = app.try_state::<crate::core::runtime::SessionManager>() {
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
    _app: &tauri::AppHandle,
    raw_path: Option<&str>,
    resolved: &Path,
    _action: &str,
    _workspace: Option<&Path>,
) -> Result<(), String> {
    let raw = raw_path.unwrap_or(".").trim();
    let raw = if raw.is_empty() { "." } else { raw };
    if !is_path_safe(raw) {
        return Err("路径不安全：包含 '..' 遍历".to_string());
    }
    ensure_read_path_permission(&resolved.to_string_lossy())
}

fn should_skip_dir_with_overrides(path: &Path, ignore_dirs: &[String]) -> bool {
    path.file_name()
        .and_then(|name| name.to_str())
        .map(|name| SKIP_DIRS.contains(&name) || ignore_dirs.iter().any(|ignored| ignored == name))
        .unwrap_or(false)
}

/// 遍历目录树，在遍历过程中内联匹配文件和目录的 glob pattern。
/// 使用 `&mut usize limit` 引用计数实现早停：每匹配一项减一，归零立即返回。
///
/// 此函数在遍历阶段完成所有过滤，避免对巨型目录树的全量收集。
///
/// `limit` 为 0 时不会收集任何条目（调用方应避免此情况）。
#[allow(clippy::too_many_arguments)]
fn collect_files_glob(
    root: &Path,
    pattern: &str,
    base: &Path,
    include_patterns: &[String],
    exclude_patterns: &[String],
    file_type: Option<&str>,
    ignore_dirs: &[String],
    limit: &mut usize,
    files: &mut Vec<PathBuf>,
) {
    if *limit == 0 {
        return;
    }

    let entries = match std::fs::read_dir(root) {
        Ok(entries) => entries,
        Err(_) => return,
    };

    for entry in entries.flatten() {
        if *limit == 0 {
            return;
        }

        let path = entry.path();

        if path.is_dir() {
            if should_skip_dir_with_overrides(&path, ignore_dirs) {
                continue;
            }
            // Check if the directory itself matches the pattern
            let relative = path.strip_prefix(base).unwrap_or(&path);
            if glob_matches(pattern, relative)
                && matches_search_filters(
                    &path,
                    base,
                    include_patterns,
                    exclude_patterns,
                    file_type,
                )
            {
                files.push(path.clone());
                *limit -= 1;
                if *limit == 0 {
                    return;
                }
            }
            // Recurse to find matching files/subdirs within
            collect_files_glob(
                &path,
                pattern,
                base,
                include_patterns,
                exclude_patterns,
                file_type,
                ignore_dirs,
                limit,
                files,
            );
        } else if path.is_file() {
            let relative = path.strip_prefix(base).unwrap_or(&path);
            if glob_matches(pattern, relative)
                && matches_search_filters(
                    &path,
                    base,
                    include_patterns,
                    exclude_patterns,
                    file_type,
                )
            {
                files.push(path.clone());
                *limit -= 1;
                if *limit == 0 {
                    return;
                }
            }
        }
    }
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

fn input_patterns(input: &serde_json::Value, key: &str) -> Vec<String> {
    input[key]
        .as_str()
        .map(split_glob_patterns)
        .unwrap_or_default()
}

fn input_string_list(input: &serde_json::Value, key: &str) -> Vec<String> {
    if let Some(raw) = input[key].as_str() {
        return raw
            .split([',', ' '])
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

fn matches_search_filters(
    path: &Path,
    base: &Path,
    include_patterns: &[String],
    exclude_patterns: &[String],
    file_type: Option<&str>,
) -> bool {
    matches_type(path, file_type)
        && matches_any_glob(path, base, include_patterns)
        && (exclude_patterns.is_empty() || !matches_any_glob(path, base, exclude_patterns))
}

fn is_code_extension(ext: &str) -> bool {
    matches!(
        ext.to_lowercase().as_str(),
        "rs" | "ts"
            | "tsx"
            | "js"
            | "jsx"
            | "vue"
            | "svelte"
            | "py"
            | "go"
            | "java"
            | "kt"
            | "kts"
            | "swift"
            | "c"
            | "h"
            | "cpp"
            | "cc"
            | "cxx"
            | "hpp"
            | "cs"
            | "php"
            | "rb"
            | "scala"
            | "sql"
            | "html"
            | "css"
            | "scss"
            | "less"
    )
}

fn is_code_file(path: &Path) -> bool {
    path.extension()
        .and_then(|ext| ext.to_str())
        .map(is_code_extension)
        .unwrap_or(false)
}

fn path_contains_component(path: &Path, component: &str) -> bool {
    path.components().any(|part| part.as_os_str() == component)
}

fn path_noise_score(path: &Path) -> usize {
    let noisy = [
        "node_modules",
        "target",
        "dist",
        "build",
        ".next",
        ".cache",
        "coverage",
    ];
    noisy
        .iter()
        .filter(|component| path_contains_component(path, component))
        .count()
}

fn code_search_rank(path: &Path) -> (usize, usize, usize, String) {
    (
        if path_contains_component(path, "src") {
            0
        } else {
            1
        },
        if is_code_file(path) { 0 } else { 1 },
        path_noise_score(path),
        normalize_path_for_match(path),
    )
}

fn sort_for_code_search(paths: &mut [PathBuf]) {
    paths.sort_by_key(|path| code_search_rank(path));
}

pub(crate) fn looks_like_definition_line(line: &str) -> bool {
    let trimmed = line.trim_start();
    trimmed.starts_with("fn ")
        || trimmed.starts_with("pub fn ")
        || trimmed.starts_with("function ")
        || trimmed.starts_with("export function ")
        || trimmed.starts_with("const ")
        || trimmed.starts_with("export const ")
        || trimmed.starts_with("class ")
        || trimmed.starts_with("export class ")
        || trimmed.starts_with("interface ")
        || trimmed.starts_with("export interface ")
        || trimmed.starts_with("type ")
        || trimmed.starts_with("export type ")
        || trimmed.starts_with("struct ")
        || trimmed.starts_with("pub struct ")
        || trimmed.starts_with("enum ")
        || trimmed.starts_with("pub enum ")
        || trimmed.starts_with("trait ")
        || trimmed.starts_with("pub trait ")
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

/// Claude Code 风格 Glob：按文件名 glob 模式快速查找文件。
pub async fn glob(
    app: &tauri::AppHandle,
    input: &serde_json::Value,
    session_id: &str,
) -> framework::ToolCallResult {
    let pattern = input["pattern"].as_str().unwrap_or("").trim();
    if pattern.is_empty() {
        return framework::ToolCallResult::error("Glob 错误: pattern 不能为空。".to_string());
    }

    let raw_path = input["path"].as_str().or_else(|| input["dir"].as_str());
    let limit = value_as_usize(input, "limit").unwrap_or(GLOB_DEFAULT_LIMIT);
    let file_type = input["type"]
        .as_str()
        .or_else(|| input["file_type"].as_str());
    let include_patterns = input_patterns(input, "include");
    let exclude_patterns = input_patterns(input, "exclude");
    let ignore_dirs = input_string_list(input, "ignore_dirs");
    let workspace = get_workspace(app, session_id).await;
    let base = match resolve_path(raw_path, workspace.as_deref()) {
        Ok(path) => path,
        Err(e) => return framework::ToolCallResult::error(e),
    };

    if let Err(e) =
        ensure_resolved_path_permission(app, raw_path, &base, "Glob 查找", workspace.as_deref())
            .await
    {
        return framework::ToolCallResult::error(e);
    }

    if !base.exists() {
        return framework::ToolCallResult::error(format!(
            "Directory does not exist: {}",
            base.display()
        ));
    }
    if !base.is_dir() {
        return framework::ToolCallResult::error(format!(
            "Path is not a directory: {}",
            base.display()
        ));
    }

    let start = Instant::now();
    let mut matches = Vec::new();
    // limit == 0 表示无限制，用 usize::MAX 模拟；否则收集到 limit 个匹配即早停
    let mut remaining = if limit == 0 { usize::MAX } else { limit };

    collect_files_glob(
        &base,
        pattern,
        &base,
        &include_patterns,
        &exclude_patterns,
        file_type,
        &ignore_dirs,
        &mut remaining,
        &mut matches,
    );

    let limit_reached = remaining == 0;
    sort_for_code_search(&mut matches);
    let truncated = limit > 0 && limit_reached;
    if limit > 0 && matches.len() > limit {
        matches.truncate(limit);
    }

    if matches.is_empty() {
        return framework::ToolCallResult::ok("No files found".to_string());
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
    framework::ToolCallResult::ok(lines.join("\n"))
}

// --- 工具注册 ---
crate::define_tools! {
    pub fn register_tools(registry) {
        ToolDef {
            name: "FindFiles",
            description: "按 glob 模式快速查找文件路径",
            search_hint: "glob wildcard filename file pattern find match",
            schema: json!({
                "name": "FindFiles",
                "description": "Fast file pattern matching tool. Supports glob patterns like \"**/*.js\" or \"src/**/*.ts\". Returns matching file paths sorted by modification time. Use this when you need to find files by name patterns.",
                "input_schema": {
                    "type": "object",
                    "properties": {
                        "pattern": {
                            "type": "string",
                            "description": "The glob pattern to match files against"
                        },
                        "dir": {
                            "type": "string",
                            "description": "The directory to search in. Alias of path. If omitted, the current workspace directory is used."
                        },
                        "path": {
                            "type": "string",
                            "description": "The directory to search in. If omitted, the current workspace directory is used. Must be a valid directory path if provided."
                        },
                        "limit": {
                            "type": "integer",
                            "description": "Maximum number of matching files to return. Defaults to 100. Pass 0 for unlimited."
                        },
                        "include": {
                            "type": "string",
                            "description": "Optional include glob filter applied after pattern matching. Example: src/** or **/*.{ts,tsx}."
                        },
                        "exclude": {
                            "type": "string",
                            "description": "Optional exclude glob filter. Example: **/*.test.ts or dist/**."
                        },
                        "type": {
                            "type": "string",
                            "description": "Optional file type filter. Common values: rust, typescript, vue, json, md."
                        },
                        "ignore_dirs": {
                            "type": ["string", "array"],
                            "items": {"type": "string"},
                            "description": "Additional directory names to ignore, as comma/space separated string or string array."
                        }
                    },
                    "required": ["pattern"]
                }
            }),
            category: "搜索检索",
            should_defer: false,
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
    fn test_type_extensions_common_aliases() {
        assert!(type_extensions("rust").contains(&"rs".to_string()));
        assert!(type_extensions("typescript").contains(&"tsx".to_string()));
    }

    // --- glob_matches 对目录名的匹配 ---

    #[test]
    fn test_glob_matches_directory_name() {
        // 目录名 "Desktop" 应直接匹配 pattern "Desktop"
        assert!(glob_matches("Desktop", Path::new("Users/Alice/Desktop")));
        assert!(glob_matches("Desktop", Path::new("Desktop")));
        // "Documents" 不匹配
        assert!(!glob_matches("Desktop", Path::new("Documents")));
    }

    #[test]
    fn test_glob_matches_file_inside_directory() {
        // 仅当 pattern 包含通配符时，目录内的文件才匹配
        assert!(glob_matches(
            "**/Desktop/**",
            Path::new("Users/Alice/Desktop/notes.txt")
        ));
        assert!(glob_matches("Desktop/*", Path::new("Desktop/notes.txt")));
        // 精确 "Desktop" 不匹配目录内的文件
        assert!(!glob_matches("Desktop", Path::new("Desktop/notes.txt")));
    }

    // --- collect_files_glob 早停行为 ---

    #[test]
    fn test_collect_files_glob_early_termination() {
        use std::fs;
        let tmp = std::env::temp_dir().join("__jarvis_glob_early_stop_test__");
        let _ = fs::remove_dir_all(&tmp);
        fs::create_dir_all(&tmp).unwrap();

        // 创建目录结构:
        // tmp/
        //   a.txt
        //   b.txt
        //   c.txt
        //   d.txt
        //   sub/
        //     e.txt
        fs::write(tmp.join("a.txt"), "").unwrap();
        fs::write(tmp.join("b.txt"), "").unwrap();
        fs::write(tmp.join("c.txt"), "").unwrap();
        fs::write(tmp.join("d.txt"), "").unwrap();
        let sub = tmp.join("sub");
        fs::create_dir(&sub).unwrap();
        fs::write(sub.join("e.txt"), "").unwrap();

        let mut files = Vec::new();
        let mut limit = 3;

        collect_files_glob(
            &tmp,
            "*.txt",
            &tmp,
            &[],
            &[],
            None,
            &[],
            &mut limit,
            &mut files,
        );

        // 收集到 limit 个匹配后应立即停止
        assert_eq!(files.len(), 3);
        assert_eq!(limit, 0); // 计数器被完全消耗

        let _ = fs::remove_dir_all(&tmp);
    }

    #[test]
    fn test_collect_files_glob_unlimited() {
        use std::fs;
        let tmp = std::env::temp_dir().join("__jarvis_glob_unlimited_test__");
        let _ = fs::remove_dir_all(&tmp);
        fs::create_dir_all(&tmp).unwrap();

        fs::write(tmp.join("x.txt"), "").unwrap();
        fs::write(tmp.join("y.txt"), "").unwrap();
        fs::write(tmp.join("z.txt"), "").unwrap();

        let mut files = Vec::new();
        let mut limit = usize::MAX; // 无限制模式
        collect_files_glob(
            &tmp,
            "*.txt",
            &tmp,
            &[],
            &[],
            None,
            &[],
            &mut limit,
            &mut files,
        );

        assert_eq!(files.len(), 3);
        // limit 从 usize::MAX 减去 3
        assert_eq!(limit, usize::MAX - 3);

        let _ = fs::remove_dir_all(&tmp);
    }

    #[test]
    fn test_collect_files_glob_matches_directory() {
        use std::fs;
        let tmp = std::env::temp_dir().join("__jarvis_glob_dir_match_test__");
        let _ = fs::remove_dir_all(&tmp);
        fs::create_dir_all(&tmp).unwrap();

        // 创建一个名为 "Desktop" 的子目录
        let desktop = tmp.join("Desktop");
        fs::create_dir(&desktop).unwrap();
        fs::write(desktop.join("notes.txt"), "").unwrap();

        let mut files = Vec::new();
        let mut limit = 10;
        collect_files_glob(
            &tmp,
            "Desktop",
            &tmp,
            &[],
            &[],
            None,
            &[],
            &mut limit,
            &mut files,
        );

        // 应匹配到 Desktop 目录本身（pattern 精确匹配目录名）
        assert!(files.iter().any(|p| p.file_name().unwrap() == "Desktop"));
        // pattern "Desktop" 不应匹配子目录内的文件，需要用 "Desktop/*" 才匹配
        assert!(!files.iter().any(|p| p.file_name().unwrap() == "notes.txt"));

        let _ = fs::remove_dir_all(&tmp);
    }
}
