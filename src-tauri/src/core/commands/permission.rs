//! # permission.rs — 权限确认与取消 Tauri 命令
//!
//! 处理前端用户对工具执行权限的审批决策（allow/reject），
//! 以及 Agent 执行的取消操作（含级联取消子 Agent）。
//!
//! ## 关键导出
//! - `resolve_permission()`: 前端提交权限决策，支持方案审批（plan_*）和普通权限
//! - `cancel_jarvis()`: 取消当前 Agent 执行，清理所有待处理权限和子 Agent

use crate::core::state::SessionManager;
use tauri::Emitter;

/// 前端提交权限决策，通过 oneshot channel 通知等待中的 Agent
#[tauri::command]
pub async fn resolve_permission(
    id: String,
    session_id: String,
    decision: String,
    content: Option<String>,
    session_manager: tauri::State<'_, SessionManager>,
    app: tauri::AppHandle,
) -> Result<(), String> {
    let ctx = session_manager.get_or_create(&session_id).await;
    if id.starts_with("plan_") {
        let status = if decision == "allow" {
            "approved"
        } else {
            "rejected"
        };
        if let Ok(Some(document)) = crate::core::session::update_plan_document_status(
            &session_id,
            &id,
            status,
            content.clone(),
        ) {
            {
                let mut memory = ctx.memory.lock().await;
                if let Some(existing) = memory
                    .plan_documents
                    .iter_mut()
                    .find(|item| item.id == document.id)
                {
                    *existing = document.clone();
                } else {
                    memory.plan_documents.push(document.clone());
                }
            }
            let _ = app.emit("plan-document-updated", document);
        }
    }

    if let Some(tx) = ctx.pending_permissions.lock().await.remove(&id) {
        let response = if let Some(modified_content) = content {
            format!("{}|||{}", decision, modified_content)
        } else {
            decision
        };
        let _ = tx.send(response);
    }
    Ok(())
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
    // 触发取消令牌，Agent 主循环会在下一次检查点退出
    if let Some(token) = ctx.cancel_token.lock().await.as_ref() {
        token.cancel();
    }
    // 拒绝所有等待用户决策的权限请求
    let pending = ctx
        .pending_permissions
        .lock()
        .await
        .drain()
        .collect::<Vec<_>>();
    for (_, tx) in pending {
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
