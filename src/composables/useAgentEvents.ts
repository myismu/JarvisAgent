/**
 * # useAgentEvents.ts — Agent 事件监听与历史恢复
 *
 * 统一注册 Tauri 事件监听器，恢复运行状态；子 Agent 的大体积逐轮轨迹按需从后端读取。
 *
 * ## Key Exports
 * - `useAgentEvents()`: 提供事件初始化与会话运行态恢复方法
 *
 * ## Dependencies
 * - Internal: `@/stores/session`, `@/stores/chat`, `@/stores/agent`, `@/stores/permission`
 * - External: `@tauri-apps/api`
 */
import { listen } from "@tauri-apps/api/event";
import type { UnlistenFn } from "@tauri-apps/api/event";
import { invoke } from "@tauri-apps/api/core";
import { useI18n } from "vue-i18n";
import { useSessionStore } from "../stores/session";
import type { SessionViewState } from "../stores/session";
import { useChatStore } from "../stores/chat";
import { useAgentStore } from "../stores/agent";
import { usePermissionStore } from "../stores/permission";
import {
  appendAgentExecutionLog,
  appendAgentText,
  appendAgentThinking,
  applyAgentStepToCurrentTurn,
  beginAgentLoop,
  finishAgentLoop,
  markAgentToolActivity,
  resetAgentCurrentTurn,
  upsertAgentToolCall,
} from "../utils/agentTurnState";
import type {
  TodoGroup,
  TodoUpdatePayload,
  PermissionRequest,
  PermissionResolvedPayload,
  PlanProposal,
  PlanDocument,
  AgentStep,
  AgentRun,
  AgentRunLoopEvent,
  SessionContextSnapshot,
  SessionUsageUpdatedPayload,
  SubAgentRun,
  SubAgentTraceEvent,
  SubAgentTraceRunSummary,
} from "../types";

interface SessionCleanupPayload {
  deletedSessionId?: string | null;
  activeSessionId?: string | null;
}

function replacePlanTextWithNotice(source: string, planContent: string, notice: string) {
  const content = planContent.trim();
  if (!content || !source.includes(content)) {
    return source;
  }
  return source.replace(content, notice);
}

/**
 * 把正文里的方案原文替换成一句"已提交到审批面板"的提示。
 *
 * `notice` 由调用方传入（成句、已翻译）：本函数在模块顶层，拿不到
 * `useI18n()` 的组件上下文；把它挪进 composable 只为一句话不值当。
 * 文案走 `plan.submittedNotice` —— 此前是硬编码中文，英文界面下会冒中文。
 */
function hideSubmittedPlanFromChat(
  view: SessionViewState,
  proposal: PlanProposal,
  notice: string,
) {
  const content = proposal.content || "";
  view.contentBuffer = replacePlanTextWithNotice(view.contentBuffer, content, notice);
  view.tempBuffer = replacePlanTextWithNotice(view.tempBuffer, content, notice);

  for (const block of view.currentTurn.textBlocks) {
    if (block.kind !== "assistant") continue;
    block.content = replacePlanTextWithNotice(block.content, content, notice);
  }
  view.currentTurn.revision += 1;
}

// 将初始化标志和 unlisten 句柄挂在 window 上，跨 Vite HMR 生命周期持久化
declare global {
  interface Window {
    __jarvisListenersInitialized?: boolean;
    __jarvisListenersInitializing?: boolean;
    __jarvisUnlisteners?: UnlistenFn[];
  }
}

