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
use tauri::Manager;

use super::common::{
    binary_file_read_error, is_locked_file_error, read_text_preserve_encoding, resolve_path,
    MAX_FILE_SIZE_BYTES, MAX_LINES_DEFAULT,
};
use super::workspace::get_workspace;

#[derive(Debug, Clone, PartialEq, Eq)]
struct SkeletonEntry {
    start_line: usize,
    end_line: usize,
    signature: String,
}

fn line_starts_skeleton_entry(path: &str, line: &str) -> bool {
    let ext = std::path::Path::new(path)
        .extension()
        .and_then(|ext| ext.to_str())
        .unwrap_or("")
        .to_lowercase();
    let trimmed = line.trim();
    match ext.as_str() {
        "vue" => trimmed.starts_with("<script")
            || trimmed.starts_with("defineProps(")
            || trimmed.starts_with("defineEmits(")
            || trimmed.starts_with("function ")
            || trimmed.starts_with("const ")
            || trimmed.starts_with("let ")
            || trimmed.starts_with("export default")
            || trimmed.starts_with("export const ")
            || trimmed.contains("defineProps<")
            || trimmed.contains("defineProps(")
            || trimmed.contains("defineEmits<")
            || trimmed.contains("defineEmits("),
        "ts" | "tsx" => trimmed.starts_with("export function ")
            || trimmed.starts_with("export const ")
            || trimmed.starts_with("export class ")
            || trimmed.starts_with("function ")
            || trimmed.starts_with("const ")
            || trimmed.starts_with("let ")
            || trimmed.starts_with("interface ")
            || trimmed.starts_with("type ")
            || trimmed.starts_with("class ")
            || trimmed.starts_with("enum ")
            || trimmed.starts_with("export type "),
        "rs" => trimmed.starts_with("pub fn ")
            || trimmed.starts_with("fn ")
            || trimmed.starts_with("pub struct ")
            || trimmed.starts_with("struct ")
            || trimmed.starts_with("pub enum ")
            || trimmed.starts_with("enum ")
            || trimmed.starts_with("pub trait ")
            || trimmed.starts_with("trait ")
            || trimmed.starts_with("impl ")
            || trimmed.starts_with("mod ")
            || trimmed.starts_with("macro_rules! "),
        _ => trimmed.starts_with("fn ")
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
            || trimmed.starts_with("export "),
    }
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

fn skeleton_entry_end(path: &str, lines: &[&str], start_idx: usize) -> usize {
    let mut balance = 0isize;
    let mut saw_brace = false;
    for (idx, line) in lines.iter().enumerate().skip(start_idx) {
        let delta = brace_delta(line);
        if delta != 0 {
            saw_brace = true;
            balance += delta;
        }
        if saw_brace && balance <= 0 && idx > start_idx {
            return idx;
        }
    }

    let start_indent = leading_indent(lines[start_idx]);
    for (idx, line) in lines.iter().enumerate().skip(start_idx + 1) {
        if line.trim().is_empty() {
            continue;
        }
        if leading_indent(line) <= start_indent && line_starts_skeleton_entry(path, line) {
            return idx.saturating_sub(1);
        }
    }
    start_idx
}

fn compact_signature(lines: &[&str], start_idx: usize, end_idx: usize) -> String {
    let mut parts = Vec::new();
    for line in lines.iter().take(end_idx + 1).skip(start_idx) {
        let trimmed = line.trim();
        if trimmed.is_empty() {
            continue;
        }
        parts.push(trimmed.to_string());
        if trimmed.ends_with('{') || trimmed.ends_with(';') || trimmed.ends_with(')') || trimmed.ends_with("=>") {
            break;
        }
    }
    parts.join(" ")
}

fn extract_skeleton_entries(path: &str, content: &str) -> Vec<SkeletonEntry> {
    let lines: Vec<&str> = content.lines().collect();
    let mut entries = Vec::new();
    for (idx, line) in lines.iter().enumerate() {
        if !line_starts_skeleton_entry(path, line) {
            continue;
        }
        let end_idx = skeleton_entry_end(path, &lines, idx);
        entries.push(SkeletonEntry {
            start_line: idx + 1,
            end_line: end_idx + 1,
            signature: compact_signature(&lines, idx, end_idx),
        });
    }
    entries
}

fn format_skeleton_entry(entry: &SkeletonEntry) -> String {
    format!("[{}-{}] {}", entry.start_line, entry.end_line, entry.signature)
}

fn extract_skeleton_lines(path: &str, content: &str) -> Vec<String> {
    extract_skeleton_entries(path, content)
        .iter()
        .map(format_skeleton_entry)
        .collect()
}

#[cfg(test)]
mod tests {
    use super::{extract_skeleton_entries, extract_skeleton_lines};

    #[test]
    fn extracts_vue_script_setup_entries() {
        let lines = extract_skeleton_lines(
            "Component.vue",
            "<template></template>\n<script setup lang=\"ts\">\nconst count = ref(0)\ndefineProps<{ title: string }>()\n</script>",
        );
        assert!(lines.iter().any(|line| line.contains("<script setup")));
        assert!(lines.iter().any(|line| line.contains("defineProps")));
    }

    #[test]
    fn extracts_typescript_entries() {
        let lines = extract_skeleton_lines(
            "store.ts",
            "import x from 'x'\nexport function load() {}\ninterface State {}\nexport type Mode = 'a'",
        );
        assert!(lines.iter().any(|line| line.contains("[2-2] export function load")));
        assert!(lines.iter().any(|line| line.contains("[3-3] interface State")));
        assert!(lines.iter().any(|line| line.contains("[4-4] export type Mode")));
    }

    #[test]
    fn extracts_multiline_typescript_signature_with_range() {
        let entries = extract_skeleton_entries(
            "store.ts",
            "export function sendMessage(\n  input: string,\n): void {\n  console.log(input)\n}\nconst next = 1",
        );
        assert_eq!(entries[0].start_line, 1);
        assert_eq!(entries[0].end_line, 5);
        assert!(entries[0].signature.contains("sendMessage"));
        assert!(entries[0].signature.contains("input: string"));
    }

    #[test]
    fn extracts_rust_entries() {
        let lines = extract_skeleton_lines(
            "lib.rs",
            "pub trait Tool {}\nimpl Tool for Read {}\nmacro_rules! demo { () => {} }",
        );
        assert!(lines.iter().any(|line| line.contains("pub trait Tool")));
        assert!(lines.iter().any(|line| line.contains("impl Tool")));
        assert!(lines.iter().any(|line| line.contains("macro_rules! demo")));
    }
}
pub async fn read_file(
    app: &tauri::AppHandle,
    input: &serde_json::Value,
    session_id: &str,
) -> String {
    let path = resolve_path(input);
    let start_line = input["start_line"].as_u64().unwrap_or(1) as usize;
    let end_line = input["end_line"].as_u64().unwrap_or(usize::MAX as u64) as usize;

    let ws = get_workspace(app, session_id).await;
    if let Err(e) = ensure_path_permission(app, path, "读取", ws.as_deref()).await {
        return e;
    }

    // 二进制扩展名检查（在读取前拒绝）
    let file_path = std::path::Path::new(path);
    if let Some(err_msg) = binary_file_read_error(file_path) {
        return err_msg;
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

    match read_text_preserve_encoding(path) {
        Ok(decoded) => {
            let content = decoded.content;
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

            // 探索模式拦截：记录文件目录，检测逐文件遍历
            if let Some(manager) = app.try_state::<crate::infra::state::state::SessionManager>() {
                let ctx = manager.get_or_create(session_id).await;
                let dir = std::path::Path::new(path)
                    .parent()
                    .map(|p| p.to_string_lossy().to_string())
                    .unwrap_or_default();
                let mut paths = ctx.read_file_paths.lock().await;
                if !paths.contains(&dir) {
                    paths.push(dir);
                }
                if paths.len() >= 4 {
                    let unique_dirs: std::collections::HashSet<_> = paths.iter().collect();
                    if unique_dirs.len() >= 3 {
                        result.push_str("\n\n💡 提示：你正在逐个读取不同目录下的文件。建议改用 FindFiles 先获取项目文件结构，再用 ReadFile 精准读取目标文件。");
                    }
                }
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
    let path = resolve_path(input);
    let ws = get_workspace(app, session_id).await;
    if let Err(e) = ensure_path_permission(app, path, "读取", ws.as_deref()).await {
        return e;
    }
    let file_path = std::path::Path::new(path);
    if let Some(err_msg) = binary_file_read_error(file_path) {
        return err_msg;
    }
    match read_text_preserve_encoding(path) {
        Ok(decoded) => {
            let content = decoded.content;
            let total_lines = content.lines().count();
            let mut skeleton = format!("[File: {}] (Total: {} lines)\n", path, total_lines);
            let skeleton_lines = extract_skeleton_lines(path, &content);
            if skeleton_lines.is_empty() {
                format!("[File: {}] (Total: {} lines)\n未提取到明显的结构骨架（可能是纯文本或不支持的语言格式）", path, total_lines)
            } else {
                skeleton.push_str(&skeleton_lines.join("\n"));
                skeleton.push('\n');
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
