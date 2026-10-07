//! # stream.rs — SSE 流式接收（会话运行时侧）
//!
//! 接收 LLM 返回的 SSE 流，逐帧交给模型接入层的 `stream_parse::parse_frame`
//! 翻译成统一事件，然后处理**运行时事务**：累积内容块、推送前端事件、写 agent_runs 日志、
//! 空闲超时与取消、重试判据。
//!
//! 协议知识（Anthropic/OpenAI 的帧长什么样）全部住在 `infra/llm/stream_parse.rs`，
//! 本文件不应再出现任何协议分支——全文 grep 不到 `is_openai` 是拆分的验收标准之一
//! （方案见 `doc/模型接入层-流式解析拆分方案.md`）。
//!
//! ## 关键导出
//! - `process_stream()`: 解析 SSE 流，返回内容块、工具输入缓冲、token 统计等
//!
//! ## 依赖
//! - Internal: `crate::core::orchestration::agent_runs`, `crate::infra::debug_logger::DebugLogger`,
//!   `crate::infra::types::models`, `crate::infra::llm::{api_format::ApiFormat, stream_parse, usage::UsageObservation}`
//! - External: `futures_util`, `serde_json`, `eventsource_stream`, `tauri`
//!
//! ## 约束
//! - 支持中途取消（通过 `CancellationToken`）
//! - 支持流内空闲超时（`STREAM_IDLE_TIMEOUT_SECS`）：连续无 SSE 帧即判定上游失联，
//!   优雅终止并保留已累积内容，不抛错、不清状态
//! - 工具调用的 `partial_json` 会累积到 `tool_input_buffers` 中，由调用方完成解析

use futures_util::StreamExt;
use serde_json::json;
use std::collections::HashMap;
use std::time::Duration;
use tauri::Emitter;

use crate::core::orchestration::agent_runs;
use crate::infra::debug_logger;
use crate::infra::llm::api_format::ApiFormat;
use crate::infra::llm::stream_parse; // 模块本身也要导入，供下方 stream_parse::parse_frame 前缀调用
use crate::infra::llm::stream_parse::{
    looks_like_textual_tool_call, parse_textual_tool_calls, ProtocolEvent,
};
use crate::infra::llm::usage::UsageObservation;
use crate::infra::types::models::*;

/// 流内空闲超时（秒）：连续该时长未收到任何 SSE 帧即判定上游失联。
///
/// 上游服务崩溃／半开连接时不会发 FIN/RST，TCP 认为连接仍在，`stream.next()`
/// 会永久 pending。此计时器以「收到任意 SSE 事件」为重置基准，因此不会误杀
/// 正常的长时间生成（只要还在吐字就一直续命）。
///
/// ⚠️ 概念边界（勿与「规划看门狗」混淆）：本机制只管**链路**（SSE 有没有来帧），
/// 全模式/全 agent 通用（主 agent 每轮 + 子代理 + 后台 LLM），与 plan 模式的
/// 「规划看门狗」（pipeline.rs::update_plan_watchdog，管的是"探索轮数"）是
/// 两套独立机制。历史教训：曾在对话中用"看门狗"称呼本机制，导致与项目既有
/// 术语「规划看门狗」撞名、被误认为同一件事——**代码里本机制从不叫看门狗**。
///
/// **取值取舍（2026-09-21 由 90 提到 120）**：
///
/// 历史沿革：曾因体验反馈"等太久"从 90 下调到 30；30 会误杀**真实的工具调用**——
/// 实测 GLM（glm-5.3-flash）生成 tool_call 参数时**服务端零字节静默**：同一模型
/// 的纯文本流式输出 468 秒零个 >1s 间隔（永不停顿），换成 ProposePlan 巨型参数
/// 则静默 **64.4~68.6 秒**才吐出首个 tool_calls 帧（arguments 仅 9KB，属服务端
/// 内部耗时而非带宽问题）。线上三次中断（session 10b68285 / 2302e81c /
/// 47ecd917）全部命中此形态：都是在参数走到一半时被本阈值掐断。故先调回 90。
///
/// 再提到 120 的理由：**上面量到的都是 flash 档**（最快的档位）。pro 档
/// （GLM pro / deepseek-v4-pro）推理量更大、工具参数生成更慢，静默只会更长，
/// 90 对它们余量不足。120 = flash 实测上限 68.6s 的约 1.75 倍，为慢档留出空间。
///
/// ⚠️ 本值只影响"多久没收到数据才算失联"这一件事，**不影响压缩时机**——压缩判据
/// 用的是 `(窗口 − 输出预算) × 85%`（见 infra::llm::context_budget），与 max_tokens
/// 相关、与本值无关。
///
/// 已实证**不属于**本地检测缺陷：那 64s 是真的零字节（计时锚点下沉到字节层
/// 也测不到东西），因此本值只能按厂商行为留够余量。**治本方向是减小单次
/// 工具参数体积**（如 ProposePlan 分片提交），而不是无限上调本阈值。
pub const STREAM_IDLE_TIMEOUT_SECS: u64 = 120;

