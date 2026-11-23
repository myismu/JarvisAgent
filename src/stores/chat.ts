import { defineStore } from "pinia";
import { ref, computed } from "vue";
import { invoke } from "@tauri-apps/api/core";
import type { JarvisResult } from "../types";
import { useSessionStore } from "./session";
import { useAgentStore } from "./agent";
import { usePermissionStore } from "./permission";
import { usePreferences } from "../composables/usePreferences";
import { buildAgentTurnSnapshot } from "../utils/agentTurnState";
import {
  stripPseudoToolCalls,
} from "../utils/agentTurnRender";

/** 从 Tauri 序列化的 Rust 枚举错误中提取人类可读消息 */
function extractErrorMessage(err: unknown): string {
  if (typeof err === "string") {
    const parsed = tryParseApiErrorBody(err);
    if (parsed) return parsed;
    return err;
  }
  if (err instanceof Error) return err.message;
  if (typeof err !== "object" || err === null) return String(err);
  const obj = err as Record<string, unknown>;
  const keys = Object.keys(obj);
  for (const key of keys) {
    const val = obj[key];
    if (key === "ExecutionBusy" || key === "ExecutionStopping") {
      return typeof val === "string" ? val : JSON.stringify(val);
    }
    if (typeof val === "string") {
      const parsed = tryParseApiErrorBody(val);
      return parsed || `${key}: ${val}`;
    }
    if (typeof val === "object" && val !== null) {
      const inner = val as Record<string, unknown>;
      if (typeof inner.body === "string" && inner.body.length > 0) {
        const parsed = tryParseApiErrorBody(inner.body);
        return parsed || (inner.body as string);
      }
      if (typeof inner.last_error === "string") return inner.last_error as string;
      const nested = extractErrorMessage(val);
      if (nested && nested !== "[object Object]") return nested;
    }
    return `${key}: ${JSON.stringify(val)}`;
  }
  return JSON.stringify(err);
}

/** 判断本次发送是否在后端登记执行前被会话占用闸门拒绝。 */
function isExecutionAdmissionError(err: unknown): boolean {
  if (typeof err !== "object" || err === null) return false;
  const obj = err as Record<string, unknown>;
  if ("ExecutionBusy" in obj || "ExecutionStopping" in obj) return true;
  return Object.values(obj).some(isExecutionAdmissionError);
}

interface SendToJarvisOutcome {
  /** 只有后端尚未接收本轮时才返回；调用方应恢复输入，不把它当成已保存消息撤回。 */
  retryDraft?: { sessionId: string | null; text: string };
}

function tryParseApiErrorBody(body: string): string | null {
  try {
    const json = JSON.parse(body);
    if (!json || typeof json !== "object") return null;
    const error = json.error || json;
    if (typeof error !== "object" || error === null) return null;
    const message = (error as any).message || "";
    const type = (error as any).type || "";
    const code = (error as any).code || "";

    if (type === "insufficient_balance" || /balance|quota|计费|余额|欠费/i.test(message)) {
      return `账户余额不足，请前往 API 平台充值后重试。${code ? ` (HTTP ${code})` : ""}`;
    }
    if (/rate.?limit|频率|限流|too many requests/i.test(message)) {
      return `API 请求频率过高，请稍后重试。${code ? ` (HTTP ${code})` : ""}`;
    }
    if (/auth|unauthorized|key|token|权限|鉴权/i.test(message)) {
      return `API Key 无效或已过期，请在设置中检查密钥配置。${code ? ` (HTTP ${code})` : ""}`;
    }
    if (message) return message;
    return null;
  } catch {
    return null;
  }
}

// 这里原有 INTERRUPT_MARKER_LINE_RE + splitInterruptedNotice（"从正文剥离中断标记"），
// 2026-09-21 删除。它与 `AgentTurn.vue` 的 splitInterruptMarker 是同一份约定，
// 两处一起删。理由（别再把它加回来）：
//   1. 阶段二改造后，中断标记不再拼进正文（kind 走 session_messages.interrupt_kind），
//      状态标注由后端 `notice` 字段结构化下发 —— 剥离对新数据本就永不命中；
//   2. 那条正则过宽："任意含 ⚠ / ✕ 的行"都算标记，而模型正文里写 ⚠ 是常事
//      （比如表格的"注意"列）；且命中后是**截断式**处理（`slice(0, match.index)`，
//      只保留匹配点之前的内容），于是一处误判会吃掉整条回复的后续正文。
//      真实事故：一份交接单只剩前一节，剩下八成内容没渲染出来。
// 旧会话若正文里仍带旧标记，按既定决策不做兼容（旧会话自行删除）。

function buildFinalResponseParts(
  view: { contentBuffer: string; tempBuffer: string; toolBuffer: string; thinkingBuffer: string },
  fallbackContent?: string,
) {
  const streamedContent = stripPseudoToolCalls(`${view.contentBuffer}${view.tempBuffer}`);
  const fallback = stripPseudoToolCalls(fallbackContent || "");
  const finalContent = streamedContent.trim() ? streamedContent : fallback;
  const liveThinking = view.thinkingBuffer.trim();
  let finalToolBuffer = view.toolBuffer;

  if (liveThinking && liveThinking !== finalContent.trim()) {
    finalToolBuffer = finalToolBuffer ? `${liveThinking}\n\n${finalToolBuffer}` : liveThinking;
  }

  return { finalContent, finalToolBuffer };
}

/** agent 完成后拉取最新上下文快照，使监控面板显示完整的最后一轮回复 */
async function refreshContextSnapshot(sessionId: string) {
  try {
    const snapshot = await invoke<any>('get_session_context_snapshot', { sessionId });
    if (snapshot) {
      useAgentStore().upsertContextSnapshot(snapshot);
    }
  } catch { /* 快照拉取不影响主流程 */ }
}

/**
 * 取本轮刚结束那次 run 的时长（毫秒）。
 *
 * 为什么要回查 `agentRuns`，而不是在前端记一个起点：
 * 1. 口径必须与刷新后一致 —— 刷新的历史快照由后端下发，前端不可能再把它对齐到一个本地计数器；
 * 2. 本轮的「开始」发生在后端（含等待授权/退避这类不应计入的时长），只有后端时间戳是准的。
 *
 * 匹配方式是本轮用户消息的 `messageId`（后端在 `start_run` 时就把它写在 run 上），
 * 同一条用户消息可能有多个 run（续跑），取 `startedAt` 最新的那个。
 *
 * 扣掉 `waitMs`（非 Agent 责任时长，已由后端落库在 run 上）后夹到 0 以上：
 * 时间戳来自墙上时钟，理论上不会为负，但跨进程/系统校时后不保证。
 *
 * @returns 毫秒数；拿不到完成态 run（未收到收尾事件、续跑无用户消息等）时返回 null。
 */
