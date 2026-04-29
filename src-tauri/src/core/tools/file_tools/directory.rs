//! # directory.rs — 提供目录列表与仓库结构树生成
//!
//! 实现 agent 可调用的目录查看工具，并为上下文构建生成过滤后的仓库树，自动跳过隐藏目录、构建产物和静态资源。
//!
//! ## Key Exports
//! - `list_directory()`: 列出指定目录下的文件和子目录
//! - `generate_repo_map()`: 递归生成仓库目录树文本
//!
//! ## Dependencies
//! - Internal: `crate::core::tools::permission`, `super::workspace`, `super::common`

use std::path::Path;

use crate::core::tools::permission::ensure_path_permission;

use super::common::{is_ignored_entry_name, is_static_asset_extension};
use super::workspace::get_workspace;

/// 列出目录内容
pub async fn list_directory(
    app: &tauri::AppHandle,
    input: &serde_json::Value,
    session_id: &str,
) -> String {
    let path_str = input["path"].as_str().unwrap_or(".");
    let ws = get_workspace(app, session_id).await;
    if let Err(e) = ensure_path_permission(app, path_str, "列出", ws.as_deref()).await {
        return e;
    }
    match std::fs::read_dir(path_str) {
        Ok(entries) => {
            let mut result = String::new();
            for entry in entries.flatten() {
                let file_name = entry.file_name().to_string_lossy().to_string();
                let file_type = if entry.path().is_dir() {
                    "[DIR]"
                } else {
                    "[FILE]"
                };
                result.push_str(&format!("{} {}\n", file_type, file_name));
            }
            if result.is_empty() {
                "目录为空".to_string()
            } else {
                result
            }
        }
        Err(e) => format!("读取目录失败: {}", e),
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
