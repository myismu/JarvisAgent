/**
 * composables/agentEvents 单元测试
 *
 * 通过注入内存事件注册器直接驱动拆分后的三大事件域（governance / stream / lifecycle）
 * 与 runState 同步逻辑，无需真实 Tauri 运行时即可验证关键边界与历史防回归守卫。
 */
import { describe, it, expect, beforeEach, afterEach, vi } from "vitest";
import { createPinia, setActivePinia } from "pinia";

const invokeMock = vi.fn();

vi.mock("@tauri-apps/api/core", () => ({
  invoke: (...args: unknown[]) => invokeMock(...args),
}));

import { useSessionStore } from "../../stores/session";
import { useChatStore } from "../../stores/chat";
import { useAgentStore } from "../../stores/agent";
import { usePermissionStore } from "../../stores/permission";
import { registerGovernanceEvents } from "../agentEvents/governanceEvents";
import { registerStreamEvents } from "../agentEvents/streamEvents";
import { registerLifecycleEvents } from "../agentEvents/lifecycleEvents";
import { applyAgentRunState, backfillRunningUserMessageId } from "../agentEvents/runState";
import { createAgentEventsLoaders } from "../agentEvents/loaders";
import type { AgentEventsContext, EventRegisterFn } from "../agentEvents/types";
import type { AgentRun, SubAgentRun } from "../../types";

type Handler = (event: { payload: any }) => void | Promise<void>;

async function createTestHarness() {
  const session = useSessionStore();
  const chat = useChatStore();
  const agent = useAgentStore();
  const perm = usePermissionStore();
  const dismissedInterruptedRuns = new Set<string>();
  const handlers = new Map<string, Handler[]>();

  const on: EventRegisterFn = async (event, handler) => {
    const list = handlers.get(event) ?? [];
    list.push(handler as Handler);
    handlers.set(event, list);
  };

  const syncActiveSessionView = (sid: string | null | undefined, followScroll = false) => {
    if (sid === session.activeSessionId) {
      chat.triggerRender();
      if (followScroll) chat.followScrollToBottom();
    }
  };

  const applyRunState = (sid: string, runs: AgentRun[]) =>
    applyAgentRunState(session, perm, dismissedInterruptedRuns, sid, runs);

  const loaders = createAgentEventsLoaders({
    session,
    chat,
    agent,
    perm,
    syncActiveSessionView,
    applyAgentRunState: applyRunState,
  });

  const ctx: AgentEventsContext = {
    t: (key, params) => (params ? `${key}:${JSON.stringify(params)}` : key),
    session,
    chat,
    agent,
    perm,
    dismissedInterruptedRuns,
    syncActiveSessionView,
    applyAgentRunState: applyRunState,
    backfillRunningUserMessageId: (run) => backfillRunningUserMessageId(session, run),
    loaders,
  };

  await registerGovernanceEvents(on, ctx);
  await registerStreamEvents(on, ctx);
  await registerLifecycleEvents(on, ctx);

  async function emit(event: string, payload: unknown) {
    const list = handlers.get(event) ?? [];
    for (const h of list) {
      await h({ payload });
    }
  }

  return { session, chat, agent, perm, emit };
}

