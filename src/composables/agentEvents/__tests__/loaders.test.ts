/**
 * composables/agentEvents/loaders.ts 单元测试
 *
 * 覆盖并发去重、轨迹加载版本号竞态防乱序以及主 Agent 运行事件按 loopIndex 排序分组。
 */
import { describe, it, expect, beforeEach, vi } from "vitest";
import { createPinia, setActivePinia } from "pinia";

const invokeMock = vi.fn();

vi.mock("@tauri-apps/api/core", () => ({
  invoke: (...args: unknown[]) => invokeMock(...args),
}));

import { useSessionStore } from "../../../stores/session";
import { useChatStore } from "../../../stores/chat";
import { useAgentStore } from "../../../stores/agent";
import { usePermissionStore } from "../../../stores/permission";
import { createAgentEventsLoaders } from "../loaders";
import type { AgentRunLoopEvent, SubAgentRun, SubAgentTraceEvent } from "../../../types";

function createDeferred<T>() {
  let resolve!: (value: T) => void;
  let reject!: (reason?: unknown) => void;
  const promise = new Promise<T>((res, rej) => {
    resolve = res;
    reject = rej;
  });
  return { promise, resolve, reject };
}

describe("createAgentEventsLoaders 并发去重与竞态版本保护", () => {
  beforeEach(() => {
    setActivePinia(createPinia());
    invokeMock.mockReset();
  });

  function setupLoaders() {
    const session = useSessionStore();
    const chat = useChatStore();
    const agent = useAgentStore();
    const perm = usePermissionStore();
    const syncActiveSessionView = vi.fn();
    const applyAgentRunState = vi.fn();
    const loaders = createAgentEventsLoaders({
      session,
      chat,
      agent,
      perm,
      syncActiveSessionView,
      applyAgentRunState,
    });
    return { session, chat, agent, perm, syncActiveSessionView, applyAgentRunState, loaders };
  }

  it("loadSubAgentTraceFromBackend：旧版本晚到响应不得覆盖新版本轨迹，已清理条目不得复活", async () => {
    const { agent, loaders } = setupLoaders();
    const first = createDeferred<SubAgentTraceEvent[]>();
    const second = createDeferred<SubAgentTraceEvent[]>();

    let callSeq = 0;
    invokeMock.mockImplementation((cmd: string) => {
      if (cmd === "list_subagent_trace_events") {
        callSeq += 1;
        return callSeq === 1 ? first.promise : second.promise;
      }
      return Promise.resolve([]);
    });

    const p1 = loaders.loadSubAgentTraceFromBackend("run_1");
    const p2 = loaders.loadSubAgentTraceFromBackend("run_1");

    // 较晚发起的 v2 先返回
    second.resolve([
      {
        eventId: "ev-2",
        runId: "run_1",
        sessionId: "s1",
        loopIndex: 2,
        eventType: "text",
        payload: "v2-fresh",
        timestamp: 2000,
      },
    ]);
    await p2;
    expect(agent.getSubAgentTrace("run_1")[0]?.payload).toBe("v2-fresh");

    // 较早发起的 v1 晚到：必须被版本号守卫丢弃
    first.resolve([
      {
        eventId: "ev-1",
        runId: "run_1",
        sessionId: "s1",
        loopIndex: 1,
        eventType: "text",
        payload: "v1-stale",
        timestamp: 1000,
      },
    ]);
    await p1;
    expect(agent.getSubAgentTrace("run_1")[0]?.payload).toBe("v2-fresh");

    // 若轨迹在加载期间被用户收起清理，晚到响应不得重新写入
    const third = createDeferred<SubAgentTraceEvent[]>();
    invokeMock.mockImplementationOnce(() => third.promise);
    const p3 = loaders.loadSubAgentTraceFromBackend("run_2");
    agent.clearSubAgentTrace("run_2");
    third.resolve([
      {
        eventId: "ev-3",
        runId: "run_2",
        sessionId: "s1",
        loopIndex: 1,
        eventType: "text",
        payload: "ghost",
        timestamp: 3000,
      },
    ]);
    await p3;
    expect(Object.prototype.hasOwnProperty.call(agent.subAgentTraceByRun, "run_2")).toBe(false);
  });

  it("loadSubAgentRunsFromBackend：同会话并发请求去重复用同一 Promise，且不误删其他会话数据", async () => {
    const { agent, loaders } = setupLoaders();
    agent.subAgentRuns = {
      other_run: {
        runId: "other_run",
        sessionId: "s_other",
        label: "keep",
        agentRole: "explore",
        promptPreview: "other",
        readOnly: true,
        status: "running",
        phase: "thinking",
        loopCount: 1,
        maxLoops: 10,
        inputTokens: 0,
        outputTokens: 0,
        startedAt: 100,
        updatedAt: 100,
      },
    };

    const runsDef = createDeferred<SubAgentRun[]>();
    const traceRunsDef = createDeferred<unknown[]>();

    invokeMock.mockImplementation((cmd: string) => {
      if (cmd === "list_subagents") return runsDef.promise;
      if (cmd === "list_subagent_trace_runs") return traceRunsDef.promise;
      return Promise.resolve([]);
    });

    const p1 = loaders.loadSubAgentRunsFromBackend("s1");
    const p2 = loaders.loadSubAgentRunsFromBackend("s1");

    runsDef.resolve([
      {
        runId: "s1_run",
        sessionId: "s1",
        label: "worker",
        agentRole: "explore",
        promptPreview: "scan",
        readOnly: true,
        status: "completed",
        phase: "finalizing",
        loopCount: 2,
        maxLoops: 10,
        inputTokens: 10,
        outputTokens: 20,
        startedAt: 200,
        updatedAt: 300,
      },
    ]);
    traceRunsDef.resolve([]);
    await Promise.all([p1, p2]);

    expect(invokeMock.mock.calls.filter(([cmd]) => cmd === "list_subagents")).toHaveLength(1);
    expect(invokeMock.mock.calls.filter(([cmd]) => cmd === "list_subagent_trace_runs")).toHaveLength(1);
    expect(Object.keys(agent.subAgentRuns).sort()).toEqual(["other_run", "s1_run"]);
  });

  it("loadAgentRunEventsFromBackend：按 runId 分组并按 loopIndex 升序排列，保留其他会话记录", async () => {
    const { agent, loaders } = setupLoaders();
    agent.agentRunEventsByRun = {
      other_run: [
        {
          eventId: "ev-99",
          runId: "other_run",
          sessionId: "s_other",
          loopIndex: 1,
          respBlocks: [],
          toolResults: [],
          status: "complete",
          inputTokens: 0,
          outputTokens: 0,
          startedAt: 100,
          updatedAt: 100,
        },
      ],
    };

    const rawEvents: AgentRunLoopEvent[] = [
      {
        eventId: "ev-2",
        runId: "r1",
        sessionId: "s1",
        loopIndex: 3,
        respBlocks: [],
        toolResults: [],
        status: "complete",
        inputTokens: 10,
        outputTokens: 5,
        startedAt: 300,
        updatedAt: 310,
      },
      {
        eventId: "ev-1",
        runId: "r1",
        sessionId: "s1",
        loopIndex: 1,
        respBlocks: [],
        toolResults: [],
        status: "complete",
        inputTokens: 10,
        outputTokens: 5,
        startedAt: 100,
        updatedAt: 110,
      },
      {
        eventId: "ev-3",
        runId: "r1",
        sessionId: "s1",
        loopIndex: 2,
        respBlocks: [],
        toolResults: [],
        status: "complete",
        inputTokens: 10,
        outputTokens: 5,
        startedAt: 200,
        updatedAt: 210,
      },
    ];
    invokeMock.mockResolvedValueOnce(rawEvents);

    await loaders.loadAgentRunEventsFromBackend("s1");

    expect(agent.getAgentRunEvents("r1").map((e) => e.loopIndex)).toEqual([1, 2, 3]);
    expect(agent.getAgentRunEvents("other_run")).toHaveLength(1);
  });
});