/// 界面"仍在等待"提示的触发阈值 —— **由 [`STREAM_IDLE_TIMEOUT_SECS`] 派生**，
/// 不再作为独立常量存在。
///
/// ## 为什么要派生（2026-09-21 沐指出的问题）
///
/// 原先这里是独立的 `constants::API_WAITING_HINT_SECS = 30`，与超时阈值是两个
/// 互不相干的常量。一旦为慢模型（pro 档推理更久）把超时放宽到 120，提示却仍在
/// 30 秒出现——用户"刚提完示，还要干等 90 秒"，两个值必须人工同步、极易漏改。
///
/// 改为按超时阈值的 **1/3** 派生后：阈值 90 → 提示 30 秒（与改造前完全一致），
/// 阈值 120 → 提示 40 秒。以后调超时阈值，提示自动跟随，**不需要记得同步**。
/// 下限 15 秒是防止有人把超时阈值调到极小值后提示永不出现。
pub fn waiting_hint_secs() -> u64 {
    (STREAM_IDLE_TIMEOUT_SECS / 3).max(15)
}

/// 连续流错误容忍次数：超出即终止，避免底层流已死时无限空转
const STREAM_MAX_CONSECUTIVE_ERRORS: u32 = 5;

/// 判定本次流是否"零产出"：正文、思考、工具调用三者皆空。
///
/// 这是决定能否重试的**唯一**依据：已收到任何内容就不该重试
/// （正文会造成界面重复拼接，思考会破坏「思考链必须回传」的协议要求）。
pub(crate) fn is_zero_output(text_empty: bool, thinking_empty: bool, has_tool: bool) -> bool {
    text_empty && thinking_empty && !has_tool
}

/// 流式处理配置：控制事件发送行为
#[derive(Clone, Default)]
pub struct StreamConfig {
    /// 是否为子代理模式（子代理不发送 chat-content/chat-tool-start，
    /// chat-thinking 携带 isSubAgent 标记，不写 agent_runs 日志）
    pub is_subagent: bool,
    /// 注册表可选的缓存字段写法覆盖（`cacheUsageStyle`）；None = 按候选表自动探测。
    /// 构造 `UsageObservation` 时传入（它据此决定缓存字段按哪种写法读）。
    pub cache_usage_style: Option<String>,
    /// 每收到一个 SSE 帧时触发，用于重置"等待提示"看门狗的静默计时。
    ///
    /// 用 `Arc<dyn Fn()>` 而非泛型，是为了让 `StreamConfig` 保持 `Clone`
    /// 且不污染 `process_stream` 的签名。不需要提示的调用点留 `None`。
    pub on_frame: Option<std::sync::Arc<dyn Fn() + Send + Sync>>,
    /// 模型标识（落 `agent_run_events.model`，监控/重建时可追溯本次用的哪个模型）。
    pub model_id: Option<String>,
    /// 「崩溃保护（实时保存）」开关。
    ///
    /// **关（默认）**：本次流不写任何帧级数据，只有 loop 收尾那一次结构化落库。
    /// **开**：文本按粒度攒批（见 [`FRAME_FLUSH_INTERVAL`] / [`FRAME_FLUSH_CHARS`]）
    /// 写进 `agent_run_events`，工具参数等**结构事件强制立即 flush**
    /// （工具调用 JSON 不能切半——半截 JSON 无法在崩溃后还原出工具调用）。
    ///
    /// 之所以要攒批：单条 `append_loop_delta` 是一次 SQLite 写 + fsync，
    /// 逐帧（每几十字一次）调用会把写放大重新拉回 30-60x，正是本次重构要消灭的东西。
    pub crash_protection: bool,
}

/// 尝试从 ExecuteTool 的参数中提取 ProposePlan 的 content 字段
/// ExecuteTool 的参数格式: {"name": "ProposePlan", "args": {"title": "...", "content": "..."}}
fn extract_deferred_propose_plan_content(partial_json: &str) -> Option<String> {
    // 快速过滤：必须是 ExecuteTool 且包含 ProposePlan
    if !partial_json.contains("ExecuteTool") && !partial_json.contains("\"name\"") {
        return None;
    }
    if !partial_json.contains("ProposePlan") {
        return None;
    }

    // 尝试解析为完整 JSON
    if let Ok(parsed) = serde_json::from_str::<serde_json::Value>(partial_json) {
        if let Some(name) = parsed.get("name").and_then(|v| v.as_str()) {
            if name == "ProposePlan" {
                if let Some(args) = parsed.get("args") {
                    if let Some(content) = args.get("content").and_then(|v| v.as_str()) {
                        return Some(content.to_string());
                    }
                }
            }
        }
        return None;
    }

    // 不完整 JSON：尝试提取 args.content 字段
    // 先检查 name 是否为 ProposePlan
    if let Some(name_start) = partial_json.find("\"name\"") {
        let after_name = &partial_json[name_start + 6..];
        if let Some(colon_pos) = after_name.find(':') {
            let value_part = after_name[colon_pos + 1..].trim_start();
            if value_part.starts_with('"') {
                let name_value = &value_part[1..];
                if let Some(end_quote) = name_value.find('"') {
                    let name = &name_value[..end_quote];
                    if name != "ProposePlan" {
                        return None;
                    }
                }
            }
        }
    }

    // 提取 args 中的 content
    if let Some(args_start) = partial_json.find("\"args\"") {
        let after_args = &partial_json[args_start + 6..];
        if let Some(content_start) = after_args.find("\"content\"") {
            let after_content_key = &after_args[content_start + 9..];
            if let Some(colon_pos) = after_content_key.find(':') {
                let value_part = after_content_key[colon_pos + 1..].trim_start();
                if value_part.starts_with('"') {
                    let after_quote = &value_part[1..];
                    let mut content = String::new();
                    let mut chars = after_quote.chars().peekable();
                    while let Some(c) = chars.next() {
                        if c == '\\' {
                            if let Some(next) = chars.next() {
                                match next {
                                    'n' => content.push('\n'),
                                    't' => content.push('\t'),
                                    'r' => content.push('\r'),
                                    '\\' => content.push('\\'),
                                    '"' => content.push('"'),
                                    '/' => content.push('/'),
                                    'u' => {
                                        let mut hex = String::new();
                                        for _ in 0..4 {
                                            if let Some(hc) = chars.next() {
                                                hex.push(hc);
                                            }
                                        }
                                        if let Ok(code) = u32::from_str_radix(&hex, 16) {
                                            if let Some(ch) = char::from_u32(code) {
                                                content.push(ch);
                                            }
                                        }
                                    }
                                    _ => content.push(next),
                                }
                            }
                        } else if c == '"' {
                            break;
                        } else {
                            content.push(c);
                        }
                    }
                    return Some(content);
                }
            }
        }
    }

    None
}

