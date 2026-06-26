//! # pipeline.rs — Agent 主循环流水线
//!
//! 实现 Agent 的 5 阶段执行流水线：初始化 → 意图验证 → 上下文构建 → 主循环 → 收尾。
//! 主循环阶段包含压缩检查、API 调用、流式处理、工具执行、反思审查等完整 Agent Loop 逻辑。
//!
//! ## 关键导出
//! - `run_pipeline()`: 主流程入口，依次执行 5 个阶段并返回 `JarvisResult`
//!
//! ## 依赖
//! - Internal: `crate::core::orchestration::agent_runs`, `crate::infra::llm::api_client`, `crate::infra::config::config::AgentConfig`, `crate::core::intent`, `crate::core::session::memory`, `crate::core::tools`, `super::reflection`
//! - External: `eventsource_stream`, `serde_json`, `tauri`, `tokio_util`, `reqwest`
//!
//! ## 约束
//! - 循环次数受 `MAX_AGENT_LOOP_BEFORE_CONFIRM` 和 `MAX_AGENT_LOOP_ABSOLUTE` 常量限制
//! - 取消令牌（`CancellationToken`）贯穿全流程，支持用户随时中断
//! - 反思审查在工具执行后触发，受 `reflection_mode` 和防循环机制控制
//! - 传递 work_mode 到工具调用链路，支持兜底防护

use std::sync::Arc;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use eventsource_stream::Eventsource;
use serde_json::json;
use tauri::{Emitter, Manager};

use crate::infra::config::config::AgentConfig;
use crate::infra::types::error::{AgentError, ApiError};
use crate::infra::debug_logger;
use crate::core::intent;
use crate::infra::llm::api_client;
use crate::infra::types::models::*;
use crate::core::orchestration::agent_runs;
use crate::core::session::{append_message, memory::*, pop_message, restore_message};
use crate::core::tools::*;

use super::context::*;
use super::stream::{process_stream, StreamConfig};
use super::tools_runner::execute_tool_calls;

/// Pipeline 各阶段共享的状态
struct PipelineState {
    app: tauri::AppHandle,
    sid: String,
    ctx: Arc<crate::infra::state::state::SessionContext>,
    cancel_token: tokio_util::sync::CancellationToken,
    request_workspace: Option<std::path::PathBuf>,
    cfg: AgentConfig,
    api_key: String,
    base_url: String,
    model_id: String,
    /// 用户消息的简短展示版本（用于 UI 存储，刷新后仍显示简短版）
    display_msg: Option<String>,
    /// break_loop 时的工具执行结果摘要（传递给前端 toolBuffer）
    tool_execution_summary: Option<String>,
    api_format: crate::infra::llm::api_format::ApiFormat,
    client: reqwest::Client,
    system_prompt: String,
    msg: String,
    image_base64_list: Option<Vec<String>>,
    thinking_override: Option<bool>,
    /// 基于 audience 的 agent loop 默认思考状态（developer → true, user → false）
    loop_think_default: bool,
    detected_intent: String,
    dynamic_context_str: String,
    user_msg_for_memory: String,
    user_msg_preview: String,
    initial_msg_index: usize,
    should_think: bool,
    run_id: String,
    /// 循环状态
    loop_count: usize,
    total_loop_count: usize,
    req_input_tokens: u64,
    req_output_tokens: u64,
    final_answer: String,
    /// 反思审查状态
    reflection_mode: String,
    total_reflections: usize,
    consecutive_reflection_nos: usize,
}

struct ContextEstimate {
    total_chars: usize,
    estimated_tokens: usize,
    message_count: usize,
    tool_schema_count: usize,
    tool_call_count: usize,
    tool_result_count: usize,
    sections: Vec<ContextSectionSnapshot>,
}

const DIRECT_DEVELOPER_INTENT: &str = "PROJECT_ACTION";

fn normalize_agent_audience(audience: &str) -> &'static str {
    match audience {
        "user" => "user",
        _ => "developer",
    }
}

/// 防御性修复：确保每个 Assistant(tool_calls) 后跟 ToolResult 消息。
///
/// 流式中断、并发修改、break_loop 等边缘 case 可能导致
/// Assistant 消息包含 ToolUse 块但后续没有对应的 ToolResult。
/// API 要求 tool_calls 后必须跟 tool_result，否则返回 400。
fn fix_broken_tool_call_pairs(messages: &mut Vec<Message>) {
    let mut i = 0;
    while i < messages.len() {
        // 检查当前消息是否为包含 ToolUse 的 Assistant 消息
        let has_tool_use = match &messages[i] {
            Message::Assistant { content: Content::Multiple(blocks) } => {
                blocks.iter().any(|b| matches!(b, ContentBlock::ToolUse { .. }))
            }
            _ => false,
        };

        if !has_tool_use {
            i += 1;
            continue;
        }

        // 收集所有 ToolUse 的 tool_use_id
        let tool_use_ids: Vec<String> = match &messages[i] {
            Message::Assistant { content: Content::Multiple(blocks) } => {
                blocks.iter()
                    .filter_map(|b| match b {
                        ContentBlock::ToolUse { id, .. } => Some(id.clone()),
                        _ => None,
                    })
                    .collect()
            }
            _ => vec![],
        };

        // 检查下一条消息是否包含对应的 ToolResult
        let next_has_results = if i + 1 < messages.len() {
            match &messages[i + 1] {
                Message::User { content: Content::Multiple(blocks) } => {
                    let result_ids: Vec<&str> = blocks.iter()
                        .filter_map(|b| match b {
                            ContentBlock::ToolResult { tool_use_id, .. } => Some(tool_use_id.as_str()),
                            _ => None,
                        })
                        .collect();
                    tool_use_ids.iter().all(|id| result_ids.contains(&id.as_str()))
                }
                _ => false,
            }
        } else {
            false
        };

        if !next_has_results {
            println!(
                "[JARVIS] 防御性修复: Assistant(tool_calls) 后缺少 ToolResult，注入占位结果 (index={})",
                i
            );
            // 为缺失的 tool_use_id 注入占位 ToolResult
            let placeholder_blocks: Vec<ContentBlock> = tool_use_ids.iter()
                .map(|id| ContentBlock::ToolResult {
                    tool_use_id: id.clone(),
                    content: "[系统注入：工具结果因中断丢失，已自动修复消息序列]".to_string(),
                })
                .collect();
            messages.insert(i + 1, Message::User {
                content: Content::Multiple(placeholder_blocks),
            });
            // 跳过刚插入的消息
            i += 2;
        } else {
            i += 1;
        }
    }
}

fn normalize_agent_work_mode(mode: &str) -> &'static str {
    match mode {
        "chat" => "chat",
        "plan" => "plan",
        _ => "edit",
    }
}

