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
  PermissionRequest,
  PlanProposal,
  PlanDocument,
  AgentStep,
  AgentRun,
  AgentRunEvent,
  SessionContextSnapshot,
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

  function hydrateCurrentTurnFromRun(view: SessionViewState, run: AgentRun) {
    resetAgentCurrentTurn(view);
    view.currentTurn.startedAt = run.startedAt || Date.now();
    beginAgentLoop(view, run.loopCount || 1);
    if (run.liveThinking) {
      appendAgentThinking(view, run.liveThinking, run.loopCount || 1);
    }
    if (run.liveToolBuffer) {
      appendAgentExecutionLog(view, run.liveToolBuffer, run.loopCount || 1);
    }
    if (run.liveContent) {
      appendAgentText(view, run.liveContent, "assistant", run.loopCount || 1);
    }
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
      const previousActiveRunId = view.activeRunId;
      if (view.resumableRunId) {
        dismissedInterruptedRuns.add(view.resumableRunId);
      }
      view.status = "RUNNING";
      view.activeRunId = running.runId;
      view.resumableRunId = null;
      view.runStartTime = running.startedAt || Date.now();
      view.streamActive = true;
      view.cancelHandled = false;
      if (!hasCurrentTurnContent(view) || previousActiveRunId !== running.runId) {
        hydrateCurrentTurnFromRun(view, running);
      }
      view.hydrated = true;
      return;
    }

    view.activeRunId = null;
    view.streamActive = false;
    view.runStartTime = null;

    if (interrupted) {
      view.resumableRunId = interrupted.runId;
      if (view.status === "RUNNING" || view.status === "IDLE" || view.status === "INTERRUPTED") {
        view.status = "INTERRUPTED";
      }
      if (!hasCurrentTurnContent(view)) {
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
    view.hydrated = true;
  }

  async function refreshSessionHistory(sessionId: string) {
    const history = await invoke<string>("get_session_history", { sessionId });
    session.replaceSessionHistory(sessionId, history);
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

  async function loadAgentRunsFromBackend(sid?: string | null, options: { refreshHistory?: boolean } = {}) {
    try {
      const effectiveSid = sid ?? session.activeSessionId;
      if (!effectiveSid) return;
      const beforeHistory = session.getSessionView(effectiveSid).jarvisResponse;
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
        if (session.getSessionView(effectiveSid).jarvisResponse !== beforeHistory) {
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
      const events = await invoke<AgentRunEvent[]>("list_agent_run_events", { sessionId: effectiveSid, runId: null });
      const grouped = events.reduce<Record<string, AgentRunEvent[]>>((acc, item) => {
        if (!acc[item.runId]) acc[item.runId] = [];
        acc[item.runId].push(item);
        return acc;
      }, {});
      for (const runEvents of Object.values(grouped)) {
        runEvents.sort((a, b) => a.timestamp - b.timestamp);
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

    // todos
    await on<TodoItem[]>("todo-update", (event) => {
      agent.todos = event.payload;
      chat.triggerRender();
    });

    // permission
    await on<PermissionRequest>("permission-request", (event) => {
      const sid = event.payload.sessionId ?? session.activeSessionId;
      if (sid) {
        perm.permissionRequests[sid] = event.payload;
      }
    });

    // plan proposal
    await on<PlanProposal>("plan-proposal", (event) => {
      const sid = event.payload.sessionId ?? session.activeSessionId;
      if (sid) {
        const view = session.getSessionView(sid);
        hideSubmittedPlanFromChat(view, event.payload);
        perm.planProposals[sid] = event.payload;
        perm.upsertPlanDocument(
          {
            id: event.payload.id,
            sessionId: sid,
            title: event.payload.title,
            content: event.payload.content,
            status: "pending",
            path: null,
            createdAt: Date.now() / 1000,
            updatedAt: Date.now() / 1000,
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

    // agent run event
    await on<AgentRunEvent>("agent-run-event", (event) => {
      const item = event.payload;
      if (!item?.runId) return;
      const events = [...(agent.agentRunEventsByRun[item.runId] ?? []), item]
        .sort((a, b) => a.timestamp - b.timestamp)
        .slice(-500);
      agent.agentRunEventsByRun = { ...agent.agentRunEventsByRun, [item.runId]: events };
    });

    // context snapshot
    await on<SessionContextSnapshot>("context-snapshot-updated", (event) => {
      const snapshot = event.payload;
      if (!snapshot?.sessionId) return;
      agent.upsertContextSnapshot(snapshot);
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
        chat.upsertToolStatusLine(view, String(toolCallId), String(tool), String(status));
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
      const fullStep = { ...step, timestamp: Date.now() } as AgentStep;
      view.agentSteps.push(fullStep);
      applyAgentStepToCurrentTurn(view, fullStep);
      view.hydrated = true;
    });

    // subagent updated
    await on<SubAgentRun>("subagent-updated", (event) => {
      const run = event.payload;
      if (!run?.runId) return;
      agent.subAgentRuns = { ...agent.subAgentRuns, [run.runId]: run };
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
        // 有实快照的轮次
        view.latestCheckpoint = {
          id: event.payload.checkpointId,
          hasOperations: event.payload.hasOperations === true,
          hasPatches: event.payload.hasPatches === true,
          canRollback: true,
        };
      } else if (event.payload?.canRollback) {
        // 纯聊天轮次：没有 checkpointId，但可以回滚消息
        view.latestCheckpoint = {
          id: "",
          hasOperations: false,
          hasPatches: false,
          canRollback: true,
        };
      }
      view.hydrated = true;
      await refreshSessionHistory(sessionId);
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
        delete perm.permissionRequests[deletedSessionId];
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
          session.setSessionUsageTotals(meta.totalInputTokens || 0, meta.totalOutputTokens || 0);

          if (!session.hasHydratedSessionView(nextActiveSessionId)) {
            const history = await invoke<string>("get_session_history", { sessionId: nextActiveSessionId });
            session.replaceSessionHistory(nextActiveSessionId, history);
          }
          await Promise.all([
            chat.loadAgentStepsFromBackend(nextActiveSessionId),
            loadSubAgentRunsFromBackend(nextActiveSessionId),
            loadSubAgentEventsFromBackend(nextActiveSessionId),
            loadPlanDocumentsFromBackend(nextActiveSessionId),
            loadAgentRunsFromBackend(nextActiveSessionId, { refreshHistory: false }),
            loadAgentRunEventsFromBackend(nextActiveSessionId),
            loadContextSnapshotFromBackend(nextActiveSessionId),
          ]);
        } else {
          session.workingDirectory = null;
          session.setSessionUsageTotals(0, 0);
          session.resetSessionView(null, session.READY_TEXT);
        }
      } catch (err) {
        console.error("同步清理后的会话失败:", err);
        session.workingDirectory = null;
        session.setSessionUsageTotals(0, 0);
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
    loadAgentRunsFromBackend,
    loadAgentRunEventsFromBackend,
    loadContextSnapshotFromBackend,
  };
}