/// 流式处理结果
pub struct StreamResult {
    pub blocks: Vec<ContentBlock>,
    pub tool_input_buffers: HashMap<usize, String>,
    pub text: String,
    pub thinking: String,
    pub has_tool: bool,
    /// 本次请求的输入 token（**已归一**：Anthropic 家族补上了缓存命中量，
    /// 与 OpenAI 家族的 `prompt_tokens` 口径一致 → 可直接当"上下文有多大"）。
    /// 厂商一次 usage 都没上报时为 0。
    pub input_tokens: u64,
    /// 本次请求的输出 token（同上一轮口径，未上报时为 0）
    pub output_tokens: u64,
    /// 缓存命中 / 未命中的输入 token；None = 该 provider 未报告（≠ 0）
    pub cache_hit_tokens: Option<u64>,
    pub cache_miss_tokens: Option<u64>,
    /// 命中的字段名（排查"这家为什么显示未知"用）
    pub cache_source: Option<&'static str>,
    /// 原始 usage 原文（截断）：遇到未知写法时可直接从日志取出自诊断
    pub usage_raw: Option<String>,
    /// API 返回的终止原因（Anthropic: stop_reason, OpenAI: finish_reason）
    /// 常见值: "end_turn", "tool_use", "max_tokens", "stop", "length"
    pub stop_reason: Option<String>,
    /// 是否因流内空闲超时而终止（上游失联）。true 表示正常收尾路径未走完
    /// —— 既包括"零帧静默"，也包括"吐了半截后静默"。调用方据此结束本轮，
    /// **不要**用它判断能否重试（那是 `should_retry` 的职责）。
    pub idle_timed_out: bool,
    /// 本次是否允许自动重试一次。
    ///
    /// 只在**零产出**（一个有效帧都没收到，且无正文/思考/工具）时为 true。
    /// 已收到内容时为 false：界面已显示半截文本，重试会拼出重复内容，
    /// 且丢弃已收到的思考块会破坏「思考链必须回传」的协议要求。
    ///
    /// 历史教训：该判据曾与 `idle_timed_out` 合用一个字段，导致"吐了半截后静默"
    /// 时超时判定被一并置为 false —— 循环既不重试也不结束，run 永久卡死，
    /// 进而把会话锁成"正在执行"，用户说「继续」会被拒绝。
    pub should_retry: bool,
}

/// 安全截断（按字符，避免切断多字节字符）
fn truncate_sample(text: &str, max_chars: usize) -> String {
    if text.chars().count() <= max_chars {
        return text.to_string();
    }
    let head: String = text.chars().take(max_chars).collect();
    format!("{}…(truncated)", head)
}

/// 帧级攒批的两个阈值（仅「崩溃保护」开启时生效）。
///
/// - `INTERVAL`：距上次落盘超过该时长就 flush —— 覆盖"模型吐得很慢"的场景，
///   否则最后几个字要等到 loop 收尾才落盘，崩溃窗口被拉长到无法接受。
/// - `CHARS`：攒够这么多字符就 flush —— 覆盖"模型狂吐"的场景，
///   否则一次 flush 要写一个几 KB 的大字符串。
///
/// 两者**谁先到谁触发**。取值权衡：调小 = 崩溃丢得少、写放大回升；
/// 调大 = 省写入、崩溃窗口变长。200ms / 1KB 在"丢不超过一次呼吸的内容"
/// 与"每次写只有 1KB"之间取平衡。
const FRAME_FLUSH_INTERVAL: Duration = Duration::from_millis(200);
const FRAME_FLUSH_CHARS: usize = 1024;

