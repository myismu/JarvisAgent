//! # file_tools.rs — 文件操作工具模块
//!
//! 提供文件读取、写入、编辑、搜索、目录列表等工具。
//! 所有写操作自动备份原始内容、创建检查点记录和快照，并包含 TOCTOU 防护。
//!
//! ## 关键导出
//! - `read_file()`: 读取文件内容（支持行号范围，超大文件自动截断）
//! - `read_file_skeleton()`: 提取文件结构骨架（函数/类/import 签名）
//! - `write_file()`: 写入文件（自动备份 + 快照 + TOCTOU 防护）
//! - `edit_file()`: 基于搜索替换编辑文件（唯一性检查 + 引号归一化）
//! - `search_repo()`: 在目录下搜索关键词（支持正则）
//! - `list_directory()`: 列出目录内容
//! - `generate_repo_map()`: 生成仓库目录树
//! - `search_in_dir()`: 递归搜索关键词
//!
//! ## 依赖
//! - Internal: `permission::ensure_path_permission`, `session::checkpoint`, `snapshot_engine::Patch`
//! - External: `similar`（diff 计算）, `regex`, `serde_json`, `tauri`
//!
//! ## 约束
//! - 文件大小限制 256KB，输出行数限制 2000 行
//! - edit_file 要求 old_text 唯一匹配，否则返回上下文帮助 LLM 自我修正
//! - 沙箱会话下所有路径操作受沙箱边界限制

use std::path::Path;
use serde_json::json;
use tauri::{Emitter, Manager};

use super::permission::ensure_path_permission;
use crate::core::session::checkpoint::{self, FileOperation, OpType};
use crate::core::snapshot_engine::Patch;
use crate::core::tools::registry::ToolDef;
use crate::core::SnapshotRegistry;

/// 文件大小限制：超过此大小拒绝读取（256KB）
const MAX_FILE_SIZE_BYTES: u64 = 256 * 1024;
/// 输出行数限制：超过此行数自动截断
const MAX_LINES_DEFAULT: usize = 2000;

/// 归一化弯引号为直引号（LLM 可能输出直引号而文件使用弯引号，用于匹配比较）
fn normalize_quotes(s: &str) -> String {
    s.replace('\u{201C}', "\"").replace('\u{201D}', "\"")   // 中文双弯引号 ""
     .replace('\u{2018}', "'").replace('\u{2019}', "'")     // 中文单弯引号 ''
     .replace('\u{FF02}', "\"")                             // 全角双引号
     .replace('\u{FF07}', "'")                              // 全角单引号
}

/// 统一换行符为 LF（写入文件前调用）
fn normalize_line_endings(content: &str) -> String {
    content.replace("\r\n", "\n").replace('\r', "\n")
}

/// 计算两个文本之间的 diff（用于快照系统）
fn compute_diff(old_text: &str, new_text: &str) -> crate::core::snapshot_engine::patch::TextDiff {
    use similar::{ChangeTag, TextDiff as SimilarDiff};
    use crate::core::snapshot_engine::patch::{TextDiff, DiffHunk, DiffLine};

    let diff = SimilarDiff::from_lines(old_text, new_text);
    let mut hunks = Vec::new();

    for op in diff.ops() {
        let old_start = op.old_range().start as u32;
        let new_start = op.new_range().start as u32;
        let old_len = op.old_range().len() as u32;
        let new_len = op.new_range().len() as u32;

        let mut lines = Vec::new();
        for change in diff.iter_changes(op) {
            let content = change.to_string();
            match change.tag() {
                ChangeTag::Equal => lines.push(DiffLine::Context { content }),
                ChangeTag::Insert => lines.push(DiffLine::Addition { content }),
                ChangeTag::Delete => lines.push(DiffLine::Deletion { content }),
            }
        }

        if !lines.is_empty() {
            hunks.push(DiffHunk {
                old_start,
                old_lines: old_len,
                new_start,
                new_lines: new_len,
                lines,
            });
        }
    }

    TextDiff { hunks }
}

/// 获取当前会话的工作目录沙箱
async fn get_workspace(app: &tauri::AppHandle, session_id: &str) -> Option<std::path::PathBuf> {
    if let Some(manager) = app.try_state::<crate::core::state::SessionManager>() {
        let ctx = manager.get_or_create(session_id).await;
        let ws = ctx.workspace.lock().await.clone();
        return ws;
    }
    None
}

