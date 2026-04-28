import { defineStore } from "pinia";
import { ref, computed } from "vue";
import type {
  AgentRun,
  AgentRunEvent,
  SubAgentRun,
  SubAgentEvent,
  TodoItem,
} from "../types";
import { useSessionStore } from "./session";

export const useAgentStore = defineStore("agent", () => {
  const agentRuns = ref<Record<string, AgentRun>>({});
  const agentRunEventsByRun = ref<Record<string, AgentRunEvent[]>>({});
  const subAgentRuns = ref<Record<string, SubAgentRun>>({});
  const subAgentEventsByRun = ref<Record<string, SubAgentEvent[]>>({});
  const todos = ref<TodoItem[]>([]);
  const focusedTaskId = ref<number | null>(null);
  const showAgentPanel = ref(false);

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
      .sort((a, b) => a.startedAt - b.startedAt);
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

  function getSubAgentEvents(runId: string): SubAgentEvent[] {
    return subAgentEventsByRun.value[runId] ?? [];
  }

  function getAgentRunEvents(runId: string): AgentRunEvent[] {
    return agentRunEventsByRun.value[runId] ?? [];
  }

  function upsertAgentRun(run: AgentRun) {
    if (!run?.runId) return;
    agentRuns.value = {
      ...agentRuns.value,
      [run.runId]: run,
    };
  }

  function focusTask(taskId: number | null | undefined) {
    if (taskId === null || taskId === undefined) return;
    focusedTaskId.value = taskId;
    window.dispatchEvent(new CustomEvent("subagent-task-focus", { detail: { taskId } }));
  }

  return {
    todos,
    agentSteps,
    agentRuns,
    agentRunEventsByRun,
    subAgentRuns,
    subAgentEventsByRun,
    focusedTaskId,
    showAgentPanel,
    currentSubAgentRuns,
    activeSubAgentRuns,
    currentAgentRuns,
    interruptedAgentRuns,
    getSubAgentEvents,
    getAgentRunEvents,
    upsertAgentRun,
    focusTask,
  };
});
