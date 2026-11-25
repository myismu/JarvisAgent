//! # edit.rs — 执行安全的文本搜索替换文件编辑
//!
//! 实现 agent 可调用的 `edit_file` 工具，在权限校验后进行唯一匹配替换，并记录快照；
//! 同时拒绝 Notebook 文本编辑、归一化行尾并执行 TOCTOU 防护。
//! 支持 `edits` 数组批量编辑（单次调用内多段替换），向后兼容单条 `old_text`/`new_text`。
//!
//! ## Key Exports
//! - `edit_file()`: 基于唯一 old_text 匹配编辑普通文本文件（支持批量）
//!
//! ## Dependencies
//! - Internal: `crate::core::rollback`, `crate::core::tools::framework::permission`, `super::notebook_guard`
//!
//! ## Constraints
//! - 不得用于 `.ipynb` 或 notebook-shaped JSON，Notebook 必须通过 cell 级工具修改

use crate::core::rollback::Patch;
use crate::core::tools::framework;
use crate::core::tools::framework::permission::ensure_path_permission;

use super::common::{
    encode_text_preserve_encoding, ensure_fresh_read, is_locked_file_error, is_unc_path,
    normalize_line_endings, normalize_quotes, read_text_preserve_encoding, record_file_read,
    resolve_path, unc_path_rejection, MAX_FILE_SIZE_BYTES,
};
use super::diff::compute_diff;
use super::workspace::{
    get_workspace, record_patch_to_snapshot, resolve_exec_path, sandbox_missing_hint,
};
use crate::core::tools::notebook_tools::notebook_guard::{
    is_notebook_path, looks_like_notebook_json, notebook_text_edit_rejection,
};

/// 行尾归一化视图：把 CRLF / 孤立 CR 统一成 LF，并保留「视图字节 → 原始字节」映射。
///
/// 为什么需要它：磁盘文件可能是 CRLF / LF 混合（历史编辑、外部工具都会造成），
/// 而模型给出的 `old_text` 通常是 LF。直接做字节级 `matches` 会因为一个不可见的
/// `\r` 全盘落空，报错还指向「内容对不上」，把排查引到错误的方向。
/// 这里只在**匹配**时用 LF 视图，替换仍落在原始区间上，因此不会把整个文件的行尾改掉。
struct EolView {
    text: String,
    /// 视图字节下标 → 原始字节下标；长度恒为 text.len() + 1
    map: Vec<usize>,
}

impl EolView {
    fn new(raw: &str) -> Self {
        let bytes = raw.as_bytes();
        let mut out = Vec::with_capacity(bytes.len());
        let mut map = Vec::with_capacity(bytes.len() + 1);
        let mut i = 0;
        while i < bytes.len() {
            map.push(i);
            if bytes[i] == b'\r' {
                out.push(b'\n');
                i += if bytes.get(i + 1) == Some(&b'\n') {
                    2
                } else {
                    1
                };
            } else {
                out.push(bytes[i]);
                i += 1;
            }
        }
        map.push(bytes.len());
        // 只删掉了 \r，其余字节原样保留，UTF-8 序列依然合法
        Self {
            text: String::from_utf8(out).unwrap_or_default(),
            map,
        }
    }

    fn raw_range(&self, start: usize, end: usize) -> std::ops::Range<usize> {
        self.map[start]..self.map[end]
    }
}