async fn record_operation(app: &tauri::AppHandle, session_id: &str, operation: FileOperation) {
    if let Some(manager) = app.try_state::<crate::core::state::SessionManager>() {
        let ctx = manager.get_or_create(session_id).await;
        let mut ops = ctx.pending_checkpoint.lock().await;
        ops.push(operation);
    }
}

async fn record_patch_to_snapshot(app: &tauri::AppHandle, session_id: &str, patch: Patch, message: Option<String>) {
    let sid = session_id;
    if let Some(registry) = app.try_state::<SnapshotRegistry>() {
        let mgr_result = registry.0.read().await.get_or_create(&sid).await;
        if let Ok(mgr) = mgr_result {
            let result = mgr.create_snapshot(vec![patch], message, None, None, None).await;
            if let Ok(snapshot) = result {
                let _ = app.emit("snapshot-created", serde_json::json!({
                    "sessionId": sid,
                    "snapshotId": snapshot.id
                }));
            }
        }
    }
}

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
            if err_msg.contains("Access is denied") || err_msg.contains("os error 32") || err_msg.contains("os error 5") || err_msg.contains("being used by another process") {
                format!("读取错误: 文件可能被其他智能体或程序锁定，请稍后重试。详细错误: {}", e)
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
            if err_msg.contains("Access is denied") || err_msg.contains("os error 32") || err_msg.contains("os error 5") || err_msg.contains("being used by another process") {
                format!("读取错误: 文件可能被其他智能体或程序锁定，请稍后重试。详细错误: {}", e)
            } else {
                format!("读取错误: {}", e)
            }
        }
    }
}

/// 写入文件（自动备份原始内容 + 自动创建快照）
pub async fn write_file(
    app: &tauri::AppHandle,
    input: &serde_json::Value,
    session_id: &str,
) -> String {
    let path = input["path"].as_str().unwrap_or("");
    let content = input["content"].as_str().unwrap_or("");
    // 统一行尾为 LF，避免 CRLF/LF 混乱
    let content = normalize_line_endings(content);
    let ws = get_workspace(app, session_id).await;
    if let Err(e) = ensure_path_permission(app, path, "写入", ws.as_deref()).await {
        return e;
    }

    let branch = checkpoint::get_active_branch(session_id);
    let branch_name = branch.name;

    let file_exists = std::path::Path::new(path).exists();
    let old_content = if file_exists {
        std::fs::read_to_string(path).ok()
    } else {
        None
    };

    // TOCTOU 防护：记录读取时的 mtime
    let read_mtime = if file_exists {
        std::fs::metadata(path).ok().and_then(|m| m.modified().ok())
    } else {
        None
    };

    let old_content_bytes = old_content.as_ref().map(|s| s.as_bytes().to_vec());
    let old_content_hash = old_content_bytes.as_ref().map(|c| checkpoint::content_hash(c));
    let backup_path = if let Some(ref content_bytes) = old_content_bytes {
        checkpoint::backup_file(session_id, &branch_name, path, content_bytes)
    } else {
        None
    };

    let new_content_hash = Some(checkpoint::content_hash(content.as_bytes()));

    let op_type = if file_exists { OpType::Write } else { OpType::Create };

    let operation = FileOperation {
        op_type,
        path: path.to_string(),
        old_content_hash,
        backup_path,
        new_content_hash,
        diff_summary: None,
    };

    // TOCTOU 防护：写入前检查文件是否在读取后被外部修改
    if let (Some(orig_mtime), Ok(current_meta)) = (read_mtime, std::fs::metadata(path)) {
        if let Ok(current_mtime) = current_meta.modified() {
            if current_mtime != orig_mtime {
                return format!("写入中止: 文件 {} 在读取后被外部修改。请重新读取后再写入。", path);
            }
        }
    }

    match std::fs::write(path, content.as_str()) {
        Ok(_) => {
            record_operation(app, session_id, operation).await;
            
            let patch = match &old_content {
                None => Patch::CreateFile {
                    path: path.to_string(),
                    content: content.to_string(),
                },
                Some(old) => Patch::UpdateFile {
                    path: path.to_string(),
                    old_content: old.clone(),
                    new_content: content.to_string(),
                    diff: Some(compute_diff(old, &content)),
                },
            };
            let action = if file_exists { "写入" } else { "创建" };
            let msg = Some(format!("{} {}", action, path));
            record_patch_to_snapshot(app, session_id, patch, msg).await;
            
            format!("成功{} {}", action, path)
        }
        Err(e) => {
            let err_msg = e.to_string();
            if err_msg.contains("Access is denied") || err_msg.contains("os error 32") || err_msg.contains("os error 5") || err_msg.contains("being used by another process") {
                format!("写入失败: 文件被其他智能体或程序锁定，请稍后重试。详细错误: {}", e)
            } else {
                format!("写入失败: {}", e)
            }
        }
    }
}

