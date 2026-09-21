//! # prompt.rs — 提示词管理 Tauri 命令
//!
//! 设置页「提示词」tab 的后端：列出/查看/保存/重置 13 个提示词文件，
//! 以及拼装后的 system 全文预览。
//!
//! ## 架构（见 doc/提示词磁盘化-可视化编辑方案.md）
//! - 内置版：`include_str!` 编译期嵌进 exe（出厂默认，兜底）；
//! - 用户版：`data/prompts/<path>` 磁盘覆盖层，**懒落盘**——只有点保存才写；
//! - 恢复默认 = 删磁盘文件；列表以代码内 `PROMPT_FILES` 注册表为准，不扫描目录。
//!
//! ## 依赖
//! - Internal: `core::agent::prompts`（注册表 + resolve + 组装）
//! - External: 无（直接 std::fs）

use crate::core::agent::prompts::{
    embedded_prompt, get_system_prompt, has_disk_override, is_registered_prompt, prompt_disk_dir,
    resolve_prompt, PromptCategory, PromptFileMeta, PROMPT_FILES,
};
use serde::Serialize;

/// 列表项：注册表元数据 + 磁盘覆盖状态 + 当前生效版体积
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PromptMeta {
    pub path: String,
    pub display_name: String,
    pub description: String,
    /// UI 分组："base" / "audience" / "mode" / "os" / "subagent"
    pub category: String,
    /// true = 进 system（新会话生效）；false = 进动态上下文（下一轮生效）
    pub goes_into_system: bool,
    /// 磁盘上是否有用户自定义版
    pub customized: bool,
    /// 当前生效版（磁盘版或内置版）的字符数
    pub size_chars: usize,
    /// 仅 os 分类有意义：是否为当前编译平台（UI 标注"本机"）
    pub is_platform_active: bool,
}

/// 详情：编辑器初始内容 + 内置原文（供对比/恢复参考）
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PromptDetail {
    pub path: String,
    pub display_name: String,
    pub description: String,
    pub goes_into_system: bool,
    /// 当前生效版（磁盘有则磁盘版，否则内置版）——编辑器初始内容
    pub current_content: String,
    /// 内置出厂版原文（"恢复默认"的预览参照）
    pub embedded_content: String,
    pub customized: bool,
}

/// os 分类的"本机"标注：path 里的平台段与编译目标一致才算激活。
fn is_platform_active(meta: &PromptFileMeta) -> bool {
    if meta.category != PromptCategory::Os {
        return false;
    }
    if cfg!(target_os = "windows") {
        meta.path.ends_with("windows.md")
    } else if cfg!(target_os = "macos") {
        meta.path.ends_with("macos.md")
    } else {
        meta.path.ends_with("linux.md")
    }
}

fn char_len(s: &str) -> usize {
    s.chars().count()
}

/// 列出全部提示词文件（以注册表为准，不扫描磁盘目录）
#[tauri::command]
pub async fn list_prompts() -> Result<Vec<PromptMeta>, String> {
    Ok(PROMPT_FILES
        .iter()
        .map(|meta| {
            let current = resolve_prompt(meta.path);
            PromptMeta {
                path: meta.path.to_string(),
                display_name: meta.display_name.to_string(),
                description: meta.description.to_string(),
                category: meta.category.as_str().to_string(),
                goes_into_system: meta.goes_into_system,
                customized: has_disk_override(meta.path),
                size_chars: char_len(&current),
                is_platform_active: is_platform_active(meta),
            }
        })
        .collect())
}

/// 获取单个提示词详情（编辑器初始内容 = 当前生效版）
#[tauri::command]
pub async fn get_prompt_detail(path: String) -> Result<PromptDetail, String> {
    let meta: &PromptFileMeta = PROMPT_FILES
        .iter()
        .find(|m| m.path == path)
        .ok_or_else(|| format!("未注册的提示词文件：{}", path))?;
    let customized = has_disk_override(meta.path);
    Ok(PromptDetail {
        path: meta.path.to_string(),
        display_name: meta.display_name.to_string(),
        description: meta.description.to_string(),
        goes_into_system: meta.goes_into_system,
        current_content: resolve_prompt(meta.path).into_owned(),
        embedded_content: embedded_prompt(meta.path).to_string(),
        customized,
    })
}

