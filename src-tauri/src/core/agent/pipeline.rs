//! # pipeline.rs — Agent 主循环流水线
//!
//! 实现 Agent 的 4 阶段执行流水线：初始化 → 循环前准备 → 主循环 → 收尾。
//! 主循环阶段包含压缩检查、API 调用、流式处理、工具执行、反思审查等完整 Agent Loop 逻辑。
//!
//! ## 四阶段流水线地图
//!
//! 入口：`run_pipeline()`（新消息）/ `resume_pipeline()`（续跑）→ `run_pipeline_inner()` 按序调度：
//!
//! | 阶段 | 函数 | 职责 |
//! |---|---|---|
//! | 1 初始化 | `setup()` | 校验会话占用、加载配置、创建取消令牌、意图分类、组装 PipelineState |
//! | 2 循环前准备 | `pre_loop()` | 崩溃恢复、注入用户消息、创建 run 记录、决定是否深度思考 |
//! | 3 主循环 | `run_main_loop()` | 调 LLM → 流式解析 → 执行工具 → 结果回写，直到 LLM 不再调工具 |
//! | 4 收尾 | `finalize()` | 检查点快照、保存会话、自动起名、记忆超预算时后台整理、组装 JarvisResult |
//!
//! ### 关于「意图验证」阶段（已作废，勿再按五阶段描述本文件）
//!
//! 这里曾规划过一个独立的第 2 阶段 `validate()`（DANGEROUS 弹权限确认 / UNCLEAR 返回澄清），
//! 它**从未落地**——`run_pipeline_inner()` 是从阶段 1 直接进阶段 2 的。后续演化为：
//! - 意图分类留在 `setup()` 内（见其文档注释第 4 步）；
//! - 权限确认**下移到工具执行期逐次审批**：`request_permission()` 由主循环的工具调用处
//!   与 `shell_tools::execution` 直接调用，受 `approval_mode`（`request_approval` /
//!   `auto_approve`）控制，不再有"跑之前先整体预检一遍"的环节。
//!
//! ### 阶段 3 主循环每轮内部子步骤
//! 取消检查 → 循环次数确认 → 后台通知注入 → 上下文压缩 → 历史快照 → 构建请求
//! → API 调用（含调度器事件 select）→ 流式处理 → 工具执行 → 反思审查 → 回写历史 → 下一轮
//!
//! ### 配套辅助函数（各阶段共用）
//! - 历史准备：`prepare_history_snapshot()` / `prepare_history_snapshot_from_messages()` / `fix_broken_tool_call_pairs()`
//! - 上下文监控：`build_context_estimate()` / `update_context_snapshot()` / `update_provider_usage_snapshot()` / `resolve_max_tokens()`
//! - 请求构建：`build_llm_request()` / `call_api_with_retry()` / `current_tools()`
//! - 流程控制：`handle_sched_event()` / `request_loop_continuation()` / `compact_if_needed()`
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
use crate::core::session::{
    append_message, append_message_with_kind, memory::*, normalize_message_ids, pop_message,
    restore_message,
};
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
    /// 会话级思考档位（L2，随会话走的**布尔**）。
    ///
    /// v14 起不再有"auto"——初值在会话创建/首次发消息时已按"设置默认 + 模型能力"
    /// 固化进 `sessions.thinking_mode`（见 [`Self::ensure_session_thinking_initialized`]）。
    session_think_mode: crate::core::session::thinking::ThinkingMode,
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
    /// 本轮用户消息落库时分配的 message_id，随 `JarvisResult` 下发前端。
    ///
    /// 为什么需要它：前端发消息时先在本地插一条用户消息占位（那一刻后端还没生成
    /// ID），拿到这个值才能把真实 ID 补回去、撤回按钮立即出现 ——
    /// 否则要等刷新从数据库重读才渲染得出来（实测 bug）。
    user_message_id: Option<String>,
    should_think: bool,
    run_id: String,
    /// 循环状态
    loop_count: usize,
    total_loop_count: usize,
    req_input_tokens: u64,
    req_output_tokens: u64,
    /// 本次运行里**子代理**消耗的 token，单独留一份计数。
    ///
    /// 为什么必须单独记：主 Agent 每个 loop 的用量在拿到 usage 时就已通过
    /// `update_provider_usage_snapshot` 逐请求累加进 `sessions.total_*` 了，
    /// 而子代理（`tools/agent_tools/subagent.rs`）有自己独立的循环，不经过那条路径。
    /// 如果收尾时把 `req_input_tokens`（含主循环 + 子代理）整份再交一次，
    /// 主循环那部分就会被**重复计一遍**。所以收尾只补这两个数。
    ///
    /// 曾经还有一对 `req_cache_hit_tokens` / `req_cache_miss_tokens`（整轮缓存累计），
    /// 现已删除：它们是唯一读者——收尾那次整轮累加——的输入，改成逐请求累加后没人读了。
    /// 缓存口径的唯一真相现在是 `sessions.total_cache_*`。
    req_sub_input_tokens: u64,
    req_sub_output_tokens: u64,
    final_answer: String,
    /// 反思审查状态
    reflection_mode: String,
    total_reflections: usize,
    consecutive_reflection_nos: usize,
    /// Plan 看门狗：plan 模式下连续无喂狗动作的空转计数。
    ///
    /// 语义是「**轮次**」而非「工具调用次数」：无工具轮（纯文本）计 1，
    /// 有工具轮按调用次数计。旧实现只累加 `tool_calls.len()`，导致纯文本
    /// 轮次恒加 0、这条判据对"无工具空转"天然免疫（2026-09-19 死循环事故）。
    plan_consecutive_stalls: usize,
    /// Plan 看门狗：plan 模式下累计无 ProposePlan / 降级 edit 的 loop 次数
    plan_total_loops_without_plan: usize,
    /// 本轮因上游失联（流内空闲超时）而中断时的原因说明。
    /// Some 表示属于"运行被打断"，收尾时必须保留现场、不得截断历史。
    interrupted_reason: Option<String>,
    /// 面向用户的状态标注（气泡下方小字），由中断收尾路径填充。
    notice: Option<String>,
    /// 本 loop 的**结构化**响应块（Text / Thinking / ToolUse，顺序即产生顺序）。
    ///
    /// 中断收尾（取消 / 上游失联 / 执行报错）改读这里，不再读 `agent_runs.live_*`
    /// 那对字符串：结构化块天然带顺序与类型，不会把思考与正文糅成一段。
    ///
    /// 生命周期：每个 loop 在流层返回后**整体覆盖**，中断收尾时读当前值。
    current_blocks: Vec<ContentBlock>,
    /// 本 loop 的正文 / 思考纯文本（`current_blocks` 为空时的兜底来源）。
    turn_text_this_turn: String,
    turn_thinking_this_turn: String,
    /// 「崩溃保护（实时保存）」——**纯全局**设置（`UiPreferences.crash_protection`）。
    ///
    /// 关（默认）：只在 loop 收尾写一次 `agent_run_events`；
    /// 开：额外开一条帧级通道，进程崩溃时最多丢一个攒批窗口（200ms / 1KB）。
    ///
    /// 这是**全局**开关而非会话级：它保护的是"进程突然没了"这种与应用状态无关的
    /// 故障，与会话内容无关，做成会话级只会让用户在多会话间反复拨动却毫无收益。
    crash_protection: bool,
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
/// 阈值**由流空闲超时派生**（`stream::waiting_hint_secs()` = 超时阈值 / 3），
/// 不再是独立常量——两者曾各自为政、必须人工同步，超时放宽后提示会落在尴尬位置。
///
/// 回归防护：首版每 30 秒无条件发一条，页面被「已 30 秒未收到数据」刷屏
/// （用户实测反馈）。现在约定：**同一段静默期只提示一次**，
/// 直到收到新数据（`touch()` 复位 `already_sent`）才允许再次提示。
fn should_emit_waiting_hint(silent_secs: u64, already_sent: bool) -> bool {
    silent_secs >= crate::core::agent::stream::waiting_hint_secs() && !already_sent
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
///
/// 格式约定（2026-09-20 统一，详见 doc/状态标注符号统一与结构化改造方案.md）：
/// 标记行形如 `**[标签]** 说明文字`，**不带 `>` 引用符**。括号标签是**枚举**，
/// 比 emoji 哨兵精确；`>` 也只是给人看的引用符，对模型理解无帮助。
///
/// 去向：本标记**只写给模型看** —— `prepare_history_snapshot_from_messages`
/// 按 `interrupt_kind` 把它拼回消息尾部，模型据此知道上一句被截断。
/// 界面侧的小字说明由结构化 `notice` 承担，前端**不再**从正文里剥离标记
/// （原 `INTERRUPT_MARKER_LINE_RE` 已于 2026-09-21 删除：它对新数据永不命中，
/// 却会把模型正文里自己写的 `⚠` 当标记、截断整条回复）。
const INTERRUPT_MARKER_RESUMABLE: &str =
    "**[回复被中断]** 上次回复在此处中断，请基于上下文继续完成。";

/// 已达轮次上限：与上面刻意不同 —— 此时模型**不该**接着写，
/// 否则会立刻再次触达上限。
const INTERRUPT_MARKER_STOPPED: &str =
    "**[回复被中断]** 已达回合上限且未获续跑授权，本轮在此停下。";

/// 规划探索到上限：与 INTERRUPT_MARKER 系同风格（`**[标签]**` 开头），
/// 语义是"停下等用户决策"，模型**不该**续写。
const PLAN_LIMIT_MARKER: &str =
    "**[规划探索已到上限]** 规划探索已达到阈值，本轮自动停下，等待用户决策。";

/// 按中断类型取"写给模型看"的标记行。
///
/// 落库时正文是**干净**的（kind 独立成列）；发送给模型前由
/// `prepare_history_snapshot_from_messages` 按 kind 把标记拼回正文尾部 ——
/// 模型必须知道自己上一句被截断了，否则续跑会重复或断片。
/// 措辞按 kind 收敛（见 INTERRUPT_MARKER_RESUMABLE 注释：变体越少、
/// prompt cache 越稳）。
fn interrupt_marker_for(kind: InterruptKind) -> &'static str {
    match kind {
        // 能接着做的三种（流超时 / 用户取消 / 执行报错）共用"接着做"措辞
        InterruptKind::StreamTimeout
        | InterruptKind::UserCancel
        | InterruptKind::PipelineError => INTERRUPT_MARKER_RESUMABLE,
        // 应用关闭：**没有半截内容可续**，用恢复占位措辞（等用户下一条新意图）
        InterruptKind::AppClosed => {
            crate::core::orchestration::agent_runs::INTERRUPT_PLACEHOLDER_NO_REPLY
        }
        // 回合上限：停下别写（否则立刻再次触顶）
        InterruptKind::LoopLimit => INTERRUPT_MARKER_STOPPED,
        // 规划探索到上限：停下等用户决策
        InterruptKind::PlanLimit => PLAN_LIMIT_MARKER,
    }
}

/// 把中断标记追加到消息的**正文块**末尾（**只在发送给模型前调用**）。
///
/// 与存储侧的方向相反：存储时正文干净、kind 独立成列；这里是"拼回"。
/// 只动 Text 块——标记不得混进 Thinking（否则模型会把系统描述当成自己的
/// 思考内容，与存储侧旧实现的约定一致）。无 Text 块时补一个。
fn append_interrupt_marker_to_message(msg: &mut Message, marker: &str) {
    const SEP: &str = "\n\n";
    let content = match msg {
        Message::Assistant { content } => content,
        // 中断类型只打在 assistant 消息上；User 侧防御性跳过
        Message::User { .. } => return,
    };
    let append_to = |text: &mut String| {
        if text.trim().is_empty() {
            *text = marker.to_string();
        } else {
            text.push_str(SEP);
            text.push_str(marker);
        }
    };
    match content {
        Content::Single(text) => append_to(text),
        Content::Multiple(blocks) => {
            let last_text = blocks.iter_mut().rev().find_map(|b| match b {
                ContentBlock::Text { text } => Some(text),
                _ => None,
            });
            match last_text {
                Some(text) => append_to(text),
                None => blocks.push(ContentBlock::Text {
                    text: marker.to_string(),
                }),
            }
        }
    }
}

struct ContextEstimate {    total_chars: usize,
    estimated_tokens: usize,
    message_count: usize,
    tool_schema_count: usize,
    tool_call_count: usize,
    tool_result_count: usize,
    sections: Vec<ContextSectionSnapshot>,
}

const DIRECT_DEVELOPER_INTENT: &str = "PROJECT_ACTION";

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