/// 基于搜索替换编辑文件（唯一性检查 + 引号归一化 + TOCTOU 防护 + 自动快照）
pub async fn edit_file(
    app: &tauri::AppHandle,
    input: &serde_json::Value,
    session_id: &str,
) -> String {
    let path = input["path"].as_str().unwrap_or("");
    let old_text = input["old_text"].as_str().unwrap_or("");
    let new_text = normalize_line_endings(input["new_text"].as_str().unwrap_or(""));
    let ws = get_workspace(app, session_id).await;
    if let Err(e) = ensure_path_permission(app, path, "编辑", ws.as_deref()).await {
        return e;
    }
    
    let branch = checkpoint::get_active_branch(session_id);
    let branch_name = branch.name;
    
    // 记录读取时的 mtime，用于 TOCTOU 防护
    let read_mtime = std::fs::metadata(path).ok().and_then(|m| m.modified().ok());

    match std::fs::read_to_string(path) {
        Ok(content) => {
            // 唯一性检查：统计 old_text 在文件中的匹配次数
            let match_count = content.matches(old_text).count();

            if match_count == 0 {
                // 尝试引号归一化匹配（LLM 可能输出直引号而文件使用弯引号）
                let normalized_content = normalize_quotes(&content);
                let normalized_old = normalize_quotes(old_text);
                if normalized_content.contains(&normalized_old) {
                    return format!(
                        "编辑失败: 未在 {} 中找到精确匹配的旧文本块，但发现引号风格不一致。\n文件使用弯引号，请使用相同的引号风格重试。",
                        path
                    );
                }
                return format!("编辑失败: 未在 {} 中找到指定的旧文本块。\n请使用 read_file 先查看文件的实际内容", path);
            }

            if match_count > 1 {
                // 返回每个匹配位置的行号 + 前后 2 行上下文，帮助 LLM 自我修正
                let mut context_msg = format!("编辑失败: 旧文本在 {} 中匹配了 {} 处，请提供更多上下文使其唯一。\n\n", path, match_count);
                let lines: Vec<&str> = content.lines().collect();
                let old_lines_count = old_text.lines().count();
                let mut search_from = 0;
                for idx in 0..match_count {
                    if let Some(pos) = content[search_from..].find(old_text) {
                        let absolute_pos = search_from + pos;
                        let line_num = content[..absolute_pos].lines().count();
                        let ctx_start = line_num.saturating_sub(2);
                        let ctx_end = (line_num + old_lines_count + 2).min(lines.len());
                        context_msg.push_str(&format!("--- 匹配 {} (第 {} 行附近) ---\n", idx + 1, line_num + 1));
                        for (i, line) in lines[ctx_start..ctx_end].iter().enumerate() {
                            let ln = ctx_start + i + 1;
                            let marker = if ln >= line_num + 1 && ln <= line_num + old_lines_count { ">>>" } else { "   " };
                            context_msg.push_str(&format!("{} {:4} | {}\n", marker, ln, line));
                        }
                        context_msg.push('\n');
                        search_from = absolute_pos + old_text.len();
                    }
                }
                context_msg.push_str("请提供更多上下文使 old_text 唯一。");
                return context_msg;
            }

            // match_count == 1，安全替换
            let old_content_bytes = content.as_bytes().to_vec();
            let old_content_hash = Some(checkpoint::content_hash(&old_content_bytes));

            let backup_path = checkpoint::backup_file(session_id, &branch_name, path, &old_content_bytes);

            let updated_content = content.replacen(old_text, &new_text, 1);
            let new_content_hash = Some(checkpoint::content_hash(updated_content.as_bytes()));

            let diff_summary = Some(format!(
                "替换: \"{}\" -> \"{}\"",
                if old_text.len() > 50 { old_text.chars().take(50).collect::<String>() } else { old_text.to_string() },
                if new_text.len() > 50 { new_text.chars().take(50).collect::<String>() } else { new_text.to_string() }
            ));

            let operation = FileOperation {
                op_type: OpType::Edit,
                path: path.to_string(),
                old_content_hash,
                backup_path,
                new_content_hash,
                diff_summary,
            };

            // TOCTOU 防护：写入前检查文件是否在读取后被外部修改
            if let (Some(orig_mtime), Ok(current_meta)) = (read_mtime, std::fs::metadata(path)) {
                if let Ok(current_mtime) = current_meta.modified() {
                    if current_mtime != orig_mtime {
                        return format!("编辑中止: 文件 {} 在读取后被外部修改。请重新读取后再编辑。", path);
                    }
                }
            }

            match std::fs::write(path, &updated_content) {
                Ok(_) => {
                    record_operation(app, session_id, operation).await;

                    let patch = Patch::UpdateFile {
                        path: path.to_string(),
                        old_content: content.clone(),
                        new_content: updated_content.clone(),
                        diff: Some(compute_diff(&content, &updated_content)),
                    };
                    let msg = Some(format!("编辑 {}", path));
                    record_patch_to_snapshot(app, session_id, patch, msg).await;

                    format!("成功编辑 {}", path)
                }
                Err(e) => {
                    let err_msg = e.to_string();
                    if err_msg.contains("Access is denied") || err_msg.contains("os error 32") || err_msg.contains("os error 5") || err_msg.contains("being used by another process") {
                        format!("编辑并保存失败: 文件被其他智能体或程序锁定，请稍后重试。详细错误: {}", e)
                    } else {
                        format!("编辑并保存失败: {}", e)
                    }
                }
            }
        }
        Err(e) => {
            let err_msg = e.to_string();
            if err_msg.contains("Access is denied") || err_msg.contains("os error 32") || err_msg.contains("os error 5") || err_msg.contains("being used by another process") {
                format!("编辑失败: 文件可能被其他智能体或程序锁定，请稍后重试。详细错误: {}", e)
            } else {
                format!("编辑失败，无法读取文件: {}", e)
            }
        }
    }
}

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
    let result = search_in_dir(&search_dir, pattern, &mut limit, compiled_regex.as_ref(), case_insensitive);
    if result.is_empty() {
        format!("未找到包含 '{}' 的内容。", pattern)
    } else {
        result
    }
}

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
            for entry in entries {
                if let Ok(e) = entry {
                    let file_name = e.file_name().to_string_lossy().to_string();
                    let file_type = if e.path().is_dir() { "[DIR]" } else { "[FILE]" };
                    result.push_str(&format!("{} {}\n", file_type, file_name));
                }
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
            .filter(|e| {
                let path = e.path();
                let file_name = path.file_name().unwrap_or_default().to_string_lossy();
                if file_name == "node_modules"
                    || file_name == "target"
                    || file_name == "dist"
                    || file_name.starts_with('.')
                {
                    return false;
                }
                if path.is_file() {
                    if let Some(ext) = path.extension().and_then(|s| s.to_str()) {
                        let ext_lower = ext.to_lowercase();
                        if [
                            "png", "ico", "icns", "jpg", "jpeg", "gif", "svg", "webp", "mp3",
                            "mp4", "wav", "woff", "woff2", "ttf", "eot",
                        ]
                        .contains(&ext_lower.as_str())
                        {
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

        if file_name == "node_modules"
            || file_name == "target"
            || file_name == "dist"
            || file_name.starts_with('.')
        {
            continue;
        }

        if path.is_dir() {
            result.push_str(&search_in_dir(&path, pattern, limit, re, case_insensitive));
        } else if path.is_file() {
            if let Some(ext) = path.extension().and_then(|s| s.to_str()) {
                let ext_lower = ext.to_lowercase();
                if [
                    "png", "ico", "icns", "jpg", "jpeg", "gif", "svg", "webp", "mp3", "mp4", "wav",
                    "woff", "woff2", "ttf", "eot", "pdf", "zip",
                ]
                .contains(&ext_lower.as_str())
                {
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

// --- 工具注册 ---
crate::define_tools! {
    pub fn register_tools(registry) {
        ToolDef {
            name: "read_file",
            description: "读取文件内容，支持按行号精确读取",
            search_hint: "read file content view",
            schema: json!({
                "name": "read_file",
                "description": "读取文件内容。支持语义化点读技术，可通过 start_line 和 end_line 获取特定代码块，避免 Context 过长。",
                "input_schema": {
                    "type": "object",
                    "properties": {
                        "path": {"type": "string"},
                        "start_line": {"type": "integer", "description": "可选。起始行号（从 1 开始）"},
                        "end_line": {"type": "integer", "description": "可选。结束行号（包含）"}
                    },
                    "required": ["path"]
                }
            }),
            should_defer: true,
            is_read_only: true,
            is_concurrency_safe: true,
            is_enabled: true,
        },
        ToolDef {
            name: "read_file_skeleton",
            description: "提取文件结构骨架（类、函数签名及行号）",
            search_hint: "skeleton structure class function signature",
            schema: json!({
                "name": "read_file_skeleton",
                "description": "提取文件结构骨架（Skeleton）。快速扫描并返回文件的类、函数签名及其行号，结合 read_file 进行精确片段阅读。",
                "input_schema": {
                    "type": "object",
                    "properties": { "path": {"type": "string"} },
                    "required": ["path"]
                }
            }),
            should_defer: true,
            is_read_only: true,
            is_concurrency_safe: true,
            is_enabled: true,
        },
        ToolDef {
            name: "write_file",
            description: "写入文件内容",
            search_hint: "write file create new",
            schema: json!({
                "name": "write_file",
                "description": "写入文件内容。",
                "input_schema": {
                    "type": "object",
                    "properties": {
                        "path": {"type": "string"},
                        "content": {"type": "string"}
                    },
                    "required": ["path", "content"]
                }
            }),
            should_defer: true,
            is_read_only: false,
            is_concurrency_safe: false,
            is_enabled: true,
        },
        ToolDef {
            name: "edit_file",
            description: "基于搜索与替换修改文件中的特定文本",
            search_hint: "edit file replace search modify",
            schema: json!({
                "name": "edit_file",
                "description": "基于搜索与替换修改文件中的特定文本片段。",
                "input_schema": {
                    "type": "object",
                    "properties": {
                        "path": {"type": "string"},
                        "old_text": {"type": "string", "description": "要替换的确切旧文本内容"},
                        "new_text": {"type": "string", "description": "替换后的新文本内容"}
                    },
                    "required": ["path", "old_text", "new_text"]
                }
            }),
            should_defer: true,
            is_read_only: false,
            is_concurrency_safe: false,
            is_enabled: true,
        },
        ToolDef {
            name: "search_repo",
            description: "在指定目录下全局搜索包含关键词的文本",
            search_hint: "search find grep text pattern",
            schema: json!({
                "name": "search_repo",
                "description": "在指定目录下全局搜索包含特定关键词或正则表达式的文本内容。自动忽略编译产物和静态资源。",
                "input_schema": {
                    "type": "object",
                    "properties": {
                        "pattern": {"type": "string", "description": "要搜索的关键词或正则表达式"},
                        "dir": {"type": "string", "description": "要搜索的目录路径，默认搜索整个项目根目录"},
                        "regex": {"type": "boolean", "description": "是否将 pattern 作为正则表达式处理，默认 false"},
                        "case_insensitive": {"type": "boolean", "description": "是否忽略大小写，默认 false"}
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
            name: "list_directory",
            description: "列出指定目录下的所有文件和文件夹",
            search_hint: "list directory folder files ls",
            schema: json!({
                "name": "list_directory",
                "description": "列出指定目录下的所有文件和文件夹。",
                "input_schema": {
                    "type": "object",
                    "properties": {
                        "path": {"type": "string", "description": "目录路径"}
                    },
                    "required": ["path"]
                }
            }),
            should_defer: true,
            is_read_only: true,
            is_concurrency_safe: true,
            is_enabled: true,
        }
    }
}
