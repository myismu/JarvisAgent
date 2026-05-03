//! # plan.rs — 方案审批工具
//!
//! 推送实施方案到前端预览面板，通过 oneshot channel 阻塞等待用户决策。
//!
//! ## 关键导出
//! - `propose_plan()`: 方案审批工具

use serde_json::json;
use tauri::{Emitter, Manager};

use crate::core::models::PlanDocument;

/// 方案审批工具：推送方案到前端预览面板，通过 oneshot channel 阻塞等待用户决策
pub async fn propose_plan(
    app: &tauri::AppHandle,
    input: &serde_json::Value,
    session_id: &str,
) -> String {
    let title = input["title"]
        .as_str()
        .or_else(|| input["plan_title"].as_str())
        .unwrap_or("实施方案");
    let mut content = input["content"]
        .as_str()
        .or_else(|| input["plan_content"].as_str())
        .or_else(|| input["plan_description"].as_str())
        .unwrap_or("")
        .to_string();

    if let Some(tasks) = input["task_breakdown"].as_array() {
        if !tasks.is_empty() {
            content.push_str("\n\n## 任务分解\n\n```json\n");
            content.push_str(&serde_json::to_string_pretty(tasks).unwrap_or_default());
            content.push_str("\n```\n");
        }
    } else if let Some(tasks) = input["task_breakdown"].as_str() {
        if !tasks.trim().is_empty() {
            content.push_str("\n\n## 任务分解\n\n");
            content.push_str(tasks);
            content.push('\n');
        }
    }

    if let Some(estimated_time) = input["estimated_time"].as_str() {
        if !estimated_time.trim().is_empty() {
            content.push_str("\n\n## 预估时间\n\n");
            content.push_str(estimated_time);
            content.push('\n');
        }
    }

    if content.trim().is_empty() {
        return "错误：方案内容不能为空。".to_string();
    }

    // 生成唯一 ID
    let session_manager = app.state::<crate::core::state::SessionManager>();
    let ctx = session_manager.get_or_create(session_id).await;
    let plan_fingerprint = format!(
        "{}:{}",
        title.trim().to_ascii_lowercase(),
        crate::core::state::stable_hash(&content)
    );
    {
        let memory = ctx.memory.lock().await;
        if let Some(existing) = memory
            .plan_documents
            .iter()
            .find(|item| item.status == "pending" && item.title == title && item.content == content)
        {
            return format!(
                "Duplicate propose_plan blocked: pending plan '{}' already exists as {}. Reuse the existing pending plan instead of creating another approval request.",
                existing.title, existing.id
            );
        }
    }
    {
        let mut pending = ctx.pending_plan_state.lock().await;
        if let Some(entry) = pending.get_mut(&plan_fingerprint) {
            entry.suppressed_count += 1;
            return format!(
                "Duplicate propose_plan blocked: pending plan '{}' already exists as {}. Do not open another identical approval panel. Suppressed duplicate #{}.",
                entry.title, entry.id, entry.suppressed_count
            );
        }
        pending.insert(
            plan_fingerprint.clone(),
            crate::core::state::PendingPlanCacheEntry {
                display: title.to_string(),
                title: title.to_string(),
                id: "pending".to_string(),
                suppressed_count: 0,
            },
        );
    }

    use std::sync::atomic::{AtomicUsize, Ordering};
    static PLAN_REQ_ID: AtomicUsize = AtomicUsize::new(1);
    let id = format!("plan_{}", PLAN_REQ_ID.fetch_add(1, Ordering::SeqCst));
    if let Some(entry) = ctx
        .pending_plan_state
        .lock()
        .await
        .get_mut(&plan_fingerprint)
    {
        entry.id = id.clone();
    }

    // 创建 oneshot channel 等待用户决策
    let (tx, rx) = tokio::sync::oneshot::channel();
    ctx.pending_permissions.lock().await.insert(id.clone(), tx);

    // Plan documents are persisted through session memory in SQLite.
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs();
    let plan_document = PlanDocument {
        id: id.clone(),
        session_id: session_id.to_string(),
        title: title.to_string(),
        content: content.clone(),
        status: "pending".to_string(),
        path: None,
        created_at: now,
        updated_at: now,
        decided_at: None,
    };
    {
        let mut memory = ctx.memory.lock().await;
        if let Some(existing) = memory
            .plan_documents
            .iter_mut()
            .find(|item| item.id == plan_document.id)
        {
            *existing = plan_document.clone();
        } else {
            memory.plan_documents.push(plan_document.clone());
        }
    }
    let _ = crate::core::session::upsert_plan_document(session_id, plan_document.clone());

    crate::jarvis_debug!(
        "JARVIS",
        "[JARVIS] 方案已推送到前端预览: {} ({})",
        title,
        id
    );

    let _ = app.emit("plan-document-updated", &plan_document);

    // 发送事件到前端，触发方案预览面板
    let _ = app.emit(
        "plan-proposal",
        json!({
            "id": id,
            "title": title,
            "content": content,
            "sessionId": session_id
        }),
    );

    // 在聊天流中也提示一下
    let _ = app.emit(
        "chat-stream",
        json!({
            "content": format!("\n> 📋 **方案已提交审阅**: 「{}」\n> 请在弹出的方案预览面板中查看详情并决策。\n", title),
            "sessionId": session_id
        }),
    );

    // 阻塞等待用户决策（通过 resolve_permission 回调）
    let decision = rx.await.unwrap_or_else(|_| "reject".to_string());

    // 解析决策和可能的修改内容
    let (final_decision, modified_content) = if decision.contains("|||") {
        let parts: Vec<&str> = decision.splitn(2, "|||").collect();
        (parts[0].to_string(), Some(parts[1].to_string()))
    } else {
        (decision, None)
    };
    ctx.pending_plan_state
        .lock()
        .await
        .remove(&plan_fingerprint);
    let decided_status = if final_decision == "reject" {
        "rejected"
    } else {
        "approved"
    };
    let decided_at = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs();
    let decided_doc = {
        let mut memory = ctx.memory.lock().await;
        memory
            .plan_documents
            .iter_mut()
            .find(|item| item.id == id)
            .map(|item| {
                item.status = decided_status.to_string();
                item.updated_at = decided_at;
                item.decided_at = Some(decided_at);
                item.clone()
            })
    };
    if let Some(doc) = decided_doc {
        let _ = crate::core::session::upsert_plan_document(session_id, doc.clone());
        let _ = app.emit("plan-document-updated", &doc);
    }

    if final_decision == "reject" {
        crate::jarvis_info!("JARVIS", "[JARVIS] 用户拒绝了方案: {}", title);
        format!("用户已拒绝此方案「{}」。请根据用户意见进行调整，或询问用户想要修改的部分。严禁继续创建 task_create 任务！", title)
    } else {
        crate::jarvis_info!("JARVIS", "[JARVIS] 用户同意了方案: {}", title);
        if let Some(content) = modified_content {
            format!("用户已同意方案「{}」并做了修改！修改后的方案内容：\n\n{}\n\n现在请使用 task_create 创建完整持久化任务图，使用 task_update 设置真实依赖关系，最后调用 run_tasks 一次性启动调度器。只有单个临时委派才直接使用 task。", title, content)
        } else {
            format!("用户已同意方案「{}」！现在请使用 task_create 创建完整持久化任务图，使用 task_update 设置真实依赖关系，最后调用 run_tasks 一次性启动调度器。只有单个临时委派才直接使用 task。", title)
        }
    }
}
