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

/**
 * 会话视图的**生命周期状态**（取代原先的布尔 `hydrated`）。
 *
 * - `idle`   ：还没向该会话要过消息（新建/清空视图后的初始态）
 * - `loading`：首屏请求在途，界面应显示加载态而不是"空会话"
 * - `ready`  ：窗口内容与后端一致，可直接渲染
 * - `stale`  ：**曾经载入过**，但已知与后端不同步（例如新增消息/事件缺片），
 *              旧内容仍可先渲染，随后需要重拉
 *
 * 用状态机而不是布尔的原因是布尔把"没加载过"和"加载失败/脏了"压成同一个 false，
 * 调用方无法区分"该不该发请求"与"能不能先渲染旧内容"。
 */
export type SessionHydration = 'idle' | 'loading' | 'ready' | 'stale';

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
  /** 视图生命周期状态；含义与合法转移见 `SessionHydration` 与各 `markSession*` 方法 */
  hydration: SessionHydration;
  /**
   * 窗口最新端消息的 seq（后端分页响应 `newestSeq`）；null = 未知 / 空会话。
   *
   * 只在首屏/重载（`loadSessionMessagesReset`）时写入：那份响应取的是最新一页，
   * 它的 `newestSeq` 就是**后端最新端**。向上翻页取的是更早的页，尾部 seq
   * 必然小于它，不能拿来回写，否则「本地尾部是否已含后端最新消息」会假阴性。
   */
  loadedNewestSeq: number | null;
  /** 置为 `stale` 的原因（审计用）；回到 `ready` 时清空 */
  staleReason: string | null;
  runStartTime: number | null;
  streamActive: boolean;
  cancelHandled: boolean;
  /**
   * 已向该会话发出过停止请求、但本轮还没有真正收尾。
   *
   * 与 `status === 'RUNNING'` 配合使用：输入框的停止按钮据此显示 spinner 并禁用，
   * 从点击一直持续到会话离开 RUNNING 为止。不能只用一个「等待 IPC 返回」的局部变量
   * —— `cancel_jarvis` 命令只是置令牌就返回，毫秒级结束，spinner 一闪而过，
   * 用户会以为没生效而反复点击。
   */
  stopping: boolean;
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

