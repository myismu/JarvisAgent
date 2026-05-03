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

use std::path::Path;

use crate::core::tools::framework::permission::ensure_path_permission;

use super::common::{is_ignored_entry_name, is_search_skipped_extension};
use super::workspace::get_workspace;

/// 在指定目录下搜索关键词（支持正则表达式）
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

    let mut limit = 50;
    let result = search_in_dir(
        &search_dir,
        pattern,
        &mut limit,
        compiled_regex.as_ref(),
        case_insensitive,
    );
    if result.is_empty() {
        format!("未找到包含 '{}' 的内容。", pattern)
    } else {
        result
    }
}

/// 在目录中递归搜索关键词（支持正则表达式）
pub fn search_in_dir(
    dir: &Path,
    pattern: &str,
    limit: &mut usize,
    re: Option<&regex::Regex>,
    case_insensitive: bool,
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
    let pattern_lower = if case_insensitive && re.is_none() {
        pattern.to_lowercase()
    } else {
        String::new()
    };

    for entry in entries.flatten() {
        let path = entry.path();
        let file_name = path.file_name().unwrap_or_default().to_string_lossy();

        if is_ignored_entry_name(&file_name) {
            continue;
        }

        if path.is_dir() {
            result.push_str(&search_in_dir(&path, pattern, limit, re, case_insensitive));
        } else if path.is_file() {
            if let Some(ext) = path.extension().and_then(|s| s.to_str()) {
                if is_search_skipped_extension(ext) {
                    continue;
                }
            }
            if let Ok(content) = std::fs::read_to_string(&path) {
                for (i, line) in content.lines().enumerate() {
                    let matched = if let Some(re) = re {
                        re.is_match(line)
                    } else if case_insensitive {
                        line.to_lowercase().contains(&pattern_lower)
                    } else {
                        line.contains(pattern)
                    };

                    if matched {
                        result.push_str(&format!(
                            "{}:{}: {}\n",
                            path.display(),
                            i + 1,
                            line.trim()
                        ));
                        *limit = limit.saturating_sub(1);
                        if *limit == 0 {
                            result.push_str("... (搜索结果过多，已被截断)\n");
                            return result;
                        }
                    }
                }
            }
        }
    }
    result
}
