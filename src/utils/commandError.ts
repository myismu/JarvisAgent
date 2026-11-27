/**
 * # commandError.ts — 跨端结构化命令错误解析工具
 *
 * 统一识别并格式化 Rust 后端返回的 `CommandError { code, message, retryable }`、
 * `AgentError` 枚举对象、原生 `Error` 实例以及旧版裸字符串错误，避免界面出现 `[object Object]`
 * 或原始 JSON 文本。
 *
 * ## Key Exports
 * - `isCommandError()`: 判断错误对象是否为后端结构化 `CommandError`
 * - `extractCommandErrorCode()`: 提取结构化错误码 `ErrorCode`
 * - `extractCommandErrorMessage()`: 提取面向用户的可读错误消息
 *
 * ## Dependencies
 * - Internal: `@/types`
 */
import type { CommandError, ErrorCode } from "../types";

/** 类型守卫：判断是否为 Rust 后端序列化的结构化 `CommandError` */
export function isCommandError(err: unknown): err is CommandError {
  if (typeof err !== "object" || err === null) return false;
  const candidate = err as Record<string, unknown>;
  return (
    typeof candidate.code === "string" &&
    typeof candidate.message === "string" &&
    typeof candidate.retryable === "boolean"
  );
}

/** 从未知错误中提取结构化错误码（非 `CommandError` 时返回 `null`） */
export function extractCommandErrorCode(err: unknown): ErrorCode | null {
  return isCommandError(err) ? err.code : null;
}

/** 判断后端结构化错误是否可安全重试（非 `CommandError` 默认返回 `false`） */
export function isCommandErrorRetryable(err: unknown): boolean {
  return isCommandError(err) ? err.retryable : false;
}

/** 提取适合展示给用户的可读错误文本，优先识别 `CommandError.message` */
export function extractCommandErrorMessage(err: unknown): string {
  if (typeof err === "string") return err;
  if (isCommandError(err)) return err.message;
  if (err instanceof Error) return err.message;
  if (typeof err === "object" && err !== null) {
    const obj = err as Record<string, unknown>;
    if (typeof obj.message === "string" && obj.message.trim()) {
      return obj.message;
    }
  }
  try {
    return JSON.stringify(err);
  } catch {
    return String(err);
  }
}
