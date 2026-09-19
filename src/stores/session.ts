import { defineStore } from "pinia";
import { ref, computed } from "vue";
import type { AgentCurrentTurn, AgentApprovalMode, AgentUserMode, SessionListFilter } from "../types";
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
  throttling: boolean;
  lastRenderAt: number;
  /** 懒加载：当前已加载最老消息的 seq（游标），null = 未分页/已全量 */
  pagedOldestSeq: number | null;
  /** 懒加载：是否还有更早的历史可加载 */
  pagedHasMore: boolean;
  /** 懒加载：上滑加载进行中（防重入） */
  pagedLoadingOlder: boolean;
  sessionInputTokens: number;
  sessionOutputTokens: number;
  sessionCacheHitTokens: number;
  sessionCacheMissTokens: number;
}

/**
 * 会话用量累计的入参。
 *
 * 用对象而不是位置参数：这四个字段语义不同却都是 number，
 * `setSessionUsageTotals(id, out, in)` 这种顺序错位编译器拦不住。
 */
export interface SessionUsageTotals {
  input?: number;
  output?: number;
  /** 缓存命中 / 未命中。缺失按 0 处理，0 在展示层等价于"未报告" */
  cacheHit?: number;
  cacheMiss?: number;
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
    throttling: false,
    lastRenderAt: 0,
    pagedOldestSeq: null,
    pagedHasMore: false,
    pagedLoadingOlder: false,
    sessionInputTokens: 0,
    sessionOutputTokens: 0,
    sessionCacheHitTokens: 0,
    sessionCacheMissTokens: 0,
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
  const pendingProjectId = ref<string | null>(null);
  /// 新建会话界面选过的模式/档位：纯内存"待应用选择"——
  /// 发送首条消息创建会话时应用到新会话（chat.ts ensureActiveSessionForSend 消费）。
  /// 刻意不写 UI 偏好设置："设置-常规设置"里的默认只由设置页改，
  /// 选择器不再隐式改写它（否则新建会话里点一下模式就把全局默认改掉了）。
  const pendingWorkMode = ref<AgentUserMode | null>(null);
  const pendingApprovalMode = ref<AgentApprovalMode | null>(null);
  /// 新建会话界面拨过的"深度思考"开关：与上面两个 pending 同构。
  ///
  /// 与它们的唯一差别：深度思考的**初值**由后端解析（设置默认 + 模型能力），
  /// 前端不参与推导；这里存的仅是"用户在会话还没创建时拨的那一下"。
  /// `null` = 用户没拨过，与 `false`（明确拨到关）语义不同。
  const pendingThinkingEnabled = ref<boolean | null>(null);
  /// 新建会话界面拨过的"只读保护"开关：与上面两个 pending 同构，但注意**它没有设置默认值**。
  ///
  /// 只读保护是全系统唯一"纯内存态"的会话闸门（既无设置态也无 session 表列），
  /// 后端 `set_agent_read_only` 刻意不落盘（见 command/permission.rs 的 doc：
  /// 记住会变成"新会话莫名写不了文件"的幽灵故障）。所以这里的 pending 不是
  /// "从设置取默认值"，而仅仅是"用户在会话还没创建时拨的那一下别丢"——
  /// 会话一建立就把它转交给后端内存态，此后与 pending 无关。
  ///
  /// `null` = 用户没拨过（不是"关闭"），与 `false`（明确拨到关）语义不同。
  const pendingReadOnly = ref<boolean | null>(null);
  const workingDirectory = ref<string | null>(null);
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
    if (view.status !== 'RUNNING') {
      view.contentBuffer = "";
      view.tempBuffer = "";
      view.toolBuffer = "";
      view.thinkingBuffer = "";
      view.streamActive = false;
      resetAgentCurrentTurn(view);
    }
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
    // 如果会话仍在运行，保留 currentTurn 避免和 checkpoint 快照产生重叠分裂
    if (view.status !== 'RUNNING') {
      view.contentBuffer = "";
      view.tempBuffer = "";
      view.toolBuffer = "";
      view.thinkingBuffer = "";
      view.streamActive = false;
      resetAgentCurrentTurn(view);
    }
  }

  function appendSessionMessage(sessionId: string | null | undefined, message: any) {
    const view = getSessionView(sessionId);
    view.messages.push(message);
  }

  /** 懒加载：把更早的历史消息插入到 messages **头部**（不触碰尾部与游标状态） */
  function prependSessionMessages(sessionId: string | null | undefined, messages: any[]) {
    if (!messages.length) return;
    const view = getSessionView(sessionId);
    const mapped = messages.map((msg) => ({
      ...msg,
      content: msg.content ?? msg.userContent,
      text: msg.text ?? (msg.role === 'user' ? msg.userContent?.replace(/<[^>]*>/g, '') : undefined),
    }));
    view.messages.unshift(...mapped);
  }

  /** 懒加载游标状态更新（由加载函数在响应后调用） */
  function setSessionPaging(
    sessionId: string | null | undefined,
    oldestSeq: number | null,
    hasMore: boolean,
  ) {
    const view = getSessionView(sessionId);
    view.pagedOldestSeq = oldestSeq;
    view.pagedHasMore = hasMore;
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

  function setSessionUsageTotals(sessionId: string | null | undefined, usage: SessionUsageTotals) {
    const view = getSessionView(sessionId);
    view.sessionInputTokens = usage.input || 0;
    view.sessionOutputTokens = usage.output || 0;
    view.sessionCacheHitTokens = usage.cacheHit || 0;
    view.sessionCacheMissTokens = usage.cacheMiss || 0;
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

  const totalInputTokens = computed(() => currentSessionView.value?.sessionInputTokens || 0);
  const totalOutputTokens = computed(() => currentSessionView.value?.sessionOutputTokens || 0);
  // 会话累计缓存命中 / 未命中：两者都为 0 表示该会话从未有请求上报过缓存字段
  const totalCacheHitTokens = computed(() => currentSessionView.value?.sessionCacheHitTokens || 0);
  const totalCacheMissTokens = computed(() => currentSessionView.value?.sessionCacheMissTokens || 0);

  return {
    sessionViews,
    activeSessionId,
    pendingProjectId,
    pendingWorkMode,
    pendingApprovalMode,
    pendingThinkingEnabled,
    pendingReadOnly,
    workingDirectory,
    totalInputTokens,
    totalOutputTokens,
    totalCacheHitTokens,
    totalCacheMissTokens,
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
    prependSessionMessages,
    setSessionPaging,
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
