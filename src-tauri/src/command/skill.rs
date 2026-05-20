//! # skill.rs — Skill 管理 Tauri 命令
//!
//! 提供前端 Skill 管理界面所需的命令：列出所有 skill、获取 skill 详情。
//!
//! ## 关键导出
//! - `list_skills()`: 返回所有 skill 的元数据列表（不含 body）
//! - `get_skill_detail()`: 返回指定 skill 的完整详情（含 body）
//!
//! ## 依赖
//! - Internal: `core::tools::load_all_skills`, `infra::types::models`
//! - External: `tiktoken_rs`

use crate::core::tools::load_all_skills;
use crate::infra::types::models::{SkillMeta, SkillDetail};
use tiktoken_rs::cl100k_base;

/// 计算文本的 token 数量
///
/// 使用 cl100k_base tokenizer（GPT-4 / Claude 使用的编码方式）。
/// 如果 tokenizer 初始化失败，回退到字符数估算。
fn count_tokens(text: &str) -> usize {
    match cl100k_base() {
        Ok(bpe) => bpe.encode_with_special_tokens(text).len(),
        Err(_) => text.chars().count(), // fallback
    }
}

/// 获取所有 skill 的元数据列表
///
/// 返回 SkillMeta 列表，不含完整 body 内容，适用于列表展示。
/// body_tokens 是 token 数量，反映 LLM 实际处理的 token 开销。
#[tauri::command]
pub async fn list_skills() -> Result<Vec<SkillMeta>, String> {
    let skills = load_all_skills();
    let activations = crate::command::app_config::get_all_skill_activations();
    Ok(skills.into_iter().map(|s| {
        let active = activations.get(&s.name).copied().unwrap_or(true);
        SkillMeta {
            name: s.name,
            description: s.description,
            path: s.path,
            body_tokens: count_tokens(&s.body),
            active,
        }
    }).collect())
}

/// 获取指定 skill 的完整详情
///
/// 根据 skill 名称查找并返回完整详情，包含 body 内容。
/// 如果未找到对应 skill，返回 None。
#[tauri::command]
pub async fn get_skill_detail(name: String) -> Result<Option<SkillDetail>, String> {
    let skills = load_all_skills();
    Ok(skills.into_iter().find(|s| s.name == name).map(|s| SkillDetail {
        name: s.name,
        description: s.description,
        path: s.path,
        body: s.body,
    }))
}
