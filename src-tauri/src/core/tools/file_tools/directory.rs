//! # directory.rs — 提供目录列表与仓库结构树生成
//!
//! 实现 agent 可调用的目录查看工具，并为上下文构建生成过滤后的仓库树，自动跳过隐藏目录、构建产物和静态资源。
//!
//! ## Key Exports
//! - `list_directory()`: 列出目录当前一层的内容（目录在前、按名排序，跳过依赖/构建目录，超上限截断）
//! - `generate_repo_map()`: 递归生成仓库目录树文本
//!
//! ## Dependencies
//! - Internal: `crate::core::tools::framework::permission`, `super::workspace`, `super::common`

use std::path::Path;

use crate::core::tools::framework;
use crate::core::tools::framework::permission::ensure_path_permission;

use super::common::{is_ignored_entry_name, is_static_asset_extension};
use super::workspace::{get_workspace, resolve_exec_path, sandbox_missing_hint};

/// 单层列目录最多输出的条目数：超出时截断并明确告知，防止 node_modules 这类目录刷屏。
const MAX_DIRECTORY_ENTRIES: usize = 200;

/// 依赖/构建产物目录名：单层列出时默认跳过。
///
/// 刻意**不**复用 `is_ignored_entry_name` 的 dot 规则：`.env` / `.vscode` 这类隐藏
/// 配置恰恰是模型常要确认的对象，单层列出时不该被吞掉；node_modules / target / dist
/// 对"目录里有什么"没有信息量，逐个列出来只会挤占输出。跳过与截断都会写进输出，
/// 不做静默过滤——目录工具谎报内容比刷屏更糟。
const SKIPPED_DIR_NAMES: &[&str] = &["node_modules", "target", "dist"];

/// 组装单层目录的输出文本（纯函数，便于单测）：
/// 目录在前、各自按名称排序；超过上限截断并给出 FindFiles 指引。
fn format_directory_listing(mut entries: Vec<(bool, String)>, skipped_dirs: usize) -> String {
    entries.sort_by(|a, b| b.0.cmp(&a.0).then_with(|| a.1.cmp(&b.1)));
    let total = entries.len();
    let mut out = String::new();
    for (is_dir, name) in entries.iter().take(MAX_DIRECTORY_ENTRIES) {
        out.push_str(if *is_dir { "[DIR] " } else { "[FILE] " });
        out.push_str(name);
        out.push('\n');
    }
    if total > MAX_DIRECTORY_ENTRIES {
        out.push_str(&format!(
            "…（已截断：共 {} 项，仅显示前 {} 项。按名称/扩展名筛选请用 FindFiles）\n",
            total, MAX_DIRECTORY_ENTRIES
        ));
    }
    if skipped_dirs > 0 {
        out.push_str(&format!(
            "（已跳过 {} 个依赖/构建目录：node_modules、target、dist）\n",
            skipped_dirs
        ));
    }
    if out.is_empty() {
        "目录为空".to_string()
    } else {
        out
    }
}

/// 列出目录内容
pub async fn list_directory(
    app: &tauri::AppHandle,
    input: &serde_json::Value,
    session_id: &str,
) -> framework::ToolCallResult {
    let path_str = input["path"].as_str().unwrap_or(".");
    let ws = get_workspace(app, session_id).await;
    if let Err(e) = ensure_path_permission(app, path_str, "列出", ws.as_deref()).await {
        return framework::ToolCallResult::error(e);
    }
    // 执行层与校验层对齐：相对路径按沙箱目录解析（而非进程 CWD），否则会越界列出目录
    let exec_path = resolve_exec_path(path_str, ws.as_deref());
    match std::fs::read_dir(&exec_path) {
        Ok(entries) => {
            let mut listing: Vec<(bool, String)> = Vec::new();
            let mut skipped_dirs = 0usize;
            for entry in entries.flatten() {
                let file_name = entry.file_name().to_string_lossy().to_string();
                let is_dir = entry.path().is_dir();
                if is_dir && SKIPPED_DIR_NAMES.contains(&file_name.as_str()) {
                    skipped_dirs += 1;
                    continue;
                }
                listing.push((is_dir, file_name));
            }
            framework::ToolCallResult::ok(format_directory_listing(listing, skipped_dirs))
        }
        Err(e) => framework::ToolCallResult::error(format!(
            "读取目录失败: {}{}",
            e,
            sandbox_missing_hint(ws.as_deref())
        )),
    }
}

