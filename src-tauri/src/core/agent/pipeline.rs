//! # pipeline.rs — Agent 主循环流水线
//!
//! 实现 Agent 的 5 阶段执行流水线：初始化 → 意图验证 → 上下文构建 → 主循环 → 收尾。
//! 主循环阶段包含压缩检查、API 调用、流式处理、工具执行、反思审查等完整 Agent Loop 逻辑。
//!
//! ## 五阶段流水线地图（功能规划）
//!
//! 入口：`run_pipeline()`（新消息）/ `resume_pipeline()`（续跑）→ `run_pipeline_inner()` 按序调度：
//!
//! | 阶段 | 函数 | 职责 |
//! |---|---|---|
//! | 1 初始化 | `setup()` | 校验会话占用、加载配置、创建取消令牌、意图分类、组装 PipelineState |
//! | 2 意图验证 | `validate()` | DANGEROUS 弹权限确认 / UNCLEAR 返回澄清，可提前结束 |
//! | 3 上下文构建 | `pre_loop()` | 崩溃恢复、注入用户消息、创建 run 记录、决定是否深度思考 |
//! | 4 主循环 | `run_main_loop()` | 调 LLM → 流式解析 → 执行工具 → 结果回写，直到 LLM 不再调工具 |
//! | 5 收尾 | `finalize()` | 检查点快照、保存会话、自动起名、记忆超预算时后台整理、组装 JarvisResult |
//!
//! ### 阶段 4 主循环每轮内部子步骤
//! 取消检查 → 循环次数确认 → 后台通知注入 → 上下文压缩 → 历史快照 → 构建请求
//! → API 调用（含调度器事件 select）→ 流式处理 → 工具执行 → 反思审查 → 回写历史 → 下一轮
//!
//! ### 配套辅助函数（各阶段共用）
//! - 历史准备：`prepare_history_snapshot()` / `prepare_history_snapshot_from_messages()` / `fix_broken_tool_call_pairs()`
//! - 上下文监控：`build_context_estimate()` / `update_context_snapshot()` / `update_provider_usage_snapshot()` / `resolve_max_tokens()`
//! - 请求构建：`build_llm_request()` / `call_api_with_retry()` / `current_tools()`
//! - 流程控制：`handle_sched_event()` / `request_loop_continuation()` / `drain_background_notifications()` / `compact_if_needed()`
//! - 异常收尾：`abort_after_error()` / `handle_cancellation()` / `store_assistant_response()`
//!
//! ## 依赖
//! - Internal: `crate::core::orchestration::agent_runs`, `crate::infra::llm::api_client`, `crate::infra::config::config::AgentConfig`, `crate::core::complex_task`, `crate::core::session::memory`, `crate::core::tools`, `super::reflection`
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
use crate::infra::llm::api_client;
use crate::infra::types::models::*;
use crate::core::orchestration::agent_runs;
use crate::core::session::{append_message, memory::*, pop_message, restore_message};
use crate::core::tools::*;

use super::context::*;
use super::stream::{process_stream, StreamConfig};
use super::tools_runner::execute_tool_calls;

/// Pipeline 各阶段共享的状态（相当于一次用户请求的“上下文对象”）
///
/// 字段分三批填充：
/// - setup()：app / sid / ctx / 取消令牌 / 配置 / API 客户端 / 系统提示词 / 意图等
/// - pre_loop()：dynamic_context_str / 用户消息展示 / initial_msg_index / should_think / run_id
/// - 主循环与收尾：loop_count / token 统计 / final_answer / 反思计数等
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
    /// 本轮能力清单（由工作模式推导，注入动态上下文 / 目录输出 / 执行期校验共用）
    capabilities: crate::core::tools::framework::capabilities::Capabilities,
    dynamic_context_str: String,
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
    /// Plan 看门狗：plan 模式下连续无喂狗动作的工具调用次数
    plan_consecutive_stalls: usize,
    /// Plan 看门狗：plan 模式下累计无 ProposePlan / 降级 edit 的 loop 次数
    plan_total_loops_without_plan: usize,
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

/// 标准化受众（audience）：非 user 一律视为 developer。
/// 受众决定 agent loop 默认是否开启深度思考（developer → 开，user → 关）。
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

