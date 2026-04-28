import { listen } from "@tauri-apps/api/event";
import { invoke } from "@tauri-apps/api/core";
import { useSessionStore } from "../stores/session";
import { useChatStore } from "../stores/chat";
import { useAgentStore } from "../stores/agent";
import { usePermissionStore } from "../stores/permission";
import type {
  TodoItem,
  PermissionRequest,
  PlanProposal,
  PlanDocument,
  AgentStep,
  AgentRun,
  AgentRunEvent,
  SubAgentRun,
  SubAgentEvent,
} from "../types";

interface SessionCleanupPayload {
  deletedSessionId?: string | null;
  activeSessionId?: string | null;
}

// 模块级标志——确保事件监听器在整个应用生命周期内只注册一次
let globalListenersInitialized = false;
// 跟踪最近收到的内容 hash，防止重复追加相同内容
let lastContentHash = "";
let lastThinkingHash = "";

export function useAgentEvents() {
  const session = useSessionStore();
  const chat = useChatStore();
  const agent = useAgentStore();
  const perm = usePermissionStore();

  function syncActiveSessionView(sessionId: string | null | undefined, scroll = false) {
    if (sessionId === session.activeSessionId) {
      chat.triggerRender();
      if (scroll) chat.forceScrollToBottom();
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

  async function loadAgentRunsFromBackend(sid?: string | null) {
    try {
      const effectiveSid = sid ?? session.activeSessionId;
      if (!effectiveSid) return;
      const runs = await invoke<AgentRun[]>("list_agent_runs", { sessionId: effectiveSid });
      const otherRuns = Object.fromEntries(
        Object.entries(agent.agentRuns).filter(([, run]) => run.sessionId !== effectiveSid)
      );
      agent.agentRuns = {
        ...otherRuns,
        ...Object.fromEntries(runs.map((run) => [run.runId, run])),
      };
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

  const initListeners = async () => {
    if (globalListenersInitialized) {
      console.warn("[JarvisAgent] 事件监听器已全局注册，跳过重复注册");
      return;
    }
    globalListenersInitialized = true;

    // todos
    await listen<TodoItem[]>("todo-update", (event) => {
      agent.todos = event.payload;
      chat.triggerRender();
    });

    // permission
    await listen<PermissionRequest>("permission-request", (event) => {
      const sid = event.payload.sessionId ?? session.activeSessionId;
      if (sid) {
        perm.permissionRequests[sid] = event.payload;
      }
    });

    // plan proposal
    await listen<PlanProposal>("plan-proposal", (event) => {
      const sid = event.payload.sessionId ?? session.activeSessionId;
      if (sid) {
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
        if (sid === session.activeSessionId) {
          agent.showAgentPanel = true;
        }
      }
    });

    // plan document updated
    await listen<PlanDocument>("plan-document-updated", (event) => {
      perm.upsertPlanDocument(event.payload);
    });

    // agent run
    await listen<AgentRun>("agent-run-updated", (event) => {
      agent.upsertAgentRun(event.payload);
    });

    // agent run event
    await listen<AgentRunEvent>("agent-run-event", (event) => {
      const item = event.payload;
      if (!item?.runId) return;
      const events = [...(agent.agentRunEventsByRun[item.runId] ?? []), item]
        .sort((a, b) => a.timestamp - b.timestamp)
        .slice(-500);
      agent.agentRunEventsByRun = { ...agent.agentRunEventsByRun, [item.runId]: events };
    });

    // chat turn start
    await listen<any>("chat-turn-start", (event) => {
      const sessionId = event.payload?.sessionId ?? session.activeSessionId;
      if (!sessionId) return;
      chat.resetRenderState();
      const view = session.getSessionView(sessionId);
      const thought = view.thinkingBuffer.trim();
      if (thought) {
        view.toolBuffer += `${thought}\n\n`;
        const summary = thought.length > 100 ? thought.substring(0, 100) + "..." : thought;
        view.toolBuffer += `> 思考: ${summary}\n\n`;
        view.agentSteps.push({ type: "thinking", content: summary, timestamp: Date.now() });
      }
      view.tempBuffer = "";
      view.thinkingBuffer = "";
      view.hydrated = true;
      syncActiveSessionView(sessionId);
    });

    // chat content — 带去重防护
    await listen<any>("chat-content", (event) => {
      const sessionId = event.payload?.sessionId ?? session.activeSessionId;
      if (!sessionId) return;
      const { content } = event.payload;
      if (!content) return;
      // 去重：相同内容不重复追加（防止监听器叠加或后端重复事件）
      const contentKey = `${sessionId}:${content}`;
      if (contentKey === lastContentHash) return;
      lastContentHash = contentKey;
      const view = session.getSessionView(sessionId);
      view.tempBuffer += content;
      view.hydrated = true;
      syncActiveSessionView(sessionId, true);
    });

    // chat thinking — 带去重防护
    await listen<any>("chat-thinking", (event) => {
      const sessionId = event.payload?.sessionId ?? session.activeSessionId;
      if (!sessionId) return;
      const { content } = event.payload;
      if (!content) return;
      // 去重：相同内容不重复追加
      const contentKey = `${sessionId}:${content}`;
      if (contentKey === lastThinkingHash) return;
      lastThinkingHash = contentKey;
      const view = session.getSessionView(sessionId);
      view.thinkingBuffer += content;
      view.hydrated = true;
      syncActiveSessionView(sessionId, true);
    });

    // chat tool start
    await listen<any>("chat-tool-start", (event) => {
      const sessionId = event.payload?.sessionId ?? session.activeSessionId;
      if (!sessionId) return;
      const view = session.getSessionView(sessionId);
      const thought = view.thinkingBuffer.trim();
      if (thought) {
        view.toolBuffer += `${thought}\n\n`;
        const summary = thought.length > 100 ? thought.substring(0, 100) + "..." : thought;
        view.toolBuffer += `> 思考与计划: ${summary}\n\n`;
        view.agentSteps.push({ type: "thinking", content: summary, timestamp: Date.now() });
      }
      view.thinkingBuffer = "";
      view.tempBuffer = "";
      view.hydrated = true;
      syncActiveSessionView(sessionId);
    });

    // chat tool debug
    await listen<any>("chat-tool-debug", (event) => {
      const sessionId = event.payload?.sessionId ?? session.activeSessionId;
      if (!sessionId) return;
      const view = session.getSessionView(sessionId);
      const { content, kind, toolCallId, tool, status } = event.payload;
      if (kind === "tool_status" && toolCallId && tool && status) {
        chat.upsertToolStatusLine(view, String(toolCallId), String(tool), String(status));
      } else if (content) {
        view.toolBuffer += content;
      }
      view.hydrated = true;
      syncActiveSessionView(sessionId, true);
    });

    // chat stream
    await listen<any>("chat-stream", (event) => {
      const sessionId = event.payload?.sessionId ?? session.activeSessionId;
      if (!sessionId) return;
      const { content } = event.payload;
      const view = session.getSessionView(sessionId);
      view.toolBuffer += content;
      view.hydrated = true;
      syncActiveSessionView(sessionId, true);
    });

    // chat turn end
    await listen<any>("chat-turn-end", (event) => {
      const sessionId = event.payload?.sessionId ?? session.activeSessionId;
      if (!sessionId) return;
      const { has_tool } = event.payload;
      const view = session.getSessionView(sessionId);
      const thought = view.thinkingBuffer.trim();
      if (thought) {
        view.toolBuffer += `${thought}\n\n`;
        const summary = thought.length > 100 ? thought.substring(0, 100) + "..." : thought;
        view.toolBuffer += has_tool ? `> 继续计划: ${summary}\n\n` : `> 思考摘要: ${summary}\n\n`;
        view.agentSteps.push({ type: has_tool ? "plan" : "thinking", content: summary, timestamp: Date.now() });
      }
      if (!has_tool) {
        view.contentBuffer += view.tempBuffer;
      }
      view.thinkingBuffer = "";
      view.tempBuffer = "";
      view.hydrated = true;
      syncActiveSessionView(sessionId, true);
    });

    // agent step
    await listen<any>("agent-step", (event) => {
      const sessionId = event.payload?.sessionId ?? session.activeSessionId;
      if (!sessionId) return;
      const step = event.payload as Omit<AgentStep, "timestamp">;
      const view = session.getSessionView(sessionId);
      view.agentSteps.push({ ...step, timestamp: Date.now() });
      view.hydrated = true;
      if (sessionId === session.activeSessionId) {
        agent.showAgentPanel = true;
      }
    });

    // subagent updated
    await listen<SubAgentRun>("subagent-updated", (event) => {
      const run = event.payload;
      if (!run?.runId) return;
      agent.subAgentRuns = { ...agent.subAgentRuns, [run.runId]: run };
      if (run.sessionId === session.activeSessionId) {
        agent.showAgentPanel = true;
      }
    });

    // subagent event
    await listen<SubAgentEvent>("subagent-event", (event) => {
      const item = event.payload;
      if (!item?.runId) return;
      const events = [...(agent.subAgentEventsByRun[item.runId] ?? []), item]
        .sort((a, b) => a.timestamp - b.timestamp)
        .slice(-300);
      agent.subAgentEventsByRun = { ...agent.subAgentEventsByRun, [item.runId]: events };
    });

    // checkpoint created
    await listen<any>("checkpoint-created", (event) => {
      const sessionId = event.payload?.sessionId ?? session.activeSessionId;
      if (!sessionId) return;
      if (event.payload?.checkpointId) {
        const view = session.getSessionView(sessionId);
        view.latestCheckpoint = {
          id: event.payload.checkpointId,
          hasOperations: event.payload.hasOperations === true,
        };
        view.hydrated = true;
      }
    });

    // active session changed
    await listen<SessionCleanupPayload>("active-session-changed", async (event) => {
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
      }

      try {
        if (nextActiveSessionId) {
          const meta = await invoke<any>("get_session_meta", { id: nextActiveSessionId });
          session.workingDirectory = meta.workingDirectory || null;
          session.setSessionUsageTotals(meta.totalInputTokens || 0, meta.totalOutputTokens || 0);

          if (!session.hasHydratedSessionView(nextActiveSessionId)) {
            const history = await invoke<string>("get_session_history", { sessionId: nextActiveSessionId });
            session.replaceSessionHistory(nextActiveSessionId, history);
            await chat.loadAgentStepsFromBackend(nextActiveSessionId);
          }
          await loadSubAgentRunsFromBackend(nextActiveSessionId);
          await loadSubAgentEventsFromBackend(nextActiveSessionId);
          await loadPlanDocumentsFromBackend(nextActiveSessionId);
          await loadAgentRunsFromBackend(nextActiveSessionId);
          await loadAgentRunEventsFromBackend(nextActiveSessionId);
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
  };

  return {
    initListeners,
    loadSubAgentRunsFromBackend,
    loadSubAgentEventsFromBackend,
    loadPlanDocumentsFromBackend,
    loadAgentRunsFromBackend,
    loadAgentRunEventsFromBackend,
  };
}