/// 保存用户自定义版（懒落盘：这一步才真正写 data/prompts/）
#[tauri::command]
pub async fn save_prompt(path: String, content: String) -> Result<(), String> {
    // 白名单：path 必须精确命中注册表，杜绝 "../" 穿越写出 data 目录
    if !is_registered_prompt(&path) {
        return Err(format!("未注册的提示词文件，拒绝保存：{}", path));
    }
    if content.trim().is_empty() {
        return Err("提示词内容不能为空（想恢复默认请用「恢复默认」删除自定义版）".to_string());
    }
    let target = prompt_disk_dir().join(&path);
    if let Some(parent) = target.parent() {
        std::fs::create_dir_all(parent).map_err(|e| format!("创建目录失败：{}", e))?;
    }
    std::fs::write(&target, content).map_err(|e| format!("写入失败：{}", e))?;
    println!("[PROMPT] 用户自定义提示词已保存：{}", path);
    Ok(())
}

/// 恢复出厂默认（删除磁盘覆盖文件，文件不存在则静默成功）
#[tauri::command]
pub async fn reset_prompt(path: String) -> Result<(), String> {
    if !is_registered_prompt(&path) {
        return Err(format!("未注册的提示词文件：{}", path));
    }
    let target = prompt_disk_dir().join(&path);
    match std::fs::remove_file(&target) {
        Ok(()) => {
            println!("[PROMPT] 提示词已恢复出厂默认：{}", path);
            Ok(())
        }
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(e) => Err(format!("删除失败：{}", e)),
    }
}

/// 预览拼装结果（当前磁盘状态下的真实产物，只读）。
///
/// - `target = "main"`：主 Agent 的 system（`get_system_prompt`）；
///   注意工作模式**不影响** system（模式规则进动态上下文），忽略 `work_mode`。
/// - `target = "subagent"`：子代理的 system（`get_subagent_system_prompt`），
///   与主 Agent 是两份独立拼装；忽略 `work_mode`。
/// - `target = "dynamic"`：随用户消息注入的动态上下文（`build_dynamic_context`，
///   按"项目操作 ACTION"场景拼装最全形态）。**`work_mode` 在这里有效**——
///   模式规则（mode_rules）是动态上下文的一部分，随 edit/plan 而变。
///
/// `workspace` 传项目路径可预览沙箱形态 + 项目结构；传空则按非沙箱会话拼装。
#[tauri::command]
pub async fn get_assembled_system_prompt(
    target: String,
    audience: String,
    work_mode: Option<String>,
    workspace: Option<String>,
) -> Result<String, String> {
    let ws_buf = workspace
        .map(std::path::PathBuf::from)
        .filter(|p| !p.as_os_str().is_empty());
    let mode = match work_mode.as_deref() {
        Some("plan") => "plan",
        _ => "edit",
    };
    match target.as_str() {
        // 子代理：cwd 语义与真实调用一致（run_subagent：无工作区时给明确占位说明）
        "subagent" => {
            let cwd = ws_buf
                .as_ref()
                .map(|p| p.to_string_lossy().to_string())
                .unwrap_or_else(|| "No session workspace is configured".to_string());
            let ws_str = ws_buf.as_ref().map(|p| p.to_string_lossy().to_string());
            Ok(crate::core::agent::prompts::get_subagent_system_prompt(
                &cwd,
                ws_str.as_deref(),
            ))
        }
        // 动态上下文：按 ACTION（项目操作）场景拼装最全形态；模式规则随所选模式变
        "dynamic" => {
            // 预览路径没有会话上下文，用 allow_all：这里展示的是"提示词模板长什么样"，
            // 不是某个会话的实际生效形态（真实会话走 pipeline，那里传的是会话快照）。
            let caps = crate::core::tools::framework::capabilities::Capabilities::for_work_mode(
                mode,
                &crate::core::tools::framework::registry::ToolFilter::allow_all(),
            );
            Ok(crate::core::agent::build_dynamic_context(
                "ACTION",
                &ws_buf,
                &caps,
                mode,
                0,
            ))
        }
        _ => Ok(get_system_prompt(&audience, "edit", ws_buf.as_deref())),
    }
}