/// 标准化工作模式（work_mode）：plan（规划）/ edit（编辑，默认）。
/// 工作模式决定系统提示词、写操作工具是否可用、以及是否走方案审批流程。
/// 说明：第二步起"chat（只读保护）"已取消，安全由权限档位（请求审批/帮我批准）承担。
fn normalize_agent_work_mode(mode: &str) -> &'static str {
    match mode {
        "plan" => "plan",
        _ => "edit",
    }
}

impl PipelineState {
    /// 阶段 1：初始化 — 会话与配置准备 + 意图分类
    ///
    /// 相当于“启动前检查 + 路由决策”，产出可执行的 PipelineState。核心逻辑：
    /// 1. 校验：会话是否正忙（已有任务在跑则拒绝）、是否配置了 API Key
    /// 2. 创建本次执行的取消令牌（用户随时可停止），并记录请求工作区
    /// 3. 读取双轴配置：受众（developer/user）× 工作模式（chat/plan/edit），据此生成系统提示词
    /// 4. 意图分类：非 chat 模式走规则快速判定（复杂任务 → TASK_PLAN 强制切 Plan 模式）；
    ///    chat 模式走三层分类（规则 → 上下文 → LLM 兜底）
    /// 5. 输入框自然语言审批：短消息 + 上轮刚提交方案 → 直接更新方案状态（同意/驳回）
    /// 6. TASK_PLAN 首轮强制切换到 plan 模式并向前端广播 agent-work-mode-changed
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
        // 步骤 1：会话占用检查 —— 同一会话已有任务在执行时拒绝新请求
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
        // 步骤 2：创建本次执行的取消令牌并挂载到会话（用户随时可停止）
        let cancel_token = tokio_util::sync::CancellationToken::new();
        *ctx.cancel_token.lock().await = Some(cancel_token.clone());

        // 步骤 3：记录本次请求的工作区（文件操作 / 沙箱边界根目录）
        let request_workspace = ctx.workspace.lock().await.clone();
        println!(
            "[DEBUG] Current Workspace for session {}: {:?}",
            sid, request_workspace
        );

        // 步骤 4：读取用户配置，校验 API Key 是否已配置
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

        // 步骤 5：创建 HTTP 客户端（后续所有 LLM 调用共用）
        let client = reqwest::Client::new();

        // 步骤 6：读取偏好（受众 × 工作模式 × 权限档位），并写入会话上下文
        // 权限档位：新会话从偏好继承；已有会话保持自己的设置
        let approval_mode_from_prefs: String;
        let (audience, work_mode) = {
            let prefs = crate::command::app_config::get_ui_preferences()
                .await
                .unwrap_or_default();
            let audience = normalize_agent_audience(&prefs.agent_audience).to_string();
            let work_mode = normalize_agent_work_mode(&prefs.agent_work_mode).to_string();
            approval_mode_from_prefs = if prefs.agent_approval_mode == "auto_approve" {
                "auto_approve".to_string()
            } else {
                "request_approval".to_string()
            };
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
        // 权限档位：会话里没有历史（新会话）时按偏好初始化，否则沿用会话自己的
        {
            let has_history = !ctx.memory.lock().await.messages.is_empty();
            if !has_history {
                *ctx.approval_mode.lock().await = approval_mode_from_prefs.clone();
            }
        }
        let system_prompt = crate::core::agent::prompts::get_system_prompt(
            &audience,
            &current_work_mode,
            request_workspace.as_deref(),
        );

        // 步骤 6.5：能力清单
        //
        // 只用于注入"能力边界声明"（动态上下文 + 工具目录结论），让模型第一轮就知道
        // 哪些能力在本模式不存在，不必用发现类工具去试探。
        // 受限模式（规划）下用户要求改文件时，不再提前截断——交给模型自己解释并走方案审批流程，
        // 这样用户少一次往返（不必先回一句"先给方案"）。
        let capabilities = crate::core::tools::framework::capabilities::Capabilities::for_work_mode(
            &current_work_mode,
        );

        // 步骤 7：判断是否携带图片 —— 意图分类时提示 LLM 结合截图理解
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
        // "用户已同意方案/要求修改方案"是审批续跑，不算复杂任务
        let is_approval_continuation = msg_for_intent.starts_with("用户已同意方案")
            || msg_for_intent.starts_with("用户要求修改方案");
        // 复杂任务判定：正则直判（命中 → 走方案审批）；"用户已同意方案/要求修改"属于审批续跑
        let detected_intent = {
            let is_complex = crate::core::complex_task::is_complex_task(&msg_for_intent);
            if is_complex && !is_approval_continuation {
                println!("[JARVIS] {} 模式：规则检测到复杂任务，首轮直接进入方案审批流程", work_mode);
                "TASK_PLAN".to_string()
            } else {
                println!("[JARVIS] {} 模式：直接进入项目操作流程", work_mode);
                DIRECT_DEVELOPER_INTENT.to_string()
            }
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
                let mut approved_any = false;
                {
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
                        approved_any = new_status == "approved";

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
                } // 释放 memory 锁，避免锁顺序死锁

                // B2：方案批准后由服务端强制切回 edit；模型侧的 SwitchWorkMode(edit) 仅作双保险
                if approved_any {
                    let mut mode = ctx.agent_work_mode.lock().await;
                    if mode.as_str() != "edit" {
                        let old_mode = mode.clone();
                        *mode = "edit".to_string();
                        let _ = app.emit(
                            "agent-work-mode-changed",
                            json!({
                                "sessionId": session_id,
                                "from": old_mode,
                                "to": "edit",
                                "reason": "方案已批准，自动切回编辑模式",
                            }),
                        );
                    }
                }
            }
        }

