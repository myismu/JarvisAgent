//! # pipeline/helpers.rs — Agent 管线辅助类型与纯函数
//!
//! 包含流超时等待提示（WaitingHint）、中断标记、上下文估算（ContextEstimate）、
//! 残缺工具配对修复与 Plan 看门狗计分等独立辅助逻辑。

use super::*;

/// 判断空闲超时后是否必须结束本轮并按中断收尾。
///
/// **实测 bug 的防护点**：首版这里用的是 `should_retry`（"零产出"），
/// 于是 `interrupted` 形态（吐了半截后静默）虽然计时器触发了，却走到了正常
/// 完成路径 —— 界面只剩半截正文，用户以为已经正常完成。
///
/// 正确语义：只要计时器触发（`idle_timed_out`）就必须收尾，**与产出多少无关**；
/// `should_retry` 只决定要不要再试一次，不决定是否收尾。
pub(super) fn must_finish_as_interrupted(idle_timed_out: bool, _should_retry: bool) -> bool {
    idle_timed_out
}

/// 当前时间（毫秒），用于"等待提示"看门狗判断静默时长。
pub(super) fn now_millis() -> u64 {
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
pub(super) fn should_emit_waiting_hint(silent_secs: u64, already_sent: bool) -> bool {
    silent_secs >= crate::core::agent::stream::waiting_hint_secs() && !already_sent
}

/// 收到新数据（或本轮不再等数据）时的统一处理：复位静默计时，并清掉前端的等待提示。
///
/// ## 为什么必须发清除事件
///
/// 2026-11-22 用户实测：上游恢复、正文继续生成后，气泡下方「已等待 N 秒未收到数据…」
/// 仍一直挂着，直到整轮 loop 结束才消失。前端 `currentTurn.notice` 是**单向写入**
/// 的字段（写入点只有 `waiting_hint` / `retry` / `reflection` 三处，无任何自清逻辑），
/// 只有换 turn 时才整体丢弃 —— 所以「已经恢复」这件事必须由后端显式告知。
///
/// ## 为什么用 swap 而不是普通读
///
/// 本函数在**每一个 SSE 帧**到达时都会被调用，帧密度极高。`swap(false)` 兼作闸门：
/// 只有「刚才确实发过提示」的那一次才 emit，其余帧只读不写；同时它顺带复位
/// 本段静默期的标记，让长时间静默之后的再次静默仍能提示。
pub(super) fn touch_and_clear(
    last_tick_ms: &std::sync::Arc<std::sync::atomic::AtomicU64>,
    hint_sent: &std::sync::Arc<std::sync::atomic::AtomicBool>,
    app: &tauri::AppHandle,
    sid: &str,
    loop_count: usize,
) {
    use std::sync::atomic::Ordering;
    last_tick_ms.store(now_millis(), Ordering::Relaxed);
    if !hint_sent.swap(false, Ordering::Relaxed) {
        return;
    }
    // 与 `retry` 的清除同一语义：等待提示是**过程性状态**，恢复后必须收掉。
    // `kinds` 带来源标记，前端只清它自己写的那一类，不误伤反思标注与中断说明。
    let _ = app.emit(
        "agent-step",
        crate::infra::types::events::AgentStepEvent::NoticeClear {
            kinds: vec!["waiting_hint".to_string()],
            session_id: sid.to_string(),
            loop_count: Some(loop_count),
        },
    );
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
pub(super) struct WaitingHint {
    /// 最近一次收到数据的时间戳（毫秒）
    pub(super) last_tick_ms: Arc<std::sync::atomic::AtomicU64>,
    /// 当前这段静默期是否已经提示过。
    ///
    /// 回归防护：首版每 30s 就发一条，页面被"已 30 秒未收到数据"刷屏。
    /// 现在同一段静默期**只提示一次**，直到收到新数据（`touch()`）才允许再提示。
    /// 这样既避免了刷屏，也保证长时间静默不会被彻底静默处理。
    pub(super) hint_sent: Arc<std::sync::atomic::AtomicBool>,
    /// 是否已结束；置位后看门狗不再提示
    pub(super) done: Arc<std::sync::atomic::AtomicBool>,
    pub(super) handle: tokio::task::JoinHandle<()>,
}

impl WaitingHint {
    /// 收到一帧数据（或请求刚发出）时调用，重置静默计时并允许再次提示
    pub(super) fn touch(&self, app: &tauri::AppHandle, sid: &str, loop_count: usize) {
        touch_and_clear(&self.last_tick_ms, &self.hint_sent, app, sid, loop_count);
    }

    /// 本轮结束，停止看门狗。
    ///
    /// 结束前同样走一次 `touch_and_clear`：本轮不会再等上游数据了，
    /// 界面上挂着的「已等待 N 秒」提示必须收掉，否则会一直挂到整轮 loop 结束
    /// （用户实测反馈的同一问题）。
    pub(super) fn finish(&self, app: &tauri::AppHandle, sid: &str, loop_count: usize) {
        touch_and_clear(&self.last_tick_ms, &self.hint_sent, app, sid, loop_count);
        self.done.store(true, std::sync::atomic::Ordering::Relaxed);
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
pub(super) const INTERRUPT_MARKER_RESUMABLE: &str =
    "**[回复被中断]** 上次回复在此处中断，请基于上下文继续完成。";

/// 已达轮次上限：与上面刻意不同 —— 此时模型**不该**接着写，
/// 否则会立刻再次触达上限。
pub(super) const INTERRUPT_MARKER_STOPPED: &str =
    "**[回复被中断]** 已达回合上限且未获续跑授权，本轮在此停下。";

/// 规划探索到上限：与 INTERRUPT_MARKER 系同风格（`**[标签]**` 开头），
/// 语义是"停下等用户决策"，模型**不该**续写。
pub(super) const PLAN_LIMIT_MARKER: &str =
    "**[规划探索已到上限]** 规划探索已达到阈值，本轮自动停下，等待用户决策。";

/// 按中断类型取"写给模型看"的标记行。
///
/// 落库时正文是**干净**的（kind 独立成列）；发送给模型前由
/// `prepare_history_snapshot_from_messages` 按 kind 把标记拼回正文尾部 ——
/// 模型必须知道自己上一句被截断了，否则续跑会重复或断片。
/// 措辞按 kind 收敛（见 INTERRUPT_MARKER_RESUMABLE 注释：变体越少、
/// prompt cache 越稳）。
pub(super) fn interrupt_marker_for(kind: InterruptKind) -> &'static str {
    match kind {
        // 能接着做的三种（流超时 / 用户取消 / 执行报错）共用"接着做"措辞
        InterruptKind::StreamTimeout | InterruptKind::UserCancel | InterruptKind::PipelineError => {
            INTERRUPT_MARKER_RESUMABLE
        }
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
pub(super) fn append_interrupt_marker_to_message(msg: &mut Message, marker: &str) {
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

pub(super) struct ContextEstimate {
    pub(super) total_chars: usize,
    pub(super) estimated_tokens: usize,
    pub(super) message_count: usize,
    pub(super) tool_schema_count: usize,
    pub(super) tool_call_count: usize,
    pub(super) tool_result_count: usize,
    /// 图片块数量（Image block 数），与 tool_call_count / tool_result_count 同族。
    ///
    /// 它原先是靠一个独立的 `attachments` 分区表达的，但那个分区的
    /// chars / estimated_tokens 与 `messages` 分区**重复计入** ——
    /// 图片块本来就躺在 messages 的 JSON 里，再单开一段等于把同一份内容算两遍，
    /// 总量虚高、`drift_percent` 跟着失真。而"块数"本身不是可加和的尺寸量，
    /// 拿它当一个分区只会让「分区之和 = 总量」这条不变式失效。
    /// 现在收敛为顶层计数：分区只剩 system / messages / tools，三者之和就是总量。
    pub(super) image_count: usize,
    pub(super) sections: Vec<ContextSectionSnapshot>,
}

pub(super) const DIRECT_DEVELOPER_INTENT: &str = "PROJECT_ACTION";

/// 每个 loop 使用的 thinking 状态：整轮恒定。
///
/// 回归防护：旧实现在 `loop_count > 0` 时回落到 audience 默认值，导致
/// 「首轮 disabled → 第二轮 enabled」的翻转；此时历史里那条 assistant 是在关闭
/// thinking 时产生的、没有 thinking 块可回传，Anthropic 协议的服务商（含 DeepSeek
/// 的 anthropic 兼容端点）会直接 400：
/// `content[].thinking in the thinking mode must be passed back to the API`。
pub(super) fn should_think_for_loop(turn_think: bool, _loop_count: usize) -> bool {
    turn_think
}

/// 判断本 loop 的工具调用中是否提交了规划方案。
///
/// 两种等价形态都要认（喂狗判定必须同时覆盖，否则提交方案的那轮会被当成
/// 空转、看门狗误触发把收尾截胡——B 修复诊断出的真实事故路径）：
/// - 裸调用：工具名就是 `ProposePlan`；
/// - 按需工具包装：工具名是 `ExecuteTool`，参数 JSON 的 `name` 字段指向
///   真实工具（按需工具的统一执行入口，模型提交方案固定走这条形态）。
pub(super) fn loop_submitted_plan(tool_calls: &[(String, String)]) -> bool {
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

/// 推进 Plan 看门狗的轮次计数，返回是否应触发。
///
/// 计分口径是**轮**：无论本轮调用了 1 个还是 20 个工具，都只 +1。
/// 旧实现按 `tool_calls.len()` 累加，一次并行读取就能把额度吃满，
/// 大项目探索阶段因此频繁误拦。
///
/// 清零条件（喂狗）：提交了方案，或已切出 plan 模式。
pub(super) fn advance_plan_watchdog(
    work_mode: &str,
    submitted_plan: bool,
    loops_without_plan: &mut usize,
) -> bool {
    use crate::infra::types::constants::PLAN_WATCHDOG_MAX_LOOPS_WITHOUT_PLAN;

    if work_mode != "plan" || submitted_plan {
        *loops_without_plan = 0;
        return false;
    }
    *loops_without_plan = loops_without_plan.saturating_add(1);
    *loops_without_plan >= PLAN_WATCHDOG_MAX_LOOPS_WITHOUT_PLAN
}

/// 判断给定的助手文本是否已存在于消息列表尾部。
///
/// 中断收尾时用于去重：流式内容可能已由 `store_assistant_response()` 落库，
/// 避免把同一段半截话写两遍。只检查尾部若干条，因为中断内容只可能出现在最近的位置。
pub(super) fn assistant_text_exists_at_tail(messages: &[Message], target: &str) -> bool {
    let target = target.trim();
    if target.is_empty() {
        return false;
    }
    messages.iter().rev().take(4).any(|msg| match msg {
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
pub(super) async fn store_interrupted_turn(
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
            if session
                .interrupt_kinds
                .get(idx)
                .cloned()
                .flatten()
                .is_some()
            {
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
        MessageSource::Interrupted,
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
pub(super) fn tail_assistant_text_index(messages: &[Message], target: &str) -> Option<usize> {
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
pub(super) fn fix_broken_tool_call_pairs(messages: &mut Vec<Message>) {
    let mut i = 0;
    while i < messages.len() {
        // 检查当前消息是否为包含 ToolUse 的 Assistant 消息
        let has_tool_use = match &messages[i] {
            Message::Assistant {
                content: Content::Multiple(blocks),
            } => blocks
                .iter()
                .any(|b| matches!(b, ContentBlock::ToolUse { .. })),
            _ => false,
        };

        if !has_tool_use {
            i += 1;
            continue;
        }

        // 收集所有 ToolUse 的 tool_use_id
        let tool_use_ids: Vec<String> = match &messages[i] {
            Message::Assistant {
                content: Content::Multiple(blocks),
            } => blocks
                .iter()
                .filter_map(|b| match b {
                    ContentBlock::ToolUse { id, .. } => Some(id.clone()),
                    _ => None,
                })
                .collect(),
            _ => vec![],
        };

        // 检查下一条消息是否包含对应的 ToolResult
        let next_has_results = if i + 1 < messages.len() {
            match &messages[i + 1] {
                Message::User {
                    content: Content::Multiple(blocks),
                } => {
                    let result_ids: Vec<&str> = blocks
                        .iter()
                        .filter_map(|b| match b {
                            ContentBlock::ToolResult { tool_use_id, .. } => {
                                Some(tool_use_id.as_str())
                            }
                            _ => None,
                        })
                        .collect();
                    tool_use_ids
                        .iter()
                        .all(|id| result_ids.contains(&id.as_str()))
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
            let placeholder_blocks: Vec<ContentBlock> = tool_use_ids
                .iter()
                .map(|id| ContentBlock::ToolResult {
                    tool_use_id: id.clone(),
                    content: "[系统注入：该工具已执行但结果因中断（取消/上游失联/报错）未能留档，\
                              已自动补齐消息序列以保证请求合法。如需确认实际效果请让用户复查，\
                              必要时可让用户回复「继续」重新推进。]"
                        .to_string(),
                })
                .collect();
            messages.insert(
                i + 1,
                Message::User {
                    content: Content::Multiple(placeholder_blocks),
                },
            );
            // 跳过刚插入的消息
            i += 2;
        } else {
            i += 1;
        }
    }
}
