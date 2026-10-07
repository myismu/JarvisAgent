import { defineStore } from "pinia";
import { ref, computed } from "vue";
import type { AgentCurrentTurn, AgentApprovalMode, AgentUserMode } from "../types";
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
  /**
   * 会话消息**加载失败**标记：结构化接口报错且没有兜底通道可用时置起，
   * 由界面显示一句可翻译的提示（`chat.loadFailed`）。
   *
   * 此前这里有一条 HTML 兜底（`get_session_history` → `jarvisResponse`），
   * 已随整条 HTML 通道删除 —— 兜底渲染的是另一套已不被渲染的 HTML，
   * 出问题时既不是同一套样式，文案还是写死的中文。
   */
  loadError: boolean;
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
  /**
   * 本次请求的首个流式片段到达时刻（毫秒）。null = 当前没有进行中的吞吐采样。
   *
   * 起点取「首个片段」而不是 loop 开始：后端在**每次请求**拿到 usage 就推一次
   * （不是回合收尾才推），工具执行发生在那之后 —— 从首个片段起算天然排除了
   * 首字延迟与工具耗时，得到的才是这一跳的生成速度，而不是"本轮平均"。
   */
  requestFirstDeltaAt: number | null;
  /** 上一次 usage 推送时的会话累计输出 token：与本次相减 = 「本次请求」的输出量 */
  requestOutputTokensBase: number;
  /** 最近一次请求的输出速度（token/秒）。null = 尚无有效采样 */
  lastRequestTokPerSec: number | null;
  /**
   * 本轮 agent 已经跑到第几步 —— **步 = 一次 agent loop 迭代**（think → act → observe）。
   *
   * 为什么纯聊天也显示「第 1 步」：一条用户消息必然至少跑一个 loop，所以步数天然 ≥ 1，
   * 不会出现"只有轮、没有步"的割裂。
   *
   * 本字段只是**内存覆盖层**，三个来源各覆盖一段窗口（组件里取最大）：
   * - `currentTurn.loop`：运行中由事件驱动，最精确；但回合结束会被 `resetAgentCurrentTurn()` 清掉
   * - 本字段：由 `chat-turn-start` 写入，覆盖"回合刚结束、还没重新加载"那段
   * - `agent_run_events` 的行数：**持久化的唯一事实源**（「每轮一行」，UNIQUE(run_id, loop_index)），
   *   覆盖"重启 App / 切走再切回"
   *
   * 所以"步"**不需要新增持久化** —— 它已经在 events 里了。
   *
   * ⚠️ 别读 `agent_runs.loop_count`：那是**从未被维护的死字段，恒为 0**
   * （后端 `agent_runs.rs::prepare_resume` 的注释写明了这件事）。
   */
  loopStepCount: number;
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

const DEFAULT_SESSION_KEY = "__default__";

function createEmptySessionView(hydrated = false): SessionViewState {
  return {
    status: "IDLE",
    messages: [],
    loadError: false,
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
    requestFirstDeltaAt: null,
    requestOutputTokensBase: 0,
    lastRequestTokPerSec: null,
    loopStepCount: 0,
  };
}

function getSessionKey(sessionId: string | null | undefined) {
  return sessionId ?? DEFAULT_SESSION_KEY;
}