impl PipelineState {
    /// 阶段 1: 会话初始化、配置加载、意图分类
    async fn setup(
        session_id: String,
        msg: String,
        thinking_override: Option<bool>,
        image_base64_list: Option<Vec<String>>,
        _agent_display_mode: Option<String>,
        reflection_mode_override: Option<String>,
        display_msg: Option<String>,
        _inject_user_message: bool,
        app: tauri::AppHandle,
        session_manager: tauri::State<'_, crate::infra::state::state::SessionManager>,
        config_state: tauri::State<'_, crate::infra::config::config::ConfigState>,
    ) -> Result<Self, AgentError> {
        println!("\n{}", "=".repeat(60));
        println!(
            "[贾维斯] 收到用户消息: {} (图片数量: {})",
            msg,
            image_base64_list.as_ref().map(|l| l.len()).unwrap_or(0)
        );
        println!("{}", "=".repeat(60));

        let sid = session_id.clone();
        let ctx = session_manager.get_or_create(&session_id).await;
        let has_active_run = ctx
            .cancel_token
            .lock()
            .await
            .as_ref()
            .map(|token| !token.is_cancelled())
            .unwrap_or(false);
        if has_active_run {
            return Err(AgentError::Session(
                "当前会话已有任务正在执行，请等待完成或先停止当前任务。".to_string(),
            ));
        }
        *ctx.session_allowed.lock().await = false;

        let cancel_token = tokio_util::sync::CancellationToken::new();
        *ctx.cancel_token.lock().await = Some(cancel_token.clone());

        let request_workspace = ctx.workspace.lock().await.clone();
        println!(
            "[DEBUG] Current Workspace for session {}: {:?}",
            sid, request_workspace
        );

        let app_cfg = config_state.0.lock().await.clone();
        let cfg = app_cfg.active_config();

        if cfg.api_key.is_empty() {
            *ctx.cancel_token.lock().await = None;
            return Err(AgentError::Config(
                "未配置 API Key，请在设置中填写".to_string(),
            ));
        }
        let api_key = cfg.api_key.clone();
        let base_url = cfg.base_url.clone();
        let model_id = cfg.main_model.clone();
        let utility_model_id = cfg.utility_model.clone();
        let api_format = cfg.api_format_enum();
        println!(
            "[JARVIS] Using model: {} (utility: {})",
            model_id, utility_model_id
        );

        let client = reqwest::Client::new();

        // 读取双轴偏好
        let (audience, work_mode) = {
            let prefs = crate::command::app_config::get_ui_preferences()
                .await
                .unwrap_or_default();
            let audience = normalize_agent_audience(&prefs.agent_audience).to_string();
            let work_mode = normalize_agent_work_mode(&prefs.agent_work_mode).to_string();
            (audience, work_mode)
        };
        *ctx.agent_audience.lock().await = audience.clone();
        // 工作模式：新会话从用户偏好初始化，已有历史的会话跨 pipeline 保持
        // 这样 plan → break_loop → 审批 → 新 pipeline 时不会被重置为 edit
        {
            let has_history = !ctx.memory.lock().await.messages.is_empty();
            let mut mode = ctx.agent_work_mode.lock().await;
            if !has_history {
                *mode = work_mode.clone();
            }
        }
        let current_work_mode = ctx.agent_work_mode.lock().await.clone();
        let system_prompt = crate::core::agent::prompts::get_system_prompt(&audience, &current_work_mode);

        let has_images = image_base64_list
            .as_ref()
            .map(|l| !l.is_empty())
            .unwrap_or(false);
        let msg_for_intent = if has_images {
            format!(
                "{}\n\n[用户同时附带了图片/截图；截图可能是报错、UI 异常、终端输出、运行结果或代码问题反馈，请结合文本判断是否属于项目操作，不要仅因有图判为 CHAT。]",
                msg
            )
        } else {
            msg.clone()
        };
        let is_approval_continuation = msg_for_intent.starts_with("用户已同意方案")
                || msg_for_intent.starts_with("用户要求修改方案");
        let detected_intent = if work_mode != "chat" {
            let rule_intent = crate::core::intent::rules::classify_by_rules(&msg_for_intent);
            if matches!(rule_intent, crate::core::intent::rules::Intent::Plan) && !is_approval_continuation {
                println!("[JARVIS] {} 模式：规则检测到复杂任务，首轮直接进入方案审批流程", work_mode);
                "TASK_PLAN".to_string()
            } else {
                println!("[JARVIS] {} 模式：跳过 LLM 意图分类，直接进入项目操作流程", work_mode);
                DIRECT_DEVELOPER_INTENT.to_string()
            }
        } else {
            let history_for_classification = ctx.memory.lock().await.messages.clone();
            intent::classify_intent(
                &client,
                &api_key,
                &base_url,
                &utility_model_id,
                api_format,
                &msg_for_intent,
                &history_for_classification,
                &session_id,
            )
            .await
        };
        println!("[JARVIS] Detected intent: {}", detected_intent);

        // 输入框自然语言审批：
        // 仅在三种条件同时满足时自动更新 plan status：
        // 1. 短消息（≤5 字），长消息不可能是纯粹审批回复
        // 2. 上一轮助手刚提交了方案（history 最后一条 assistant 含 ProposePlan 或方案提交文本）
        // 3. plan_documents 中存在 pending 方案
        if msg.trim().chars().count() <= 5 {
            // 上一轮 Agent 最后一次工具行动是否为提方案
            // 找最后一条含 tool_use 的 assistant 消息，检查是否为 ProposePlan
            let was_proposing_plan = {
                let memory = ctx.memory.lock().await;
                memory.messages.iter().rev()
                    .find_map(|m| {
                        if let Message::Assistant { content } = m {
                            if let Content::Multiple(blocks) = content {
                                let has_tool = blocks.iter().any(|b| matches!(b, ContentBlock::ToolUse { .. }));
                                if has_tool {
                                    let is_plan = blocks.iter().any(|b| matches!(b, ContentBlock::ToolUse { name, .. } if name == "ProposePlan"));
                                    return Some(is_plan);
                                }
                            }
                        }
                        None
                    })
                    .unwrap_or(false)
            };

            if was_proposing_plan {
                let mut memory = ctx.memory.lock().await;
                let pending_plans: Vec<_> = memory
                    .plan_documents
                    .iter()
                    .filter(|doc| doc.status == "pending")
                    .map(|doc| (doc.id.clone(), doc.title.clone()))
                    .collect();

                if !pending_plans.is_empty() {
                    // 区分同意/拒绝：意图分类器对两类都返回 ACTION，需靠消息文本判断
                    let msg_trim = msg.trim();
                    let is_reject = msg_trim.starts_with("不")
                        || msg_trim == "拒绝"
                        || msg_trim == "reject"
                        || msg_trim == "no";
                    let new_status = if is_reject { "revision_requested" } else { "approved" };

                    for (plan_id, plan_title) in &pending_plans {
                        if let Ok(Some(doc)) = crate::core::session::update_plan_document_status(
                            &session_id, plan_id, new_status, None,
                        ) {
                            if let Some(existing) = memory.plan_documents.iter_mut().find(|d| d.id == doc.id) {
                                *existing = doc.clone();
                            }
                            let _ = app.emit("plan-document-updated", &doc);
                        }
                        println!("[JARVIS] 输入框审批：方案「{}」→ {}", plan_title, new_status);
                    }
                }
            }
        }

        let resolved_reflection_mode = reflection_mode_override
            .filter(|m| !m.is_empty())
            .unwrap_or_else(|| cfg.reflection_mode.clone());

        let mut state = Self {
            app,
            sid,
            ctx,
            cancel_token,
            request_workspace,
            cfg,
            api_key,
            base_url,
            model_id,
            api_format,
            client,
            system_prompt,
            msg,
            image_base64_list,
            thinking_override,
            loop_think_default: audience == "developer",
            detected_intent: detected_intent.clone(),
            // 以下字段在后续阶段填充
            dynamic_context_str: String::new(),
            user_msg_for_memory: String::new(),
            user_msg_preview: String::new(),
            initial_msg_index: 0,
            should_think: false,
            run_id: String::new(),
            loop_count: 0,
            total_loop_count: 0,
            req_input_tokens: 0,
            req_output_tokens: 0,
            final_answer: String::new(),
            reflection_mode: resolved_reflection_mode,
            total_reflections: 0,
            consecutive_reflection_nos: 0,
            display_msg,
            tool_execution_summary: None,
        };

        if detected_intent == "TASK_PLAN" && current_work_mode != "plan" {
            println!("[JARVIS] 意图前置拦截：TASK_PLAN 意图，首轮强制切换到 Plan 模式");
            *state.ctx.agent_work_mode.lock().await = "plan".to_string();
            state.system_prompt = crate::core::agent::prompts::get_system_prompt(&audience, "plan");
            state.detected_intent = "TASK_PLAN".to_string();
            let _ = state.app.emit(
                "agent-work-mode-changed",
                json!({
                    "sessionId": state.sid,
                    "from": work_mode,
                    "to": "plan",
                    "reason": "意图分类检测到复杂任务，自动切换到计划模式",
                }),
            );
        }

        Ok(state)
    }

    /// 阶段 2: 意图验证 — DANGEROUS 权限确认 / UNCLEAR 澄清
    /// 返回 Some(JarvisResult) 表示需要提前返回
    async fn validate(&mut self) -> Result<Option<JarvisResult>, AgentError> {
        if self.detected_intent == "DANGEROUS" {
            let decision = request_permission(
                &self.app,
                &self.sid,
                &format!(
                    "△ 检测到可能的危险操作意图：「{}」\n确认要继续执行吗？",
                    self.msg
                ),
            )
            .await;
            if decision == "reject" {
                println!("[JARVIS] 用户拒绝了危险操作");
                *self.ctx.cancel_token.lock().await = None;
                return Ok(Some(JarvisResult {
                    status: "CANCELLED".to_string(),
                    content: "操作已取消。如果这是一个误判，请重新更具体地描述您的需求。"
                        .to_string(),
                    input_tokens: 0,
                    output_tokens: 0,
                    session_input_tokens: 0,
                    session_output_tokens: 0,
                    user_message_id: None,
                    tool_execution_summary: None,
                }));
            }
            println!("[JARVIS] 用户确认了危险操作，继续执行");
        }

        if self.detected_intent == "UNCLEAR" {
            println!("[JARVIS] 意图不明确，询问用户澄清");
            let clarification = "先生，我不太确定您的意思。请问您具体想要做什么呢？\n\n例如：\n- **闲聊** — 随便聊聊天\n- **读写代码** — 查看、修改、审查代码\n- **运行命令** — 执行脚本、编译、部署\n- **咨询问题** — 技术概念、用法疑问\n- **记忆查询** — 查看之前的对话记录\n- **设置** — 配置修改\n- **危险操作** — 删除文件等不可逆操作\n\n请描述您的需求，我来为您处理。";
            *self.ctx.cancel_token.lock().await = None;
            return Ok(Some(JarvisResult {
                status: "CLARIFICATION_NEEDED".to_string(),
                content: clarification.to_string(),
                input_tokens: 0,
                output_tokens: 0,
                session_input_tokens: 0,
                session_output_tokens: 0,
                user_message_id: None,
                tool_execution_summary: None,
            }));
        }

        Ok(None)
    }

    /// 阶段 3: 上下文构建 + 消息注入 + Agent Run 启动
    async fn pre_loop(&mut self) {
        self.dynamic_context_str = build_dynamic_context(
            &self.detected_intent,
            &self.request_workspace,
        );

        self.user_msg_for_memory = self.msg.clone();
        self.user_msg_preview = if self.msg.chars().count() > 50 {
            self.msg.chars().take(50).collect::<String>()
        } else {
            self.msg.clone()
        };

        let memory_after_user_message = {
            let mut session = self.ctx.memory.lock().await;
            if crate::command::session::recover_interrupted_into_memory(
                &self.sid,
                &mut session,
            ) {
                let recovered_memory = session.clone();
                crate::core::session::save_session(&self.sid, &recovered_memory, None);
                if let Some(interrupted_run) = agent_runs::find_interrupted_run(&self.sid) {
                    let _ = agent_runs::mark_run_recovered(&interrupted_run.run_id);
                }

                // 检测程序崩溃时残留的 InProgress 任务，注入恢复指令给 LLM
                let tm = crate::core::orchestration::tasks::TaskManager::for_session(&self.sid);
                let in_progress: Vec<_> = tm.get_all_tasks()
                    .into_iter()
                    .filter(|t| t.status == crate::infra::types::models::TaskStatus::InProgress)
                    .collect();
                if !in_progress.is_empty() {
                    let task_list: String = in_progress.iter()
                        .map(|t| format!("  • Task #{}: {}", t.id, t.subject))
                        .collect::<Vec<_>>()
                        .join("\n");
                    let recovery_msg = format!(
                        "【系统恢复通知】\n\
                        程序上次非正常结束（崩溃/强退），以下 {} 个任务在执行中被中断：\n\
                        {}\n\n\
                        请按以下步骤处理：\n\
                        1. 检查工作目录中的实际文件状态，判断哪些任务已完成、部分完成、未开始\n\
                        2. 已完成的任务 → 用 UpdateTask 标为 completed\n\
                        3. 部分完成的任务 → 用 UpdateTask 标为 pending（或保持 InProgress 不处理），评估剩余工作\n\
                        4. 未开始的任务 → 保持 pending\n\
                        5. 完成状态整理后，重新调用 RunSubagentsSequentially 继续执行未完成的任务",
                        in_progress.len(), task_list
                    );
                    append_message(&mut session, Message::Assistant {
                        content: Content::Single(recovery_msg),
                    }, "internal");
                    println!("[JARVIS] 恢复：检测到 {} 个 InProgress 任务，已注入恢复指令", in_progress.len());
                }

                let _ = self.app.emit("session-updated", ());
            }
            let mut active_sid = Some(self.sid.clone());
            self.initial_msg_index = inject_user_message(
                &mut session,
                self.display_msg.as_deref().unwrap_or(&self.msg),
                &self.image_base64_list,
                &mut active_sid,
            );
            session.clone()
        };
        crate::core::session::save_session(&self.sid, &memory_after_user_message, None);
        let _ = self.app.emit("session-updated", ());

        // 深度思考决策：
        // - 首轮：用户 thinking_override 优先（一次性），否则使用 audience 默认值
        // - 后续轮：始终使用 audience 默认值（developer → true, user → false）
        if let Some(override_val) = self.thinking_override {
            self.should_think = override_val;
            self.thinking_override = None; // 消费后清除，不再影响后续轮次
        } else {
            self.should_think = self.loop_think_default;
        }

        let user_message_id = {
            let session = self.ctx.memory.lock().await;
            session.message_ids.get(self.initial_msg_index).cloned()
        };
        println!("[JARVIS] start_run: message_id={:?} initial_msg_index={}", user_message_id, self.initial_msg_index);
        self.run_id = agent_runs::start_run(&self.app, &self.sid, &self.msg, None, user_message_id);
        *self.ctx.active_run_id.lock().await = Some(self.run_id.clone());
        {
            let session = self.ctx.memory.lock().await;
            agent_runs::save_checkpoint(
                &self.app,
                &self.run_id,
                &self.sid,
                self.total_loop_count,
                session.messages.clone(),
                self.req_input_tokens,
                self.req_output_tokens,
                "用户消息已写入",
            );
        }
    }