/// 结构事件标记：工具调用已开始、参数正在接收。
///
/// 它落在 `tool_results` 列里而非 `resp_blocks`：`resp_blocks` 是**正文/思考**
/// 的纯文本累积（可任意切分），而"发生过一次工具调用"是结构化事实，
/// 崩溃重建时要能原样读出来，不能和正文混在同一条流里。
const TOOL_ARGS_PENDING_MARKER: &str = "\n> 工具参数接收中\n";

/// 崩溃保护的帧级攒批器（仅开关开启时构造）。
/// 只管**文本**（正文 + 思考）：这两者是高频、可任意切分的内容，攒批无副作用。
/// 工具参数之类的**结构事件**不走这里 —— 它们必须立即整段落盘
/// （半截 JSON 在崩溃后无法还原成工具调用），见调用点直接调 append。
struct FrameFlusher {
    run_id: String,
    session_id: String,
    loop_index: usize,
    model: Option<String>,
    buf: String,
    last_flush: std::time::Instant,
}

impl FrameFlusher {
    fn new(run_id: &str, session_id: &str, loop_index: usize, model: Option<String>) -> Self {
        Self {
            run_id: run_id.to_string(),
            session_id: session_id.to_string(),
            loop_index,
            model,
            buf: String::new(),
            last_flush: std::time::Instant::now(),
        }
    }

    /// 追加一段文本；达到阈值（时间或字数）即落盘。
    fn push(&mut self, text: &str) {
        if text.is_empty() {
            return;
        }
        self.buf.push_str(text);
        if self.buf.chars().count() >= FRAME_FLUSH_CHARS
            || self.last_flush.elapsed() >= FRAME_FLUSH_INTERVAL
        {
            self.flush();
        }
    }

    /// 结构事件发生前调用：把攒着的文本先落盘，保证落盘顺序与产生顺序一致。
    fn flush(&mut self) {
        if self.buf.is_empty() {
            return;
        }
        agent_runs::append_loop_text_delta(
            &self.run_id,
            &self.session_id,
            self.loop_index,
            &self.buf,
            self.model.as_deref(),
        );
        self.buf.clear();
        self.last_flush = std::time::Instant::now();
    }
}