/// 在原始内容中查找 `old_text` 的全部匹配区间（容忍行尾差异）。
///
/// 先做字节级精确匹配（纯 LF 或 old_text 自带 CRLF 时最快），一处都没有时才退到
/// 行尾归一化视图。`limit` 用于「只取第一处」的场景，避免为一条编辑扫全文件。
fn find_edit_ranges(
    raw: &str,
    old_text: &str,
    limit: Option<usize>,
) -> Vec<std::ops::Range<usize>> {
    if old_text.is_empty() {
        return Vec::new();
    }
    let mut ranges = Vec::new();
    let mut from = 0;
    while let Some(pos) = raw[from..].find(old_text) {
        let start = from + pos;
        let end = start + old_text.len();
        ranges.push(start..end);
        if limit.is_some_and(|n| ranges.len() >= n) {
            return ranges;
        }
        from = end.max(start + 1);
    }
    if !ranges.is_empty() {
        return ranges;
    }

    let view = EolView::new(raw);
    let needle = normalize_line_endings(old_text);
    if needle.is_empty() {
        return ranges;
    }
    let mut from = 0;
    while let Some(pos) = view.text[from..].find(&needle) {
        let start = from + pos;
        let end = start + needle.len();
        ranges.push(view.raw_range(start, end));
        if limit.is_some_and(|n| ranges.len() >= n) {
            return ranges;
        }
        from = end.max(start + 1);
    }
    ranges
}

struct SingleEdit {
    old_text: String,
    new_text: String,
    replace_all: bool,
}

/// 解析 old_text 参数（别名兼容）
fn resolve_old_text(input: &serde_json::Value) -> String {
    input["old_text"]
        .as_str()
        .or_else(|| input["old_str"].as_str())
        .or_else(|| input["old_string"].as_str())
        .or_else(|| input["old_content"].as_str())
        .or_else(|| input["search"].as_str())
        .unwrap_or("")
        .to_string()
}

/// 解析 new_text 参数（别名兼容）
fn resolve_new_text(input: &serde_json::Value) -> String {
    normalize_line_endings(
        input["new_text"]
            .as_str()
            .or_else(|| input["new_str"].as_str())
            .or_else(|| input["new_string"].as_str())
            .or_else(|| input["new_content"].as_str())
            .or_else(|| input["replace"].as_str())
            .unwrap_or(""),
    )
}

/// 解析编辑列表：优先 `edits` 数组，否则退到单条 `old_text`/`new_text`
fn resolve_edits(input: &serde_json::Value) -> Vec<SingleEdit> {
    if let Some(arr) = input["edits"].as_array() {
        arr.iter()
            .map(|item| SingleEdit {
                old_text: resolve_old_text(item),
                new_text: resolve_new_text(item),
                replace_all: item["replace_all"].as_bool().unwrap_or(false),
            })
            .collect()
    } else {
        // 单条模式：只有确实给了 old_text 才构造一条编辑。
        //
        // 之前这里是无条件构造，哪怕整个参数对象是空的也会返回一条「空编辑」，
        // 于是 `edits.is_empty()` 的守卫永远不触发，参数缺失被伪装成
        // 「读不到文件」（真实事故：args 丢失后报 os error 3，查了很久）。
        let old_text = resolve_old_text(input);
        if old_text.is_empty() {
            Vec::new()
        } else {
            vec![SingleEdit {
                old_text,
                new_text: resolve_new_text(input),
                replace_all: input["replace_all"].as_bool().unwrap_or(false),
            }]
        }
    }
}