    /// 处理调度器事件。
    /// 返回 (needs_llm, scheduler_done)：
    /// - needs_llm: 有事件注入了对话，需要 LLM 处理
    /// - scheduler_done: 调度器已结束（AllDone），调用方应退出等待
    async fn handle_sched_event(&mut self, event: crate::core::orchestration::scheduler::SchedulerEvent) -> (bool, bool) {
        use crate::core::orchestration::scheduler::SchedulerEvent;
        match event {
            SchedulerEvent::TaskCompleted { task_id, subject, tokens: _ } => {
                println!("[JARVIS] 调度器: Task #{} ({}) 完成", task_id, subject);
                let _ = self.app.emit("chat-stream", json!({
                    "content": format!("\n> [OK] Task #{} 完成: {}\n", task_id, subject),
                    "sessionId": self.sid,
                }));
                (false, false)
            }
            SchedulerEvent::TaskFailed { task_id, subject, reason, error_detail } => {
                println!("[JARVIS] 调度器: Task #{} ({}) 失败: {}", task_id, subject, reason);
                let _ = self.app.emit("chat-stream", json!({
                    "content": format!("\n> [FAIL] Task #{} 失败({}): {}\n", task_id, reason, subject),
                    "sessionId": self.sid,
                }));
                let mut session = self.ctx.memory.lock().await;
                append_message(&mut session, Message::Assistant {
                    content: Content::Single(format!(
                        "调度器通知：Task #{}「{}」执行失败（原因：{}）。\n错误详情：\n{}\n\n请根据以上信息决策：重试该任务 / 将其拆分为更小子任务 / 跳过该任务继续执行其他任务。",
                        task_id, subject, reason, error_detail
                    )),
                }, "internal");
                (true, false)
            }
            SchedulerEvent::AllDone { completed, failed, report } => {
                println!("[JARVIS] 调度器: 全部完成 {}成功 {}失败", completed, failed);
                *self.ctx.scheduler_rx.lock().await = None;
                let _ = self.app.emit("chat-stream", json!({
                    "content": format!("\n> [调度报告] {}成功 {}失败\n\n{}\n", completed, failed, report),
                    "sessionId": self.sid,
                }));
                let mut session = self.ctx.memory.lock().await;
                append_message(&mut session, Message::Assistant {
                    content: Content::Single(format!(
                        "调度器报告：所有任务已执行完毕。\n{}",
                        report
                    )),
                }, "internal");
                (true, true)
            }
        }
    }

