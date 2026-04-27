use std::path::Path;
use tauri::{Emitter, Manager};

use super::permission::ensure_path_permission;
use crate::core::checkpoint::{self, FileOperation, OpType};
use crate::core::snapshot_engine::Patch;
use crate::core::SnapshotRegistry;

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

            let mut result = format!("[File: {}] (Total: {} lines", path, total_lines);
            if actual_start > 1 || actual_end < total_lines {
                result.push_str(&format!(", Showing: {}-{}", actual_start, actual_end));
            }
            result.push_str(")\n");

            for (i, line) in lines[start_idx..actual_end].iter().enumerate() {
                let line_num = start_idx + i + 1;
                result.push_str(&format!("{:4} | {}\n", line_num, line));
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
    
    match std::fs::write(path, content) {
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
                    diff: None,
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

/// 基于搜索替换编辑文件（自动备份原始内容 + 自动创建快照）
pub async fn edit_file(
    app: &tauri::AppHandle,
    input: &serde_json::Value,
    session_id: &str,
) -> String {
    let path = input["path"].as_str().unwrap_or("");
    let old_text = input["old_text"].as_str().unwrap_or("");
    let new_text = input["new_text"].as_str().unwrap_or("");
    let ws = get_workspace(app, session_id).await;
    if let Err(e) = ensure_path_permission(app, path, "编辑", ws.as_deref()).await {
        return e;
    }
    
    let branch = checkpoint::get_active_branch(session_id);
    let branch_name = branch.name;
    
    match std::fs::read_to_string(path) {
        Ok(content) => {
            if !content.contains(old_text) {
                format!("编辑失败: 未在 {} 中找到指定的旧文本块。", path)
            } else {
                let old_content_bytes = content.as_bytes().to_vec();
                let old_content_hash = Some(checkpoint::content_hash(&old_content_bytes));
                
                let backup_path = checkpoint::backup_file(session_id, &branch_name, path, &old_content_bytes);
                
                let updated_content = content.replacen(old_text, new_text, 1);
                let new_content_hash = Some(checkpoint::content_hash(updated_content.as_bytes()));
                
                let diff_summary = Some(format!(
                    "替换: \"{}\" -> \"{}\"",
                    if old_text.len() > 50 { &old_text[..50] } else { old_text },
                    if new_text.len() > 50 { &new_text[..50] } else { new_text }
                ));
                
                let operation = FileOperation {
                    op_type: OpType::Edit,
                    path: path.to_string(),
                    old_content_hash,
                    backup_path,
                    new_content_hash,
                    diff_summary,
                };
                
                match std::fs::write(path, &updated_content) {
                    Ok(_) => {
                        record_operation(app, session_id, operation).await;
                        
                        let patch = Patch::UpdateFile {
                            path: path.to_string(),
                            old_content: content.clone(),
                            new_content: updated_content,
                            diff: None,
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

/// 在指定目录下搜索关键词
pub async fn search_repo(
    app: &tauri::AppHandle,
    input: &serde_json::Value,
    session_id: &str,
) -> String {
    let pattern = input["pattern"].as_str().unwrap_or("");
    let dir_str = input["dir"].as_str().unwrap_or(".");
    let ws = get_workspace(app, session_id).await;
    if let Err(e) = ensure_path_permission(app, dir_str, "搜索", ws.as_deref()).await {
        return e;
    }

    let path = Path::new(dir_str);
    let search_dir = if path.is_absolute() {
        path.to_path_buf()
    } else {
        std::env::current_dir().unwrap_or_default().join(path)
    };

    let mut limit = 50;
    let result = search_in_dir(&search_dir, pattern, &mut limit);
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

/// 在目录中递归搜索关键词
pub fn search_in_dir(dir: &Path, pattern: &str, limit: &mut usize) -> String {
    let mut result = String::new();
    if *limit == 0 {
        return result;
    }

    let entries = match std::fs::read_dir(dir) {
        Ok(e) => e,
        Err(_) => return result,
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
            result.push_str(&search_in_dir(&path, pattern, limit));
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
                    if line.contains(pattern) {
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