/// 校验单条编辑，返回 (old_text引用, 错误消息)
fn validate_single_edit(
    edit: &SingleEdit,
    content: &str,
    idx: usize,
    path: &str,
    is_batch: bool,
) -> Result<usize, String> {
    let label = if is_batch {
        format!("edits[{}]", idx)
    } else {
        "old_text".to_string()
    };

    if edit.old_text.is_empty() {
        return Err(format!(
            "编辑失败: {} old_text 为空。EditFile 是搜索替换工具，old_text 是要被替换的原文片段（至少 3~5 行以确保唯一匹配），new_text 是替换后的新内容。如需替换整个文件请使用 WriteFile。",
            label
        ));
    }
    if edit.old_text.trim().len() < 10 {
        return Err(format!(
            "编辑失败: {} old_text 太短（< 10 个有效字符），请包含至少 3~5 行上下文使匹配唯一。",
            label
        ));
    }

    let match_count = find_edit_ranges(content, &edit.old_text, None).len();

    if match_count == 0 {
        // 行尾也一并归一化：跨行片段在 CRLF 文件里会因 \r 而漏判
        let normalized_content = normalize_quotes(&normalize_line_endings(content));
        let normalized_old = normalize_quotes(&normalize_line_endings(&edit.old_text));
        if normalized_content.contains(&normalized_old) {
            return Err(format!(
                "编辑失败: {} 未在 {} 中找到精确匹配的旧文本块，但发现引号风格不一致。\n文件使用弯引号，请使用相同的引号风格重试。",
                label, path
            ));
        }
        // 走到这一行说明「必须先读」那道闸门已经过了：未读或读后过期会在
        // common.rs 的 freshness_error 处直接中止，根本到不了这里。
        // 所以失败原因是内容对不上，不是“没读文件”——文案必须据实说，
        // 笼统让人去 read_file 会把排查方向带偏到“重读”上，而问题在 old_text 本身。
        return Err(format!(
            "编辑失败: {} 未在 {} 中找到与 old_text 逐字一致的片段（本次已通过读取校验，失败原因是内容对不上，不是没读过文件）。\n常见原因：缩进或空白不同、换行符差异、全角/半角或引号风格不同、old_text 与实际内容不一致（例如误把 new_text 当成了 old_text）。\n请重新 ReadFile 取当前内容逐字符核对，或缩小 old_text 范围（仍保持 3~5 行）后重试。",
            label, path
        ));
    }

    if !edit.replace_all && match_count > 1 {
        let mut context_msg = format!(
            "编辑失败: {} 旧文本在 {} 中匹配了 {} 处，请提供更多上下文使其唯一，或设置 replace_all=true 替换所有匹配。\n\n",
            label, path, match_count
        );
        let lines: Vec<&str> = content.lines().collect();
        let old_lines_count = edit.old_text.lines().count();
        let mut search_from = 0;
        for m_idx in 0..match_count {
            if let Some(pos) = content[search_from..].find(&edit.old_text) {
                let absolute_pos = search_from + pos;
                let line_num = content[..absolute_pos].lines().count();
                let ctx_start = line_num.saturating_sub(2);
                let ctx_end = (line_num + old_lines_count + 2).min(lines.len());
                context_msg.push_str(&format!(
                    "--- 匹配 {} (第 {} 行附近) ---\n",
                    m_idx + 1,
                    line_num + 1
                ));
                for (i, line) in lines[ctx_start..ctx_end].iter().enumerate() {
                    let ln = ctx_start + i + 1;
                    let marker = if ln > line_num && ln <= line_num + old_lines_count {
                        ">>>"
                    } else {
                        "   "
                    };
                    context_msg.push_str(&format!("{} {:4} | {}\n", marker, ln, line));
                }
                context_msg.push('\n');
                search_from = absolute_pos + edit.old_text.len();
            }
        }
        context_msg
            .push_str("请提供更多上下文使 old_text 唯一，或设置 replace_all=true 替换所有匹配。");
        return Err(context_msg);
    }

    Ok(match_count)
}

/// 应用阶段找不到 old_text 时的统一报错。
///
/// 这个错误只可能出现在批量编辑里：单条的 old_text 刚在校验阶段命中过，
/// 而校验与应用之间内容没有变化。所以文案直接指向「前面的编辑改写了它」。
fn apply_miss_error(batch_size: usize, idx: usize, old_text: &str) -> String {
    let label = if batch_size > 1 {
        format!("edits[{}]", idx)
    } else {
        "old_text".to_string()
    };
    let preview: String = old_text.chars().take(40).collect();
    format!(
        "编辑中止: {} 在应用阶段找不到 old_text —— 该片段大概率已被前面的编辑改写。\n批量编辑的每条 old_text 都是对着原文件校验的，若前一条的 new_text 覆盖了后一条的 old_text 所在区域，就会出现这种冲突。\n请拆分提交，或调整顺序/范围使各条互不重叠。\n冲突片段（前 40 字）：{}",
        label, preview
    )
}