    /// 阶段 4: 主循环 — 压缩 → 请求构建 → API 调用 → 流处理 → 工具执行
    async fn run_main_loop(&mut self) -> Result<(), AgentError> {
        loop {
            println!("[JARVIS] 主循环开始: loop_count={}, total_loop_count={}", self.loop_count, self.total_loop_count);

            // 取消检查
            if self.cancel_token.is_cancelled() {
                println!("[JARVIS] 主循环: cancel_token 已取消，退出循环");
                self.handle_cancellation().await;
                break;
            }

            // 循环次数确认
            if self.loop_count >= crate::infra::types::constants::MAX_AGENT_LOOP_BEFORE_CONFIRM {
                let decision = self.request_loop_continuation().await;
                if !decision {
                    break;
                }
            }

            // 后台通知注入
            self.drain_background_notifications().await;

            // Token 压缩
            self.compact_if_needed().await;

            // 后续轮次：重置 should_think 为 audience 默认值，确保 agent loop 思考状态一致
            if self.loop_count > 0 {
                self.should_think = self.loop_think_default;
            }

            // 历史快照准备
            let history_snapshot = self.prepare_history_snapshot().await;

            // 构建请求并更新上下文快照
            let audience = self.ctx.agent_audience.lock().await.clone();
            let work_mode = self.ctx.agent_work_mode.lock().await.clone();
            let (req_json, is_openai) = self.build_llm_request(history_snapshot, &audience, &work_mode);

            // 调试日志
            let request_json = serde_json::to_string_pretty(&req_json).unwrap_or_default();
            println!("[MAIN AGENT] loop {} request ({} bytes)", self.total_loop_count + 1, request_json.len());
            debug_logger::debug_logger().log_request(&self.sid, "MAIN", self.total_loop_count + 1, &request_json);

            if self.cancel_token.is_cancelled() {
                continue;
            }

            // 调度器 channel 接收端（异步 select! 用）
            let sched_rx = self.ctx.scheduler_rx.lock().await.take();

            // API 调用 + 调度器事件 select!：spawn API 到后台 task，select! 等结果
            let (response, sched_rx) = if let Some(mut rx) = sched_rx {
                let req_json_clone = req_json.clone();
                let client = self.client.clone();
                let base_url = self.base_url.clone();
                let api_key = self.api_key.clone();
                let api_format = self.api_format;
                let app = self.app.clone();
                let sid = self.sid.clone();
                let run_id_clone = self.run_id.clone();
                let cancel_token = self.cancel_token.clone();
                let ctx = self.ctx.clone();
                let api_handle = tokio::spawn(async move {
                    let api_request = api_client::api_call_with_retry(
                        &client, &base_url, &req_json_clone, &api_key, api_format, 3, &app, &sid,
                    );
                    let timeout_result = tokio::time::timeout(Duration::from_secs(120), api_request);
                    tokio::select! {
                        result = timeout_result => {
                            match result {
                                Ok(inner) => inner.map(|r| Some(r)),
                                Err(_) => {
                                    let error = ApiError::Network("API 请求超过 120 秒未返回响应头，已自动终止。".to_string());
                                    let _ = agent_runs::fail_run(&app, &run_id_clone, error.to_string());
                                    *ctx.cancel_token.lock().await = None;
                                    Err(error.into())
                                }
                            }
                        }
                        _ = cancel_token.cancelled() => {
                            Ok(None)
                        }
                    }
                });

                tokio::select! {
                    result = api_handle => {
                        match result {
                            Ok(Ok(Some(resp))) => (Some(resp), Some(rx)),
                            Ok(Ok(None)) => { *self.ctx.scheduler_rx.lock().await = Some(rx); continue; },
                            Ok(Err(e)) => {
                                // API 调用失败 → 记录诊断日志后返回 Err
                                println!("[JARVIS] API 调用失败，终止主循环: {}", e);
                                let messages_json = {
                                    let session = self.ctx.memory.lock().await;
                                    serde_json::to_string_pretty(&session.messages).unwrap_or_default()
                                };
                                crate::infra::debug_logger::debug_logger().log_api_error(
                                    &self.sid, "MAIN", self.total_loop_count + 1,
                                    &e.to_string(), &messages_json,
                                );
                                *self.ctx.scheduler_rx.lock().await = Some(rx);
                                return Err(e.into());
                            }
                            Err(_) => { *self.ctx.scheduler_rx.lock().await = Some(rx); continue; },
                        }
                    }
                    event = rx.recv() => {
                        if let Some(ev) = event {
                            let (_needs_llm, scheduler_done) = self.handle_sched_event(ev).await;
                            if !scheduler_done {
                                // 调度器未结束，放回 receiver 继续等
                                *self.ctx.scheduler_rx.lock().await = Some(rx);
                            }
                            // scheduler_done 时 handle_sched_event 已清除 rx，不放回
                        } else {
                            // channel 关闭（调度器异常退出），不放回
                        }
                        self.loop_count += 1;
                        self.total_loop_count += 1;
                        continue;
                    }
                    _ = self.cancel_token.cancelled() => {
                        *self.ctx.scheduler_rx.lock().await = Some(rx);
                        continue;
                    }
                }
            } else {
                // 无活跃调度器，正常阻塞等待 LLM
                let resp = match self.call_api_with_retry(&req_json).await {
                    Ok(Some(r)) => r,
                    Ok(None) => continue,
                    Err(e) => {
                        // API 调用失败 → 记录诊断日志后返回 Err
                        println!("[JARVIS] API 调用失败，终止主循环: {}", e);
                        let messages_json = {
                            let session = self.ctx.memory.lock().await;
                            serde_json::to_string_pretty(&session.messages).unwrap_or_default()
                        };
                        crate::infra::debug_logger::debug_logger().log_api_error(
                            &self.sid, "MAIN", self.total_loop_count + 1,
                            &e.to_string(), &messages_json,
                        );
                        return Err(e.into());
                    }
                };
                (Some(resp), None)
            };

            let Some(response) = response else { continue; };

            // 流式处理（含一次断流重试）
            let stream_result = {
                let mut stream = response.bytes_stream().eventsource();
                let mut result = process_stream(
                    &mut stream,
                    is_openai,
                    &self.app,
                    &self.sid,
                    &self.run_id,
                    self.total_loop_count + 1,
                    &self.cancel_token,
                    StreamConfig::default(),
                )
                .await;

                // 如果流提前结束且未收到工具调用也未收到正文，重试一次
                if result.text.is_empty() && !result.has_tool && !self.cancel_token.is_cancelled()
                {
                    println!("[JARVIS] 流式响应提前终止，尝试重试一次...");
                    match self.call_api_with_retry(&req_json).await {
                        Ok(Some(resp)) => {
                            let mut stream2 = resp.bytes_stream().eventsource();
                            result = process_stream(
                                &mut stream2,
                                is_openai,
                                &self.app,
                                &self.sid,
                                &self.run_id,
                                self.total_loop_count + 1,
                                &self.cancel_token,
                                StreamConfig::default(),
                            )
                            .await;
                        }
                        Ok(None) => {}
                        Err(e) => {
                            println!("[JARVIS] 流式重试失败: {}", e);
                        }
                    }
                }

                result
            };

            let (
                mut current_blocks,
                tool_input_buffers,
                current_text_this_turn,
                mut current_thinking_this_turn,
                turn_has_tool,
                turn_in_tokens,
                turn_out_tokens,
            ) = (
                stream_result.blocks,
                stream_result.tool_input_buffers,
                stream_result.text,
                stream_result.thinking,
                stream_result.has_tool,
                stream_result.input_tokens,
                stream_result.output_tokens,
            );

            // 检测输出截断状态
            let is_truncated = matches!(
                stream_result.stop_reason.as_deref(),
                Some("max_tokens") | Some("length")
            );
            if is_truncated {
                println!(
                    "[JARVIS] 检测到输出截断 (stop_reason={:?})，将在工具结果中提示 LLM",
                    stream_result.stop_reason
                );
            }

            let tool_input_buffers_count = tool_input_buffers.len();
            self.req_input_tokens += turn_in_tokens;
            self.req_output_tokens += turn_out_tokens;
            if turn_in_tokens > 0 || turn_out_tokens > 0 {
                self.update_provider_usage_snapshot(turn_in_tokens, turn_out_tokens);
            }

            // 提取工具调用信息
            let tool_calls: Vec<(String, String)> = tool_input_buffers
                .iter()
                .filter_map(|(idx, buf)| {
                    if let Some(ContentBlock::ToolUse { name, .. }) = current_blocks.get(*idx) {
                        Some((name.clone(), buf.clone()))
                    } else {
                        None
                    }
                })
                .collect();

            // 提取工具名称供反思审查使用
            let tool_names_for_reflection: Vec<String> =
                tool_calls.iter().map(|(name, _)| name.clone()).collect();

            // 记录响应摘要
            debug_logger::debug_logger().log_response(
                &self.sid,
                "MAIN",
                self.total_loop_count + 1,
                current_text_this_turn.len(),
                current_thinking_this_turn.len(),
                tool_calls.len(),
                turn_in_tokens,
                turn_out_tokens,
            );

            // 记录思考过程
            debug_logger::debug_logger().log_thoughts(
                &self.sid,
                "MAIN",
                self.total_loop_count + 1,
                &current_thinking_this_turn,
                &current_text_this_turn,
                &tool_calls,
                self.req_input_tokens,
                self.req_output_tokens,
            );

            // 工具执行
            let work_mode = self.ctx.agent_work_mode.lock().await.clone();
            let (mut tool_results, manual_compact, sub_in, sub_out) = execute_tool_calls(
                &mut current_blocks,
                tool_input_buffers,
                &self.app,
                &self.sid,
                &self.run_id,
                self.total_loop_count + 1,
                &self.cancel_token,
                &self.detected_intent,
                &work_mode,
            )
            .await;
            self.req_input_tokens += sub_in;
            self.req_output_tokens += sub_out;

            // 截断感知：如果输出被 max_tokens 截断导致工具参数不完整，
            // 在错误的 ToolResult 中追加明确提示，让 LLM 知道原因并调整策略
            if is_truncated && turn_has_tool {
                let has_parse_error = tool_results.iter().any(|block| {
                    if let ContentBlock::ToolResult { content, .. } = block {
                        content.contains("参数解析失败")
                    } else {
                        false
                    }
                });
                if has_parse_error {
                    // 找到最后一个解析失败的 ToolResult，追加截断提示
                    for block in tool_results.iter_mut().rev() {
                        if let ContentBlock::ToolResult { content, .. } = block {
                            if content.contains("参数解析失败") {
                                content.push_str(
                                    "\n\n⚠️ 上述参数解析失败的原因是：你的输出被 max_tokens 截断了，\
                                    工具调用的 JSON 参数不完整。请减少单次输出量——\
                                    每次只创建 1-2 个文件，而非一次性生成所有文件。"
                                );
                                break;
                            }
                        }
                    }
                    println!("[JARVIS] 已在 ToolResult 中注入截断提示");
                }
            }

            let _ = self.app.emit(
                "chat-turn-end",
                json!({
                    "has_tool": turn_has_tool,
                    "sessionId": self.sid,
                    "loopCount": self.total_loop_count + 1
                }),
            );

            // 调度器 receiver 放回 ctx，下一轮 select! 继续用
            if let Some(rx) = sched_rx {
                let mut slot = self.ctx.scheduler_rx.lock().await;
                if slot.is_none() {
                    *slot = Some(rx);
                }
            }

            if self.cancel_token.is_cancelled() {
                continue;
            }

            // 存储助手回复
            self.store_assistant_response(&current_blocks).await;

            // 检查工具是否请求结束本轮循环（如 ProposePlan 提交方案后等待用户审批）
            {
                let mut flags = self.ctx.tool_result_flags.lock().await;
                let should_break = flags.values().any(|(brk, _)| *brk);
                if should_break || tool_results.is_empty() {
                    println!(
                        "[JARVIS] break诊断: should_break={}, flags={:?}, tool_results.len()={}",
                        should_break,
                        flags.iter().map(|(k, (brk, err))| format!("{}→break={},err={}", k, brk, err)).collect::<Vec<_>>(),
                        tool_results.len()
                    );
                }
                flags.clear();
                if should_break {
                    // break 前必须存储 tool_results，否则下次 pipeline 会因
                    // Assistant(tool_calls) 后缺少 ToolResult 导致 API 400 错误
                    if !tool_results.is_empty() {
                        let mut session = self.ctx.memory.lock().await;
                        append_message(&mut session, Message::User {
                            content: Content::Multiple(tool_results.clone()),
                        }, "chat");
                    }
                    let tool_summary: String = tool_results.iter()
                        .filter_map(|block| {
                            if let ContentBlock::ToolResult { content, .. } = block {
                                Some(content.as_str())
                            } else {
                                None
                            }
                        })
                        .collect::<Vec<_>>()
                        .join("\n\n");
                    // 保存工具结果摘要，传递给前端 toolBuffer
                    self.tool_execution_summary = if tool_summary.is_empty() { None } else { Some(tool_summary.clone()) };
                    self.final_answer = if current_text_this_turn.trim().is_empty() {
                        tool_summary
                    } else if tool_summary.is_empty() {
                        current_text_this_turn
                    } else {
                        format!("{}\n\n{}", current_text_this_turn, tool_summary)
                    };
                    println!("[JARVIS] 主循环: 工具请求 break_loop，结束本轮");
                    break;
                }
            }

            // 判断是否继续循环
            println!(
                "[JARVIS] 主循环判断: tool_results.is_empty()={}, tool_results.len()={}, has_tool={}, tool_input_buffers.len()={}, text_len={}",
                tool_results.is_empty(), tool_results.len(), turn_has_tool,
                tool_input_buffers_count, current_text_this_turn.len()
            );
            if tool_results.is_empty() {
                self.final_answer = current_text_this_turn;
                // 模型可能只返回 thinking 而没有 text（DeepSeek 等模型常见）
                // 此时将 thinking 提升为回复，避免前端空响应
                if self.final_answer.trim().is_empty()
                    && !current_thinking_this_turn.trim().is_empty()
                {
                    self.final_answer = std::mem::take(&mut current_thinking_this_turn);
                }

                // 第3层防御：响应后置拦截 — 仅在 Edit/Plan 模式下检测 LLM 是否绕过 ProposePlan
                // tool_results 为空说明 LLM 没用任何工具，纯文本输出了方案
                let work_mode = self.ctx.agent_work_mode.lock().await.clone();
                if (work_mode == "edit" || work_mode == "plan")
                    && crate::core::intent::plan_detector::detect_plan_in_text(&self.final_answer)
                {
                    println!(
                        "[JARVIS] 响应后置拦截：检测到正文中的计划内容，重定向到 ProposePlan"
                    );
                    let _ = self.app.emit(
                        "chat-stream",
                        json!({
                            "content": "\n> [!] **检测到计划性内容，正在重定向到方案审批流程...**\n",
                            "sessionId": self.sid,
                            "loopCount": self.total_loop_count + 1
                        }),
                    );

                    let redirect_msg = if work_mode == "plan" {
                        format!(
                            "【系统拦截通知】\n\
                            你当前处于规划模式，必须通过 ProposePlan 工具提交方案，不能在正文中直接输出计划内容。\n\
                            \n\
                            请调用 ProposePlan 工具，将你刚才的计划内容作为 content 参数提交。\n\
                            \n\
                            你刚才输出的内容摘要：\n{}",
                            self.final_answer.chars().take(500).collect::<String>()
                        )
                    } else {
                        format!(
                            "【系统拦截通知】\n\
                            你刚才在回复正文中输出了计划/步骤/方案内容，这违反了规则。\n\
                            计划必须通过 ProposePlan 工具提交到审批面板，不能写在正文里。\n\
                            \n\
                            请立即执行以下操作：\n\
                            1. 如果当前不在 Plan 模式，先调用 SwitchWorkMode(mode=\"plan\") 切换\n\
                            2. 调用 ProposePlan 工具，将你刚才的计划内容作为 content 参数提交\n\
                            3. 等待用户审批\n\
                            \n\
                            你刚才输出的内容摘要：\n{}",
                            self.final_answer.chars().take(500).collect::<String>()
                        )
                    };

                    {
                        let mut session = self.ctx.memory.lock().await;
                        append_message(&mut session, Message::User {
                            content: Content::Single(redirect_msg),
                        }, "internal");
                    }

                    self.loop_count += 1;
                    self.total_loop_count += 1;
                    continue;
                }

                {
                    let session = self.ctx.memory.lock().await;
                    agent_runs::save_checkpoint(
                        &self.app,
                        &self.run_id,
                        &self.sid,
                        self.total_loop_count + 1,
                        session.messages.clone(),
                        self.req_input_tokens,
                        self.req_output_tokens,
                        "模型已给出最终回复",
                    );
                }

                // 调度器仍在运行时，不退出循环——等待调度器事件
                if self.ctx.scheduler_rx.lock().await.is_some() {
                    println!("[JARVIS] 主循环: LLM 无工具调用但调度器仍在运行，等待调度器事件");
                    // 取出 receiver，释放锁后再 await
                    let rx_opt = self.ctx.scheduler_rx.lock().await.take();
                    if let Some(mut rx) = rx_opt {
                        tokio::select! {
                            event = rx.recv() => {
                                if let Some(ev) = event {
                                    let (_needs_llm, scheduler_done) = self.handle_sched_event(ev).await;
                                    if !scheduler_done {
                                        // 调度器未结束，放回 receiver 继续等
                                        *self.ctx.scheduler_rx.lock().await = Some(rx);
                                    }
                                    // scheduler_done=true 时 handle_sched_event 已清除 rx，不放回
                                } else {
                                    // channel 关闭 = 调度器异常退出，不放回
                                }
                            }
                            _ = self.cancel_token.cancelled() => {
                                *self.ctx.scheduler_rx.lock().await = Some(rx);
                            }
                        }
                    }
                    // rx 已被取走（AllDone/异常/取消），回到循环顶部
                    // 如果 rx 被放回 → 下一轮继续等待
                    // 如果 rx 未放回 → 下一轮 scheduler_rx.is_some()=false → break
                    self.loop_count += 1;
                    self.total_loop_count += 1;
                    continue;
                }

                break;
            } else {
                println!("[JARVIS] 主循环: tool_results 不为空，添加到历史消息并继续循环");
                // 先检查是否需要反思审查（在获取 session 锁之前）
                let should_reflect = super::reflection::strategy::should_reflect(
                    &self.reflection_mode,
                    self.loop_count,
                    self.total_reflections,
                    self.consecutive_reflection_nos,
                    &tool_names_for_reflection,
                    &current_thinking_this_turn,
                );

                // 添加工具结果到 session
                {
                    let mut session = self.ctx.memory.lock().await;
                    append_message(&mut session, Message::User {
                        content: Content::Multiple(tool_results),
                    }, "chat");
                } // session 锁在这里释放

                // —— 反思审查：工具结果已写入 session，审查 Agent 携带完整上下文判断 ——
                if should_reflect {
                    println!("[审查 Agent] 触发反思 (mode={}, model={})", self.reflection_mode, self.cfg.utility_model);
                    // 获取 session 消息用于审查
                    let session_messages = {
                        let session = self.ctx.memory.lock().await;
                        session.messages.clone()
                    };

                    match super::reflection::strategy::execute_review(
                        &self.client,
                        &self.api_key,
                        &self.base_url,
                        &self.cfg.utility_model,
                        self.api_format,
                        &session_messages,
                        &self.msg,
                        &current_blocks,
                        &[],
                    )
                    .await
                    {
                        Ok(super::reflection::ReflectionJudgment::Ok) => {
                            println!("[审查 Agent] 判断: OK");
                            self.total_reflections += 1;
                            self.consecutive_reflection_nos = 0;
                            let _ = self.app.emit("agent-step", json!({
                                "type": "reflection",
                                "sessionId": self.sid,
                                "loopCount": self.total_loop_count + 1,
                                "judgment": "ok",
                            }));
                        }
                        Ok(super::reflection::ReflectionJudgment::NotOk { reason, suggestion }) => {
                            println!(
                                "[审查 Agent] 判断: NO — {}\n建议: {}",
                                reason, suggestion
                            );
                            self.total_reflections += 1;
                            self.consecutive_reflection_nos += 1;
                            let _ = self.app.emit("agent-step", json!({
                                "type": "reflection",
                                "sessionId": self.sid,
                                "loopCount": self.total_loop_count + 1,
                                "judgment": "not_ok",
                                "reason": reason,
                                "suggestion": suggestion,
                            }));

                            // 注入修正建议到 session
                            let mut session = self.ctx.memory.lock().await;
                            append_message(&mut session, Message::User {
                                content: Content::Single(format!(
                                    "审查发现以下问题：{}\n建议修正：{}\n请根据建议修正后继续。",
                                    reason, suggestion
                                )),
                            }, "internal");
                        }
                        Err(e) => {
                            // 审查调用失败不影响主流程，仅记录日志
                            println!("[审查 Agent] 调用失败: {}", e);
                        }
                    }
                }

                // 执行后续操作
                let mut session = self.ctx.memory.lock().await;
                if manual_compact {
                    let _ = auto_compact(
                        &self.sid,
                        &mut session,
                        &self.client,
                        &self.api_key,
                        &self.base_url,
                        &self.model_id,
                        self.api_format,
                    )
                    .await;
                }
                agent_runs::save_checkpoint(
                    &self.app,
                    &self.run_id,
                    &self.sid,
                    self.total_loop_count + 1,
                    session.messages.clone(),
                    self.req_input_tokens,
                    self.req_output_tokens,
                    "工具结果已写回上下文",
                );
            }
            self.loop_count += 1;
            self.total_loop_count += 1;
            println!("[JARVIS] 主循环: 进入下一轮 loop_count={}, total_loop_count={}", self.loop_count, self.total_loop_count);

            if self.total_loop_count >= crate::infra::types::constants::MAX_AGENT_LOOP_ABSOLUTE {
                self.final_answer = format!(
                    "代理执行超过绝对上限 {} 轮，为防止死循环已强制停止。",
                    crate::infra::types::constants::MAX_AGENT_LOOP_ABSOLUTE
                );
                break;
            }
        }

        Ok(())
    }