/// 生成仓库目录树
pub fn generate_repo_map(dir: &Path, prefix: &str, depth: usize, max_depth: usize) -> String {
    if depth > max_depth {
        return format!("{}...\n", prefix);
    }

    let mut result = String::new();
    let mut entries: Vec<_> = match std::fs::read_dir(dir) {
        Ok(iter) => iter
            .filter_map(Result::ok)
            .filter(|entry| {
                let path = entry.path();
                let file_name = path.file_name().unwrap_or_default().to_string_lossy();
                if is_ignored_entry_name(&file_name) {
                    return false;
                }
                if path.is_file() {
                    if let Some(ext) = path.extension().and_then(|s| s.to_str()) {
                        if is_static_asset_extension(ext) {
                            return false;
                        }
                    }
                }
                true
            })
            .collect(),
        Err(_) => return result,
    };

    entries.sort_by_key(|e| e.path());

    for (i, entry) in entries.iter().enumerate() {
        let path = entry.path();
        let file_name = path
            .file_name()
            .unwrap_or_default()
            .to_string_lossy()
            .into_owned();

        let is_last = i == entries.len() - 1;
        let connector = if is_last { "└── " } else { "├── " };
        result.push_str(&format!("{}{}{}\n", prefix, connector, file_name));

        if path.is_dir() {
            let new_prefix = if is_last {
                format!("{}    ", prefix)
            } else {
                format!("{}│   ", prefix)
            };
            result.push_str(&generate_repo_map(&path, &new_prefix, depth + 1, max_depth));
        }
    }
    result
}

#[cfg(test)]
mod tests {
    //! `format_directory_listing` 的行为锁：排序、跳过依赖/构建目录、截断提示。
    //! 这些规则直接决定模型看到的目录视图，改动时必须有测试兜着。

    use super::{format_directory_listing, MAX_DIRECTORY_ENTRIES};

    fn entry(is_dir: bool, name: &str) -> (bool, String) {
        (is_dir, name.to_string())
    }

    #[test]
    fn dirs_come_first_then_alphabetical() {
        let out = format_directory_listing(
            vec![
                entry(false, "b.txt"),
                entry(true, "zdir"),
                entry(false, "a.txt"),
                entry(true, "adir"),
            ],
            0,
        );
        let lines: Vec<&str> = out.lines().collect();
        assert_eq!(
            lines,
            vec!["[DIR] adir", "[DIR] zdir", "[FILE] a.txt", "[FILE] b.txt"]
        );
    }

    #[test]
    fn caps_entries_and_explains_truncation() {
        let entries: Vec<(bool, String)> = (0..MAX_DIRECTORY_ENTRIES + 5)
            .map(|i| entry(false, &format!("f{:03}.txt", i)))
            .collect();
        let out = format_directory_listing(entries, 0);
        let item_lines = out.lines().filter(|l| l.starts_with("[FILE]")).count();
        assert_eq!(item_lines, MAX_DIRECTORY_ENTRIES, "只输出前 200 项");
        assert!(
            out.contains(&format!("已截断：共 {} 项", MAX_DIRECTORY_ENTRIES + 5)),
            "截断数量要如实告知"
        );
        assert!(out.contains("FindFiles"), "截断时要给出下一步指引");
    }

    #[test]
    fn skipped_dirs_are_reported_not_silent() {
        let out = format_directory_listing(vec![], 2);
        assert!(out.contains("已跳过 2 个依赖/构建目录"));
        assert!(
            !out.contains("目录为空"),
            "跳过了内容就不能说目录为空，要说明跳过了什么"
        );
    }

    #[test]
    fn truly_empty_directory_says_so() {
        assert_eq!(format_directory_listing(vec![], 0), "目录为空");
    }
}