/// 判断本 loop 的工具调用中是否提交了规划方案。
///
/// 两种等价形态都要认（喂狗判定必须同时覆盖，否则提交方案的那轮会被当成
/// 空转、看门狗误触发把收尾截胡——B 修复诊断出的真实事故路径）：
/// - 裸调用：工具名就是 `ProposePlan`；
/// - 延迟工具包装：工具名是 `ExecuteTool`，参数 JSON 的 `name` 字段指向
///   真实工具（延迟工具的统一执行入口，模型提交方案固定走这条形态）。
fn loop_submitted_plan(tool_calls: &[(String, String)]) -> bool {
    tool_calls.iter().any(|(name, input)| {
        if name == "ProposePlan" {
            return true;
        }
        if name == "ExecuteTool" {
            if let Ok(value) = serde_json::from_str::<serde_json::Value>(input) {
                return value.get("name").and_then(|n| n.as_str()) == Some("ProposePlan");
            }
        }
        false
    })
}

/// 判断给定的助手文本是否已存在于消息列表尾部。
///
/// 中断收尾时用于去重：流式内容可能已由 `store_assistant_response()` 落库，
/// 避免把同一段半截话写两遍。只检查尾部若干条，因为中断内容只可能出现在最近的位置。
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

/// 中断收尾共用落库：把当轮已生成的内容（思考 + 正文）与中断提示合并成
/// **一条**助手消息，保持一问一答。
///
/// ## 设计口径（2026-09-19 沐定案，丙方案）
///
/// 目标是**保持一问一答的对话结构**：中断不额外新增助手占位消息，
/// 而是把"半截内容 + 中断提示"收敛进同一条消息里。
///
/// 为什么提示必须落在**正文块**末尾而不是单独成条：
/// 前端渲染助手气泡时以正文块为锚 —— 若思考块有内容而正文块为空，
/// 界面会从**上一条**助手消息处取文渲染，用户看到的内容顺序会割裂。
/// 因此提示恒挂在正文块末尾；正文为空（纯思考期中止）时，
/// **把提示本身作为正文块**，保证渲染锚点存在。
///
/// ## 两个写入分支
///
/// - **尾部已有本轮的助手消息**（主循环 `store_assistant_response` 已落当轮，
///   这是最常见的情况）→ **原地追加提示**到该消息的正文块，不新增消息。
/// - **尾部没有**（如思考期中止、内容尚未落库）→ 新建一条合并消息。
///
/// 幂等：正文块尾部已含 `marker` 时直接返回，重复收尾不会写两遍。
///
/// 思考内容按 `Thinking` 块落库（空签名），与 `abort_after_error` 的
/// 既有先例一致：思考按折叠样式展示、不冒充正文，空签名出网由 adapters 剥离。
///
/// 返回 true 表示本次确实改动了会话历史。
///
/// ## 阶段二改造（2026-09-20）：标记不再拼进正文
///
/// 此前把 `**[回复被中断]** …` 直接 `format!` 拼在正文块尾部，界面侧只能靠
/// 正则从文本里"猜着剥"。现在改为把**中断类型**写进 `interrupt_kinds`
/// （与 messages/sources 平行的数组，随消息落库），正文保持干净：
/// - 模型侧：发送前由 `prepare_history_snapshot_from_messages` 按 kind 拼回标记；
/// - 界面侧：`command/history.rs` 读 kind 生成 notice 小字。
///
/// **分支一的 `source` 保持 `chat` 不变**——那条消息是模型的真实回复，
/// 在发给模型的白名单里，改 source 会让它掉出模型上下文（阶段二的硬约束）。
async fn store_interrupted_turn(
    session: &mut crate::infra::types::models::SessionMemory,
    text: &str,
    thinking: &str,
    kind: InterruptKind,
) -> bool {
    let text = text.trim();
    let thinking = thinking.trim();
    let kind_str = kind.as_str();

    // 分支一：尾部已有本轮的助手消息（`source="chat"`，主循环已落当轮）
    // → 就地为它打上中断类型，不新增消息。
    //
    // 幂等：该消息已带 kind 时直接返回，重复收尾不会写两遍。
    if !text.is_empty() {
        if let Some(idx) = tail_assistant_text_index(&session.messages, text) {
            normalize_message_ids(session);
            if session.interrupt_kinds.get(idx).cloned().flatten().is_some() {
                return false;
            }
            if idx < session.interrupt_kinds.len() {
                session.interrupt_kinds[idx] = Some(kind_str.to_string());
            }
            return true;
        }
    }

    // 分支二：尾部没有（思考期中止、内容尚未落库）→ 新建一条消息。
    // 正文即半截内容；**正文块恒存在**（渲染锚点，前端以正文块定位气泡）——
    // 无产出时留空块，界面小字由 kind 驱动的 notice 承担。
    let mut blocks: Vec<ContentBlock> = Vec::new();
    if !thinking.is_empty() {
        blocks.push(ContentBlock::Thinking {
            thinking: thinking.to_string(),
            signature: String::new(),
        });
    }
    blocks.push(ContentBlock::Text {
        text: text.to_string(),
    });

    append_message_with_kind(
        session,
        Message::Assistant {
            content: Content::Multiple(blocks),
        },
        "interrupted",
        Some(kind_str),
    );
    true
}

