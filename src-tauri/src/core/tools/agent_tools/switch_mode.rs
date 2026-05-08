//! # switch_mode.rs — 工作模式切换工具
//!
//! Agent 可在运行中通过此工具切换 WorkMode（chat/edit/plan），
//! Audience（user/developer）不变。

use serde_json::json;
use tauri::{Emitter, Manager};

/// 切换 Agent 工作模式（只改 WorkMode，不改 Audience）
pub async fn switch_work_mode(
    app: &tauri::AppHandle,
    input: &serde_json::Value,
    session_id: &str,
) -> String {
    let session_manager = app.state::<crate::core::state::SessionManager>();
    let ctx = session_manager.get_or_create(session_id).await;
    let current_mode = ctx.agent_work_mode.lock().await.clone();
    let target_mode = input["mode"].as_str().unwrap_or("edit").to_string();
    let reason = input["reason"].as_str().unwrap_or("").to_string();

    if !["chat", "edit", "plan"].contains(&target_mode.as_str()) {
        return format!(
            "错误：不支持的工作模式「{}」。支持的模式：chat、edit、plan。",
            target_mode
        );
    }

    if current_mode == target_mode {
        return format!("当前已经处于「{}」模式，无需切换。", current_mode);
    }

    // Chat 禁止切到 Plan
    if current_mode == "chat" && target_mode == "plan" {
        return "错误：聊天模式下不能切换到计划模式。请先切换到编辑模式。".to_string();
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

    format!(
        "已从「{}」模式切换到「{}」模式。{}",
        current_mode,
        target_mode,
        if reason.is_empty() {
            String::new()
        } else {
            format!("原因：{}", reason)
        }
    )
}
