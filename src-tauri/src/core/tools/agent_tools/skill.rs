//! # skill.rs — 技能加载工具
//!
//! 按名称从 skills 目录加载技能知识文件。
//!
//! ## 关键导出
//! - `load_skill()`: 按名称加载技能知识

use super::super::load_all_skills;
use tauri::Manager;

/// 加载技能
pub async fn load_skill(
    app: &tauri::AppHandle,
    input: &serde_json::Value,
    session_id: &str,
) -> String {
    let skill_name = input["name"].as_str().unwrap_or("");
    if let Some(manager) = app.try_state::<crate::core::state::SessionManager>() {
        let ctx = manager.get_or_create(session_id).await;
        let scope = crate::core::state::active_run_scope_key(app, session_id).await;
        let key = format!("{}:{}", scope, skill_name.to_ascii_lowercase());
        let mut state = ctx.loaded_skill_state.lock().await;
        if let Some(entry) = state.get_mut(&key) {
            entry.suppressed_count += 1;
            return format!(
                "Repeated LoadSkill blocked: skill '{}' was already loaded in this agent run. Use the previous skill content instead of loading it again. Suppressed duplicate #{}.",
                entry.display, entry.suppressed_count
            );
        }
        state.insert(
            key,
            crate::core::state::ToolDedupeCacheEntry {
                display: skill_name.to_string(),
                suppressed_count: 0,
                running: false,
            },
        );
    }
    let skills = load_all_skills();
    match skills.into_iter().find(|s| s.name == skill_name) {
        Some(skill) => format!("<skill name=\"{}\">\n{}\n</skill>", skill.name, skill.body),
        None => {
            let available: Vec<String> = load_all_skills().into_iter().map(|s| s.name).collect();
            format!(
                "错误：未找到技能 '{}'。可用技能: {:?}",
                skill_name, available
            )
        }
    }
}
