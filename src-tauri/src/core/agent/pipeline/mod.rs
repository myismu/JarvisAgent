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
//! | 1 初始化 | `setup()` | 校验会话占用与沙箱目录、加载配置、创建取消令牌、复杂任务判定、组装 PipelineState |
//! | 2 循环前准备 | `pre_loop()` | 崩溃恢复、注入用户消息、创建 run 记录、决定是否深度思考 |
//! | 3 主循环 | `run_main_loop()` | 调 LLM → 流式解析 → 执行工具 → 结果回写，直到 LLM 不再调工具 |
//! | 4 收尾 | `finalize()` | 检查点快照、保存会话、自动起名、记忆超预算时后台整理、组装 JarvisResult |
//!
//! ### 关于「意图验证」阶段（已作废，勿再按五阶段描述本文件）
//!
//! 这里曾规划过一个独立的第 2 阶段 `validate()`（DANGEROUS 弹权限确认 / UNCLEAR 返回澄清），
//! 它**从未落地**——`run_pipeline_inner()` 是从阶段 1 直接进阶段 2 的。后续演化为：
//! - 意图分类**整体取消**（含曾短暂存在的"规则 → 上下文 → LLM 兜底"三层分类），
//!   `setup()` 内只剩复杂任务布尔检测；
//! - 权限确认**下移到工具执行期逐次审批**：`request_permission()` 由主循环的工具调用处
//!   与 `shell_tools::execution` 直接调用，受 `approval_mode`（`request_approval` /
//!   `auto_approve`）控制，不再有"跑之前先整体预检一遍"的环节。
//!
//! ### 阶段 3 主循环每轮内部子步骤
//! 取消检查 → 循环次数确认 → 上下文压缩 → 历史快照 → 构建请求
//! → API 调用 → 流式处理 → 工具执行 → 反思审查 → 回写历史 → 下一轮
//!
//! 注：这里**没有**「后台通知注入」——后台任务结果不再进会话上下文，
//! 改走前端 `bg-task-done` / `background-failed` 事件提醒，模型需要时用
//! `CheckBackgroundCommand` 自查。这样也消除了 background 注入对
//! 「消息级 user/assistant 交替」的破坏。
//!
//! ### 配套辅助函数（各阶段共用）
//! - 历史准备：`prepare_history_snapshot()` / `prepare_history_snapshot_from_messages()` / `fix_broken_tool_call_pairs()`
//! - 上下文监控：`build_context_estimate()` / `update_context_snapshot()` / `update_provider_usage_snapshot()` / `resolve_max_tokens()`
//! - 请求构建：`build_llm_request()` / `call_api_with_retry()` / `current_tools()`
//! - 流程控制：`request_loop_continuation()` / `compact_if_needed()`
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
use tauri::{Emitter, Manager};

use crate::core::orchestration::agent_runs;
use crate::core::session::{
    append_message, append_message_with_kind, memory::*, normalize_message_ids, pop_message,
    restore_message,
};
use crate::core::tools::*;
use crate::infra::config::config::AgentConfig;
use crate::infra::debug_logger;
use crate::infra::llm::api_client;
use crate::infra::types::error::{AgentError, ApiError};
use crate::infra::types::models::*;

use super::context::*;
use super::stream::{process_stream, StreamConfig};
use super::tools_runner::execute_tool_calls;

mod context_metrics;
mod finalize;
mod helpers;
mod main_loop;
mod recovery;
mod setup;
#[cfg(test)]
mod tests;

use helpers::*;

