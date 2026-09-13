//! # memory.rs — 全局记忆读写工具
//!
//! 主 Agent 通过 `ReadMemory` / `UpdateMemory` 直接读写跨会话的全局记忆文件，
//! `ConsolidateMemory` 触发一次全量整理（LLM 重写：合并同类项、压缩、清理过期条目）。
//!
//! ## 关键导出
//! - `read_memory()`: 读取全局记忆（可只取某小节）
//! - `update_memory()`: 对记忆做单条增改删（局部操作，不重写全文）
//! - `consolidate_memory()`: 触发一次全量整理
//! - `parse_memory()` / `render_memory()` / `extract_profile()`: 结构化解析与渲染
//!
//! ## 依赖
//! - Internal: `crate::core::session::memory`, `crate::core::agent::prompts`, `crate::infra::config`
//! - External: `tauri`
//!
//! ## 约束
//! - 记忆文件位于 agent_home 下、不在会话工作区沙箱内，因此这里直接原生读写，不走 file_tools
//! - 增改删只影响目标条目；整文件重写统一走 ConsolidateMemory，避免逐条写入都重生成上千字
//! - 结构缺失时只放行「空文件 + 新增」，其余一律拒绝，避免把解析不到的既有内容整段抹掉
//! - 记忆不参与工作区快照/回滚：它是跨会话资产，不是工作区产物

use tauri::{Emitter, Manager};

use crate::core::agent::prompts::MEMORY_CURATOR_SYSTEM;
use crate::core::session::memory as session_memory;
use crate::core::tools::framework;
use crate::infra::config::config::ConfigState;

/// 记忆文件的固定小节，数组顺序即渲染顺序
pub const MEMORY_SECTIONS: [&str; 5] = ["身份", "交互偏好", "工程偏好", "审美偏好", "环境"];

/// 随每轮上下文注入的精简画像小节（其余小节按需读取）
pub const PROFILE_SECTIONS: [&str; 2] = ["身份", "交互偏好"];

/// 记忆文件的长度预算（字符数）
pub const MEMORY_BUDGET_CHARS: usize = 1000;

/// 记忆超过该长度时，后台触发一次整理
pub const MEMORY_CONSOLIDATE_THRESHOLD_CHARS: usize = 1500;

/// 单条目的长度上限，防止把整段话塞进来
const MAX_ITEM_CHARS: usize = 120;

/// 新建记忆文件时的占位文本（见 `session::memory::create_memory_file`）
const SCAFFOLD_PLACEHOLDER: &str = "(暂无记录)";

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MemorySection {
    pub title: String,
    pub items: Vec<String>,
}

/// 解析记忆文件：`## 小节` 作标题，`- 条目` 作条目
pub fn parse_memory(content: &str) -> Vec<MemorySection> {
    let mut sections: Vec<MemorySection> = Vec::new();
    for raw_line in content.lines() {
        let line = raw_line.trim();
        if let Some(title) = line.strip_prefix("## ") {
            let title = title.trim();
            if !title.is_empty() {
                sections.push(MemorySection {
                    title: title.to_string(),
                    items: Vec::new(),
                });
            }
            continue;
        }
        if line.starts_with('#') {
            continue;
        }
        if let Some(item) = line.strip_prefix("- ") {
            let item = item.trim();
            if item.is_empty() {
                continue;
            }
            match sections.last_mut() {
                Some(section) => section.items.push(item.to_string()),
                None => sections.push(MemorySection {
                    title: "未分类".to_string(),
                    items: vec![item.to_string()],
                }),
            }
        }
    }
    sections.retain(|s| !s.items.is_empty());
    sections
}

/// 是否是「空骨架」：只有标题和 `(暂无记录)` 占位，没有任何真实条目。
///
/// 新建/清空后的记忆文件就是这种形态，此时允许首次写入直接建立结构。
/// 反过来，只要文件里有任何解析不到的正文，就不算空——那种情况宁可报错，
/// 也不能悄悄重写掉用户已有的内容。
fn is_empty_scaffold(content: &str) -> bool {
    content
        .lines()
        .map(str::trim)
        .filter(|line| !line.is_empty())
        .filter(|line| !line.starts_with('#'))
        .all(|line| line == SCAFFOLD_PLACEHOLDER)
}