    /// 阶段 5: 检查点创建 + 会话保存 + 记忆代理 + 结果组装
    async fn finalize(mut self) -> JarvisResult {
        let was_cancelled = self.cancel_token.is_cancelled();
        let was_loop_timeout = *self.ctx.loop_continuation_pending.lock().await;

        // 崩溃兜底：提交前先把补丁持久化到 agent_run_patches 表
        {
            let manager = self.app.state::<crate::infra::state::state::SessionManager>();
            let ctx = manager.get_or_create(&self.sid).await;
            let patches: Vec<_> = ctx.pending_patches.lock().await.clone();
            for p in &patches {
                let _ = crate::infra::db::insert_agent_run_patch(
                    &self.sid,
                    &p.run_id,
                    p.seq,
                    &p.patch,
                    p.message.as_deref(),
                    p.trigger_user_memory_index,
                    p.trigger_user_message_id.as_deref(),
                );
            }
        }

        // 创建检查点快照（仅在有文件编辑时创建实快照，纯聊天轮次不创建）
        {
            let has_patches =
                crate::core::tools::file_tools::has_pending_patches(&self.app, &self.sid).await;

            let checkpoint_id = if has_patches {
                crate::core::tools::file_tools::commit_pending_snapshot(
                    &self.app,
                    &self.sid,
                    self.user_msg_preview.clone(),
                    Some(self.initial_msg_index),
                )
                .await
            } else {
                // 纯聊天轮次 → 不创建快照，仅记录日志
                println!("[JARVIS] 纯聊天轮次，跳过快照创建");
                None
            };

            if let Some(id) = &checkpoint_id {
                println!("[JARVIS] 已创建本轮文件快照: {} (来自快照引擎)", id);
            }
            let _ = self.app.emit(
                "checkpoint-created",
                serde_json::json!({
                    "sessionId": self.sid,
                    "checkpointId": checkpoint_id,
                    "hasPatches": has_patches,
                    "canRollback": true,
                    "message": self.user_msg_preview
                }),
            );
        }

        let memory = self.ctx.memory.lock().await.clone();
        let session_meta = if memory.messages.is_empty() && was_cancelled {
            None
        } else {
            let meta = if was_cancelled {
                crate::core::session::save_session(&self.sid, &memory, None)
            } else {
                crate::core::session::save_session(
                    &self.sid,
                    &memory,
                    Some((self.req_input_tokens, self.req_output_tokens)),
                )
            };
            println!("[JARVIS] 会话 {} 已自动保存", self.sid);
            Some(meta)
        };
        if let Some(ref meta) = session_meta {
            let _ = self.app.emit("session-updated", ());

            if !was_cancelled && meta.message_count >= 2 && meta.title_source == "default" {
                let app_clone = self.app.clone();
                let sid_clone = self.sid.clone();
                let memory_clone = memory.clone();
                tokio::spawn(async move {
                    if let Err(e) = crate::command::session::auto_name_session(
                        app_clone,
                        sid_clone,
                        memory_clone,
                    )
                    .await
                    {
                        println!("[JARVIS] Auto-naming failed: {}", e);
                    }
                });
            }
        }

        // 记忆代理
        let reply_for_memory = self.final_answer.clone();
        let cfg_clone = self.cfg.clone();
        let sid_for_memory = self.sid.clone();
        let app_for_memory = self.app.clone();
        tokio::spawn(async move {
            run_memory_agent(app_for_memory, self.user_msg_for_memory, reply_for_memory, cfg_clone, sid_for_memory).await;
        });

        let status = if was_cancelled {
            "CANCELLED"
        } else if was_loop_timeout {
            "PAUSED_LOOP_LIMIT"
        } else {
            "FINISH"
        };
        if was_loop_timeout {
            self.final_answer = format!(
                "代理执行已达到 {} 回合上限，等待用户确认是否继续。",
                crate::infra::types::constants::MAX_AGENT_LOOP_BEFORE_CONFIRM
            );
        }

        // 会话日志
        {
            debug_logger::debug_logger().log_session_summary(
                &self.sid,
                self.req_input_tokens,
                self.req_output_tokens,
                status,
            );
        }

        // Agent Run 完成（超时续跑不标记完成）
        if !was_cancelled && !was_loop_timeout {
            agent_runs::complete_run(
                &self.app,
                &self.run_id,
                self.req_input_tokens,
                self.req_output_tokens,
                Some(self.final_answer.chars().take(180).collect()),
            );
        }

        let session_input_tokens = session_meta
            .as_ref()
            .map(|meta| meta.total_input_tokens)
            .unwrap_or(0);
        let session_output_tokens = session_meta
            .as_ref()
            .map(|meta| meta.total_output_tokens)
            .unwrap_or(0);

        {
            let mut active_run_id = self.ctx.active_run_id.lock().await;
            if active_run_id.as_deref() == Some(&self.run_id) {
                *active_run_id = None;
            }
        }
        *self.ctx.cancel_token.lock().await = None;

        JarvisResult {
            status: status.to_string(),
            content: self.final_answer,
            input_tokens: self.req_input_tokens,
            output_tokens: self.req_output_tokens,
            session_input_tokens,
            session_output_tokens,
            user_message_id: None,
            tool_execution_summary: self.tool_execution_summary,
        }
    }