/// 在消息列表**尾部**查找正文与 `target` 一致的助手消息，返回其下标。
///
/// 只扫尾部若干条：中断轮的助手消息只可能出现在最近的位置。
/// 用于中断收尾时判断"当轮是否已被 `store_assistant_response` 落库"——
/// 命中则原地追加提示，避免再补一条助手消息。
///
/// **匹配必须容忍已追加过提示的正文**：提示是追加到正文块**末尾**的，
/// 一旦追加成功，正文就变成 `原文 + 分隔 + 提示`，此时再用全等比对会失配，
/// 导致重复收尾时错判为"尾部没有当轮内容"而新建第二条消息（破坏一问一答）。
/// 故匹配口径为：**正文以 `target` 开头**即可（追加提示只改尾部，不改前缀）。
fn tail_assistant_text_index(messages: &[Message], target: &str) -> Option<usize> {
    let target = target.trim();
    if target.is_empty() {
        return None;
    }
    messages
        .iter()
        .enumerate()
        .rev()
        .take(4)
        .find(|(_, msg)| match msg {
            Message::Assistant { content } => match content {
                Content::Single(text) => text.trim_start().starts_with(target),
                Content::Multiple(blocks) => blocks.iter().any(|b| match b {
                    ContentBlock::Text { text } => text.trim_start().starts_with(target),
                    _ => false,
                }),
            },
            _ => false,
        })
        .map(|(idx, _)| idx)
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

impl PipelineState {
    /// **首次固化**会话的深度思考档位。
    ///
    /// ## 为什么需要它
    ///
    /// 新建会话时（`create_session`）还不知道本次要用哪个模型，因此拿不到
    /// `thinking_forced`，无法解析"跟随全局"的默认档位，只能先写占位 `false`。
    /// 真正可解析的时点是**第一条消息**——此时 `model_id` 已确定。
    ///
    /// ## 判定"是否需要固化"
    ///
    /// 用 `ctx` 里是否已有"用户表态"来区分。但 `ctx.thinking_mode` 只是布尔，
    /// 无法区分"占位 false"与"用户主动选的 false"。因此这里改用**另一条事实**：
    /// 会话行是否已被固化过。
    ///
    /// 为免再加一列，采用更简单的口径：**只有"本会话从未跑过主 Agent"时才固化**，
    /// 判据是 `ctx.turn_think` 尚未被写过（`None`）。跑过一轮之后，用户若拨了开关，
    /// 值会由 `set_session_thinking_enabled` 命令直接写入，不会再进这里。
    ///
    /// 这样"用户拨过 → 用用户的值"与"用户没拨 → 用设置默认"两条路径不会互相覆盖。
    async fn ensure_session_thinking_initialized(
        app: &tauri::AppHandle,
        session_id: &str,
        ctx: &std::sync::Arc<crate::infra::state::state::SessionContext>,
        thinking_forced: bool,
    ) {
        // 已经跑过主 Agent：说明值已固化（或用户已表态），不再改动
        if ctx.turn_think.lock().await.is_some() {
            return;
        }

        // 读设置里的默认档位（L1），解析为确定布尔
        let default_raw = crate::command::app_config::read_file()
            .ui_preferences
            .thinking_default;
        let resolved = crate::core::session::thinking::ThinkingDefault::parse(&default_raw)
            .resolve(thinking_forced);

        // 写内存态 + 落库（**只在这里写一次**，此后设置改动不再影响本会话）
        *ctx.thinking_mode.lock().await = resolved;
        if let Err(e) = crate::core::session::update_session_thinking_mode(session_id, resolved) {
            // 落库失败不阻断本轮：内存值仍然生效，下轮会重试固化
            eprintln!("[JARVIS] 固化会话思考档位失败（不阻断本轮）: {}", e);
            return;
        }
        // 通知前端刷新（监控窗口 / 主窗口的开关都需要跟着显示）
        let _ = app.emit(
            "session-thinking-mode-changed",
            serde_json::json!({
                "sessionId": session_id,
                "thinkingEnabled": resolved,
            }),
        );
    }

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

        // 步骤 3.5：沙箱目录存在性校验。
        // 会话绑定的项目目录可能事后被删除/移动：目录没了还继续跑，
        // 校验层按沙箱 join 放行、执行层却找不到文件，模型会在错误目录里打转。
        if let Some(ws) = &request_workspace {
            if !ws.exists() {
                *ctx.cancel_token.lock().await = None;
                return Err(AgentError::Session(format!(
                    "会话绑定的项目目录已不存在: {}（可能已被删除或移动）。请重建会话并重新挂载项目后再试。",
                    ws.display()
                )));
            }
        }

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

        // 步骤 6：工作模式 / 权限档位 / 用户类型以会话上下文为准。
        //
        // 三者已是会话级属性（sessions 表落库，get_or_create 恢复：落库值，否则设置默认），
        // pipeline 不再做任何"按偏好覆盖"——否则会把用户在本会话内的切换/落库值冲掉
        // （历史包袱：这里曾按 has_history 用偏好覆盖模式与档位、每轮覆盖受众；
        // 设置里的默认现在只决定"新会话的初始值"这一个用途）。
        let current_work_mode = ctx.agent_work_mode.lock().await.clone();
        let current_audience = ctx.agent_audience.lock().await.clone();
        // system 提示词：**会话级缓存，get_or_assemble**。
        // 提示词磁盘化后（data/prompts/ 用户可覆盖），如果不缓存，用户改了 md 文件
        // 后同一会话的下一个 turn 会重组 system → 缓存前缀整体失效。
        // 缓存住 = 会话内字节恒定从"约定"升级为"代码保证"，改动只对新会话生效。
        let system_prompt = {
            let mut cache = ctx.system_prompt_cache.lock().await;
            if cache.is_none() {
                *cache = Some(crate::core::agent::prompts::get_system_prompt(
                    &current_audience,
                    &current_work_mode,
                    request_workspace.as_deref(),
                ));
            }
            cache.clone().expect("just populated")
        };

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
                println!("[JARVIS] {} 模式：规则检测到复杂任务，首轮直接进入方案审批流程", current_work_mode);
                "TASK_PLAN".to_string()
            } else {
                println!("[JARVIS] {} 模式：直接进入项目操作流程", current_work_mode);
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
                        drop(mode);
                        // 模式是会话级属性：同步落库（sessions.work_mode），随会话恢复
                        if let Err(e) =
                            crate::core::session::update_session_work_mode(&session_id, "edit")
                        {
                            eprintln!("[JARVIS] 工作模式落库失败（会话 {}）：{}", session_id, e);
                        }
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

        // 步骤 8.5：解析深度思考档位（v14 起只有 L2 会话布尔，不再有 L1 现读）
        //
        // 关键变化：**设置默认值（L1）不在这里参与裁决**。
        // 它只在"会话还没有值"时被解析一次并落库（见下），此后这个会话就只认库里的布尔。
        // 这样设置页改默认档位**永远不会倒灌已有会话**——正是本次重构要修的问题。
        //
        // 落点说明：新建会话时 `create_session` 只能写占位 `false`（那时还不知道主模型），
        // 所以真正的初值必须在**第一条消息**、拿到 `model_id` 之后解析。
        {
            let caps = crate::infra::llm::registry::query_capabilities(&model_id);
            let thinking_forced = caps.as_ref().map(|c| c.thinking_forced).unwrap_or(false);
            Self::ensure_session_thinking_initialized(&app, &sid, &ctx, thinking_forced).await;
        }
        let session_think_mode =
            crate::core::session::thinking::ThinkingMode(*ctx.thinking_mode.lock().await);

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
            detected_intent: detected_intent.clone(),
            capabilities,
            // 以下字段在后续阶段填充
            dynamic_context_str: String::new(),
                        user_msg_preview: String::new(),
            initial_msg_index: 0,
            user_message_id: None,
            should_think: false,
            turn_think: false,
            turn_thinking_decision: crate::core::session::thinking::ThinkingDecision::default(),
            run_id: String::new(),
            loop_count: 0,
            total_loop_count: 0,
            req_input_tokens: 0,
            req_output_tokens: 0,
            req_sub_input_tokens: 0,
            req_sub_output_tokens: 0,
            final_answer: String::new(),
            reflection_mode: resolved_reflection_mode,
            total_reflections: 0,
            consecutive_reflection_nos: 0,
            plan_consecutive_stalls: 0,
            plan_total_loops_without_plan: 0,
            interrupted_reason: None,
            notice: None,
            current_blocks: Vec::new(),
            turn_text_this_turn: String::new(),
            turn_thinking_this_turn: String::new(),
            // 崩溃保护是纯全局设置：读一次当轮快照，中途改设置不影响进行中的轮次
            crash_protection: crate::command::app_config::read_file()
                .ui_preferences
                .crash_protection,
            display_msg,
            tool_execution_summary: None,
        };

        // 步骤 10：TASK_PLAN 前置拦截 —— 复杂任务首轮强制切到 plan 模式并广播
        if detected_intent == "TASK_PLAN" && current_work_mode != "plan" {
            println!("[JARVIS] 意图前置拦截：TASK_PLAN 意图，首轮强制切换到 Plan 模式");
            *state.ctx.agent_work_mode.lock().await = "plan".to_string();
            // 模式是会话级属性：同步落库（sessions.work_mode），随会话恢复
            if let Err(e) =
                crate::core::session::update_session_work_mode(&state.sid, "plan")
            {
                eprintln!("[JARVIS] 工作模式落库失败（会话 {}）：{}", state.sid, e);
            }
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

    /// 阶段 2：上下文构建 + 消息注入 + Agent Run 启动
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
        // 注意：active_run_id 在拿 memory 锁**之前**读取（统一锁顺序，避免交叉持有）
        let active_run_id_before_recovery = self.ctx.active_run_id.lock().await.clone();
        let memory_after_user_message = {
            let mut session = self.ctx.memory.lock().await;
            // 3a. 崩溃恢复：上次 run 异常中断时，把残留消息恢复进内存。
            // 走唯一闸门 `ensure_session_recovered`（原子抢占锁 + 闸门内统一落库/盖章），
            // 本入口不再自行 save/mark。
            let recovered = crate::command::session::ensure_session_recovered(
                &self.sid,
                &mut session,
                active_run_id_before_recovery.as_deref(),
            );
            if recovered {
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
            let (initial_msg_index, user_message_id) = inject_user_message(
                &mut session,
                self.display_msg.as_deref().unwrap_or(&self.msg),
                &self.image_base64_list,
                &self.dynamic_context_str,
                &mut active_sid,
            );
            self.initial_msg_index = initial_msg_index;
            self.user_message_id = Some(user_message_id);
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
            model_caps.as_ref(),
        );
        self.turn_thinking_decision = decision.clone();
        let turn_think = decision.enabled;
        self.turn_think = turn_think;
        self.should_think = turn_think;
        // 供子 Agent 继承（设计文档 K3）：主 Agent 本轮用什么档位，本轮派生的子 Agent 就用什么
        *self.ctx.turn_think.lock().await = Some(turn_think);
        println!(
            "[JARVIS] 本轮 thinking 状态固定为 {}（override={:?}, 会话档位={}, 裁决={:?}）",
            if turn_think { "enabled" } else { "disabled" },
            override_val,
            self.session_think_mode.0,
            decision.reason
        );

        let user_message_id = {
            let session = self.ctx.memory.lock().await;
            session.message_ids.get(self.initial_msg_index).cloned()
        };
        println!("[JARVIS] start_run: message_id={:?} initial_msg_index={}", user_message_id, self.initial_msg_index);
        // 步骤 6：在 agent_runs 表登记本次 run（实时进度 + 崩溃恢复用）
        // （不再传用户消息文本：它只曾用于 v19 删掉的 user_message_preview 列）
        self.run_id = agent_runs::start_run(&self.app, &self.sid, None, user_message_id);
        // 步骤 7：标记当前 run 为活跃。
        //
        // v15 起这里**不再**落 checkpoint：崩溃重建的唯一数据源是「每轮一行」的
        // `agent_run_events`，而本轮还没有任何 loop 走完，落一条空 checkpoint
        // 只会在恢复时重建出空内容（→ NeedsClosure 收口），没有信息增量。
        *self.ctx.active_run_id.lock().await = Some(self.run_id.clone());
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

    /// 阶段 3：主循环 — Agent Loop 心脏（调 LLM → 流式解析 → 工具执行 → 循环）
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
                    self.append_interrupted_marker(InterruptKind::LoopLimit)
                        .await;
                    self.interrupted_reason = Some("达到回合上限且未获续跑授权".to_string());
                    break;
                }
            }

            // 后台任务结果不再注入会话上下文：失败/完成信息走前端界面提醒
            // （bg-task-done / background-failed 事件 → 聊天流小字），用户看到后
            // 主动发消息让 Agent 排查（排查用 CheckBackgroundCommand）。
            // 这样也消除了 background 注入对（User+Assistant 两条）破坏
            // 消息级 user/assistant 交替的问题。

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
            let (req_json, req_api_format) = self.build_llm_request(history_snapshot);

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
                    req_api_format,
                    &self.app,
                    &self.sid,
                    &self.run_id,
                    self.total_loop_count + 1,
                    &self.cancel_token,
                    StreamConfig {
                        is_subagent: false,
                        cache_usage_style: cache_usage_style.clone(),
                        on_frame: frame_tick.clone(),
                        model_id: Some(self.model_id.clone()),
                        crash_protection: self.crash_protection,
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
                                    req_api_format,
                                    &self.app,
                                    &self.sid,
                                    &self.run_id,
                                    self.total_loop_count + 1,
                                    &self.cancel_token,
                                    StreamConfig {
                                        is_subagent: false,
                                        cache_usage_style,
                                        on_frame: frame_tick.clone(),
                                        model_id: Some(self.model_id.clone()),
                                        crash_protection: self.crash_protection,
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

            // 中断收尾要读的结构化现场（取消 / 报错可能发生在本 loop 的任意后续位置）
            self.current_blocks = current_blocks.clone();
            self.turn_text_this_turn = current_text_this_turn.clone();
            self.turn_thinking_this_turn = current_thinking_this_turn.clone();

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

            // 检测输出截断状态。
            //
            // 这里**只留日志，不再向 ToolResult 注入提示**：截断的处置建议已由
            // `infra::llm::adapters::ToolInputErrorKind::Truncated::advice()` 承担
            // （见 tools_runner.rs 生成 failure 处）。两处都写会在同一个工具结果里
            // 出现两条内容重叠的建议；且本处原文案写死了"每次只创建 1-2 个文件"，
            // 源自写文件场景，对 ProposePlan 这类调用并不贴切。
            // 分类 advice 的依据是 serde 对参数 JSON 的真实报错，比 `stop_reason` 更准。
            let is_truncated = matches!(
                stream_result.stop_reason.as_deref(),
                Some("max_tokens") | Some("length")
            );
            if is_truncated {
                println!(
                    "[JARVIS] 检测到输出截断 (stop_reason={:?})；截断提示由工具参数错误分类统一给出",
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
            // 单独记一份，供回合收尾补一次累加（主循环的用量已在逐请求路径落库）
            self.req_sub_input_tokens += sub_in;
            self.req_sub_output_tokens += sub_out;

            // （原「截断感知注入」已移除：截断的处置建议改由工具参数错误分类统一给出，
            //   见上方 `is_truncated` 处注释与 `ToolInputErrorKind::Truncated::advice()`。
            //   保留 `is_truncated` 仅用于日志诊断。）

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
            //
            // 取消且无工具结果时跳过（2026-09-19 双写修复）：此时本轮半截内容
            // （可能只有 thinking，正文还没开始）由 handle_cancellation 的
            // store_interrupted_turn 统一收口成一条合并消息。若这里照常写入，
            // 半截内容会先落一条 src=chat 的消息；随后 store_interrupted_turn 的
            // 分支一去重只匹配 Text 块、匹配不到 thinking-only 消息，会再建一条
            // src=interrupted —— 同一段内容落库两遍（实测 seq 23/24 重复）。
            // 工具轮不受影响：tool_results 非空时仍照常写入保持配对完整。
            if !(was_cancelled_this_loop && tool_results.is_empty()) {
                self.store_assistant_response(&current_blocks).await;
            }

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
                            "content": "\n> **[方案重定向]** 检测到计划性内容，正在重定向到方案审批流程。\n",
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

                    // ⚠️ 关键修复（2026-09-19）：看门狗必须在这条「无工具 + 计划正文」
                    // 路径上也被评估。旧实现只挂在工具执行分支，而拦截死循环走的正是
                    // 无工具分支 —— 计数器永远停在 0，看门狗结构性失效，只能靠用户手点
                    // 取消脱困。
                    if self.update_plan_watchdog(&work_mode, &[]) {
                        println!(
                            "[JARVIS] Plan 看门狗触发（拦截路径）：consecutive={}, loops_without_plan={}",
                            self.plan_consecutive_stalls, self.plan_total_loops_without_plan
                        );
                        self.handle_plan_watchdog_summary().await;
                        break;
                    }

                    self.loop_count += 1;
                    self.total_loop_count += 1;
                    continue;
                }

                // 本轮（无工具 = 最终回复）整轮 fallback 落库。
                //
                // ⚠️ 正常路径不靠这里：步骤 9 的 `store_assistant_response` 之后，
                // 紧跟的 `record_loop_event` 会把 resp（含 thinking/tool_use）与
                // tool_results 一起结构化写入 `agent_run_events`。这条 **continue 路径**
                // 绕过了那两处（它不落 session，也不走下面的工具分支），若不补写，
                // "拦截重定向"这类轮次就会在崩溃重建时凭空消失。
                //
                // thinking 从 `current_thinking_this_turn` 补回：它不是 `current_blocks`
                // 的可靠来源（流层对 thinking 的落块与文本累积是两条独立路径），
                // 而这里需要的是"这一轮模型说过什么"的完整快照。
                let resp_for_event = take_resp_blocks_with_thinking(
                    &current_blocks,
                    &mut current_thinking_this_turn,
                );
                self.record_loop_event(&resp_for_event, &[]).await;

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

                // —— 模式切换快照：本 loop 的 SwitchWorkMode 真实变更了 work_mode ——
                // 不修改已发出的 <context_snapshot>，而是生成一条 seq 更大的完整新快照，
                // 并入本条 tool_result 消息的块数组尾部（不独立成条）：
                // - 历史前缀仍逐字节命中缓存（该消息本轮新产生、从未入缓存前缀）
                // - system 的“最新快照优先”规则随即切到新模式现场
                // - session_messages 保持消息级 user/assistant 严格交替（数据结构对称）
                // 出网时 Context 块由 adapters 逐块 materialize 成 Text，与块位置无关；
                // 前端 user_display_content 对 Context 块本就跳过，不影响界面渲染。
                let mode_after_execution = { self.ctx.agent_work_mode.lock().await.clone() };
                let mode_switched = mode_after_execution != work_mode;
                if mode_switched {
                    println!(
                        "[JARVIS] 工作模式在本 loop 内切换：{} -> {}，新快照并入工具结果消息",
                        work_mode, mode_after_execution
                    );
                    self.merge_mode_snapshot_into_blocks(&mut tool_results, &mode_after_execution)
                        .await;
                }

                // 添加工具结果（模式切换时末块为新快照）到 session
                {
                    let mut session = self.ctx.memory.lock().await;
                    append_message(&mut session, Message::User {
                        content: Content::Multiple(tool_results.clone()),
                    }, "chat");
                    // 模式切换当轮立即落库：保证崩溃恢复时 snapshot_seq 与新快照同时存在，避免 seq 回退/重复
                    if mode_switched {
                        crate::core::session::save_session(&self.sid, &session, None);
                    }
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
                agent_runs::upsert_loop_event(
                    &self.run_id,
                    &self.sid,
                    self.total_loop_count + 1,
                    current_blocks.clone(),
                    tool_results.clone(),
                    "complete",
                    None,
                    turn_in_tokens,
                    turn_out_tokens,
                    Some(self.model_id.clone()),
                );
                drop(session);

                // B3 Plan 看门狗：仅 plan 模式；连续无喂狗的工具调用 / 累计无 ProposePlan 的空转达到阈值时，
                // 先做一次缓存友好的 LLM 进度小结，再强制停下交还决策权。
                // 传 tool_calls（含参数 JSON）而非纯名字列表：喂狗判定要解包 ExecuteTool 包装。
                let current_mode_after = self.ctx.agent_work_mode.lock().await.clone();
                if self.update_plan_watchdog(&current_mode_after, &tool_calls) {
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

    /// 阶段 4：收尾 — 持久化 + 快照 + 记忆 + 结果组装
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

        // 3. 保存会话到 SQLite；纯聊天且被取消时不落空会话
        //
        // 收尾这里**不再交整轮 token**：主循环的每个 loop 已在
        // `update_provider_usage_snapshot` 里逐请求累加过，再交一次就是重复计数。
        // 只剩子代理那一份要补——它走的是 `subagent.rs` 自己的循环，不经过逐请求路径。
        //
        // 取消的回合也照交：那部分 token 确实花掉了，旧实现在这里传 `None`
        // 等于把子代理用量一并丢掉。
        let memory = self.ctx.memory.lock().await.clone();

        let session_meta = if memory.messages.is_empty() && was_cancelled {
            None
        } else {
            let meta = crate::core::session::save_session(
                &self.sid,
                &memory,
                Some(crate::core::session::TokenUsageDelta {
                    input: self.req_sub_input_tokens,
                    output: self.req_sub_output_tokens,
                    // 子代理的缓存用量未被采集，保持 0（展示层把 0 读作"未报告"）
                    cache_hit: 0,
                    cache_miss: 0,
                }),
            );
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
        } else if interrupted_reason.is_some() {
            // 方案 6：主动收尾（取消/错误中止/看门狗）已在此前把 run 置为
            // Interrupted/Cancelled，且上方 save_session 已把半截内容与中断
            // 标记落库——收尾至此**声明完成**，盖 closed 图章把 run 移出
            // 恢复候选（门卫 find_interrupted_run 不捞 closed）。此前这类
            // run 停留 Interrupted，收尾后的正常刷新就会被门卫重放比对，
            // 因标记只在消息层而误判缺失、重复补同一段（2302e81c）。
            // 若 save 与盖章之间崩溃（毫秒级窗口），run 停留 Interrupted
            // 被门卫捞出，由 messages_equivalent 的剥标记比对兜底。
            agent_runs::mark_run_closed(&self.app, &self.run_id);
        }

        let session_input_tokens = session_meta
            .as_ref()
            .map(|meta| meta.total_input_tokens)
            .unwrap_or(0);
        let session_output_tokens = session_meta
            .as_ref()
            .map(|meta| meta.total_output_tokens)
            .unwrap_or(0);
        // 会话级缓存累计：0 表示该会话从未有请求上报过缓存字段（展示层显示 --）
        let session_cache_hit_tokens = session_meta
            .as_ref()
            .map(|meta| meta.total_cache_hit_tokens)
            .unwrap_or(0);
        let session_cache_miss_tokens = session_meta
            .as_ref()
            .map(|meta| meta.total_cache_miss_tokens)
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
            session_cache_hit_tokens,
            session_cache_miss_tokens,
            user_message_id: self.user_message_id.clone(),
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
    /// 现改为：把**本轮的结构化响应块**（`current_blocks`）连同中断提示
    /// **合并成一条**助手消息落库（见 `store_interrupted_turn` 的丙方案说明），保持一问一答。
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
        let notice_text = format!("本轮执行中断：{}", error);
        println!("[JARVIS] 异常收尾（保留现场）: {}", error);

        // 1. 把已流式输出但尚未入库的内容补进历史。
        //
        // v15 起这里改读**内存中的结构化块**（`PipelineState.current_blocks`），
        // 不再是 `agent_runs.live_content / live_thinking` 那对字符串。理由：
        //
        // - 旧实现是"每帧把正文追加进一段大字符串"，思考与正文挤在两条独立的
        //   累积流里，中断时按字符串顺序拼回去 → 思考必然落在正文之前/之后某一侧，
        //   **与块的真实交错顺序脱节**（这正是方案 §3.1 记的"正文糅合"）。
        // - 结构化块从流层出来就是 `Vec<ContentBlock>`（Text / Thinking / ToolUse
        //   各占一块，顺序即产生顺序），既不用猜顺序，也不会把工具调用当成正文。
        //
        // 兜底：`current_blocks` 为空（如尚未收到任何帧就被取消）而 turn 级字符串
        // 有内容时，用字符串重建，避免"什么都没有"时把内容丢掉。
        let current_blocks = self.current_blocks.clone();
        let (thinking, text) = if current_blocks.is_empty() {
            (
                self.turn_thinking_this_turn.trim().to_string(),
                self.turn_text_this_turn.trim().to_string(),
            )
        } else {
            let (t, k) = blocks_to_text_and_thinking(&current_blocks);
            (k, t)
        };

        // 落库口径（丙方案）：半截内容 + 中断提示合并成**一条**助手消息。
        // 错误文本本身**不作为**助手消息内容（避免被当成模型发言污染上下文），
        // 只由统一的 INTERRUPT_MARKER_RESUMABLE 提示"该接着做"。
        {
            let mut session = self.ctx.memory.lock().await;
            store_interrupted_turn(
                &mut session,
                &text,
                &thinking,
                InterruptKind::PipelineError,
            )
            .await;
        }

        self.final_answer = text;
        self.notice = Some(notice_text.clone());
        self.interrupted_reason = Some(error.to_string());

        // 3. 落库：本轮标记为 interrupted（**保留**帧级通道已写的半截内容）
        self.mark_loop_event_interrupted(error.to_string()).await;
        // run 级中断类型与消息级一致（上面 store_interrupted_turn 用的就是 PipelineError）
        agent_runs::interrupt_run(
            &self.app,
            &self.run_id,
            InterruptKind::PipelineError,
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
    /// 阶段二起正文**不再放标记文本**（留空块作渲染锚点），中断类型走
    /// `interrupt_kinds`；标记在发送给模型前由 `interrupt_marker_for` 按 kind 拼回。
    ///
    /// 返回该消息在会话历史中的下标。
    async fn append_interrupted_marker(&mut self, kind: InterruptKind) -> usize {
        let mut session = self.ctx.memory.lock().await;
        append_message_with_kind(
            &mut session,
            Message::Assistant {
                content: Content::Multiple(vec![ContentBlock::Text {
                    text: String::new(),
                }]),
            },
            "interrupted",
            Some(kind.as_str()),
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

        let hint_secs = crate::core::agent::stream::waiting_hint_secs();
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
                    let silent_secs = silent_ms / 1000;
                    // 剩余秒数按当前生效的终止阈值算。
                    // ⚠️ 这里用 `STREAM_IDLE_TIMEOUT_SECS` 是**准确**的：等待响应头
                    // 阶段由 `API_RESPONSE_HEADER_TIMEOUT_SECS` 收尾，两者当前同为
                    // 120s，故两个阶段的剩余时间算法一致。**若将来把这两个阈值改成
                    // 不相等**，需要按阶段把各自的阈值传进来，不能再用同一个常量。
                    let remain_secs = crate::core::agent::stream::STREAM_IDLE_TIMEOUT_SECS
                        .saturating_sub(silent_secs);
                    let _ = app.emit(
                        "agent-step",
                        json!({
                            "type": "waiting_hint",
                            "content": format!(
                                "⏳ 已等待 {} 秒未收到数据；若持续无响应，约 {} 秒后会自动终止（无需手动停止）。深度思考或上游繁忙时可能较慢。",
                                silent_secs, remain_secs
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

    /// 把本 loop 的完整轮次打上 `interrupted` 标记。
    ///
    /// 与 [`Self::record_loop_event`] 共用同一把尺子（同一个 `loop_index`、同一套
    /// resp/tool 块），只差 `status` 与 `error`——中断轮同样要有 events 行，
    /// 否则崩溃重建时"进行到一半就被打断的那一轮"会整轮消失。
    ///
    /// 之所以要**标记而不是删除**：删除等于宣称"这轮没发生过"，而它的工具可能
    /// 已经改了文件；标记为 interrupted 才能让重建如实还原现场。
    ///
    /// 轮号与收尾写入同取 `total_loop_count + 1`（见 [`Self::record_loop_event`] 的
    /// 取号说明）。`total_loop_count == 0` 时**不再提前返回**：帧级通道可能已经
    /// 建了第 1 行（streaming），漏标会让崩溃重建把它当有效半截内容重放；
    /// UPDATE 0 行本就无害，交给 SQL 即可。
    async fn mark_loop_event_interrupted(&self, error: String) {
        if self.run_id.is_empty() {
            return;
        }
        agent_runs::mark_loop_event_interrupted(
            &self.run_id,
            &self.sid,
            self.total_loop_count + 1,
            error,
        );
    }

    /// 把本 loop 的响应与工具结果结构化写入 `agent_run_events`（崩溃重建的唯一数据源）。
    ///
    /// ## 取号说明（⚠️ 曾因注释错误取错轮号）
    ///
    /// `total_loop_count` 在**每轮结束时**才自增（主循环各 continue/break 分支），
    /// `process_stream` 收到的帧级轮号是 `total_loop_count + 1`。收尾覆盖必须用
    /// **同一个号**，才能落回帧级通道建的那一行——此处曾误用未自增的
    /// `total_loop_count`，把本轮内容覆盖到**上一轮**的 events 行上
    /// （上一轮 tool_results 丢失、取消标记错标上一轮）。
    async fn record_loop_event(&self, resp_blocks: &[ContentBlock], tool_results: &[ContentBlock]) {
        agent_runs::upsert_loop_event(
            &self.run_id,
            &self.sid,
            self.total_loop_count + 1,
            resp_blocks.to_vec(),
            tool_results.to_vec(),
            "complete",
            None,
            self.req_input_tokens,
            self.req_output_tokens,
            Some(self.model_id.clone()),
        );
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

        // 取回已流式输出的部分结果。
        //
        // v15 起改读**内存中的结构化块**（理由见 `abort_after_error`）：
        // 块顺序即真实产生顺序，思考/正文/工具调用各归其位，不再靠字符串拼接猜。
        // 兜底与 `abort_after_error` 一致：块为空时回落到 turn 级字符串。
        let (partial, partial_thinking) = {
            let blocks = self.current_blocks.clone();
            if blocks.is_empty() {
                let t = self.turn_text_this_turn.trim().to_string();
                let k = self.turn_thinking_this_turn.trim().to_string();
                (
                    if !t.is_empty() {
                        t
                    } else if !self.final_answer.is_empty()
                        && self.final_answer != "用户已取消执行。"
                    {
                        std::mem::take(&mut self.final_answer)
                    } else {
                        String::new()
                    },
                    k,
                )
            } else {
                let (t, k) = blocks_to_text_and_thinking(&blocks);
                (t, k)
            }
        };

        // `reason` 面向用户（走 notice 结构化字段，纯文本、不经 Markdown 渲染，
        // 故不带引用符/加粗/emoji —— 见 doc/状态标注符号统一与结构化改造方案.md）；
        // 写入历史的标记面向 LLM（会进上下文），故用最小信息量的统一措辞，
        // 避免模型把系统视角描述当成自己的话。
        let reason = "用户已取消执行，以上为保留的部分结果，历史未截断。";

        // 落库口径（丙方案）：半截内容 + 中断提示合并成**一条**助手消息，
        // 保持一问一答；正文为空时把提示本身作为正文块，保证前端渲染锚点存在。
        {
            let mut session = self.ctx.memory.lock().await;
            store_interrupted_turn(
                &mut session,
                &partial,
                &partial_thinking,
                InterruptKind::UserCancel,
            )
            .await;
        }

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
        // 标记 run 为 CANCELLED，并向前端发送取消通知。
        // 轮次事件同样打上 interrupted（保留帧级通道已写的半截内容）。
        self.mark_loop_event_interrupted("用户取消".to_string()).await;
        agent_runs::cancel_run(
            &self.app,
            &self.run_id,
            self.req_input_tokens,
            self.req_output_tokens,
            Some(self.final_answer.clone()),
        );
        println!("[JARVIS] 已把中断轮内容与提示合并落库（interrupted）");
    }

    /// 上游失联（流内空闲超时）收尾：保留现场，结束本轮，把决策权交还用户。
    ///
    /// 流层已采用「优雅终止」——`process_stream` 返回已累积的部分结果而非抛错，
    /// 因此这里不需要（也不应该）走 `fail_run` 的抹除路径。服务恢复后用户
    /// 直接说"继续"即可接上，因为历史完整且标记对模型可见。
    async fn handle_stream_idle_timeout(&mut self, stream_result: crate::core::agent::StreamResult) {
        let partial = stream_result.text.trim().to_string();
        println!(
            "[JARVIS] 上游超时收尾：已累积文本 {} 字，工具={}，loop={}",
            partial.chars().count(),
            stream_result.has_tool,
            self.total_loop_count + 1
        );

        // 本轮的完整响应块（含 thinking / tool_use）——断流时流层已优雅返回，
        // 这里把它整轮留档，供崩溃重建原样还原（工具调用不能丢：它可能已经改了文件）。
        let mut thinking_for_event = stream_result.thinking.clone();
        let resp_blocks_for_event =
            take_resp_blocks_with_thinking(&stream_result.blocks, &mut thinking_for_event);

        // 面向用户的文案：会渲染成气泡下方的小字（notice），
        // 因此不用 Markdown 引用符号 —— 小字是纯文本，`>` 会原样显示。
        //
        // ⚠️ 归因范围（勿再混淆）：本函数是**流空闲超时**（SSE 30s 零帧）的收尾，
        // 与 plan 模式的「规划看门狗」（update_plan_watchdog / 阈值 6 次·10 轮）
        // 是两套完全不同的机制，只有本函数走这里。文案只描述"等待"这个客观事实，
        // **不得断言服务端状态**（"已停止响应/已失联"都不行）——静默可能是服务
        // 繁忙、链路抖动或半开连接，在链路上不可区分，断言会误导用户。
        let reason = format!(
            "上游长时间未返回数据（已等待 {} 秒），本轮已自动终止。\
             已保留的部分结果见上，历史未截断。回复「继续」即可接着做。",
            crate::core::agent::stream::STREAM_IDLE_TIMEOUT_SECS
        );

        // 落库口径（丙方案）：半截内容落成**一条**助手消息，中断类型结构化写进
        // `interrupt_kinds`（标记文本在发送给模型前由 kind 拼回，见
        // `interrupt_marker_for`）。断流时流层已优雅返回，思考内容同样随正文落库。
        {
            let mut session = self.ctx.memory.lock().await;
            store_interrupted_turn(
                &mut session,
                &partial,
                &stream_result.thinking,
                InterruptKind::StreamTimeout,
            )
            .await;
        }

        self.final_answer = partial.clone();
        self.notice = Some(reason.clone());
        // 状态字符串（进 agent_runs.error / 审计，非用户可见）：同样只描述等待事实，
        // 不断言服务端。见本函数上方 handle_stream_idle_timeout 的归因注释。
        self.interrupted_reason = Some("上游响应超时（流内空闲超时）".to_string());

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

        // 记录到审计层（agent_run_events），供界面「执行详情」展示中断原因；
        // 同时保留本轮已收到的响应块，崩溃重建时可原样还原。
        // 轮号 +1 与帧级通道/收尾覆盖同尺（见 record_loop_event 取号说明）。
        agent_runs::upsert_loop_event(
            &self.run_id,
            &self.sid,
            self.total_loop_count + 1,
            resp_blocks_for_event,
            Vec::new(),
            "interrupted",
            Some(reason.clone()),
            0,
            0,
            Some(self.model_id.clone()),
        );

        // ⚠️ 把 run 状态落为 Interrupted（与错误中止/用户取消两条收尾路对齐）。
        //
        // 此前本路径只写 events 行、不更新 agent_runs.status —— run 永久停留
        // Running，重启后被 find_interrupted_run 以 STALE 判定当崩溃遗留捞出，
        // 触发恢复链把已收尾的内容再补一遍（session 10b68285 重复数据事故的
        // 直接根因）。统一收尾处对 interrupted_reason 路径刻意跳过 complete_run，
        // 所以这里必须自己落状态。
        //
        // 注意：本处 reason 属**流空闲超时**（stream.rs 的 STREAM_IDLE_TIMEOUT_SECS），
        // 不是 plan 模式的「规划看门狗」——后者走 handle_plan_watchdog_summary
        // 且以 chat source 正常收尾，不会进本路径。措辞只描述超时事实，不断言服务端。
        agent_runs::interrupt_run(
            &self.app,
            &self.run_id,
            InterruptKind::StreamTimeout,
            self.req_input_tokens,
            self.req_output_tokens,
        );
    }

    /// 循环上限确认（满 30 轮触发）：弹窗请用户授权继续
    /// 返回 true 表示继续（重置 loop_count），false 表示终止；
    /// 超时或拒绝时置 loop_continuation_pending，用户稍后可通过 resume_pipeline 续跑
    async fn request_loop_continuation(&mut self) -> bool {
        let _ = self.app.emit(
            "chat-stream",
            json!({
                "content": format!("\n> **[等待确认]** 代理执行已达到 {} 回合，等待用户确认是否继续。\n", crate::infra::types::constants::MAX_AGENT_LOOP_BEFORE_CONFIRM),
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
            Vec::new(),
        )
        .await;
        if decision.is_allowed() {
            // 清除待续跑标记（用户及时响应了）
            *self.ctx.loop_continuation_pending.lock().await = false;
            self.loop_count = 0;
            let _ = self.app.emit(
                "chat-stream",
                json!({
                    "content": "\n> **[已授权]** 用户已授权继续执行。\n",
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

    /// 上下文压缩检查（项目内唯一的 LLM 摘要压缩，属单级）：超阈值时调 LLM 把旧历史
    /// 压缩成一段摘要；压缩前后保证用户最新消息仍在历史末尾，且后续轮次能正确计算
    /// initial_msg_index
    ///
    /// 判据算法统一在 `infra::llm::context_budget`（与子代理共用，见该模块文档）：
    /// - 阈值 =（模型窗口 − 输出预算）× 70%
    /// - 占用 = 本地估算 ×（上一轮实测 ÷ 上一轮估算）
    async fn compact_if_needed(&mut self) {
        // 1. 估算当前上下文 token（消息 + 工具 schema）
        let (messages_for_estimate, sources_for_estimate, kinds_for_estimate) = {
            let session = self.ctx.memory.lock().await;
            (
                session.messages.clone(),
                session.sources.clone(),
                session.interrupt_kinds.clone(),
            )
        };
        let history_snapshot = self.prepare_history_snapshot_from_messages(
            messages_for_estimate,
            &sources_for_estimate,
            &kinds_for_estimate,
        );
        let tools = self.current_tools();
        let estimate = self.build_context_estimate(&history_snapshot, &tools);
        let est_now = estimate.estimated_tokens;

        // 2. 判据：分母用模型的真实窗口并预留输出预算；分子用上一轮的厂商实测值
        //    标定本地估算（估算系统性偏低，直接比会一路不触发）。
        let window = crate::infra::llm::context_budget::resolve_context_window(&self.model_id);
        let output_budget = crate::infra::llm::context_budget::resolve_output_budget(
            &self.model_id,
            self.cfg.max_tokens,
        );
        let trigger =
            crate::infra::llm::context_budget::compact_trigger_tokens(window, output_budget);
        // 上一轮快照里的「实测 input / 本地估算」是同一时刻写入的一对，可直接作标定基准。
        // 首轮（或快照缺失）为 None，此时退回纯估算。
        let (prev_measured, est_prev) = crate::core::session::get_context_snapshot(&self.sid)
            .ok()
            .flatten()
            .map(|snapshot| (snapshot.provider_input_tokens, Some(snapshot.estimated_tokens)))
            .unwrap_or((None, None));
        let tokens = crate::infra::llm::context_budget::calibrated_context_tokens(
            prev_measured,
            est_prev,
            est_now,
        );

        // >85% 可用窗口：LLM 摘要压缩
        if crate::infra::llm::context_budget::should_compact(tokens, trigger) {
            println!(
                "[贾维斯] 上下文占用 {} > {}% 可用窗口（窗口 {} − 输出预算 {} = {}, 本地估算 {}, 上轮实测 {:?}）, 触发 LLM 摘要压缩",
                tokens,
                crate::infra::llm::context_budget::COMPACT_TRIGGER_PERCENT,
                window,
                output_budget,
                window.saturating_sub(output_budget),
                est_now,
                prev_measured
            );

            let mut session = self.ctx.memory.lock().await;
            // 2. 压缩前先临时取出最后一条用户消息，压缩完成后再放回。
            //
            // ⚠️ 注意「最后一条是用户消息」并不等于「最后一条是本轮提问」：
            // 工具结果同样是 `Message::User`（Anthropic 约定）。所以这段 pop/restore
            // 单独用**保不住本轮提问** —— 真正保证本轮任务不被压掉的是下面的
            // 「本轮区间白名单」（`initial_msg_index` 之后整段不参与压缩）。
            // 它在这里的职责只剩一个：避免压缩后历史以 User 结尾（Anthropic 要求
            // 首条须为 User、且相邻同角色需合并，这里补 Assistant 垫片兜住）。
            let mut last_user_msg = None;
            if let Some(Message::User { .. }) = session.messages.last() {
                last_user_msg = pop_message(&mut session);
            }

            // 2b. 本轮区间白名单：`initial_msg_index` = 本轮用户消息在 messages 中的下标
            //     （见 pre_loop 的 inject_user_message）。它之后的所有消息都属于
            //     「本轮任务」——本轮提问、本轮 assistant 块、本轮工具结果、
            //     中途切换工作模式追加的 context 快照 —— 一律不参与压缩。
            //
            //     取**连续区间**而非挑选消息：Assistant(ToolUse) 与其后的
            //     User(ToolResult) 必须成对出现在请求里，切开会让 provider 直接 400。
            let turn_start = self.initial_msg_index.min(session.messages.len());
            let turn_messages: Vec<Message> = session.messages.drain(turn_start..).collect();
            let turn_sources: Vec<String> = if session.sources.len() >= turn_messages.len() {
                let keep = session.sources.len().saturating_sub(turn_messages.len());
                session.sources.drain(keep..).collect()
            } else {
                Vec::new()
            };
            // 中断类型与本轮区间同尺 drain —— 三个平行数组必须一起摘出来、一起拼回，
            // 否则压缩后 kind 会错配到别的消息上（拼回时按索引取，越界得 None）。
            let turn_kinds: Vec<Option<String>> =
                if session.interrupt_kinds.len() >= turn_messages.len() {
                    let keep = session
                        .interrupt_kinds
                        .len()
                        .saturating_sub(turn_messages.len());
                    session.interrupt_kinds.drain(keep..).collect()
                } else {
                    Vec::new()
                };
            let turn_len = turn_messages.len();

            // 2c. 本轮就是全部历史（`initial_msg_index` 指向第 0 条）⇒ 没有可压的前缀。
            //     此时**不切开本轮**，直接放弃本次压缩，等下一次用户提问重新划界。
            //     切开本轮正是要消除的问题：模型会重做已做过的动作。
            let prefix_len = session.messages.len();
            if prefix_len == 0 {
                // 放回本轮区间后直接返回。
                //（用 append_message_with_kind 而非直接 push：它会先 normalize_message_ids
                // 把 message_ids / sources / interrupt_kinds 与 messages 重新对齐 ——
                // drain 之后它们长度已不同步）
                for (i, (msg, src)) in turn_messages.into_iter().zip(turn_sources).enumerate() {
                    let kind = turn_kinds.get(i).cloned().flatten();
                    append_message_with_kind(&mut session, msg, &src, kind.as_deref());
                }
                // 本轮起点 = 放回后「区间起点」的下标
                self.initial_msg_index = session.messages.len().saturating_sub(turn_len);
                if let Some((msg, message_id)) = last_user_msg {
                    restore_message(&mut session, msg, message_id, "chat");
                    self.initial_msg_index = session.messages.len().saturating_sub(1);
                }
                println!(
                    "[JARVIS] 本轮即为全部历史（{} 条），无可压缩前缀，跳过本次压缩 —— 本轮区间保持完整",
                    session.messages.len()
                );
                return;
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

            // 无论压缩成功与否，本轮区间都必须原样拼回
            match compact_result {
                Err(e) => {
                    println!("[JARVIS] 自动压缩失败: {}，继续使用原始上下文", e);
                    for (i, (msg, src)) in turn_messages.into_iter().zip(turn_sources).enumerate() {
                        let kind = turn_kinds.get(i).cloned().flatten();
                        append_message_with_kind(&mut session, msg, &src, kind.as_deref());
                    }
                    // 前缀未被压，本轮起点仍落在原前缀之后
                    self.initial_msg_index = prefix_len;
                }
                Ok(()) => {
                    // 压缩成功：前缀已被压成「压缩请求 + 摘要」两条，
                    // 本轮区间原样拼回，本轮起点 = 压缩后的前缀长度。
                    //
                    // ⚠️ 本轮提问「原本就在区间里」（turn_start 就是它的下标），
                    // 所以这里**不能**再用 last_user_msg 恢复一次，否则会插入两条提问。
                    // last_user_msg 只用于「最后一条是工具结果」的情形（见下），
                    // 那种情况下它属于本轮区间，已被上面拼回。
                    let compacted_len = session.messages.len();
                    for (i, (msg, src)) in turn_messages.into_iter().zip(turn_sources).enumerate() {
                        let kind = turn_kinds.get(i).cloned().flatten();
                        append_message_with_kind(&mut session, msg, &src, kind.as_deref());
                    }
                    self.initial_msg_index = compacted_len;
                    println!(
                        "[JARVIS] 压缩完成：前缀 {} 条 → {} 条，本轮区间 {} 条原样保留（本轮起点 = {}）",
                        prefix_len,
                        compacted_len,
                        session.messages.len() - compacted_len,
                        self.initial_msg_index
                    );
                }
            }

            // 3. 兜底：历史不能以 User 结尾（补一条 Assistant 垫片）。
            //    正常路径下本轮区间末尾是 assistant 块或工具结果，这里只是防御。
            if let Some((msg, message_id)) = last_user_msg {
                // ⚠️ 去重：最后一条用户消息若已随本轮区间拼回（message_id 已存在），
                //    就不能再恢复一次 —— 否则历史里会出现两条同样的提问。
                let already_present = session
                    .message_ids
                    .iter()
                    .any(|existing| existing == &message_id);
                if already_present {
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
                } else {
                    restore_message(&mut session, msg, message_id, "chat");
                    self.initial_msg_index = session.messages.len().saturating_sub(1);
                }
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
        interrupt_kinds: &[Option<String>],
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
        for (idx, (mut msg, src)) in messages.into_iter().zip(sources.iter()).enumerate() {
            if !matches!(src.as_str(), "chat" | "compact" | "context" | "interrupted") {
                continue;
            }
            // 中断类型 → 把标记拼回正文末尾（**模型必须看到"上一句被截断"**，
            // 否则续跑会重复或断片）。存储侧正文是干净的，标记只在发送前出现。
            if let Some(kind) = interrupt_kinds
                .get(idx)
                .and_then(|k| k.as_deref())
                .and_then(InterruptKind::from_db)
            {
                append_interrupt_marker_to_message(&mut msg, interrupt_marker_for(kind));
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
        let interrupt_kinds = session.interrupt_kinds.clone();
        drop(session); // 释放锁
        self.prepare_history_snapshot_from_messages(messages, &sources, &interrupt_kinds)
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
                                    ContentBlock::Context { text } => {
                                        // 动态上下文块（意图标签 / 能力边界 / 项目结构 / 用户画像）。
                                        // 出网前由 `materialize_context_blocks_for_wire` 翻译成普通 Text
                                        // 一并发出，所以它**真实占用** prompt token，这里必须原样渲染。
                                        // 早期版本落到 `_ => {}` 被静默丢弃，是估算偏低的原因之一。
                                        let trimmed = text.trim();
                                        if !trimmed.is_empty() {
                                            out.push_str("  ← [Context]\n");
                                            out.push_str(trimmed);
                                            out.push('\n');
                                        }
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
    /// 每个 section 记录独立字符数/token 数，方便 UI 定位占比。
    ///
    /// **口径：按实际发出去的 payload 计数。** 这里刻意**不**调用
    /// `adapters::strip_context_blocks`——出网路径
    /// （`adapters::materialize_context_blocks_for_wire`）会把每条用户消息里的
    /// `<context>` 动态上下文块翻译成普通 Text 一并发给模型，它真实占用 prompt token。
    /// 早先版本先 strip 再计数，把这部分整块漏算，实测长会话里估算 17.3k vs 实际 52.0k，
    /// 连带把压缩触发阈值拖到 3 倍之后。`Session Messages` 的 raw JSON 里因此会出现
    /// `"type": "context"` 块——那是内部表示，出网时会被翻译，不是协议字段。
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

        /// 计数口径与展示内容分离的版本：`content` 仍用可读文本（面板展示用），
        /// 但 `chars` / `estimated_tokens` 按 `count_text` 计算（真实 payload 口径）。
        ///
        /// 2026-09-17 起 `messages` 分区改用这个：`format_messages_readable` 会把
        /// `tool_result` 截断到 3 行、ToolUse 截到 200 字符、Thinking 截到 80 字符，
        /// 而真实发出去的是完整内容 —— 拿截断文本计数会系统性低估
        /// （长会话实测 17.3k vs 实际 52.0k，约 3 倍），压缩该触发时不触发。
        fn section_with_raw_counting_full(
            model_id: &str,
            key: &str,
            label: &str,
            display_content: String,
            counting_text: &str,
            item_count: usize,
            raw: String,
        ) -> ContextSectionSnapshot {
            let chars = counting_text.chars().count();
            let token_count = crate::infra::llm::token_count::count_text(model_id, counting_text);
            ContextSectionSnapshot {
                key: key.to_string(),
                label: label.to_string(),
                chars,
                estimated_tokens: token_count.tokens,
                token_count_method: token_count.method.as_str().to_string(),
                item_count,
                // 面板展开看的是可读文本；计数走 counting_text
                content: display_content,
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

        // 直接数**未 strip** 的 history：Context 块会被翻译成 Text 发出去，必须计入。
        let (tool_call_count, tool_result_count, image_count, thinking_count) =
            count_blocks(history_snapshot);
        let messages_text = Self::format_messages_readable(history_snapshot);
        let messages_json = serde_json::to_string_pretty(history_snapshot).unwrap_or_default();
        let tools_json = serde_json::to_string_pretty(tools).unwrap_or_default();
        let mut sections = vec![
            section(
                &self.model_id,
                "system",
                "System Prompt",
                self.system_prompt.clone(),
                1,
            ),
            // 这里曾有一个独立的 `dynamic` 分区（`self.dynamic_context_str`）。
            // 它已经**不再需要**：动态上下文由 `pre_loop` 经 `inject_user_message`
            // 写进本轮用户消息的 `<context>` 块，随 history 一起落库；
            // 上面的 `messages` 分区现在按未 strip 的 history 计数，已经包含它。
            // 保留独立分区等于把本轮动态上下文重复计一次，总量会虚高。
            // 需要看它有多大时，展开 `messages` 分区找 `[Context]` 段即可。
            // ⚠️ 计数口径（2026-09-17 修）：`format_messages_readable` 会把 tool_result
            // 截断到 3 行、ToolUse 截到 200 字符、Thinking 截到 80 字符 —— 那是**给人看**的
            // 可读摘要。真实发给厂商的 payload 是完整内容，用截断文本计数会系统性低估
            // （长会话实测 17.3k vs 实际 52.0k，约 3 倍），导致压缩该触发时不触发。
            // 故：`content`（面板展示）继续用可读文本，`chars` / `estimated_tokens`
            // （判据消费）改用完整 JSON。
            section_with_raw_counting_full(
                &self.model_id,
                "messages",
                "Session Messages",
                messages_text,
                &messages_json,
                history_snapshot.len(),
                messages_json.clone(),
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
            message_count: history_snapshot.len(),
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
            // 必须与请求体里的 `max_tokens` **同源**（`resolve_max_tokens()`）。原先写死
            // `MAX_TOKENS_CONTEXT`(=8192)，导致快照里给用户看的输出上限与真正发出去的不是一个数；
            // 自动压缩判据（`context_budget`）的输出预算也是同一个函数，三处从此对齐。
            max_output_tokens: self.resolve_max_tokens(),
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

        // 会话累计用量：**每次请求**就落库，而不是攒到回合收尾再交一次。
        //
        // 旧实现把整轮增量押在收尾的 `save_session` 里，带来两个毛病：
        // 1. 概览栏的「累计命中」在整轮跑完前一直停在上一轮的值（长回合里看着像卡死）；
        // 2. 回合被取消时收尾那步传 `None`，整轮 token 直接丢掉、永久不进累计。
        // 改成逐请求累加后两者同时消失——请求既然已经发生，用量就已经产生了。
        //
        // 缓存字段的语义要守住：`None` 表示这家没报告，按 0 累加（不要回退成
        // `input - hit` 之类的推导，各家 `input_tokens` 口径不同，见 `SessionMeta` 的注释）。
        let delta = crate::core::session::TokenUsageDelta {
            input: input_tokens,
            output: output_tokens,
            cache_hit: cache_hit_tokens.unwrap_or(0),
            cache_miss: cache_miss_tokens.unwrap_or(0),
        };
        match crate::core::session::accumulate_session_token_usage(&self.sid, delta) {
            // 推的是**累加后的累计值**，前端整值覆盖写入，不自己再累一遍
            Ok(Some(totals)) => {
                let _ = self.app.emit(
                    "session-usage-updated",
                    serde_json::json!({
                        "sessionId": self.sid,
                        "inputTokens": totals.input,
                        "outputTokens": totals.output,
                        "cacheHitTokens": totals.cache_hit,
                        "cacheMissTokens": totals.cache_miss,
                    }),
                );
            }
            Ok(None) => {}
            Err(err) => eprintln!("[JARVIS] 累加会话用量失败: {}", err),
        }
    }

    /// 解析 max_tokens：用户覆盖 > 模型注册表 > 常量兜底
    ///
    /// 它与上下文压缩判据里的「输出预算」是**同一个数**（都走
    /// `infra::llm::context_budget::resolve_output_budget`）——给模型留出的输出
    /// 空间，必须和真正发出去的 `max_tokens` 对得上。
    fn resolve_max_tokens(&self) -> i32 {
        crate::infra::llm::context_budget::resolve_output_budget(
            &self.model_id,
            self.cfg.max_tokens,
        ) as i32
    }

    /// 构建 LLM API 请求体（内部统一按 Anthropic 结构建模）
    ///
    /// - 总是流式请求（stream: true），写入系统提示词、工具 schema、思考配置、温度等
    /// - OpenAI 格式模型：经 adapters 翻译消息/工具，并按模型注册表注入各家“思考参数”
    /// - 返回值第二项 ApiFormat 交给 stream.rs 与 UsageObservation，协议差异在模型接入层内部消化
    fn build_llm_request(
        &self,
        history_snapshot: Vec<Message>,
    ) -> (serde_json::Value, crate::infra::llm::api_format::ApiFormat) {
        // 1. 取系统提示词与工具定义，并更新上下文监控快照
        // system 在 setup 阶段只组装一次并保持字节恒定，整个会话内不再随 work_mode 变化。
        let system_prompt = self.system_prompt.clone();
        let tools = self.current_tools();
        self.update_context_snapshot(&history_snapshot, &tools);

        let max_tokens = self.resolve_max_tokens();

        // 采样参数：注册表声明不接受的模型一律剥离。Anthropic 自 Opus 4.7 起、
        // 以及 Opus 5 / Sonnet 5 / Fable 5 全系已废弃 temperature/top_p/top_k，
        // 传了直接 400（改造前这个能力标志全仓没人读）。
        let sampling_ok = crate::infra::llm::registry::supports_sampling_params(&self.model_id);
        let mut request_body = AnthropicRequest {
            model: self.model_id.clone(),
            max_tokens,
            system: system_prompt.clone(),
            messages: history_snapshot,
            tools,
            stream: true,
            thinking: None,
            temperature: if sampling_ok { self.cfg.temperature } else { None },
            top_p: if sampling_ok { self.cfg.top_p } else { None },
            top_k: if sampling_ok { self.cfg.top_k } else { None },
            output_config: None,
        };

        // 思考参数走注册表统一决策（`registry::plan_anthropic_thinking`）。原先这里
        // 写死 `{type: enabled|disabled, budget_tokens: 1024}`，而该形态在 Opus 4.7
        // 及之后已被移除（Fable 5 系连 disabled 都拒），直连必然 400。
        let thinking_plan = crate::infra::llm::registry::plan_anthropic_thinking(
            &self.model_id,
            self.should_think,
            None,
        );
        let thinking_active = thinking_plan.thinking_active();
        request_body.thinking = thinking_plan.thinking;
        request_body.output_config = thinking_plan.output_config;
        // 用计划里的实际状态判断，而不是 `self.should_think`：`adaptive_only` 类模型
        // 无法关闭，调用方传 false 时思考依然是开着的。
        if thinking_active && request_body.max_tokens <= 1024 {
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
            (serde_json::to_value(openai_req).unwrap(), crate::infra::llm::api_format::ApiFormat::OpenAI)
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
            (serde_json::to_value(request_body).unwrap(), crate::infra::llm::api_format::ApiFormat::Anthropic)
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
                        // 本轮标记为 interrupted（保留帧级通道已写的半截内容）
                        self.mark_loop_event_interrupted(error.to_string()).await;
                        agent_runs::fail_run(
                            &self.app,
                            &self.run_id,
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
                // 本轮标记为 interrupted（保留帧级通道已写的半截内容）
                self.mark_loop_event_interrupted(e.to_string()).await;
                agent_runs::fail_run(
                    &self.app,
                    &self.run_id,
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
    /// - 本 loop 提交了规划方案（`loop_submitted_plan`：裸 ProposePlan 或
    ///   经 ExecuteTool 包装——只匹配裸名时，包装形态的提交轮不喂狗，看门狗误触发）；
    /// - 本 loop 已把 work_mode 切出 plan（SwitchWorkMode 到 edit）。
    ///
    /// **调用位置要求**：必须在"有工具"和"无工具"两条路径上都能到达。
    /// 旧实现只挂在工具执行分支，而"纯文本空转"走的是无工具分支，
    /// 计数器永远停在 0、看门狗结构性失效（2026-09-19 死循环事故根因）。
    fn update_plan_watchdog(&mut self, work_mode: &str, tool_calls: &[(String, String)]) -> bool {
        use crate::infra::types::constants::{PLAN_WATCHDOG_MAX_CONSECUTIVE_STALLS, PLAN_WATCHDOG_MAX_LOOPS_WITHOUT_PLAN};

        if work_mode != "plan" {
            self.plan_consecutive_stalls = 0;
            self.plan_total_loops_without_plan = 0;
            return false;
        }

        if loop_submitted_plan(tool_calls) {
            self.plan_consecutive_stalls = 0;
            self.plan_total_loops_without_plan = 0;
            return false;
        }

        // 无工具轮（tool_calls 为空）计 1：轮次本身就是一次空转，
        // 不能因为"没调用工具"就加 0（否则纯文本空转永远触发不了）。
        let stall_increment = tool_calls.len().max(1);
        self.plan_consecutive_stalls = self.plan_consecutive_stalls.saturating_add(stall_increment);
        self.plan_total_loops_without_plan = self.plan_total_loops_without_plan.saturating_add(1);

        self.plan_consecutive_stalls >= PLAN_WATCHDOG_MAX_CONSECUTIVE_STALLS
            || self.plan_total_loops_without_plan >= PLAN_WATCHDOG_MAX_LOOPS_WITHOUT_PLAN
    }

    /// B3 触发后：先复用当前 system + 历史做一次缓存友好的 LLM 进度小结，
    /// 再把决策权交还用户并强制结束当前 loop。
    async fn handle_plan_watchdog_summary(&mut self) {
        use crate::infra::types::constants::{PLAN_WATCHDOG_MAX_CONSECUTIVE_STALLS, PLAN_WATCHDOG_MAX_LOOPS_WITHOUT_PLAN};

        // 触发指令带实际计数：触发时两个计数器尚未清零（触发分支直接 break），
        // 把真实值与阈值一起告诉模型，让它对"空转了多久"有量化感知。
        let instruction = format!(
            "【系统通知】规划探索已达到看门狗阈值（本轮已累计 {} 次工具调用未提交方案 / {} 轮未提交方案；阈值：{} 次或 {} 轮）。请立即停止探索，不要调用任何工具，只用一段话输出当前进度小结与下一步建议（继续探索 / 缩小范围 / 直接执行）。",
            self.plan_consecutive_stalls,
            self.plan_total_loops_without_plan,
            PLAN_WATCHDOG_MAX_CONSECUTIVE_STALLS,
            PLAN_WATCHDOG_MAX_LOOPS_WITHOUT_PLAN,
        );
        let mut snapshot = self.prepare_history_snapshot().await;
        snapshot.push(Message::User {
            content: Content::Single(instruction),
        });

        let (req_json, req_api_format) = self.build_llm_request(snapshot);
        let mut summary = String::new();
        match self.call_api_with_retry(&req_json).await {
            Ok(Some(resp)) => {
                let mut stream = resp.bytes_stream().eventsource();
                let parsed = process_stream(
                    &mut stream,
                    req_api_format,
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
                        model_id: Some(self.model_id.clone()),
                        // 小结是内部一次性的短请求，没有"崩溃后要续跑"的语义
                        crash_protection: false,
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
        self.tool_execution_summary = Some(summary.clone());
        let _ = self.app.emit(
            "chat-stream",
            json!({
                "content": "\n> **[规划探索已到上限]** 规划探索未收敛，已自动停下并把决策权交还给你。可选择：继续探索 / 缩小范围 / 直接执行。\n",
                "sessionId": self.sid,
                "loopCount": self.total_loop_count + 1
            }),
        );

        // 小结落库：此前看门狗收尾只进 final_answer 与事件流，session_messages
        // 零痕迹（尾部悬 tool_result，刷新后聊天流回退到上一条正文）。
        // 这里把小结补成 assistant 消息并立即整仓落库，尾部以 assistant 收口。
        //
        // 正文只放小结本身，中断类型走 `interrupt_kinds`（阶段二改造）；
        // source 保持 `chat`——这条消息**会发给 LLM**，标记由 `interrupt_marker_for`
        // 在发送前按 kind 拼回（见 doc/状态标注符号统一与结构化改造方案.md）。
        {
            let mut session = self.ctx.memory.lock().await;
            if !assistant_text_exists_at_tail(&session.messages, &summary) {
                append_message_with_kind(
                    &mut session,
                    Message::Assistant {
                        content: Content::Single(summary.clone()),
                    },
                    "chat",
                    Some(InterruptKind::PlanLimit.as_str()),
                );
            }
            crate::core::session::save_session(&self.sid, &session, None);
        }
    }

    /// 中途模式切换：生成一条 seq 更大的完整上下文快照，并入本 loop 的
    /// tool_result 块数组（尾部追加，不回改旧前缀、不独立成条）。
    ///
    /// 旧形态（独立 user/context 消息）会在 session_messages 里产生
    /// user→user 相邻，破坏消息级 user/assistant 严格交替；并入后快照随
    /// 工具结果消息落库，序列保持 assistant(tool_use) → user(tool_result+快照)
    /// → assistant 的标准工具循环形态。缓存安全性不变：tool_result 消息
    /// 是本轮新产生、从未入缓存前缀，加块不影响已缓存前缀。
    /// 落库由调用侧在 append 后立即执行（防 seq 回退/重复的动机不变）。
    async fn merge_mode_snapshot_into_blocks(&mut self, blocks: &mut Vec<ContentBlock>, mode: &str) {
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
        blocks.push(ContentBlock::Context { text: snapshot });
    }
}

/// 取本轮响应块，并把"有思考但块里没有"的情形补上。
///
/// 为什么需要：流层把思考累积到 `current_thinking_this_turn`，落块走的是另一条
/// 独立分支（Anthropic 精确改写第 i 块 / OpenAI 按"当前块是不是思考"追加），
/// 两者并不保证一一对应。而写 `agent_run_events` 需要的是"这一轮模型说过什么"
/// 的完整快照——缺了 thinking 就等于丢了思考链，恢复时无法回传给 Anthropic。
///
/// `take` 掉（而非 `clone`）是刻意的：调用点都是"这轮到此为止"，
/// 留着只会被下一轮误当成自己的思考。
fn take_resp_blocks_with_thinking(
    blocks: &[ContentBlock],
    thinking_this_turn: &mut String,
) -> Vec<ContentBlock> {
    let mut out = blocks.to_vec();
    if !thinking_this_turn.trim().is_empty()
        && !out
            .iter()
            .any(|b| matches!(b, ContentBlock::Thinking { thinking, .. } if !thinking.trim().is_empty()))
    {
        out.insert(
            0,
            ContentBlock::Thinking {
                thinking: std::mem::take(thinking_this_turn),
                signature: String::new(),
            },
        );
    }
    out
}

/// 从结构化响应块里取出（正文, 思考）两段纯文本。
///
/// 只认 Text / Thinking 两类块：ToolUse 不是"模型说的话"，ToolResult 属于工具侧，
/// 都不是中断提示该合并的正文。各类型内部按顺序拼接（`join("\n\n")`），
/// 与 `store_interrupted_turn` 的分块口径一致。
fn blocks_to_text_and_thinking(blocks: &[ContentBlock]) -> (String, String) {
    let mut texts: Vec<String> = Vec::new();
    let mut thinkings: Vec<String> = Vec::new();
    for block in blocks {
        match block {
            ContentBlock::Text { text } if !text.trim().is_empty() => {
                texts.push(text.trim().to_string())
            }
            ContentBlock::Thinking { thinking, .. } if !thinking.trim().is_empty() => {
                thinkings.push(thinking.trim().to_string())
            }
            _ => {}
        }
    }
    (texts.join("\n\n"), thinkings.join("\n\n"))
}

/// 主流程入口：依次执行 4 个阶段
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
/// 依次执行：阶段 1 setup → 阶段 2 pre_loop
/// → 阶段 3 run_main_loop（出错走 abort_after_error）→ 阶段 4 finalize
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

    // ── 阶段 2：循环前准备（崩溃恢复 + 注入用户消息 + 启动 run）──
    state.pre_loop().await;

    // ── 阶段 3：主循环（调 LLM → 执行工具 → 直到得出最终答案）──
    if let Err(err) = state.run_main_loop().await {
        state.abort_after_error(&err).await;
        return Err(err);
    }

    // ── 阶段 4：收尾（持久化 / 快照 / 记忆 / 结果组装）──
    Ok(state.finalize().await)
}

#[cfg(test)]
mod plan_watchdog_feeding_tests {
    use super::loop_submitted_plan;

    fn call(name: &str, input: &str) -> (String, String) {
        (name.to_string(), input.to_string())
    }

    /// 裸 ProposePlan 喂狗（既有行为，回归防护）
    #[test]
    fn bare_propose_plan_feeds_the_watchdog() {
        let calls = vec![call("ProposePlan", "{}")];
        assert!(loop_submitted_plan(&calls));
    }

    /// ExecuteTool 包装的 ProposePlan 必须喂狗——模型提交方案的固定形态，
    /// 旧实现只匹配裸名，导致提交轮不喂狗、看门狗误触发截胡收尾（B 事故路径）
    #[test]
    fn execute_tool_wrapped_propose_plan_feeds_the_watchdog() {
        let calls = vec![call(
            "ExecuteTool",
            r#"{"name":"ProposePlan","args":{"title":"方案","content":"..."}}"#,
        )];
        assert!(loop_submitted_plan(&calls));
    }

    /// ExecuteTool 包装的其他延迟工具不算提交方案，照常计空转
    #[test]
    fn execute_tool_wrapped_other_tools_do_not_feed() {
        let calls = vec![call(
            "ExecuteTool",
            r#"{"name":"ReadFile","args":{"path":"src/main.rs"}}"#,
        )];
        assert!(!loop_submitted_plan(&calls));
    }

    /// 普通核心工具不算提交方案
    #[test]
    fn core_tools_do_not_feed() {
        let calls = vec![call("ListTasks", "{}"), call("ReadFile", "{}")];
        assert!(!loop_submitted_plan(&calls));
    }

    /// 参数 JSON 坏了不能 panic，按"未提交"处理
    #[test]
    fn malformed_execute_tool_input_does_not_panic() {
        let calls = vec![call("ExecuteTool", "{not-json")];
        assert!(!loop_submitted_plan(&calls));
    }

    /// 同轮混合调用：一个 ExecuteTool(ProposePlan) 就算提交（喂狗是 any 语义）
    #[test]
    fn mixed_calls_with_one_submit_feed() {
        let calls = vec![
            call("ExecuteTool", r#"{"name":"SearchWorkspace","args":{}}"#),
            call("ExecuteTool", r#"{"name":"ProposePlan","args":{}}"#),
        ];
        assert!(loop_submitted_plan(&calls));
    }
}

#[cfg(test)]
mod thinking_freeze_tests {
    use super::should_think_for_loop;

    /// 回归：整轮每个 loop 的 thinking 状态必须恒定不变。
    ///
    /// 旧实现在 `loop_count > 0` 时回落到 audience 默认，于是第二轮变 enabled，
    /// 而历史里那条 assistant 是在关闭 thinking 时产生的、没有 thinking 块可回传，
    /// Anthropic 协议（含 DeepSeek 的 anthropic 兼容端点）会直接 400：
    /// `content[].thinking in the thinking mode must be passed back to the API`。
    ///
    /// v14 起"整轮的初始值"由 `thinking::decide` 一次裁决给出，本函数只负责守住"不翻转"。
    #[test]
    fn thinking_state_is_frozen_within_a_turn() {
        let cases = [
            crate::core::session::thinking::ThinkingMode::ON,
            crate::core::session::thinking::ThinkingMode::OFF,
        ];
        for mode in cases {
            let turn_think = mode.0;
            for loop_count in 0..=5 {
                assert_eq!(
                    should_think_for_loop(turn_think, loop_count),
                    turn_think,
                    "loop {} 不得把 thinking 翻转",
                    loop_count
                );
            }
        }
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
            "**[回复被中断]** 上次回复在此处中断，请基于上下文继续完成。"
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

    /// 两种标记共享统一前缀；`strip_interrupt_markers`（agent_runs.rs）的
    /// "重建消息 vs 现存消息"等价比较依赖它
    #[test]
    fn both_share_detectable_prefix() {
        for marker in [INTERRUPT_MARKER_RESUMABLE, INTERRUPT_MARKER_STOPPED] {
            assert!(
                marker.contains("[回复被中断]"),
                "等价比较逻辑依赖该标签，不得修改：{marker}"
            );
            assert!(
                marker.starts_with("**["),
                "应以 `**[标签]` 开头（2026-09-20 风格统一：去掉 `>` 引用符与 emoji），\
                 便于统一剥离：{marker}"
            );
            assert!(
                !marker.contains('⚠'),
                "不应再含 emoji 哨兵：{marker}"
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
    use crate::core::agent::stream::{waiting_hint_secs, STREAM_IDLE_TIMEOUT_SECS};

    /// 静默未达阈值：不提示（避免正常生成被打扰）
    #[test]
    fn no_hint_before_threshold() {
        assert!(!should_emit_waiting_hint(waiting_hint_secs() - 1, false));
        assert!(!should_emit_waiting_hint(0, false));
    }

    /// 刚好达到阈值且本段静默期未提示过 → 提示
    #[test]
    fn hints_once_at_threshold() {
        assert!(should_emit_waiting_hint(waiting_hint_secs(), false));
    }

    /// **回归防护（用户实测 bug）**：同一段静默期继续延长时**不得重复提示**。
    /// 首版每 30s 无条件发一条，页面被「已 30 秒未收到数据」刷屏。
    #[test]
    fn does_not_repeat_within_same_silence() {
        for extra in [0, 30, 60, 300] {
            assert!(
                !should_emit_waiting_hint(waiting_hint_secs() + extra, true),
                "静默 {} 秒时不应重复提示",
                waiting_hint_secs() + extra
            );
        }
    }

    /// **提示阈值必须由超时阈值派生**：以后者为唯一事实源，改一处即可。
    /// 硬编码独立的 30 曾在超时放宽到 120 后把用户晾在中间 90 秒。
    #[test]
    fn hint_threshold_is_derived_from_idle_timeout() {
        assert_eq!(waiting_hint_secs(), (STREAM_IDLE_TIMEOUT_SECS / 3).max(15));
        assert!(
            waiting_hint_secs() < STREAM_IDLE_TIMEOUT_SECS,
            "提示必须早于终止：hint={} timeout={}",
            waiting_hint_secs(),
            STREAM_IDLE_TIMEOUT_SECS
        );
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

/// 上下文估算口径的回归测试。
///
/// `build_context_estimate` 的 `messages` 分区是按 `format_messages_readable` 的
/// 输出做 token 计数的，所以那段摘要**必须覆盖真正发出去的内容**。
///
/// 动态上下文（`<context>` 块）出网前会被
/// `adapters::materialize_context_blocks_for_wire` 翻译成普通 Text 一并发出，
/// 但早先的渲染器把它落到 `_ => {}` 静默丢弃，于是估算系统性偏低
/// （实测长会话里估算 17.3k、实际 52.0k），连带把压缩触发阈值拖到 3 倍之后。
#[cfg(test)]
mod context_estimate_tests {
    use super::*;

    /// 动态上下文块必须出现在摘要里，且排在用户原文之前（与落库顺序一致）
    #[test]
    fn context_blocks_are_rendered_in_readable_digest() {
        let ctx = "<context_snapshot>\n项目结构索引占位\n</context_snapshot>";
        let messages = vec![Message::User {
            content: Content::Multiple(vec![
                ContentBlock::Context {
                    text: ctx.to_string(),
                },
                ContentBlock::Text {
                    text: "帮我看看".to_string(),
                },
            ]),
        }];

        let out = PipelineState::format_messages_readable(&messages);

        assert!(
            out.contains("项目结构索引占位"),
            "动态上下文块被丢弃了，估算会偏低：\n{out}"
        );
        assert!(
            out.contains("[Context]"),
            "应带 [Context] 标记，便于在浮层里定位：\n{out}"
        );
        assert!(out.contains("帮我看看"), "用户原文不能丢：\n{out}");
        assert!(
            out.find("[Context]") < out.find("帮我看看"),
            "块顺序应与消息内顺序一致（上下文在前）：\n{out}"
        );
    }

    /// 空白的上下文块不得产生噪音行
    #[test]
    fn empty_context_block_renders_nothing() {
        let messages = vec![Message::User {
            content: Content::Multiple(vec![
                ContentBlock::Context {
                    text: "   ".to_string(),
                },
                ContentBlock::Text {
                    text: "hi".to_string(),
                },
            ]),
        }];

        let out = PipelineState::format_messages_readable(&messages);
        assert!(!out.contains("[Context]"), "空白上下文块不应渲染：\n{out}");
    }
}

#[cfg(test)]
mod interrupted_turn_store_tests {
    //! 「中断落库保持一问一答」的语义防护（2026-09-19 丙方案定案）。
    //!
    //! 背景事故：取消/断流/报错三条收尾路径各自乱拼——
    //! 把多段正文拼成一条、又额外补一条 assistant 占位提示，
    //! 结果一次中断产出好几条助手消息，且正文为空时前端**向上一条**助手
    //! 消息借文渲染，用户看到的内容顺序割裂。
    //!
    //! 现口径：半截内容（思考 + 正文）+ 中断提示**合并成一条**消息；
    //! 正文为空（纯思考期中止）时把提示本身作为正文块，保证渲染锚点存在。
    use super::store_interrupted_turn;
    use crate::infra::types::models::{Content, ContentBlock, InterruptKind, Message, SessionMemory};

    /// 测试用中断类型（阶段二起 `store_interrupted_turn` 收 kind，不再收标记文本）。
    /// 取 `PipelineError` 代表"能接着做"一类——它的标记文本仍是
    /// `INTERRUPT_MARKER_RESUMABLE`，供"发送前拼回"的断言使用。
    const KIND: InterruptKind = InterruptKind::PipelineError;

    fn empty_memory() -> SessionMemory {
        SessionMemory::default()
    }

    /// 模拟正常路径已落库的助手消息（`store_assistant_response` 的产物：裸正文，无提示）
    fn seed_plain_assistant(session: &mut SessionMemory, text: &str) {
        super::append_message(
            session,
            Message::Assistant {
                content: Content::Multiple(vec![ContentBlock::Text {
                    text: text.to_string(),
                }]),
            },
            "chat",
        );
    }

    fn only_blocks(session: &SessionMemory) -> Vec<ContentBlock> {
        blocks_at(session, 0)
    }

    fn blocks_at(session: &SessionMemory, idx: usize) -> Vec<ContentBlock> {
        match &session.messages[idx] {
            Message::Assistant {
                content: Content::Multiple(blocks),
            } => blocks.clone(),
            other => panic!("期望 Multiple 助手消息，实际: {other:?}"),
        }
    }

    fn body_text(blocks: &[ContentBlock]) -> String {
        blocks
            .iter()
            .rev()
            .find_map(|b| match b {
                ContentBlock::Text { text } => Some(text.clone()),
                _ => None,
            })
            .expect("正文块必须存在（渲染锚点）")
    }

    // ── 分支二：尾部没有当轮内容 → 新建一条合并消息 ──

    /// 有正文 + 有思考：一条消息，思考在前正文在后，正文末尾附提示
    #[tokio::test]
    async fn text_and_thinking_merge_into_single_message() {
        let mut session = empty_memory();
        let wrote = store_interrupted_turn(&mut session, "半截正文", "半截思考", KIND).await;

        assert!(wrote, "有内容时必须落库");
        assert_eq!(session.messages.len(), 1, "必须只有一条助手消息（一问一答）");
        let blocks = only_blocks(&session);
        assert!(
            matches!(&blocks[0], ContentBlock::Thinking { .. }),
            "思考块在前"
        );
        let body = body_text(&blocks);
        assert!(body.starts_with("半截正文"), "正文必须保留原文：{body}");
        assert!(body.contains("半截正文"));
        assert!(
            !body.contains("回复被中断"),
            "阶段二起标记不进正文（只写 interrupt_kinds）：{body}"
        );
        assert_eq!(
            session.interrupt_kinds.last().cloned().flatten().as_deref(),
            Some(KIND.as_str()),
            "中断类型必须打在该消息上"
        );
        assert_eq!(session.sources[0], "interrupted");
    }

    /// 纯思考期中止（正文为空）：提示本身作为正文块 —— 前端渲染锚点不能缺
    #[tokio::test]
    async fn thinking_only_promotes_marker_to_body() {
        let mut session = empty_memory();
        let wrote = store_interrupted_turn(&mut session, "", "只有思考", KIND).await;

        assert!(wrote);
        assert_eq!(session.messages.len(), 1);
        let blocks = only_blocks(&session);
        assert_eq!(
            body_text(&blocks),
            "",
            "无产出时正文留空块（渲染锚点），小字由 kind 提供"
        );
    }

    /// 正文期中止（无思考）：只有正文块，末尾附提示
    #[tokio::test]
    async fn text_only_appends_marker() {
        let mut session = empty_memory();
        store_interrupted_turn(&mut session, "半截正文", "", KIND).await;

        let blocks = only_blocks(&session);
        assert_eq!(blocks.len(), 1, "无思考时不应凭空造 Thinking 块");
        let body = body_text(&blocks);
        assert!(!body.contains("回复被中断"), "标记不得进正文：{body}");
        assert!(body.contains("半截正文"));
    }

    /// 完全无产出：仍需一条只有提示的助手消息（否则界面无锚点，会向上借文）
    #[tokio::test]
    async fn nothing_produced_still_anchors_marker() {
        let mut session = empty_memory();
        let wrote = store_interrupted_turn(&mut session, "   ", "\n", KIND).await;

        assert!(wrote, "无产出也必须给出提示锚点");
        assert_eq!(session.messages.len(), 1);
        assert_eq!(
            body_text(&only_blocks(&session)),
            "",
            "无产出时正文留空块（渲染锚点）"
        );
    }

    // ── 分支一：尾部已有当轮内容 → 原地追加提示 ──

    /// **核心场景**：主循环 `store_assistant_response` 已落当轮裸正文，
    /// 收尾时必须在**同一条消息**的正文末尾追加提示，既不新增消息、也不丢提示。
    ///
    /// 旧口径（整块精确比对即视为"已落库"直接返回）会让提示**永远写不进去**——
    /// 这正是本次要修的行为。
    #[tokio::test]
    async fn appends_marker_to_existing_tail_message() {
        let mut session = empty_memory();
        seed_plain_assistant(&mut session, "半截正文");

        let wrote = store_interrupted_turn(&mut session, "半截正文", "", KIND).await;

        assert!(wrote, "提示必须被写入（不能因正文已存在就跳过）");
        assert_eq!(session.messages.len(), 1, "必须仍是一条消息（一问一答）");
        let body = body_text(&only_blocks(&session));
        assert!(body.starts_with("半截正文"), "原文不能丢：{body}");
        assert!(!body.contains("回复被中断"), "标记不得进正文：{body}");
    }

    /// 已有消息带思考+正文时，提示只追加到正文块，绝不污染思考块
    #[tokio::test]
    async fn marker_never_pollutes_thinking_block() {
        let mut session = empty_memory();
        super::append_message(
            &mut session,
            Message::Assistant {
                content: Content::Multiple(vec![
                    ContentBlock::Thinking {
                        thinking: "内心独白".to_string(),
                        signature: String::new(),
                    },
                    ContentBlock::Text {
                        text: "半截正文".to_string(),
                    },
                ]),
            },
            "chat",
        );

        store_interrupted_turn(&mut session, "半截正文", "内心独白", KIND).await;

        let blocks = only_blocks(&session);
        match &blocks[0] {
            ContentBlock::Thinking { thinking, .. } => {
                assert!(
                    !thinking.contains("回复被中断"),
                    "思考块不得混入系统提示：{thinking}"
                );
            }
            other => panic!("第一个块应为思考，实际: {other:?}"),
        }
        assert!(!body_text(&blocks).contains("回复被中断"));
    }

    /// 幂等：同一收尾重复调用，提示只追加一次
    #[tokio::test]
    async fn repeated_store_is_idempotent() {
        let mut session = empty_memory();
        seed_plain_assistant(&mut session, "半截正文");

        assert!(store_interrupted_turn(&mut session, "半截正文", "", KIND).await);
        let after_first = session.messages.len();
        let first_body = body_text(&only_blocks(&session));

        let wrote = store_interrupted_turn(&mut session, "半截正文", "", KIND).await;
        assert!(!wrote, "重复收尾不应再次改动");
        assert_eq!(session.messages.len(), after_first, "不得新增消息");
        assert_eq!(
            body_text(&only_blocks(&session)),
            first_body,
            "正文不得被追加两次"
        );
    }
}
