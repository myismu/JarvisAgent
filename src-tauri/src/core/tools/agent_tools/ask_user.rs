//! # agent_tools/ask_user.rs — 运行期用户咨询工具（AskUser）
//!
//! 在 Agent 运行期间遇到需求歧义、多方案分叉或缺少必要业务参数时，向前端弹出结构化
//! 咨询卡片（单选/多选 + 其他自填），阻塞等待用户即时回复而不中断当前 Agent Loop。
//!
//! ## Key Exports
//! - `ask_user()`: `AskUser` 工具主入口（复用 `WaitIntervalGuard` 计时 + `CancellationToken` 取消联动）
//!
//! ## Dependencies
//! - Internal: `crate::core::tools::framework`, `crate::infra::state::state`, `crate::infra::types::events`
//! - External: `serde_json`, `tauri`, `tokio`
//!
//! ## Constraints
//! - 仅主 Agent 可调用（子代理在工具白名单与分发层均被禁止）
//! - 不设自动超时；仅在用户提交、点击跳过、本轮取消或通道关闭时结束等待

use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;
use tauri::{Emitter, Manager};
use tokio::sync::oneshot;

use crate::core::tools::framework;
use crate::core::tools::framework::permission::WaitIntervalGuard;
use crate::infra::state::state::{
    PendingUserQuestion, SessionContext, SessionManager, UserQuestionDecision,
};
use crate::infra::types::events::{
    UserQuestionAnswerItem, UserQuestionItem, UserQuestionOption, UserQuestionRequestEvent,
    UserQuestionResolvedEvent,
};

const MAX_QUESTIONS: usize = 5;
const MIN_OPTIONS: usize = 2;
const MAX_OPTIONS: usize = 6;

/// 挂起咨询条目的 RAII 清理守卫（与 `PendingPermissionGuard` 同构）：
/// 无论正常返回、取消还是 future 被意外 drop，都确保移除内存挂起项并通知前端收起咨询框。
struct PendingQuestionGuard {
    ctx: Arc<SessionContext>,
    app: tauri::AppHandle,
    session_id: String,
    id: String,
}

impl Drop for PendingQuestionGuard {
    fn drop(&mut self) {
        let ctx = self.ctx.clone();
        let app = self.app.clone();
        let session_id = self.session_id.clone();
        let id = self.id.clone();

        let cleanup = async move {
            let removed = ctx.pending_questions.lock().await.remove(&id);
            if removed.is_some() {
                let _ = app.emit(
                    "user-question-resolved",
                    UserQuestionResolvedEvent {
                        id,
                        session_id,
                        status: "interrupted".to_string(),
                    },
                );
            }
        };

        if let Ok(handle) = tokio::runtime::Handle::try_current() {
            handle.spawn(cleanup);
        }
    }
}