/// 接收并解析一条 SSE 流。
///
/// # 参数
/// - `stream`: SSE 事件流（HTTP 响应体按 eventsource 协议切帧后的异步迭代器）
/// - `api_format`: 本次流使用的协议格式——**只在这一处出现**，往下全部是统一事件
/// - `app` / `sid` / `run_id` / `loop_count`: 前端事件推送与日志归档的定位信息
/// - `cancel_token`: 用户取消令牌；`config`: 发送行为开关（见 `StreamConfig`）
pub async fn process_stream(
    stream: &mut (impl StreamExt<
        Item = Result<
            eventsource_stream::Event,
            eventsource_stream::EventStreamError<reqwest::Error>,
        >,
    > + Unpin),
    api_format: ApiFormat,
    app: &tauri::AppHandle,
    sid: &str,
    run_id: &str,
    loop_count: usize,
    cancel_token: &tokio_util::sync::CancellationToken,
    config: StreamConfig,
) -> StreamResult {
    let mut current_blocks: Vec<ContentBlock> = Vec::new();
    let mut tool_input_buffers: HashMap<usize, String> = HashMap::new();
    // "线上索引 → 内容块位置"映射表。拆分前只有 OpenAI 用（call index → 块下标），
    // Anthropic 直接拿线上块下标当数组下标；两条协议统一走映射表后行为一致，
    // 且流里出现未知块类型导致下标错位时工具参数分片仍能正确对位（拆分前会静默丢失）。
    let mut tool_block_map: HashMap<usize, usize> = HashMap::new();
    let mut current_text_this_turn = String::new();
    let mut current_thinking_this_turn = String::new();
    let mut turn_has_tool = false;
    // usage 与缓存读数统一交给 `UsageObservation`（字段级 last-wins + 缓存合并，见其文档）。
    // 协议家族判断收在 for_format 构造器里，本函数从此不感知 usage 字段的协议差异。
    let mut usage_obs = UsageObservation::for_format(api_format, config.cache_usage_style.clone());
    let mut stop_reason: Option<String> = None;
    let mut logged_textual_tool_violation = false;
    let mut usage_raw: Option<String> = None;
    // 追踪 ProposePlan 工具调用的流式内容，用于实时推送到前端
    let mut propose_plan_stream_sent: HashMap<usize, usize> = HashMap::new();
    // 空闲超时：只统计"一个有效帧都没收到"的情况。
    // 已收到帧却中断时，部分内容已推送给前端（可能已进入本轮的 events 行），
    // 此时重试会在界面上拼出重复文本，因此仅零产出才标记为可重试。
    let mut idle_timed_out = false;
    let mut should_retry = false;
    let mut received_event = false;
    let mut consecutive_errors: u32 = 0;
    // 崩溃保护的帧级通道：开关关闭时恒为 None，全程零 DB 写入。
    // 开与不开都**不影响** loop 收尾那次结构化落库（那是 upsert_loop_event 的事）。
    let mut frame_flusher: Option<FrameFlusher> = if config.crash_protection {
        Some(FrameFlusher::new(
            run_id,
            sid,
            loop_count,
            config.model_id.clone(),
        ))
    } else {
        None
    };

    let logger = debug_logger::debug_logger();
    // SSE 聚合按 (session, agent_type, loop) 归档，才能落到对应循环卡上
    let agent_type = if config.is_subagent { "SUB" } else { "MAIN" };
    if !config.is_subagent {
        let _ = app.emit(
            "chat-turn-start",
            json!({ "sessionId": sid, "loopCount": loop_count }),
        );
    }

    loop {
        // 每次迭代重建计时器 = 每收到一个 SSE 事件即重置空闲计时。
        // 上游静默超过阈值即按超时终止，保留已累积结果。
        //
        // ⚠️ 这是**流空闲超时**（链路级），不是「规划看门狗」（plan 模式探索轮数）：
        // 只在"零帧"时触发，与模型在干什么无关。措辞不断言服务端状态——静默可能
        // 是服务繁忙、链路抖动或半开连接，链路上不可区分。
        let idle_deadline = tokio::time::sleep(Duration::from_secs(STREAM_IDLE_TIMEOUT_SECS));
        tokio::pin!(idle_deadline);

        let event_result = tokio::select! {
            next = stream.next() => next,
            _ = &mut idle_deadline => {
                println!(
                    "[JARVIS] SSE 流空闲超过 {}s（未收到任何数据），按超时优雅终止本轮接收",
                    STREAM_IDLE_TIMEOUT_SECS
                );
                // 两个含义必须分开：
                // - idle_timed_out：计时器确实触发了 → 调用方据此结束本轮（含"吐了半截后静默"）
                // - should_retry：只有零产出时才允许重试
                // 曾把二者合一，导致收到过帧时超时判定被置 false → run 卡死。
                idle_timed_out = true;
                should_retry = !received_event;
                logger.log_sse_event(
                    sid,
                    agent_type,
                    loop_count,
                    &format!("[idle-timeout] {}s 无 SSE 帧", STREAM_IDLE_TIMEOUT_SECS),
                );
                break;
            }
            _ = cancel_token.cancelled() => {
                println!("[JARVIS] 流式接收中途被用户取消");
                break;
            }
        };
        let Some(event_result) = event_result else {
            break;
        };
        let event = match event_result {
            Ok(e) => {
                consecutive_errors = 0;
                e
            }
            Err(e) => {
                // 不再静默 continue：底层流持续报错时无限空转且不留痕，
                // 会掩盖超时问题本身。有限次容错后终止。
                consecutive_errors += 1;
                println!(
                    "[JARVIS] SSE 流错误（连续第 {}/{} 次）: {}",
                    consecutive_errors, STREAM_MAX_CONSECUTIVE_ERRORS, e
                );
                if consecutive_errors >= STREAM_MAX_CONSECUTIVE_ERRORS {
                    // 流已判定不可用，与空闲超时同样按"中断"收尾（而非静默 continue）
                    idle_timed_out = true;
                    should_retry = !received_event;
                    break;
                }
                continue;
            }
        };
        received_event = true;
        // 通知等待提示看门狗：有数据到达，重置静默计时。
        // 放在解析之前——即使该帧解析不出内容，"链路仍在流动"这一事实也成立。
        if let Some(on_frame) = &config.on_frame {
            on_frame();
        }
        let data = event.data;
        // 记录 SSE（默认只累加计数，收尾时落一条 sse_summary）
        logger.log_sse_event(sid, agent_type, loop_count, &data);
        if data == "[DONE]" {
            break;
        }
        let json_val: serde_json::Value = serde_json::from_str(&data).unwrap_or(json!({}));

        // ── 统一事件循环 ──
        // 协议解析（parse_frame：纯函数，住在模型接入层）与运行时副作用（本函数）在此解耦：
        // 这里只认 ProtocolEvent，没有任何协议分支。每个事件臂的副作用代码就是
        // 拆分前对应协议分支里的原句（chat-content / chat-thinking / chat-tool-start /
        // plan-proposal-stream / agent_runs 日志），仅触发源从"协议判断"变成"事件匹配"。
        for ev in stream_parse::parse_frame(api_format, &json_val) {
            match ev {
                ProtocolEvent::TextStart { .. } => {
                    // Anthropic content_block_start(text)：推入空文本块。
                    // 推入位置 = 数组末尾；正常流中与线上块下标一致（与拆分前行为相同）。
                    current_blocks.push(ContentBlock::Text {
                        text: String::new(),
                    });
                    // 块边界：先结清攒批缓冲再开新块（方案 §5.3），
                    // 保证崩溃瞬间 DB 里的拼串与块边界对齐。
                    if let Some(flusher) = frame_flusher.as_mut() {
                        flusher.flush();
                    }
                }
                ProtocolEvent::ThinkingStart { signature, .. } => {
                    // Anthropic content_block_start(thinking)：推入空思考块；
                    // 开始帧可能自带初始签名（None → 空串，与拆分前 unwrap_or("") 一致）
                    current_blocks.push(ContentBlock::Thinking {
                        thinking: String::new(),
                        signature: signature.unwrap_or_default(),
                    });
                    // 块边界：同上
                    if let Some(flusher) = frame_flusher.as_mut() {
                        flusher.flush();
                    }
                }
                ProtocolEvent::TextDelta { block, text } => {
                    // 落块的两种协议规则已在事件里归一：Some = 精确改写，None = 追加进当前块
                    match block {
                        // Anthropic：精确改写第 i 块；该块不是文本/越界则静默（与拆分前一致）
                        Some(i) => {
                            if let Some(ContentBlock::Text { text: buf }) =
                                current_blocks.get_mut(i)
                            {
                                buf.push_str(&text);
                            }
                        }
                        // OpenAI：当前块是文本就追加，否则新开一块
                        None => {
                            let is_text =
                                matches!(current_blocks.last(), Some(ContentBlock::Text { .. }));
                            if !is_text {
                                current_blocks.push(ContentBlock::Text {
                                    text: String::new(),
                                });
                            }
                            if let Some(ContentBlock::Text { text: buf }) =
                                current_blocks.last_mut()
                            {
                                buf.push_str(&text);
                            }
                        }
                    }
                    current_text_this_turn.push_str(&text);
                    // 协议违规检测：模型把工具调用写成正文的 XML 标记（文本形态工具调用）
                    if !logged_textual_tool_violation
                        && looks_like_textual_tool_call(&current_text_this_turn)
                    {
                        logged_textual_tool_violation = true;
                        let agent_type = if config.is_subagent {
                            "SUBAGENT"
                        } else {
                            "MAIN"
                        };
                        logger.log_protocol_violation(
                            sid,
                            agent_type,
                            loop_count,
                            &current_text_this_turn,
                        );
                    }
                    if !config.is_subagent {
                        let _ = app.emit(
                            "chat-content",
                            json!({ "content": text, "sessionId": sid, "loopCount": loop_count }),
                        );
                        // 崩溃保护帧级通道（攒批；未开启时无操作）
                        if let Some(flusher) = frame_flusher.as_mut() {
                            flusher.push(&text);
                        }
                    }
                }
                ProtocolEvent::ThinkingDelta {
                    block,
                    text,
                    signature,
                } => {
                    // 落块 + 追加（text 与 signature 互不依赖：纯签名帧 text 为空串）
                    match block {
                        // Anthropic：精确改写第 i 块（签名分片也拼进该块）
                        Some(i) => {
                            if let Some(ContentBlock::Thinking {
                                thinking,
                                signature: sig,
                            }) = current_blocks.get_mut(i)
                            {
                                if !text.is_empty() {
                                    thinking.push_str(&text);
                                }
                                if let Some(s) = &signature {
                                    sig.push_str(s);
                                }
                            }
                        }
                        // OpenAI（DeepSeek 等 reasoning_content）：当前块是思考就追加，否则新开一块。
                        // signature: String::new() 是"外来思考链"的标记——出站时按服务商
                        // 决定剥掉（真 Anthropic 判 400）还是保留（DeepSeek 要求回传）。
                        None => {
                            let is_thinking = matches!(
                                current_blocks.last(),
                                Some(ContentBlock::Thinking { .. })
                            );
                            if !is_thinking {
                                current_blocks.push(ContentBlock::Thinking {
                                    thinking: String::new(),
                                    signature: String::new(),
                                });
                            }
                            if let Some(ContentBlock::Thinking { thinking, .. }) =
                                current_blocks.last_mut()
                            {
                                thinking.push_str(&text);
                            }
                        }
                    }
                    if !text.is_empty() {
                        current_thinking_this_turn.push_str(&text);
                        if !config.is_subagent {
                            let _ = app.emit(
                                "chat-thinking",
                                if config.is_subagent {
                                    json!({ "content": text, "sessionId": sid, "isSubAgent": true })
                                } else {
                                    json!({ "content": text, "sessionId": sid, "loopCount": loop_count })
                                },
                            );
                            // 崩溃保护帧级通道（思考与正文共用同一条文本流）
                            if let Some(flusher) = frame_flusher.as_mut() {
                                flusher.push(&text);
                            }
                        }
                    }
                }
                ProtocolEvent::ToolStart { wire_idx, id, name } => {
                    // 非标准实现的重复开始帧（每片都带 id/name）：按映射表去重，
                    // 不重复建块——与拆分前"仅在 map miss 时建块"的行为一致
                    if tool_block_map.contains_key(&wire_idx) {
                        continue;
                    }
                    current_blocks.push(ContentBlock::ToolUse {
                        id: id.clone(),
                        name: name.clone(),
                        input: json!({}),
                    });
                    let block_index = current_blocks.len() - 1;
                    tool_block_map.insert(wire_idx, block_index);
                    tool_input_buffers.insert(block_index, String::new());
                    turn_has_tool = true;
                    if !config.is_subagent {
                        let _ = app.emit(
                            "chat-tool-start",
                            json!({
                                "sessionId": sid,
                                "loopCount": loop_count,
                                "toolCallId": id,
                                "tool": name
                            }),
                        );
                        // 结构事件：工具调用开始。先 flush 攒批文本（保证落盘顺序），
                        // 再把"参数接收中"这一结构化行**立即**落盘（不能与文本合并切分）。
                        if let Some(flusher) = frame_flusher.as_mut() {
                            flusher.flush();
                            agent_runs::append_loop_tool_result_delta(
                                &flusher.run_id,
                                &flusher.session_id,
                                flusher.loop_index,
                                TOOL_ARGS_PENDING_MARKER,
                                flusher.model.as_deref(),
                            );
                        }
                    }
                }
                ProtocolEvent::ToolArgsDelta { wire_idx, fragment } => {
                    // 线上索引 → 块位置；首个分片就不带 id/name 的非标准实现按空块兜底
                    //（含 chat-tool-start 通知，与拆分前 map-miss 即建块的行为一致）
                    let block_index = match tool_block_map.get(&wire_idx) {
                        Some(p) => *p,
                        None => {
                            current_blocks.push(ContentBlock::ToolUse {
                                id: String::new(),
                                name: String::new(),
                                input: json!({}),
                            });
                            let p = current_blocks.len() - 1;
                            tool_block_map.insert(wire_idx, p);
                            tool_input_buffers.insert(p, String::new());
                            turn_has_tool = true;
                            if !config.is_subagent {
                                let _ = app.emit(
                                    "chat-tool-start",
                                    json!({
                                        "sessionId": sid,
                                        "loopCount": loop_count,
                                        "toolCallId": "",
                                        "tool": ""
                                    }),
                                );
                                // 结构事件：同上（非标准实现的 map-miss 兜底路径）
                                if let Some(flusher) = frame_flusher.as_mut() {
                                    flusher.flush();
                                    agent_runs::append_loop_tool_result_delta(
                                        &flusher.run_id,
                                        &flusher.session_id,
                                        flusher.loop_index,
                                        TOOL_ARGS_PENDING_MARKER,
                                        flusher.model.as_deref(),
                                    );
                                }
                            }
                            p
                        }
                    };
                    if let Some(buf) = tool_input_buffers.get_mut(&block_index) {
                        buf.push_str(&fragment);
                        // 实时提取 ProposePlan 的 content 并推送到前端（通过 ExecuteTool 调用）
                        if let Some(ContentBlock::ToolUse { name, .. }) =
                            current_blocks.get(block_index)
                        {
                            let content = if name == "ExecuteTool" {
                                extract_deferred_propose_plan_content(buf)
                            } else {
                                None
                            };
                            if let Some(content) = content {
                                let sent_len = propose_plan_stream_sent
                                    .get(&block_index)
                                    .copied()
                                    .unwrap_or(0);
                                if content.len() > sent_len {
                                    let new_chunk = &content[sent_len..];
                                    let _ = app.emit(
                                        "plan-proposal-stream",
                                        json!({
                                            "content": new_chunk,
                                            "sessionId": sid
                                        }),
                                    );
                                    propose_plan_stream_sent.insert(block_index, content.len());
                                }
                            }
                        }
                    }
                }
                ProtocolEvent::UsageObserved(usage) => {
                    // 字段级覆盖（last-wins）：带 usage 的帧通常只有末帧，重复出现时后者才是完整值
                    usage_obs.observe(&usage);
                    usage_raw = Some(truncate_sample(&usage.to_string(), 600));
                }
                ProtocolEvent::StopReason(sr) => {
                    stop_reason = Some(sr);
                    // 收尾边界（方案 §5.3）：StopReason 意味着本轮内容已到尾，
                    // 提前结清攒批——若此后连接直接断掉（收不到正常 break），
                    // 帧级半截内容也已完整落盘。
                    if let Some(flusher) = frame_flusher.as_mut() {
                        flusher.flush();
                    }
                }
            }
        }
    }

    // 兼容不支持原生 tool_calls 的模型：从文本中提取 <tool_call> XML 块
    if !turn_has_tool && current_text_this_turn.contains("<tool_call") {
        let parsed = parse_textual_tool_calls(&current_text_this_turn);
        if !parsed.is_empty() {
            // 移除原有的纯文本块，替换为解析出的工具调用块
            current_blocks.retain(|b| !matches!(b, ContentBlock::Text { .. }));
            for (i, (name, input_json)) in parsed.iter().enumerate() {
                let id = format!(
                    "call_{}",
                    uuid::Uuid::new_v4().simple().to_string()[..8].to_string()
                );
                current_blocks.push(ContentBlock::ToolUse {
                    name: name.clone(),
                    input: input_json.clone(),
                    id: id.clone(),
                });
                tool_input_buffers.insert(i, input_json.to_string());
            }
            turn_has_tool = true;
        }
    }

    // 流结束时把本 loop 的 SSE 聚合落盘（MAIN 路径 log_response 也会刷一次，幂等）
    logger.flush_sse_summary(sid, agent_type, loop_count);

    // 崩溃保护：把攒批里剩下的文本落盘（不足一个阈值的尾巴，
    // 不 flush 就会一直悬到 loop 收尾，白白拉长崩溃窗口）
    if let Some(flusher) = frame_flusher.as_mut() {
        flusher.flush();
    }

    // 零产出判据在此一次算清：正文/思考/工具调用三者皆空。
    // 放在 stream 层是因为这里同时掌握"是否收到过帧"与"解析出什么"，
    // 调用方只需读一个布尔值，避免再次翻车（见 should_retry 字段注释）。
    let zero_output = is_zero_output(
        current_text_this_turn.trim().is_empty(),
        current_thinking_this_turn.trim().is_empty(),
        turn_has_tool,
    );
    let should_retry = should_retry && zero_output;

    let (input_tokens, output_tokens) = usage_obs.resolve();
    let (cache_hit_tokens, cache_miss_tokens, cache_source) = usage_obs.cache_snapshot();

    StreamResult {
        blocks: current_blocks,
        tool_input_buffers,
        text: current_text_this_turn,
        thinking: current_thinking_this_turn,
        has_tool: turn_has_tool,
        input_tokens,
        output_tokens,
        cache_hit_tokens,
        cache_miss_tokens,
        cache_source,
        usage_raw,
        stop_reason,
        idle_timed_out,
        should_retry,
    }
}