/// 渲染回 Markdown：固定一级标题，小节按 MEMORY_SECTIONS 顺序排列，空节省略
pub fn render_memory(sections: &[MemorySection]) -> String {
    let mut ordered: Vec<&MemorySection> = Vec::new();
    for name in MEMORY_SECTIONS {
        if let Some(section) = sections.iter().find(|s| s.title == name) {
            ordered.push(section);
        }
    }
    for section in sections {
        if !ordered.iter().any(|s| s.title == section.title) {
            ordered.push(section);
        }
    }

    let mut out = String::from("# Global Memory\n");
    for section in ordered {
        if section.items.is_empty() {
            continue;
        }
        out.push_str(&format!("\n## {}\n", section.title));
        for item in &section.items {
            out.push_str(&format!("- {}\n", item));
        }
    }
    out
}

/// 精简画像：只取常驻小节。
///
/// - 文件里有小节结构：只返回 PROFILE_SECTIONS（可能为空串，其余小节交给 ReadMemory）
/// - 文件里没有小节结构但确有正文（历史遗留的自由格式）：整体兜底，宁可多带也不静默丢信息
/// - 空骨架：返回空串，避免把 `# Global Memory` 这行标题当画像注入
pub fn extract_profile(content: &str) -> String {
    let sections = parse_memory(content);
    if sections.is_empty() {
        return if is_empty_scaffold(content) {
            String::new()
        } else {
            content.trim().to_string()
        };
    }

    let mut out = String::new();
    for section in sections
        .iter()
        .filter(|s| PROFILE_SECTIONS.contains(&s.title.as_str()))
    {
        out.push_str(&format!("## {}\n", section.title));
        for item in &section.items {
            out.push_str(&format!("- {}\n", item));
        }
    }
    out.trim_end().to_string()
}

fn normalize_item(raw: &str) -> String {
    raw.trim().trim_start_matches('-').trim().replace('\n', " ")
}

/// 在小节列表上应用一次增改删，返回给模型看的操作摘要
fn apply_memory_edit(
    sections: &mut Vec<MemorySection>,
    action: &str,
    section_name: &str,
    content: &str,
    match_text: &str,
) -> Result<String, String> {
    if !MEMORY_SECTIONS.contains(&section_name) {
        return Err(format!(
            "未知小节「{}」。可用小节：{}",
            section_name,
            MEMORY_SECTIONS.join(" / ")
        ));
    }

    match action {
        "add" | "replace" => {
            if content.is_empty() {
                return Err("content 不能为空。".to_string());
            }
            if content.chars().count() > MAX_ITEM_CHARS {
                return Err(format!(
                    "单条记忆不能超过 {} 字，请压缩后重试。",
                    MAX_ITEM_CHARS
                ));
            }
        }
        "remove" => {}
        other => {
            return Err(format!(
                "action 必须是 add / replace / remove 之一，收到「{}」。",
                other
            ))
        }
    }

    if matches!(action, "replace" | "remove") && match_text.is_empty() {
        return Err("replace / remove 需要提供 match（定位已有条目的关键词）。".to_string());
    }

    let section_exists = sections.iter().any(|s| s.title == section_name);
    if !section_exists {
        if action == "add" {
            sections.push(MemorySection {
                title: section_name.to_string(),
                items: Vec::new(),
            });
        } else {
            return Err(format!(
                "记忆里还没有「{}」小节，没有可{}的条目。",
                section_name,
                if action == "remove" {
                    "删除"
                } else {
                    "更新"
                }
            ));
        }
    }

    let Some(section) = sections.iter_mut().find(|s| s.title == section_name) else {
        return Err("定位记忆小节失败，请重试。".to_string());
    };

    match action {
        "add" => {
            if section.items.iter().any(|item| item == content) {
                return Ok(format!(
                    "「{}」中已存在相同条目，未重复写入。",
                    section_name
                ));
            }
            section.items.push(content.to_string());
            Ok(format!("已在「{}」新增：{}", section_name, content))
        }
        "replace" => match section
            .items
            .iter()
            .position(|item| item.contains(match_text))
        {
            Some(index) => {
                let old = section.items[index].clone();
                section.items[index] = content.to_string();
                Ok(format!("已更新「{}」：{} → {}", section_name, old, content))
            }
            None => Err(format!(
                "在「{}」中找不到包含「{}」的条目。先用 ReadMemory 查看当前条目。",
                section_name, match_text
            )),
        },
        _ => match section
            .items
            .iter()
            .position(|item| item.contains(match_text))
        {
            Some(index) => {
                let removed = section.items.remove(index);
                Ok(format!("已从「{}」删除：{}", section_name, removed))
            }
            None => Err(format!(
                "在「{}」中找不到包含「{}」的条目。先用 ReadMemory 查看当前条目。",
                section_name, match_text
            )),
        },
    }
}