/// 解析并校验 `AskUser` 工具入参，返回 `(可选标题, 问题列表)`。
pub(crate) fn parse_ask_user_input(
    input: &serde_json::Value,
) -> Result<(Option<String>, Vec<UserQuestionItem>), String> {
    let title = input
        .get("title")
        .and_then(|v| v.as_str())
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(ToString::to_string);

    let raw_questions = input
        .get("questions")
        .and_then(|v| v.as_array())
        .ok_or_else(|| "参数错误：缺少 questions 数组（1~5 道结构化问题）".to_string())?;

    if raw_questions.is_empty() {
        return Err("参数错误：questions 不能为空，至少提供 1 道问题".to_string());
    }
    if raw_questions.len() > MAX_QUESTIONS {
        return Err(format!(
            "参数错误：单次咨询最多支持 {} 道问题（当前 {} 道），请合并精简后再提问",
            MAX_QUESTIONS,
            raw_questions.len()
        ));
    }

    let mut parsed = Vec::with_capacity(raw_questions.len());
    for (idx, item) in raw_questions.iter().enumerate() {
        let question_text = item
            .get("question")
            .and_then(|v| v.as_str())
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .ok_or_else(|| format!("参数错误：第 {} 题缺少非空 question 文本", idx + 1))?
            .to_string();

        let id = item
            .get("id")
            .and_then(|v| v.as_str())
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .map(ToString::to_string)
            .unwrap_or_else(|| format!("q{}", idx + 1));

        let header = item
            .get("header")
            .and_then(|v| v.as_str())
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .map(ToString::to_string);

        let multi_select = item
            .get("multi_select")
            .or_else(|| item.get("multiSelect"))
            .and_then(|v| v.as_bool())
            .unwrap_or(false);

        let allow_custom = item
            .get("allow_custom")
            .or_else(|| item.get("allowCustom"))
            .and_then(|v| v.as_bool())
            .unwrap_or(true);

        let raw_options = item
            .get("options")
            .and_then(|v| v.as_array())
            .ok_or_else(|| format!("参数错误：第 {} 题缺少 options 选项数组", idx + 1))?;

        if raw_options.len() < MIN_OPTIONS || raw_options.len() > MAX_OPTIONS {
            return Err(format!(
                "参数错误：第 {} 题的 options 数量需在 {}~{} 项之间（当前 {} 项）",
                idx + 1,
                MIN_OPTIONS,
                MAX_OPTIONS,
                raw_options.len()
            ));
        }

        let mut options = Vec::with_capacity(raw_options.len());
        for (opt_idx, opt) in raw_options.iter().enumerate() {
            let parsed_opt = if let Some(label_str) = opt.as_str() {
                let trimmed = label_str.trim();
                if trimmed.is_empty() {
                    return Err(format!(
                        "参数错误：第 {} 题第 {} 个选项为空",
                        idx + 1,
                        opt_idx + 1
                    ));
                }
                UserQuestionOption {
                    label: trimmed.to_string(),
                    description: None,
                    recommended: false,
                }
            } else {
                let label = opt
                    .get("label")
                    .and_then(|v| v.as_str())
                    .map(str::trim)
                    .filter(|s| !s.is_empty())
                    .ok_or_else(|| {
                        format!(
                            "参数错误：第 {} 题第 {} 个选项缺少 label",
                            idx + 1,
                            opt_idx + 1
                        )
                    })?
                    .to_string();
                let description = opt
                    .get("description")
                    .and_then(|v| v.as_str())
                    .map(str::trim)
                    .filter(|s| !s.is_empty())
                    .map(ToString::to_string);
                let recommended = opt
                    .get("recommended")
                    .and_then(|v| v.as_bool())
                    .unwrap_or(false);
                UserQuestionOption {
                    label,
                    description,
                    recommended,
                }
            };
            options.push(parsed_opt);
        }

        parsed.push(UserQuestionItem {
            id,
            header,
            question: question_text,
            multi_select,
            options,
            allow_custom,
        });
    }

    Ok((title, parsed))
}

/// 将用户提交的结构化答复整理为模型易读且可直接落库回看的文本摘要。
pub(crate) fn format_user_answers(
    questions: &[UserQuestionItem],
    answers: &[UserQuestionAnswerItem],
) -> String {
    let mut lines = Vec::with_capacity(questions.len() + 2);
    lines.push("【用户咨询回复】".to_string());
    for (idx, q) in questions.iter().enumerate() {
        let answer = answers.iter().find(|a| a.question_id == q.id);
        let mut parts: Vec<String> = Vec::new();
        if let Some(ans) = answer {
            if !ans.selected_options.is_empty() {
                parts.push(ans.selected_options.join("、"));
            }
            if let Some(custom) = ans
                .custom_text
                .as_deref()
                .map(str::trim)
                .filter(|s| !s.is_empty())
            {
                parts.push(format!("自定义补充：{}", custom));
            }
        }
        let summary = if parts.is_empty() {
            "（未作答，可按推荐方案处理）".to_string()
        } else {
            parts.join("；")
        };
        lines.push(format!("{}. {} -> {}", idx + 1, q.question, summary));
    }
    lines.push("请严格依据上述用户选择继续当前执行。".to_string());
    lines.join("\n")
}

