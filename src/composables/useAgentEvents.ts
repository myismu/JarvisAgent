/**
 * # useAgentEvents.ts — Agent 事件监听与历史恢复
 *
 * 统一注册 Tauri 事件监听器，并从后端恢复主 Agent、子 Agent、计划文档和上下文快照状态。
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
  TodoItem,
  TodoUpdatePayload,
  PermissionRequest,
  PlanProposal,
  PlanDocument,
  AgentStep,
  AgentRun,
  AgentRunLoopEvent,
  SessionContextSnapshot,
  SessionUsageUpdatedPayload,
  SubAgentRun,
  SubAgentEvent,
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

function hideSubmittedPlanFromChat(view: SessionViewState, proposal: PlanProposal) {
  const notice = `我已整理实施方案「${proposal.title}」，请在右侧方案审批面板中审阅。`;
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
  const session = useSessionStore();
  const chat = useChatStore();
  const agent = useAgentStore();
  const perm = usePermissionStore();
  const dismissedInterruptedRuns = new Set<string>();

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
      view.hydrated = true;
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
      view.hydrated = true;
      return;
    }

    view.resumableRunId = null;
    if (view.status === "RUNNING" || view.status === "INTERRUPTED") {
      view.status = "IDLE";
    }
    view.currentTurn.isRunning = false;
    // run 正常收口：同上，清理未走完正式提交流程的流式态方案预览
    perm.clearStreamingPlanProposal(sessionId);
    view.hydrated = true;
  }

  async function refreshSessionHistory(sessionId: string) {
    try {
      const messages = await invoke<any[]>("get_session_messages", { sessionId });
      session.replaceSessionMessages(sessionId, messages);
    } catch {
      const history = await invoke<string>("get_session_history", { sessionId });
      session.replaceSessionHistory(sessionId, history);
    }
  }

  async function loadSubAgentRunsFromBackend(sid?: string | null) {
    try {
      const effectiveSid = sid ?? session.activeSessionId;
      if (!effectiveSid) return;
      const runs = await invoke<SubAgentRun[]>("list_subagents", { sessionId: effectiveSid });
      const otherRuns = Object.fromEntries(
        Object.entries(agent.subAgentRuns).filter(([, run]) => run.sessionId !== effectiveSid)
      );
      agent.subAgentRuns = {
        ...otherRuns,
        ...Object.fromEntries(runs.map((run) => [run.runId, run])),
      };
    } catch (err) {
      console.error("加载子 Agent 运行记录失败:", err);
    }
  }

  async function loadSubAgentEventsFromBackend(sid?: string | null) {
    try {
      const effectiveSid = sid ?? session.activeSessionId;
      if (!effectiveSid) return;
      const events = await invoke<SubAgentEvent[]>("list_subagent_events", { sessionId: effectiveSid, runId: null });
      const grouped = events.reduce<Record<string, SubAgentEvent[]>>((acc, item) => {
        if (!acc[item.runId]) acc[item.runId] = [];
        acc[item.runId].push(item);
        return acc;
      }, {});
      for (const runEvents of Object.values(grouped)) {
        runEvents.sort((a, b) => a.timestamp - b.timestamp);
      }
      const otherEvents = Object.fromEntries(
        Object.entries(agent.subAgentEventsByRun).filter(([, runEvents]) => {
          return runEvents.length > 0 && runEvents[0].sessionId !== effectiveSid;
        })
      );
      agent.subAgentEventsByRun = { ...otherEvents, ...grouped };
    } catch (err) {
      console.error("加载子 Agent 事件历史失败:", err);
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
      const todos = await invoke<TodoItem[]>("get_session_todos", { sessionId: effectiveSid });
      agent.todosBySession = {
        ...agent.todosBySession,
        [effectiveSid]: todos,
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
        await refreshSessionHistory(effectiveSid);
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

    // todos（按会话隔离）
    await on<TodoUpdatePayload>("todo-update", (event) => {
      const { todos, sessionId } = event.payload;
      if (sessionId) {
        agent.todosBySession = { ...agent.todosBySession, [sessionId]: todos };
      }
      chat.triggerRender();
    });

    // permission
    await on<PermissionRequest>("permission-request", (event) => {
      // 入队而不是覆盖：并行子代理可能同时发起多个权限请求
      perm.enqueuePermission(event.payload);
    });

    await on<{ id: string; sessionId: string; decision?: string; decisionText?: string }>("permission-resolved", (event) => {
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
        const view = session.getSessionView(sid);
        view.hydrated = true;
        syncActiveSessionView(sid, false);
      }
    });

    // plan proposal
    await on<PlanProposal>("plan-proposal", (event) => {
      const sid = event.payload.sessionId ?? session.activeSessionId;
      if (sid) {
        const view = session.getSessionView(sid);
        hideSubmittedPlanFromChat(view, event.payload);
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
        view.hydrated = true;
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
      beginAgentLoop(view, payloadLoop(event.payload));
      view.streamActive = true;
      view.hydrated = true;
      syncActiveSessionView(sessionId, true);
    });

    // chat content — 流式片段必须逐段追加，不能按文本内容去重。
    await on<any>("chat-content", (event) => {
      const sessionId = event.payload?.sessionId ?? session.activeSessionId;
      if (!sessionId) return;
      const { content } = event.payload;
      if (!content) return;
      const view = session.getSessionView(sessionId);
      view.tempBuffer += content;
      appendAgentText(view, content, "assistant", payloadLoop(event.payload));
      view.streamActive = true;
      view.hydrated = true;
      syncActiveSessionView(sessionId, true);
    });

    // chat thinking — 同样保留所有片段，避免重复词或重复标点被误删。
    await on<any>("chat-thinking", (event) => {
      if (event.payload?.isSubAgent) return; // 子代理事件不注入主聊天
      const sessionId = event.payload?.sessionId ?? session.activeSessionId;
      if (!sessionId) return;
      const { content } = event.payload;
      if (!content) return;
      const view = session.getSessionView(sessionId);
      view.thinkingBuffer += content;
      appendAgentThinking(view, content, payloadLoop(event.payload));
      view.streamActive = true;
      view.hydrated = true;
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
      view.hydrated = true;
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
      view.hydrated = true;
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
      view.hydrated = true;
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
      view.hydrated = true;
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
      if (step.type === "waiting_hint" || step.type === "retry") {
        if (step.content) {
          view.currentTurn.notice = step.content;
        }
        view.streamActive = true;
        view.hydrated = true;
        syncActiveSessionView(sessionId, true);
        return;
      }
      const fullStep = { ...step, timestamp: Date.now() } as AgentStep;
      view.agentSteps.push(fullStep);
      applyAgentStepToCurrentTurn(view, fullStep);
      view.hydrated = true;
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
          agent.subAgentRuns = { ...agent.subAgentRuns, ...pendingSubAgentRuns };
          pendingSubAgentRuns = {};
          subAgentFlushTimer = null;
        }, 2000);
      }
    });

    // subagent event
    await on<SubAgentEvent>("subagent-event", (event) => {
      const item = event.payload;
      if (!item?.runId) return;
      const events = [...(agent.subAgentEventsByRun[item.runId] ?? []), item]
        .sort((a, b) => a.timestamp - b.timestamp)
        .slice(-300);
      agent.subAgentEventsByRun = { ...agent.subAgentEventsByRun, [item.runId]: events };
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
      view.hydrated = true;
      // 会话仍在运行时不刷新消息——checkpoint 的部分回复会和 live currentTurn 重叠
      // ERROR 状态不刷新——错误消息仅在客户端 messages 中，刷新会冲掉
      if (view.status !== 'RUNNING' && view.status !== 'ERROR') {
        await refreshSessionHistory(sessionId);
      }
      syncActiveSessionView(sessionId, false);
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
        agent.subAgentEventsByRun = Object.fromEntries(
          Object.entries(agent.subAgentEventsByRun).filter(([, events]) => {
            return events.some((item) => item.sessionId !== deletedSessionId);
          })
        );
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

          if (!session.hasHydratedSessionView(nextActiveSessionId)) {
            try {
              const messages = await invoke<any[]>("get_session_messages", { sessionId: nextActiveSessionId });
              session.replaceSessionMessages(nextActiveSessionId, messages);
            } catch {
              const history = await invoke<string>("get_session_history", { sessionId: nextActiveSessionId });
              session.replaceSessionHistory(nextActiveSessionId, history);
            }
          }
          await Promise.all([
            loadSubAgentRunsFromBackend(nextActiveSessionId),
            loadSubAgentEventsFromBackend(nextActiveSessionId),
            loadPlanDocumentsFromBackend(nextActiveSessionId),
            loadAgentRunsFromBackend(nextActiveSessionId, { refreshHistory: false }),
            loadAgentRunEventsFromBackend(nextActiveSessionId),
            loadContextSnapshotFromBackend(nextActiveSessionId),
          ]);
        } else {
          session.workingDirectory = null;
          session.setSessionUsageTotals(null, {});
          session.resetSessionView(null, session.READY_TEXT);
        }
      } catch (err) {
        console.error("同步清理后的会话失败:", err);
        session.workingDirectory = null;
        session.setSessionUsageTotals(null, {});
        if (nextActiveSessionId) {
          session.resetSessionView(nextActiveSessionId, session.READY_TEXT);
        } else {
          session.resetSessionView(null, session.READY_TEXT);
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
    loadSubAgentEventsFromBackend,
    loadPlanDocumentsFromBackend,
    loadTodosFromBackend,
    loadAgentRunsFromBackend,
    loadAgentRunEventsFromBackend,
    loadContextSnapshotFromBackend,
  };
}
