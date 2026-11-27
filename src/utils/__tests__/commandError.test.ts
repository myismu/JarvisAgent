/**
 * utils/commandError.ts 单元测试
 *
 * 验证后端结构化 CommandError、原生 Error 与字符串错误的统一识别与提取。
 */
import { describe, it, expect } from "vitest";
import {
  extractCommandErrorCode,
  extractCommandErrorMessage,
  isCommandError,
  isCommandErrorRetryable,
} from "../commandError";

describe("commandError 结构化错误解析", () => {
  it("准确识别后端 CommandError 并提取 code、retryable 与可读 message", () => {
    const err = {
      code: "INVALID_INPUT",
      message: "项目名不能为空",
      retryable: false,
    };
    expect(isCommandError(err)).toBe(true);
    expect(extractCommandErrorCode(err)).toBe("INVALID_INPUT");
    expect(extractCommandErrorMessage(err)).toBe("项目名不能为空");
    expect(isCommandErrorRetryable(err)).toBe(false);

    const retryableErr = {
      code: "CONFLICT",
      message: "当前任务尚未结束",
      retryable: true,
    };
    expect(isCommandErrorRetryable(retryableErr)).toBe(true);
  });

  it("向后兼容字符串、原生 Error 及未知对象", () => {
    expect(isCommandError("旧版字符串错误")).toBe(false);
    expect(extractCommandErrorCode("旧版字符串错误")).toBeNull();
    expect(isCommandErrorRetryable("旧版字符串错误")).toBe(false);
    expect(extractCommandErrorMessage("旧版字符串错误")).toBe("旧版字符串错误");

    expect(extractCommandErrorMessage(new Error("网络断开"))).toBe("网络断开");
    expect(extractCommandErrorMessage({ detail: 500 })).toBe('{"detail":500}');
  });
});