function resolveRunDurationMs(sessionId: string, messageId?: string | null): number | null {
  if (!messageId) return null;
  const runs = Object.values(useAgentStore().agentRuns).filter(
    (run) => run.sessionId === sessionId && run.messageId === messageId && run.finishedAt,
  );
  if (!runs.length) return null;
  // 续跑会基于同一条用户消息另起一个 run：最新开始的才是本轮的真实收尾
  const run = runs.reduce((latest, item) =>
    item.startedAt > latest.startedAt ? item : latest,
  );
  // `waitMs` 由后端落库在 run 上（AgentRun.waitMs），为 NULL 表示这一轮没有等待，按 0 计。
  const waitMs = Math.max(0, run.waitMs ?? 0);
  const elapsed = (run.finishedAt ?? run.startedAt) - run.startedAt;
  return Math.max(0, elapsed - waitMs);
}

/**
 * `get_session_messages_paged` 的返回类型（与后端 `PagedSessionMessages` 字段一一对应）。
 *
 * - `oldestSeq`：本页最老消息的 seq，作为上滑翻页游标；
 * - `newestSeq`：本页最新消息的 seq；首屏那一页即**后端最新端**，写进 `loadedNewestSeq`
 *   （空会话为 null）；
 * - `totalCount`：该会话可见消息总条数（口径是消息数，不是轮数）。
 */
interface PagedSessionMessagesResponse {
  messages: any[];
  hasMore: boolean;
  oldestSeq: number | null;
  newestSeq: number | null;
  totalCount: number;
}

