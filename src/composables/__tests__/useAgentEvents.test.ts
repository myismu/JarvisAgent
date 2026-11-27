/**
 * composables/useAgentEvents.ts 入口单元测试
 *
 * 验证按域订阅（监控窗口跳过流式对话域）、初始化中途异常回滚与复位
 * `__jarvisListenersInitializing`，以及 `disposeListeners()` 释放全部监听与定时器。
 */
import { describe, it, expect, beforeEach, afterEach, vi } from "vitest";
import { createPinia, setActivePinia } from "pinia";

const listenMock = vi.fn();
const invokeMock = vi.fn();

vi.mock("@tauri-apps/api/event", () => ({
  listen: (...args: unknown[]) => listenMock(...args),
}));

vi.mock("@tauri-apps/api/core", () => ({
  invoke: (...args: unknown[]) => invokeMock(...args),
}));

vi.mock("vue-i18n", async (importOriginal) => {
  const actual = await importOriginal<typeof import("vue-i18n")>();
  return {
    ...actual,
    useI18n: () => ({
      t: (key: string) => key,
    }),
  };
});

import { useAgentEvents, disposeAgentListeners } from "../useAgentEvents";

describe("useAgentEvents initListeners 生命周期与按域订阅", () => {
  beforeEach(() => {
    setActivePinia(createPinia());
    listenMock.mockReset();
    invokeMock.mockReset();
    vi.stubGlobal("window", {} as Window);
  });

  afterEach(() => {
    disposeAgentListeners();
    vi.unstubAllGlobals();
  });

  it("按域订阅：监控窗口仅注册 governance + lifecycle，跳过 stream 与聊天历史同步事件", async () => {
    const registeredEvents: string[] = [];
    const unlistenSpies: Array<ReturnType<typeof vi.fn>> = [];
    listenMock.mockImplementation(async (event: string) => {
      registeredEvents.push(event);
      const unlisten = vi.fn();
      unlistenSpies.push(unlisten);
      return unlisten;
    });

    const { initListeners, disposeListeners } = useAgentEvents();
    await initListeners({
      domains: ["governance", "lifecycle"],
      syncChatMessages: false,
    });

    expect(registeredEvents).toContain("permission-request");
    expect(registeredEvents).toContain("agent-run-updated");
    expect(registeredEvents).toContain("subagent-updated");
    expect(registeredEvents).toContain("context-snapshot-updated");
    expect(registeredEvents).not.toContain("chat-content");
    expect(registeredEvents).not.toContain("chat-thinking");
    expect(registeredEvents).not.toContain("session-messages-changed");
    expect(registeredEvents).not.toContain("checkpoint-created");
    expect(window.__jarvisListenersInitialized).toBe(true);
    expect(window.__jarvisListenersInitializing).toBe(false);

    disposeListeners();
    for (const spy of unlistenSpies) {
      expect(spy).toHaveBeenCalledTimes(1);
    }
    expect(window.__jarvisUnlisteners).toBeUndefined();
    expect(window.__jarvisListenersInitialized).toBe(false);
  });

  it("注册中途抛错时回滚已注册句柄并复位 initializing 标志，允许重试成功", async () => {
    const firstUnlisten = vi.fn();
    let callCount = 0;
    listenMock.mockImplementation(async () => {
      callCount += 1;
      if (callCount === 1) return firstUnlisten;
      if (callCount === 2) throw new Error("mock listen failure");
      return vi.fn();
    });

    const { initListeners } = useAgentEvents();
    await expect(
      initListeners({ domains: ["governance"], syncChatMessages: false }),
    ).rejects.toThrow("mock listen failure");

    // 已注册的第一个句柄必须被回滚，且初始化锁必须释放
    expect(firstUnlisten).toHaveBeenCalledTimes(1);
    expect(window.__jarvisListenersInitializing).toBe(false);
    expect(window.__jarvisListenersInitialized).toBe(false);
    expect(window.__jarvisUnlisteners).toBeUndefined();

    // 重试初始化应能顺利完成
    await initListeners({ domains: ["governance"], syncChatMessages: false });
    expect(window.__jarvisListenersInitialized).toBe(true);
    expect(window.__jarvisListenersInitializing).toBe(false);
  });

  it("HMR 重复初始化时先卸载旧监听器再注册新句柄，且并发调用直接去重", async () => {
    const round1Unlistens: Array<ReturnType<typeof vi.fn>> = [];
    const round2Unlistens: Array<ReturnType<typeof vi.fn>> = [];
    let round = 1;
    let releaseFirstListen!: () => void;
    let blockFirstCall = false;

    listenMock.mockImplementation(async () => {
      if (blockFirstCall) {
        blockFirstCall = false;
        await new Promise<void>((resolve) => {
          releaseFirstListen = resolve;
        });
      }
      const unlisten = vi.fn();
      if (round === 1) round1Unlistens.push(unlisten);
      else round2Unlistens.push(unlisten);
      return unlisten;
    });

    const { initListeners } = useAgentEvents();

    // 模拟并发初始化：第一个挂起在 listen 上，第二个应命中 initializing 锁直接跳过
    blockFirstCall = true;
    const firstInit = initListeners({ domains: ["governance"], syncChatMessages: false });
    const concurrentInit = initListeners({ domains: ["governance"], syncChatMessages: false });
    await concurrentInit;
    releaseFirstListen();
    await firstInit;

    const firstCount = round1Unlistens.length;
    expect(firstCount).toBeGreaterThan(0);

    // 模拟 HMR 重新执行 initListeners：第一轮所有 unlisten 必须先被释放，防止监听器翻倍
    round = 2;
    await initListeners({ domains: ["governance"], syncChatMessages: false });

    for (const spy of round1Unlistens) {
      expect(spy).toHaveBeenCalledTimes(1);
    }
    expect(round2Unlistens.length).toBe(firstCount);
    expect(window.__jarvisUnlisteners?.length).toBe(firstCount);
  });
});
