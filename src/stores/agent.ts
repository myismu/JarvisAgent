/**
 * # agent.ts — Agent 前端状态与会话级轨迹索引
 *
 * 保存 Agent 运行态、循环事件以及按需加载的子 Agent 持久化轨迹索引。
 *
 * ## Key Exports
 * - `useAgentStore()`: 提供 Agent 运行态与子 Agent 轨迹状态
 *
 * ## Dependencies
 * - Internal: `../types`, `./session`
 * - External: `pinia`, `vue`
 *
 * ## Constraints
 * - 子 Agent 详细轨迹按需加载，列表只保存轻量索引
 */
import { defineStore } from "pinia";
import { ref, computed } from "vue";
import type {
  AgentRun,
  AgentRunLoopEvent,
  SessionContextSnapshot,
  SubAgentRun,
  SubAgentTraceEvent,
  SubAgentTraceRunSummary,
  TodoItem,
} from "../types";
import { useSessionStore } from "./session";

export const useAgentStore = defineStore("agent", () => {
  const agentRuns = ref<Record<string, AgentRun>>({});
  const agentRunEventsByRun = ref<Record<string, AgentRunLoopEvent[]>>({});
  const subAgentRuns = ref<Record<string, SubAgentRun>>({});
  const subAgentTraceByRun = ref<Record<string, SubAgentTraceEvent[]>>({});
  const subAgentTraceRunsBySession = ref<Record<string, SubAgentTraceRunSummary[]>>({});
  const pinnedSubAgentTraceRunId = ref<string | null>(null);
  const contextSnapshots = ref<Record<string, SessionContextSnapshot>>({});
  const todosBySession = ref<Record<string, TodoItem[]>>({});
  const focusedTaskId = ref<number | null>(null);
  const showAgentPanel = ref(false);

  const currentTodos = computed(() => {
    const session = useSessionStore();
    const sessionId = session.activeSessionId;
    if (!sessionId) return [];
    return todosBySession.value[sessionId] ?? [];
  });

  const agentSteps = computed(() => {
    const session = useSessionStore();
    return session.currentSessionView.agentSteps;
  });

  const currentSubAgentRuns = computed(() => {
    const session = useSessionStore();
    const sessionId = session.activeSessionId;
    if (!sessionId) return [];
    return Object.values(subAgentRuns.value)
      .filter((run) => run.sessionId === sessionId)
      .sort((a, b) => {
        const aTask = a.taskId ?? Number.MAX_SAFE_INTEGER;
        const bTask = b.taskId ?? Number.MAX_SAFE_INTEGER;
        if (aTask !== bTask) return aTask - bTask;
        return a.startedAt - b.startedAt;
      });
  });

  const currentSubAgentTraceRuns = computed(() => {
    const session = useSessionStore();
    const sessionId = session.activeSessionId;
    if (!sessionId) return [];
    return subAgentTraceRunsBySession.value[sessionId] ?? [];
  });

  const activeSubAgentRuns = computed(() => {
    return currentSubAgentRuns.value.filter((run) => run.status === "running");
  });

  const currentAgentRuns = computed(() => {
    const session = useSessionStore();
    const sessionId = session.activeSessionId;
    if (!sessionId) return [];
    return Object.values(agentRuns.value)
      .filter((run) => run.sessionId === sessionId)
      .sort((a, b) => b.startedAt - a.startedAt);
  });

  const interruptedAgentRuns = computed(() => {
    return currentAgentRuns.value.filter(
      (run) => run.status === "interrupted" && run.resumable
    );
  });

  const currentContextSnapshot = computed(() => {
    const session = useSessionStore();
    const sessionId = session.activeSessionId;
    if (!sessionId) return null;
    return contextSnapshots.value[sessionId] ?? null;
  });

  function getSubAgentTrace(runId: string): SubAgentTraceEvent[] { return subAgentTraceByRun.value[runId] ?? []; }

  function clearSubAgentTrace(runId: string) {
    if (pinnedSubAgentTraceRunId.value === runId) return;
    if (!Object.prototype.hasOwnProperty.call(subAgentTraceByRun.value, runId)) return;
    const next = { ...subAgentTraceByRun.value };
    delete next[runId];
    subAgentTraceByRun.value = next;
  }

  function pinSubAgentTrace(runId: string | null) {
    pinnedSubAgentTraceRunId.value = runId;
  }

  function getAgentRunEvents(runId: string): AgentRunLoopEvent[] {
    return agentRunEventsByRun.value[runId] ?? [];
  }

  function upsertAgentRun(run: AgentRun) {
    if (!run?.runId) return;
    agentRuns.value = {
      ...agentRuns.value,
      [run.runId]: run,
    };
  }

  function upsertContextSnapshot(snapshot: SessionContextSnapshot | null | undefined) {
    if (!snapshot?.sessionId) return;
    contextSnapshots.value = {
      ...contextSnapshots.value,
      [snapshot.sessionId]: snapshot,
    };
  }

  function clearContextSnapshot(sessionId: string | null | undefined) {
    if (!sessionId) return;
    const next = { ...contextSnapshots.value };
    delete next[sessionId];
    contextSnapshots.value = next;
  }

  function focusTask(taskId: number | null | undefined) {
    if (taskId === null || taskId === undefined) return;
    focusedTaskId.value = taskId;
    window.dispatchEvent(new CustomEvent("subagent-task-focus", { detail: { taskId } }));
  }

  return {
    todosBySession,
    currentTodos,
    agentSteps,
    agentRuns,
    agentRunEventsByRun,
    subAgentRuns,
    subAgentTraceByRun,
    subAgentTraceRunsBySession,
    pinnedSubAgentTraceRunId,
    contextSnapshots,
    focusedTaskId,
    showAgentPanel,
    currentSubAgentRuns,
    currentSubAgentTraceRuns,
    activeSubAgentRuns,
    currentAgentRuns,
    interruptedAgentRuns,
    currentContextSnapshot,
    getSubAgentTrace,
    clearSubAgentTrace,
    pinSubAgentTrace,
    getAgentRunEvents,
    upsertAgentRun,
    upsertContextSnapshot,
    clearContextSnapshot,
    focusTask,
  };
});