    // ─── 主循环辅助方法 ───

    /// 处理错误中止：记录错误事件、标记 run 失败、清理状态
    async fn abort_after_error(&self, error: &AgentError) {
        if !self.run_id.is_empty() {
            // 记录错误事件到 agent_run_events 表
            agent_runs::record_tool_result(
                &self.app,
                &self.run_id,
                "pipeline",
                Some(error.to_string()),
                None,
                self.total_loop_count,
            );
            agent_runs::fail_run(&self.app, &self.run_id, error.to_string());
            let mut active_run_id = self.ctx.active_run_id.lock().await;
            if active_run_id.as_deref() == Some(&self.run_id) {
                *active_run_id = None;
            }
        }
        *self.ctx.cancel_token.lock().await = None;
    }

    async fn handle_cancellation(&mut self) {
        println!(
            "[JARVIS] 用户已取消执行，保留用户消息并恢复部分输出，user index {}",
            self.initial_msg_index
        );

        // 直接查 agent_runs 表的 live_content（不受 running 状态保护逻辑影响）
        let run = crate::core::orchestration::agent_run_repository::list_runs(Some(&self.sid))
            .ok()
            .and_then(|runs| runs.into_iter().find(|r| r.run_id == self.run_id));
        let live_content = run.as_ref().map(|r| r.live_content.clone()).unwrap_or_default();
        let live_thinking = run.as_ref().map(|r| r.live_thinking.clone()).unwrap_or_default();

        let mut answer = if !live_content.trim().is_empty() {
            live_content
        } else if !live_thinking.trim().is_empty() {
            live_thinking
        } else if !self.final_answer.is_empty()
            && self.final_answer != "用户已取消执行。"
        {
            std::mem::take(&mut self.final_answer)
        } else {
            String::new()
        };

        if answer.trim().is_empty() {
            answer = "用户已取消执行，无部分结果。".to_string();
        } else {
            answer = format!(
                "{}\n\n> ✕ **用户已取消执行（以上为部分结果）**",
                answer
            );
        }

        {
            let mut session = self.ctx.memory.lock().await;
            let keep_len = (self.initial_msg_index + 1).min(session.messages.len());
            session.messages.truncate(keep_len);
            session.message_ids.truncate(keep_len);
            self.final_answer = answer.clone();
            append_message(&mut session, Message::Assistant {
                content: Content::Single(self.final_answer.clone()),
            }, "chat");
        }
        let _ = self.app.emit(
            "chat-stream",
            json!({
                "content": "\n> ✕ **用户已取消执行**\n",
                "sessionId": self.sid,
                "loopCount": self.total_loop_count + 1
            }),
        );
        let _ = self.app.emit(
            "agent-step",
            json!({
                "type": "cancelled",
                "sessionId": self.sid,
                "loopCount": self.total_loop_count + 1
            }),
        );
        agent_runs::cancel_run(
            &self.app,
            &self.run_id,
            self.req_input_tokens,
            self.req_output_tokens,
            Some(self.final_answer.clone()),
        );
    }

    /// 循环上限确认：请求用户授权继续
    /// 返回 true 表示继续，false 表示终止
    async fn request_loop_continuation(&mut self) -> bool {
        let _ = self.app.emit(
            "chat-stream",
            json!({
                "content": format!("\n> **代理执行已达到 {} 回合，正在等待用户确认是否继续...**\n", crate::infra::types::constants::MAX_AGENT_LOOP_BEFORE_CONFIRM),
                "sessionId": self.sid,
                "loopCount": self.total_loop_count + 1
            }),
        );
        let decision = request_permission(
            &self.app,
            &self.sid,
            &format!(
                "代理执行已达到 {} 回合，可能任务较为复杂或陷入循环。是否继续执行？",
                crate::infra::types::constants::MAX_AGENT_LOOP_BEFORE_CONFIRM
            ),
        )
        .await;
        if decision == "allow" || decision == "allow_session" {
            // 清除待续跑标记（用户及时响应了）
            *self.ctx.loop_continuation_pending.lock().await = false;
            self.loop_count = 0;
            let _ = self.app.emit(
                "chat-stream",
                json!({
                    "content": "\n> **用户已授权继续执行。**\n",
                    "sessionId": self.sid,
                    "loopCount": self.total_loop_count + 1
                }),
            );
            true
        } else {
            // 超时或用户拒绝 — 标记待续跑，不立即设 final_answer
            // 如果是超时：用户稍后仍可通过 resolve_permission 发出 allow 来 resume
            *self.ctx.loop_continuation_pending.lock().await = true;
            false
        }
    }

    /// 注入后台任务完成通知到会话中
    async fn drain_background_notifications(&self) {
        let notifs =
            crate::infra::background::BackgroundManager::drain_notifications(&self.app).await;
        if !notifs.is_empty() {
            let mut notif_text = String::new();
            for n in notifs {
                notif_text.push_str(&format!("[bg:{}] {}: {}\n", n.task_id, n.status, n.result));
            }
            let mut session = self.ctx.memory.lock().await;
            append_message(&mut session, Message::User {
                content: Content::Single(format!(
                    "<background-results>\n{}\n</background-results>",
                    notif_text
                )),
            }, "background");
            append_message(&mut session, Message::Assistant {
                content: Content::Single("Noted background results.".to_string()),
            }, "background");
        }
    }

    /// 检查 token 用量并在需要时执行压缩
    async fn compact_if_needed(&mut self) {
        let (messages_for_estimate, sources_for_estimate) = {
            let session = self.ctx.memory.lock().await;
            (session.messages.clone(), session.sources.clone())
        };
        let history_snapshot = self.prepare_history_snapshot_from_messages(
            messages_for_estimate,
            &sources_for_estimate,
        );
        let tools = self.current_tools();
        let estimate = self.build_context_estimate(&history_snapshot, &tools);
        let tokens = estimate.estimated_tokens;
        let trigger = crate::infra::types::constants::MAX_TOKENS_COMPACT_TRIGGER;

        // >70% 上限：LLM 摘要压缩
        if tokens > trigger * 70 / 100 {
            println!(
                "[贾维斯] 上下文 > {}% 上限 ({}/{}), 触发 LLM 摘要压缩",
                70, tokens, trigger
            );

            let mut session = self.ctx.memory.lock().await;
            let mut last_user_msg = None;
            if let Some(Message::User { .. }) = session.messages.last() {
                last_user_msg = pop_message(&mut session);
            }

            let compact_result = auto_compact(
                &self.sid,
                &mut session,
                &self.client,
                &self.api_key,
                &self.base_url,
                &self.cfg.utility_model,
                self.api_format,
            )
            .await;

            if let Err(e) = compact_result {
                println!("[JARVIS] 自动压缩失败: {}，继续使用原始上下文", e);
            } else {
                self.initial_msg_index = session.messages.len();
            }

            if let Some((msg, message_id)) = last_user_msg {
                let needs_assistant_pad = match session.messages.last() {
                    Some(Message::User { .. }) => true,
                    None => true,
                    _ => false,
                };
                if needs_assistant_pad {
                    append_message(&mut session, Message::Assistant {
                        content: Content::Single("Context compressed.".to_string()),
                    }, "internal");
                }
                self.initial_msg_index = session.messages.len();
                restore_message(&mut session, msg, message_id, "chat");
            }
        }
    }

    fn current_tools(&self) -> Vec<serde_json::Value> {
        get_tools_definition()
    }

    fn prepare_history_snapshot_from_messages(
        &self,
        messages: Vec<Message>,
        sources: &[String],
    ) -> Vec<Message> {
        // 过滤掉 internal/background 消息，LLM 不需要看到系统内部通知
        // 同时计算 initial_msg_index 的偏移量
        let mut filtered = Vec::new();
        let mut index_shift = 0usize;
        for (i, (msg, src)) in messages.into_iter().zip(sources.iter()).enumerate() {
            if matches!(src.as_str(), "chat" | "compact" | "context") {
                filtered.push(msg);
            } else if i < self.initial_msg_index {
                index_shift += 1;
            }
        }
        let mut messages = filtered;
        let adjusted_msg_index = self.initial_msg_index.saturating_sub(index_shift);

        let mut history_snapshot = if self.detected_intent == "CHAT" {
            for msg in &mut messages {
                if let Message::User { content } = msg {
                    if let Content::Multiple(blocks) = content {
                        for block in blocks {
                            if let ContentBlock::ToolResult {
                                ref mut content, ..
                            } = block
                            {
                                // 错误信息对 LLM 决策至关重要，保留不截断
                                let is_error = content.contains("错误")
                                    || content.contains("失败")
                                    || content.contains("error")
                                    || content.contains("Error")
                                    || content.contains("denied")
                                    || content.contains("拒绝");
                                if !is_error {
                                    *content =
                                        "[系统截断：为闲聊模式节省Token，工具返回的冗长详情已被折叠。]"
                                            .to_string();
                                }
                            }
                        }
                    }
                }
            }
            messages
        } else {
            messages
        };

        // 防御性检查：确保每个 Assistant(tool_calls) 后跟 ToolResult
        // 修复流式中断、并发修改等边缘 case 导致的消息序列断裂
        fix_broken_tool_call_pairs(&mut history_snapshot);

        restore_image_data(&mut history_snapshot);
        inject_context_into_history(
            &mut history_snapshot,
            adjusted_msg_index,
            &self.dynamic_context_str,
        );

        history_snapshot
    }

