/**
 * usageBreakdown.ts 单元测试
 *
 * 验证后端分组用量载荷的防御性归一化：缺字段/负数/NaN 归零，
 * 且 hasDetail 判定遵循「后端显式布尔 > 分组键存在 > false」顺序。
 */
import { describe, it, expect } from "vitest";
import {
  createEmptySessionUsageBreakdown,
  normalizeSessionUsageBreakdown,
} from "../usageBreakdown";

describe("normalizeSessionUsageBreakdown", () => {
  it("输入为空或非对象时返回 hasDetail=false 的归零快照", () => {
    const empty = createEmptySessionUsageBreakdown();
    expect(normalizeSessionUsageBreakdown(null)).toEqual(empty);
    expect(normalizeSessionUsageBreakdown(undefined)).toEqual(empty);
    expect(normalizeSessionUsageBreakdown("bad")).toEqual(empty);
    expect(empty.hasDetail).toBe(false);
  });

  it("把负数、NaN、Infinity、字符串全部收敛为 0，并四舍五入合法小数", () => {
    const res = normalizeSessionUsageBreakdown({
      main: {
        input: 1200.6,
        output: -50,
        cacheHit: Number.NaN,
        cacheMiss: Number.POSITIVE_INFINITY,
        requestCount: "3",
        cacheReported: 2,
      },
      unreportedRequestCount: -1,
      unreportedInput: 99.4,
    });

    expect(res.main).toEqual({
      input: 1201,
      output: 0,
      cacheHit: 0,
      cacheMiss: 0,
      requestCount: 0,
      cacheReported: 2,
    });
    expect(res.unreportedRequestCount).toBe(0);
    expect(res.unreportedInput).toBe(99);
    // 包含 main 分组键且未显式给 hasDetail 时，自动推导为 true
    expect(res.hasDetail).toBe(true);
  });

  it("后端显式给出 hasDetail 布尔值时优先采信", () => {
    const forcedFalse = normalizeSessionUsageBreakdown({
      main: { input: 100 },
      hasDetail: false,
    });
    expect(forcedFalse.hasDetail).toBe(false);

    const forcedTrue = normalizeSessionUsageBreakdown({
      hasDetail: true,
    });
    expect(forcedTrue.hasDetail).toBe(true);
  });

  it("既无分组键也无显式 hasDetail 时，hasDetail 为 false", () => {
    const res = normalizeSessionUsageBreakdown({ unreportedInput: 500 });
    expect(res.hasDetail).toBe(false);
    expect(res.unreportedInput).toBe(500);
  });
});
