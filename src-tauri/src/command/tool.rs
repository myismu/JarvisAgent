//! # tool.rs — 工具开关相关命令
//!
//! 让用户在设置面板里逐个启停工具。**关掉 = 模型完全看不到该工具** ——
//! 它不进 `tools` 参数、按需工具目录里也搜不到，而不是"在、但调用被拒"那种软禁用。
//!
//! ## 关键导出
//! - `list_tools()`: 列出全部工具（名称 / 描述 / 分类 / 是否按需 / 是否启用）
//! - `set_tool_active()`: 启用或停用某个工具
//!
//! ## 依赖
//! - Internal: `core::tools::framework::registry`（工具注册表）、`command::app_config`（持久化）
//!
//! ## 约束
//! - 状态存在 `app-config.json` 的 `tools` 字段，与技能那套同构：**只存被关掉的**，
//!   未记录的一律默认启用。
//! - **生效时机是"新会话"**：核心工具的 schema 必须会话内字节恒定，否则每轮
//!   `tools` 参数一变、prompt cache 整体失效。要让当前会话立刻用上，得清会话快照
//!   重建（后续阶段的 `apply_tool_filter_now`）。
//! - ⚠️ 本开关**不是安全边界**：它管的是"模型看不看得见"，不是"准不准做"。
//!   真正的闸门是 `policy.rs` 的权限策略与只读保护 —— 用户关掉某个工具只是不想让它
//!   出现在选项里，不构成任何防护承诺。

use crate::core::tools::framework::registry::ToolRegistry;
use serde::Serialize;

/// 工具的元数据（供设置面板渲染开关列表）。
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ToolMeta {
    pub name: String,
    /// 一句话简述（`ToolDef.description`），列表上直接显示
    pub description: String,
    pub category: String,
    /// true = 按需工具（需 GetToolCatalog → DiscoverTools 三步才能用）。
    /// UI 据此分组：核心工具常驻 schema，按需工具按需发现。
    pub deferred: bool,
    /// 是否启用。未在配置里出现过的工具默认启用。
    pub enabled: bool,
    /// 完整 JSON Schema（含参数名 / 类型 / 必填 / 参数说明），供界面上"展开看详情"。
    ///
    /// 直接透传注册表里那份 —— 它就是模型实际收到的东西，所以界面上看到的和模型
    /// 看到的一定一致，不会出现"文档说一套、模型收另一套"。
    pub schema: serde_json::Value,
}

/// 列出全部已注册工具（含启用状态）。
#[tauri::command]
pub async fn list_tools() -> Result<Vec<ToolMeta>, String> {
    let registry = ToolRegistry::global();
    let states = crate::command::app_config::get_all_tool_states();

    let mut tools: Vec<ToolMeta> = registry
        .all_tool_names()
        .into_iter()
        .filter_map(|name| registry.get(name))
        .map(|def| ToolMeta {
            name: def.name.to_string(),
            description: def.description.to_string(),
            category: def.category.to_string(),
            deferred: def.should_defer,
            enabled: states.get(def.name).copied().unwrap_or(true),
            schema: def.schema.clone(),
        })
        .collect();

    // 按分类 + 名称排序：UI 直接渲染，不必自己排
    tools.sort_by(|a, b| a.category.cmp(&b.category).then_with(|| a.name.cmp(&b.name)));
    Ok(tools)
}

/// 启用或停用某个工具。
///
/// ⚠️ 改完只对**新会话**生效（核心工具 schema 必须会话内字节恒定，详见文件头约束）。
/// 想让当前会话立刻用上，调 [`apply_tool_filter_now`]。
#[tauri::command]
pub async fn set_tool_active(tool_name: String, enabled: bool) -> Result<(), String> {
    crate::command::app_config::set_tool_enabled(&tool_name, enabled)
}

// 这里曾有一个 `apply_tool_filter_now`（丢掉会话快照、让当前会话立刻用上最新开关）。
// 已删除（2026-09-21）：那个按钮摆在设置页上会暗示"不点就不生效"，而实际上新会话
// 本来就会生效 —— 它制造的是误导，不是能力。真要验证效果，开个新会话的成本远比
// "烧一次 prompt cache + 多一个生效时机的概念"低。
//
// `SessionContext::reset_tool_filter()` 保留着（快照机制该有对称的清除操作），
// 只是目前没有调用者。
