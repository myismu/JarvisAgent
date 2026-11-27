/**
 * stores/agent.ts 单元测试
 *
 * 覆盖会话级运行态隔离排序、子 Agent 轨迹 pin 保护、上下文快照写入/清理以及任务聚焦事件。
 */
import { describe, it, expect, beforeEach, afterEach, vi } from "vitest";
import { createPinia, setActivePinia } from "pinia";
import { useSessionStore } from "../session";
import { useAgentStore } from "../agent";
import type { AgentRun, SessionContextSnapshot, SubAgentRun } from "../../types";

function makeSubAgentRun(partial: Partial<SubAgentRun> & { runId: string; sessionId: string }): SubAgentRun {
  return {
    label: "worker",
    agentRole: "explore",
    promptPreview: "preview",
    readOnly: true,
    status: "running",
    phase: "thinking",
    loopCount: 1,
    maxLoops: 10,
    inputTokens: 0,
    outputTokens: 0,
    startedAt: 1000,
    updatedAt: 1000,
    ...partial,
  };
}

function makeAgentRun(partial: Partial<AgentRun> & { runId: string; sessionId: string }): AgentRun {
  return {
    status: "completed",
    loopCount: 1,
    inputTokens: 0,
    outputTokens: 0,
    startedAt: 1000,
    updatedAt: 1000,
    resumable: false,
    ...partial,
  };
}

describe("useAgentStore 会话隔离、排序与轨迹生命周期", () => {
  beforeEach(() => {
    setActivePinia(createPinia());
  });

  afterEach(() => {
    vi.unstubAllGlobals();
  });

  it("按当前活跃会话隔离并正确排序 SubAgentRun、AgentRun 与 TodoGroups", () => {
    const session = useSessionStore();
    const agent = useAgentStore();
    session.activeSessionId = "s1";

    agent.todoGroupsBySession = {
      s1: [
        {
          id: "g1",
          items: [
            { id: "t1", content: "项1", status: "completed", activeForm: "项1" },
            { id: "t2", content: "项2", status: "in_progress", activeForm: "项2" },
          ],
        },
      ],
      s2: [
        {
          id: "g2",
          items: [{ id: "t3", content: "其他", status: "pending", activeForm: "其他" }],
        },
      ],
    };

    agent.subAgentRuns = {
      sub_no_task: makeSubAgentRun({ runId: "sub_no_task", sessionId: "s1", startedAt: 100, status: "completed" }),
      sub_t2: makeSubAgentRun({ runId: "sub_t2", sessionId: "s1", taskId: 2, startedAt: 300, status: "running" }),
      sub_t1_late: makeSubAgentRun({ runId: "sub_t1_late", sessionId: "s1", taskId: 1, startedAt: 250, status: "running" }),
      sub_t1_early: makeSubAgentRun({ runId: "sub_t1_early", sessionId: "s1", taskId: 1, startedAt: 150, status: "completed" }),
      sub_other: makeSubAgentRun({ runId: "sub_other", sessionId: "s2", taskId: 1, startedAt: 50, status: "running" }),
    };

    agent.upsertAgentRun(makeAgentRun({ runId: "r_old", sessionId: "s1", startedAt: 100, status: "interrupted", resumable: true }));
    agent.upsertAgentRun(makeAgentRun({ runId: "r_non_resume", sessionId: "s1", startedAt: 200, status: "interrupted", resumable: false }));
    agent.upsertAgentRun(makeAgentRun({ runId: "r_new", sessionId: "s1", startedAt: 300, status: "running", resumable: false }));
    agent.upsertAgentRun(makeAgentRun({ runId: "r_s2", sessionId: "s2", startedAt: 999, status: "interrupted", resumable: true }));

    expect(agent.currentTodos.map((item) => item.id)).toEqual(["t1", "t2"]);
    // SubAgent 按 taskId 升序，同 taskId 按 startedAt 升序，无 taskId 排最后
    expect(agent.currentSubAgentRuns.map((r) => r.runId)).toEqual([
      "sub_t1_early",
      "sub_t1_late",
      "sub_t2",
      "sub_no_task",
    ]);
    expect(agent.activeSubAgentRuns.map((r) => r.runId)).toEqual(["sub_t1_late", "sub_t2"]);

    // AgentRun 按 startedAt 降序，interruptedAgentRuns 仅含 interrupted && resumable
    expect(agent.currentAgentRuns.map((r) => r.runId)).toEqual(["r_new", "r_non_resume", "r_old"]);
    expect(agent.interruptedAgentRuns.map((r) => r.runId)).toEqual(["r_old"]);

    // 切到无会话态时全部返回空数组 / null
    session.activeSessionId = null;
    expect(agent.currentTodos).toEqual([]);
    expect(agent.currentSubAgentRuns).toEqual([]);
    expect(agent.currentAgentRuns).toEqual([]);
    expect(agent.currentContextSnapshot).toBeNull();
  });

  it("pinSubAgentTrace 保护已固定轨迹不被误清，取消固定后可回收", () => {
    const agent = useAgentStore();
    agent.subAgentTraceByRun = {
      run_a: [
        {
          eventId: "ev-1",
          runId: "run_a",
          sessionId: "s1",
          loopIndex: 1,
          eventType: "text",
          payload: "trace-a",
          timestamp: 1000,
        },
      ],
    };

    agent.pinSubAgentTrace("run_a");
    agent.clearSubAgentTrace("run_a");
    expect(agent.getSubAgentTrace("run_a")).toHaveLength(1);

    agent.pinSubAgentTrace(null);
    agent.clearSubAgentTrace("run_a");
    expect(agent.getSubAgentTrace("run_a")).toEqual([]);
  });

  it("upsertContextSnapshot / clearContextSnapshot 与 focusTask 行为准确", () => {
    const dispatchEvent = vi.fn();
    vi.stubGlobal("window", { dispatchEvent });
    vi.stubGlobal(
      "CustomEvent",
      class MockCustomEvent {
        type: string;
        detail: unknown;
        constructor(type: string, init?: { detail?: unknown }) {
          this.type = type;
          this.detail = init?.detail;
        }
      },
    );

    const session = useSessionStore();
    const agent = useAgentStore();
    session.activeSessionId = "s1";

    const snap: SessionContextSnapshot = {
      sessionId: "s1",
      loopCount: 1,
      model: "claude-sonnet",
      intent: "edit",
      apiFormat: "anthropic",
      createdAt: 1000,
      totalChars: 1000,
      estimatedTokens: 300,
      cacheHistory: [],
      maxOutputTokens: 8192,
      messageCount: 2,
      toolSchemaCount: 10,
      toolCallCount: 0,
      toolResultCount: 0,
      imageCount: 0,
      sections: [],
    };

    agent.upsertContextSnapshot(snap);
    expect(agent.currentContextSnapshot?.model).toBe("claude-sonnet");

    agent.clearContextSnapshot("s1");
    expect(agent.currentContextSnapshot).toBeNull();

    agent.focusTask(42);
    expect(agent.focusedTaskId).toBe(42);
    expect(dispatchEvent).toHaveBeenCalledTimes(1);
    expect(dispatchEvent.mock.calls[0][0]).toMatchObject({
      type: "subagent-task-focus",
      detail: { taskId: 42 },
    });
  });
});