/// 对所有编辑应用替换（所有校验通过后才调用）。
///
/// 返回 Err 而不是跳过：批量编辑中，前一条的 new_text 可能吃掉后一条的 old_text，
/// 而校验阶段两条都是对着**原始内容**比对的，所以都能通过。旧实现在应用阶段
/// 找不到就跳过那一条、整体仍报成功——用户以为改了三处，实际只落了两处，
/// 且 `total_replacements` 还会按校验结果谎报数量。这是数据完整性问题，
/// 宁可整次中止让用户重来，也不能静默丢改动。
fn apply_edits(content: &str, edits: &[SingleEdit]) -> Result<String, String> {
    // 纯 LF 文件没有行尾歧义，直接走原来的字节级替换，保持既有行为与性能
    if !content.contains('\r') {
        return apply_edits_exact(content, edits);
    }
    let mut result = content.to_string();
    for (idx, edit) in edits.iter().enumerate() {
        let limit = if edit.replace_all { None } else { Some(1) };
        // 定位可以容忍行尾差异，但落盘一律 LF：与 .gitattributes 的 eol=lf
        // 以及 write.rs 的口径保持一致，避免工具继续制造混合行尾。
        // 倒序替换保证前面的偏移不受影响。
        let mut ranges = find_edit_ranges(&result, &edit.old_text, limit);
        if ranges.is_empty() {
            return Err(apply_miss_error(edits.len(), idx, &edit.old_text));
        }
        while let Some(range) = ranges.pop() {
            result.replace_range(range, &edit.new_text);
        }
    }
    Ok(result)
}

/// 精确字节级替换（不含行尾容忍）：纯 LF 文件的快路径。
fn apply_edits_exact(content: &str, edits: &[SingleEdit]) -> Result<String, String> {
    let mut result = content.to_string();
    for (idx, edit) in edits.iter().enumerate() {
        if !result.contains(&edit.old_text) {
            return Err(apply_miss_error(edits.len(), idx, &edit.old_text));
        }
        if edit.replace_all {
            result = result.replace(&edit.old_text, &edit.new_text);
        } else {
            result = result.replacen(&edit.old_text, &edit.new_text, 1);
        }
    }
    Ok(result)
}

/// 描述工具入参的顶层形状：只列 key 与 JSON 类型，**不记录任何内容**。
///
/// 用途：参数在到达工具前被改写成非对象时，`path` 会退化成空串，
/// 而错误只会显示「读不到文件」。把形状带进错误文案，一次复现就能判断
/// 是「字段没送到」还是「整个 args 不是对象」，不必再去翻审计日志。
fn describe_input_shape(input: &serde_json::Value) -> String {
    fn type_name(value: &serde_json::Value) -> &'static str {
        match value {
            serde_json::Value::Null => "null",
            serde_json::Value::Bool(_) => "bool",
            serde_json::Value::Number(_) => "number",
            serde_json::Value::String(_) => "string",
            serde_json::Value::Array(_) => "array",
            serde_json::Value::Object(_) => "object",
        }
    }

    match input {
        serde_json::Value::Object(map) => {
            let mut keys: Vec<String> = map
                .iter()
                .map(|(key, value)| format!("{}:{}", key, type_name(value)))
                .collect();
            keys.sort();
            if keys.is_empty() {
                "空对象".to_string()
            } else {
                keys.join(", ")
            }
        }
        other => format!("非对象({})", type_name(other)),
    }
}

