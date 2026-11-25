/**
 * contextUsage.ts 单元测试
 *
 * 验证上下文占用读数的唯一口径：厂商实测值优先、本地估算兜底、
 * 分母未知时不编造百分比、以及手动压缩的消息条数下限。
 */
import { describe, it, expect } from "vitest";
import {
  COMPACT_MIN_MESSAGES,
  resolveContextTokens,
  isContextMeasured,
  resolveContextPercent,
  canManuallyCompact,
} from "../contextUsage";

describe("resolveContextTokens & isContextMeasured", () => {
  it("厂商实测值 > 0 时优先采用实测值并标记为实测", () => {
    const snapshot = { providerInputTokens: 52000, estimatedTokens: 17300 };
    expect(resolveContextTokens(snapshot)).toBe(52000);
    expect(isContextMeasured(snapshot)).toBe(true);
  });

  it("实测值缺失或为 0 时回退本地估算，且标记为非实测", () => {
    expect(resolveContextTokens({ providerInputTokens: 0, estimatedTokens: 8500 })).toBe(8500);
    expect(isContextMeasured({ providerInputTokens: 0, estimatedTokens: 8500 })).toBe(false);
    expect(resolveContextTokens({ providerInputTokens: null, estimatedTokens: 4200 })).toBe(4200);
    expect(isContextMeasured({ providerInputTokens: null, estimatedTokens: 4200 })).toBe(false);
  });

  it("快照为空或估算值为负时安全归零", () => {
    expect(resolveContextTokens(null)).toBe(0);
    expect(resolveContextTokens(undefined)).toBe(0);
    expect(resolveContextTokens({ estimatedTokens: -100 })).toBe(0);
    expect(isContextMeasured(null)).toBe(false);
  });
});

describe("resolveContextPercent", () => {
  it("正常计算占用百分比并保留一位小数", () => {
    expect(resolveContextPercent(50000, 200000)).toBe(25);
    expect(resolveContextPercent(33333, 100000)).toBe(33.3);
  });

  it("超出窗口上限时封顶 100%", () => {
    expect(resolveContextPercent(250000, 200000)).toBe(100);
  });

  it("分母未知、分母非正或分子非正时返回 null（不编造百分比）", () => {
    expect(resolveContextPercent(1000, null)).toBeNull();
    expect(resolveContextPercent(1000, undefined)).toBeNull();
    expect(resolveContextPercent(1000, 0)).toBeNull();
    expect(resolveContextPercent(1000, -100)).toBeNull();
    expect(resolveContextPercent(0, 200000)).toBeNull();
  });
});

describe("canManuallyCompact", () => {
  it("严格大于 COMPACT_MIN_MESSAGES（6）时才允许手动压缩", () => {
    expect(COMPACT_MIN_MESSAGES).toBe(6);
    expect(canManuallyCompact(undefined)).toBe(false);
    expect(canManuallyCompact(null)).toBe(false);
    expect(canManuallyCompact(0)).toBe(false);
    expect(canManuallyCompact(6)).toBe(false);
    expect(canManuallyCompact(7)).toBe(true);
  });
});
