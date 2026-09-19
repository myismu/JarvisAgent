/**
 * # thinking.ts — 深度思考档位的纯函数镜像（与 Rust 侧同源）
 *
 * 这是 `src-tauri/src/core/session/thinking.rs` 的 TS 镜像。
 * **两侧必须共用同一组测试向量**，任何一侧改判定规则都要同步另一侧。
 *
 * ## 职责边界（重要）
 * 后端才是唯一真相，且**「跟随全局」的解析只发生在后端**：
 * 新建会话首次发消息时，后端按「设置默认档位 + 模型能力」算出确定布尔并固化入库。
 * 前端**不解析跟随、不读设置默认、不查模型能力**去推导档位——
 * 只负责把后端下发的 `thinkingEnabled` / `resolvedEnabled` 渲染出来。
 *
 * ## 两层语义（v14 起）
 * | 层 | 名称 | 形态 | 来源 |
 * |---|---|---|---|
 * | L2 | 会话档位 | **布尔**（库中 `INTEGER NOT NULL`，无 NULL） | 后端快照 `thinkingEnabled` |
 * | S  | 设置默认 | 三值 `follow_global` / `on` / `off` | `UiPreferences.thinkingDefault` |
 *
 * 单轮覆盖（旧的 L3）已废弃：`ask_jarvis` 不再接收 `thinkingOverride`。
 */

/** 设置默认档位（三值，「跟随全局」由**后端**按模型能力解析） */
export type ThinkingDefault = "follow_global" | "on" | "off";

/** 模型思考能力（来自 `get_model_capabilities`） */
export interface ThinkingCaps {
  thinking: boolean;
  thinking_forced?: boolean;
}

/** 裁决原因（与 Rust `ThinkingReason` 一一对应） */
export type ThinkingReason =
  | "Unsupported"
  | "ForcedByModel"
  | "ClampedByForced"
  | "SessionMode";

export interface ThinkingDecision {
  enabled: boolean;
  reason: ThinkingReason;
}

/** 解析设置默认档位；未知值回落 `follow_global`（与 Rust `ThinkingDefault::parse` 一致） */
export function parseThinkingDefault(raw: string | null | undefined): ThinkingDefault {
  const value = (raw ?? "").trim().toLowerCase();
  if (value === "on") return "on";
  if (value === "off") return "off";
  // 兼容历史值 'auto' 与任何脏值：统一落到「跟随全局」
  return "follow_global";
}

/**
 * 唯一决策逻辑（与 Rust `thinking::decide` 逐行对应）。
 *
 * 优先序：能力夹紧 ▸ 会话档位。会话档位已是确定布尔，不再有"未表态"分支。
 */
export function decideThinking(
  sessionEnabled: boolean,
  caps: ThinkingCaps | null,
): ThinkingDecision {
  const forced = caps?.thinking_forced ?? false;
  const thinkingSupported = caps?.thinking ?? true;

  // 1. 模型不支持思考：无视一切意愿
  if (!thinkingSupported) {
    return { enabled: false, reason: "Unsupported" };
  }

  // 2. 会话档位（布尔）→ 能力夹紧
  if (forced) {
    // 模型强制开启：用户想关也关不掉，区分语义便于诊断
    return sessionEnabled
      ? { enabled: true, reason: "ForcedByModel" }
      : { enabled: true, reason: "ClampedByForced" };
  }
  return { enabled: sessionEnabled, reason: "SessionMode" };
}

/**
 * 开关是否可点。
 *
 * 只有**确知**模型不支持思考、或模型强制思考时才禁用。
 *
 * 早期版本把 `caps === null`（能力尚未探测）当成"不支持思考"而禁用，导致新建
 * 会话里开关显示为"开"却点不动，发出首条消息触发能力探测后才恢复——典型的
 * "看起来坏了"。**未知 ≠ 不支持**。
 */
export function isThinkingToggleDisabled(caps: ThinkingCaps | null): boolean {
  if (caps === null) return false; // 未知：允许用户先表达意愿
  if (!caps.thinking) return true; // 确知不支持
  return caps.thinking_forced === true; // 确知强制开启
}

/**
 * 后端下发的档位快照。
 *
 * 对应 `get_session_thinking` / `set_session_thinking_enabled` 的返回值，
 * 以及 `session-thinking-mode-changed` 事件的 payload。
 */
export interface ThinkingSnapshot {
  sessionId: string;
  /** 会话库里存的确定布尔（用户意图 / 建会话时固化的值） */
  thinkingEnabled: boolean;
  /** **实际生效**的值（已过模型能力夹紧，如 DeepSeek 上恒为 true） */
  resolvedEnabled: boolean;
  reason: ThinkingReason | string;
  noticeI18nKey?: string | null;
}
