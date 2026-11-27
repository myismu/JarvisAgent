/**
 * stores/chat.ts 单元测试
 *
 * 覆盖会话视图装载状态机与懒加载分页防重入：
 * 1. ensureSessionViewLoaded：idle 发首屏 IPC、ready 零 IPC、并发调用复用同一在途 Promise
 * 2. markSessionViewStale 标脏后，再次 ensureSessionViewLoaded 重新拉取最新首屏
 * 3. loadSessionMessagesOlder：按 oldestSeq 向前翻页头部插入，hasMore=false 时零 IPC
 */
import { describe, it, expect, beforeEach, afterEach, vi } from "vitest";
import { createPinia, setActivePinia } from "pinia";
import { isReactive } from "vue";

const invokeMock = vi.fn();

vi.mock("@tauri-apps/api/core", () => ({
  invoke: (...args: unknown[]) => invokeMock(...args),
}));

vi.mock("../../composables/usePreferences", () => ({
  usePreferences: () => ({
    agentAudience: { value: "developer" },
    reflectionMode: "smart",
  }),
}));

import { useSessionStore } from "../session";
import { useChatStore } from "../chat";
import { usePermissionStore } from "../permission";

describe("useChatStore hydration & paging", () => {
  beforeEach(() => {
    setActivePinia(createPinia());
    invokeMock.mockReset();
  });

  it("ensureSessionViewLoaded：idle 拉首屏、ready 零 IPC、并发复用在途 Promise", async () => {
    const session = useSessionStore();
    const chat = useChatStore();

    let resolvePaged!: (val: unknown) => void;
    invokeMock.mockImplementation((cmd: string) => {
      if (cmd === "get_session_messages_paged") {
        return new Promise((resolve) => {
          resolvePaged = resolve;
        });
      }
      return Promise.resolve(null);
    });

    // 同一会话并发触发两次加载 → 应合并为一次 IPC
    const p1 = chat.ensureSessionViewLoaded("s1", "mount");
    const p2 = chat.ensureSessionViewLoaded("s1", "switch");
    expect(session.getSessionView("s1").hydration).toBe("loading");

    resolvePaged({
      messages: [
        { id: "u1", role: "user", userContent: "第一问" },
        { id: "a1", role: "assistant", content: "第一答" },
      ],
      hasMore: true,
      oldestSeq: 12,
      newestSeq: 20,
      totalCount: 20,
    });
    await Promise.all([p1, p2]);

    const pagedCalls = invokeMock.mock.calls.filter(([cmd]) => cmd === "get_session_messages_paged");
    expect(pagedCalls).toHaveLength(1);

    const view = session.getSessionView("s1");
    expect(view.hydration).toBe("ready");
    expect(view.messages).toHaveLength(2);
    expect(view.pagedOldestSeq).toBe(12);
    expect(view.pagedHasMore).toBe(true);
    expect(view.loadedNewestSeq).toBe(20);

    // 已是 ready：再次调用应零 IPC 直接返回
    await chat.ensureSessionViewLoaded("s1", "revisit");
    expect(
      invokeMock.mock.calls.filter(([cmd]) => cmd === "get_session_messages_paged"),
    ).toHaveLength(1);
  });

  it("markSessionViewStale 标脏后，再次 ensureSessionViewLoaded 重新拉取最新首屏", async () => {
    const session = useSessionStore();
    const chat = useChatStore();

    let pagedCallCount = 0;
    invokeMock.mockImplementation((cmd: string) => {
      if (cmd === "get_session_messages_paged") {
        pagedCallCount += 1;
        if (pagedCallCount === 1) {
          return Promise.resolve({
            messages: [{ id: "m1", role: "user", userContent: "旧消息" }],
            hasMore: false,
            oldestSeq: 1,
            newestSeq: 1,
            totalCount: 1,
          });
        }
        return Promise.resolve({
          messages: [
            { id: "m1", role: "user", userContent: "旧消息" },
            { id: "m2", role: "assistant", content: "新回复" },
          ],
          hasMore: false,
          oldestSeq: 1,
          newestSeq: 2,
          totalCount: 2,
        });
      }
      return Promise.resolve(null);
    });

    await chat.ensureSessionViewLoaded("s1", "init");
    expect(session.getSessionView("s1").messages).toHaveLength(1);

    chat.markSessionViewStale("s1", "messages-changed:turn-end");
    expect(session.getSessionView("s1").hydration).toBe("stale");

    await chat.ensureSessionViewLoaded("s1", "re-enter");
    expect(session.getSessionView("s1").hydration).toBe("ready");
    expect(session.getSessionView("s1").messages).toHaveLength(2);
  });

  it("loadSessionMessagesOlder：按 oldestSeq 向前翻页头部插入，无更多历史时零 IPC", async () => {
    const session = useSessionStore();
    const chat = useChatStore();

    session.replaceSessionMessages("s1", [{ id: "m3", role: "user", userContent: "最新" }]);
    session.setSessionPaging("s1", 10, true, 15);

    invokeMock.mockResolvedValueOnce({
      messages: [
        { id: "m1", role: "user", userContent: "更早1" },
        { id: "m2", role: "assistant", content: "更早2" },
      ],
      hasMore: false,
      oldestSeq: 1,
      newestSeq: 9,
      totalCount: 15,
    });

    const loaded = await chat.loadSessionMessagesOlder("s1");
    expect(loaded).toBe(true);
    expect(invokeMock).toHaveBeenCalledWith("get_session_messages_paged", {
      sessionId: "s1",
      beforeSeq: 10,
      limit: 5,
    });

    const view = session.getSessionView("s1");
    expect(view.messages.map((m) => m.id)).toEqual(["m1", "m2", "m3"]);
    expect(view.pagedOldestSeq).toBe(1);
    expect(view.pagedHasMore).toBe(false);
    // 向上翻页不得覆盖 loadedNewestSeq（窗口最新端仍为 15）
    expect(view.loadedNewestSeq).toBe(15);

    // hasMore=false 后再次翻页应直接返回 false，不再发 IPC
    const loadedAgain = await chat.loadSessionMessagesOlder("s1");
    expect(loadedAgain).toBe(false);
    expect(invokeMock).toHaveBeenCalledTimes(1);
  });
});

