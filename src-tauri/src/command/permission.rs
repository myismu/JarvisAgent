//! # permission.rs — 权限确认与取消 Tauri 命令
//!
//! 处理前端用户对工具执行权限的审批决策（allow/reject），
//! 以及 Agent 执行的取消操作（含级联取消子 Agent）。
//!
//! ## 关键导出
//! - `resolve_permission()`: 前端提交权限决策，支持方案审批（plan_*）和普通权限
//! - `cancel_jarvis()`: 取消当前 Agent 执行，清理所有待处理权限和子 Agent

use crate::infra::state::state::SessionManager;
use tauri::Emitter;

/// 前端提交权限决策，通过 oneshot channel 通知等待中的 Agent。
/// 方案审批采用产品层状态机：只更新方案状态，后续由前端发起新的用户轮次。
#[tauri::command]
pub async fn resolve_permission(
    id: String,
    session_id: String,
    decision: String,
    content: Option<String>,
    session_manager: tauri::State<'_, SessionManager>,
    app: tauri::AppHandle,
) -> Result<serde_json::Value, String> {
    let ctx = session_manager.get_or_create(&session_id).await;
    if id.starts_with("plan_") {
        let status = if decision == "allow" { "approved" } else { "revision_requested" };
        if let Ok(Some(doc)) = crate::core::session::update_plan_document_status(
            &session_id, &id, status, content.clone(),
        ) {
            {
                let mut memory = ctx.memory.lock().await;
                if let Some(existing) = memory.plan_documents.iter_mut().find(|item| item.id == doc.id) {
                    *existing = doc.clone();
                } else {
                    memory.plan_documents.push(doc.clone());
                }
            }
            let _ = app.emit("plan-document-updated", &doc);
        }
    }

    let channel_alive = if let Some((_, tx)) = ctx.pending_permissions.lock().await.remove(&id) {
        let resp = if let Some(ref mc) = content { format!("{}|||{}", decision, mc) } else { decision.clone() };
        tx.send(resp).is_ok()
    } else {
        false
    };

    // 产品层审批：方案决策只更新方案文档/清理权限通道，后续由前端发起新的用户轮次。
    // 同时清理 cancel_token，确保审批续跑的 ask_jarvis 不会因 has_active_run 被拦截。
    if id.starts_with("plan_") {
        *ctx.cancel_token.lock().await = None;
        return Ok(serde_json::json!({ "needsResume": false }));
    }

    // 循环上限续跑：超时后用户点了"允许"，channel 已死但标记还在 → 通知前端 resume
    if !channel_alive && decision == "allow" {
        let pending = *ctx.loop_continuation_pending.lock().await;
        if pending {
            *ctx.loop_continuation_pending.lock().await = false;
            *ctx.cancel_token.lock().await = None;
            return Ok(serde_json::json!({
                "needsResume": true,
                "resumeWith": "用户已授权继续执行，请继续之前未完成的任务。"
            }));
        }
    }

    Ok(serde_json::json!({ "needsResume": false }))
}

/// 取消 Agent 执行：触发取消令牌、拒绝所有待处理权限、级联取消子 Agent
#[tauri::command]
pub async fn cancel_jarvis(
    session_id: String,
    session_manager: tauri::State<'_, SessionManager>,
    app: tauri::AppHandle,
) -> Result<(), String> {
    println!("[JARVIS] 收到取消请求: {}", session_id);
    let ctx = session_manager.get_or_create(&session_id).await;
    if let Some(token) = ctx.cancel_token.lock().await.as_ref() {
        token.cancel();
    }
    *ctx.cancel_token.lock().await = None;
    // 拒绝所有等待用户决策的权限请求
    let pending = ctx
        .pending_permissions
        .lock()
        .await
        .drain()
        .collect::<Vec<_>>();
    for (_, (_, tx)) in pending {
        let _ = tx.send("reject".to_string());
    }
    // 级联取消该会话下所有运行中的子 Agent
    let cancelled_subagents =
        crate::core::orchestration::subagents::SubAgentMonitor::cancel_session(&app, &session_id)
            .await;
    if !cancelled_subagents.is_empty() {
        println!(
            "[JARVIS] Cancelled {} running subagent(s) for session {}",
            cancelled_subagents.len(),
            session_id
        );
    }
    Ok(())
}

/// 查询当前会话的权限状态
#[tauri::command]
pub async fn get_permission_state(
    session_id: String,
    session_manager: tauri::State<'_, SessionManager>,
) -> Result<serde_json::Value, String> {
    let ctx = session_manager.get_or_create(&session_id).await;
    let session_allowed = *ctx.session_allowed.lock().await;
    let pending_count = ctx.pending_permissions.lock().await.len();
    Ok(serde_json::json!({
        "sessionAllowed": session_allowed,
        "pendingCount": pending_count,
    }))
}

/// 撤销"允许本次会话"授权
#[tauri::command]
pub async fn revoke_session_permission(
    session_id: String,
    session_manager: tauri::State<'_, SessionManager>,
) -> Result<(), String> {
    let ctx = session_manager.get_or_create(&session_id).await;
    *ctx.session_allowed.lock().await = false;
    Ok(())
}