export const useSessionStore = defineStore("session", () => {
  const sessionViews = ref<Record<string, SessionViewState>>({
    [DEFAULT_SESSION_KEY]: createEmptySessionView(true),
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

  function getSessionView(sessionId: string | null | undefined) {
    const key = getSessionKey(sessionId);
    if (!sessionViews.value[key]) {
      sessionViews.value[key] = createEmptySessionView(false);
    }
    return sessionViews.value[key];
  }

  function hasHydratedSessionView(sessionId: string | null | undefined) {
    const key = getSessionKey(sessionId);
    return Boolean(sessionViews.value[key]?.hydrated);
  }

  function resetSessionView(sessionId: string | null | undefined) {
    const key = getSessionKey(sessionId);
    sessionViews.value[key] = createEmptySessionView(true);
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

  /**
   * 标记/清除"会话消息加载失败"。
   *
   * 取代了原先的 `replaceSessionHistory(html)` —— 那条路把后端拼好的 HTML
   * 塞进一个**已不再被渲染**的字段，出错时用户看到的既不是同一套样式、
   * 文案还是写死的中文。现在只置一个布尔位，由界面显示可翻译的提示。
   */
  function setSessionLoadError(sessionId: string | null | undefined, failed: boolean) {
    getSessionView(sessionId).loadError = failed;
  }

  function replaceSessionMessages(sessionId: string | null | undefined, messages: any[]) {
    const view = getSessionView(sessionId);
    view.messages = messages.map((msg) => ({
      ...msg,
      // 后端返回 userContent，前端 Vue 组件使用 text/images（或 content 作为兼容）
      content: msg.content ?? msg.userContent,
      text: msg.text ?? (msg.role === 'user' ? msg.userContent?.replace(/<[^>]*>/g, '') : undefined),
    }));
    view.loadError = false;
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

  function setSessionUsageTotals(sessionId: string | null | undefined, usage: SessionUsageTotals) {
    const view = getSessionView(sessionId);
    view.sessionInputTokens = usage.input || 0;
    view.sessionOutputTokens = usage.output || 0;
    view.sessionCacheHitTokens = usage.cacheHit || 0;
    view.sessionCacheMissTokens = usage.cacheMiss || 0;
  }

  /**
   * 记下「本次请求的首个流式片段到达时刻」。
   *
   * 只在还没有起点时写入 —— 一个请求会推来成百上千个片段，只有第一个是起点；
   * 后续片段若覆盖它，算出来的速度会随片段密度虚高。
   */
  function markRequestFirstDelta(sessionId: string | null | undefined, at: number) {
    const view = getSessionView(sessionId);
    if (view.requestFirstDeltaAt === null) view.requestFirstDeltaAt = at;
  }

  /**
   * 一次请求结束（usage 推送到达）时结算**本次请求**的吞吐。
   *
   * 口径是「最近一次请求」而不是会话累计 —— 累计值在长会话里会被历史稀释，
   * 读数就不再反映"现在快不快"。
   *
   *   Δ输出 = 本次推送的累计输出 − 上次推送时的累计输出
   *   Δ时间 = 本次推送时刻 − 本次请求首个片段时刻
   *
   * 输出增量 ≤ 0（厂商未上报、或该请求没有输出）时不写入：**保留上一次的可读值**，
   * 比清成 null 更有用 —— 界面上那一格宁可显示一个稍旧的真值，也不要闪成空。
   */
  function settleRequestThroughput(
    sessionId: string | null | undefined,
    outputTotal: number,
    at: number,
  ) {
    const view = getSessionView(sessionId);
    const delta = outputTotal - view.requestOutputTokensBase;
    const startedAt = view.requestFirstDeltaAt;
    // 基线无条件推进：漏掉一次结算会让下一次的增量把两次请求算在一起
    view.requestOutputTokensBase = outputTotal;
    view.requestFirstDeltaAt = null;
    if (delta <= 0 || startedAt === null) return;
    const seconds = (at - startedAt) / 1000;
    if (seconds <= 0) return;
    view.lastRequestTokPerSec = delta / seconds;
  }

  /**
   * 记下本轮 agent 跑到第几步（`chat-turn-start` 携带的 loop 序号）。
   *
   * 后端**每进入一个 loop 就发一次**这个事件，值是**本轮内**的 loop 序号（从 1 起），
   * 所以这里直接覆盖：新一轮天然把上一轮的步数清掉，不需要额外的重置逻辑。
   *
   * 只写内存 —— 跨重启的持久化由 `agent_run_events` 承担（见 `loopStepCount` 注释），
   * 不新增任何存储。
   */
  function noteLoopStep(sessionId: string | null | undefined, step: number) {
    const view = getSessionView(sessionId);
    view.loopStepCount = step;
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
    getSessionView,
    hasHydratedSessionView,
    resetSessionView,
    deleteSessionView,
    clearSessionBuffers,
    setSessionLoadError,
    replaceSessionMessages,
    appendSessionMessage,
    prependSessionMessages,
    setSessionPaging,
    setSessionUsageTotals,
    markRequestFirstDelta,
    settleRequestThroughput,
    noteLoopStep,
    isSessionRunning,
    currentSessionView,
    currentSessionStatus,
    isCurrentSessionRunning,
    isAnySessionRunning,
    getSessionKey,
  };
});