    /// 准备历史消息快照（含图片恢复 + 上下文注入）
    async fn prepare_history_snapshot(&self) -> Vec<Message> {
        let session = self.ctx.memory.lock().await;
        let messages = session.messages.clone();
        let sources = session.sources.clone();
        drop(session); // 释放锁
        self.prepare_history_snapshot_from_messages(messages, &sources)
    }

    /// 将消息列表转换为人类可读的对话文本，用于上下文快照展示
    fn format_messages_readable(messages: &[Message]) -> String {
        let mut out = String::new();
        for (i, msg) in messages.iter().enumerate() {
            let idx = i + 1;
            match msg {
                Message::User { content } => {
                    out.push_str(&format!("[User] (msg {})\n", idx));
                    match content {
                        Content::Single(text) => {
                            let trimmed = text.trim();
                            if !trimmed.is_empty() {
                                out.push_str(trimmed);
                                out.push('\n');
                            }
                        }
                        Content::Multiple(blocks) => {
                            for block in blocks {
                                match block {
                                    ContentBlock::Text { text } => {
                                        let trimmed = text.trim();
                                        if !trimmed.is_empty() {
                                            out.push_str(trimmed);
                                            out.push('\n');
                                        }
                                    }
                                    ContentBlock::ToolResult {
                                        tool_use_id,
                                        content: tc,
                                    } => {
                                        let tc_lines: Vec<&str> = tc.lines().collect();
                                        let preview = if tc_lines.len() > 3 {
                                            format!("{}\n  …", tc_lines[..3].join("\n"))
                                        } else {
                                            tc_lines.join("\n")
                                        };
                                        let short_id = &tool_use_id[tool_use_id.len().saturating_sub(12)..];
                                        out.push_str(&format!(
                                            "  ← ToolResult: {}\n    {}\n",
                                            short_id, preview
                                        ));
                                    }
                                    ContentBlock::Image { .. } => {
                                        out.push_str("  ← [Image]\n");
                                    }
                                    _ => {}
                                }
                            }
                        }
                    }
                }
                Message::Assistant { content } => {
                    out.push_str(&format!("[Assistant] (msg {})\n", idx));
                    match content {
                        Content::Single(text) => {
                            let trimmed = text.trim();
                            if !trimmed.is_empty() {
                                out.push_str(trimmed);
                                out.push('\n');
                            }
                        }
                        Content::Multiple(blocks) => {
                            for block in blocks {
                                match block {
                                    ContentBlock::Text { text } => {
                                        let trimmed = text.trim();
                                        if !trimmed.is_empty() {
                                            out.push_str(trimmed);
                                            out.push('\n');
                                        }
                                    }
                                    ContentBlock::ToolUse {
                                        id: _,
                                        name,
                                        input,
                                    } => {
                                        let input_str =
                                            serde_json::to_string(input).unwrap_or_default();
                                        let truncated = if input_str.len() > 200 {
                                            let mut end = 200;
                                            while end > 0 && !input_str.is_char_boundary(end) {
                                                end -= 1;
                                            }
                                            format!("{}…", &input_str[..end])
                                        } else {
                                            input_str
                                        };
                                        out.push_str(&format!(
                                            "  → ToolCall: {}({})\n",
                                            name, truncated
                                        ));
                                    }
                                    ContentBlock::Thinking { thinking, .. } => {
                                        let preview = if thinking.len() > 80 {
                                            let mut end = 80;
                                            while end > 0 && !thinking.is_char_boundary(end) {
                                                end -= 1;
                                            }
                                            format!("{}…", &thinking[..end])
                                        } else {
                                            thinking.clone()
                                        };
                                        out.push_str(&format!(
                                            "  … Thinking: {}\n",
                                            preview.replace('\n', " ")
                                        ));
                                    }
                                    _ => {}
                                }
                            }
                        }
                    }
                }
            }
            out.push('\n');
        }
        out
    }

    fn build_context_estimate(
        &self,
        history_snapshot: &[Message],
        tools: &[serde_json::Value],
    ) -> ContextEstimate {
        fn section(
            model_id: &str,
            key: &str,
            label: &str,
            content: String,
            item_count: usize,
        ) -> ContextSectionSnapshot {
            let chars = content.chars().count();
            let token_count = crate::infra::llm::token_count::count_text(model_id, &content);
            ContextSectionSnapshot {
                key: key.to_string(),
                label: label.to_string(),
                chars,
                estimated_tokens: token_count.tokens,
                token_count_method: token_count.method.as_str().to_string(),
                item_count,
                content,
                truncated: false,
                raw_content: None,
            }
        }

        fn section_with_raw(
            model_id: &str,
            key: &str,
            label: &str,
            content: String,
            item_count: usize,
            raw: String,
        ) -> ContextSectionSnapshot {
            let chars = content.chars().count();
            let token_count = crate::infra::llm::token_count::count_text(model_id, &content);
            ContextSectionSnapshot {
                key: key.to_string(),
                label: label.to_string(),
                chars,
                estimated_tokens: token_count.tokens,
                token_count_method: token_count.method.as_str().to_string(),
                item_count,
                content,
                truncated: false,
                raw_content: Some(raw),
            }
        }

        fn strip_dynamic_context(
            messages: &[Message],
            initial_msg_index: usize,
            dynamic_context: &str,
        ) -> Vec<Message> {
            let mut cleaned = messages.to_vec();
            if dynamic_context.is_empty() {
                return cleaned;
            }
            if let Some(Message::User { content }) = cleaned.get_mut(initial_msg_index) {
                match content {
                    Content::Single(text) => {
                        let prefix = format!("{}\n\n[User Input]:\n", dynamic_context);
                        if let Some(rest) = text.strip_prefix(&prefix) {
                            *text = rest.to_string();
                        }
                    }
                    Content::Multiple(blocks) => {
                        if let Some(ContentBlock::Text { text }) = blocks.first() {
                            if text.trim() == dynamic_context.trim() {
                                blocks.remove(0);
                            }
                        }
                    }
                }
            }
            cleaned
        }

        fn count_blocks(messages: &[Message]) -> (usize, usize, usize, usize) {
            let mut tool_calls = 0;
            let mut tool_results = 0;
            let mut images = 0;
            let mut thinking = 0;
            for message in messages {
                let Content::Multiple(blocks) = (match message {
                    Message::User { content } | Message::Assistant { content } => content,
                }) else {
                    continue;
                };
                for block in blocks {
                    match block {
                        ContentBlock::ToolUse { .. } => tool_calls += 1,
                        ContentBlock::ToolResult { .. } => tool_results += 1,
                        ContentBlock::Image { .. } => images += 1,
                        ContentBlock::Thinking { .. } => thinking += 1,
                        ContentBlock::Text { .. } => {}
                    }
                }
            }
            (tool_calls, tool_results, images, thinking)
        }

        let cleaned_messages = strip_dynamic_context(
            history_snapshot,
            self.initial_msg_index,
            &self.dynamic_context_str,
        );
        let (tool_call_count, tool_result_count, image_count, thinking_count) =
            count_blocks(history_snapshot);
        let messages_text = Self::format_messages_readable(&cleaned_messages);
        let messages_json = serde_json::to_string_pretty(&cleaned_messages).unwrap_or_default();
        let tools_json = serde_json::to_string_pretty(tools).unwrap_or_default();
        let mut sections = vec![
            section(
                &self.model_id,
                "system",
                "System Prompt",
                self.system_prompt.clone(),
                1,
            ),
            section(
                &self.model_id,
                "dynamic",
                "Dynamic Context",
                self.dynamic_context_str.clone(),
                1,
            ),
            section_with_raw(
                &self.model_id,
                "messages",
                "Session Messages",
                messages_text,
                cleaned_messages.len(),
                messages_json,
            ),
            section_with_raw(
                &self.model_id,
                "tools",
                "Tools Schema",
                tools_json.clone(),
                tools.len(),
                tools_json,
            ),
        ];
        if image_count > 0 {
            sections.push(section(
                &self.model_id,
                "attachments",
                "Attachments / Images",
                format!("当前请求中包含 {} 个图片块。近期图片会恢复为 base64，远期图片会折叠为文本摘要。", image_count),
                image_count,
            ));
        }
        if tool_result_count > 0 || thinking_count > 0 {
            sections.push(section(
                &self.model_id,
                "runtime",
                "Tool Results / Thinking",
                format!(
                    "tool_result: {}\nthinking: {}",
                    tool_result_count, thinking_count
                ),
                tool_result_count + thinking_count,
            ));
        }

        let total_chars = sections.iter().map(|item| item.chars).sum();
        let estimated_tokens = sections.iter().map(|item| item.estimated_tokens).sum();
        ContextEstimate {
            total_chars,
            estimated_tokens,
            message_count: cleaned_messages.len(),
            tool_schema_count: tools.len(),
            tool_call_count,
            tool_result_count,
            sections,
        }
    }

