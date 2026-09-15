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
    /// 会话级思考档位（L2，随会话走）。`Auto` 时按 `loop_think_default` 裁决。
    session_think_mode: crate::core::session::thinking::ThinkingMode,
    /// **L1 预设默认**（已与全局 audience 回退合并为确定布尔值）。
    ///
    /// 语义已**收窄**（设计文档决策 D2）：它不再是"每轮的思考值"，而是
    /// `profiles[].thinkingDefault` 解析结果——预设为 `auto` 时回落
    /// `agent_audience == "developer"`（`pipeline.rs` 步骤 6 计算）。
    /// 仅在会话也未表态（L2 = `auto`）时生效。
    loop_think_default: bool,
    /// 本轮（一次用户指令 = 可能多个 loop）最终采用的思考状态。
    ///
    /// 必须整轮恒定：Anthropic 协议的 thinking 模式要求历史里 assistant 的 thinking
    /// 块原样回传，若首轮 disabled、第二轮又变 enabled，历史里那条 assistant 根本没有
    /// thinking 块，服务商会直接 400
    /// （`content[].thinking in the thinking mode must be passed back to the API`）。
    turn_think: bool,
    /// 本轮思考裁决结果（含 reason 与 notice key），随 `JarvisResult` 下发前端。
    turn_thinking_decision: crate::core::session::thinking::ThinkingDecision,
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
    /// 本轮因上游失联（流内空闲超时）而中断时的原因说明。
    /// Some 表示属于"运行被打断"，收尾时必须保留现场、不得截断历史。
    interrupted_reason: Option<String>,
    /// 面向用户的状态标注（气泡下方小字），由中断收尾路径填充。
    notice: Option<String>,
}

/// 判断空闲超时后是否必须结束本轮并按中断收尾。
///
/// **实测 bug 的防护点**：首版这里用的是 `should_retry`（"零产出"），
/// 于是 `interrupted` 形态（吐了半截后静默）虽然计时器触发了，却走到了正常
/// 完成路径 —— 界面只剩半截正文，用户以为已经正常完成。
///
/// 正确语义：只要计时器触发（`idle_timed_out`）就必须收尾，**与产出多少无关**；
/// `should_retry` 只决定要不要再试一次，不决定是否收尾。
fn must_finish_as_interrupted(idle_timed_out: bool, _should_retry: bool) -> bool {
    idle_timed_out
}

/// 当前时间（毫秒），用于"等待提示"看门狗判断静默时长。
fn now_millis() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

/// 判断此刻是否应当发出"仍在等待"提示。
///
/// 回归防护：首版每 30 秒无条件发一条，页面被「已 30 秒未收到数据」刷屏
/// （用户实测反馈）。现在约定：**同一段静默期只提示一次**，
/// 直到收到新数据（`touch()` 复位 `already_sent`）才允许再次提示。
fn should_emit_waiting_hint(silent_secs: u64, already_sent: bool) -> bool {
    silent_secs >= crate::infra::types::constants::API_WAITING_HINT_SECS && !already_sent
}

/// 等待上游响应期间的"进度提示"看门狗句柄。
///
/// 上游深度思考、网络迟滞、或**收到响应头后正文长时间静默**时，界面上
/// 可能长时间没有任何事件，用户只看到转圈，无法区分"模型在思考"与
/// "服务已经死了"。本看门狗让等待**可见**。
///
/// ## 为什么必须跨越两个阶段
///
/// 首个版本只覆盖"等待响应头"阶段，一旦 `call_api_with_retry` 返回就 `abort()`。
/// 但真实故障几乎都发生在**响应头已到、正文静默**阶段（响应头通常很快返回），
/// 于是看门狗永远没机会触发。因此改为：从请求发出前启动，一直存活到本轮流
/// 读取结束，由 `touch()` 在每个 SSE 帧到达时续期。
///
/// ## 为什么按"静默期"而非"累计耗时"判定
///
/// 若按累计耗时，一个流畅生成几分钟的正常请求会不断被提示打扰。
/// 按静默期则只在**真的没有数据**时提示，语义与流内空闲超时一致。
struct WaitingHint {
    /// 最近一次收到数据的时间戳（毫秒）
    last_tick_ms: Arc<std::sync::atomic::AtomicU64>,
    /// 当前这段静默期是否已经提示过。
    ///
    /// 回归防护：首版每 30s 就发一条，页面被"已 30 秒未收到数据"刷屏。
    /// 现在同一段静默期**只提示一次**，直到收到新数据（`touch()`）才允许再提示。
    /// 这样既避免了刷屏，也保证长时间静默不会被彻底静默处理。
    hint_sent: Arc<std::sync::atomic::AtomicBool>,
    /// 是否已结束；置位后看门狗不再提示
    done: Arc<std::sync::atomic::AtomicBool>,
    handle: tokio::task::JoinHandle<()>,
}

impl WaitingHint {
    /// 收到一帧数据（或请求刚发出）时调用，重置静默计时并允许再次提示
    fn touch(&self) {
        self.last_tick_ms
            .store(now_millis(), std::sync::atomic::Ordering::Relaxed);
        self.hint_sent
            .store(false, std::sync::atomic::Ordering::Relaxed);
    }

    /// 本轮结束，停止看门狗
    fn finish(&self) {
        self.done
            .store(true, std::sync::atomic::Ordering::Relaxed);
        self.handle.abort();
    }
}

/// 写入对话历史的"中断标记"（**会发给 LLM**，故措辞需统一）。
///
/// 按「模型能否接着做」二分，而不是按中断原因四分：
/// - **能接着做**（上游失联 / 执行报错 / 用户取消）→ `INTERRUPT_MARKER_RESUMABLE`
/// - **不能接着做**（已达轮次上限）→ `INTERRUPT_MARKER_STOPPED`
///
/// 为什么必须统一：该标记进入对话历史后，**之后每一轮请求都会原样重发**。
/// 多种措辞 = 多个 prompt 前缀变体，会让 provider 的 prompt cache 更易失效
/// （项目内既有约定：不改写已发出去过的前缀）。而原因并不改变模型的动作 ——
/// 它只能接着写；用户取消后的新意图由用户下一条消息表达。
///
/// 具体原因仍完整保留在**用户可见的 notice**（气泡下方小字）、
/// `interrupted_reason`（状态）与 `agent_runs.error`（审计）里，信息不丢失。
const INTERRUPT_MARKER_RESUMABLE: &str =
    "> ⚠️ **[回复被中断]** 上次回复在此处中断，请基于上下文继续完成。";

/// 已达轮次上限：与上面刻意不同 —— 此时模型**不该**接着写，
/// 否则会立刻再次触达上限。
const INTERRUPT_MARKER_STOPPED: &str =
    "> ⚠️ **[回复被中断]** 已达回合上限且未获续跑授权，本轮在此停下。";