export function useAgentEvents() {
  const { t } = useI18n();
  const session = useSessionStore();
  const chat = useChatStore();
  const agent = useAgentStore();
  const perm = usePermissionStore();
  const dismissedInterruptedRuns = new Set<string>();
  const subAgentTraceLoadVersion = new Map<string, number>();
  const subAgentRunsLoads = new Map<string, Promise<void>>();

  function syncActiveSessionView(sessionId: string | null | undefined, followScroll = false) {
    if (sessionId === session.activeSessionId) {
      chat.triggerRender();
      if (followScroll) chat.followScrollToBottom();
    }
  }

  function payloadLoop(payload: any): number | null {
    const raw = payload?.loopCount ?? payload?.loop_count ?? payload?.loop;
    const value = Number(raw);
    return Number.isFinite(value) && value > 0 ? value : null;
  }

  function commitTempBuffer(view: { contentBuffer: string; tempBuffer: string }) {
    if (!view.tempBuffer) return;
    view.contentBuffer += view.tempBuffer;
    view.tempBuffer = "";
  }

  function commitThinkingBuffer(
    view: { thinkingBuffer: string; toolBuffer: string; agentSteps: AgentStep[] },
    type: AgentStep["type"] = "thinking",
  ) {
    const thought = view.thinkingBuffer.trim();
    if (!thought) return;
    view.toolBuffer += `${thought}\n\n`;
    view.agentSteps.push({ type, content: thought, timestamp: Date.now() });
  }

  function hasCurrentTurnContent(view: SessionViewState) {
    const turn = view.currentTurn;
    return Boolean(
      turn.textBlocks.some((block) => block.content.trim()) ||
        turn.thinkingBlocks.some((block) => block.content.trim()) ||
        turn.toolCalls.length > 0 ||
        turn.logs.some((log) => log.content.trim())
    );
  }

  function latestRun(runs: AgentRun[], predicate: (run: AgentRun) => boolean) {
    return runs
      .filter(predicate)
      .sort((a, b) => (b.startedAt || 0) - (a.startedAt || 0))[0] ?? null;
  }

  /// 用 run 记录重建「当前 turn」的展示态。
  ///
  /// v15 起 `AgentRun` **不再携带 live_content / live_thinking / live_tool_buffer**
  /// （那三列已删除，改由「每轮一行」的 agent_run_events 承载）。所以这里不再
  /// 灌任何正文/思考，只恢复**运行态标记**（开始时间、轮次、是否进行中）。
  ///
  /// 这个取舍是刻意的：run 记录从来不是内容的权威来源——中断后要显示什么，
  /// 由 `recover_interrupted_into_memory` 从 events 重放并落进会话历史，
  /// 界面按历史渲染即可。之前在这里灌 live_* 反而制造了"刷新多一条"的重复渲染
  /// （见 `applyAgentRunState` 里那两处 `hasPersistedHistory` 守卫的注释）。
  function hydrateCurrentTurnFromRun(view: SessionViewState, run: AgentRun) {
    resetAgentCurrentTurn(view);
    view.currentTurn.startedAt = run.startedAt || Date.now();
    beginAgentLoop(view, run.loopCount || 1);
    view.currentTurn.isRunning = run.status === "running";
    view.currentTurn.revision += 1;
  }

  /**
   * 把后端 run 记录同步成视图的**运行态**（RUNNING / INTERRUPTED / 收口）。
   *
   * ⚠️ 本函数**不推进 `hydration`**（会话消息窗口的生命周期状态）。
   *
   * 历史教训（2026-11-23 定位）：这里原本在三条分支上都写了 `view.hydrated = true`，
   * 布尔时代的语义是「视图有内容可渲染」。迁移到 `hydration` 状态机后，
   * `markSessionReady` 的含义变成「消息窗口已与后端一致」，于是 `idle` 视图被无条件
   * 提升为 `ready`：切会话时 `Sidebar.switchToSession` 先跑 `loadAgentRunsFromBackend`
   * （会走到本函数），再跑 `ensureSessionViewLoaded` —— 后者看到 `ready` 直接 no-op，
   * 首屏 `get_session_messages_paged` 一次都不发，界面停在空白 WelcomeScreen。
   *
   * 判据：run 状态与消息窗口是两个正交的事实源。只有真正把消息写进视图的路径
   * （`replaceSessionMessages` / 事件补齐）才有资格置 `ready`。
   */
  function applyAgentRunState(sessionId: string, runs: AgentRun[]) {
    const view = session.getSessionView(sessionId);
    const running = latestRun(runs, (run) => run.sessionId === sessionId && run.status === "running");
    const interrupted = latestRun(
      runs,
      (run) =>
        run.sessionId === sessionId &&
        run.status === "interrupted" &&
        run.resumable &&
        !dismissedInterruptedRuns.has(run.runId)
    );

    if (running) {
      // 如果前端已确认结束（IDLE / FINISH / ERROR），不再被后端事件覆写
      if (view.status !== "RUNNING" && view.status !== "INTERRUPTED") {
        return;
      }
      if (view.resumableRunId) {
        dismissedInterruptedRuns.add(view.resumableRunId);
      }
      const prevRunId = view.activeRunId;
      view.status = "RUNNING";
      view.activeRunId = running.runId;
      view.resumableRunId = null;
      view.runStartTime = running.startedAt || Date.now();
      view.streamActive = true;
      view.cancelHandled = false;
      // 只在 runId 真的切了才重建（首次运行由 streaming 事件构建，不从 DB 覆盖）。
      // 另加"会话尚无持久化消息"条件，理由同下方 interrupted 分支：
      // 历史里已有该 run 的部分正文时，再注入 live_content 会渲染成两份。
      if (prevRunId && prevRunId !== running.runId && view.messages.length === 0) {
        hydrateCurrentTurnFromRun(view, running);
      }
      // 不置 ready：本分支只重建了 currentTurn（内存展示层），messages 仍是空的
      return;
    }

    view.activeRunId = null;
    view.streamActive = false;
    view.runStartTime = null;
    // run 中断收口：流式态方案预览（"生成中..."）已无主，清掉防面板永久卡死
    perm.clearStreamingPlanProposal(sessionId);

    if (interrupted) {
      view.resumableRunId = interrupted.runId;
      if (view.status === "RUNNING" || view.status === "IDLE" || view.status === "INTERRUPTED") {
        view.status = "INTERRUPTED";
      }
      // 仅当会话**尚无已持久化消息**时，才用 run 的 live_* 重建当前 turn。
      //
      // 为什么必须加这个条件：`applyAgentRunState` 在本函数里被调用**两次**
      // （历史加载前一次、加载后一次）。第一次调用时 currentTurn 还是空的，
      // 原来的守卫 `!hasCurrentTurnContent` 会通过，于是把 `live_content`
      // 拼进当前 turn；随后历史消息又渲染了同一条正文 → **界面多出一份**。
      // （实测表现：刷新后多一条 assistant，再刷新才正常。）
      //
      // 已存在持久化消息时，历史就是权威来源，`live_content` 只是它的流式备份，
      // 不能再渲染一遍。
      const hasPersistedHistory = view.messages.length > 0;
      if (!hasPersistedHistory && !hasCurrentTurnContent(view)) {
        hydrateCurrentTurnFromRun(view, interrupted);
      }
      view.currentTurn.isRunning = false;
      // 同上：仅同步运行态，不代表消息窗口已对齐
      return;
    }

    view.resumableRunId = null;
    if (view.status === "RUNNING" || view.status === "INTERRUPTED") {
      view.status = "IDLE";
    }
    view.currentTurn.isRunning = false;
    // run 正常收口：同上，清理未走完正式提交流程的流式态方案预览
    perm.clearStreamingPlanProposal(sessionId);
    // 收口同样不置 ready：消息由 checkpoint-created / session-messages-changed
    // 等落库事件驱动重载，那几条路径各自负责推进 hydration。
  }

  /**
   * 把本轮用户消息的数据库 id 回填到本地占位消息。
   *
   * 为什么要靠 run 事件而不是命令回执：
   * - 前端发消息时只有本地占位 id（`pending_*`），真实 UUID 由后端在
   *   `pre_loop` 注入用户消息时生成并落库；
   * - `ask_jarvis` 的返回值要等**整轮结束**才带回 `user_message_id`，
   *   一旦模型调用报错、命令以 Err 返回，那条回填代码永远不会执行；
   * - 而 run 记录在开跑时就通过 `agent-run-updated` 带出了 `message_id`，
   *   这是同一事实更早、且异常路径也能到达的通道。
   *
   * 撤回按钮的渲染条件是 `rollbackCheckpointId || messageId`：检查点由收尾
   * 生成，运行中必然为空；少了这一步，运行期间和失败后都只剩占位 id，
   * 只能等刷新从历史消息里拿回真实 id。
   *
   * 只认 running 且不早于本轮发送时刻的 run：切会话 / 重启恢复会批量加载
   * 历史 run，不能把旧 id 贴到新的占位消息上。
   */
  function backfillRunningUserMessageId(run: AgentRun) {
    if (run.status !== "running" || !run.messageId) return;
    const view = session.getSessionView(run.sessionId);
    if (view.status !== "RUNNING" || !view.runStartTime) return;
    if ((run.startedAt || 0) < view.runStartTime) return;
    // 只认列表尾部那条用户消息：正常发送时占位消息刚被追加到末尾，
    // 后面还没有任何助手消息；续跑（resume）不会追加用户消息，尾部是
    // 上一轮的助手消息 / 错误快照，这里必须放过，否则会把续跑新消息的
    // id 贴到上一条用户消息上，撤回目标就错了。
    const last = view.messages[view.messages.length - 1];
    if (!last || last.role !== "user" || last.messageId) return;
    if (!String(last.id).startsWith("pending_")) return;
    last.messageId = run.messageId;
  }

  async function refreshSessionHistory(sessionId: string, reason: string) {
    try {
      // 入口三：强制重载会话视图（分页首屏 + 重置分页游标）
      await chat.reloadSessionView(sessionId, reason);
    } catch (err) {
      // 无 HTML 兜底（通道已删除）：置标记，界面显示可翻译的提示
      console.error("加载会话消息失败:", err);
      session.setSessionLoadError(sessionId, true);
    }
  }

  async function loadSubAgentRunsFromBackend(sid?: string | null) {
    const effectiveSid = sid ?? session.activeSessionId;
    if (!effectiveSid) return;
    const existingLoad = subAgentRunsLoads.get(effectiveSid);
    if (existingLoad) return existingLoad;

    const load = (async () => {
      try {
        const [runs, traceRuns] = await Promise.all([
          invoke<SubAgentRun[]>("list_subagents", { sessionId: effectiveSid }),
          invoke<SubAgentTraceRunSummary[]>("list_subagent_trace_runs", { sessionId: effectiveSid }),
        ]);
        const otherRuns = Object.fromEntries(
          Object.entries(agent.subAgentRuns).filter(([, run]) => run.sessionId !== effectiveSid)
        );
        agent.subAgentRuns = {
          ...otherRuns,
          ...Object.fromEntries(runs.map((run) => [run.runId, run])),
        };
        agent.subAgentTraceRunsBySession = {
          ...agent.subAgentTraceRunsBySession,
          [effectiveSid]: traceRuns,
        };
      } catch (err) {
        console.error("加载子 Agent 运行记录失败:", err);
      }
    })();
    subAgentRunsLoads.set(effectiveSid, load);
    try {
      await load;
    } finally {
      if (subAgentRunsLoads.get(effectiveSid) === load) subAgentRunsLoads.delete(effectiveSid);
    }
  }

  async function loadSubAgentTraceFromBackend(runId: string) {
    const version = (subAgentTraceLoadVersion.get(runId) ?? 0) + 1;
    subAgentTraceLoadVersion.set(runId, version);
    agent.subAgentTraceByRun[runId] ??= [];
    try {
      const events = await invoke<SubAgentTraceEvent[]>("list_subagent_trace_events", { runId });
      if (subAgentTraceLoadVersion.get(runId) === version && Object.prototype.hasOwnProperty.call(agent.subAgentTraceByRun, runId)) {
        agent.subAgentTraceByRun[runId] = events;
      }
    } catch (err) {
      console.error("加载子 Agent 轨迹失败:", err);
    }
  }

  async function loadPlanDocumentsFromBackend(sid?: string | null) {
    try {
      const effectiveSid = sid ?? session.activeSessionId;
      if (!effectiveSid) return;
      const documents = await invoke<PlanDocument[]>("list_plan_documents", { sessionId: effectiveSid });
      perm.planDocumentsBySession = {
        ...perm.planDocumentsBySession,
        [effectiveSid]: documents,
      };
    } catch (err) {
      console.error("加载计划文档失败:", err);
      const effectiveSid = sid ?? session.activeSessionId;
      if (effectiveSid) {
        perm.planDocumentsBySession = { ...perm.planDocumentsBySession, [effectiveSid]: [] };
      }
    }
  }

  async function loadTodosFromBackend(sid?: string | null) {
    try {
      const effectiveSid = sid ?? session.activeSessionId;
      if (!effectiveSid) return;
      const groups = await invoke<TodoGroup[]>("get_session_todos", { sessionId: effectiveSid });
      agent.todoGroupsBySession = {
        ...agent.todoGroupsBySession,
        [effectiveSid]: groups,
      };
    } catch (err) {
      console.error("加载 todos 失败:", err);
    }
  }

  async function loadAgentRunsFromBackend(sid?: string | null, options: { refreshHistory?: boolean } = {}) {
    try {
      const effectiveSid = sid ?? session.activeSessionId;
      if (!effectiveSid) return;
      const beforeMsgCount = session.getSessionView(effectiveSid).messages.length;
      const runs = await invoke<AgentRun[]>("list_agent_runs", { sessionId: effectiveSid });
      const otherRuns = Object.fromEntries(
        Object.entries(agent.agentRuns).filter(([, run]) => run.sessionId !== effectiveSid)
      );
      agent.agentRuns = {
        ...otherRuns,
        ...Object.fromEntries(runs.map((run) => [run.runId, run])),
      };
      applyAgentRunState(effectiveSid, runs);
      if (options.refreshHistory !== false) {
        await refreshSessionHistory(effectiveSid, 'agent-runs');
        applyAgentRunState(effectiveSid, runs);
        if (session.getSessionView(effectiveSid).messages.length !== beforeMsgCount) {
          chat.triggerRender();
        }
      }
      syncActiveSessionView(effectiveSid, false);
    } catch (err) {
      console.error("加载主 Agent 执行记录失败:", err);
    }
  }

  async function loadAgentRunEventsFromBackend(sid?: string | null) {
    try {
      const effectiveSid = sid ?? session.activeSessionId;
      if (!effectiveSid) return;
      const events = await invoke<AgentRunLoopEvent[]>("list_agent_run_events", { sessionId: effectiveSid, runId: null });
      const grouped = events.reduce<Record<string, AgentRunLoopEvent[]>>((acc, item) => {
        if (!acc[item.runId]) acc[item.runId] = [];
        acc[item.runId].push(item);
        return acc;
      }, {});
      // 排序口径：同 run 内按 loopIndex 升序（轮次顺序才是重建顺序）；
      // 跨 run 无所谓——每 run 一个桶，桶之间不比较。
      for (const runEvents of Object.values(grouped)) {
        runEvents.sort((a, b) => (a.loopIndex || 0) - (b.loopIndex || 0));
      }
      const otherEvents = Object.fromEntries(
        Object.entries(agent.agentRunEventsByRun).filter(([, runEvents]) => {
          return runEvents.length > 0 && runEvents[0].sessionId !== effectiveSid;
        })
      );
      agent.agentRunEventsByRun = { ...otherEvents, ...grouped };
    } catch (err) {
      console.error("加载主 Agent 事件历史失败:", err);
    }
  }

  async function loadContextSnapshotFromBackend(sid?: string | null) {
    try {
      const effectiveSid = sid ?? session.activeSessionId;
      if (!effectiveSid) return;
      const snapshot = await invoke<SessionContextSnapshot | null>("get_session_context_snapshot", {
        sessionId: effectiveSid,
      });
      if (snapshot) {
        agent.upsertContextSnapshot(snapshot);
      } else {
        agent.clearContextSnapshot(effectiveSid);
      }
    } catch (err) {
      console.error("加载上下文快照失败:", err);
    }
  }

  const initListeners = async () => {
    // 跨 HMR 生命周期：如果当前 window 已有监听器，先全部清理再重新注册
    if (window.__jarvisUnlisteners) {
      console.warn("[JarvisAgent] HMR 检测：清理旧事件监听器，重新注册");
      for (const unlisten of window.__jarvisUnlisteners) {
        unlisten();
      }
      window.__jarvisUnlisteners = undefined;
      window.__jarvisListenersInitialized = false;
    }

    if (window.__jarvisListenersInitializing) {
      console.warn("[JarvisAgent] 事件监听器正在注册，跳过重复注册");
      return;
    }

    if (window.__jarvisListenersInitialized) {
      if (window.__jarvisUnlisteners) {
        console.warn("[JarvisAgent] 事件监听器已全局注册，跳过重复注册");
        return;
      }
      console.warn("[JarvisAgent] 检测到过期监听器标志，重新注册");
      window.__jarvisListenersInitialized = false;
    }
    window.__jarvisListenersInitializing = true;

    const unlisteners: UnlistenFn[] = [];
    // 辅助：注册事件监听器并收集 unlisten 句柄
    async function on<T>(event: string, handler: (event: { payload: T }) => void) {
      const unlisten = await listen<T>(event, handler);
      unlisteners.push(unlisten);
    }

    // todos（按会话隔离，按组存储）
    await on<TodoUpdatePayload>("todo-update", (event) => {
      const { todos, groups, sessionId } = event.payload;
      if (sessionId) {
        // 后端同时给扁平数组和分组；缺 groups 时退化成一个组（兼容旧事件）
        const nextGroups =
          groups ?? (todos.length > 0 ? [{ id: sessionId, items: todos }] : []);
        agent.todoGroupsBySession = { ...agent.todoGroupsBySession, [sessionId]: nextGroups };
      }
      chat.triggerRender();
    });

    // permission
    await on<PermissionRequest>("permission-request", (event) => {
      // 入队而不是覆盖：并行子代理可能同时发起多个权限请求
      perm.enqueuePermission(event.payload);
    });

    await on<PermissionResolvedPayload>("permission-resolved", (event) => {
      const sid = event.payload.sessionId ?? session.activeSessionId;
      // 按 id 精确移除：一张卡的决议不能把排在它后面的请求一起抹掉
      perm.removePermission(sid, event.payload.id);
    });

    // memory update notice (transient, not persisted)
    await on<{ sessionId?: string; summary: string }>("memory-updated", (event) => {
      chat.memoryNotice = event.payload.summary;
      // auto-dismiss after 30 seconds
      setTimeout(() => {
        if (chat.memoryNotice === event.payload.summary) {
          chat.memoryNotice = null;
        }
      }, 30000);
    });

    // 后台任务失败提醒（transient, not persisted）
    //
    // 后台任务结果不再注入会话上下文（见后端 drain_background_notifications 的移除）：
    // 失败信息改走这条通知条给用户看，用户决定是否发消息让 Agent 排查
    // （排查用 CheckBackgroundCommand 读后台输出）。
    // 秒挂场景不走这里：错误已经在启动时的 tool_result 里带回，模型当轮就能看到。
    await on<{ sessionId?: string; taskId: string; command: string; result: string }>(
      "background-failed",
      (event) => {
        const p = event.payload;
        if (!p) return;
        const sid = p.sessionId ?? session.activeSessionId;
        // 只提醒当前活跃会话的任务；非活跃会话的任务在监控面板可见
        if (!sid || sid !== session.activeSessionId) return;
        const cmd = (p.command || "").slice(0, 60);
        const text = `后台任务 ${p.taskId}（${cmd}）失败：${p.result}。可发消息让 Agent 继续检查。`;
        chat.memoryNotice = text;
        // 30 秒自动消失；startsWith 防止误清掉之后的其他提醒
        setTimeout(() => {
          if (chat.memoryNotice?.startsWith(`后台任务 ${p.taskId}`)) {
            chat.memoryNotice = null;
          }
        }, 30000);
      },
    );

    // plan proposal stream (chunked)
    await on<{ sessionId?: string; content: string }>("plan-proposal-stream", (event) => {
      const sid = event.payload.sessionId ?? session.activeSessionId;
      if (sid) {
        perm.updatePlanProposalStreamingContent(sid, event.payload.content);
        session.markSessionReady(sid);
        syncActiveSessionView(sid, false);
      }
    });

    // plan proposal
    await on<PlanProposal>("plan-proposal", (event) => {
      const sid = event.payload.sessionId ?? session.activeSessionId;
      if (sid) {
        const view = session.getSessionView(sid);
        hideSubmittedPlanFromChat(
          view,
          event.payload,
          t("plan.submittedNotice", { title: event.payload.title }),
        );
        perm.finalizePlanProposal(sid, event.payload);
        perm.upsertPlanDocument(
          {
            id: event.payload.id,
            sessionId: sid,
            title: event.payload.title,
            content: event.payload.content,
            status: "pending",
            path: null,
            // 时间戳口径：毫秒（v16 起全项目 DB 统一毫秒，与后端 PlanDocument 一致）
            createdAt: Date.now(),
            updatedAt: Date.now(),
            decidedAt: null,
          },
          sid
        );
        session.markSessionReady(sid);
        syncActiveSessionView(sid, true);
      }
    });

    // plan document updated
    await on<PlanDocument>("plan-document-updated", (event) => {
      perm.upsertPlanDocument(event.payload);
    });

    // agent run
    await on<AgentRun>("agent-run-updated", (event) => {
      const run = event.payload;
      agent.upsertAgentRun(run);
      // 用户消息 id 随 run 下发：一开跑就回填，撤回按钮不必等整轮结束或刷新
      backfillRunningUserMessageId(run);
      const runs = Object.values(agent.agentRuns).filter((item) => item.sessionId === run.sessionId);
      applyAgentRunState(run.sessionId, runs);
      syncActiveSessionView(run.sessionId, false);
    });

    // 注意：v15 起后端**不再**推送 `agent-run-event`。
    //
    // 旧的 `agent_run_events` 是「一条事件一行」的日志流，每 loop 会产生 start /
    // delta / tool / complete 若干行，前端必须靠推送逐条追加才不至于落后。
    // 现在它改成「1 loop 1 行」的**重建数据源**（`(run_id, loop_index)` 唯一键，
    // 整行覆盖写），语义从"事件流"变成了"快照表"——推送增量已无意义：
    // 同一行会被反复覆盖，前端拼出来的中间态只会是半截 JSON。
    //
    // 因此这里不再注册监听器。界面要看的运行态内容（正文/思考/工具）走
    // `chat-content` / `chat-thinking` / `agent-step` 等流式事件；
    // 重建数据只在 `loadAgentRunEventsFromBackend` 里整批拉取。

    // context snapshot
    await on<SessionContextSnapshot>("context-snapshot-updated", (event) => {
      const snapshot = event.payload;
      if (!snapshot?.sessionId) return;
      agent.upsertContextSnapshot(snapshot);
    });

    // 会话累计用量：后端**每次请求**拿到 usage 后就推一次（不是回合收尾才推）。
    // 概览栏的「累计命中」靠它才能在长回合里逐 loop 跳动，而不是停在上一轮的值。
    // 载荷是**绝对值**，这里整值覆盖；再累加一遍会滚成两倍。
    await on<SessionUsageUpdatedPayload>("session-usage-updated", (event) => {
      const usage = event.payload;
      if (!usage?.sessionId) return;
      session.setSessionUsageTotals(usage.sessionId, {
        input: usage.inputTokens,
        output: usage.outputTokens,
        cacheHit: usage.cacheHitTokens,
        cacheMiss: usage.cacheMissTokens,
      });
      // 按来源分组用量：与总量同口径，载荷是**绝对值**，整值覆盖，不自行累加。
      // 旧载荷没有 breakdown 字段时归一到「全 0 + hasDetail=false」，不报错、不抛异常。
      session.setSessionUsageBreakdown(usage.sessionId, usage.breakdown);
      // 这一次请求到此结束：结算它的输出速度
      // （概览栏的 tok/s 就是这一跳的读数，不是会话累计）
      session.settleRequestThroughput(usage.sessionId, usage.outputTokens || 0, Date.now());
    });

    // chat turn start
    await on<any>("chat-turn-start", (event) => {
      const sessionId = event.payload?.sessionId ?? session.activeSessionId;
      if (!sessionId) return;
      const view = session.getSessionView(sessionId);
      if (view.resumableRunId) {
        dismissedInterruptedRuns.add(view.resumableRunId);
      }
      view.resumableRunId = null;
      view.status = "RUNNING";
      commitTempBuffer(view);
      commitThinkingBuffer(view);
      view.thinkingBuffer = "";
      const startedLoop = payloadLoop(event.payload);
      beginAgentLoop(view, startedLoop);
      // 概览栏的「步」= 本轮 agent 跑到了第几次 loop 迭代：这里记下实时值。
      // （不在 `currentTurn` 上读它：那个缓冲是 run-scoped，回合结束就被重置。
      //   跨重启的持久化由 `agent_run_events` 承担，见 session.ts 的 loopStepCount 注释。）
      if (startedLoop) session.noteLoopStep(sessionId, startedLoop);
      view.streamActive = true;
      session.markSessionReady(sessionId);
      syncActiveSessionView(sessionId, true);
    });

    // chat content — 流式片段必须逐段追加，不能按文本内容去重。
    await on<any>("chat-content", (event) => {
      const sessionId = event.payload?.sessionId ?? session.activeSessionId;
      if (!sessionId) return;
      const { content } = event.payload;
      if (!content) return;
      // 主 Agent 正文片段 = 本次请求生成的开始（子代理的内容不算，那不是这一次请求）
      if (!event.payload?.isSubAgent) session.markRequestFirstDelta(sessionId, Date.now());
      const view = session.getSessionView(sessionId);
      view.tempBuffer += content;
      appendAgentText(view, content, "assistant", payloadLoop(event.payload));
      view.streamActive = true;
      session.markSessionReady(sessionId);
      syncActiveSessionView(sessionId, true);
    });

    // chat thinking — 同样保留所有片段，避免重复词或重复标点被误删。
    await on<any>("chat-thinking", (event) => {
      if (event.payload?.isSubAgent) return; // 子代理事件不注入主聊天
      const sessionId = event.payload?.sessionId ?? session.activeSessionId;
      if (!sessionId) return;
      const { content } = event.payload;
      if (!content) return;
      // 思考片段同样是本次请求的输出，也要算起点：
      // 推理模型常常先吐一长段思考再吐正文，只认正文会把这前半段排除在窗口外
      session.markRequestFirstDelta(sessionId, Date.now());
      const view = session.getSessionView(sessionId);
      view.thinkingBuffer += content;
      appendAgentThinking(view, content, payloadLoop(event.payload));
      view.streamActive = true;
      session.markSessionReady(sessionId);
      syncActiveSessionView(sessionId, true);
    });

    // chat tool start
    await on<any>("chat-tool-start", (event) => {
      const sessionId = event.payload?.sessionId ?? session.activeSessionId;
      if (!sessionId) return;
      const view = session.getSessionView(sessionId);
      commitTempBuffer(view);
      commitThinkingBuffer(view);
      view.thinkingBuffer = "";
      markAgentToolActivity(view, payloadLoop(event.payload));
      if (event.payload?.toolCallId && event.payload?.tool) {
        upsertAgentToolCall(
          view,
          String(event.payload.toolCallId),
          String(event.payload.tool),
          "pending",
          payloadLoop(event.payload),
        );
      }
      view.streamActive = true;
      session.markSessionReady(sessionId);
      syncActiveSessionView(sessionId, true);
    });

    // chat tool debug
    await on<any>("chat-tool-debug", (event) => {
      const sessionId = event.payload?.sessionId ?? session.activeSessionId;
      if (!sessionId) return;
      const view = session.getSessionView(sessionId);
      const { content, kind, toolCallId, tool, status } = event.payload;
      if (kind === "tool_status" && toolCallId && tool && status) {
        upsertAgentToolCall(view, String(toolCallId), String(tool), String(status), payloadLoop(event.payload));
      } else if (content) {
        view.toolBuffer += content;
        appendAgentExecutionLog(view, content, payloadLoop(event.payload));
      }
      view.streamActive = true;
      session.markSessionReady(sessionId);
      syncActiveSessionView(sessionId, true);
    });

    // chat stream — 工具/子代理输出
    await on<any>("chat-stream", (event) => {
      if (event.payload?.isSubAgent) return; // 子代理事件不注入主聊天
      const sessionId = event.payload?.sessionId ?? session.activeSessionId;
      if (!sessionId) return;
      const { content } = event.payload;
      if (!content) return;
      const view = session.getSessionView(sessionId);
      view.toolBuffer += content;
      appendAgentExecutionLog(view, content, payloadLoop(event.payload));
      view.streamActive = true;
      session.markSessionReady(sessionId);
      syncActiveSessionView(sessionId, true);
    });

    // chat turn end
    await on<any>("chat-turn-end", (event) => {
      const sessionId = event.payload?.sessionId ?? session.activeSessionId;
      if (!sessionId) return;
      const { has_tool } = event.payload;
      const view = session.getSessionView(sessionId);
      commitThinkingBuffer(view, has_tool ? "plan" : "thinking");
      commitTempBuffer(view);
      view.thinkingBuffer = "";
      finishAgentLoop(view, Boolean(has_tool));
      view.streamActive = has_tool;
      session.markSessionReady(sessionId);
      syncActiveSessionView(sessionId, true);
    });

    // agent step
    await on<any>("agent-step", (event) => {
      if (event.payload?.isSubAgent) return; // 子代理事件不注入主聊天
      const sessionId = event.payload?.sessionId ?? session.activeSessionId;
      if (!sessionId) return;
      const step = event.payload as Omit<AgentStep, "timestamp">;
      const view = session.getSessionView(sessionId);
      // 等待提示与重试进度都是**过程性状态标注**：写进 turn.notice
      // （渲染在气泡下方小字），不进正文、不落库。
      // 经 chat-stream 会被当成模型输出写进回复气泡内部（历史遗留问题）。
      //
      // 后端只发**秒数参数**（`silentSecs`/`remainSecs`/`waitSecs`），句子在这里用
      // 语言包拼 —— 它曾经直接发中文 `content`，英文界面下会冒中文。
      // 与中断类标注（后端发 `notice.*` key、由 `noticeTextFromBackend` 成句）
      // 共用同一个语言包命名空间，两处措辞口径统一。
      //
      // ⚠️ 2026-11-22：写入时**必须带 `noticeKind`**。notice 是单向写入的字段，
      // 进程态标注在事情办成后没有任何自清机制（会一直挂到整轮 loop 结束）；
      // 清除由后端的 `notice_clear` 驱动，它按来源精确匹配，没这个标记就清不掉。
      if (step.type === "waiting_hint") {
        view.currentTurn.notice = t("notice.waitingHint", {
          silent: step.silentSecs ?? 0,
          remain: step.remainSecs ?? 0,
        });
        view.currentTurn.noticeKind = "waiting_hint";
        view.streamActive = true;
        session.markSessionReady(sessionId);
        syncActiveSessionView(sessionId, true);
        return;
      }
      if (step.type === "retry") {
        view.currentTurn.notice = t("notice.retry", {
          secs: step.waitSecs ?? 0,
          attempt: step.attempt ?? 0,
          max: step.max ?? 0,
        });
        view.currentTurn.noticeKind = "retry";
        view.streamActive = true;
        session.markSessionReady(sessionId);
        syncActiveSessionView(sessionId, true);
        return;
      }
      // 过程态标注的**清除**信号：后端在“重试成功拿到响应头”“SSE 收到新数据”
      // “本轮不再等上游数据”三个恢复点发出。
      // 只清**同来源**的标注 —— 重试恢复不该顺手抹掉「反思审查要求改进」这类
      // 仍然有效的提示；noticeKind 不匹配就保持原样。
      if (step.type === "notice_clear") {
        const kinds = step.kinds ?? [];
        const currentKind = view.currentTurn.noticeKind;
        if (view.currentTurn.notice && currentKind && kinds.includes(currentKind)) {
          view.currentTurn.notice = undefined;
          view.currentTurn.noticeKind = undefined;
          view.currentTurn.revision += 1;
          syncActiveSessionView(sessionId, true);
        }
        return;
      }
      // 反思审查的判定：**只报「判为需要改进」**（not_ok）。
      // 判 ok 是常态，每次都往这里写会把气泡下方那行字变成噪音；
      // not_ok 才是用户真正需要知道的事件 —— Agent 即将自我纠正。
      // 走 notice 而不进正文/工具卡：它是"过程性状态标注"，
      // 与 waiting_hint / retry 同一类，由 AgentTurnNotice 渲染在气泡下方。
      if (step.type === "reflection") {
        if (step.judgment && step.judgment !== "ok") {
          const detail = [step.reason, step.suggestion]
            .map((part) => (part || "").trim())
            .filter(Boolean)
            .join(" — ");
          view.currentTurn.notice = detail
            ? `${t("input.reflectionFlagged")} — ${detail}`
            : t("input.reflectionFlagged");
          view.currentTurn.noticeKind = "reflection";
        }
        view.streamActive = true;
        session.markSessionReady(sessionId);
        syncActiveSessionView(sessionId, true);
        return;
      }
      const fullStep = { ...step, timestamp: Date.now() } as AgentStep;
      view.agentSteps.push(fullStep);
      applyAgentStepToCurrentTurn(view, fullStep);
      session.markSessionReady(sessionId);
    });

    // subagent updated（节流批量更新，减轻 5s 心跳 × N 子 Agent 的渲染压力）
    let pendingSubAgentRuns: Record<string, SubAgentRun> = {};
    let subAgentFlushTimer: ReturnType<typeof setTimeout> | null = null;

    await on<SubAgentRun>("subagent-updated", (event) => {
      const run = event.payload;
      if (!run?.runId) return;
      pendingSubAgentRuns[run.runId] = run;
      if (!subAgentFlushTimer) {
        subAgentFlushTimer = setTimeout(() => {
          const flushedRuns = Object.values(pendingSubAgentRuns);
          agent.subAgentRuns = { ...agent.subAgentRuns, ...pendingSubAgentRuns };
          const summariesBySession = { ...agent.subAgentTraceRunsBySession };
          for (const updatedRun of flushedRuns) {
            const sessionRuns = summariesBySession[updatedRun.sessionId] ?? [];
            const index = sessionRuns.findIndex((item) => item.runId === updatedRun.runId);
            const summary: SubAgentTraceRunSummary = {
              runId: updatedRun.runId,
              sessionId: updatedRun.sessionId,
              parentRunId: index >= 0 ? sessionRuns[index].parentRunId : null,
              label: updatedRun.label,
              startedAt: updatedRun.startedAt,
              loopCount: updatedRun.loopCount,
            };
            const nextRuns = [...sessionRuns];
            if (index >= 0) nextRuns[index] = summary;
            else nextRuns.push(summary);
            summariesBySession[updatedRun.sessionId] = nextRuns.sort((a, b) => b.startedAt - a.startedAt);
          }
          agent.subAgentTraceRunsBySession = summariesBySession;
          pendingSubAgentRuns = {};
          subAgentFlushTimer = null;
        }, 2000);
      }
    });

    await on<{runId:string}>("subagent-trace-updated", (event) => {
      const runId = event.payload?.runId;
      if (runId && Object.prototype.hasOwnProperty.call(agent.subAgentTraceByRun, runId)) {
        void loadSubAgentTraceFromBackend(runId);
      }
    });


    // checkpoint created
    await on<any>("checkpoint-created", async (event) => {
      const sessionId = event.payload?.sessionId ?? session.activeSessionId;
      if (!sessionId) return;
      const view = session.getSessionView(sessionId);
      if (event.payload?.checkpointId) {
        view.latestCheckpoint = {
          id: event.payload.checkpointId,
          hasOperations: event.payload.hasOperations === true,
          hasPatches: event.payload.hasPatches === true,
          canRollback: true,
        };
      } else if (event.payload?.canRollback) {
        view.latestCheckpoint = {
          id: "",
          hasOperations: false,
          hasPatches: false,
          canRollback: true,
        };
      }
      session.markSessionReady(sessionId);
      // 会话仍在运行时不刷新消息——checkpoint 的部分回复会和 live currentTurn 重叠
      // ERROR 状态不刷新——错误消息仅在客户端 messages 中，刷新会冲掉
      if (view.status !== 'RUNNING' && view.status !== 'ERROR') {
        await refreshSessionHistory(sessionId, 'checkpoint');
      }
      syncActiveSessionView(sessionId, false);
    });

    // 会话消息落库变更（后端在「消息真正写进 DB」后广播）
    //
    // 这是**事件驱动的缓存失效**：事件只告诉我们「哪个会话的后端数据变了」，
    // 拉到什么由 chat store 的 hydration 状态机裁决。
    // 核心取舍：非活跃会话**只标脏、绝不发 IPC** —— 后台会话（别的窗口/后台任务
    // 在那边写消息）爱怎么写都不影响前台的请求量，等用户切回来时由
    // `ensureSessionViewLoaded` 消费 stale 态拉一次，切回只拉一次。
    //
    // 载荷是后端的固定契约 `{ sessionId, count, lastSeq, reason }`，前端不改名/不加字段：
    // - `count`：本批写了多少条消息（只用于合并与日志，不参与判断）
    // - `lastSeq`：本批最后一条消息的 seq（空批为 null）
    // - `reason`：写入来源（如 turn-end / subagent / rollback），拼进审计用 reason
    //
    // 合并：同一微任务内同一会话的多次到达合成一条待办（见下方 pending 表）。
    // 第二层去重由 chat store 的在途请求表兜底（并发重载复用同一个 Promise）。
    //
    // 手工验证（非活跃会话切回只拉一次）：
    // 1. 打开会话 A，再切到会话 B（此时 A 非活跃，视图停在 ready）；
    // 2. 在 B 里跑一轮落库次数多的任务（工具调用多），全程让 B 保持前台；
    // 3. devtools 控制台应反复出现 A 的 `[chat] markSessionViewStale: session=A ...
    //    hydration=ready->stale`，但**没有** A 的 `session-view load start`
    //    （非活跃会话零 IPC）；
    // 4. 切回 A：控制台应只出现**一条** `session-view load start/done: session=A`；
    // 5. 事件在活跃会话 RUNNING 中到达时只标脏（`markSessionViewStale`，不重载），
    //    回合收尾由既有路径（agent-run-updated / checkpoint-created 的强制重载）消费 stale。
    type SessionMessagesChangedPayload = {
      sessionId: string;
      count: number;
      lastSeq: number | null;
      reason: string;
    };

    // 待处理表：sessionId → 合并后的一批变更（同一微任务内同会话只留一条）
    const pendingMessagesChanged = new Map<
      string,
      { reason: string; count: number; lastSeq: number | null }
    >();
    let messagesChangedFlushScheduled = false;

    /**
     * 消费一批「消息变更」待办。
     *
     * 三种分支对应三种状态下的正确处置，注释在各分支上。
     */
    async function applySessionMessagesChanged(
      sessionId: string,
      batch: { reason: string; count: number; lastSeq: number | null },
    ) {
      const reason = `messages-changed:${batch.reason}`;
      const view = session.getSessionView(sessionId);
      console.debug(
        `[events] session-messages-changed: session=${sessionId} count=${batch.count} lastSeq=${batch.lastSeq} reason=${reason} status=${view.status} hydration=${view.hydration}`,
      );

      // 分支一：会话正在跑（无论是否前台）→ 只标脏，不重载。
      // 正在流式输出时前端有 live currentTurn（正文/思考/工具都在流式事件里追加），
      // 此刻重载会和流式内容重叠成两份；回合结束时的既有收尾路径
      // （agent-run-updated / checkpoint-created 里的强制重载）会消费这个 stale。
      if (view.status === "RUNNING") {
        chat.markSessionViewStale(sessionId, reason);
        return;
      }

      // 分支二：活跃会话且不在运行 → 立刻与后端对齐（这一步才发 IPC）。
      // 注意先置脏：`ensureSessionViewLoaded` 在 `ready` 态是零 IPC 的 no-op，
      // 而事件已经明确告知「后端变了」，不置脏就白等。置脏只对 `ready` 生效
      // （`idle`/`loading` 由 session store 忽略，它们本来就要走首屏/在途请求）。
      if (sessionId === session.activeSessionId) {
        if (view.hydration === "ready") {
          chat.markSessionViewStale(sessionId, reason);
        }
        try {
          // stale → 一次重载；loading → 复用同一在途 Promise；idle → 拉首屏
          await chat.ensureSessionViewLoaded(sessionId, reason);
        } catch (err) {
          // 无 HTML 兜底（通道已删除）：置标记，界面显示可翻译的提示
          console.error("加载会话消息失败:", err);
          session.setSessionLoadError(sessionId, true);
        }
        return;
      }

      // 分支三：非活跃会话 → 只标脏，**不发 IPC**。
      // 数据在后端已是权威，界面此刻没人看；等切回该会话时
      // `active-session-changed` → `ensureSessionViewLoaded` 消费 stale 拉一次。
      chat.markSessionViewStale(sessionId, reason);
    }

    /**
     * 微任务级合并：同一事件在同一个微任务里多次到达（后端同一批写入连发多条）
     * 时只保留一条待办，避免短时间重复重载。跨微任务的重复由 chat store 的在途请求表兜底。
     */
    function scheduleMessagesChangedFlush() {
      if (messagesChangedFlushScheduled) return;
      messagesChangedFlushScheduled = true;
      queueMicrotask(() => {
        messagesChangedFlushScheduled = false;
        if (pendingMessagesChanged.size === 0) return;
        const batches = Array.from(pendingMessagesChanged.entries());
        pendingMessagesChanged.clear();
        for (const [sessionId, batch] of batches) {
          void applySessionMessagesChanged(sessionId, batch);
        }
      });
    }

    await on<SessionMessagesChangedPayload>("session-messages-changed", (event) => {
      const p = event.payload;
      if (!p?.sessionId) return;
      const existing = pendingMessagesChanged.get(p.sessionId);
      pendingMessagesChanged.set(p.sessionId, {
        // reason 取最后一次（最新来源更能说明「为什么变」）。
        // count / lastSeq 是**后端消息集合的快照值**，不是增量：合并时取最新值，
        // 不能累加（累加会得到一个不存在于任何时刻的假计数）。
        reason: p.reason || "unknown",
        count: p.count ?? existing?.count ?? 0,
        lastSeq: p.lastSeq ?? existing?.lastSeq ?? null,
      });
      scheduleMessagesChangedFlush();
    });

    // active session changed
    await on<SessionCleanupPayload>("active-session-changed", async (event) => {
      const deletedSessionId = event.payload?.deletedSessionId ?? null;
      const nextActiveSessionId = event.payload?.activeSessionId ?? null;

      if (deletedSessionId) {
        session.deleteSessionView(deletedSessionId);
      }

      session.activeSessionId = nextActiveSessionId;
      if (deletedSessionId) {
        delete perm.planProposals[deletedSessionId];
        delete perm.planDocumentsBySession[deletedSessionId];
        perm.clearPermissions(deletedSessionId);
        agent.agentRuns = Object.fromEntries(
          Object.entries(agent.agentRuns).filter(([, run]) => run.sessionId !== deletedSessionId)
        );
        agent.agentRunEventsByRun = Object.fromEntries(
          Object.entries(agent.agentRunEventsByRun).filter(([, events]) => {
            return events.some((item) => item.sessionId !== deletedSessionId);
          })
        );
        agent.subAgentRuns = Object.fromEntries(
          Object.entries(agent.subAgentRuns).filter(([, run]) => run.sessionId !== deletedSessionId)
        );
        const remainingTraceRuns = { ...agent.subAgentTraceRunsBySession };
        delete remainingTraceRuns[deletedSessionId];
        agent.subAgentTraceRunsBySession = remainingTraceRuns;
        agent.subAgentTraceByRun = Object.fromEntries(Object.entries(agent.subAgentTraceByRun).filter(([, events]) => events.some((item) => item.sessionId !== deletedSessionId)));
        agent.clearContextSnapshot(deletedSessionId);
      }

      try {
        if (nextActiveSessionId) {
          const meta = await invoke<any>("get_session_meta", { id: nextActiveSessionId });
          session.workingDirectory = meta.workingDirectory || null;
          session.setSessionUsageTotals(nextActiveSessionId, {
            input: meta.totalInputTokens || 0,
            output: meta.totalOutputTokens || 0,
            cacheHit: meta.totalCacheHitTokens || 0,
            cacheMiss: meta.totalCacheMissTokens || 0,
          });

          // 入口一：确保视图已装载（idle → 拉首屏，ready → 零 IPC，stale → 重拉）
          // 不再自己判 `hasHydratedSessionView`，需不需要拉的裁决统一收进 chat store。
          try {
            await chat.ensureSessionViewLoaded(nextActiveSessionId, 'session-fallback');
          } catch (err) {
            // 无 HTML 兜底（通道已删除）：置标记，界面显示可翻译的提示
            console.error("加载会话消息失败:", err);
            session.setSessionLoadError(nextActiveSessionId, true);
          }
          await Promise.all([
            loadSubAgentRunsFromBackend(nextActiveSessionId),
            loadPlanDocumentsFromBackend(nextActiveSessionId),
            loadAgentRunsFromBackend(nextActiveSessionId, { refreshHistory: false }),
            loadAgentRunEventsFromBackend(nextActiveSessionId),
            loadContextSnapshotFromBackend(nextActiveSessionId),
          ]);
        } else {
          session.workingDirectory = null;
          session.setSessionUsageTotals(null, {});
          session.resetSessionView(null);
        }
      } catch (err) {
        console.error("同步清理后的会话失败:", err);
        session.workingDirectory = null;
        session.setSessionUsageTotals(null, {});
        if (nextActiveSessionId) {
          session.resetSessionView(nextActiveSessionId);
        } else {
          session.resetSessionView(null);
        }
      }

      chat.resetRenderState();
      chat.triggerRender();
      chat.forceScrollToBottom();
    });

    // 保存所有 unlisten 句柄到 window 级别（跨 HMR 生命周期）
    window.__jarvisUnlisteners = unlisteners;
    window.__jarvisListenersInitialized = true;
    window.__jarvisListenersInitializing = false;

    // Vite HMR 清理：热更新时自动注销旧监听器
    if (import.meta.hot) {
      import.meta.hot.dispose(() => {
        if (window.__jarvisUnlisteners) {
          for (const unlisten of window.__jarvisUnlisteners) {
            unlisten();
          }
          window.__jarvisUnlisteners = undefined;
          window.__jarvisListenersInitialized = false;
          window.__jarvisListenersInitializing = false;
        }
      });
    }
  };

  return {
    initListeners,
    loadSubAgentRunsFromBackend,
    loadSubAgentTraceFromBackend,
    loadPlanDocumentsFromBackend,
    loadTodosFromBackend,
    loadAgentRunsFromBackend,
    loadAgentRunEventsFromBackend,
    loadContextSnapshotFromBackend,
  };
}
