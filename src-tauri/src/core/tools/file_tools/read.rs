//! # read.rs — 提供受权限控制的文件读取与结构骨架提取工具
//!
//! 实现 agent 可调用的只读文件工具，支持按行范围读取、超大文件截断提示、文件锁错误识别，以及面向代码导航的结构骨架提取。
//!
//! ## Key Exports
//! - `read_file()`: 读取文件内容并按行号范围格式化输出
//! - `read_file_skeleton()`: 提取 import、类型、函数等结构签名
//!
//! ## Dependencies
//! - Internal: `crate::core::tools::framework::permission`, `super::workspace`, `super::common`

use crate::core::tools::framework::permission::ensure_path_permission;

use super::common::{is_locked_file_error, MAX_FILE_SIZE_BYTES, MAX_LINES_DEFAULT};
use super::workspace::get_workspace;

/// 读取文件内容（支持行号范围）
pub async fn read_file(
    app: &tauri::AppHandle,
    input: &serde_json::Value,
    session_id: &str,
) -> String {
    let path = input["path"].as_str().unwrap_or("");
    let start_line = input["start_line"].as_u64().unwrap_or(1) as usize;
    let end_line = input["end_line"].as_u64().unwrap_or(usize::MAX as u64) as usize;

    let ws = get_workspace(app, session_id).await;
    if let Err(e) = ensure_path_permission(app, path, "读取", ws.as_deref()).await {
        return e;
    }

    // 文件大小限制检查
    if let Ok(meta) = std::fs::metadata(path) {
        if meta.len() > MAX_FILE_SIZE_BYTES {
            return format!(
                "读取错误: 文件 {} 过大 ({} bytes)，超过限制 {} bytes。\n请使用 start_line/end_line 参数分段读取。",
                path, meta.len(), MAX_FILE_SIZE_BYTES
            );
        }
    }

    match std::fs::read_to_string(path) {
        Ok(content) => {
            let lines: Vec<&str> = content.lines().collect();
            let total_lines = lines.len();
            let actual_start = start_line.max(1);
            let actual_end = if end_line == usize::MAX {
                total_lines
            } else {
                end_line.min(total_lines)
            };
            let start_idx = actual_start.saturating_sub(1);

            if start_idx >= total_lines {
                return format!("起始行 {} 超过文件总行数 {}", start_line, total_lines);
            }

            // 输出行数截断
            let display_end = if actual_end - start_idx > MAX_LINES_DEFAULT {
                start_idx + MAX_LINES_DEFAULT
            } else {
                actual_end
            };
            let truncated = display_end < actual_end;

            let mut result = format!("[File: {}] (Total: {} lines", path, total_lines);
            if actual_start > 1 || actual_end < total_lines || truncated {
                result.push_str(&format!(", Showing: {}-{}", actual_start, display_end));
            }
            result.push_str(")\n");

            for (i, line) in lines[start_idx..display_end].iter().enumerate() {
                let line_num = start_idx + i + 1;
                result.push_str(&format!("{:4} | {}\n", line_num, line));
            }

            if truncated {
                result.push_str(&format!(
                    "\n... 输出已截断（显示 {} 行，总共 {} 行）。请使用 start_line={}/end_line={} 继续读取。",
                    MAX_LINES_DEFAULT, actual_end - start_idx, display_end + 1, actual_end
                ));
            }

            result
        }
        Err(e) => {
            let err_msg = e.to_string();
            if is_locked_file_error(&err_msg) {
                format!(
                    "读取错误: 文件可能被其他智能体或程序锁定，请稍后重试。详细错误: {}",
                    e
                )
            } else {
                format!("读取错误: {}", e)
            }
        }
    }
}

/// 提取文件结构骨架（函数/类/import 签名）
pub async fn read_file_skeleton(
    app: &tauri::AppHandle,
    input: &serde_json::Value,
    session_id: &str,
) -> String {
    let path = input["path"].as_str().unwrap_or("");
    let ws = get_workspace(app, session_id).await;
    if let Err(e) = ensure_path_permission(app, path, "读取", ws.as_deref()).await {
        return e;
    }
    match std::fs::read_to_string(path) {
        Ok(content) => {
            let total_lines = content.lines().count();
            let mut skeleton = format!("[File: {}] (Total: {} lines)\n", path, total_lines);
            let mut found = false;
            for (i, line) in content.lines().enumerate() {
                let trimmed = line.trim();
                if trimmed.starts_with("fn ")
                    || trimmed.starts_with("pub fn ")
                    || trimmed.starts_with("struct ")
                    || trimmed.starts_with("pub struct ")
                    || trimmed.starts_with("class ")
                    || trimmed.starts_with("def ")
                    || trimmed.starts_with("import ")
                    || trimmed.starts_with("use ")
                    || trimmed.starts_with("impl ")
                    || trimmed.starts_with("interface ")
                    || trimmed.starts_with("type ")
                    || trimmed.starts_with("export ")
                {
                    skeleton.push_str(&format!("{:4} | {}\n", i + 1, line));
                    found = true;
                }
            }
            if !found {
                format!("[File: {}] (Total: {} lines)\n未提取到明显的结构骨架（可能是纯文本或不支持的语言格式）", path, total_lines)
            } else {
                skeleton
            }
        }
        Err(e) => {
            let err_msg = e.to_string();
            if is_locked_file_error(&err_msg) {
                format!(
                    "读取错误: 文件可能被其他智能体或程序锁定，请稍后重试。详细错误: {}",
                    e
                )
            } else {
                format!("读取错误: {}", e)
            }
        }
    }
}
