/**
 * # usageBreakdown.ts — 会话用量「按来源分组」的归一化
 *
 * 分组用量的数据来源（事件载荷 / IPC 回执）都是**后端的绝对快照**，
 * 但字段可能缺一半：旧后端没有分组字段，新后端也可能只给部分来源。
 * 界面若直接读 `payload.breakdown.main.input`，缺字段时会拿到 undefined，
 * 参与计算就变成 NaN。因此所有消费方都先过这一层：
 *
 * - 缺字段 / null / 非有限数 / 负数 → 0
 * - 完全没有分组对象 → 返回 `hasDetail: false` 的归零快照
 *
 * 注意这里是**归零**而不是「保留上一份」：调用方拿到的是这一时刻的完整读法。
 * 需要「后端没给就别覆盖」的语义时（例如 IPC 回执里根本没这个字段），
 * 由调用方先判断字段是否存在再调用，不要用 0 覆盖真数据。
 *
 * ## Key Exports
 * - `createEmptySessionUsageBreakdown()`: 归零快照（hasDetail = false）
 * - `normalizeSessionUsageBreakdown(raw)`: 把任意形状的载荷归一化成完整快照
 */
import type { SessionUsageBreakdown, UsageSourceBreakdown } from "../types";

/**
 * 把任意来源的数值收敛成合法的 token / 次数字。
 *
 * 负数与非有限数一律当 0：这类值只可能来自损坏的载荷，
 * 让它们流进界面会显示成 `-3` 或 `NaN`，比显示 0 更容易误导。
 */
function toCount(value: unknown): number {
  if (typeof value !== "number" || !Number.isFinite(value)) return 0;
  return Math.max(0, Math.round(value));
}

/** 单个来源分组的空值（全部计数为 0） */
export function createEmptyUsageSourceBreakdown(): UsageSourceBreakdown {
  return {
    input: 0,
    output: 0,
    cacheHit: 0,
    cacheMiss: 0,
    requestCount: 0,
    cacheReported: 0,
  };
}

/**
 * 会话分组用量的归零快照。
 *
 * `hasDetail = false` 是关键：它告诉界面「这只是占位，别把 0 当成真实分布」，
 * 界面此时应退回只显示总量。
 */
export function createEmptySessionUsageBreakdown(): SessionUsageBreakdown {
  return {
    main: createEmptyUsageSourceBreakdown(),
    subagent: createEmptyUsageSourceBreakdown(),
    system: createEmptyUsageSourceBreakdown(),
    unreportedRequestCount: 0,
    unreportedInput: 0,
    hasDetail: false,
  };
}

/** 归一化单个来源分组：逐个字段补 0，不保留任何 undefined */
function normalizeSource(raw: unknown): UsageSourceBreakdown {
  const src = (raw && typeof raw === "object" ? raw : {}) as Record<string, unknown>;
  return {
    input: toCount(src.input),
    output: toCount(src.output),
    cacheHit: toCount(src.cacheHit),
    cacheMiss: toCount(src.cacheMiss),
    requestCount: toCount(src.requestCount),
    cacheReported: toCount(src.cacheReported),
  };
}

/**
 * 把后端载荷归一化成完整的会话分组快照（永远返回可用对象，不返回 null）。
 *
 * `hasDetail` 的判定顺序：
 * 1. 后端显式给了布尔值 → 照它（最权威，后端最清楚自己有没有统计）；
 * 2. 否则看有没有任何一个来源分组对象 → 有就视为有明细；
 * 3. 都没有 → false（纯占位）。
 */
export function normalizeSessionUsageBreakdown(raw: unknown): SessionUsageBreakdown {
  if (!raw || typeof raw !== "object") return createEmptySessionUsageBreakdown();
  const src = raw as Record<string, unknown>;
  const hasGroup = "main" in src || "subagent" in src || "system" in src;
  return {
    main: normalizeSource(src.main),
    subagent: normalizeSource(src.subagent),
    system: normalizeSource(src.system),
    unreportedRequestCount: toCount(src.unreportedRequestCount),
    unreportedInput: toCount(src.unreportedInput),
    hasDetail: typeof src.hasDetail === "boolean" ? src.hasDetail : hasGroup,
  };
}
