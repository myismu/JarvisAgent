use crate::core::state::SessionManager;
use tauri::Emitter;

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
        let status = if decision == "allow" { "approved" } else { "rejected" };
        if let Ok(Some(document)) = crate::core::sessions::update_plan_document_status(
            &session_id,
            &id,
            status,
            content.clone(),
        ) {
            {
                let mut memory = ctx.memory.lock().await;
                if let Some(existing) = memory.plan_documents.iter_mut().find(|item| item.id == document.id) {
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
    let pending = ctx.pending_permissions.lock().await.drain().collect::<Vec<_>>();
    for (_, tx) in pending {
        let _ = tx.send("reject".to_string());
    }
    let cancelled_subagents =
        crate::core::subagents::SubAgentMonitor::cancel_session(&app, &session_id).await;
    if !cancelled_subagents.is_empty() {
        println!(
            "[JARVIS] Cancelled {} running subagent(s) for session {}",
            cancelled_subagents.len(),
            session_id
        );
    }
    Ok(())
}
