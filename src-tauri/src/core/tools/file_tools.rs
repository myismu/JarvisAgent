// --- 文件操作工具模块 ---
// read_file, read_file_skeleton, write_file, edit_file, search_repo, list_directory

use std::path::Path;

use super::permission::{ensure_path_permission, is_path_safe};

/// 读取文件内容（支持行号范围）
pub async fn read_file(
    _app: &tauri::AppHandle,
    input: &serde_json::Value,
) -> String {
    let path = input["path"].as_str().unwrap_or("");
    let start_line = input["start_line"].as_u64().unwrap_or(1) as usize;
    let end_line = input["end_line"].as_u64().unwrap_or(usize::MAX as u64) as usize;

    if !is_path_safe(path) {
        return "路径不安全".to_string();
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
        Err(e) => format!("读取错误: {}", e),
    }
}

/// 提取文件结构骨架（函数/类/import 签名）
pub async fn read_file_skeleton(
    _app: &tauri::AppHandle,
    input: &serde_json::Value,
) -> String {
    let path = input["path"].as_str().unwrap_or("");
    if !is_path_safe(path) {
        return "路径不安全".to_string();
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
        Err(e) => format!("读取错误: {}", e),
    }
}

/// 写入文件
pub async fn write_file(
    _app: &tauri::AppHandle,
    input: &serde_json::Value,
) -> String {
    let path = input["path"].as_str().unwrap_or("");
    let content = input["content"].as_str().unwrap_or("");
    if !is_path_safe(path) {
        return "路径不安全".to_string();
    }
    match std::fs::write(path, content) {
        Ok(_) => format!("成功写入 {}", path),
        Err(e) => format!("写入失败: {}", e),
    }
}

/// 基于搜索替换编辑文件
pub async fn edit_file(
    _app: &tauri::AppHandle,
    input: &serde_json::Value,
) -> String {
    let path = input["path"].as_str().unwrap_or("");
    let old_text = input["old_text"].as_str().unwrap_or("");
    let new_text = input["new_text"].as_str().unwrap_or("");
    if !is_path_safe(path) {
        return "路径不安全".to_string();
    }
    match std::fs::read_to_string(path) {
        Ok(content) => {
            if !content.contains(old_text) {
                format!("编辑失败: 未在 {} 中找到指定的旧文本块。", path)
            } else {
                let updated_content = content.replacen(old_text, new_text, 1);
                match std::fs::write(path, updated_content) {
                    Ok(_) => format!("成功编辑 {}", path),
                    Err(e) => format!("编辑并保存失败: {}", e),
                }
            }
        }
        Err(e) => format!("编辑失败，无法读取文件: {}", e),
    }
}

/// 在指定目录下搜索关键词
pub async fn search_repo(
    _app: &tauri::AppHandle,
    input: &serde_json::Value,
) -> String {
    let pattern = input["pattern"].as_str().unwrap_or("");
    let dir_str = input["dir"].as_str().unwrap_or(".");
    if !is_path_safe(dir_str) {
        return "路径不安全".to_string();
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
) -> String {
    let path_str = input["path"].as_str().unwrap_or(".");
    if let Err(e) = ensure_path_permission(app, path_str, "列出").await {
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