describe("agentEvents 事件域集成测试", () => {
  beforeEach(() => {
    setActivePinia(createPinia());
    invokeMock.mockReset();
    // Vitest Node 环境补齐 requestAnimationFrame，避免 chat.triggerRender() 报未定义
    vi.stubGlobal("requestAnimationFrame", (cb: FrameRequestCallback) => {
      cb(0);
      return 0;
    });
  });

  afterEach(() => {
    vi.unstubAllGlobals();
    vi.useRealTimers();
  });

  it("权限队列与方案审批：按 id 精确移除权限请求，提交方案自动替换正文原文", async () => {
    const { session, perm, emit } = await createTestHarness();
    session.activeSessionId = "s1";

    await emit("permission-request", { id: "p1", sessionId: "s1", message: "改文件 A" });
    await emit("permission-request", { id: "p2", sessionId: "s1", message: "改文件 B" });
    expect(perm.permissionQueue.map((item) => item.id)).toEqual(["p1", "p2"]);

    // 按 id 精确移除 p1，p2 仍保留
    await emit("permission-resolved", {
      id: "p1",
      sessionId: "s1",
      decision: "allow",
      decisionText: "已允许",
    });
    expect(perm.permissionQueue.map((item) => item.id)).toEqual(["p2"]);

    // 正文缓冲中包含方案原文，plan-proposal 到达后应被替换成 i18n 提示并登记 pending 文档
    const view = session.getSessionView("s1");
    view.contentBuffer = "前言\n完整方案内容\n后记";
    await emit("plan-proposal", {
      id: "plan_1",
      sessionId: "s1",
      title: "重构方案",
      content: "完整方案内容",
    });
    expect(view.contentBuffer).toContain("plan.submittedNotice");
    expect(view.contentBuffer).not.toContain("完整方案内容");
    expect(perm.currentPlanDocuments[0]?.id).toBe("plan_1");
    expect(perm.currentPlanDocuments[0]?.status).toBe("pending");
  });

  it("流式对话与子代理隔离：isSubAgent=true 不注入主聊天，notice_clear 仅清同来源标注", async () => {
    const { session, emit } = await createTestHarness();
    session.activeSessionId = "s1";

    await emit("chat-turn-start", { sessionId: "s1", loopCount: 2 });
    expect(session.getSessionView("s1").loopStepCount).toBe(2);

    // 主代理思考与正文正常写入
    await emit("chat-thinking", { sessionId: "s1", content: "主思考", loop: 2 });
    await emit("chat-content", { sessionId: "s1", content: "主正文", loop: 2 });
    // 子代理事件必须被忽略
    await emit("chat-thinking", { sessionId: "s1", content: "子思考", isSubAgent: true });
    await emit("chat-stream", { sessionId: "s1", content: "子输出", isSubAgent: true });

    const view = session.getSessionView("s1");
    expect(view.thinkingBuffer).toBe("主思考");
    expect(view.tempBuffer).toBe("主正文");
    expect(view.toolBuffer).toBe("");

    // 反思标注写入 noticeKind='reflection'，随后 retry 的 notice_clear 不得误伤它
    await emit("agent-step", {
      sessionId: "s1",
      type: "reflection",
      judgment: "not_ok",
      reason: "漏改边界",
      suggestion: "补单测",
    });
    expect(view.currentTurn.noticeKind).toBe("reflection");
    expect(view.currentTurn.notice).toContain("漏改边界");

    await emit("agent-step", {
      sessionId: "s1",
      type: "notice_clear",
      kinds: ["retry", "waiting_hint"],
    });
    // 来源不匹配，反思标注保持原样
    expect(view.currentTurn.noticeKind).toBe("reflection");

    await emit("agent-step", {
      sessionId: "s1",
      type: "notice_clear",
      kinds: ["reflection"],
    });
    expect(view.currentTurn.notice).toBeUndefined();
    expect(view.currentTurn.noticeKind).toBeUndefined();
  });

  it("agent-run-updated：不推进 hydration，且在开跑时把真实 messageId 回填到尾部 pending 用户消息", async () => {
    const { session, emit } = await createTestHarness();
    session.activeSessionId = "s1";

    const view = session.getSessionView("s1");
    view.status = "RUNNING";
    view.runStartTime = 1000;
    view.messages = [{ id: "pending_1", role: "user", text: "你好" }];
    expect(view.hydration).toBe("idle");

    const run: AgentRun = {
      runId: "ar_1",
      sessionId: "s1",
      status: "running",
      messageId: "msg_real_uuid",
      loopCount: 1,
      inputTokens: 0,
      outputTokens: 0,
      startedAt: 1200,
      updatedAt: 1200,
      resumable: false,
    };
    await emit("agent-run-updated", run);

    // 真实 UUID 已回填，但 hydration 保持 idle 不被误置 ready
    expect(view.messages[0].messageId).toBe("msg_real_uuid");
    expect(view.hydration).toBe("idle");
  });

  it("session-messages-changed：同一微任务合并，非活跃会话只标脏零 IPC，活跃空闲会话触发重载", async () => {
    const { session, emit } = await createTestHarness();
    session.activeSessionId = "s_active";
    session.markSessionReady("s_active");
    session.markSessionReady("s_bg");

    invokeMock.mockImplementation((cmd: string) => {
      if (cmd === "get_session_messages_paged") {
        return Promise.resolve({
          messages: [{ id: "m1", role: "assistant", content: "已同步" }],
          hasMore: false,
          oldestSeq: 1,
          newestSeq: 1,
          totalCount: 1,
        });
      }
      return Promise.resolve(null);
    });

    // 同一同步栈内连发两次 → 同一微任务内合并（不能在中间 await，否则微任务已提前冲刷）
    void emit("session-messages-changed", {
      sessionId: "s_bg",
      count: 1,
      lastSeq: 5,
      reason: "turn-end",
    });
    void emit("session-messages-changed", {
      sessionId: "s_bg",
      count: 2,
      lastSeq: 6,
      reason: "subagent",
    });

    void emit("session-messages-changed", {
      sessionId: "s_active",
      count: 1,
      lastSeq: 1,
      reason: "turn-end",
    });
    void emit("session-messages-changed", {
      sessionId: "s_active",
      count: 2,
      lastSeq: 2,
      reason: "turn-end",
    });

    // 等待微任务队列冲刷
    await Promise.resolve();
    await Promise.resolve();

    expect(session.getSessionView("s_bg").hydration).toBe("stale");
    expect(session.getSessionView("s_bg").staleReason).toBe("messages-changed:subagent");
    // 只有活跃会话发了且仅发了一次分页拉取 IPC
    const pagedCalls = invokeMock.mock.calls.filter(([cmd]) => cmd === "get_session_messages_paged");
    expect(pagedCalls).toHaveLength(1);
    expect(pagedCalls[0]).toEqual([
      "get_session_messages_paged",
      {
        sessionId: "s_active",
        beforeSeq: null,
        limit: 5,
      },
    ]);
  });

  it("subagent-updated：2 秒内多次到达合并为一次批量写入 store", async () => {
    vi.useFakeTimers();
    const { agent, emit } = await createTestHarness();

    const makeSubRun = (runId: string, loopCount: number): SubAgentRun => ({
      runId,
      sessionId: "s1",
      label: `子任务 ${runId}`,
      agentRole: "research",
      promptPreview: "preview",
      readOnly: true,
      status: "running",
      phase: "streaming",
      loopCount,
      maxLoops: 10,
      inputTokens: 100,
      outputTokens: 50,
      startedAt: 1000,
      updatedAt: 1000 + loopCount * 100,
    });

    await emit("subagent-updated", makeSubRun("sub_1", 1));
    await emit("subagent-updated", makeSubRun("sub_1", 2));
    await emit("subagent-updated", makeSubRun("sub_2", 1));

    // 2 秒前尚未刷入 store
    expect(Object.keys(agent.subAgentRuns)).toHaveLength(0);

    await vi.advanceTimersByTimeAsync(2000);

    expect(agent.subAgentRuns["sub_1"]?.loopCount).toBe(2);
    expect(agent.subAgentRuns["sub_2"]?.loopCount).toBe(1);
    expect(agent.subAgentTraceRunsBySession["s1"]).toHaveLength(2);
  });
});