function createEmptySessionView(hydration: SessionHydration = 'idle'): SessionViewState {
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
    hydration,
    loadedNewestSeq: null,
    staleReason: null,
    runStartTime: null,
    streamActive: false,
    cancelHandled: false,
    stopping: false,
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
    // 无会话态（欢迎页）没有"从后端载入"这回事，直接给 ready，避免界面把它当空会话
    [DEFAULT_SESSION_KEY]: createEmptySessionView('ready'),
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

  /// 欢迎页快捷指令 → 输入框的待填草稿：WelcomeScreen 与 TerminalInput 是兄弟组件
  /// （分别在 ChatArea / App.vue 下），用 store 做这个一次性交接，TerminalInput
  /// 消费后立即清空（不持久化，不写历史）。
  const inputDraft = ref<string | null>(null);

  function getSessionView(sessionId: string | null | undefined) {
    const key = getSessionKey(sessionId);
    if (!sessionViews.value[key]) {
      sessionViews.value[key] = createEmptySessionView();
    }
    return sessionViews.value[key];
  }

  /**
   * 该会话视图是否**曾经载入过**消息（`ready` / `stale` 都算）。
   *
   * 判据是"载入过"而不是"当前是干净的"：调用方用它决定要不要再发一次首屏请求，
   * `stale` 表示内容旧了但已经渲染过，不该因为"不是 ready"就退回空白首屏。
   */
  function hasLoadedSessionView(sessionId: string | null | undefined) {
    const hydration = sessionViews.value[getSessionKey(sessionId)]?.hydration;
    return hydration === 'ready' || hydration === 'stale';
  }

  /**
   * 旧名封装：仅作为过渡期薄封装保留，避免遗漏调用点（新代码请用 `hasLoadedSessionView`）。
   *
   * @deprecated 语义已从"hydrated 布尔"改为"是否载入过"，用新名表达真实含义。
   */
  function hasHydratedSessionView(sessionId: string | null | undefined) {
    return hasLoadedSessionView(sessionId);
  }

  /**
   * 首屏请求发出时调用：`idle` / `stale` → `loading`。
   *
   * 非法转移防护：已经是 `ready` 时**不**回退到 `loading`。刷新（checkpoint 收尾、
   * config 变更）走的是"后台重拉"，旧窗口仍然可用；若置成 loading，界面会先闪空白，
   * 并行加载的守卫也会误判成"还没载入过"而重复发请求。
   */
  function markSessionLoading(sessionId: string | null | undefined) {
    const view = getSessionView(sessionId);
    if (view.hydration === 'ready') return;
    view.hydration = 'loading';
  }

  /** 数据真正写入视图（整批替换/事件补齐）后调用：任何状态 → `ready`，并清空脏原因。 */
  function markSessionReady(sessionId: string | null | undefined) {
    const view = getSessionView(sessionId);
    view.hydration = 'ready';
    view.staleReason = null;
  }

  /**
   * 标记窗口与后端不同步：`ready` → `stale`，并记下原因（审计/排查用）。
   *
   * 非法转移防护：只有 `ready` 才能变脏 —— `idle`/`loading` 本来就没有"已有的内容"，
   * 强行置 stale 会让 `hasLoadedSessionView` 误判成"曾经载入过"，跳过首屏请求。
   */
  function markSessionStale(sessionId: string | null | undefined, reason: string) {
    const view = getSessionView(sessionId);
    if (view.hydration !== 'ready') return;
    view.hydration = 'stale';
    view.staleReason = reason;
  }

  function resetSessionView(sessionId: string | null | undefined) {
    const key = getSessionKey(sessionId);
    // 新建会话没有历史：直接 ready（同「无会话态」口径），首屏无需请求
    sessionViews.value[key] = createEmptySessionView('ready');
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
    // 整批替换完成 = 窗口就绪；上一次的脏原因到此失效
    view.hydration = 'ready';
    view.staleReason = null;
    // 分页游标一并复位（2026-11-22）：本函数是「整批替换」，新首屏从尾部开始，
    // 旧游标指向的位置已不在 `messages` 里。不复位的话，上滑翻页会按旧游标再取一次，
    // 表现为重复内容或直接跳到错误区间。
    // `loadSessionMessagesReset` 紧随其后会 `setSessionPaging` 覆盖成真实值，
    // 因此这里只给一个安全默认值（无更早历史）；其他调用方若不设游标，也不会误拉取。
    view.pagedOldestSeq = null;
    view.pagedHasMore = false;
    // 窗口**最新端** seq 同样由调用方随后补充（本页响应当前不返回 newestSeq），
    // 这里给 null = 未知，调用方拿不到就保持未知，不要瞎猜成 0。
    view.loadedNewestSeq = null;
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
    newestSeq: number | null = null,
  ) {
    const view = getSessionView(sessionId);
    // 窗口最老端（上滑游标）
    view.pagedOldestSeq = oldestSeq;
    view.pagedHasMore = hasMore;
    // 窗口最新端（用于判定"本地尾部是否已含后端最新消息"）；响应暂不提供时保持 null
    if (newestSeq !== null) view.loadedNewestSeq = newestSeq;
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
    inputDraft,
    totalInputTokens,
    totalOutputTokens,
    totalCacheHitTokens,
    totalCacheMissTokens,
    getSessionView,
    hasLoadedSessionView,
    /** @deprecated 过渡期旧名，等价于 `hasLoadedSessionView` */
    hasHydratedSessionView,
    markSessionLoading,
    markSessionReady,
    markSessionStale,
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