fn emit_memory_updated(app: &tauri::AppHandle, session_id: &str, summary: &str) {
    let _ = app.emit(
        "memory-updated",
        serde_json::json!({ "sessionId": session_id, "summary": summary }),
    );
}

/// 读取全局记忆（可只取一个小节）
pub async fn read_memory(
    _app: &tauri::AppHandle,
    input: &serde_json::Value,
    _session_id: &str,
) -> framework::ToolCallResult {
    // 持锁读，避免读到写入中途的半截文件
    let content = {
        let _guard = session_memory::lock_memory_file();
        session_memory::read_memory_file(&session_memory::get_global_memory_path(), "Global Memory")
    };
    let wanted = input["section"].as_str().unwrap_or("").trim();

    if wanted.is_empty() {
        return framework::ToolCallResult::ok(format!("[全局记忆全文]\n{}", content.trim()));
    }

    let sections = parse_memory(&content);
    match sections.iter().find(|s| s.title == wanted) {
        Some(section) => framework::ToolCallResult::ok(format!(
            "[全局记忆 · {}]\n{}",
            section.title,
            section
                .items
                .iter()
                .map(|item| format!("- {}", item))
                .collect::<Vec<_>>()
                .join("\n")
        )),
        None => framework::ToolCallResult::error(format!(
            "记忆中没有「{}」小节（可能是空的）。可用小节：{}",
            wanted,
            MEMORY_SECTIONS.join(" / ")
        )),
    }
}

/// 在锁内完成「读 → 改 → 写」，返回 (操作摘要, 渲染后的全文)。
///
/// `ops` 每项是 (action, section, content, match)。
///
/// 加锁是必须的：记忆工具是并行执行的（tools_runner 用 tokio::spawn + join_all），
/// 不串起来的话并发调用会各自读到同一份旧内容、各自通过校验、各自整份覆盖写回，
/// 最后一个赢、其余静默丢失，而每个调用都返回成功。
fn edit_memory_file(
    path: &std::path::Path,
    ops: &[(&str, &str, &str, &str)],
) -> Result<(String, String), String> {
    let _guard = session_memory::lock_memory_file();
    let original = session_memory::read_memory_file(path, "Global Memory");
    let mut sections = parse_memory(&original);

    // 结构缺失时的处理：
    // - 空骨架 + 全部新增 → 放行，由 add 建立标准结构（否则第一次记录永远写不进去）
    // - 空骨架 + 有改删 → 明确告知记忆是空的
    // - 有正文但解析不出小节 → 拒绝，别把解析不到的内容重写掉
    if sections.is_empty() {
        if is_empty_scaffold(&original) {
            if ops.iter().any(|(action, ..)| *action != "add") {
                return Err(
                    "记忆目前是空的，没有可修改或删除的条目。请用 action=add 新增。".to_string(),
                );
            }
        } else {
            return Err(
                "全局记忆没有可识别的小节结构，无法安全地做局部修改。先用 ReadMemory 查看当前内容，确认后可用 ConsolidateMemory 重建标准结构。"
                    .to_string(),
            );
        }
    }

    // 整批一起提交：任一条不合法就整体不写，避免留下半批结果
    let mut summaries: Vec<String> = Vec::with_capacity(ops.len());
    for (action, section, content, match_text) in ops {
        summaries.push(apply_memory_edit(
            &mut sections,
            action,
            section,
            content,
            match_text,
        )?);
    }

    let rendered = render_memory(&sections);
    if !session_memory::write_if_unchanged(path, &original, &rendered) {
        return Err("记忆在本次修改期间被其它操作改写，请重试。".to_string());
    }
    Ok((summaries.join("\n"), rendered))
}