/// 执行 `AskUser` 工具：向前端推送结构化咨询框，复用 `WaitIntervalGuard` 扣减等待耗时，
/// 并在用户作答、跳过或本轮取消时返回结构化结果。
pub async fn ask_user(
    app: &tauri::AppHandle,
    input: &serde_json::Value,
    session_id: &str,
) -> framework::ToolCallResult {
    let (title, questions) = match parse_ask_user_input(input) {
        Ok(v) => v,
        Err(err) => return framework::ToolCallResult::error(err),
    };

    let session_manager = app.state::<SessionManager>();
    let ctx = session_manager.get_or_create(session_id).await;

    static QUESTION_REQ_ID: AtomicUsize = AtomicUsize::new(1);
    let id = format!("uq_{}", QUESTION_REQ_ID.fetch_add(1, Ordering::SeqCst));

    let (tx, rx) = oneshot::channel();
    {
        let mut pending = ctx.pending_questions.lock().await;
        pending.retain(|_, entry| !entry.responder.is_closed());
        pending.insert(
            id.clone(),
            PendingUserQuestion {
                title: title.clone(),
                questions: questions.clone(),
                responder: tx,
            },
        );
    }

    let _guard = PendingQuestionGuard {
        ctx: ctx.clone(),
        app: app.clone(),
        session_id: session_id.to_string(),
        id: id.clone(),
    };

    // 复用权限审批同一套等待区间守卫，将等待用户回复的时间计入 agent_runs.wait_ms
    let mut wait_guard = WaitIntervalGuard::new(ctx.clone());

    let _ = app.emit(
        "user-question-request",
        UserQuestionRequestEvent {
            id: id.clone(),
            session_id: session_id.to_string(),
            title,
            questions: questions.clone(),
        },
    );

    let cancel = ctx.current_cancel_token();
    let decision = match cancel {
        Some(token) => tokio::select! {
            result = rx => result.unwrap_or_else(|_| UserQuestionDecision::Interrupted {
                reason: "咨询通道已关闭".to_string(),
            }),
            _ = token.cancelled() => UserQuestionDecision::Interrupted {
                reason: "本轮执行已被用户取消".to_string(),
            },
        },
        None => rx
            .await
            .unwrap_or_else(|_| UserQuestionDecision::Interrupted {
                reason: "咨询通道已关闭".to_string(),
            }),
    };

    wait_guard.finish();

    // 异常取消路径负责兜底移除条目并通知前端关闭咨询框
    {
        let removed = ctx.pending_questions.lock().await.remove(&id);
        if removed.is_some() {
            let status = match &decision {
                UserQuestionDecision::Answered(_) => "answered",
                UserQuestionDecision::Skipped => "skipped",
                UserQuestionDecision::Interrupted { .. } => "interrupted",
            };
            let _ = app.emit(
                "user-question-resolved",
                UserQuestionResolvedEvent {
                    id: id.clone(),
                    session_id: session_id.to_string(),
                    status: status.to_string(),
                },
            );
        }
    }

    match decision {
        UserQuestionDecision::Answered(answers) => {
            framework::ToolCallResult::ok(format_user_answers(&questions, &answers))
        }
        UserQuestionDecision::Skipped => framework::ToolCallResult::ok(
            "【用户跳过咨询】用户选择跳过本次提问，请直接采用你的推荐方案继续执行。".to_string(),
        ),
        UserQuestionDecision::Interrupted { reason } => {
            framework::ToolCallResult::error(format!("咨询未完成：{}", reason))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn parse_input_validates_bounds_and_defaults() {
        assert!(parse_ask_user_input(&json!({})).is_err());
        assert!(parse_ask_user_input(&json!({ "questions": [] })).is_err());
        assert!(parse_ask_user_input(&json!({
            "questions": [{ "question": "选哪一个？", "options": ["仅一项"] }]
        }))
        .is_err());

        let (title, qs) = parse_ask_user_input(&json!({
            "title": "确认技术方案",
            "questions": [
                {
                    "header": "存储方案",
                    "question": "本地缓存希望采用哪种方式？",
                    "options": [
                        { "label": "SQLite", "description": "复用现有数据库", "recommended": true },
                        { "label": "JSON 文件", "description": "轻量易读" }
                    ]
                }
            ]
        }))
        .expect("valid input");

        assert_eq!(title.as_deref(), Some("确认技术方案"));
        assert_eq!(qs.len(), 1);
        assert_eq!(qs[0].id, "q1");
        assert!(!qs[0].multi_select);
        assert!(qs[0].allow_custom);
        assert!(qs[0].options[0].recommended);
    }

    #[test]
    fn format_answers_combines_selected_and_custom_text() {
        let (_, qs) = parse_ask_user_input(&json!({
            "questions": [
                {
                    "id": "q_db",
                    "question": "数据库选型？",
                    "multi_select": true,
                    "options": ["SQLite", "PostgreSQL"]
                },
                {
                    "id": "q_ui",
                    "question": "交互风格？",
                    "options": ["卡片内联", "居中弹窗"]
                }
            ]
        }))
        .expect("valid");

        let formatted = format_user_answers(
            &qs,
            &[
                UserQuestionAnswerItem {
                    question_id: "q_db".to_string(),
                    selected_options: vec!["SQLite".to_string()],
                    custom_text: Some("需要 WAL 模式".to_string()),
                },
                UserQuestionAnswerItem {
                    question_id: "q_ui".to_string(),
                    selected_options: vec![],
                    custom_text: None,
                },
            ],
        );

        assert!(formatted.contains("1. 数据库选型？ -> SQLite；自定义补充：需要 WAL 模式"));
        assert!(formatted.contains("2. 交互风格？ -> （未作答，可按推荐方案处理）"));
    }
}