        // 步骤 8：确定反思审查模式（取配置默认值，可被本次请求覆盖）
        let resolved_reflection_mode = reflection_mode_override
            .filter(|m| !m.is_empty())
            .unwrap_or_else(|| cfg.reflection_mode.clone());

        // 步骤 9：组装 PipelineState（意图、提示词、取消令牌等已就绪）
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
            capabilities,
            // 以下字段在后续阶段填充
            dynamic_context_str: String::new(),
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
            plan_consecutive_stalls: 0,
            plan_total_loops_without_plan: 0,
            display_msg,
            tool_execution_summary: None,
        };

        // 步骤 10：TASK_PLAN 前置拦截 —— 复杂任务首轮强制切到 plan 模式并广播
        if detected_intent == "TASK_PLAN" && current_work_mode != "plan" {
            println!("[JARVIS] 意图前置拦截：TASK_PLAN 意图，首轮强制切换到 Plan 模式");
            *state.ctx.agent_work_mode.lock().await = "plan".to_string();
            // system 必须全程字节恒定：这里只切换 work_mode，不重建 system。
            state.detected_intent = "TASK_PLAN".to_string();
            let _ = state.app.emit(
                "agent-work-mode-changed",
                json!({
                    "sessionId": state.sid,
                    "from": current_work_mode,
                    "to": "plan",
                    "reason": "意图分类检测到复杂任务，自动切换到计划模式",
                }),
            );
        }

        Ok(state)
    }

    /// 阶段 3：上下文构建 + 消息注入 + Agent Run 启动
    ///
    /// 主循环开始前的一次性准备：
    /// 1. 构建动态上下文（意图相关提示、工作区信息等）
    /// 2. 崩溃恢复：若上次 run 异常中断（含 InProgress 残留任务），注入“恢复指令”给 LLM
    /// 3. 把用户消息（含图片）注入会话历史，记录其在消息列表中的位置 initial_msg_index
    /// 4. 决定首轮是否深度思考（用户临时开关优先，否则按受众默认值）
    /// 5. 在 agent_runs 表登记本次 run，并保存第一个检查点
    async fn pre_loop(&mut self) {
        // 步骤 1：构建动态上下文（意图相关提示 + 工作区信息）
        // 快照 seq 使用 SessionMemory 的持久化单调计数器，压缩/重启后仍严格递增，
        // 保证“以 seq 最大（最新）的快照为准”不会因 messages.len() 回退而失效。
        let current_mode = { self.ctx.agent_work_mode.lock().await.clone() };
        let snapshot_seq = {
            let mut session = self.ctx.memory.lock().await;
            session.snapshot_seq = session.snapshot_seq.saturating_add(1);
            session.snapshot_seq
        };
        // 能力清单位于快照内，必须与当前 work_mode 保持一致（审批通过/首轮强制切 plan 后亦然）
        self.capabilities = crate::core::tools::framework::capabilities::Capabilities::for_work_mode(
            &current_mode,
        );
        self.dynamic_context_str = build_dynamic_context(
            &self.detected_intent,
            &self.request_workspace,
            &self.capabilities,
            &current_mode,
            snapshot_seq,
        );

        // 步骤 2：准备用户消息的短版预览（给 UI 展示）
        self.user_msg_preview = if self.msg.chars().count() > 50 {
            self.msg.chars().take(50).collect::<String>()
        } else {
            self.msg.clone()
        };

        // 步骤 3：恢复 + 注入 —— 处理崩溃残留并把本次用户消息写入历史
        let memory_after_user_message = {
            let mut session = self.ctx.memory.lock().await;
            // 3a. 崩溃恢复：上次 run 异常中断时，把残留消息恢复进内存
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
            // 3b. 把用户消息（含图片）注入历史，记录起始位置 initial_msg_index
            let mut active_sid = Some(self.sid.clone());
            self.initial_msg_index = inject_user_message(
                &mut session,
                self.display_msg.as_deref().unwrap_or(&self.msg),
                &self.image_base64_list,
                &self.dynamic_context_str,
                &mut active_sid,
            );
            session.clone()
        };
        // 步骤 4：注入后立即落库（防止崩溃丢消息）
        crate::core::session::save_session(&self.sid, &memory_after_user_message, None);
        let _ = self.app.emit("session-updated", ());

        // 深度思考决策：
        // - 首轮：用户 thinking_override 优先（一次性），否则使用 audience 默认值
        // - 后续轮：始终使用 audience 默认值（developer → true, user → false）
        // 步骤 5：决定首轮是否深度思考（用户临时开关优先，否则按受众默认）
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
        // 步骤 6：在 agent_runs 表登记本次 run（实时进度 + 崩溃恢复用）
        self.run_id = agent_runs::start_run(&self.app, &self.sid, &self.msg, None, user_message_id);
        // 步骤 7：标记当前 run 为活跃，并保存第一个检查点
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

    /// 处理调度器事件（异步调度模式下，主循环与 LLM 请求做 select 时消费）。
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

    /// 阶段 4：主循环 — Agent Loop 心脏（调 LLM → 流式解析 → 工具执行 → 循环）
    ///
    /// 每轮循环的执行顺序：
    /// 1. 取消检查 / 循环次数确认（满 30 轮弹窗询问）/ 后台通知注入 / 上下文压缩检查
    /// 2. 准备历史快照（过滤内部消息、修复残缺工具配对、恢复图片、注入动态上下文）
    /// 3. build_llm_request 按模型格式构建请求（OpenAI 出口时翻译协议）
    /// 4. 调 API：有活跃调度器时与调度器事件做 select（异步并行），否则直接等待（120s 超时 + 重试）
    /// 5. process_stream 流式解析：边收边推前端，累积工具参数分片
    /// 6. execute_tool_calls 并行执行工具，结果以 ToolResult 写回历史
    /// 7. 判断是否继续：有工具结果 → 下一轮；无工具结果 = 最终答案，结束循环
    ///
    /// 循环终止条件：无工具结果 / 工具请求 break_loop（如 ProposePlan 等待审批）/
    /// 用户取消 / 达到 200 轮绝对上限 / API 连续失败
    async fn run_main_loop(&mut self) -> Result<(), AgentError> {
        loop {
            println!("[JARVIS] 主循环开始: loop_count={}, total_loop_count={}", self.loop_count, self.total_loop_count);

            // ════ 主循环每轮开始 ════
            // 步骤 1：取消检查 —— 用户点停止则退出循环
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
                    // 用户拒绝续跑 / 确认未完成：给一个明确的收尾说明，避免留下空气泡
                    if self.final_answer.trim().is_empty() {
                        self.final_answer =
                            "已停止执行。需要继续时告诉我，我会接着上次的进度往下做。".to_string();
                    }
                    break;
                }
            }

            // 步骤 2：后台通知注入 —— 把后台任务完成结果推给 LLM 决策
            // 后台通知注入
            self.drain_background_notifications().await;

            // 步骤 3：上下文压缩检查（超上限 70% 时自动摘要旧历史）
            // Token 压缩
            self.compact_if_needed().await;

            // 后续轮次：重置 should_think 为 audience 默认值，确保 agent loop 思考状态一致
            if self.loop_count > 0 {
                self.should_think = self.loop_think_default;
            }

            // 步骤 4：准备发给 LLM 的历史快照（过滤内部消息、修复残缺配对等）
            // 历史快照准备
            let history_snapshot = self.prepare_history_snapshot().await;

            // 步骤 5：构建 LLM 请求（OpenAI 出口时按模型翻译协议）+ 更新上下文快照
            // 构建请求并更新上下文快照
            let (req_json, is_openai) = self.build_llm_request(history_snapshot);

            // 调试日志
            let request_json = serde_json::to_string_pretty(&req_json).unwrap_or_default();
            println!("[MAIN AGENT] loop {} request ({} bytes)", self.total_loop_count + 1, request_json.len());
            debug_logger::debug_logger().log_request(&self.sid, "MAIN", self.total_loop_count + 1, &request_json);

            if self.cancel_token.is_cancelled() {
                continue;
            }

            // 步骤 6：调用 LLM API —— 有活跃调度器时与其事件并行 select 等待
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

            // 步骤 7：SSE 流式解析 —— 边收边推前端、累积工具参数分片
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

            // 步骤 8：并行执行工具调用（tools_runner 三阶段流水线）
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

            // 步骤 9：把本轮助手响应（文本/思考/工具调用）写入会话历史
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

            // 步骤 10：判断是否继续 —— 无工具结果即为最终答案，结束循环
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
                    && crate::core::complex_task::detect_plan_in_text(&self.final_answer)
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
                // LLM 请求了 CompactConversation：工具结果写回后立即执行压缩
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
                drop(session);

                // B3 Plan 看门狗：仅 plan 模式；连续无喂狗的工具调用 / 累计无 ProposePlan 的空转达到阈值时，
                // 先做一次缓存友好的 LLM 进度小结，再强制停下交还决策权。
                let current_mode_after = self.ctx.agent_work_mode.lock().await.clone();
                if self.update_plan_watchdog(&current_mode_after, &tool_names_for_reflection) {
                    println!(
                        "[JARVIS] Plan 看门狗触发：consecutive={}, loops_without_plan={}",
                        self.plan_consecutive_stalls, self.plan_total_loops_without_plan
                    );
                    self.handle_plan_watchdog_summary().await;
                    break;
                }
            }
            self.loop_count += 1;
            self.total_loop_count += 1;
            // 步骤 11：进入下一轮循环（累计轮数，超 200 轮绝对上限强制停止）
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

    /// 阶段 5：收尾 — 持久化 + 快照 + 记忆 + 结果组装
    ///
    /// 1. 崩溃兜底：把内存中未落库的编辑补丁先写入 agent_run_patches 表
    /// 2. 文件快照：本轮有文件改动才创建 Git 检查点（纯聊天轮次跳过），供 UI 回滚
    /// 3. 保存会话到 SQLite（含累计 token），必要时异步调 LLM 自动起名
    /// 4. 记忆超预算时后台整理一次（不阻塞返回；新增事实由主 Agent 的 UpdateMemory 负责）
    /// 5. 按取消/循环超时/正常三种情况组装 JarvisResult（FINISH / CANCELLED / PAUSED_LOOP_LIMIT）
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

        // 3. 保存会话到 SQLite（含累计 token）；纯聊天且被取消时不落空会话
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

        // 自动命名：消息数足够且未命名过时，后台调 LLM 生成标题
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

        // 4. 记忆维护：只有全局记忆超过阈值时才后台整理一次
        // 原先是每轮结束都跑一次 LLM 重写，既贵，又会让记忆越滚越乱。
        // 新增事实由主 Agent 的 UpdateMemory 负责，这里只做合并压缩。
        if global_memory_char_count()
            > crate::core::tools::agent_tools::MEMORY_CONSOLIDATE_THRESHOLD_CHARS
        {
            let cfg_clone = self.cfg.clone();
            let sid_for_memory = self.sid.clone();
            let app_for_memory = self.app.clone();
            tokio::spawn(async move {
                run_memory_curator(app_for_memory, cfg_clone, sid_for_memory).await;
            });
        }

        // 汇总状态：正常结束 / 用户取消 / 循环超时暂停
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
    /// 异常收尾：主循环报错时调用，把错误记录到 agent_runs 表并清理运行状态
    /// （标记 run 失败、清空 active_run_id、释放取消令牌，保证下次可重新执行）
    async fn abort_after_error(&self, error: &AgentError) {
        // 仅当已有 run 记录时，才执行失败登记与状态清理
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

    /// 用户取消处理：保留用户消息、恢复已流式输出的部分内容作为答案，
    /// 截断会话历史到本次用户消息之后，并标记 run 为 CANCELLED
    async fn handle_cancellation(&mut self) {
        println!(
            "[JARVIS] 用户已取消执行，保留用户消息并恢复部分输出，user index {}",
            self.initial_msg_index
        );

        // 直接查 agent_runs 表的 live_content（不受 running 状态保护逻辑影响）
        // 1. 从 agent_runs 表取回已流式输出的部分结果（live_content / thinking）
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
            // 2. 截断历史到本次用户消息之后，把部分结果作为最终答案写回
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
        // 3. 标记 run 为 CANCELLED，并向前端发送取消通知
        agent_runs::cancel_run(
            &self.app,
            &self.run_id,
            self.req_input_tokens,
            self.req_output_tokens,
            Some(self.final_answer.clone()),
        );
    }

    /// 循环上限确认（满 30 轮触发）：弹窗请用户授权继续
    /// 返回 true 表示继续（重置 loop_count），false 表示终止；
    /// 超时或拒绝时置 loop_continuation_pending，用户稍后可通过 resume_pipeline 续跑
    async fn request_loop_continuation(&mut self) -> bool {
        let _ = self.app.emit(
            "chat-stream",
            json!({
                "content": format!("\n> **代理执行已达到 {} 回合，正在等待用户确认是否继续...**\n", crate::infra::types::constants::MAX_AGENT_LOOP_BEFORE_CONFIRM),
                "sessionId": self.sid,
                "loopCount": self.total_loop_count + 1
            }),
        );
        // 弹权限确认：允许 → 继续并重置本轮计数；超时/拒绝 → 置可续跑标记
        let decision = request_permission(
            &self.app,
            &self.sid,
            &format!(
                "代理执行已达到 {} 回合，可能任务较为复杂或陷入循环。是否继续执行？",
                crate::infra::types::constants::MAX_AGENT_LOOP_BEFORE_CONFIRM
            ),
            PermissionKind::LoopContinuation,
        )
        .await;
        if decision.is_allowed() {
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
            // 只有"没拿到结论"（取消、通道关闭）才保留续跑入口；
            // 用户明确点了拒绝，就不该再给一个"稍后点允许继续"的后门
            if decision.is_rejected() {
                *self.ctx.loop_continuation_pending.lock().await = false;
                println!("[JARVIS] 用户拒绝继续执行，循环终止");
            } else {
                *self.ctx.loop_continuation_pending.lock().await = true;
                println!(
                    "[JARVIS] 循环续跑确认未完成（{}），保留续跑入口",
                    decision.status_label()
                );
            }
            false
        }
    }

    /// 注入后台任务完成通知到会话中
    async fn drain_background_notifications(&self) {
        // 取出后台任务完成的待消费通知（无通知则无事可做）
        let notifs =
            crate::infra::background::BackgroundManager::drain_notifications(&self.app).await;
        if !notifs.is_empty() {
            let mut notif_text = String::new();
            for n in notifs {
                notif_text.push_str(&format!("[bg:{}] {}: {}\n", n.task_id, n.status, n.result));
            }
            let mut session = self.ctx.memory.lock().await;
            // 以 User/Assistant 对写入会话（source=background，发给 LLM 前会被过滤）
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

    /// 上下文压缩检查（项目内唯一的 LLM 摘要压缩，属单级）：本地估算 token，
    /// 超过上限（100k）的 70% 时调 LLM 把旧历史压缩成一段摘要；
    /// 压缩前后保证用户最新消息仍在历史末尾，且后续轮次能正确计算 initial_msg_index
    async fn compact_if_needed(&mut self) {
        // 1. 估算当前上下文 token（消息 + 工具 schema）
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
            // 2. 压缩前先临时取出最后一条用户消息，压缩完成后再放回
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

            // 3. 把最新用户消息恢复到历史末尾，保证上下文连贯
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

    /// 获取当前会话可用的工具定义（固定核心工具集，参数不变以命中 prompt cache）
    fn current_tools(&self) -> Vec<serde_json::Value> {
        get_tools_definition()
    }

    /// 从完整消息列表生成“发给 LLM 的历史快照”（只读处理，不改原历史）：
    /// 1. 过滤 internal/background 内部消息
    /// 2. 折叠往轮图片、恢复本轮图片的 base64（动态上下文已随消息落库，不再另行注入）
    /// 3. 修复残缺的 Assistant(tool_calls) → ToolResult 配对（防 API 400）
    ///
    /// 这里不改写工具结果内容：任何对「已经发出去过的前缀」的改写都会让 provider 的
    /// prompt cache 整体失效，比省下的 token 贵得多。
    fn prepare_history_snapshot_from_messages(
        &self,
        messages: Vec<Message>,
        sources: &[String],
    ) -> Vec<Message> {
        // 步骤 1：过滤 internal/background 内部消息（LLM 不需要看到系统内部通知），
        // 并把「本轮用户消息」的下标换算到过滤后的快照坐标系——initial_msg_index
        // 是过滤前的下标，而图片折叠要的是过滤后的下标。
        let session_turn_start = self.initial_msg_index;
        let mut filtered: Vec<Message> = Vec::with_capacity(messages.len());
        let mut snapshot_turn_start: Option<usize> = None;
        for (idx, (msg, src)) in messages.into_iter().zip(sources.iter()).enumerate() {
            if !matches!(src.as_str(), "chat" | "compact" | "context") {
                continue;
            }
            if idx == session_turn_start {
                snapshot_turn_start = Some(filtered.len());
            }
            filtered.push(msg);
        }
        // 兜底：没命中就把所有图片当往轮处理（不会 panic，也不会把 base64 全量发出去）
        let current_turn_start = snapshot_turn_start.unwrap_or(filtered.len());

        let mut history_snapshot = filtered;

        // 防御性检查：确保每个 Assistant(tool_calls) 后跟 ToolResult
        // 修复流式中断、并发修改等边缘 case 导致的消息序列断裂
        fix_broken_tool_call_pairs(&mut history_snapshot);

        // 步骤 2：折叠往轮图片、恢复本轮图片。动态上下文已随消息落库，这里不再注入
        restore_image_data(&mut history_snapshot, current_turn_start);

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

    /// 估算一次请求的上下文用量（消息 + 工具 schema，按分区统计），
    /// 结果用于前端上下文监控展示与压缩触发判断；
    /// 每个 section 记录独立字符数/token 数，方便 UI 定位占比
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
                        ContentBlock::Context { .. } => {}
                    }
                }
            }
            (tool_calls, tool_results, images, thinking)
        }

        let cleaned_messages = crate::infra::llm::adapters::strip_context_blocks(history_snapshot);
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
                format!("当前请求中包含 {} 个图片块。本轮的图片会恢复为 base64，往轮图片折叠为文本摘要。", image_count),
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

    /// 构建 LLM API 请求体（内部统一按 Anthropic 结构建模）
    ///
    /// - 总是流式请求（stream: true），写入系统提示词、工具 schema、思考配置、温度等
    /// - OpenAI 格式模型：经 adapters 翻译消息/工具，并按模型注册表注入各家“思考参数”
    /// - 返回值第二项 is_openai 告诉 stream.rs 按哪种协议解析 SSE 事件
    fn build_llm_request(
        &self,
        history_snapshot: Vec<Message>,
    ) -> (serde_json::Value, bool) {
        // 1. 取系统提示词与工具定义，并更新上下文监控快照
        // system 在 setup 阶段只组装一次并保持字节恒定，整个会话内不再随 work_mode 变化。
        let system_prompt = self.system_prompt.clone();
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

        // 出网前把内部 Context 块降级为普通 Text（协议不认 "context" 类型）
        crate::infra::llm::adapters::materialize_context_blocks_for_wire(
            &mut request_body.messages,
        );

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
            // 2. OpenAI 出口：翻译消息/工具，并按模型注册表注入该模型的思考参数
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
            // Anthropic 出口：丢掉无 signature 的 thinking 块，避免回传被判 400
            let messages = crate::infra::llm::adapters::strip_unsigned_thinking_for_anthropic(
                &request_body.messages,
            );
            request_body.messages = messages;
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

    /// 存储助手回复到会话历史（过滤空文本/空思考块，工具块原样保留）
    async fn store_assistant_response(&self, current_blocks: &[ContentBlock]) {
        let mut session = self.ctx.memory.lock().await;
        // 过滤空文本/空思考块，仅保留有效内容
        let filtered_blocks: Vec<ContentBlock> = current_blocks
            .iter()
            .filter(|block| match block {
                ContentBlock::Text { text } => !text.trim().is_empty(),
                ContentBlock::Thinking { thinking, .. } => !thinking.trim().is_empty(),
                ContentBlock::ToolUse { .. }
                | ContentBlock::ToolResult { .. }
                | ContentBlock::Image { .. }
                | ContentBlock::Context { .. } => true,
            })
            .cloned()
            .collect();
        // 写入会话历史（chat 来源，下一轮请求会发给 LLM）
        if !filtered_blocks.is_empty() {
            append_message(&mut session, Message::Assistant {
                content: Content::Multiple(filtered_blocks),
            }, "chat");
        }
    }

    /// B3 Plan 看门狗：更新计数并判断是否触发（仅 plan 模式）。
    ///
    /// 喂狗动作（重置两个计数器）：
    /// - 本 loop 调用了 ProposePlan；
    /// - 本 loop 已把 work_mode 切出 plan（SwitchWorkMode 到 edit）。
    fn update_plan_watchdog(&mut self, work_mode: &str, tool_names: &[String]) -> bool {
        use crate::infra::types::constants::{PLAN_WATCHDOG_MAX_CONSECUTIVE_STALLS, PLAN_WATCHDOG_MAX_LOOPS_WITHOUT_PLAN};

        if work_mode != "plan" {
            self.plan_consecutive_stalls = 0;
            self.plan_total_loops_without_plan = 0;
            return false;
        }

        if tool_names.iter().any(|n| n == "ProposePlan") {
            self.plan_consecutive_stalls = 0;
            self.plan_total_loops_without_plan = 0;
            return false;
        }

        self.plan_consecutive_stalls = self.plan_consecutive_stalls.saturating_add(tool_names.len());
        self.plan_total_loops_without_plan = self.plan_total_loops_without_plan.saturating_add(1);

        self.plan_consecutive_stalls >= PLAN_WATCHDOG_MAX_CONSECUTIVE_STALLS
            || self.plan_total_loops_without_plan >= PLAN_WATCHDOG_MAX_LOOPS_WITHOUT_PLAN
    }

    /// B3 触发后：先复用当前 system + 历史做一次缓存友好的 LLM 进度小结，
    /// 再把决策权交还用户并强制结束当前 loop。
    async fn handle_plan_watchdog_summary(&mut self) {
        let instruction = "【系统通知】规划探索已达到看门狗阈值。请立即停止探索，不要调用任何工具，只用一段话输出当前进度小结与下一步建议（继续探索 / 缩小范围 / 直接执行）。";
        let mut snapshot = self.prepare_history_snapshot().await;
        snapshot.push(Message::User {
            content: Content::Single(instruction.to_string()),
        });

        let (req_json, is_openai) = self.build_llm_request(snapshot);
        let mut summary = String::new();
        match self.call_api_with_retry(&req_json).await {
            Ok(Some(resp)) => {
                let mut stream = resp.bytes_stream().eventsource();
                let parsed = process_stream(
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
                summary = parsed.text.trim().to_string();
            }
            Ok(None) => {}
            Err(e) => {
                println!("[JARVIS] Plan 看门狗小结调用失败: {}", e);
            }
        }

        if summary.is_empty() {
            summary = "本次规划探索尚未收敛，已自动停止。请选择：继续探索 / 缩小范围 / 直接执行。".to_string();
        }

        self.final_answer = summary.clone();
        self.tool_execution_summary = Some(summary);
        let _ = self.app.emit(
            "chat-stream",
            json!({
                "content": "\n> [!] **规划探索未收敛，已自动停下并把决策权交还给你。** 可选择：继续探索 / 缩小范围 / 直接执行。\n",
                "sessionId": self.sid,
                "loopCount": self.total_loop_count + 1
            }),
        );
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

/// 续跑入口：用户在上轮“循环超时暂停”后授权继续，或恢复被中断的 run 时调用。
/// 与 run_pipeline 的差别：inject_user_message=false（不重复注入用户消息），
/// 以“继续原因”作为本轮输入。
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

/// 流水线总调度（run_pipeline / resume_pipeline 共用）
/// 
/// 依次执行：阶段 1 setup → 阶段 2 validate（可提前返回）→ 阶段 3 pre_loop
/// → 阶段 4 run_main_loop（出错走 abort_after_error）→ 阶段 5 finalize
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
    // ── 阶段 1：初始化（会话与配置准备 + 意图分类）──
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

    // ── 阶段 3：循环前准备（崩溃恢复 + 注入用户消息 + 启动 run）──
    state.pre_loop().await;

    // ── 阶段 4：主循环（调 LLM → 执行工具 → 直到得出最终答案）──
    if let Err(err) = state.run_main_loop().await {
        state.abort_after_error(&err).await;
        return Err(err);
    }

    // ── 阶段 5：收尾（持久化 / 快照 / 记忆 / 结果组装）──
    Ok(state.finalize().await)
}