/// Pipeline 各阶段共享的状态（相当于一次用户请求的“上下文对象”）
///
/// 字段分三批填充：
/// - setup()：app / sid / ctx / 取消令牌 / 配置 / API 客户端 / 系统提示词 / 意图等
/// - pre_loop()：dynamic_context_str / 用户消息展示 / initial_msg_index / should_think / run_id
/// - 主循环与收尾：loop_count / token 统计 / final_answer / 反思计数等
struct PipelineState {
    app: tauri::AppHandle,
    sid: String,
    ctx: Arc<crate::core::runtime::SessionContext>,
    /// 生命周期守卫：正常收尾时释放，异常退出时清理本轮占用。
    execution_lease: crate::core::runtime::ExecutionLease,
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
    /// 本会话的工具开关快照（setup 时从 `SessionContext` 取，整轮复用）。
    ///
    /// 存一份而不是每轮现取：`current_tools()` 是同步函数、快照获取是异步的；
    /// 更重要的是本轮所有"可见性判断"必须用**同一份** —— 否则又会出现
    /// "目录里看不见、调用时却放行"的两套真相。
    tool_filter: crate::core::tools::framework::registry::ToolFilter,
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
    /// 用途仅限**展示/诊断**（回合收尾的日志行、`agent_runs` 汇总里的子代理占比）：
    /// 它**不参与** `sessions.total_*` 累计。子代理（`tools/agent_tools/subagent.rs`）
    /// 有自己独立的循环，每一轮拿到 usage 就经 `session::usage::record_request_usage`
    /// 直写明细与累计——收尾时再把这几个数交一次，就是**同一笔账记两遍**。
    ///
    /// 历史教训：这两个字段曾经的唯一读者是收尾那次 `save_session` 增量；
    /// 而主循环当时也走收尾整轮累加，账本靠"只交子代理那一份"勉强对得上。
    /// 子代理改成逐请求直写后，这层手工补偿就成了纯重复计数，故收尾不再读取它们。
    req_sub_input_tokens: u64,
    req_sub_output_tokens: u64,
    final_answer: String,
    /// 反思审查状态
    reflection_mode: String,
    total_reflections: usize,
    consecutive_reflection_nos: usize,
    /// Plan 看门狗：plan 模式下连续未提交 ProposePlan 的**轮数**。
    ///
    /// 按轮计分，不按工具调用次数：一轮里并行读多少文件都只 +1。
    /// 提交方案或切出 plan 模式即清零。
    plan_total_loops_without_plan: usize,
    /// 本轮因上游失联（流内空闲超时）而中断时的原因说明。
    /// Some 表示属于"运行被打断"，收尾时必须保留现场、不得截断历史。
    interrupted_reason: Option<String>,
    /// 面向用户的状态标注 —— 只存 **i18n key**（气泡下方小字），由中断收尾路径填充。
    ///
    /// 这里曾经直接存中文句子。后端写死文案会让界面切成英文后仍冒中文，
    /// 而且同一个中断类型在"运行期"与"历史重放"两条路径上各写了一份措辞、
    /// 会漂移出"刷新前后说法不一样"。现在只出 key，文案全在前端语言包。
    notice_i18n_key: Option<&'static str>,
    /// 状态标注的补充数据（原样展示、不翻译，如具体错误文本）
    notice_detail: Option<String>,
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
        && !out.iter().any(
            |b| matches!(b, ContentBlock::Thinking { thinking, .. } if !thinking.trim().is_empty()),
        )
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
#[allow(clippy::too_many_arguments)]
pub async fn run_pipeline(
    session_id: String,
    msg: String,
    thinking_override: Option<bool>,
    image_base64_list: Option<Vec<String>>,
    agent_display_mode: Option<String>,
    reflection_mode_override: Option<String>,
    display_msg: Option<String>,
    app: tauri::AppHandle,
    session_manager: tauri::State<'_, crate::core::runtime::SessionManager>,
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
    session_manager: tauri::State<'_, crate::core::runtime::SessionManager>,
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
#[allow(clippy::too_many_arguments)]
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
    session_manager: tauri::State<'_, crate::core::runtime::SessionManager>,
    config_state: tauri::State<'_, crate::infra::config::config::ConfigState>,
) -> Result<JarvisResult, AgentError> {
    // ── 阶段 1：初始化（会话与配置准备 + 复杂任务判定）──
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
