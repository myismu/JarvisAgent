/**
 * 上下文占用的统一口径（**唯一来源**）。
 *
 * 前端有两处要回答"当前上下文有多大"：输入框的读数环（`TerminalInput.vue`）与右侧监控面板
 * （`ContextInspector.vue`）。两处必须共用同一个分子——否则同一时刻会显示两个不同的数，
 * 用户不知道该信哪个，排查压缩行为时也会被带偏。
 *
 * ## 分子优先级
 * 1. `providerInputTokens` —— 厂商 API 上报的真实 prompt token，**唯一权威**。
 *    后端已把各家族口径归一（OpenAI 家族取 `prompt_tokens`，Anthropic 家族 = 命中 + 未命中），
 *    所以这里不需要再判厂商。
 * 2. `estimatedTokens` —— 厂商未上报时的兜底。**系统性偏低**：它的 messages 段是对
 *    `tool_result` 截断到 3 行的"可读摘要"做的计数（`pipeline.rs::format_messages_readable`），
 *    工具结果越多偏低越多，实测长会话里 17.3k vs 实际 52.0k。
 *
 * 正因如此，展示时必须能区分两者（`isContextMeasured`），估算值要带 `≈` /
 * "本地估算"标记，不能与实测值长得一样。
 *
 * ## 分母
 * `maxContextTokens` 来自**编译期内嵌的 `model_registry.json`**（`registry.rs`），
 * 不是 API 上报；模型没登记窗口时为 null，此时不编百分比。
 */

/** 计算占用所需的最小快照字段集（便于单测与跨组件复用） */
export interface ContextUsageSnapshot {
  estimatedTokens?: number | null;
  providerInputTokens?: number | null;
  maxContextTokens?: number | null;
}

/**
 * 当前上下文占用（token）。
 *
 * 快照缺失返回 0；厂商实测值必须 > 0 才采信（0 表示本轮尚未回来，不能当成"上下文是空的"）。
 */
export function resolveContextTokens(
  snapshot?: ContextUsageSnapshot | null,
): number {
  if (!snapshot) return 0;
  const actual = snapshot.providerInputTokens;
  if (typeof actual === 'number' && actual > 0) return actual;
  return Math.max(0, snapshot.estimatedTokens || 0);
}

/** 该读数是否为厂商实测值（false = 本地估算兜底） */
export function isContextMeasured(
  snapshot?: ContextUsageSnapshot | null,
): boolean {
  const actual = snapshot?.providerInputTokens;
  return typeof actual === 'number' && actual > 0;
}

/**
 * 占用百分比（保留一位小数，上限 100）。
 *
 * 分母未知返回 null —— **不知道窗口就不编百分比**，与"未知 ≠ 0"的既有口径一致。
 */
export function resolveContextPercent(
  tokens: number,
  maxContextTokens?: number | null,
): number | null {
  if (!maxContextTokens || maxContextTokens <= 0 || tokens <= 0) return null;
  const percent = (tokens / maxContextTokens) * 100;
  return Math.round(Math.min(100, percent) * 10) / 10;
}

// ───────────────────────────── 手动压缩的可用性 ─────────────────────────────

/**
 * 手动压缩的消息条数下限（**与后端同源**）。
 *
 * 镜像 Rust 侧唯一真源 `infra::types::constants::COMPACT_MIN_MESSAGES`（= 6）。
 * 后端判据在 `command/session.rs::compact_inner()`：`messages.len() <= keep` 就直接返回
 * "消息不足…无需压缩"，**全程不看 token**。改一侧必须同步另一侧。
 *
 * ⚠️ 2026-09-17 改名 + 改语义：旧名 `COMPACT_KEEP_RECENT_MESSAGES` 兼管"自动压缩后保留几条第尾"
 * 与"手动压缩下限"两件事。前者已撤销（自动压缩现在**不留尾巴**，压完只剩摘要），
 * 本常量只剩后者一层含义，故改名以免误导。
 */
export const COMPACT_MIN_MESSAGES = 6;

/**
 * 建议压缩的占用百分比（**仅用于提示，不是闸门**）。
 *
 * 注意它**不等于**后端的自动压缩阈值：自动压缩是
 * `(窗口 − 输出预算) × COMPACT_TRIGGER_PERCENT`（见 `infra/llm/context_budget.rs`，
 * 2026-09-17 起为 85%），200K 窗口 / 32K 输出预算下实际在窗口的 71.4% 触发，
 * 与本值 70% 接近。所以这里只回答"要不要提醒用户"，不回答"能不能压"。
 */
export const COMPACT_SUGGEST_PERCENT = 70;

/**
 * 手动压缩按钮能不能按。**判据是消息条数，不是 token 占比。**
 *
 * 分工：条数下限是防误触的**功能闸门**（用户主动点，后端会不会真干活由它决定）；
 * token 占比是**容量信号**，只用来提示"建议压缩"。两者混用会出现两种错配：
 * 按钮亮着点下去后端回"消息不足"，或按钮灰着其实能压。
 *
 * 条数取 `snapshot.messageCount`。它在后端有两个生产点，都可能被前端看到：
 * - `command/session.rs` 取 `memory.messages.len()`（与后端判据**等值**）；
 * - `pipeline.rs::update_context_snapshot()` 取过滤后的 `history_snapshot.len()`
 *   （只保留 chat/compact/context/interrupted 来源），因此**只会偏小、不会偏大**。
 *
 * 偏差方向是安全的：`count_frontend ≤ count_backend` ⇒ 前端放行时后端必然也放行，
 * 不会出现"亮了点不动"。宁可偶尔早一格禁用，也不要给一个按下去被拒的按钮。
 */
export function canManuallyCompact(messageCount?: number | null): boolean {
  return typeof messageCount === 'number' && messageCount > COMPACT_MIN_MESSAGES;
}