export const useChatStore = defineStore("chat", () => {
  let scrollToBottomCb: ((force?: boolean) => void) | null = null;
  const sendGeneration: Record<string, number> = {};

  const rollbackRecalledMessage = ref("");
  const memoryNotice = ref<string | null>(null);

  const toolBuffer = computed({
    get: () => {
      const session = useSessionStore();
      return session.currentSessionView.toolBuffer;
    },
    set: (value: string) => {
      const session = useSessionStore();
      const view = session.getSessionView(session.activeSessionId);
      view.toolBuffer = value;
      session.markSessionReady(session.activeSessionId);
    },
  });

  const contentBuffer = computed({
    get: () => {
      const session = useSessionStore();
      return session.currentSessionView.contentBuffer;
    },
    set: (value: string) => {
      const session = useSessionStore();
      const view = session.getSessionView(session.activeSessionId);
      view.contentBuffer = value;
      session.markSessionReady(session.activeSessionId);
    },
  });

  const tempBuffer = computed({
    get: () => {
      const session = useSessionStore();
      return session.currentSessionView.tempBuffer;
    },
    set: (value: string) => {
      const session = useSessionStore();
      const view = session.getSessionView(session.activeSessionId);
      view.tempBuffer = value;
      session.markSessionReady(session.activeSessionId);
    },
  });

  const thinkingBuffer = computed({
    get: () => {
      const session = useSessionStore();
      return session.currentSessionView.thinkingBuffer;
    },
    set: (value: string) => {
      const session = useSessionStore();
      const view = session.getSessionView(session.activeSessionId);
      view.thinkingBuffer = value;
      session.markSessionReady(session.activeSessionId);
    },
  });

  const lastUserMessage = computed({
    get: () => {
      const session = useSessionStore();
      return session.currentSessionView.lastUserMessage;
    },
    set: (value: string) => {
      const session = useSessionStore();
      const view = session.getSessionView(session.activeSessionId);
      view.lastUserMessage = value;
      session.markSessionReady(session.activeSessionId);
    },
  });

  const showRecallEdit = computed({
    get: () => {
      const session = useSessionStore();
      return session.currentSessionView.showRecallEdit;
    },
    set: (value: boolean) => {
      const session = useSessionStore();
      const view = session.getSessionView(session.activeSessionId);
      view.showRecallEdit = value;
      session.markSessionReady(session.activeSessionId);
    },
  });

  const latestCheckpoint = computed({
    get: () => {
      const session = useSessionStore();
      return session.currentSessionView.latestCheckpoint;
    },
    set: (value: any) => {
      const session = useSessionStore();
      const view = session.getSessionView(session.activeSessionId);
      view.latestCheckpoint = value;
      session.markSessionReady(session.activeSessionId);
    },
  });

  const messages = computed({
    get: () => {
      const session = useSessionStore();
      return session.currentSessionView.messages;
    },
    set: (value: any[]) => {
      const session = useSessionStore();
      const view = session.getSessionView(session.activeSessionId);
      view.messages = value;
      session.markSessionReady(session.activeSessionId);
    },
  });

  function resetRenderState(sessionId?: string | null) {
    const session = useSessionStore();
    const view = session.getSessionView(sessionId ?? session.activeSessionId);
    view.throttling = false;
    view.lastRenderAt = 0;
  }

  function registerScrollCb(cb: (force?: boolean) => void) {
    scrollToBottomCb = cb;
  }

  function forceScrollToBottom() {
    scrollToBottomCb?.(true);
  }

  function followScrollToBottom() {
    scrollToBottomCb?.(false);
  }

  function triggerRender() {
    const session = useSessionStore();
    const view = session.getSessionView(session.activeSessionId);
    if (view.throttling) return;
    const now = performance.now();
    const elapsed = now - view.lastRenderAt;
    const MIN_INTERVAL = 33;
    if (elapsed < MIN_INTERVAL) {
      view.throttling = true;
      setTimeout(() => {
        const view2 = session.getSessionView(session.activeSessionId);
        view2.lastRenderAt = performance.now();
        renderTick.value++;
        view2.throttling = false;
      }, MIN_INTERVAL - elapsed);
    } else {
      view.throttling = true;
      view.lastRenderAt = now;
      requestAnimationFrame(() => {
        renderTick.value++;
        view.throttling = false;
      });
    }
  }

  /**
   * 本轮 agent 消息的 id。
   *
   * 必须与 ChatArea 给 live 轮次用的 vnode key 一致（`view.liveTurnId`）：
   * 收尾时正式消息沿用同一个 id，Vue 才会 patch 复用同一个组件实例；
   * 换成 `agent_${Date.now()}` 会让整棵子树卸载重建（Markdown 重新解析，闪一下）。
   * liveTurnId 为空时退回时间戳，只为兜底不产出 undefined id。
   */
  function agentMessageId(view: { liveTurnId: string | null }) {
    return view.liveTurnId ?? `agent_${Date.now()}`;
  }

  // 渲染 tick，用于触发流式滚动
  const renderTick = ref(0);

  /**
   * 提交权限决策。
   * @param decision allow | allow_session | reject（allow_session=「本项目允许」：登记会话键并落盘 data/permissions/，跨会话记住）
   * @param feedback 仅 reject 时使用：用户的拒绝说明，会回灌给模型，让它别换个写法重试
   */
  async function resolvePermission(decision: string, feedback?: string) {
    const perm = usePermissionStore();
    const session = useSessionStore();
    const request = perm.permissionRequest;
    if (request) {
      const reqId = request.id;
      const sid = request.sessionId ?? session.activeSessionId;
      const trimmed = feedback?.trim() ?? "";
      const result = await invoke<{ needsResume?: boolean; resumeWith?: string }>("resolve_permission", {
        id: reqId,
        sessionId: sid,
        decision,
        content: trimmed ? trimmed : null,
      });
      if (sid) perm.removePermission(sid, reqId);
      // 循环上限超时后续跑：用 resume_jarvis 续跑，不注入新的用户消息
      if (result?.needsResume && result?.resumeWith && sid) {
        const session = useSessionStore();
        const requestView = session.getSessionView(sid);
        requestView.status = "RUNNING";
        requestView.streamActive = false;
        // 续跑是新一轮：重新生成本轮 live 标识，收尾消息与它保持同一个 vnode key。
        requestView.liveTurnId = `live_${crypto.randomUUID()}`;
        const res = await invoke<JarvisResult>("resume_jarvis", {
          sessionId: sid,
          reason: result.resumeWith,
        });
        if (res.status !== "PAUSED_LOOP_LIMIT") {
          const resumeContent = stripPseudoToolCalls(res.content);
          const snapshot = buildAgentTurnSnapshot(
            requestView.currentTurn,
            resumeContent,
            "",
            res.status,
            res.notice_i18n_key ?? undefined,
            res.notice_detail ?? undefined,
          );
          session.appendSessionMessage(sid, { role: "agent", id: agentMessageId(requestView), snapshot });
          session.clearSessionBuffers(sid);
          resetRenderState(sid);
        }
      }
    }
  }

  async function resolvePlan(decision: string, modifiedContent?: string, planDoc?: { id: string; sessionId?: string }) {
    const perm = usePermissionStore();
    const session = useSessionStore();
    const planId = perm.planProposal?.id || planDoc?.id;
    const sid = perm.planProposal?.sessionId || planDoc?.sessionId || session.activeSessionId;
    if (!planId || !sid) return null;

    const result = await invoke<{ needsResume?: boolean; resumeMessage?: string }>("resolve_permission", {
      id: planId,
      sessionId: sid,
      decision,
      content: modifiedContent ?? null,
    });
    if (sid && perm.planProposals[sid]?.id === planId) {
      delete perm.planProposals[sid];
    }
    return result;
  }

  async function continueFromApprovedPlan(title: string, _content: string) {
    const fullMsg = [
      `用户已同意方案「${title}」。严格按以下步骤执行，禁止自己直接执行任务：`,
      `1. 用 ExecuteTool 执行 SwitchWorkMode(mode="edit", reason="方案已审批通过，切回编辑模式执行")`,
      `2. 用 ExecuteTool 执行 CreateTask，批量创建方案中所有任务（传 tasks 数组，含 depends_on 依赖关系）`,
      `3. 用 ExecuteTool 执行 RunSubagentsSequentially，启动调度器由子 Agent 执行任务`,
      `4. 调度完成后读取报告，向用户汇报结果`,
    ].join('\n');
    return sendToJarvis(
      fullMsg,
      undefined,
      undefined,
      false,
      true, // skipRunningCheck — 方案审批续跑是新的用户轮次，不应取消前一轮
      `已同意方案「${title}」`, // 简短展示消息
    );
  }

  async function requestPlanRevision(title: string, feedback: string) {
    return sendToJarvis(
      `用户要求修改方案「${title}」。修改意见：${feedback}\n\n请根据以上意见重新提交一份可审批方案，不要直接执行。`,
      undefined,
      undefined,
      false,
      true, // skipRunningCheck
    );
  }

  async function resumeFromPlan(resumeMessage: string) {
    return sendToJarvis(resumeMessage);
  }

  /**
   * 把"会话界面当前的模式/档位/只读选择"（pending）转交给指定会话。
   *
   * 只负责**无会话态**的 pending：已有会话的拨动由 TerminalInput 立即写后端，
   * 不经过这里；只有"会话尚未创建"时拨下的选择才等到此刻转交。
   * 
   * 拨了不发消息就切走仍算没表态，切入会话时会被 syncPermissionFromSession 清空。
   *
   * 两条路径共用：① 新建会话（先创建再转交）② 已有会话（直接转交）。
   * 应用失败不阻断发送，只记日志。
   *
   * ⚠️ 三者的存储性质不同，别看成一回事：
   * - 工作模式 / 权限档位 / 深度思考：后端 `set_session_*` 会**落库**到 sessions 表，跨重启保留；
   * - 只读保护：后端 `set_agent_read_only` **只写进程内存**，重启即归零——
   *   这里"转交"的不是持久化，而是让后端内存态与新会话对上。
   */
  async function flushPendingSessionPrefs(sessionId: string) {
    const session = useSessionStore();
    if (session.pendingWorkMode) {
      try {
        await invoke("set_session_work_mode", { sessionId, mode: session.pendingWorkMode });
      } catch (e) {
        console.error('Failed to apply pending work mode:', e);
      }
      session.pendingWorkMode = null;
    }
    if (session.pendingApprovalMode) {
      try {
        await invoke("set_session_approval_mode", { sessionId, mode: session.pendingApprovalMode });
      } catch (e) {
        console.error('Failed to apply pending approval mode:', e);
      }
      session.pendingApprovalMode = null;
    }
    // 深度思考：判据同样是 `!== null`——`false`（明确拨到关）也是一次有效表态。
    // 注意它**只有布尔**：`follow_global` 的解析在首条消息时由后端完成并固化，
    // 前端从不解析"跟随全局"，也不读设置默认值。
    if (session.pendingThinkingEnabled !== null) {
      try {
        await invoke("set_session_thinking_enabled", {
          id: sessionId,
          enabled: session.pendingThinkingEnabled,
        });
      } catch (e) {
        console.error('Failed to apply pending thinking mode:', e);
      }
      session.pendingThinkingEnabled = null;
    }
    // 只读保护：判据是 `!== null` 而不是真值判断——`false`（明确拨到关）
    // 也是一次有效表态，必须转交；只有 `null`（用户没拨过）才跳过。
    if (session.pendingReadOnly !== null) {
      try {
        await invoke("set_agent_read_only", { sessionId, enabled: session.pendingReadOnly });
      } catch (e) {
        console.error('Failed to apply pending read-only flag:', e);
      }
      session.pendingReadOnly = null;
    }
  }

  async function ensureActiveSessionForSend() {
    const session = useSessionStore();
    if (session.activeSessionId) {
      // 已有会话：把界面上拨过但还没落库的选择补写进 session（发送消息 = 表态）。
      // 必须有这一步——否则在已有会话里切模式/档位将永远写不进 session 表。
      await flushPendingSessionPrefs(session.activeSessionId);
      return session.activeSessionId;
    }

    const meta = await invoke<any>("create_session", {
      projectId: session.pendingProjectId,
    });

    // 新建会话界面选过的模式/档位（pending）：应用到刚创建的会话——
    // 必须在赋值 activeSessionId 之前完成，syncPermissionFromSession 的 watch
    // 才能拉到应用后的值。刻意不写 UI 偏好设置："设置-常规设置"里的默认只由设置页改，
    // 新建会话里的选择只属于这一个会话。
    await flushPendingSessionPrefs(meta.id);

    session.activeSessionId = meta.id;
    session.workingDirectory = meta.workingDirectory || null;
    session.pendingProjectId = null;
    session.resetSessionView(meta.id);
    session.setSessionUsageTotals(meta.id, {
      input: meta.totalInputTokens || 0,
      output: meta.totalOutputTokens || 0,
      cacheHit: meta.totalCacheHitTokens || 0,
      cacheMiss: meta.totalCacheMissTokens || 0,
    });

    // 记录新建会话使用的模型，后续切回来时自动恢复
    const config = await invoke<any>("get_config");
    if (config.activeProfileId) {
      await invoke("update_session_profile", { id: meta.id, profileId: config.activeProfileId });
    }
    return meta.id as string;
  }

  /**
   * 首屏/重载的**并发去重表**：sessionId → 在途 Promise。
   *
   * 同一个会话只允许有一个首屏请求在飞：三个入口
   * （`ensureSessionViewLoaded` / `reloadSessionView` / 迁移期的 `loadSessionMessagesReset`）
   * 最终都汇到同一个实现，后来者拿到的是**同一个 Promise** —— 既不会重复发 IPC，
   * 也不会出现「后到的响应覆盖先到的响应」把界面刷成半截。
   *
   * 验证方式：devtools 里对同一 sessionId 连续两次调用
   * `await chat.ensureSessionViewLoaded(id, 'manual')`（不 await 第一次直接并发发起），
   * 控制台应只出现一条 `session-view load start`，第二条是 `deduped`；
   * 若两次都落在 idle 也会命中同一条在途任务。
   */
  const inFlightSessionLoads = new Map<string, Promise<void>>();

  /**
   * ⚠️ **仅供本 store 内部与迁移期使用，新调用方必须走三入口**
   * （`ensureSessionViewLoaded` / `markSessionViewStale` / `reloadSessionView`）。
   * 直接调用会绕过入口处的「该不该拉」裁决与状态机转移。
   *
   * 首屏/重载的**唯一实现**：拉取**最新 5 个完整轮**替换当前界面，并记录分页游标。
   * 注意后端 limit 语义 = 每页**轮数**（一条可渲染 user + 其后所有消息 = 一轮），
   * 长任务单轮几十条消息时页大小自适应，不会出现半轮。
   *
   * @param reason 审计用原因（只进日志，不参与逻辑）。调用方应写明「谁在什么场景下触发」。
   */
  async function loadSessionMessagesReset(sessionId: string, reason = "unspecified"): Promise<void> {
    const session = useSessionStore();
    const view = session.getSessionView(sessionId);

    // 并发去重：同一会话已有请求在途 → 复用同一个 Promise，不再发第二次 IPC
    const existing = inFlightSessionLoads.get(sessionId);
    if (existing) {
      console.debug(
        `[chat] session-view load deduped: session=${sessionId} reason=${reason} (in-flight, hydration=${view.hydration})`,
      );
      return existing;
    }

    // 首屏请求在途：idle/stale → loading（已是 ready 时不动，后台重拉不闪空白）
    const hydrationBefore = view.hydration;
    session.markSessionLoading(sessionId);
    const hydrationAfter = session.getSessionView(sessionId).hydration;
    // 每次进入加载都留一条带状态转移的调试日志，便于审计
    console.debug(
      `[chat] session-view load start: session=${sessionId} reason=${reason} hydration=${hydrationBefore}->${hydrationAfter}`,
    );

    const task = (async () => {
      try {
        const res = await invoke<PagedSessionMessagesResponse>(
          "get_session_messages_paged",
          { sessionId, beforeSeq: null, limit: 5 },
        );
        // 整批替换内部会置 ready；调用方紧接着用真实游标覆盖分页边界
        session.replaceSessionMessages(sessionId, res.messages);
        // 分页边界：最老端 = 后端下发的 oldestSeq（上滑游标）；
        // 最新端 = 后端下发的 newestSeq（首屏即会话最新端，空会话为 null）
        session.setSessionPaging(sessionId, res.oldestSeq, res.hasMore, res.newestSeq);
        console.debug(
          `[chat] session-view load done: session=${sessionId} reason=${reason} messages=${res.messages.length} oldestSeq=${res.oldestSeq} newestSeq=${res.newestSeq} totalCount=${res.totalCount} hasMore=${res.hasMore} hydration=ready`,
        );
      } catch (err) {
        // 失败：置 loadError，并把 hydration 回落 idle，让下一次 ensure 能重试
        const failedView = session.getSessionView(sessionId);
        failedView.loadError = true;
        failedView.hydration = "idle";
        failedView.staleReason = null;
        console.error(
          `[chat] session-view load failed: session=${sessionId} reason=${reason} hydration->idle`,
          err,
        );
        throw err;
      }
    })();

    inFlightSessionLoads.set(sessionId, task);
    try {
      await task;
    } finally {
      // 只删自己那一条：万一已被新请求替换，不能顺手把后来者的在途记录删掉
      if (inFlightSessionLoads.get(sessionId) === task) {
        inFlightSessionLoads.delete(sessionId);
      }
    }
  }

  /**
   * 入口一：**确保**会话视图已装载（幂等，日常唯一该用的加载入口）。
   *
   * 状态机：
   * - `idle`    → 发起首屏加载；
   * - `loading` → 复用同一 in-flight Promise（不起第二个请求）；
   * - `ready`   → 直接返回，不发 IPC；
   * - `stale`   → 重载（窗口内容旧了，需要与后端对齐）。
   *
   * 无会话态（`__default__` 欢迎页）没有后端首屏可拉，传空 sessionId 直接返回。
   *
   * @param reason 审计用原因，例如 `switch-session`（点击切会话）/ `app-start`（启动恢复）/
   *   `session-fallback`（事件驱动的会话回落）
   */
  async function ensureSessionViewLoaded(sessionId: string, reason: string): Promise<void> {
    if (!sessionId) return;
    const session = useSessionStore();
    const view = session.getSessionView(sessionId);

    switch (view.hydration) {
      case "ready":
        console.debug(
          `[chat] ensureSessionViewLoaded: session=${sessionId} reason=${reason} hydration=ready (no-op)`,
        );
        return;
      case "loading": {
        const pending = inFlightSessionLoads.get(sessionId);
        console.debug(
          `[chat] ensureSessionViewLoaded: session=${sessionId} reason=${reason} hydration=loading (reuse in-flight=${!!pending})`,
        );
        // 正常情况下 loading 必然伴随一条在途任务；没有则说明状态是脏的（历史失败路径遗留），
        // 直接补一次加载来自愈，而不是卡在 loading 永远不出内容。
        if (pending) return pending;
        return loadSessionMessagesReset(sessionId, `ensure(loading-recover):${reason}`);
      }
      case "stale":
        console.debug(
          `[chat] ensureSessionViewLoaded: session=${sessionId} reason=${reason} hydration=stale (reload, staleReason=${view.staleReason ?? "unknown"})`,
        );
        return loadSessionMessagesReset(sessionId, `ensure(stale):${reason}`);
      default:
        // 'idle'：还没向该会话要过消息 / 上次加载失败回落
        console.debug(
          `[chat] ensureSessionViewLoaded: session=${sessionId} reason=${reason} hydration=idle (load)`,
        );
        return loadSessionMessagesReset(sessionId, `ensure(idle):${reason}`);
    }
  }

  /**
   * 入口二：把会话视图**标脏**（只改状态与 staleReason，**不发 IPC**）。
   *
   * 用于「知道数据变了、但此刻不必立刻重拉」的场景（例如事件到达但会话不在前台）：
   * 标脏后由随后的 `ensureSessionViewLoaded` 或下一次进入会话完成重拉。
   * 合法的 `ready` → `stale` 转移由 session store 保证：`idle`/`loading` 时置脏会被忽略
   * （它们本来就没有「已有内容」，强行置脏会让 `hasLoadedSessionView` 误判成载入过）。
   */
  function markSessionViewStale(sessionId: string, reason: string): void {
    const session = useSessionStore();
    const hydrationBefore = session.getSessionView(sessionId).hydration;
    session.markSessionStale(sessionId, reason);
    const hydrationAfter = session.getSessionView(sessionId).hydration;
    console.debug(
      `[chat] markSessionViewStale: session=${sessionId} reason=${reason} hydration=${hydrationBefore}->${hydrationAfter}`,
    );
  }

  /**
   * 入口三：**强制重载**会话视图（无视当前状态，除非已有同会话请求在途而改为复用）。
   *
   * 只在「四类数据确实被后端改写了」的场景调用（会话消息、分页边界、运行状态相关事件、
   * 压缩/回滚等会重排历史的功能）；日常进入会话请用 `ensureSessionViewLoaded`，
   * 否则每次挂载都白拉一次首屏。
   *
   * `ready` 态下走「后台重拉」：`markSessionLoading` 不会把 ready 降成 loading，
   * 旧窗口继续渲染（不闪空白），响应到达后整批替换。
   */
  async function reloadSessionView(sessionId: string, reason: string): Promise<void> {
    if (!sessionId) return;
    console.debug(`[chat] reloadSessionView: session=${sessionId} reason=${reason} (forced)`);
    return loadSessionMessagesReset(sessionId, `force:${reason}`);
  }

  /**
   * 懒加载：向上翻页——以当前最老已加载 seq 为游标取更早 5 个完整轮，**头部插入**。
   * 返回是否真的加载了内容（调用方据此做滚动锚定）。
   * pagedLoadingOlder 防重入；hasMore=false 时不请求。
   */
  async function loadSessionMessagesOlder(sessionId: string): Promise<boolean> {
    const session = useSessionStore();
    const view = session.getSessionView(sessionId);
    if (!view.pagedHasMore || view.pagedLoadingOlder) return false;
    view.pagedLoadingOlder = true;
    const start = Date.now();
    try {
      const res = await invoke<PagedSessionMessagesResponse>(
        "get_session_messages_paged",
        { sessionId, beforeSeq: view.pagedOldestSeq, limit: 5 },
      );
      if (!res.messages.length) {
        session.setSessionPaging(sessionId, null, false);
        return false;
      }
      // 最小展示 300ms：本地 DB 查询毫秒级完成，spinner 一闪而过没有
      // "正在加载"的感知；等够展示时间后再插入内容，插入与锚定一次完成
      const elapsed = Date.now() - start;
      if (elapsed < 300) {
        await new Promise((resolve) => setTimeout(resolve, 300 - elapsed));
      }
      session.prependSessionMessages(sessionId, res.messages);
      session.setSessionPaging(sessionId, res.oldestSeq, res.hasMore);
      return true;
    } finally {
      view.pagedLoadingOlder = false;
    }
  }

  /**
   * @param thinkingOverride 单轮思考档位覆盖：
   *   - `null` / `undefined`（**正常路径**）：后端按会话档位（`sessions.thinking_mode`）
   *     + 预设默认 + 模型能力自行裁决，前端不参与决策；
   *   - `true` / `false`：仅供程序化调用或测试强制指定单轮值。
   */
  async function sendToJarvis(msg: string, thinkingOverride?: boolean | null, imageBase64List?: string[], resumeOnly = false, skipRunningCheck = false, uiDisplayMsg?: string): Promise<SendToJarvisOutcome | undefined> {
    const session = useSessionStore();

    if (!msg && (!imageBase64List || imageBase64List.length === 0)) return;

    if (!skipRunningCheck) {
      const currentView = session.getSessionView(session.activeSessionId);
      if (currentView.status === 'RUNNING' || currentView.stopping) {
        // 停止命令直到上一轮完整收尾才返回；收尾失败时不继续并发发送。
        const stopped = await cancelJarvis(session.activeSessionId ?? undefined);
        if (!stopped) {
          return { retryDraft: { sessionId: session.activeSessionId, text: msg } };
        }
      }
    }

    let sessionIdAtStart: string;
    try {
      sessionIdAtStart = await ensureActiveSessionForSend();
    } catch (err) {
      // 会话创建失败（典型：挂载项目目录已被删除/改名）。此时没有真实会话可挂，
      // 错误快照挂到 __default__ 视图：欢迎页让位，错误卡片直接显示在对话区。
      console.error("[chat] 创建会话失败:", err);
      const errMsg = extractErrorMessage(err) || "创建会话失败";
      const view = session.getSessionView(null);
      session.appendSessionMessage(null, {
        role: "agent",
        id: agentMessageId(view),
        snapshot: buildAgentTurnSnapshot(
          view.currentTurn,
          `**执行出错：${errMsg}**`,
          "",
          "ERROR",
        ),
      });
      view.status = "ERROR";
      triggerRender();
      scrollToBottomCb?.();
      return { retryDraft: { sessionId: session.activeSessionId, text: msg } };
    }
    sendGeneration[sessionIdAtStart] = (sendGeneration[sessionIdAtStart] || 0) + 1;
    const myGeneration = sendGeneration[sessionIdAtStart];
    const requestView = session.getSessionView(sessionIdAtStart);
    const pendingUserMessageId = resumeOnly ? null : `pending_${crypto.randomUUID()}`;
    // 本轮 live 渲染的稳定 id：请求开始时生成一次，收尾时正式消息沿用同一个值。
    // ChatArea 用它作 vnode key，保证收尾时是 patch 复用而不是卸载重建
    // （见 SessionViewState.liveTurnId 的注释）。
    requestView.liveTurnId = `live_${crypto.randomUUID()}`;

    try {
      const recovered = await invoke<boolean>("recover_interrupted_session_messages", {
        sessionId: sessionIdAtStart,
      });
      if (recovered) {
        // 恢复落库的消息在尾部，必然包含在首屏窗口内：走入口三强制重载（首屏 + 分页游标重置）
        await reloadSessionView(sessionIdAtStart, 'interrupt-recovery');
        session.clearSessionBuffers(sessionIdAtStart);
        resetRenderState(sessionIdAtStart);
      }
    } catch (err) {
      console.warn("恢复中断消息失败:", err);
    }

    requestView.latestCheckpoint = null;
    requestView.showRecallEdit = false;
    requestView.currentTurnStepsStart = requestView.agentSteps.length;
    session.markSessionReady(sessionIdAtStart);
    requestView.status = "RUNNING";
    requestView.activeRunId = null;
    requestView.resumableRunId = null;
    requestView.runStartTime = Date.now();
    requestView.streamActive = false;
    requestView.cancelHandled = false;
    // 新一轮开始：上一次的「停止中」标记到此失效
    requestView.stopping = false;
    session.clearSessionBuffers(sessionIdAtStart);
    resetRenderState(sessionIdAtStart);

    if (!resumeOnly) {
      // 只往结构化消息数组里追加 —— 界面渲染的是 `messages`。
      // 这里此前还**顺带拼一份 HTML** 塞进 `jarvisResponse`（图片内联 + 长消息折叠 +
      // 内联 onclick 里的"展开全部/收起"中文），而那个字段早已不被任何组件渲染，
      // 纯属每发一条消息白拼一次。HTML 通道已整体删除。
      session.appendSessionMessage(sessionIdAtStart, {
        role: "user",
        id: pendingUserMessageId!,
        text: (uiDisplayMsg ?? msg) || "",
        images: imageBase64List && imageBase64List.length > 0 ? [...imageBase64List] : null,
      });
    }

    requestView.lastUserMessage = resumeOnly ? "" : msg;
    if (sessionIdAtStart === session.activeSessionId) {
      triggerRender();
      scrollToBottomCb?.(true);
    }

    try {
      const res = resumeOnly
        ? await invoke<JarvisResult>("resume_jarvis", {
            sessionId: sessionIdAtStart,
            reason: msg,
          })
        : await invoke<JarvisResult>("ask_jarvis", {
            sessionId: sessionIdAtStart,
            msg,
            thinkingOverride: thinkingOverride ?? null,
            imageBase64List: imageBase64List ?? null,
            agentDisplayMode: usePreferences().agentAudience.value,
            reflectionMode: usePreferences().reflectionMode ?? "smart",
            displayMsg: uiDisplayMsg ?? null,
          });

      // 停止旧轮后可能已经开始新轮；旧 IPC 回执不能再写入新轮的用户消息或状态。
      if (myGeneration !== sendGeneration[sessionIdAtStart]) return;

      // 用后端返回值回填前端用户消息，使撤回按钮立即可见：
      // - messageId：后端为本轮用户消息分配的 UUID（发消息时前端只有本地占位 id）
      // - 回滚模式 / 检查点 id：Git 快照在收尾时才创建，发消息那一刻还不存在，
      //   靠返回值实时补上，「会话和代码撤回」立即出现（旧行为要等刷新
      //   走 get_session_messages 重查库才同步）。字段口径与刷新路径一致。
      // 续跑（resumeOnly）不注入用户消息，无需回填。
      if (!resumeOnly) {
        const msgs = requestView.messages;
        for (let i = msgs.length - 1; i >= 0; i--) {
          if (msgs[i].id === pendingUserMessageId) {
            if (res.user_message_id) msgs[i].messageId = res.user_message_id;
            // 与刷新路径同口径：本轮有文件补丁 → both（会话和代码撤回）；
            // 快照创建失败时 id 允许为空串（刷新路径同样会下发「both 但无 id」）
            if (res.checkpoint_has_patches === true) {
              msgs[i].rollbackMode = "both";
              msgs[i].rollbackCheckpointId = res.checkpoint_id ?? "";
            }
            break;
          }
        }
      }

      const sessionSwitched = sessionIdAtStart !== session.activeSessionId;
      if (!sessionSwitched) {
        session.setSessionUsageTotals(sessionIdAtStart, {
          input: res.session_input_tokens || 0,
          output: res.session_output_tokens || 0,
          cacheHit: res.session_cache_hit_tokens || 0,
          cacheMiss: res.session_cache_miss_tokens || 0,
        });
        // 分组用量与四个扁平总量分开写：总量回写不得冲掉分组数据。
        // 仅在后端真的返回了明细时才覆盖（旧后端无该字段 → 保持原值）。
        if (res.session_usage_breakdown) {
          session.setSessionUsageBreakdown(sessionIdAtStart, res.session_usage_breakdown);
        }
      }
      requestView.lastUserMessage = resumeOnly ? "" : msg;

      if (res.status === "CANCELLED") {
        if (!requestView.cancelHandled) {
          const cancellationFallback = res.content && res.content !== "用户已取消执行。" ? res.content : "";
          // 正文原样使用：取消的状态说明由下面设置的固定 notice 承担
          const cleanedFallback = stripPseudoToolCalls(cancellationFallback);
          const { finalContent, finalToolBuffer } = buildFinalResponseParts(requestView, cleanedFallback);
          const hasPartialContent = finalContent || finalToolBuffer;
          if (hasPartialContent || cleanedFallback.trim()) {
            const canceledSnapshot = buildAgentTurnSnapshot(
              requestView.currentTurn,
              finalContent || cleanedFallback,
              finalToolBuffer,
              "CANCELLED",
              // 与后端中断收尾同一个 key：这条是"取消"路径的本地兜底快照，
              // 措辞必须与后端 `notice.userCancel` 一致，否则同一次取消
              // 在不同刷新时机下会显示两种说法
              "notice.userCancel",
            );
            session.appendSessionMessage(sessionIdAtStart, { role: "agent", id: agentMessageId(requestView), snapshot: canceledSnapshot });
          }
          requestView.latestCheckpoint = null;
          session.clearSessionBuffers(sessionIdAtStart);
          resetRenderState(sessionIdAtStart);
          requestView.lastUserMessage = resumeOnly ? "" : msg;
          requestView.showRecallEdit = !resumeOnly;
          session.markSessionReady(sessionIdAtStart);
        }
        if (myGeneration === sendGeneration[sessionIdAtStart]) {
          requestView.runStartTime = null;
          requestView.streamActive = false;
          requestView.status = "IDLE";
          requestView.activeRunId = null;
          // 取消已真正收尾：按钮的「停止中」状态到此结束。
          // 必须挂在 generation 判断里：旧请求的取消回执不能清掉新请求的停止标记。
          requestView.stopping = false;
        }
        if (myGeneration === sendGeneration[sessionIdAtStart]) {
          requestView.cancelHandled = false;
        }
        if (!sessionSwitched && myGeneration === sendGeneration[sessionIdAtStart]) {
          triggerRender();
          scrollToBottomCb?.();
        }
        return;
      }

      if (res.status === "CLARIFICATION_NEEDED") {
        const clarificationSnapshot = buildAgentTurnSnapshot(
          requestView.currentTurn,
          stripPseudoToolCalls(res.content || ""),
          "",
          res.status,
        );
        requestView.latestCheckpoint = null;
        session.clearSessionBuffers(sessionIdAtStart);
        session.appendSessionMessage(sessionIdAtStart, { role: "agent", id: agentMessageId(requestView), snapshot: clarificationSnapshot });
        resetRenderState(sessionIdAtStart);
        if (myGeneration === sendGeneration[sessionIdAtStart]) {
          requestView.streamActive = false;
          requestView.status = "IDLE";
          requestView.activeRunId = null;
        }
        if (!sessionSwitched && myGeneration === sendGeneration[sessionIdAtStart]) {
          triggerRender();
          scrollToBottomCb?.();
        }
        // steps persist removed — session_messages is the source of truth
        return;
      }

      const { finalContent: rawFinalContent, finalToolBuffer: streamedToolBuffer } =
        buildFinalResponseParts(requestView, res.content);
      // 正文原样使用；状态标注**只**取后端结构化字段（见本文件上方的删除说明）
      const finalContent = rawFinalContent;
      // 状态标注只取后端结构化字段（i18n key + 可选补充数据），文案由语言包出
      const interruptedNoticeKey = res.notice_i18n_key ?? undefined;
      const interruptedNoticeDetail = res.notice_detail ?? undefined;
      // break_loop 时后端通过 tool_execution_summary 传递工具结果，补充到 toolBuffer
      const finalToolBuffer = streamedToolBuffer || (res as any).toolExecutionSummary || "";
      const sessionInputTokens = res.session_input_tokens ?? (res as any).sessionInputTokens ?? 0;
      const sessionOutputTokens = res.session_output_tokens ?? (res as any).sessionOutputTokens ?? 0;
      // 会话累计缓存命中 / 未命中：后端从 sessions 表读回，0 表示该会话从未上报过缓存字段
      const sessionCacheHitTokens = res.session_cache_hit_tokens ?? (res as any).sessionCacheHitTokens ?? 0;
      const sessionCacheMissTokens = res.session_cache_miss_tokens ?? (res as any).sessionCacheMissTokens ?? 0;
      
      // 本轮时长：从后端落库的 run 时间戳算（见 resolveRunDurationMs）。
      // 必须在 buildAgentTurnSnapshot 之前取：写进快照后，界面就不必再靠本地时钟计时。
      const turnDurationMs = resolveRunDurationMs(sessionIdAtStart, res.user_message_id);
      // 先拍快照（保留执行过程），再清空 live 缓冲区
      const snapshot = buildAgentTurnSnapshot(
        requestView.currentTurn,
        finalContent,
        finalToolBuffer,
        res.status,
        interruptedNoticeKey,
        interruptedNoticeDetail,
        turnDurationMs,
      );
      session.clearSessionBuffers(sessionIdAtStart);

      if (myGeneration === sendGeneration[sessionIdAtStart]) {
        requestView.status = "IDLE";
        requestView.activeRunId = null;
        requestView.resumableRunId = null;
        requestView.streamActive = false;
        requestView.runStartTime = null;
        requestView.latestCheckpoint = null;
        requestView.stopping = false;
      }
      session.appendSessionMessage(sessionIdAtStart, {
        role: "agent",
        id: agentMessageId(requestView),
        snapshot,
      });

      resetRenderState(sessionIdAtStart);

      if (!sessionSwitched && myGeneration === sendGeneration[sessionIdAtStart]) {
        triggerRender();
        scrollToBottomCb?.();
      }
      // agent_steps persist removed
      const sessionAfterSave = useSessionStore();
      if (sessionIdAtStart === sessionAfterSave.activeSessionId) {
        // 回合结束的总量回写：只动四个扁平累计值。
        // 分组明细不在这里写 —— 那会让上一请求推来的明细被空值冲掉（见 setSessionUsageBreakdown）。
        sessionAfterSave.setSessionUsageTotals(sessionIdAtStart, {
          input: sessionInputTokens,
          output: sessionOutputTokens,
          cacheHit: sessionCacheHitTokens,
          cacheMiss: sessionCacheMissTokens,
        });
        // 后端若随本轮回执给了分组明细，则一并写入（整值覆盖，不累加）；没给就保持原值。
        if (res.session_usage_breakdown) {
          sessionAfterSave.setSessionUsageBreakdown(sessionIdAtStart, res.session_usage_breakdown);
        }
      }
      // agent 完成后主动拉取最新上下文快照，使监控面板的 Session Messages 包含完整最后一轮回复
      refreshContextSnapshot(sessionIdAtStart);
    } catch (err) {
      console.error("[chat] ask_jarvis 失败，原始错误:", err);
      const errMsg = extractErrorMessage(err) || "未知错误";
      console.error("[chat] 格式化后:", errMsg);
      const admissionRejected = isExecutionAdmissionError(err);

      // 占用闸门在后端写入用户消息之前拒绝请求，只移除本次前端占位消息。
      if (admissionRejected && pendingUserMessageId) {
        const pendingIndex = requestView.messages.findIndex(
          (message) => message.id === pendingUserMessageId,
        );
        if (pendingIndex >= 0) requestView.messages.splice(pendingIndex, 1);
      }

      // 已有更新一轮时，旧请求的错误回执不能清理新一轮的流数据或追加错误卡片。
      if (myGeneration !== sendGeneration[sessionIdAtStart]) return;

      session.clearSessionBuffers(sessionIdAtStart);
      resetRenderState(sessionIdAtStart);

      // 构建错误快照，追加到 chat.messages（而非遗留字段 jarvisResponse）
      const snapshot = buildAgentTurnSnapshot(
        requestView.currentTurn,
        `**执行出错：${errMsg}**`,
        "",
        "ERROR",
      );
      session.appendSessionMessage(sessionIdAtStart, {
        role: "agent",
        id: agentMessageId(requestView),
        snapshot,
      });

      if (myGeneration === sendGeneration[sessionIdAtStart]) {
        requestView.showRecallEdit = false;
        requestView.status = "ERROR";
        requestView.activeRunId = null;
        requestView.streamActive = false;
        requestView.runStartTime = null;
        requestView.stopping = false;
      }
      if (sessionIdAtStart === session.activeSessionId) {
        triggerRender();
        scrollToBottomCb?.();
      }
      if (admissionRejected && !resumeOnly) {
        return { retryDraft: { sessionId: sessionIdAtStart, text: msg } };
      }
    }
  }

  /**
   * 停止指定会话的执行。
   *
   * 目标会话的确定规则（2026-11 修复）：
   * - 传入 `preferSessionId` 就用它；缺省就是当前激活会话；
   * - **绝不**退化成「任意一个 RUNNING 会话」。旧实现缺省时按 `sessionViews`
   *   的键顺序取第一个 RUNNING（键顺序 = 视图创建顺序，即最早打开过的会话），
   *   多会话并行时停止指令会落到别的对话上——实测连点停止依次停掉了其他会话，
   *   当前会话反而最后才停。
   *
   * `view.stopping` 在这里同步置起（早于任何 await），停止按钮据此显示 spinner
   * 并保持禁用，直到本轮真正收尾（`sendToJarvis` 的状态回落路径负责清除）。
   */
  async function cancelJarvis(preferSessionId?: string): Promise<boolean> {
    const session = useSessionStore();
    const perm = usePermissionStore();
    const targetSid = preferSessionId ?? session.activeSessionId ?? undefined;
    if (!targetSid) return false;
    const view = session.sessionViews[targetSid];
    // 只允许停止正在运行的会话；已停止/未运行的视图不发送请求，
    // 避免取消指令"漂移"到无关会话。
    if (!view || (view.status !== 'RUNNING' && !view.stopping)) return false;
    view.cancelHandled = false;
    view.stopping = true;
    perm.clearPermissions(targetSid);
    delete perm.planProposals[targetSid];
    try {
      await invoke("cancel_jarvis", { sessionId: targetSid });
      return true;
    } catch (err) {
      // 命令失败（IPC 异常等）时撤回「停止中」，否则按钮会永远禁用
      view.stopping = false;
      console.error("取消执行失败:", err);
      return false;
    }
  }

  async function recallAndEdit(): Promise<string> {
    const session = useSessionStore();
    const sessionId = session.activeSessionId;
    if (!sessionId) return "";
    const view = session.getSessionView(sessionId);
    const target = [...view.messages]
      .reverse()
      .find((message) => message.role === "user" && message.messageId);
    if (!target) {
      console.error("撤回失败：当前消息尚未保存，无法定位准确的撤回目标");
      return "";
    }

    try {
      const recalledText = await invoke<string>("recall_message", {
        sessionId,
        messageId: target.messageId,
        pruneMetadataCutoff: null,
      });
      // 与后端按 messageId 截断保持一致，移除目标消息及其后的全部本地消息。
      const targetIndex = view.messages.findIndex((message) => message.id === target.id);
      if (targetIndex >= 0) view.messages.splice(targetIndex);

      // 这里此前还有一段在 `jarvisResponse`（HTML 字符串）上做 lastIndexOf 截断的代码 ——
      // 那个字段不被任何组件渲染，纯属白算；HTML 通道已整体删除。

      view.agentSteps = view.agentSteps.slice(0, view.currentTurnStepsStart);
      if (view.agentSteps.length === 0) {
        view.currentTurnStepsStart = 0;
      }
      view.showRecallEdit = false;
      view.lastUserMessage = "";
      triggerRender();

      return recalledText || "";
    } catch (err) {
      console.error("撤回失败:", err);
      return "";
    }
  }

  function dismissRecallEdit() {
    showRecallEdit.value = false;
  }

  async function cancelSubAgentRun(runId: string) {
    const agent = useAgentStore();
    try {
      const run = await invoke<import("../types").SubAgentRun>("cancel_subagent_run", { runId });
      agent.subAgentRuns = {
        ...agent.subAgentRuns,
        [run.runId]: run,
      };
    } catch (err) {
      console.error("取消子 Agent 失败:", err);
    }
  }

  async function resumeAgentRun(runId: string) {
    const session = useSessionStore();
    try {
      const plan = await invoke<{ sessionId: string; prompt: string }>(
        "prepare_resume_agent_run",
        { runId }
      );
      if (plan.sessionId !== session.activeSessionId) {
        console.warn("恢复执行的会话不是当前会话", plan.sessionId);
        return;
      }
      try {
        // ⚠️ 2026-11-22 改为分页首屏：原先用全量 `get_session_messages` +
        // `replaceSessionMessages`，会把懒加载窗口一次性替换成整个会话，
        // 且不重置分页游标。恢复执行与进入会话统一成同一口径。
        // 已属于本 store 内部调用，直接用实现层（已在上方“ensure”入口声明口径）
        await loadSessionMessagesReset(plan.sessionId, "resume-agent-run");
      } catch (err) {
        // 无 HTML 兜底（通道已删除）：置标记，界面显示可翻译的提示
        console.error("加载会话消息失败:", err);
        session.setSessionLoadError(plan.sessionId, true);
      }
      session.clearSessionBuffers(plan.sessionId);
      resetRenderState(plan.sessionId);
      triggerRender();
      await sendToJarvis(plan.prompt);
    } catch (err) {
      console.error("恢复执行失败:", err);
    }
  }

  return {
    renderTick,
    rollbackRecalledMessage,
    memoryNotice,
    toolBuffer,
    contentBuffer,
    tempBuffer,
    thinkingBuffer,
    lastUserMessage,
    showRecallEdit,
    latestCheckpoint,
    messages,
    resetRenderState,
    registerScrollCb,
    forceScrollToBottom,
    followScrollToBottom,
    triggerRender,
    resolvePermission,
    resolvePlan,
    continueFromApprovedPlan,
    requestPlanRevision,
    resumeFromPlan,
    sendToJarvis,
    cancelJarvis,
    recallAndEdit,
    dismissRecallEdit,
    cancelSubAgentRun,
    resumeAgentRun,
    /**
     * @internal 仅供本 store 内部与迁移期使用；新调用方必须走
     * `ensureSessionViewLoaded` / `markSessionViewStale` / `reloadSessionView`。
     */
    loadSessionMessagesReset,
    loadSessionMessagesOlder,
    ensureSessionViewLoaded,
    markSessionViewStale,
    reloadSessionView,
  };
});
