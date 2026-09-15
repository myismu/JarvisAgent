/**
 * # thinking.ts — 深度思考档位的纯函数镜像（与 Rust 侧同源）
 *
 * 这是 `src-tauri/src/core/session/thinking.rs` 的 TS 镜像。
 * **两侧必须共用同一组测试向量**，任何一侧改判定规则都要同步另一侧。
 *
 * ## 为什么前端要重复实现一遍
 * 后端才是唯一真相（`sessions.thinking_mode`），但"能力查询"是异步的：
 * 如果 UI 等后端返回才更新开关，切会话/切预设时会出现肉眼可见的抖动与
 * "点了没反应"的错觉。因此前端在本地**同步**算一次投影用于渲染，
 * 而**真正下发的请求不依赖它**（`ask_jarvis` 不再接收 thinkingOverride），
 * 所以即使投影短暂过期也污染不到真实请求。
 *
 * ## 三层语义
 * | 层 | 名称 | 来源 |
 * |---|---|---|
 * | L3 | 单轮覆盖 | 仅程序化调用，UI 不使用 |
 * | L2 | 会话档位 `thinking_mode` | 后端 `switch_session` / `get_session_thinking` |
 * | L1 | 预设默认 `thinkingDefault` | `app-config.json` 的激活预设 |
 */

/** 会话级表态（L2） */
export type ThinkingMode = "auto" | "always" | "never";

/** 预设默认（L1） */
export type ThinkingDefault = "auto" | "on" | "off";

/** 模型思考能力（来自 `get_model_capabilities`） */
export interface ThinkingCaps {
  thinking: boolean;
  thinking_forced?: boolean;
}

export type ThinkingReason =
  | "Unsupported"
  | "ForcedByModel"
  | "ClampedByForced"
  | "SessionAlways"
  | "SessionNever"
  | "ProfileDefault"
  | "Override";

export interface ThinkingDecision {
  enabled: boolean;
  reason: ThinkingReason;
}

/** 解析会话档位；未知值回落 `auto`（与 Rust `ThinkingMode::parse` 一致） */
export function parseThinkingMode(raw: string | null | undefined): ThinkingMode {
  const value = (raw ?? "").trim().toLowerCase();
  if (value === "always" || value === "on" || value === "true") return "always";
  if (value === "never" || value === "off" || value === "false") return "never";
  return "auto";
}

/** 解析预设默认；未知值回落 `auto`（与 Rust `ThinkingDefault::parse` 一致） */
export function parseThinkingDefault(raw: string | null | undefined): ThinkingDefault {
  const value = (raw ?? "").trim().toLowerCase();
  if (value === "on") return "on";
  if (value === "off") return "off";
  return "auto";
}

/** 把预设默认与全局受众回退合并为确定布尔值（决策 D2） */
export function resolveProfileDefault(
  profileDefault: ThinkingDefault,
  audienceIsDeveloper: boolean,
): boolean {
  if (profileDefault === "on") return true;
  if (profileDefault === "off") return false;
  return audienceIsDeveloper;
}

/**
 * 唯一决策逻辑（与 Rust `thinking::decide` 逐行对应）。
 *
 * 优先序：L3 覆盖 ▸ 能力夹紧 ▸ L2 会话 ▸ L1 预设。
 */
export function decideThinking(
  overrideVal: boolean | null,
  sessionMode: ThinkingMode,
  profileResolvedDefault: boolean,
  caps: ThinkingCaps | null,
): ThinkingDecision {
  const forced = caps?.thinking_forced ?? false;
  const thinkingSupported = caps?.thinking ?? true;

  // 1. 模型不支持思考：无视一切意愿
  if (!thinkingSupported) {
    return { enabled: false, reason: "Unsupported" };
  }

  // 2. 单轮覆盖优先（L3）
  if (overrideVal !== null) {
    return clampToModel(overrideVal, "Override", forced, true);
  }

  // 3. 会话档位（L2）
  switch (sessionMode) {
    case "always":
      return clampToModel(true, "SessionAlways", forced, true);
    case "never":
      return clampToModel(false, "SessionNever", forced, true);
    default:
      // 会话未表态 → 按预设推导。没有任何"用户意愿"被违反，故不提示。
      return clampToModel(profileResolvedDefault, "ProfileDefault", forced, false);
  }
}

/**
 * 按模型硬约束夹紧用户意愿。
 *
 * `noticeOnConflict` 现在只用于**区分两种"被强制开启"的语义**（不再产生任何提示 UI）：
 * - `false`（会话 auto）：没有用户意愿被违反 → `ForcedByModel`；
 * - `true`（用户明确表态过想关闭）：意愿被否决 → `ClampedByForced`。
 *
 * 两者实际下发的请求参数相同（都是开启），差别只在裁决原因，便于诊断与测试断言。
 */
function clampToModel(
  userWants: boolean,
  reason: ThinkingReason,
  forced: boolean,
  noticeOnConflict: boolean,
): ThinkingDecision {
  if (forced) {
    return !noticeOnConflict || userWants
      ? { enabled: true, reason: "ForcedByModel" }
      : { enabled: true, reason: "ClampedByForced" };
  }
  return { enabled: userWants, reason };
}

/**
 * 开关是否可点。
 *
 * 只有**确知**模型不支持思考、或模型强制思考时才禁用。
 *
 * 早期版本把 `caps === null`（能力尚未探测）当成"不支持思考"而禁用，导致新建
 * 会话里开关显示为"开"却点不动，发出首条消息触发能力探测后才恢复——典型的
 * "看起来坏了"。**未知 ≠ 不支持**，与 `thinking_override: Option<bool>`、
 * `sessions.thinking_mode` 可空是同一个原则。
 */
export function isThinkingToggleDisabled(caps: ThinkingCaps | null): boolean {
  if (caps === null) return false; // 未知：允许用户先表达意愿
  if (!caps.thinking) return true; // 确知不支持
  return caps.thinking_forced === true; // 确知强制开启
}

/** 后端下发的档位快照（`get_session_thinking` / `set_session_thinking_mode` 的返回） */
export interface ThinkingSnapshot {
  sessionId: string;
  thinkingMode: ThinkingMode;
  /** 当前激活预设的默认档位（已与全局受众回退合并）——由后端给出，前端不重复解析预设 */
  profileResolvedDefault: boolean;
  resolvedEnabled: boolean;
  reason: ThinkingReason | string;
  noticeI18nKey?: string | null;
}