/// 单条增改删全局记忆；`items` 可一次新增多条
pub async fn update_memory(
    app: &tauri::AppHandle,
    input: &serde_json::Value,
    session_id: &str,
) -> framework::ToolCallResult {
    let action = input["action"].as_str().unwrap_or("").trim().to_string();
    let section_name = input["section"].as_str().unwrap_or("").trim().to_string();
    let content = normalize_item(input["content"].as_str().unwrap_or(""));
    let match_text = input["match"].as_str().unwrap_or("").trim().to_string();

    // 一次记多条：模型经常要一口气记好几条事实，拆成多个并发调用会互相覆盖
    let batch: Vec<String> = input["items"]
        .as_array()
        .map(|items| {
            items
                .iter()
                .map(|item| normalize_item(item.as_str().unwrap_or("")))
                .filter(|item| !item.is_empty())
                .collect()
        })
        .unwrap_or_default();

    let mut ops: Vec<(&str, &str, &str, &str)> = Vec::new();
    if batch.is_empty() {
        ops.push((
            action.as_str(),
            section_name.as_str(),
            content.as_str(),
            match_text.as_str(),
        ));
    } else {
        for item in &batch {
            ops.push(("add", section_name.as_str(), item.as_str(), ""));
        }
    }

    let path = session_memory::get_global_memory_path();
    let (summary, rendered) = match edit_memory_file(&path, &ops) {
        Ok(result) => result,
        Err(message) => return framework::ToolCallResult::error(message),
    };

    emit_memory_updated(app, session_id, &summary);
    let total = rendered.chars().count();
    if total > MEMORY_BUDGET_CHARS {
        return framework::ToolCallResult::ok(format!(
            "{}\n提醒：记忆已 {} 字，超过 {} 字预算，建议调用 ConsolidateMemory 整理压缩。",
            summary, total, MEMORY_BUDGET_CHARS
        ));
    }
    framework::ToolCallResult::ok(summary)
}

