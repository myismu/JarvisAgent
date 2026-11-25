/**
 * 前端通用格式化与文本判定工具单元测试
 *
 * 覆盖：tokenDisplay.ts / duration.ts / timeline.ts / userMessage.ts
 */
import { describe, it, expect, vi, beforeEach, afterEach } from "vitest";
import { formatTokenCount } from "../tokenDisplay";
import { formatDurationMs } from "../duration";
import { formatRelativeTime, getFileOpPaths, getFileOpLabel } from "../timeline";
import { plainUserText, isLongUserText } from "../userMessage";

describe("formatTokenCount", () => {
  it("按阈值格式化整数 / k / m", () => {
    expect(formatTokenCount(0)).toBe("0");
    expect(formatTokenCount(842)).toBe("842");
    expect(formatTokenCount(999.4)).toBe("999");
    expect(formatTokenCount(1_000)).toBe("1.0k");
    expect(formatTokenCount(3_450)).toBe("3.5k");
    expect(formatTokenCount(1_000_000)).toBe("1.0m");
    expect(formatTokenCount(1_250_000)).toBe("1.3m");
  });

  it("缺失值、负数、非有限数安全归零", () => {
    expect(formatTokenCount(null)).toBe("0");
    expect(formatTokenCount(undefined)).toBe("0");
    expect(formatTokenCount(-100)).toBe("0");
    expect(formatTokenCount(Number.NaN)).toBe("0");
    expect(formatTokenCount(Number.POSITIVE_INFINITY)).toBe("0");
  });
});

describe("formatDurationMs", () => {
  it("小于 1 分钟按向下取整的秒数输出", () => {
    expect(formatDurationMs(0)).toBe("0 秒");
    expect(formatDurationMs(999)).toBe("0 秒");
    expect(formatDurationMs(1_500)).toBe("1 秒");
    expect(formatDurationMs(59_999)).toBe("59 秒");
  });

  it("大于等于 1 分钟输出分和秒，整分不带 0 秒", () => {
    expect(formatDurationMs(60_000)).toBe("1 分");
    expect(formatDurationMs(63_800)).toBe("1 分 3 秒");
    expect(formatDurationMs(120_000)).toBe("2 分");
  });

  it("异常值安全归零", () => {
    expect(formatDurationMs(null)).toBe("0 秒");
    expect(formatDurationMs(undefined)).toBe("0 秒");
    expect(formatDurationMs(-5000)).toBe("0 秒");
    expect(formatDurationMs(Number.NaN)).toBe("0 秒");
  });
});

describe("timeline utils", () => {
  beforeEach(() => {
    vi.useFakeTimers();
    vi.setSystemTime(new Date("2026-11-25T12:00:00.000Z"));
  });

  afterEach(() => {
    vi.useRealTimers();
  });

  it("formatRelativeTime 按毫秒差值正确分档", () => {
    const now = Date.now();
    expect(formatRelativeTime(now - 10_000)).toBe("刚刚");
    expect(formatRelativeTime(now - 5 * 60_000)).toBe("5 分钟前");
    expect(formatRelativeTime(now - 3 * 3_600_000)).toBe("3 小时前");
    // 超过 24 小时回退为本地化日期字符串
    const oldText = formatRelativeTime(now - 2 * 86_400_000);
    expect(oldText).not.toContain("分钟前");
    expect(oldText).not.toContain("小时前");
    expect(oldText.length).toBeGreaterThan(0);
  });

  it("getFileOpPaths 与 getFileOpLabel 处理已知与未知操作", () => {
    expect(getFileOpPaths("edit").length).toBeGreaterThan(0);
    expect(getFileOpPaths("update_file")).toEqual(getFileOpPaths("edit"));
    expect(getFileOpPaths("unknown_op")).toEqual([]);

    expect(getFileOpLabel("edit")).toBe("编辑");
    expect(getFileOpLabel("update")).toBe("编辑");
    expect(getFileOpLabel("rename")).toBe("重命名");
    expect(getFileOpLabel("custom_op")).toBe("custom_op");
  });
});

describe("userMessage utils", () => {
  it("plainUserText 剥离 HTML 标签并处理空值", () => {
    expect(plainUserText("<b>hello</b> <span>world</span>")).toBe("hello world");
    expect(plainUserText(null)).toBe("");
    expect(plainUserText(undefined)).toBe("");
  });

  it("isLongUserText 正确判定 6 行 / 500 字符阈值，并折叠连续空行", () => {
    expect(isLongUserText("")).toBe(false);
    expect(isLongUserText("a\nb\nc\nd\ne\nf")).toBe(false); // 刚好 6 行
    expect(isLongUserText("a\nb\nc\nd\ne\nf\ng")).toBe(true); // 7 行
    // 连续空行会被压成两换行，不应误判为长消息
    expect(isLongUserText("段落一\n\n\n\n\n段落二")).toBe(false);
    expect(isLongUserText("x".repeat(500))).toBe(false);
    expect(isLongUserText("x".repeat(501))).toBe(true);
  });
});
