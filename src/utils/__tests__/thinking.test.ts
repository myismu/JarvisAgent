/**
 * thinking.ts 单元测试
 *
 * 对应 Rust 侧 `core/session/thinking.rs` 的前端纯函数镜像，
 * 保证深度思考档位解析、能力夹紧与开关禁用逻辑前后端口径一致。
 */
import { describe, it, expect } from "vitest";
import {
  parseThinkingDefault,
  decideThinking,
  isThinkingToggleDisabled,
} from "../thinking";

describe("parseThinkingDefault", () => {
  it("正确解析 on / off（忽略大小写与首尾空格）", () => {
    expect(parseThinkingDefault("on")).toBe("on");
    expect(parseThinkingDefault(" ON ")).toBe("on");
    expect(parseThinkingDefault("off")).toBe("off");
    expect(parseThinkingDefault("Off")).toBe("off");
  });

  it("历史值 auto 与未知/空值统一回落 follow_global", () => {
    expect(parseThinkingDefault("auto")).toBe("follow_global");
    expect(parseThinkingDefault("follow_global")).toBe("follow_global");
    expect(parseThinkingDefault("")).toBe("follow_global");
    expect(parseThinkingDefault(null)).toBe("follow_global");
    expect(parseThinkingDefault(undefined)).toBe("follow_global");
    expect(parseThinkingDefault("invalid_value")).toBe("follow_global");
  });
});

describe("decideThinking", () => {
  it("模型不支持思考时无视会话档位，返回 Unsupported", () => {
    expect(decideThinking(true, { thinking: false })).toEqual({
      enabled: false,
      reason: "Unsupported",
    });
    expect(decideThinking(false, { thinking: false, thinking_forced: true })).toEqual({
      enabled: false,
      reason: "Unsupported",
    });
  });

  it("模型强制思考时，区分用户原本想开（ForcedByModel）与被夹紧（ClampedByForced）", () => {
    expect(decideThinking(true, { thinking: true, thinking_forced: true })).toEqual({
      enabled: true,
      reason: "ForcedByModel",
    });
    expect(decideThinking(false, { thinking: true, thinking_forced: true })).toEqual({
      enabled: true,
      reason: "ClampedByForced",
    });
  });

  it("模型可选思考或能力尚未探测（null）时，遵循会话档位", () => {
    expect(decideThinking(true, { thinking: true, thinking_forced: false })).toEqual({
      enabled: true,
      reason: "SessionMode",
    });
    expect(decideThinking(false, { thinking: true })).toEqual({
      enabled: false,
      reason: "SessionMode",
    });
    expect(decideThinking(true, null)).toEqual({
      enabled: true,
      reason: "SessionMode",
    });
    expect(decideThinking(false, null)).toEqual({
      enabled: false,
      reason: "SessionMode",
    });
  });
});

describe("isThinkingToggleDisabled", () => {
  it("能力未知（null）时允许点击，避免误禁用", () => {
    expect(isThinkingToggleDisabled(null)).toBe(false);
  });

  it("确知不支持或强制开启时禁用，可选思考时允许点击", () => {
    expect(isThinkingToggleDisabled({ thinking: false })).toBe(true);
    expect(isThinkingToggleDisabled({ thinking: true, thinking_forced: true })).toBe(true);
    expect(isThinkingToggleDisabled({ thinking: true, thinking_forced: false })).toBe(false);
    expect(isThinkingToggleDisabled({ thinking: true })).toBe(false);
  });
});
