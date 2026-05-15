import { defineStore } from "pinia";
import { ref, computed } from "vue";
import type { AgentCurrentTurn, SessionListFilter } from "../types";
import { createEmptyAgentCurrentTurn, resetAgentCurrentTurn } from "../utils/agentTurnState";

interface LatestCheckpoint {
  id: string;
  hasOperations: boolean;
  hasPatches: boolean;
  canRollback: boolean;
}

export interface SessionViewState {
  status: string;
  messages: any[];
  jarvisResponse: string;
  toolBuffer: string;
  contentBuffer: string;
  tempBuffer: string;
  thinkingBuffer: string;
  lastUserMessage: string;
  showRecallEdit: boolean;
  latestCheckpoint: LatestCheckpoint | null;
  agentSteps: any[];
  currentTurnStepsStart: number;
  hydrated: boolean;
  runStartTime: number | null;
  streamActive: boolean;
  cancelHandled: boolean;
  activeRunId: string | null;
  resumableRunId: string | null;
  currentTurn: AgentCurrentTurn;
}

const READY_TEXT = "Ready for input...";
const DEFAULT_SESSION_KEY = "__default__";

function createEmptySessionView(initialHistory = READY_TEXT, hydrated = false): SessionViewState {
  return {
    status: "IDLE",
    messages: [],
    jarvisResponse: initialHistory,
    toolBuffer: "",
    contentBuffer: "",
    tempBuffer: "",
    thinkingBuffer: "",
    lastUserMessage: "",
    showRecallEdit: false,
    latestCheckpoint: null,
    agentSteps: [],
    currentTurnStepsStart: 0,
    hydrated,
    runStartTime: null,
    streamActive: false,
    cancelHandled: false,
    activeRunId: null,
    resumableRunId: null,
    currentTurn: createEmptyAgentCurrentTurn(),
  };
}

function getSessionKey(sessionId: string | null | undefined) {
  return sessionId ?? DEFAULT_SESSION_KEY;
}

