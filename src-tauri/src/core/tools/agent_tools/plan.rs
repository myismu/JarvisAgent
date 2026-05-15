//! # plan.rs — 方案审批工具
//!
//! 推送实施方案到前端预览面板，通过 oneshot channel 阻塞等待用户决策。
//!
//! ## 关键导出
//! - `propose_plan()`: 方案审批工具

use serde_json::json;
use tauri::{Emitter, Manager};

use crate::infra::types::models::PlanDocument;

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
    let session_manager = app.state::<crate::infra::state::state::SessionManager>();
    let ctx = session_manager.get_or_create(session_id).await;
    let plan_fingerprint = format!(
        "{}:{}",
        title.trim().to_ascii_lowercase(),
        crate::infra::state::state::stable_hash(&content)
    );
    {
        let memory = ctx.memory.lock().await;
        if let Some(existing) = memory
            .plan_documents
            .iter()
            .find(|item| item.status == "pending" && item.title == title && item.content == content)
        {
            return format!(
                "Duplicate ProposePlan blocked: pending plan '{}' already exists as {}. Reuse the existing pending plan instead of creating another approval request.",
                existing.title, existing.id
            );
        }
    }
    {
        let mut pending = ctx.pending_plan_state.lock().await;
        if let Some(entry) = pending.get_mut(&plan_fingerprint) {
            entry.suppressed_count += 1;
            return format!(
                "Duplicate ProposePlan blocked: pending plan '{}' already exists as {}. Do not open another identical approval panel. Suppressed duplicate #{}.",
                entry.title, entry.id, entry.suppressed_count
            );
        }
        pending.insert(
            plan_fingerprint.clone(),
            crate::infra::state::state::PendingPlanCacheEntry {
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

    // 创建 oneshot channel（保留通道用于 resolve_permission 检测 Agent 是否存活）
    let (tx, _rx) = tokio::sync::oneshot::channel();
    {
        let mut perms = ctx.pending_permissions.lock().await;
        let now = std::time::Instant::now();
        perms.retain(|_, (ts, _)| now.duration_since(*ts).as_secs() < 300);
        perms.insert(id.clone(), (std::time::Instant::now(), tx));
    }

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

    // 方案审批可能持续较长时间，立即 flush 会话消息到 DB，防止切换会话时丢失
    {
        let memory = ctx.memory.lock().await;
        let _ = crate::core::session::save_session(session_id, &memory, None);
    }

    // 断点续传：方案已保存，Agent 主动停止。用户决策触发新 ask_jarvis。
    crate::jarvis_info!("JARVIS", "[JARVIS] 方案已提交，Agent 暂停等待审批: {} ({})", title, id);
    format!("方案「{}」已提交审批面板。Agent 已停止等待用户决策。用户决策后将自动断点续传。", title)
}