describe("useChatStore render throttling & scroll callbacks", () => {
  beforeEach(() => {
    setActivePinia(createPinia());
    invokeMock.mockReset();
    vi.useFakeTimers();
    let now = 1000;
    vi.stubGlobal("performance", { now: () => now });
    vi.stubGlobal("requestAnimationFrame", (cb: FrameRequestCallback) => {
      cb(now);
      return 1;
    });
    vi.stubGlobal("cancelAnimationFrame", vi.fn());
  });

  afterEach(() => {
    vi.unstubAllGlobals();
    vi.useRealTimers();
  });

  it("33ms 节流窗口内切换会话，原会话 throttling 正常复位不永久锁死", async () => {
    const session = useSessionStore();
    const chat = useChatStore();
    session.activeSessionId = "s1";

    // 第一次触发走 rAF 立即完成，记录 s1 的 lastRenderAt = 1000
    chat.triggerRender();
    expect(chat.renderTick).toBe(1);

    // 紧接着第二次触发（elapsed = 0 < 33ms）进入 33ms setTimeout 节流分支
    chat.triggerRender();
    expect(chat.renderTick).toBe(1);

    // 在 33ms 定时器到期前切到 s2
    session.activeSessionId = "s2";

    // 推进 40ms，s1 的定时器到期：s1 状态复位但不应替 s2 误增 renderTick
    await vi.advanceTimersByTimeAsync(40);
    expect(chat.renderTick).toBe(1);

    // 切回 s1 后再次 triggerRender，必须能正常递增（若旧 bug 存在此处会永久被锁死）
    session.activeSessionId = "s1";
    vi.stubGlobal("performance", { now: () => 2000 });
    chat.triggerRender();
    expect(chat.renderTick).toBe(2);
  });

  it("resetRenderState 取消在途节流定时器且允许立即重新渲染", async () => {
    const session = useSessionStore();
    const chat = useChatStore();
    session.activeSessionId = "s1";

    chat.triggerRender();
    expect(chat.renderTick).toBe(1);

    // 进入 33ms 定时器节流态
    chat.triggerRender();
    // 立即复位后，定时器应被取消，且再调用可立即走 rAF
    chat.resetRenderState("s1");
    await vi.advanceTimersByTimeAsync(50);
    expect(chat.renderTick).toBe(1);

    chat.triggerRender();
    expect(chat.renderTick).toBe(2);
  });

  it("unregisterScrollCb 仅清除匹配回调，不误清后续注册的新回调", () => {
    const chat = useChatStore();
    const cb1 = vi.fn();
    const cb2 = vi.fn();

    chat.registerScrollCb(cb1);
    chat.registerScrollCb(cb2);
    // 旧组件卸载尝试注销 cb1，不得清掉已生效的 cb2
    chat.unregisterScrollCb(cb1);
    chat.forceScrollToBottom();
    expect(cb1).not.toHaveBeenCalled();
    expect(cb2).toHaveBeenCalledWith(true);

    // 注销匹配的 cb2 后不再触发
    chat.unregisterScrollCb(cb2);
    chat.forceScrollToBottom();
    expect(cb2).toHaveBeenCalledTimes(1);
  });

  it("历史消息与终态快照应用 markRaw 跳过深度 Proxy，待回填用户消息保持响应式", () => {
    const session = useSessionStore();
    const historySnapshot = {
      version: 1 as const,
      status: "FINISH",
      textBlocks: [],
      thinkingBlocks: [],
      toolCalls: [],
      logs: [],
      createdAt: 1000,
    };

    session.replaceSessionMessages("s1", [
      { id: "a1", role: "agent", snapshot: historySnapshot },
    ]);
    const view = session.getSessionView("s1");
    expect(isReactive(view.messages[0])).toBe(false);
    expect(isReactive(view.messages[0].snapshot)).toBe(false);

    // 运行期刚发出的 pending 用户消息必须保持响应式，以便回执回填 messageId / rollbackMode
    session.appendSessionMessage("s1", {
      id: "pending_1",
      role: "user",
      text: "测试消息",
    });
    expect(isReactive(view.messages[1])).toBe(true);
  });
});