/// 基于搜索替换编辑文件（唯一性检查 + 引号归一化 + TOCTOU 防护 + 自动快照）
/// 支持 `edits` 数组批量编辑，向后兼容单条 `old_text`/`new_text`
pub async fn edit_file(
    app: &tauri::AppHandle,
    input: &serde_json::Value,
    session_id: &str,
) -> framework::ToolCallResult {
    let path = resolve_path(input);
    let edits = resolve_edits(input);
    let is_batch = edits.len() > 1;

    // 参数形态守卫（2026-11-23）：路径为空时立刻报参数问题，而不是继续往下走。
    //
    // 背景：args 在到达工具前若被改写成非对象，`resolve_path` 退回空串，
    // `resolve_exec_path` 把空串 join 到工作区根目录，`fs::read` 读一个目录
    // 得到 ERROR_PATH_NOT_FOUND，错误最终显示成「无法读取文件」——
    // 现象与「文件不存在」完全一样，但真正原因是参数没送到。
    if path.trim().is_empty() {
        return framework::ToolCallResult::error(format!(
            "编辑失败: 缺少必填参数 'path'（收到的参数: {}）。\
             经 ExecuteTool 调用时，参数必须写在 args 里，例如 \
             ExecuteTool(name=\"EditFile\", args={{\"path\":\"文件路径\",\"old_text\":\"...\",\"new_text\":\"...\"}})。",
            describe_input_shape(input)
        ));
    }

    if edits.is_empty() {
        return framework::ToolCallResult::error(format!(
            "编辑失败: 缺少 old_text 或 edits 参数（收到的参数: {}）",
            describe_input_shape(input)
        ));
    }

    if is_unc_path(&path) {
        return framework::ToolCallResult::error(unc_path_rejection("编辑", &path));
    }
    let ws = get_workspace(app, session_id).await;
    if let Err(e) = ensure_path_permission(app, &path, "编辑", ws.as_deref()).await {
        return framework::ToolCallResult::error(e);
    }
    // 执行层与校验层对齐：相对路径按沙箱目录解析（而非进程 CWD），否则会越界编辑
    let path = resolve_exec_path(&path, ws.as_deref());
    if is_notebook_path(&path) {
        return framework::ToolCallResult::error(notebook_text_edit_rejection(&path));
    }

    let read_mtime = std::fs::metadata(&path)
        .ok()
        .and_then(|m| m.modified().ok());

    match read_text_preserve_encoding(&path) {
        Ok(decoded) => {
            let content = decoded.content;
            let encoding = decoded.encoding;

            let file_size = std::fs::metadata(&path).map(|m| m.len()).unwrap_or(0);
            if file_size > MAX_FILE_SIZE_BYTES {
                return framework::ToolCallResult::error(format!(
                    "编辑失败: 文件 {} 过大 ({} bytes)，超过限制 {} bytes。",
                    path, file_size, MAX_FILE_SIZE_BYTES
                ));
            }

            if looks_like_notebook_json(&content) {
                return framework::ToolCallResult::error(notebook_text_edit_rejection(&path));
            }

            // 先读后改闸门：会话内没读过、或读后被外部改过 → 拦。
            // 放在匹配校验**之前**：blind edit 的报错应该是"请先读文件"，
            // 而不是一条误导性的匹配失败（见 common::ensure_fresh_read 的模块注释）。
            if let Err(e) = ensure_fresh_read(app, session_id, &path, &content).await {
                return framework::ToolCallResult::error(e);
            }

            // 逐条校验
            let mut total_replacements = 0usize;
            for (i, edit) in edits.iter().enumerate() {
                let count = match validate_single_edit(edit, &content, i, &path, is_batch) {
                    Ok(c) => c,
                    Err(e) => return framework::ToolCallResult::error(e),
                };
                if edit.replace_all {
                    total_replacements += count;
                } else {
                    total_replacements += 1;
                }
            }

            // 所有校验通过 → 原子替换（应用阶段再撞车就整次中止，不写盘）
            let updated_content = match apply_edits(&content, &edits) {
                Ok(c) => c,
                Err(e) => return framework::ToolCallResult::error(e),
            };

            // TOCTOU 防护
            if let (Some(orig_mtime), Ok(current_meta)) = (read_mtime, std::fs::metadata(&path)) {
                if let Ok(current_mtime) = current_meta.modified() {
                    if current_mtime != orig_mtime {
                        return framework::ToolCallResult::error(format!(
                            "编辑中止: 文件 {} 在读取后被外部修改。请重新读取后再编辑。",
                            path
                        ));
                    }
                }
            }

            let bytes = match encode_text_preserve_encoding(&updated_content, encoding) {
                Ok(bytes) => bytes,
                Err(e) => {
                    return framework::ToolCallResult::error(format!("编辑并保存失败: {}", e))
                }
            };

            match std::fs::write(&path, bytes) {
                Ok(_) => {
                    let patch = Patch::update_file_patch(
                        session_id,
                        path.to_string(),
                        content.clone(),
                        updated_content.clone(),
                        Some(compute_diff(&content, &updated_content)),
                    );
                    let msg = if is_batch {
                        Some(format!(
                            "批量编辑 {} 处 → {} ({} 条)",
                            total_replacements,
                            path,
                            edits.len()
                        ))
                    } else if total_replacements > 1 {
                        Some(format!("全局替换 {} 处 → {}", total_replacements, path))
                    } else {
                        Some(format!("编辑 {}", path))
                    };
                    record_patch_to_snapshot(app, session_id, patch, msg).await;
                    // 写入成功 → 刷新认知为编辑后的新版本，连续编辑不会被误判为"外部修改"
                    record_file_read(app, session_id, &path, &updated_content).await;

                    if is_batch {
                        framework::ToolCallResult::ok(format!(
                            "成功: 在 {} 中批量替换了 {} 处（{} 条编辑）",
                            path,
                            total_replacements,
                            edits.len()
                        ))
                    } else if total_replacements > 1 {
                        framework::ToolCallResult::ok(format!(
                            "成功: 在 {} 中替换了 {} 处匹配",
                            path, total_replacements
                        ))
                    } else {
                        framework::ToolCallResult::ok(format!("成功编辑 {}", path))
                    }
                }
                Err(e) => {
                    let err_msg = e.to_string();
                    if is_locked_file_error(&err_msg) {
                        framework::ToolCallResult::error(format!(
                            "编辑并保存失败: 文件被其他智能体或程序锁定，请稍后重试。详细错误: {}",
                            e
                        ))
                    } else {
                        framework::ToolCallResult::error(format!(
                            "编辑并保存失败: {}{}（目标路径: {}）",
                            e,
                            sandbox_missing_hint(ws.as_deref()),
                            path
                        ))
                    }
                }
            }
        }
        Err(e) => {
            let err_msg = e.to_string();
            if is_locked_file_error(&err_msg) {
                framework::ToolCallResult::error(format!(
                    "编辑失败: 文件可能被其他智能体或程序锁定，请稍后重试。详细错误: {}",
                    e
                ))
            } else {
                framework::ToolCallResult::error(format!(
                    "编辑失败，无法读取文件: {}{}（目标路径: {}）",
                    e,
                    sandbox_missing_hint(ws.as_deref()),
                    path
                ))
            }
        }
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn replace_replaces_all_occurrences() {
        let content = "line1\nfoo\nline3\nfoo\nline5";
        let result = content.replace("foo", "bar");
        assert_eq!(result, "line1\nbar\nline3\nbar\nline5");
    }

    #[test]
    fn replacen_replaces_only_first() {
        let content = "line1\nfoo\nline3\nfoo\nline5";
        let result = content.replacen("foo", "bar", 1);
        assert_eq!(result, "line1\nbar\nline3\nfoo\nline5");
    }

    #[test]
    fn matches_counts_correctly() {
        let content = "foo bar foo baz foo";
        assert_eq!(content.matches("foo").count(), 3);
    }

    #[test]
    fn replacement_with_zero_matches_is_noop() {
        let content = "hello world";
        let result = content.replace("xyz", "abc");
        assert_eq!(result, "hello world");
        assert_eq!(content.matches("xyz").count(), 0);
    }

    #[test]
    fn batch_edit_replaces_all_in_order() {
        let content = "header\nbody\nfooter";
        let edits = vec![
            super::SingleEdit {
                old_text: "header".into(),
                new_text: "HEAD".into(),
                replace_all: false,
            },
            super::SingleEdit {
                old_text: "footer".into(),
                new_text: "FOOT".into(),
                replace_all: false,
            },
        ];
        let result = super::apply_edits(content, &edits);
        assert_eq!(result.unwrap(), "HEAD\nbody\nFOOT");
    }

    #[test]
    fn eol_view_maps_bytes_to_raw_offsets() {
        // 视图下标必须能准确映回原始字节，否则替换会切在半个 \r\n 上
        let view = super::EolView::new("a\r\nb");
        assert_eq!(view.raw_range(0, 3), 0..4);
    }

    #[test]
    fn pure_lf_file_uses_exact_path() {
        // 纯 LF 文件走原来的字节级替换，不应因行尾处理引入额外开销或改变行为
        let content = "a\nb\nc";
        let ranges = super::find_edit_ranges(content, "a\nb", None);
        assert_eq!(ranges, vec![0..3]);
    }

    #[test]
    fn lf_old_text_matches_crlf_content() {
        // 模型给的 old_text 是 LF，文件是 CRLF：应当能定位到原始区间（含 \r\n）
        let content = "a\r\nb\r\nc";
        let ranges = super::find_edit_ranges(content, "a\nb", None);
        assert_eq!(ranges, vec![0..4]);
    }

    #[test]
    fn edit_writes_lf_even_in_crlf_file() {
        // 定位容忍行尾差异，但落盘一律 LF：与 .gitattributes 的 eol=lf 口径一致
        let content = "a\r\nb\r\nc";
        let edits = vec![super::SingleEdit {
            old_text: "a\nb".into(),
            new_text: "x\ny".into(),
            replace_all: false,
        }];
        assert_eq!(super::apply_edits(content, &edits).unwrap(), "x\ny\r\nc");
    }

    #[test]
    fn mixed_file_edits_write_lf() {
        // 定位要能跨过混合行尾，但落盘统一 LF
        let content = "a\nb\nc\r\nd\r\ne";
        let edits = vec![
            super::SingleEdit {
                old_text: "a\nb".into(),
                new_text: "x\ny".into(),
                replace_all: false,
            },
            super::SingleEdit {
                old_text: "c\nd".into(),
                new_text: "z\nw".into(),
                replace_all: false,
            },
        ];
        assert_eq!(
            super::apply_edits(content, &edits).unwrap(),
            "x\ny\nz\nw\r\ne"
        );
    }

    #[test]
    fn replace_all_covers_multi_line_crlf_matches() {
        // replace_all + 跨行匹配：两处 CRLF 区间都要命中，且写回 LF
        let content = "a\r\nb\r\nX\r\na\r\nb";
        let edits = vec![super::SingleEdit {
            old_text: "a\nb".into(),
            new_text: "c\nd".into(),
            replace_all: true,
        }];
        assert_eq!(
            super::apply_edits(content, &edits).unwrap(),
            "c\nd\r\nX\r\nc\nd"
        );
    }

    #[test]
    fn batch_edit_with_replace_all() {
        let content = "a a a";
        let edits = vec![super::SingleEdit {
            old_text: "a".into(),
            new_text: "b".into(),
            replace_all: true,
        }];
        let result = super::apply_edits(content, &edits);
        assert_eq!(result.unwrap(), "b b b");
    }

    #[test]
    fn conflicting_batch_edit_aborts_instead_of_silently_skipping() {
        // 前一条的 new_text 吃掉了后一条的 old_text：两条对着原文件都能匹配，
        // 但应用阶段第二条必然找不到。旧实现会静默跳过并仍报成功，
        // 用户看到「改了两处」而磁盘上只落了一处。
        let content = "line1\nSECTION\nline3\n";
        let edits = vec![
            super::SingleEdit {
                old_text: "line1\nSECTION".into(),
                new_text: "REPLACED".into(),
                replace_all: false,
            },
            super::SingleEdit {
                old_text: "SECTION\nline3".into(),
                new_text: "OTHER".into(),
                replace_all: false,
            },
        ];

        let err = super::apply_edits(content, &edits).unwrap_err();
        assert!(
            err.contains("edits[1]"),
            "报错应点名冲突的那一条编辑: {}",
            err
        );
    }
}
