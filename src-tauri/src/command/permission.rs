//! # permission.rs — 权限确认与取消 Tauri 命令
//!
//! 处理前端用户对工具执行权限的审批决策，以及 Agent 执行的取消操作（含级联取消子 Agent）。
//!
//! 决策协议（结构化，不走字符串拼接）：
//! - `allow` / `allow_session` → 放行本次 / 放行本会话
//! - `reject` + 可选 `content`（用户的拒绝说明）→ 明确拒绝，说明会回灌给模型
//! - 取消/中断 → `Interrupted`，与"用户拒绝"区分开
//!
//! ## 关键导出
//! - `resolve_permission()`: 前端提交权限决策，支持方案审批（plan_*）和普通权限
//! - `cancel_jarvis()`: 取消当前 Agent 执行，清理所有待处理权限和子 Agent

use crate::core::tools::framework::permission::PermissionDecision;
use crate::infra::state::state::SessionManager;
use tauri::Emitter;

/// 把前端传来的 decision/content 解析成结构化决策。
/// 未知取值一律按"明确拒绝"处理（安全默认），并保留用户说明。
fn parse_decision(decision: &str, content: Option<String>) -> PermissionDecision {
    let feedback = content
        .map(|text| text.trim().to_string())
        .filter(|text| !text.is_empty());
    match decision {
        "allow" => PermissionDecision::Allow,
        "allow_session" => PermissionDecision::AllowSession,
        _ => PermissionDecision::Reject { feedback },
    }
}

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

    let parsed = parse_decision(&decision, content.clone());
    let channel_alive = if let Some(entry) = ctx.pending_permissions.lock().await.remove(&id) {
        entry.responder.send(parsed.clone()).is_ok()
    } else {
        false
    };

    // 产品层审批：方案决策只更新方案文档/清理权限通道，后续由前端发起新的用户轮次。
    // 同时清理 cancel_token，确保审批续跑的 ask_jarvis 不会因 has_active_run 被拦截。
    if id.starts_with("plan_") {
        *ctx.cancel_token.lock().await = None;
        let _ = app.emit("permission-resolved", serde_json::json!({
            "id": id,
            "sessionId": session_id,
            "decision": parsed.status_label(),
            "decisionText": parsed.model_note(),
        }));
        return Ok(serde_json::json!({ "needsResume": false }));
    }

    // 循环上限续跑：请求此前因中断（取消/通道关闭）结束、channel 已死，但续跑标记还在，
    // 用户此时点"允许" → 通知前端用 resume_jarvis 续跑
    if !channel_alive && parsed.is_allowed() {
        let pending = *ctx.loop_continuation_pending.lock().await;
        if pending {
            *ctx.loop_continuation_pending.lock().await = false;
            *ctx.cancel_token.lock().await = None;
            let _ = app.emit("permission-resolved", serde_json::json!({
                "id": id,
                "sessionId": session_id,
                "decision": parsed.status_label(),
                "decisionText": parsed.model_note(),
            }));
            return Ok(serde_json::json!({
                "needsResume": true,
                "resumeWith": "用户已授权继续执行，请继续之前未完成的任务。"
            }));
        }
    }

    let _ = app.emit("permission-resolved", serde_json::json!({
        "id": id,
        "sessionId": session_id,
        "decision": parsed.status_label(),
        "decisionText": parsed.model_note(),
    }));
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
    // 结束所有等待用户决策的权限请求：标记为"中断"而不是"用户拒绝"，
    // 并逐条广播 permission-resolved，让界面上的权限卡立刻消失
    let pending = ctx
        .pending_permissions
        .lock()
        .await
        .drain()
        .collect::<Vec<_>>();
    for (id, entry) in pending {
        let _ = entry.responder.send(PermissionDecision::Interrupted {
            reason: "本轮执行已被取消".to_string(),
        });
        let _ = app.emit(
            "permission-resolved",
            serde_json::json!({
                "id": id,
                "sessionId": session_id,
                "decision": "interrupted",
                "decisionText": "本轮执行已被取消",
            }),
        );
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

/// 查询当前会话的权限状态：待你确认的请求 + 已允许的范围
#[tauri::command]
pub async fn get_permission_state(
    session_id: String,
    session_manager: tauri::State<'_, SessionManager>,
) -> Result<serde_json::Value, String> {
    let ctx = session_manager.get_or_create(&session_id).await;
    // 只统计普通权限请求，排除方案审批（plan proposals 有独立的审批面板）
    let pending_list: Vec<serde_json::Value> = ctx.pending_permissions.lock().await
        .iter()
        .filter(|(k, _)| !k.starts_with("plan_"))
        .map(|(id, entry)| serde_json::json!({
            "id": id,
            "message": entry.message,
            // 只有"工具确认"且这条操作**真的有范围键**时才提供"本次会话都允许"：
            // 循环续跑确认若照抄这套语义，会顺带把其它工具调用一起放行；
            // 没有范围键的操作（例如改工作目录）点了按钮也没有任何效果。
            // 口径与 permission::request_permission 广播的 permission-request 一致。
            "allowSession": entry.kind.allows_session_wide_approval() && entry.allowance.is_some(),
            "kind": entry.kind.as_str(),
        }))
        .collect();
    let allowances: Vec<serde_json::Value> = ctx
        .session_allowances
        .lock()
        .await
        .iter()
        .map(|a| {
            serde_json::json!({
                "kind": a.kind,
                "scope": a.scope,
                "label": a.label,
            })
        })
        .collect();
    Ok(serde_json::json!({
        "allowanceCount": allowances.len(),
        "allowances": allowances,
        "pendingCount": pending_list.len(),
        "pending": pending_list,
    }))
}

/// 查询当前会话的权限设置：档位 + 只读保护 + 已允许范围
#[tauri::command]
pub async fn get_session_permission_settings(
    session_id: String,
    session_manager: tauri::State<'_, SessionManager>,
) -> Result<serde_json::Value, String> {
    let ctx = session_manager.get_or_create(&session_id).await;
    let mode = ctx.approval_mode.lock().await.clone();
    let read_only = ctx.read_only_enabled().await;
    let allowances: Vec<serde_json::Value> = ctx
        .session_allowances
        .lock()
        .await
        .iter()
        .map(|a| {
            serde_json::json!({
                "kind": a.kind,
                "scope": a.scope,
                "label": a.label,
            })
        })
        .collect();
    Ok(serde_json::json!({
        "approvalMode": mode,
        "readOnly": read_only,
        "allowances": allowances,
    }))
}

/// 切换当前会话的权限档位：request_approval（请求审批）/ auto_approve（帮我批准）
#[tauri::command]
pub async fn set_session_approval_mode(
    session_id: String,
    mode: String,
    session_manager: tauri::State<'_, SessionManager>,
    app: tauri::AppHandle,
) -> Result<(), String> {
    if !["request_approval", "auto_approve"].contains(&mode.as_str()) {
        return Err(format!(
            "不支持的权限档位「{}」。支持：request_approval（请求审批）、auto_approve（帮我批准）。",
            mode
        ));
    }
    let ctx = session_manager.get_or_create(&session_id).await;
    *ctx.approval_mode.lock().await = mode.clone();
    let _ = app.emit(
        "approval-mode-changed",
        serde_json::json!({ "sessionId": session_id, "mode": mode }),
    );
    println!("[JARVIS] 会话 {} 权限档位切换为：{}", session_id, mode);

    // 切档位后把**已经挂起**的权限卡按新档位重判一遍：新档位下不用问的直接放行。
    // 没有这一步，已弹出的卡仍然挂在界面上等点击，观感就是"切了档位没反应"。
    // 重判复用 policy::judge，不会与执行期判定出现两套口径；仍要问的（命令/删除/
    // 改名/覆盖已有文件/批量）原样保留。返回值只是清扫条数，失败不阻断切档。
    let swept =
        crate::core::tools::framework::policy_guard::sweep_pending_on_mode_change(&app, &session_id)
            .await;
    if swept > 0 {
        println!(
            "[JARVIS] 档位切换清扫：消化了 {} 条挂起请求",
            swept
        );
    }
    Ok(())
}

/// 切换当前会话的只读保护。
///
/// 开启后本会话禁止一切会改动工作区的操作：写 / 编辑 / 删除 / 改名 / 打补丁文件、
/// 执行非只读命令、起后台服务、派子代理、改工作目录。读文件与搜索不受影响。
///
/// **刻意不落盘**（与"本次会话都允许"同构，纯内存态）：这是"这一次别碰我的代码"，
/// 不是长期偏好。记住了反而会变成幽灵故障——新会话里 agent 莫名写不了文件，
/// 用户还不知道是哪个开关干的。
#[tauri::command]
pub async fn set_agent_read_only(
    session_id: String,
    enabled: bool,
    session_manager: tauri::State<'_, SessionManager>,
    app: tauri::AppHandle,
) -> Result<bool, String> {
    let ctx = session_manager.get_or_create(&session_id).await;
    *ctx.agent_read_only.lock().await = enabled;
    let _ = app.emit(
        "agent-read-only-changed",
        serde_json::json!({ "sessionId": session_id, "enabled": enabled }),
    );
    println!(
        "[JARVIS] 会话 {} 只读保护：{}",
        session_id,
        if enabled { "开启" } else { "关闭" }
    );
    Ok(enabled)
}

/// 撤销一条"本会话已允许"（操作类别 + 范围）
#[tauri::command]
pub async fn revoke_session_allowance(
    session_id: String,
    kind: String,
    scope: String,
    session_manager: tauri::State<'_, SessionManager>,
    app: tauri::AppHandle,
) -> Result<(), String> {
    let ctx = session_manager.get_or_create(&session_id).await;
    ctx.session_allowances
        .lock()
        .await
        .retain(|a| !(a.kind == kind && a.scope == scope));
    let _ = app.emit(
        "session-allowances-changed",
        serde_json::json!({ "sessionId": session_id }),
    );
    Ok(())
}

/// 清空当前会话的全部"已允许"
#[tauri::command]
pub async fn clear_session_allowances(
    session_id: String,
    session_manager: tauri::State<'_, SessionManager>,
    app: tauri::AppHandle,
) -> Result<(), String> {
    let ctx = session_manager.get_or_create(&session_id).await;
    ctx.session_allowances.lock().await.clear();
    let _ = app.emit(
        "session-allowances-changed",
        serde_json::json!({ "sessionId": session_id }),
    );
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reject_keeps_user_feedback() {
        let decision = parse_decision("reject", Some("  别删这个文件，改成加注释  ".to_string()));
        assert!(decision.is_rejected());
        assert!(!decision.is_allowed());
        let note = decision.model_note();
        assert!(note.contains("别删这个文件，改成加注释"));
        // 必须明确禁止等价重试，否则模型会换个写法再试一次
        assert!(note.contains("不要用等价写法重试"));
    }

    #[test]
    fn blank_feedback_is_treated_as_absent() {
        let decision = parse_decision("reject", Some("   ".to_string()));
        assert_eq!(decision, PermissionDecision::Reject { feedback: None });
    }

    #[test]
    fn allow_variants_pass_through() {
        assert_eq!(parse_decision("allow", None), PermissionDecision::Allow);
        assert_eq!(
            parse_decision("allow_session", None),
            PermissionDecision::AllowSession
        );
        assert!(parse_decision("allow_session", None).is_allowed());
    }

    #[test]
    fn unknown_decision_falls_back_to_reject() {
        // 安全默认：认不出来的指令绝不等于放行
        let decision = parse_decision("whatever", None);
        assert!(decision.is_rejected());
        assert!(!decision.is_allowed());
    }
}