export const useSessionStore = defineStore("session", () => {
  const sessionViews = ref<Record<string, SessionViewState>>({
    [DEFAULT_SESSION_KEY]: createEmptySessionView(READY_TEXT, true),
  });
  const activeSessionId = ref<string | null>(null);
  const pendingWorkingDirectory = ref<string | null>(null);
  const workingDirectory = ref<string | null>(null);
  const totalInputTokens = ref(0);
  const totalOutputTokens = ref(0);
  const sessionListFilter = ref<SessionListFilter>({});

  function setSessionListFilter(filter: SessionListFilter) {
    sessionListFilter.value = { ...filter };
  }

  function clearSessionListFilter() {
    sessionListFilter.value = {};
  }

  function getSessionListFilterPayload(): SessionListFilter | null {
    const filter = sessionListFilter.value;
    const payload: SessionListFilter = {};
    if (filter.keyword?.trim()) payload.keyword = filter.keyword.trim();
    if (filter.fromTs) payload.fromTs = filter.fromTs;
    if (filter.toTs) payload.toTs = filter.toTs;
    if (filter.profileId?.trim()) payload.profileId = filter.profileId.trim();
    if (filter.model?.trim()) payload.model = filter.model.trim();
    if (filter.tool?.trim()) payload.tool = filter.tool.trim();
    if (filter.hasToolCalls !== undefined && filter.hasToolCalls !== null) {
      payload.hasToolCalls = filter.hasToolCalls;
    }
    return Object.keys(payload).length ? payload : null;
  }

  function getSessionView(sessionId: string | null | undefined, initialHistory = READY_TEXT) {
    const key = getSessionKey(sessionId);
    if (!sessionViews.value[key]) {
      sessionViews.value[key] = createEmptySessionView(initialHistory, false);
    }
    return sessionViews.value[key];
  }

  function hasHydratedSessionView(sessionId: string | null | undefined) {
    const key = getSessionKey(sessionId);
    return Boolean(sessionViews.value[key]?.hydrated);
  }

  function resetSessionView(sessionId: string | null | undefined, initialHistory = READY_TEXT) {
    const key = getSessionKey(sessionId);
    sessionViews.value[key] = createEmptySessionView(initialHistory, true);
  }

  function deleteSessionView(sessionId: string | null | undefined) {
    const key = getSessionKey(sessionId);
    delete sessionViews.value[key];
  }

  function clearSessionBuffers(sessionId: string | null | undefined) {
    const view = getSessionView(sessionId);
    view.toolBuffer = "";
    view.contentBuffer = "";
    view.tempBuffer = "";
    view.thinkingBuffer = "";
    view.streamActive = false;
    resetAgentCurrentTurn(view);
  }

  function replaceSessionHistory(sessionId: string | null | undefined, history: string) {
    const view = getSessionView(sessionId);
    view.jarvisResponse = history && history.trim() ? history : READY_TEXT;
    view.messages = [];
    view.hydrated = true;
    // 清除 live turn 缓冲区，避免与历史 HTML 重复渲染最后一条 agent 回复
    view.contentBuffer = "";
    view.tempBuffer = "";
    view.toolBuffer = "";
    view.thinkingBuffer = "";
    view.streamActive = false;
    resetAgentCurrentTurn(view);
  }

  function replaceSessionMessages(sessionId: string | null | undefined, messages: any[]) {
    const view = getSessionView(sessionId);
    view.messages = messages.map((msg) => ({
      ...msg,
      // 后端返回 userContent，前端 Vue 组件使用 text/images（或 content 作为兼容）
      content: msg.content ?? msg.userContent,
      text: msg.text ?? (msg.role === 'user' ? msg.userContent?.replace(/<[^>]*>/g, '') : undefined),
    }));
    view.jarvisResponse = READY_TEXT;
    view.hydrated = true;
    // 清除 live turn 缓冲区
    view.contentBuffer = "";
    view.tempBuffer = "";
    view.toolBuffer = "";
    view.thinkingBuffer = "";
    view.streamActive = false;
    resetAgentCurrentTurn(view);
  }

  function appendSessionMessage(sessionId: string | null | undefined, message: any) {
    const view = getSessionView(sessionId);
    view.messages.push(message);
  }

  function appendSessionHistory(sessionId: string | null | undefined, html: string) {
    const view = getSessionView(sessionId);
    if (view.jarvisResponse === READY_TEXT) {
      view.jarvisResponse = "";
    }
    view.jarvisResponse += html;
    view.hydrated = true;
  }

  function removeTrailingUserMessageFromView(sessionId: string | null | undefined = activeSessionId.value) {
    const view = getSessionView(sessionId);
    const lastUserIdx = view.jarvisResponse.lastIndexOf('<div class="chat-message user-message"');
    if (lastUserIdx !== -1) {
      const lastAgentIdx = view.jarvisResponse.lastIndexOf('<div class="chat-message agent-message"');
      if (lastAgentIdx < lastUserIdx) {
        view.jarvisResponse = view.jarvisResponse.substring(0, lastUserIdx);
      }
    }
  }

  function setSessionUsageTotals(inputTokens: number, outputTokens: number) {
    totalInputTokens.value = inputTokens || 0;
    totalOutputTokens.value = outputTokens || 0;
  }

  function isSessionRunning(sessionId: string): boolean {
    return sessionViews.value[sessionId]?.status === "RUNNING";
  }

  const currentSessionView = computed(() => getSessionView(activeSessionId.value));

  const currentSessionStatus = computed(() => currentSessionView.value.status);

  const isCurrentSessionRunning = computed(() => currentSessionView.value.status === "RUNNING");

  const isAnySessionRunning = computed(() =>
    Object.values(sessionViews.value).some((v) => v.status === "RUNNING")
  );

  const runningSessionId = ref<string | null>(null);

  return {
    sessionViews,
    activeSessionId,
    runningSessionId,
    pendingWorkingDirectory,
    workingDirectory,
    totalInputTokens,
    totalOutputTokens,
    sessionListFilter,
    setSessionListFilter,
    clearSessionListFilter,
    getSessionListFilterPayload,
    getSessionView,
    hasHydratedSessionView,
    resetSessionView,
    deleteSessionView,
    clearSessionBuffers,
    replaceSessionHistory,
    replaceSessionMessages,
    appendSessionMessage,
    appendSessionHistory,
    removeTrailingUserMessageFromView,
    setSessionUsageTotals,
    isSessionRunning,
    currentSessionView,
    currentSessionStatus,
    isCurrentSessionRunning,
    isAnySessionRunning,
    READY_TEXT,
    getSessionKey,
  };
});