    /// 更新本轮请求的上下文 token 快照
    fn update_context_snapshot(&self, history_snapshot: &[Message], tools: &[serde_json::Value]) {
        fn now_ms() -> u64 {
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap_or_default()
                .as_millis() as u64
        }

        let estimate = self.build_context_estimate(history_snapshot, tools);
        let snapshot = SessionContextSnapshot {
            session_id: self.sid.clone(),
            run_id: Some(self.run_id.clone()),
            loop_count: self.total_loop_count + 1,
            model: self.model_id.clone(),
            intent: self.detected_intent.clone(),
            api_format: self.api_format.as_str().to_string(),
            created_at: now_ms(),
            total_chars: estimate.total_chars,
            estimated_tokens: estimate.estimated_tokens,
            provider_input_tokens: None,
            provider_output_tokens: None,
            provider_total_tokens: None,
            drift_percent: None,
            max_context_tokens: crate::infra::llm::registry::query_capabilities(&self.model_id)
                .and_then(|capabilities| capabilities.max_context_tokens),
            max_output_tokens: crate::infra::types::constants::MAX_TOKENS_CONTEXT,
            message_count: estimate.message_count,
            tool_schema_count: estimate.tool_schema_count,
            tool_call_count: estimate.tool_call_count,
            tool_result_count: estimate.tool_result_count,
            sections: estimate.sections,
        };

        if let Err(err) = crate::core::session::save_context_snapshot(&snapshot) {
            eprintln!("[JARVIS] 保存上下文快照失败: {}", err);
        }
        let _ = self.app.emit("context-snapshot-updated", &snapshot);
    }

    fn update_provider_usage_snapshot(&self, input_tokens: u64, output_tokens: u64) {
        let total_tokens = input_tokens.saturating_add(output_tokens);
        let drift = crate::core::session::get_context_snapshot(&self.sid)
            .ok()
            .flatten()
            .and_then(|snapshot| {
                crate::infra::llm::token_count::drift_percent(
                    snapshot.estimated_tokens,
                    input_tokens,
                )
            });

        match crate::core::session::update_context_snapshot_usage(
            &self.sid,
            input_tokens,
            output_tokens,
            total_tokens,
            drift,
        ) {
            Ok(Some(snapshot)) => {
                let _ = self.app.emit("context-snapshot-updated", &snapshot);
            }
            Ok(None) => {}
            Err(err) => eprintln!("[JARVIS] 更新上下文 usage 失败: {}", err),
        }
    }

    /// 解析 max_tokens：用户覆盖 > 模型注册表 > 常量兜底
    fn resolve_max_tokens(&self) -> i32 {
        self.cfg.max_tokens
            .or_else(|| {
                crate::infra::llm::registry::query_capabilities(&self.model_id)
                    .map(|cap| cap.max_tokens as i32)
            })
            .unwrap_or(crate::infra::types::constants::MAX_TOKENS_CONTEXT)
    }

    /// 构建 LLM API 请求体
    fn build_llm_request(
        &self,
        history_snapshot: Vec<Message>,
        audience: &str,
        work_mode: &str,
    ) -> (serde_json::Value, bool) {
        let system_prompt = crate::core::agent::prompts::get_system_prompt(audience, work_mode);
        let tools = self.current_tools();
        self.update_context_snapshot(&history_snapshot, &tools);

        let max_tokens = self.resolve_max_tokens();

        let mut request_body = AnthropicRequest {
            model: self.model_id.clone(),
            max_tokens,
            system: system_prompt.clone(),
            messages: history_snapshot,
            tools,
            stream: true,
            thinking: None,
            temperature: self.cfg.temperature,
            top_p: self.cfg.top_p,
            top_k: self.cfg.top_k,
        };

        request_body.thinking = Some(ThinkingConfig {
            r#type: Some(if self.should_think { "enabled" } else { "disabled" }.to_string()),
            budget_tokens: if self.should_think { Some(1024) } else { None },
            enable: None,
        });
        if self.should_think && request_body.max_tokens <= 1024 {
            request_body.max_tokens = 4096;
        }

        if self.api_format.is_openai() {
            use crate::infra::llm::adapters::{
                should_backfill_deepseek_reasoning_content,
                translate_messages_to_openai_with_reasoning_backfill, translate_tools_to_openai,
            };
            let backfill_reasoning = should_backfill_deepseek_reasoning_content(
                &self.model_id,
                &self.cfg.base_url,
                self.should_think,
            );
            let openai_msgs = translate_messages_to_openai_with_reasoning_backfill(
                &request_body.system,
                &request_body.messages,
                backfill_reasoning,
            );
            let openai_tools = translate_tools_to_openai(&request_body.tools);
            let mut openai_req = OpenAIRequest {
                model: self.model_id.clone(),
                max_tokens: Some(max_tokens),
                messages: openai_msgs,
                tools: if openai_tools.is_empty() {
                    None
                } else {
                    Some(openai_tools)
                },
                stream: true,
                stream_options: Some(StreamOptions {
                    include_usage: true,
                }),
                reasoning_effort: None,
                thinking: None,
                thinking_budget: None,
                enable_thinking: None,
                extra_body: None,
                parameters: None,
                temperature: request_body.temperature,
                top_p: request_body.top_p,
            };

            crate::infra::llm::registry::apply_thinking_for_model(
                &mut openai_req, &self.model_id, self.should_think,
            );
            (serde_json::to_value(openai_req).unwrap(), true)
        } else {
            (serde_json::to_value(request_body).unwrap(), false)
        }
    }

    /// 调用 LLM API（含取消检查）
    /// 返回 None 表示请求被取消，调用方应 continue 到下一轮循环
    async fn call_api_with_retry(
        &self,
        req_json: &serde_json::Value,
    ) -> Result<Option<reqwest::Response>, AgentError> {
        let api_request = api_client::api_call_with_retry(
            &self.client,
            &self.base_url,
            req_json,
            &self.api_key,
            self.api_format,
            3,
            &self.app,
            &self.sid,
        );

        match tokio::select! {
            result = tokio::time::timeout(Duration::from_secs(120), api_request) => {
                match result {
                    Ok(result) => result,
                    Err(_) => {
                        let error = ApiError::Network("API 请求超过 120 秒未返回响应头，已自动终止。".to_string());
                        // 记录错误事件到 agent_run_events 表
                        agent_runs::record_tool_result(
                            &self.app,
                            &self.run_id,
                            "api_call",
                            Some(error.to_string()),
                            None,
                            self.total_loop_count,
                        );
                        agent_runs::fail_run(&self.app, &self.run_id, error.to_string());
                        *self.ctx.cancel_token.lock().await = None;
                        return Err(error.into());
                    }
                }
            }
            _ = self.cancel_token.cancelled() => {
                return Ok(None);
            }
        } {
            Ok(resp) => Ok(Some(resp)),
            Err(e) => {
                // 记录错误事件到 agent_run_events 表
                agent_runs::record_tool_result(
                    &self.app,
                    &self.run_id,
                    "api_call",
                    Some(e.to_string()),
                    None,
                    self.total_loop_count,
                );
                agent_runs::fail_run(&self.app, &self.run_id, e.to_string());
                *self.ctx.cancel_token.lock().await = None;
                Err(e.into())
            }
        }
    }

    /// 存储助手回复到会话历史中
    async fn store_assistant_response(&self, current_blocks: &[ContentBlock]) {
        let mut session = self.ctx.memory.lock().await;
        let filtered_blocks: Vec<ContentBlock> = current_blocks
            .iter()
            .filter(|block| match block {
                ContentBlock::Text { text } => !text.trim().is_empty(),
                ContentBlock::Thinking { thinking, .. } => !thinking.trim().is_empty(),
                ContentBlock::ToolUse { .. }
                | ContentBlock::ToolResult { .. }
                | ContentBlock::Image { .. } => true,
            })
            .cloned()
            .collect();
        if !filtered_blocks.is_empty() {
            append_message(&mut session, Message::Assistant {
                content: Content::Multiple(filtered_blocks),
            }, "chat");
        }
    }
}

/// 主流程入口：依次执行 5 个阶段
pub async fn run_pipeline(
    session_id: String,
    msg: String,
    thinking_override: Option<bool>,
    image_base64_list: Option<Vec<String>>,
    agent_display_mode: Option<String>,
    reflection_mode_override: Option<String>,
    display_msg: Option<String>,
    app: tauri::AppHandle,
    session_manager: tauri::State<'_, crate::infra::state::state::SessionManager>,
    config_state: tauri::State<'_, crate::infra::config::config::ConfigState>,
) -> Result<JarvisResult, AgentError> {
    run_pipeline_inner(
        session_id,
        msg,
        thinking_override,
        image_base64_list,
        agent_display_mode,
        reflection_mode_override,
        display_msg,
        true,
        app,
        session_manager,
        config_state,
    )
    .await
}

pub async fn resume_pipeline(
    session_id: String,
    reason: String,
    app: tauri::AppHandle,
    session_manager: tauri::State<'_, crate::infra::state::state::SessionManager>,
    config_state: tauri::State<'_, crate::infra::config::config::ConfigState>,
) -> Result<JarvisResult, AgentError> {
    run_pipeline_inner(
        session_id,
        reason,
        None,
        None,
        None,
        None,
        None,
        false,
        app,
        session_manager,
        config_state,
    )
    .await
}

async fn run_pipeline_inner(
    session_id: String,
    msg: String,
    thinking_override: Option<bool>,
    image_base64_list: Option<Vec<String>>,
    agent_display_mode: Option<String>,
    reflection_mode_override: Option<String>,
    display_msg: Option<String>,
    inject_user_message: bool,
    app: tauri::AppHandle,
    session_manager: tauri::State<'_, crate::infra::state::state::SessionManager>,
    config_state: tauri::State<'_, crate::infra::config::config::ConfigState>,
) -> Result<JarvisResult, AgentError> {
    // 阶段 1: 初始化
    let mut state = PipelineState::setup(
        session_id,
        msg,
        thinking_override,
        image_base64_list,
        agent_display_mode,
        reflection_mode_override,
        display_msg,
        inject_user_message,
        app,
        session_manager,
        config_state,
    )
    .await?;

    // 阶段 2: 意图验证 (可能提前返回)
    if let Some(early_result) = state.validate().await? {
        return Ok(early_result);
    }

    // 阶段 3: 循环前准备
    state.pre_loop().await;

    // 阶段 4: 主循环
    if let Err(err) = state.run_main_loop().await {
        state.abort_after_error(&err).await;
        return Err(err);
    }

    // 阶段 5: 收尾
    Ok(state.finalize().await)
}