struct ContextEstimate {    total_chars: usize,
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

/// 本轮（一次用户指令）最终采用的 thinking 状态：用户临时开关优先，否则用受众默认值。
///
/// 已被 `core::session::thinking::decide` 取代（后者还包含 L2 会话档位与能力夹紧）。
/// 保留仅为回归测试使用——它固化了"L1=auto 时回落 audience"的语义（设计文档决策 D2）。
#[cfg(test)]
fn resolve_turn_think(override_val: Option<bool>, loop_default: bool) -> bool {
    override_val.unwrap_or(loop_default)
}

/// 每个 loop 使用的 thinking 状态：整轮恒定。
///
/// 回归防护：旧实现在 `loop_count > 0` 时回落到 audience 默认值，导致
/// 「首轮 disabled → 第二轮 enabled」的翻转；此时历史里那条 assistant 是在关闭
/// thinking 时产生的、没有 thinking 块可回传，Anthropic 协议的服务商（含 DeepSeek
/// 的 anthropic 兼容端点）会直接 400：
/// `content[].thinking in the thinking mode must be passed back to the API`。
fn should_think_for_loop(turn_think: bool, _loop_count: usize) -> bool {
    turn_think
}

/// 判断给定的助手文本是否已存在于消息列表尾部。
///
/// 中断收尾时用于去重：流式内容可能已由 `store_assistant_response()` 或
/// `agent_runs.live_content` 落库，避免把同一段半截话写两遍。
/// 只检查尾部若干条，因为中断内容只可能出现在最近的位置。
fn assistant_text_exists_at_tail(messages: &[Message], target: &str) -> bool {
    let target = target.trim();
    if target.is_empty() {
        return false;
    }
    messages
        .iter()
        .rev()
        .take(4)
        .any(|msg| match msg {
            Message::Assistant { content } => match content {
                Content::Single(text) => text.trim() == target,
                Content::Multiple(blocks) => blocks.iter().any(|b| match b {
                    ContentBlock::Text { text } => text.trim() == target,
                    _ => false,
                }),
            },
            _ => false,
        })
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
                    content: "[系统注入：该工具已执行但结果因中断（取消/上游失联/报错）未能留档，\
                              已自动补齐消息序列以保证请求合法。如需确认实际效果请让用户复查，\
                              必要时可让用户回复「继续」重新推进。]"
                        .to_string(),
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
        // 统一构造：带连接超时与 TCP keepalive，避免半开连接永久挂起。
        // 不在此设置整体 timeout —— 会误杀长时间流式生成，分层时限见
        // api_client::build_client 的说明。
        let client = api_client::build_streaming_client();

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

        // 步骤 8.5：解析深度思考的两层来源（设计文档 §4）
        //
        // - L2 会话档位：随会话走，切会话即切档位（与 profileId 对称）
        // - L1 预设默认：`profiles[].thinkingDefault`，为 `auto` 时回落全局 audience
        //
        // 决策本身不在这里做，而是在 `start_run` 用 `thinking::decide` 统一裁决；
        // 这里只负责把两个来源取出来，避免在决策点再做 IO。
        let session_think_mode = crate::core::session::thinking::ThinkingMode::parse(
            ctx.thinking_mode
                .lock()
                .await
                .as_deref()
                .unwrap_or("auto"),
        );
        let profile_thinking_default =
            crate::core::session::thinking::ThinkingDefault::parse(&cfg.thinking_default);
        let loop_think_default = profile_thinking_default.resolve(audience == "developer");

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
            session_think_mode,
            loop_think_default,
            detected_intent: detected_intent.clone(),
            capabilities,
            // 以下字段在后续阶段填充
            dynamic_context_str: String::new(),
                        user_msg_preview: String::new(),
            initial_msg_index: 0,
            should_think: false,
            turn_think: false,
            turn_thinking_decision: crate::core::session::thinking::ThinkingDecision::default(),
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
            interrupted_reason: None,
            notice: None,
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

        // 深度思考决策（整轮一次性决定，之后不再改变）
        //
        // 优先序（设计文档 §4）：L3 单轮覆盖 ▸ 能力夹紧 ▸ L2 会话档位 ▸ L1 预设默认。
        // 注意"夹紧"：模型硬约束（不支持思考 / 强制思考）会无视会话与覆盖的相反意愿，
        // 但**不回写** sessions.thinking_mode（不变量 I3），用户表态原样保留。
        //
        // 本轮所有 loop 都用同一个值：中途翻转会让 Anthropic 协议的 thinking 链要求不成立
        //（首轮 disabled 就没有 thinking 块可回传，第二轮突然 enabled 会 400）。
        let override_val = self.thinking_override.take();
        let model_caps = crate::infra::llm::registry::query_capabilities(&self.model_id);
        let decision = crate::core::session::thinking::decide(
            override_val,
            self.session_think_mode,
            self.loop_think_default,
            model_caps.as_ref(),
        );
        self.turn_thinking_decision = decision.clone();
        let turn_think = decision.enabled;
        self.turn_think = turn_think;
        self.should_think = turn_think;
        // 供子 Agent 继承（设计文档 K3）：主 Agent 本轮用什么档位，本轮派生的子 Agent 就用什么
        *self.ctx.turn_think.lock().await = Some(turn_think);
        println!(
            "[JARVIS] 本轮 thinking 状态固定为 {}（override={:?}, 会话档位={:?}, 预设默认={}, 裁决={:?}）",
            if turn_think { "enabled" } else { "disabled" },
            override_val,
            self.session_think_mode,
            self.loop_think_default,
            decision.reason
        );

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
                    // 用户拒绝续跑 / 确认未完成：给一个明确的收尾说明，避免留下空气泡。
                    // 该说明必须落库——旧实现只写在内存 final_answer 里，
                    // 刷新界面后用户看不到任何"为什么停了"的交代。
                    let notice = "已停止执行。需要继续时告诉我，我会接着上次的进度往下做。";
                    if self.final_answer.trim().is_empty() {
                        self.final_answer = notice.to_string();
                    }
                    // 写入历史的标记面向 LLM（会进上下文），不含"告诉我"这类
                    // 对用户说的措辞——模型会把它们当成自己的话。
                    self.append_interrupted_marker(INTERRUPT_MARKER_STOPPED)
                        .await;
                    self.interrupted_reason = Some("达到回合上限且未获续跑授权".to_string());
                    break;
                }
            }

            // 步骤 2：后台通知注入 —— 把后台任务完成结果推给 LLM 决策
            // 后台通知注入
            self.drain_background_notifications().await;

            // 步骤 3：上下文压缩检查（超上限 70% 时自动摘要旧历史）
            // Token 压缩
            self.compact_if_needed().await;

            // 后续轮次：沿用本轮固定的 thinking 状态（不再回落 audience 默认值）。
            // 中途从 disabled 翻成 enabled 会让历史里那条没有 thinking 的 assistant
            // 无法满足 Anthropic 的"thinking 必须回传"要求 → 400。
            if self.loop_count > 0 {
                self.should_think = should_think_for_loop(self.turn_think, self.loop_count);
            }

            // 步骤 4：准备发给 LLM 的历史快照（过滤内部消息、修复残缺配对等）
            // 历史快照准备
            let history_snapshot = self.prepare_history_snapshot().await;

            // 步骤 5：构建 LLM 请求（OpenAI 出口时按模型翻译协议）+ 更新上下文快照
            // 构建请求并更新上下文快照
            let (req_json, is_openai) = self.build_llm_request(history_snapshot);

            // 调试日志：logger 内部按 request_base + messages 增量落盘，
            // 这里只打印一个体量（compact 序列化，不再为打印付出 pretty 的开销）
            println!(
                "[MAIN AGENT] loop {} request ({} bytes)",
                self.total_loop_count + 1,
                serde_json::to_string(&req_json).map(|s| s.len()).unwrap_or(0)
            );
            debug_logger::debug_logger().log_request(&self.sid, "MAIN", self.total_loop_count + 1, &req_json);

            if self.cancel_token.is_cancelled() {
                continue;
            }

            // 步骤 6：调用 LLM API —— 有活跃调度器时与其事件并行 select 等待
            // 调度器 channel 接收端（异步 select! 用）
            let sched_rx = self.ctx.scheduler_rx.lock().await.take();

            // 等待提示看门狗：从请求发出前一直存活到本轮流读取结束。
            // 由 SSE 帧到达续期，因此只在**真的没有数据**时才提示
            // （上游深度思考、网络迟滞、收到响应头后正文静默）。
            // 仅无调度器的主路径启用；有调度器时事件本身频繁，无需提示。
            let progress_watchdog = if sched_rx.is_none() {
                Some(self.spawn_waiting_hint())
            } else {
                None
            };
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
                let in_tokens = self.req_input_tokens;
                let out_tokens = self.req_output_tokens;
                let api_handle = tokio::spawn(async move {
                    let api_request = api_client::api_call_with_retry(
                        &client, &base_url, &req_json_clone, &api_key, api_format,
                        api_client::MAX_API_RETRIES, &app, &sid,
                    );
                    let timeout_result = tokio::time::timeout(
                        Duration::from_secs(
                            crate::infra::types::constants::API_RESPONSE_HEADER_TIMEOUT_SECS,
                        ),
                        api_request,
                    );
                    tokio::select! {
                        result = timeout_result => {
                            match result {
                                Ok(inner) => inner.map(|r| Some(r)),
                                Err(_) => {
                                    let error = ApiError::Network(format!(
                                        "API 请求超过 {} 秒未返回响应头，已自动终止。",
                                        crate::infra::types::constants::API_RESPONSE_HEADER_TIMEOUT_SECS
                                    ));
                                    let _ = agent_runs::fail_run(
                                        &app,
                                        &run_id_clone,
                                        error.to_string(),
                                        in_tokens,
                                        out_tokens,
                                    );
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
                // 无活跃调度器，正常阻塞等待 LLM。
                // 看门狗已在分支外启动，这里只负责发请求并把它传给流阶段。
                let api_outcome = self.call_api_with_retry(&req_json).await;
                // 不在此 abort：看门狗要跨到流读取阶段才有效。
                // 响应头很快返回，真正的静默几乎都发生在正文阶段。
                let resp = match api_outcome {
                    Ok(Some(r)) => {
                        if let Some(wd) = &progress_watchdog {
                            wd.touch();
                        }
                        r
                    }
                    // 提前退出前必须停掉看门狗（统一在块外 finish），否则会残留并事后补发提示
                    Ok(None) => {
                        *self.ctx.scheduler_rx.lock().await = None;
                        if let Some(wd) = &progress_watchdog {
                            wd.finish();
                        }
                        continue;
                    }
                    Err(e) => {
                        if let Some(wd) = &progress_watchdog {
                            wd.finish();
                        }
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

            // 流读取期间到达的帧会通过 on_frame 续期，真正空闲才提示。
            // 本轮结束（无论哪条路径）都必须 finish，否则会残留任务。
            let frame_tick = progress_watchdog.as_ref().map(|wd| {
                let tick = wd.last_tick_ms.clone();
                std::sync::Arc::new(move || {
                    tick.store(now_millis(), std::sync::atomic::Ordering::Relaxed);
                }) as std::sync::Arc<dyn Fn() + Send + Sync>
            });

            let Some(response) = response else {
                if let Some(wd) = &progress_watchdog {
                    wd.finish();
                }
                continue;
            };
            // 步骤 7：SSE 流式解析 —— 边收边推前端、累积工具参数分片
            // 流式处理（含一次断流重试）
            let stream_result = {
                // 注册表可选的缓存字段写法覆盖（一般不用；非标准命名时才在 model_registry.json 里写）
                let cache_usage_style =
                    crate::infra::llm::registry::cache_usage_style_for(&self.model_id);
                let mut stream = response.bytes_stream().eventsource();
                let mut result = process_stream(
                    &mut stream,
                    is_openai,
                    &self.app,
                    &self.sid,
                    &self.run_id,
                    self.total_loop_count + 1,
                    &self.cancel_token,
                    StreamConfig {
                        is_subagent: false,
                        cache_usage_style: cache_usage_style.clone(),
                        on_frame: frame_tick.clone(),
                    },
                )
                .await;

                // 重试条件：只由 stream 层给出的 `should_retry` 决定（零产出才为 true）。
                //
                // 为什么不在这里用 `idle_timed_out` 判断：该标记表示"空闲计时器触发了"，
                // 包含"吐了半截后静默"这种**不该重试**的情况。二者必须分开，
                // 否则会出现"既没重试、又没结束本轮"的卡死（见 StreamResult 注释）。
                if result.should_retry && !self.cancel_token.is_cancelled() {
                    if result.idle_timed_out {
                        println!("[JARVIS] 上游静默超时且零产出，尝试重试一次...");
                    } else {
                        println!("[JARVIS] 流式响应零产出即终止，尝试重试一次...");
                    }
                    // 重试给一个更短的、**覆盖全流程**的预算（请求头 + 读流）。
                    // 首次已等满一个空闲周期，若重试再等满，用户感知的等待会翻倍。
                    let retry_budget = Duration::from_secs(
                        crate::infra::types::constants::API_STREAM_RETRY_BUDGET_SECS,
                    );
                    let retry_fut = async {
                        if let Ok(Some(resp)) = self.call_api_with_retry(&req_json).await {
                            let mut stream2 = resp.bytes_stream().eventsource();
                            return Some(
                                process_stream(
                                    &mut stream2,
                                    is_openai,
                                    &self.app,
                                    &self.sid,
                                    &self.run_id,
                                    self.total_loop_count + 1,
                                    &self.cancel_token,
                                    StreamConfig {
                                        is_subagent: false,
                                        cache_usage_style,
                                        on_frame: frame_tick.clone(),
                                    },
                                )
                                .await,
                            );
                        }
                        None
                    };
                    match tokio::time::timeout(retry_budget, retry_fut).await {
                        Ok(Some(r)) => {
                            result = r;
                            if result.idle_timed_out {
                                println!("[JARVIS] 重试后仍为上游失联（零产出），不再重试");
                            }
                        }
                        Ok(None) => {}
                        Err(_) => {
                            println!(
                                "[JARVIS] 流式重试超过 {}s 预算（含读流），放弃重试",
                                crate::infra::types::constants::API_STREAM_RETRY_BUDGET_SECS
                            );
                            // 重试也没拿到结果 → 与空闲超时同样按中断收尾
                            result.idle_timed_out = true;
                        }
                    }
                }

                result
            };

            // 本轮流读取已结束：停掉等待提示看门狗（含重试流）。
            // 后面还有工具执行/下一轮循环，此处收尾最贴合"不再等上游数据"的语义。
            if let Some(wd) = &progress_watchdog {
                wd.finish();
            }

            // 上游失联 → 保留现场并结束本轮，把决策权交还用户
            // （服务恢复后一句"继续"即可接上）。
            //
            // 注意：这里的判据是 `idle_timed_out`（计时器确实触发了），
            // **不是** `should_retry`。二者语义不同：
            // - "吐了半截后静默"时 should_retry=false（不该重试），但仍必须收尾并给出中断提示，
            //   否则界面只剩半截正文，用户会误以为已经正常完成（实测反馈的 bug）。
            if must_finish_as_interrupted(
                stream_result.idle_timed_out,
                stream_result.should_retry,
            ) {
                self.handle_stream_idle_timeout(stream_result).await;
                break;
            }

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

            // ── 缓存命中：用"端点能力记忆"解释本次观测 ──
            // 有的厂商（实测 Kimi、小米 anthropic）在 0 命中时【不返回缓存字段】，
            // 因此单看一次响应无法区分"这家不报告"与"这次没命中（预热期）"。
            // 规则：该端点此前确认会上报 ⇒ 字段缺失按 0 命中解释；从未见过 ⇒ 保持未知。
            let cache_endpoint = crate::infra::llm::usage_memory::endpoint_key(
                &self.base_url,
                self.api_format.as_str(),
            );
            let cache_obs = crate::infra::llm::usage_memory::observe(
                &cache_endpoint,
                stream_result.cache_hit_tokens.is_some(),
                stream_result.cache_source,
            );
            if cache_obs.first_seen {
                println!(
                    "[JARVIS] 已记住端点 {} 会上报缓存字段（source={:?}）：此后字段缺失按 0 命中解释",
                    cache_endpoint, stream_result.cache_source
                );
            }
            let cache_cap = crate::infra::llm::usage_memory::capability(&cache_endpoint);
            let cache_outcome = crate::infra::llm::usage_memory::resolve_cache_outcome(
                stream_result.cache_hit_tokens,
                stream_result.cache_miss_tokens,
                stream_result.cache_source,
                cache_obs.reports_cache,
                cache_cap.cache_source.as_deref(),
                turn_in_tokens,
            );
            let cache_hit_tokens = cache_outcome.hit;
            let cache_miss_tokens = cache_outcome.miss;
            let cache_source = cache_outcome.source.as_deref();
            let cache_point = cache_hit_tokens.map(|hit| CacheHitPoint {
                loop_count: self.total_loop_count + 1,
                hit_tokens: hit,
                miss_tokens: cache_miss_tokens.unwrap_or(0),
                source: cache_outcome.source.clone(),
            });

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
                self.update_provider_usage_snapshot(
                    turn_in_tokens,
                    turn_out_tokens,
                    cache_hit_tokens,
                    cache_miss_tokens,
                    cache_source,
                    cache_point.as_ref(),
                );
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

            // 记录响应摘要（含缓存命中：provider 未报告时是 None，不是 0）
            debug_logger::debug_logger().log_response(
                &self.sid,
                "MAIN",
                self.total_loop_count + 1,
                current_text_this_turn.len(),
                current_thinking_this_turn.len(),
                tool_calls.len(),
                turn_in_tokens,
                turn_out_tokens,
                cache_hit_tokens,
                cache_miss_tokens,
                cache_source,
                // provider 本次没报字段、数值来自端点记忆推断 ⇒ 日志里标明
                stream_result.cache_hit_tokens.is_none() && cache_hit_tokens.is_some(),
                stream_result.usage_raw.as_deref(),
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

            // 取消检查移到这里之前只做「下一轮是否继续」的判断。
            let was_cancelled_this_loop = self.cancel_token.is_cancelled();
            if was_cancelled_this_loop {
                println!(
                    "[JARVIS] 检测到取消：仍先落库本轮的助手回复与工具结果，避免「工具已执行但无记录」"
                );
            }

            // 步骤 9：把本轮助手响应（文本/思考/工具调用）写入会话历史。
            // 必须在取消判定之后仍执行——工具已在步骤 8 实际运行（可能已改文件），
            // 其结果必须留档；否则历史会出现 Assistant(ToolUse) 缺失 ToolResult 的残缺配对。
            self.store_assistant_response(&current_blocks).await;

            // 工具结果为 User 消息，取消时同样必须落库以保持配对完整
            if was_cancelled_this_loop {
                if !tool_results.is_empty() {
                    let mut session = self.ctx.memory.lock().await;
                    append_message(&mut session, Message::User {
                        content: Content::Multiple(tool_results.clone()),
                    }, "chat");
                }
                continue;
            }

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

                // —— 模式切换快照：本 loop 的 SwitchWorkMode 真实变更了 work_mode ——
                // 不修改已发出的 <context_snapshot>，而是在消息尾部追加一条 seq 更大的完整新快照，
                // 历史前缀仍逐字节命中缓存；system 的“最新快照优先”规则随即切到新模式现场。
                let mode_after_execution = { self.ctx.agent_work_mode.lock().await.clone() };
                if mode_after_execution != work_mode {
                    println!(
                        "[JARVIS] 工作模式在本 loop 内切换：{} -> {}，追加完整上下文快照",
                        work_mode, mode_after_execution
                    );
                    self.append_mode_snapshot(&mode_after_execution).await;
                }

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

        // 汇总状态：正常结束 / 用户取消 / 循环超时暂停 / 运行被打断
        // interrupted_reason 由 handle_cancellation / handle_stream_idle_timeout 设置，
        // 且必须先于 was_cancelled 判断：取消也会置起 cancel_token，但语义是"中断"。
        let interrupted_reason = self.interrupted_reason.clone();
        let status = if interrupted_reason.is_some() {
            "INTERRUPTED"
        } else if was_cancelled {
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

        if let Some(reason) = &interrupted_reason {
            println!(
                "[JARVIS] 本轮为中断收尾（{}）：历史已保留，等待用户决定是否继续",
                reason
            );
        }

        // Agent Run 完成（中断与超时续跑都不标记完成，保持可续跑语义）
        if !was_cancelled && !was_loop_timeout && interrupted_reason.is_none() {
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

        println!(
            "[JARVIS] finalize：status={}，notice={:?}，content 长度={}",
            status,
            self.notice.as_deref().unwrap_or("<无>"),
            self.final_answer.chars().count()
        );

        JarvisResult {
            status: status.to_string(),
            // content 只放模型正文/部分结果；状态标注走 notice 单独下发，
            // 避免标记混进正文被渲染到回复气泡内部（实测反馈的 UI 问题）。
            content: self.final_answer,
            input_tokens: self.req_input_tokens,
            output_tokens: self.req_output_tokens,
            session_input_tokens,
            session_output_tokens,
            user_message_id: None,
            tool_execution_summary: self.tool_execution_summary,
            notice: self.notice,
            thinking_enabled: Some(self.turn_thinking_decision.enabled),
            thinking_reason: Some(format!("{:?}", self.turn_thinking_decision.reason)),
            thinking_notice_i18n_key: self
                .turn_thinking_decision
                .notice_i18n_key
                .map(|s| s.to_string()),
        }
    }

    // ─── 主循环辅助方法 ───

    /// 异常收尾：把中断原因记录到审计层与诊断层，**保留已产生的内容与历史**，
    /// 并清理运行状态（清空 active_run_id、释放取消令牌，保证下次可重新执行）。
    ///
    /// 历史演进说明：旧实现只做「记错误 → fail_run → 返回 Err」，现场全部丢弃。
    /// 由于 `run_pipeline_inner` 在此之后直接 `return Err`，`finalize()` 不会执行，
    /// 于是会话历史里连一句中断说明都没有，用户误以为"根本没执行"。
    /// 现改为：先把 live_content / live_thinking 补进历史，再落一条 `interrupted` 标记。
    ///
    /// 遵守的约束：**错误文本不作为助手消息内容**（避免被当成模型发言污染上下文），
    /// 只写入 agent_runs.error 与 agent_run_events。
    async fn abort_after_error(&mut self, error: &AgentError) {
        if self.run_id.is_empty() {
            *self.ctx.cancel_token.lock().await = None;
            return;
        }

        // 面向用户的小字标注：错误原文可能很长且含技术细节，
        // 这里给一句可读的结论，完整错误走事件层与 agent_runs.error（界面"执行详情"可查）。
        let notice_text = format!("⚠ 本轮执行中断：{}", error);
        println!("[JARVIS] 异常收尾（保留现场）: {}", error);

        // 1. 把已流式输出但尚未入库的内容补进历史（live_content 全程累积，是中断时的快照）
        let run = crate::core::orchestration::agent_run_repository::list_runs(Some(&self.sid))
            .ok()
            .and_then(|runs| runs.into_iter().find(|r| r.run_id == self.run_id));
        let live_content = run.as_ref().map(|r| r.live_content.clone()).unwrap_or_default();
        let live_thinking = run.as_ref().map(|r| r.live_thinking.clone()).unwrap_or_default();
        let partial = if !live_content.trim().is_empty() {
            live_content.trim().to_string()
        } else if !live_thinking.trim().is_empty() {
            live_thinking.trim().to_string()
        } else {
            String::new()
        };
        if !partial.is_empty() {
            let mut session = self.ctx.memory.lock().await;
            if !assistant_text_exists_at_tail(&session.messages, &partial) {
                append_message(
                    &mut session,
                    Message::Assistant {
                        content: Content::Single(partial.clone()),
                    },
                    "chat",
                );
            }
        }

        // 2. 追加中断标记（source = interrupted：模型可见，用于"继续"时定位断点）。
        //
        //    标记使用**统一措辞**（中断原因已在用户可见的 notice 中给出，
        //    历史标记只需让模型知道"该接着做"）。原因见常量注释。
        self.append_interrupted_marker(INTERRUPT_MARKER_RESUMABLE)
            .await;
        self.final_answer = partial.clone();
        self.notice = Some(notice_text.clone());
        self.interrupted_reason = Some(error.to_string());

        // 3. 落库：审计层事件 + 诊断层错误
        agent_runs::record_tool_result(
            &self.app,
            &self.run_id,
            "pipeline",
            Some(error.to_string()),
            None,
            self.total_loop_count,
        );
        agent_runs::interrupt_run(
            &self.app,
            &self.run_id,
            error.to_string(),
            self.req_input_tokens,
            self.req_output_tokens,
        );

        // 4. 中断后立即持久化，保证用户刷新后仍能看到保留的内容与中断标记
        {
            let memory = self.ctx.memory.lock().await.clone();
            crate::core::session::save_session(&self.sid, &memory, None);
            let _ = self.app.emit("session-updated", ());
        }

        // 5. 通知前端（与用户取消区分：type = interrupted）
        //    状态标注走 notice 结构化下发，不再推进正文
        let _ = self.app.emit(
            "agent-step",
            json!({
                "type": "interrupted",
                "reason": "pipeline_error",
                "sessionId": self.sid,
                "loopCount": self.total_loop_count + 1
            }),
        );

        {
            let mut active_run_id = self.ctx.active_run_id.lock().await;
            if active_run_id.as_deref() == Some(&self.run_id) {
                *active_run_id = None;
            }
        }
        *self.ctx.cancel_token.lock().await = None;
    }

    /// 中断收尾共用逻辑：**保留全部已持久化历史**，并把中断提示追加为一条
    /// `source = "interrupted"` 的助手消息。
    ///
    /// 与「用户主动撤销」（撤回消息 / 回滚检查点）的关键区别：
    /// 撤销是用户希望内容消失，截断正确；中断只是运行被打断，
    /// **任何一方都不希望数据消失**，因此这里绝不 truncate。
    ///
    /// `source = "interrupted"` 的语义定位：
    /// - LLM 上下文：**可见**（`prepare_history_snapshot_from_messages` 白名单），
    ///   使"继续"时模型能看到自己中断于何处；
    /// - UI 渲染：**可见**（`command/history.rs` 的渲染门），否则中断轮次会从界面消失。
    ///
    /// 返回该消息在会话历史中的下标。
    async fn append_interrupted_marker(&mut self, reason: &str) -> usize {
        let mut session = self.ctx.memory.lock().await;
        append_message(
            &mut session,
            Message::Assistant {
                content: Content::Single(reason.to_string()),
            },
            "interrupted",
        );
        session.messages.len() - 1
    }

    /// 等待上游响应期间的"进度提示"看门狗。
    ///
    /// 上游深度思考、网络迟滞、或者**收到响应头后正文长时间静默**时，界面上
    /// 可能长时间没有任何事件，用户只看到转圈，无法区分"模型在思考"与
    /// "服务已经死了"。本看门狗让等待**可见**。
    ///
    /// ## 为什么必须跨越两个阶段
    ///
    /// 首个版本的看门狗只覆盖"等待响应头"阶段，一旦 `call_api_with_retry`
    /// 返回就 `abort()`。但真实故障几乎都发生在**响应头已到、正文静默**阶段
    /// （响应头通常很快返回），于是看门狗永远没机会触发。
    ///
    /// 因此改为：从请求发出前启动，一直存活到本轮流结束，
    /// 启动进度提示看门狗。返回后应立即 `touch()` 一次以开始计时。
    fn spawn_waiting_hint(&self) -> WaitingHint {
        use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};

        let hint_secs = crate::infra::types::constants::API_WAITING_HINT_SECS;
        let last_tick_ms = Arc::new(AtomicU64::new(now_millis()));
        let hint_sent = Arc::new(AtomicBool::new(false));
        let done = Arc::new(AtomicBool::new(false));
        let app = self.app.clone();
        let sid = self.sid.clone();
        let loop_count = self.total_loop_count + 1;
        let tick = last_tick_ms.clone();
        let sent = hint_sent.clone();
        let stop = done.clone();

        let handle = tokio::spawn(async move {
            // 轮询粒度取提示阈值的一半，保证提示足够及时
            let step = Duration::from_secs((hint_secs / 2).max(1));
            loop {
                tokio::time::sleep(step).await;
                if stop.load(Ordering::Relaxed) {
                    return;
                }
                let silent_ms = now_millis().saturating_sub(tick.load(Ordering::Relaxed));
                // 同一段静默期只提示一次：收到新数据后 touch() 会复位 sent
                if should_emit_waiting_hint(silent_ms / 1000, sent.load(Ordering::Relaxed)) {
                    sent.store(true, Ordering::Relaxed);
                    // 走 agent-step 而非 chat-stream：等待提示属于**状态标注**，
                    // 应渲染在气泡下方的小字里；经 chat-stream 会混进模型正文，
                    // 在回复气泡内部显示（实测反馈的 UI 问题）。
                    let _ = app.emit(
                        "agent-step",
                        json!({
                            "type": "waiting_hint",
                            "content": format!(
                                "⏳ 已 {} 秒未收到数据，仍在等待模型响应…深度思考或上游繁忙时可能较慢，接口超时会自动终止，无需手动停止。",
                                silent_ms / 1000
                            ),
                            "sessionId": sid,
                            "loopCount": loop_count
                        }),
                    );
                }
            }
        });

        WaitingHint {
            last_tick_ms,
            hint_sent,
            done,
            handle,
        }
    }

    /// 用户取消处理：保留全部历史与已流式输出的部分内容，把中断原因作为
    /// `interrupted` 消息追加，并标记 run 为 CANCELLED。
    ///
    /// 历史演进说明：旧实现在此处把历史 `truncate` 到本轮用户消息之后，
    /// 结果是「已跑完的 10 轮工具调用 + 思考」被整段删除，只剩一句残文；
    /// 且因取消检查位于 `store_assistant_response()` 之前，中断轮的回复与
    /// 工具结果也从未写入。现改为不截断、只追加标记。
    async fn handle_cancellation(&mut self) {
        println!(
            "[JARVIS] 用户已取消执行：保留全部历史，user index {}",
            self.initial_msg_index
        );

        // 取回已流式输出的部分结果（live_content / thinking 全程累积，是中断时的唯一快照）
        let run = crate::core::orchestration::agent_run_repository::list_runs(Some(&self.sid))
            .ok()
            .and_then(|runs| runs.into_iter().find(|r| r.run_id == self.run_id));
        let live_content = run.as_ref().map(|r| r.live_content.clone()).unwrap_or_default();
        let live_thinking = run.as_ref().map(|r| r.live_thinking.clone()).unwrap_or_default();

        let partial = if !live_content.trim().is_empty() {
            live_content.trim().to_string()
        } else if !live_thinking.trim().is_empty() {
            live_thinking.trim().to_string()
        } else if !self.final_answer.is_empty() && self.final_answer != "用户已取消执行。" {
            std::mem::take(&mut self.final_answer)
        } else {
            String::new()
        };

        // 半截内容若已由 store_assistant_response 落库，则不重复写入；
        // 否则补一条 chat 消息，保证用户仍能看到中断前已生成的内容。
        if !partial.is_empty() {
            let mut session = self.ctx.memory.lock().await;
            if !assistant_text_exists_at_tail(&session.messages, &partial) {
                append_message(
                    &mut session,
                    Message::Assistant {
                        content: Content::Single(partial.clone()),
                    },
                    "chat",
                );
            }
        }

        // `reason` 面向用户；写入历史的标记面向 LLM（会进上下文），
        // 故用最小信息量的统一措辞，避免模型把系统视角描述当成自己的话。
        let reason = "> ✕ **用户已取消执行（以上为保留的部分结果，历史未截断）**";
        let marker_index = self
            .append_interrupted_marker(INTERRUPT_MARKER_RESUMABLE)
            .await;
        self.final_answer = partial.clone();
        self.notice = Some(reason.to_string());
        self.interrupted_reason = Some("用户取消".to_string());

        // 状态标注走 notice 结构化下发，不再推进正文
        let _ = self.app.emit(
            "agent-step",
            json!({
                "type": "cancelled",
                "sessionId": self.sid,
                "loopCount": self.total_loop_count + 1
            }),
        );
        // 标记 run 为 CANCELLED，并向前端发送取消通知
        agent_runs::cancel_run(
            &self.app,
            &self.run_id,
            self.req_input_tokens,
            self.req_output_tokens,
            Some(self.final_answer.clone()),
        );
        println!(
            "[JARVIS] 已写入中断标记（interrupted），消息下标 {}",
            marker_index
        );
    }

    /// 上游失联（流内空闲超时）收尾：保留现场，结束本轮，把决策权交还用户。
    ///
    /// 流层已采用「优雅终止」——`process_stream` 返回已累积的部分结果而非抛错，
    /// 因此这里不需要（也不应该）走 `fail_run` 的抹除路径。服务恢复后用户
    /// 直接说"继续"即可接上，因为历史完整且标记对模型可见。
    async fn handle_stream_idle_timeout(&mut self, stream_result: crate::core::agent::StreamResult) {
        let partial = stream_result.text.trim().to_string();
        println!(
            "[JARVIS] 上游失联收尾：已累积文本 {} 字，工具={}，loop={}",
            partial.chars().count(),
            stream_result.has_tool,
            self.total_loop_count + 1
        );

        // 面向用户的文案：会渲染成气泡下方的小字（notice），
        // 因此不用 Markdown 引用符号 —— 小字是纯文本，`>` 会原样显示。
        let reason = format!(
            "⚠ 上游服务已停止响应（连续 {} 秒未收到数据），已自动终止本轮。\
             以上为已保留的部分结果，历史未截断。回复「继续」即可接着做。",
            crate::core::agent::stream::STREAM_IDLE_TIMEOUT_SECS
        );

        // 写入历史的标记文案与 `reason` 刻意不同：
        // `reason` 面向用户（含"服务已停止响应"这类系统视角描述），
        // 而 `interrupted` 消息**会发给 LLM**（见白名单），
        // 故用统一的"接着做"措辞（见 INTERRUPT_MARKER_RESUMABLE 注释）。
        let llm_marker = INTERRUPT_MARKER_RESUMABLE;

        let mut session = self.ctx.memory.lock().await;
        if !partial.is_empty() && !assistant_text_exists_at_tail(&session.messages, &partial) {
            append_message(
                &mut session,
                Message::Assistant {
                    content: Content::Single(partial.clone()),
                },
                "chat",
            );
        }
        drop(session);

        self.append_interrupted_marker(llm_marker).await;
        // content 只保留部分结果；状态标注走 notice（气泡下方小字），
        // 这样前端无需从正文里"猜"哪部分是标注。
        self.final_answer = partial.clone();
        self.notice = Some(reason.clone());
        self.interrupted_reason = Some("上游服务失联（流内空闲超时）".to_string());

        // 状态标注不再经 chat-stream 推进正文（会挤进回复气泡内部），
        // 改由 JarvisResult.notice 结构化下发，前端渲染为气泡下方小字。
        let _ = self.app.emit(
            "agent-step",
            json!({
                "type": "interrupted",
                "reason": "stream_idle_timeout",
                "sessionId": self.sid,
                "loopCount": self.total_loop_count + 1
            }),
        );
        println!(
            "[JARVIS] 中断收尾完成：notice={:?}，content 长度={}",
            self.notice.as_deref().unwrap_or("<无>"),
            self.final_answer.chars().count()
        );

        // 记录到审计层（agent_run_events），供界面「执行详情」展示中断原因
        agent_runs::record_tool_result(
            &self.app,
            &self.run_id,
            "stream_idle_timeout",
            Some(reason.clone()),
            None,
            self.total_loop_count,
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
            // 循环续跑确认没有会话级允许语义：照抄"本次会话都允许"会顺带放行其它工具调用
            None,
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
        //
        // `interrupted` 刻意列入白名单：运行被打断（取消/失联/报错）后，模型需要
        // 看到自己中断于何处，用户回一句"继续"才能顺着接上。半截正文出现在
        // 上下文里是预期行为，因为它真实反映了中断时的状态。
        let session_turn_start = self.initial_msg_index;
        let mut filtered: Vec<Message> = Vec::with_capacity(messages.len());
        let mut snapshot_turn_start: Option<usize> = None;
        for (idx, (msg, src)) in messages.into_iter().zip(sources.iter()).enumerate() {
            if !matches!(src.as_str(), "chat" | "compact" | "context" | "interrupted") {
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
        // 沿用上一次快照里的缓存口径与逐 loop 趋势：
        // - 趋势不能每次重算快照就清零；
        // - 缓存数值也一并沿用（它描述的是"这条前缀最近一次观测到的命中情况"），
        //   否则每轮请求构建期间 UI 都会闪回「?」——厂商在请求进行中并不会重新表态。
        let previous_cache = crate::core::session::get_context_snapshot(&self.sid)
            .ok()
            .flatten()
            .map(|previous| {
                (
                    previous.cache_hit_tokens,
                    previous.cache_miss_tokens,
                    previous.cache_source,
                    previous.cache_history,
                )
            })
            .unwrap_or((None, None, None, Vec::new()));
        let (prev_cache_hit, prev_cache_miss, prev_cache_source, cache_history) = previous_cache;
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
            cache_hit_tokens: prev_cache_hit,
            cache_miss_tokens: prev_cache_miss,
            cache_source: prev_cache_source,
            cache_history,
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

    fn update_provider_usage_snapshot(
        &self,
        input_tokens: u64,
        output_tokens: u64,
        cache_hit_tokens: Option<u64>,
        cache_miss_tokens: Option<u64>,
        cache_source: Option<&str>,
        cache_point: Option<&CacheHitPoint>,
    ) {
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
            cache_hit_tokens,
            cache_miss_tokens,
            cache_source,
            cache_point,
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
            // Anthropic 出口的 thinking 块策略按服务商分两种：
            // - 真 Anthropic：无 signature 的 thinking 回传会被判 400 → 必须剥掉；
            // - DeepSeek 这类端点：思考模式下**要求**把 thinking 原样带回，剥掉会报
            //   `content[].thinking in the thinking mode must be passed back to the API`
            //   （与 OpenAI 出口的 reasoning_content 回填是同一件事的两面）。
            if crate::infra::llm::adapters::should_strip_unsigned_thinking(
                &self.model_id,
                &self.cfg.base_url,
                self.should_think,
            ) {
                let messages = crate::infra::llm::adapters::strip_unsigned_thinking_for_anthropic(
                    &request_body.messages,
                );
                request_body.messages = messages;
            } else {
                println!(
                    "[JARVIS] Anthropic 出口：保留无签名 thinking 块（{} 要求回传思考链）",
                    self.model_id
                );
            }
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
            api_client::MAX_API_RETRIES,
            &self.app,
            &self.sid,
        );

        match tokio::select! {
            result = tokio::time::timeout(
                Duration::from_secs(
                    crate::infra::types::constants::API_RESPONSE_HEADER_TIMEOUT_SECS,
                ),
                api_request,
            ) => {
                match result {
                    Ok(result) => result,
                    Err(_) => {
                        let error = ApiError::Network(format!(
                            "API 请求超过 {} 秒未返回响应头，已自动终止。",
                            crate::infra::types::constants::API_RESPONSE_HEADER_TIMEOUT_SECS
                        ));
                        // 记录错误事件到 agent_run_events 表
                        agent_runs::record_tool_result(
                            &self.app,
                            &self.run_id,
                            "api_call",
                            Some(error.to_string()),
                            None,
                            self.total_loop_count,
                        );
                        agent_runs::fail_run(
                            &self.app,
                            &self.run_id,
                            error.to_string(),
                            self.req_input_tokens,
                            self.req_output_tokens,
                        );
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
                agent_runs::fail_run(
                    &self.app,
                    &self.run_id,
                    e.to_string(),
                    self.req_input_tokens,
                    self.req_output_tokens,
                );
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
                    StreamConfig {
                        is_subagent: false,
                        cache_usage_style:
                            crate::infra::llm::registry::cache_usage_style_for(&self.model_id),
                        // 看门狗小结是短请求，不需要等待提示
                        on_frame: None,
                    },
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

    /// 中途模式切换：在消息尾部追加一条 seq 更大的完整上下文快照（追加，不回改旧前缀）。
    async fn append_mode_snapshot(&mut self, mode: &str) {
        let seq = {
            let mut session = self.ctx.memory.lock().await;
            session.snapshot_seq = session.snapshot_seq.saturating_add(1);
            session.snapshot_seq
        };

        let capabilities = crate::core::tools::framework::capabilities::Capabilities::for_work_mode(mode);
        let snapshot = build_dynamic_context(
            &self.detected_intent,
            &self.request_workspace,
            &capabilities,
            mode,
            seq,
        );
        self.capabilities = capabilities;

        if snapshot.is_empty() {
            return;
        }

        self.dynamic_context_str = snapshot.clone();
        let snapshot_memory = {
            let mut session = self.ctx.memory.lock().await;
            append_message(&mut session, Message::User {
                content: Content::Multiple(vec![ContentBlock::Context { text: snapshot }]),
            }, "context");
            session.clone()
        };
        // 立即落库，保证崩溃恢复时 snapshot_seq 与这条新快照同时存在，避免 seq 回退/重复
        crate::core::session::save_session(&self.sid, &snapshot_memory, None);
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

#[cfg(test)]
mod thinking_freeze_tests {
    use super::{resolve_turn_think, should_think_for_loop};

    /// 回归：override=false + audience 默认 true（当前 DeepSeek 用户的实际组合）时，
    /// 整轮每个 loop 都必须是 disabled。旧实现在 loop>0 回落到 audience 默认，
    /// 于是第二轮变 enabled，历史里没有 thinking 块的 assistant 无法满足
    /// Anthropic 的「thinking 必须回传」要求 → 400。
    #[test]
    fn thinking_state_is_frozen_within_a_turn() {
        let turn_think = resolve_turn_think(Some(false), true);
        assert!(!turn_think, "用户临时关闭应覆盖受众默认值");
        for loop_count in 0..=5 {
            assert!(
                !should_think_for_loop(turn_think, loop_count),
                "loop {} 不得把 thinking 翻转成 enabled",
                loop_count
            );
        }
    }

    #[test]
    fn thinking_stays_enabled_for_the_whole_turn() {
        let turn_think = resolve_turn_think(Some(true), false);
        assert!(turn_think);
        for loop_count in 0..=5 {
            assert!(should_think_for_loop(turn_think, loop_count));
        }
    }

    #[test]
    fn thinking_falls_back_to_audience_default_without_override() {
        assert!(resolve_turn_think(None, true));
        assert!(!resolve_turn_think(None, false));
    }
}

#[cfg(test)]
mod interrupted_tail_dedup_tests {
    use super::assistant_text_exists_at_tail;
    use crate::infra::types::models::*;

    fn assistant_single(text: &str) -> Message {
        Message::Assistant {
            content: Content::Single(text.to_string()),
        }
    }

    fn assistant_blocks(blocks: Vec<ContentBlock>) -> Message {
        Message::Assistant {
            content: Content::Multiple(blocks),
        }
    }

    /// 中断收尾时若半截内容已由 store_assistant_response 落库，不得重复写入
    #[test]
    fn detects_existing_tail_text() {
        let messages = vec![assistant_single("沐先生，很")];
        assert!(assistant_text_exists_at_tail(&messages, "沐先生，很"));
    }

    /// 前后空白不应影响判定，否则同一段话会被写两遍
    #[test]
    fn ignores_surrounding_whitespace() {
        let messages = vec![assistant_single("  沐先生，很  ")];
        assert!(assistant_text_exists_at_tail(&messages, "沐先生，很"));
    }

    /// 多块消息里的 Text 块同样要能被识别（流式回复常为多块）
    #[test]
    fn detects_text_inside_multiple_blocks() {
        let messages = vec![assistant_blocks(vec![
            ContentBlock::Thinking {
                thinking: "先看看目录".to_string(),
                signature: String::new(),
            },
            ContentBlock::Text {
                text: "正在读取文件".to_string(),
            },
        ])];
        assert!(assistant_text_exists_at_tail(&messages, "正在读取文件"));
    }

    /// 不同内容不得误判为已存在，否则中断内容会被漏写
    #[test]
    fn does_not_match_different_text() {
        let messages = vec![assistant_single("上一轮已经说完的完整回复")];
        assert!(!assistant_text_exists_at_tail(&messages, "本轮被打断的半截话"));
    }

    /// 空目标视为不存在，避免把空串当命中而跳过写入
    #[test]
    fn empty_target_is_never_a_match() {
        let messages = vec![assistant_single("")];
        assert!(!assistant_text_exists_at_tail(&messages, "   "));
    }

    /// 只检查尾部若干条：很早以前的相同文本不应影响本次判定
    #[test]
    fn only_inspects_recent_tail() {
        let mut messages = vec![assistant_single("重复的一句话")];
        for _ in 0..6 {
            messages.push(assistant_single("中间过程的其它回复"));
        }
        assert!(
            !assistant_text_exists_at_tail(&messages, "重复的一句话"),
            "超出尾部窗口的历史不应被当作本次已写入"
        );
    }

    /// 用户消息里的相同文本不算命中（只认助手消息）
    #[test]
    fn user_message_does_not_count() {
        let messages = vec![Message::User {
            content: Content::Single("沐先生，很".to_string()),
        }];
        assert!(!assistant_text_exists_at_tail(&messages, "沐先生，很"));
    }
}

#[cfg(test)]
mod interrupt_marker_tests {
    //! **措辞收敛的防回归**：中断标记按「模型能否接着做」二分。
    //!
    //! 背景：曾按中断原因写了 4 种措辞（超时/报错/取消/轮次上限）。
    //! 该标记进入对话历史后**每轮都会重发**，多种措辞 = 多个 prompt 前缀变体，
    //! 会让 provider 的 prompt cache 更易失效；而原因并不改变模型的动作。
    //! 因此"能接着做"的三种统一成一句。
    use super::{INTERRUPT_MARKER_RESUMABLE, INTERRUPT_MARKER_STOPPED};

    /// 能接着做的三种中断（超时/报错/取消）必须完全一致 —— 措辞漂移会让
    /// 缓存命中率下降，且难以察觉。
    #[test]
    fn resumable_markers_are_identical() {
        assert_eq!(
            INTERRUPT_MARKER_RESUMABLE,
            "> ⚠️ **[回复被中断]** 上次回复在此处中断，请基于上下文继续完成。"
        );
    }

    /// 轮次上限刻意不同：此时模型**不该**接着写，否则立刻再次触达上限
    #[test]
    fn stopped_marker_is_distinct() {
        assert_ne!(INTERRUPT_MARKER_RESUMABLE, INTERRUPT_MARKER_STOPPED);
        assert!(
            INTERRUPT_MARKER_STOPPED.contains("回合上限"),
            "轮次上限标记应说明停下原因"
        );
    }

    /// 两种标记共享统一前缀，前端 `splitInterruptMarker` 依赖它做识别
    #[test]
    fn both_share_detectable_prefix() {
        for marker in [INTERRUPT_MARKER_RESUMABLE, INTERRUPT_MARKER_STOPPED] {
            assert!(
                marker.contains("[回复被中断]"),
                "前端剥离逻辑依赖该前缀，不得修改：{marker}"
            );
            assert!(
                marker.trim_start().starts_with('>'),
                "应以 Markdown 引用开头，便于统一剥离：{marker}"
            );
        }
    }
}

#[cfg(test)]
mod stream_retry_gate_tests {
    //! 重试判据测试。
    //!
    //! 判据已收敛到 `stream::is_zero_output`（见该函数注释），本模块只断言
    //! "零产出才重试"这一条策略本身。**关键回归**：曾把 `idle_timed_out` 与
    //! "是否允许重试"合用一个字段，导致"吐了半截后静默"时超时判定被置 false，
    //! run 永久卡死并锁住会话。因此这里必须覆盖"收到内容后不重试"的每一种形态。
    use crate::core::agent::stream::is_zero_output;

    /// 上游静默超时且零产出 → 允许重试（最需要重试的场景）
    #[test]
    fn zero_output_retries() {
        assert!(is_zero_output(true, true, false));
    }

    /// 已收到正文 → 不重试（避免界面文本拼接重复）
    #[test]
    fn partial_text_does_not_retry() {
        assert!(!is_zero_output(false, true, false));
    }

    /// 只收到思考 → 不重试（丢弃思考块会破坏思考链回传要求）
    #[test]
    fn thinking_only_does_not_retry() {
        assert!(!is_zero_output(true, false, false));
    }

    /// 已产生工具调用 → 不重试（工具可能已执行，重试会重复副作用）
    #[test]
    fn tool_call_does_not_retry() {
        assert!(!is_zero_output(true, true, true));
    }

    /// 回归防护（本次线上 bug）：正文与思考**都收到部分**后静默 → 不重试。
    /// 这正是 interrupted 模式的行为。此前该场景会因字段含义混用而既不重试
    /// 也不结束本轮，run 卡死。
    #[test]
    fn partial_text_and_thinking_does_not_retry() {
        assert!(!is_zero_output(false, false, false));
    }
}

#[cfg(test)]
mod waiting_hint_tests {
    use super::should_emit_waiting_hint;
    use crate::infra::types::constants::API_WAITING_HINT_SECS;

    /// 静默未达阈值：不提示（避免正常生成被打扰）
    #[test]
    fn no_hint_before_threshold() {
        assert!(!should_emit_waiting_hint(API_WAITING_HINT_SECS - 1, false));
        assert!(!should_emit_waiting_hint(0, false));
    }

    /// 刚好达到阈值且本段静默期未提示过 → 提示
    #[test]
    fn hints_once_at_threshold() {
        assert!(should_emit_waiting_hint(API_WAITING_HINT_SECS, false));
    }

    /// **回归防护（用户实测 bug）**：同一段静默期继续延长时**不得重复提示**。
    /// 首版每 30s 无条件发一条，页面被「已 30 秒未收到数据」刷屏。
    #[test]
    fn does_not_repeat_within_same_silence() {
        for extra in [0, 30, 60, 300] {
            assert!(
                !should_emit_waiting_hint(API_WAITING_HINT_SECS + extra, true),
                "静默 {} 秒时不应重复提示",
                API_WAITING_HINT_SECS + extra
            );
        }
    }
}

#[cfg(test)]
mod idle_terminal_tests {
    //! 「空闲超时必须以中断收尾」的语义防护。
    //!
    //! **实测 bug**：首版只在"零产出"时才收尾，于是 `interrupted` 形态
    //! （吐了半截后静默）虽然触发了计时器，却走到了正常完成路径 ——
    //! 界面只剩半截正文，用户以为已经正常完成。
    use super::must_finish_as_interrupted;

    /// 计时器触发就必须收尾，与是否重试、产出多少无关
    #[test]
    fn idle_timeout_always_terminates() {
        // (idle_timed_out, should_retry) → 必须以中断收尾
        assert!(must_finish_as_interrupted(true, true)); // hang：零帧静默
        assert!(
            must_finish_as_interrupted(true, false),
            "吐了半截后静默也必须收尾并给出中断提示——这正是实测漏掉的场景"
        );
    }

    /// 未触发空闲超时时不得收尾（否则正常完成会被误判为中断）
    #[test]
    fn normal_completion_does_not_terminate() {
        assert!(!must_finish_as_interrupted(false, false));
        assert!(!must_finish_as_interrupted(false, true));
    }

    /// 防回归：收尾判据**不得**依赖 should_retry（用错字段即本次 bug）
    #[test]
    fn termination_does_not_depend_on_retry_flag() {
        for idle in [true, false] {
            assert_eq!(
                must_finish_as_interrupted(idle, true),
                must_finish_as_interrupted(idle, false),
                "should_retry 不应影响收尾判定（idle_timed_out={idle}）"
            );
        }
    }
}
