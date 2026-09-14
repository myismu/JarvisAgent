//! # switch_mode.rs — 工作模式切换工具
//!
//! Agent 可在运行中通过此工具切换 WorkMode（edit/plan），Audience（user/developer）不变。
//!
//! Agent 只能在 edit（编辑）与 plan（规划）之间切换；
//! 「权限档位」（请求审批 / 帮我批准）由用户在界面上控制，不属于本工具的职责范围。

use crate::core::tools::framework;
use serde_json::json;
use tauri::{Emitter, Manager};

/// 切换 Agent 工作模式（edit / plan，只改 WorkMode，不改 Audience）
pub async fn switch_work_mode(
    app: &tauri::AppHandle,
    input: &serde_json::Value,
    session_id: &str,
) -> framework::ToolCallResult {
    let session_manager = app.state::<crate::infra::state::state::SessionManager>();
    let ctx = session_manager.get_or_create(session_id).await;
    let current_mode = ctx.agent_work_mode.lock().await.clone();
    let target_mode = input["mode"].as_str().unwrap_or("edit").to_string();
    let reason = input["reason"].as_str().unwrap_or("").to_string();

    if !["edit", "plan"].contains(&target_mode.as_str()) {
        return framework::ToolCallResult::error(format!(
            "错误：不支持的工作模式「{}」。本工具只能切换 edit（编辑）和 plan（规划）。\
权限档位（请求审批 / 帮我批准）由用户在界面上控制，Agent 不能自行切换。",
            target_mode
        ));
    }

    if current_mode == target_mode {
        return framework::ToolCallResult::ok(format!("当前已经处于「{}」模式，无需切换。", current_mode));
    }

    *ctx.agent_work_mode.lock().await = target_mode.clone();

    let _ = app.emit(
        "agent-work-mode-changed",
        json!({
            "sessionId": session_id,
            "from": current_mode,
            "to": target_mode,
            "reason": reason,
        }),
    );

    framework::ToolCallResult::ok(format!(
        "已从「{}」模式切换到「{}」模式。{}\n\n【系统通知】工作模式已变化：本回合后续按「{}」模式规则执行，可用延迟工具与能力边界已相应变化；请立即调用 GetToolCatalog 重新获取当前模式下的延迟工具目录，并以最新目录为准，不要继续沿用切换前的旧目录。",
        current_mode,
        target_mode,
        if reason.is_empty() {
            String::new()
        } else {
            format!("原因：{}", reason)
        },
        target_mode
    ))
}