describe("useChatStore 核心主链路（发送、并发代次、取消、撤回与续跑）", () => {
  beforeEach(() => {
    setActivePinia(createPinia());
    invokeMock.mockReset();
    let seq = 0;
    vi.stubGlobal("crypto", {
      randomUUID: () => `uuid-${++seq}`,
    });
    vi.stubGlobal("performance", { now: () => 1000 });
    vi.stubGlobal("requestAnimationFrame", (cb: FrameRequestCallback) => {
      cb(1000);
      return 1;
    });
    vi.stubGlobal("cancelAnimationFrame", vi.fn());
  });

  afterEach(() => {
    vi.unstubAllGlobals();
  });

  it("sendToJarvis：正常发送后按回执回填 messageId 与代码回滚模式并落盘快照", async () => {
    const session = useSessionStore();
    const chat = useChatStore();
    session.activeSessionId = "s1";

    invokeMock.mockImplementation(async (cmd: string) => {
      if (cmd === "recover_interrupted_session_messages") return false;
      if (cmd === "ask_jarvis") {
        return {
          status: "FINISH",
          content: "已完成修改",
          user_message_id: "msg-server-1",
          checkpoint_has_patches: true,
          checkpoint_id: "cp-1",
          session_input_tokens: 120,
          session_output_tokens: 45,
          session_cache_hit_tokens: 60,
          session_cache_miss_tokens: 60,
        };
      }
      if (cmd === "get_session_context_snapshot") return null;
      return null;
    });

    await chat.sendToJarvis("请帮我重构");

    const view = session.getSessionView("s1");
    expect(view.status).toBe("IDLE");
    expect(view.messages).toHaveLength(2);
    expect(view.messages[0]).toMatchObject({
      role: "user",
      text: "请帮我重构",
      messageId: "msg-server-1",
      rollbackMode: "both",
      rollbackCheckpointId: "cp-1",
    });
    expect(view.messages[1]).toMatchObject({
      role: "agent",
    });
    expect(view.messages[1].snapshot?.status).toBe("FINISH");
    expect(view.messages[1].snapshot?.finalContent).toBe("已完成修改");
    expect(view.sessionInputTokens).toBe(120);
    expect(view.sessionOutputTokens).toBe(45);
  });

  it("sendToJarvis 并发代次竞态：旧请求晚到回执绝不污染新轮状态与消息", async () => {
    const session = useSessionStore();
    const chat = useChatStore();
    session.activeSessionId = "s1";

    let resolveFirstAsk!: (val: unknown) => void;
    let resolveSecondAsk!: (val: unknown) => void;
    let askCount = 0;

    invokeMock.mockImplementation((cmd: string) => {
      if (cmd === "recover_interrupted_session_messages") return Promise.resolve(false);
      if (cmd === "get_session_context_snapshot") return Promise.resolve(null);
      if (cmd === "ask_jarvis") {
        askCount += 1;
        return new Promise((resolve) => {
          if (askCount === 1) resolveFirstAsk = resolve;
          else resolveSecondAsk = resolve;
        });
      }
      return Promise.resolve(null);
    });

    const p1 = chat.sendToJarvis("第一轮", null, undefined, false, true);
    await vi.waitFor(() => expect(askCount).toBe(1));

    // 启动第二轮（generation 递增），且第二轮先完成
    const p2 = chat.sendToJarvis("第二轮", null, undefined, false, true);
    await vi.waitFor(() => expect(askCount).toBe(2));

    resolveSecondAsk({
      status: "FINISH",
      content: "第二轮回复",
      user_message_id: "msg-2",
      session_input_tokens: 20,
      session_output_tokens: 10,
      session_cache_hit_tokens: 0,
      session_cache_miss_tokens: 0,
    });
    await p2;

    const view = session.getSessionView("s1");
    expect(view.status).toBe("IDLE");
    const countAfterSecond = view.messages.length;

    // 第一轮旧回执晚到：必须被 myGeneration !== sendGeneration 拦截丢弃
    resolveFirstAsk({
      status: "FINISH",
      content: "过期的第一轮回复",
      user_message_id: "msg-1-stale",
      session_input_tokens: 999,
      session_output_tokens: 999,
      session_cache_hit_tokens: 0,
      session_cache_miss_tokens: 0,
    });
    await p1;

    expect(view.messages).toHaveLength(countAfterSecond);
    expect(view.messages.some((m) => m.snapshot?.finalContent === "过期的第一轮回复")).toBe(false);
    expect(view.sessionInputTokens).toBe(20);
  });

  it("sendToJarvis 异常兜底：ExecutionBusy 移除占位消息并返回 retryDraft；CommandError 写入错误快照", async () => {
    const session = useSessionStore();
    const chat = useChatStore();
    session.activeSessionId = "s1";

    invokeMock.mockImplementation(async (cmd: string) => {
      if (cmd === "recover_interrupted_session_messages") return false;
      if (cmd === "ask_jarvis") throw { ExecutionBusy: "会话正在执行" };
      return null;
    });

    const busyOutcome = await chat.sendToJarvis("冲突发送", null, undefined, false, true);
    const view = session.getSessionView("s1");
    expect(busyOutcome).toEqual({ retryDraft: { sessionId: "s1", text: "冲突发送" } });
    // 占位用户消息已被移除，仅剩错误快照
    expect(view.messages.some((m) => m.role === "user")).toBe(false);
    expect(view.status).toBe("ERROR");

    // 换普通结构化 CommandError 失败：保留用户消息并提取结构化错误 message 写入 noticeDetail
    invokeMock.mockImplementation(async (cmd: string) => {
      if (cmd === "recover_interrupted_session_messages") return false;
      if (cmd === "ask_jarvis") {
        throw { code: "CONFIG_ERROR", message: "未配置有效模型预设", retryable: false };
      }
      return null;
    });

    const normalErrOutcome = await chat.sendToJarvis("正常失败", null, undefined, false, true);
    expect(normalErrOutcome).toBeUndefined();
    const lastMsg = view.messages[view.messages.length - 1];
    expect(lastMsg.snapshot?.status).toBe("ERROR");
    expect(lastMsg.snapshot?.noticeI18nKey).toBe("notice.executionError");
    expect(lastMsg.snapshot?.noticeDetail).toBe("未配置有效模型预设");
  });

  it("cancelJarvis：精准取消目标会话并清权限，不漂移其他会话；IPC 失败时复位 stopping", async () => {
    const session = useSessionStore();
    const perm = usePermissionStore();
    const chat = useChatStore();

    session.activeSessionId = "s2";
    const v1 = session.getSessionView("s1");
    const v2 = session.getSessionView("s2");
    v1.status = "RUNNING";
    v2.status = "IDLE";

    // 当前活跃会话 s2 未运行：cancelJarvis() 绝不能漂移去停正在运行的 s1
    expect(await chat.cancelJarvis()).toBe(false);
    expect(invokeMock).not.toHaveBeenCalled();

    // 明确指定取消 s1：清理 s1 权限并调用后端 cancel_jarvis
    perm.enqueuePermission({ id: "p1", message: "允许？", sessionId: "s1" });
    invokeMock.mockResolvedValueOnce(undefined);
    expect(await chat.cancelJarvis("s1")).toBe(true);
    expect(invokeMock).toHaveBeenCalledWith("cancel_jarvis", { sessionId: "s1" });
    expect(v1.stopping).toBe(true);
    expect(perm.isSessionWaitingForUser("s1")).toBe(false);

    // 若后端取消命令抛错，必须撤回 stopping 防止停止按钮永久禁用
    v1.stopping = false;
    invokeMock.mockRejectedValueOnce(new Error("ipc down"));
    expect(await chat.cancelJarvis("s1")).toBe(false);
    expect(v1.stopping).toBe(false);
  });

  it("recallAndEdit：定位最新已落库用户消息，截断其后本地消息并返回原文", async () => {
    const session = useSessionStore();
    const chat = useChatStore();
    session.activeSessionId = "s1";

    const view = session.getSessionView("s1");
    view.messages = [
      { id: "u1", role: "user", messageId: "mid-1", text: "第一句" },
      { id: "a1", role: "agent", snapshot: { version: 1, status: "FINISH", textBlocks: [], thinkingBlocks: [], toolCalls: [], logs: [], createdAt: 1 } },
      { id: "u2", role: "user", messageId: "mid-2", text: "第二句" },
      { id: "a2", role: "agent", snapshot: { version: 1, status: "FINISH", textBlocks: [], thinkingBlocks: [], toolCalls: [], logs: [], createdAt: 2 } },
    ];
    view.showRecallEdit = true;

    invokeMock.mockResolvedValueOnce("第二句");
    const text = await chat.recallAndEdit();

    expect(text).toBe("第二句");
    expect(invokeMock).toHaveBeenCalledWith("recall_message", {
      sessionId: "s1",
      messageId: "mid-2",
      pruneMetadataCutoff: null,
    });
    expect(view.messages.map((m) => m.id)).toEqual(["u1", "a1"]);
    expect(view.showRecallEdit).toBe(false);
  });

  it("resumeAgentRun：会话匹配时重载首屏并续跑，会话不匹配时安全早退", async () => {
    const session = useSessionStore();
    const chat = useChatStore();
    session.activeSessionId = "s1";

    // 情况 1：返回的 plan 属于其他会话 s_other → 早退，不触发 ask_jarvis
    invokeMock.mockResolvedValueOnce({
      runId: "run-other",
      sessionId: "s_other",
      prompt: "继续执行",
    });
    await chat.resumeAgentRun("run-other");
    expect(invokeMock.mock.calls.some(([cmd]) => cmd === "ask_jarvis")).toBe(false);

    // 情况 2：会话匹配 → 重载首屏并调用 ask_jarvis 发送续跑提示
    invokeMock.mockReset();
    invokeMock.mockImplementation(async (cmd: string) => {
      if (cmd === "prepare_resume_agent_run") {
        return { runId: "run-1", sessionId: "s1", prompt: "请继续未完成任务" };
      }
      if (cmd === "get_session_messages_paged") {
        return { messages: [], hasMore: false, oldestSeq: null, newestSeq: null, totalCount: 0 };
      }
      if (cmd === "get_session_usage") return null;
      if (cmd === "recover_interrupted_session_messages") return false;
      if (cmd === "ask_jarvis") {
        return {
          status: "FINISH",
          content: "续跑完成",
          user_message_id: "msg-resume",
          session_input_tokens: 10,
          session_output_tokens: 5,
          session_cache_hit_tokens: 0,
          session_cache_miss_tokens: 0,
        };
      }
      if (cmd === "get_session_context_snapshot") return null;
      return null;
    });

    await chat.resumeAgentRun("run-1");
    expect(invokeMock.mock.calls.some(([cmd]) => cmd === "get_session_messages_paged")).toBe(true);
    const askCall = invokeMock.mock.calls.find(([cmd]) => cmd === "ask_jarvis");
    expect(askCall?.[1]).toMatchObject({ sessionId: "s1", msg: "请继续未完成任务" });
  });
});