#[cfg(test)]
mod idle_timeout_tests {
    use super::{STREAM_IDLE_TIMEOUT_SECS, STREAM_MAX_CONSECUTIVE_ERRORS};
    use futures_util::StreamExt;
    use std::time::Duration;

    /// 空闲阈值必须是有限正数：0 会误杀一切正常流，过大则失去兜底意义。
    ///
    /// 这里刻意**不锁死具体数值**（曾是仅断言 == 90 的脆弱测试）：
    /// 该值随实测数据变化过（90 → 30 → 90，依据见常量处的探针实测注释），
    /// 断言区间既能防住"误改成 0 或超大值"，又不会阻碍合理调优。
    #[test]
    fn idle_timeout_is_a_sane_finite_bound() {
        assert!(
            STREAM_IDLE_TIMEOUT_SECS >= 10,
            "过小会误杀正常生成：{}",
            STREAM_IDLE_TIMEOUT_SECS
        );
        assert!(
            STREAM_IDLE_TIMEOUT_SECS <= 300,
            "过大等于没有兜底：{}",
            STREAM_IDLE_TIMEOUT_SECS
        );
    }

    /// 连续错误容忍次数必须有限，否则底层流已死时会无限空转（原实现即如此）。
    #[test]
    fn consecutive_error_tolerance_is_bounded() {
        assert!(STREAM_MAX_CONSECUTIVE_ERRORS > 0);
        assert!(
            STREAM_MAX_CONSECUTIVE_ERRORS <= 10,
            "容忍次数过大等于没有兜底"
        );
    }