/// 触发一次全局记忆整理（LLM 全量重写）
pub async fn consolidate_memory(
    app: &tauri::AppHandle,
    input: &serde_json::Value,
    session_id: &str,
) -> framework::ToolCallResult {
    // 同一次 run 内只允许触发一次，避免模型反复整理
    if let Some(manager) = app.try_state::<crate::infra::state::state::SessionManager>() {
        let ctx = manager.get_or_create(session_id).await;
        let scope = crate::infra::state::state::active_run_scope_key(app, session_id).await;
        let mut cache = ctx.dedupe_cache.lock().await;
        let state = cache.entry("memory_consolidate".to_string()).or_default();
        if let Some(entry) = state.get_mut(&scope) {
            entry.suppressed_count += 1;
            return framework::ToolCallResult::ok(format!(
                "Repeated ConsolidateMemory blocked: ConsolidateMemory was already requested in this agent run. Use the result or answer the user now. Suppressed duplicate #{}.",
                entry.suppressed_count
            ));
        }
        state.insert(
            scope,
            crate::infra::state::state::ToolDedupeCacheEntry {
                display: "consolidate".to_string(),
                suppressed_count: 0,
                running: false,
            },
        );
    }

    let config = app
        .state::<ConfigState>()
        .0
        .lock()
        .await
        .clone()
        .active_config();

    if config.api_key.is_empty() {
        return framework::ToolCallResult::error("未配置 API Key，无法整理记忆。".to_string());
    }

    let path = session_memory::get_global_memory_path();
    let original = session_memory::read_memory_file(&path, "Global Memory");
    let focus = input["focus"].as_str().unwrap_or("").trim();
    let focus_line = if focus.is_empty() {
        String::new()
    } else {
        format!("\n本次整理重点：{}。", focus)
    };

    let user_content = format!(
        "【当前全局记忆】\n{}\n\n【整理要求】\n按系统提示的结构与预算重写这份记忆：合并同类项、删除过期与不合格条目、压缩冗余表述，不要新增没有依据的事实。若当前记忆为空，只返回一级标题。{}",
        original.trim(),
        focus_line
    );

    let Some(new_content) = session_memory::rewrite_global_memory(
        session_id,
        &config,
        MEMORY_CURATOR_SYSTEM,
        user_content,
        "ConsolidateMemory",
    )
    .await
    else {
        return framework::ToolCallResult::ok("记忆整理未返回内容，保持原样。".to_string());
    };

    let written = {
        let _guard = session_memory::lock_memory_file();
        session_memory::write_if_unchanged(&path, &original, &new_content)
    };
    if !written {
        return framework::ToolCallResult::ok(
            "整理结果未写入：期间记忆已被其它操作修改，请重新调用 ConsolidateMemory。".to_string(),
        );
    }

    emit_memory_updated(app, session_id, "全局记忆已整理");
    framework::ToolCallResult::ok(format!(
        "记忆整理完成：{} 字 → {} 字。\n\n{}",
        original.chars().count(),
        new_content.chars().count(),
        new_content.trim()
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    const SAMPLE: &str = "# Global Memory\n\n## 身份\n- 称呼：沐风\n\n## 交互偏好\n- 语言：简体中文\n- 风格：结构化\n\n## 工程偏好\n- Rust + Vue\n\n## 审美偏好\n- 极简\n\n## 环境\n- Windows\n";

    fn sections_of(content: &str) -> Vec<MemorySection> {
        parse_memory(content)
    }

    #[test]
    fn parse_keeps_sections_and_items() {
        let sections = parse_memory(SAMPLE);
        assert_eq!(sections.len(), 5);
        assert_eq!(sections[0].title, "身份");
        assert_eq!(sections[1].title, "交互偏好");
        assert_eq!(sections[1].items, vec!["语言：简体中文", "风格：结构化"]);
    }

    #[test]
    fn render_orders_sections_and_is_stable() {
        let mut sections = parse_memory(SAMPLE);
        sections.reverse();
        let rendered = render_memory(&sections);
        let titles: Vec<&str> = rendered
            .lines()
            .filter(|line| line.starts_with("## "))
            .collect();
        assert_eq!(
            titles,
            vec![
                "## 身份",
                "## 交互偏好",
                "## 工程偏好",
                "## 审美偏好",
                "## 环境"
            ]
        );
        assert_eq!(render_memory(&parse_memory(&rendered)), rendered);
    }

    #[test]
    fn extract_profile_only_returns_profile_sections() {
        let profile = extract_profile(SAMPLE);
        assert!(profile.contains("## 身份"));
        assert!(profile.contains("## 交互偏好"));
        assert!(!profile.contains("工程偏好"));
        assert!(!profile.contains("Windows"));
    }

    #[test]
    fn extract_profile_is_empty_when_profile_sections_have_no_items() {
        let only_environment = "# Global Memory\n\n## 环境\n- Windows\n";
        assert_eq!(extract_profile(only_environment), "");
    }

    #[test]
    fn extract_profile_is_empty_for_blank_scaffold() {
        assert_eq!(extract_profile("# Global Memory\n"), "");
        assert_eq!(extract_profile("# Global Memory\n\n(暂无记录)\n"), "");
    }

    #[test]
    fn extract_profile_falls_back_to_full_text_without_structure() {
        let raw = "用户偏好简洁回答。\n没有任何小节结构。";
        assert_eq!(extract_profile(raw), raw);
    }

    #[test]
    fn render_drops_empty_sections() {
        let sections = vec![MemorySection {
            title: "环境".to_string(),
            items: vec![],
        }];
        let rendered = render_memory(&sections);
        assert!(!rendered.contains("## 环境"));
        assert!(rendered.starts_with("# Global Memory"));
    }

    #[test]
    fn empty_scaffold_detection() {
        assert!(is_empty_scaffold("# Global Memory\n"));
        assert!(is_empty_scaffold("# Global Memory\n\n(暂无记录)\n"));
        assert!(is_empty_scaffold(""));
        assert!(!is_empty_scaffold("# Global Memory\n用户偏好简洁\n"));
    }

    #[test]
    fn add_bootstraps_structure_from_blank_scaffold() {
        let mut sections = sections_of("# Global Memory\n");
        assert!(sections.is_empty());

        let summary = apply_memory_edit(&mut sections, "add", "身份", "称呼：沐风", "")
            .expect("空文件上的首次新增必须成功");
        assert!(summary.contains("身份"));

        let rendered = render_memory(&sections);
        assert!(rendered.contains("## 身份"));
        assert!(rendered.contains("- 称呼：沐风"));
        assert_eq!(extract_profile(&rendered), "## 身份\n- 称呼：沐风");
    }

    #[test]
    fn add_rejects_unknown_section() {
        let mut sections = sections_of(SAMPLE);
        let err = apply_memory_edit(&mut sections, "add", "爱好", "摄影", "").unwrap_err();
        assert!(err.contains("未知小节"));
    }

    #[test]
    fn add_is_idempotent_for_identical_item() {
        let mut sections = sections_of(SAMPLE);
        let summary = apply_memory_edit(&mut sections, "add", "身份", "称呼：沐风", "").unwrap();
        assert!(summary.contains("已存在相同条目"));
        assert_eq!(sections[0].items.len(), 1);
    }

    #[test]
    fn replace_and_remove_match_by_keyword() {
        let mut sections = sections_of(SAMPLE);
        apply_memory_edit(&mut sections, "replace", "交互偏好", "语言：英文", "语言").unwrap();
        assert!(sections[1].items.contains(&"语言：英文".to_string()));

        apply_memory_edit(&mut sections, "remove", "交互偏好", "", "风格").unwrap();
        assert!(!sections[1].items.iter().any(|item| item.contains("风格")));
    }

    #[test]
    fn replace_and_remove_need_existing_section() {
        let mut sections = sections_of(SAMPLE);
        let summary =
            apply_memory_edit(&mut sections, "replace", "环境", "Linux", "Windows").unwrap();
        assert_eq!(summary, "已更新「环境」：Windows → Linux");

        let mut blank = sections_of("# Global Memory\n");
        let err = apply_memory_edit(&mut blank, "remove", "身份", "", "称呼").unwrap_err();
        assert!(err.contains("还没有「身份」小节"));
    }

    #[test]
    fn replace_and_remove_require_match() {
        let mut sections = sections_of(SAMPLE);
        let err = apply_memory_edit(&mut sections, "remove", "身份", "", "").unwrap_err();
        assert!(err.contains("需要提供 match"));
    }

    #[test]
    fn item_length_is_capped() {
        let mut sections = sections_of(SAMPLE);
        let long = "长".repeat(MAX_ITEM_CHARS + 1);
        let err = apply_memory_edit(&mut sections, "add", "身份", &long, "").unwrap_err();
        assert!(err.contains("不能超过"));
    }

    #[test]
    fn batch_edits_are_all_or_nothing() {
        let dir = std::env::temp_dir().join("jarvis_memory_batch_test");
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("global_memory.md");
        std::fs::write(&path, "# Global Memory\n\n## 身份\n- 旧条目\n").unwrap();

        // 一批全成功
        let (summary, rendered) = edit_memory_file(
            &path,
            &[("add", "身份", "条目A", ""), ("add", "身份", "条目B", "")],
        )
        .unwrap();
        assert!(summary.contains("条目A") && summary.contains("条目B"));
        assert!(rendered.contains("条目A") && rendered.contains("条目B"));

        // 其中一条超长 → 整批都不写（不能留半批结果）
        let long = "长".repeat(MAX_ITEM_CHARS + 1);
        let err = edit_memory_file(
            &path,
            &[("add", "身份", "条目C", ""), ("add", "身份", &long, "")],
        )
        .unwrap_err();
        assert!(err.contains("不能超过"));
        let after = std::fs::read_to_string(&path).unwrap();
        assert!(
            !after.contains("条目C"),
            "整批失败时不应写入任何一条：\n{}",
            after
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn concurrent_edits_do_not_lose_items() {
        // 回归：4 个并发 UpdateMemory 曾各自读到同一份旧内容、各自整份覆盖写回，
        // 结果只剩最后一个调用的条目，其余静默丢失（每个调用却都返回成功）。
        // 加锁后必须全部保留。
        let dir = std::env::temp_dir().join("jarvis_memory_concurrency_test");
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("global_memory.md");
        std::fs::write(&path, "# Global Memory\n\n## 身份\n- 旧条目\n").unwrap();

        let barrier = std::sync::Arc::new(std::sync::Barrier::new(4));
        let mut handles = Vec::new();
        for item in ["条目A", "条目B", "条目C", "条目D"] {
            let path = path.clone();
            let barrier = barrier.clone();
            handles.push(std::thread::spawn(move || {
                barrier.wait();
                edit_memory_file(&path, &[("add", "身份", item, "")]).map(|_| ())
            }));
        }
        for handle in handles {
            assert!(handle.join().unwrap().is_ok());
        }

        let rendered = std::fs::read_to_string(&path).unwrap();
        for item in ["旧条目", "条目A", "条目B", "条目C", "条目D"] {
            assert!(
                rendered.contains(item),
                "丢失条目「{}」：\n{}",
                item,
                rendered
            );
        }
        let _ = std::fs::remove_dir_all(&dir);
    }
}