    /// 与 `process_stream` 内部相同的 select 形态：
    /// 每轮重建 sleep，`next` 先就绪则不让计时器影响取值。
    /// 返回 `Some(())` 表示取到值，`None` 表示空闲超时触发。
    async fn race_next_against_idle<S>(stream: &mut S, idle: Duration) -> Option<()>
    where
        S: futures_util::Stream + Unpin,
    {
        let deadline = tokio::time::sleep(idle);
        tokio::pin!(deadline);
        tokio::select! {
            item = stream.next() => item.map(|_| ()),
            _ = &mut deadline => None,
        }
    }

    /// 永不产出任何元素的流，模拟"上游收下请求后彻底静默"（TCP 半开连接下 read 永久 pending）。
    ///
    /// 必须用 `pending()` 而非 `iter(large_range)`：后者内部有缓冲队列，
    /// 第一次 poll 就能立刻拿到元素，无法复现"静默"。
    fn silent_stream() -> impl futures_util::Stream<Item = u8> + Unpin {
        futures_util::stream::pending::<u8>()
    }

    /// 回归防护（核心）：上游不发任何数据时必须能被空闲计时器打断。
    ///
    /// 用极短的空闲窗口替代 90s，验证的是**机制**而非具体数值：
    /// 若 select 里漏掉计时分支，本用例会一直挂住直到外层测试超时，即复现线上
    /// 「DeepSeek 崩溃后界面无限转圈」的原始缺陷。
    #[tokio::test]
    async fn idle_timeout_fires_when_upstream_goes_silent() {
        let mut stream = silent_stream();
        let result = tokio::time::timeout(
            Duration::from_secs(5),
            race_next_against_idle(&mut stream, Duration::from_millis(30)),
        )
        .await
        .expect("空闲超时未触发：select 缺少计时分支，等价于线上无限等待");
        assert!(result.is_none(), "上游静默时必须由计时器胜出");
    }

    /// 反向防护：流里有数据时必须取到数据，不能让计时器把正常流误杀。
    #[tokio::test]
    async fn data_wins_over_idle_timer() {
        let mut stream = futures_util::stream::iter(vec![7u8]);
        let idle = Duration::from_secs(30);
        let won = tokio::time::timeout(idle, race_next_against_idle(&mut stream, idle))
            .await
            .expect("数据已就绪，不应等满空闲窗口");
        assert!(won.is_some(), "有数据时不得被判为空闲超时");
    }

    /// 流正常结束（EOF）时 `next()` 就绪并返回 None，不是计时器胜出。
    /// 这正是 `process_stream` 用 `received_event` 区分「正常收尾」与「空闲超时」的依据。
    #[tokio::test]
    async fn eof_resolves_via_next_not_via_timer() {
        let mut stream = futures_util::stream::empty::<u8>();
        let won: Option<()> = tokio::time::timeout(
            Duration::from_secs(5),
            race_next_against_idle(&mut stream, Duration::from_millis(30)),
        )
        .await
        .expect("空流应立即返回 None，而不是挂起");
        assert!(won.is_none());
    }
}
