/**
 * # loaders.ts — 会话运行态、轨迹与历史数据后端加载器
 *
 * 封装子 Agent 运行列表/逐轮轨迹、方案文档、待办清单、主 Agent 运行记录/事件、
 * 上下文快照以及会话历史的后端 IPC 拉取逻辑，带并发去重与竞态版本保护。
 *
 * ## Key Exports
 * - `createAgentEventsLoaders()`: 创建供事件监听器和页面组件共用的全部加载方法
 *
 * ## Dependencies
 * - Internal: `@/stores/session`, `@/stores/chat`, `@/stores/agent`, `@/stores/permission`, `@/types`
 * - External: `@tauri-apps/api/core`
 */
import {
  getSessionContextSnapshot,
  getSessionTodos,
  listAgentRunEvents,
  listAgentRuns,
  listPlanDocuments,
  listSubagents,
  listSubagentTraceEvents,
  listSubagentTraceRuns,
} from "../../api";
import type { useSessionStore } from "../../stores/session";
import type { useChatStore } from "../../stores/chat";
import type { useAgentStore } from "../../stores/agent";
import type { usePermissionStore } from "../../stores/permission";
import type { AgentRun, AgentRunLoopEvent } from "../../types";
import type { AgentEventsLoaders } from "./types";

interface LoaderDeps {
  session: ReturnType<typeof useSessionStore>;
  chat: ReturnType<typeof useChatStore>;
  agent: ReturnType<typeof useAgentStore>;
  perm: ReturnType<typeof usePermissionStore>;
  syncActiveSessionView: (sessionId: string | null | undefined, followScroll?: boolean) => void;
  applyAgentRunState: (sessionId: string, runs: AgentRun[]) => void;
}

export function createAgentEventsLoaders(deps: LoaderDeps): AgentEventsLoaders {
  const { session, chat, agent, perm, syncActiveSessionView, applyAgentRunState } = deps;
  // 轨迹按需加载版本号：防止旧请求后返回覆盖新请求结果
  const subAgentTraceLoadVersion = new Map<string, number>();
  // 同一会话的子 Agent 列表在途请求去重表
  const subAgentRunsLoads = new Map<string, Promise<void>>();

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
          listSubagents(effectiveSid),
          listSubagentTraceRuns(effectiveSid),
        ]);
        const otherRuns = Object.fromEntries(
          Object.entries(agent.subAgentRuns).filter(([, run]) => run.sessionId !== effectiveSid),
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
      const events = await listSubagentTraceEvents(runId);
      if (
        subAgentTraceLoadVersion.get(runId) === version &&
        Object.prototype.hasOwnProperty.call(agent.subAgentTraceByRun, runId)
      ) {
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
      const documents = await listPlanDocuments(effectiveSid);
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
      const groups = await getSessionTodos(effectiveSid);
      agent.todoGroupsBySession = {
        ...agent.todoGroupsBySession,
        [effectiveSid]: groups,
      };
    } catch (err) {
      console.error("加载 todos 失败:", err);
    }
  }

  async function loadAgentRunsFromBackend(
    sid?: string | null,
    options: { refreshHistory?: boolean } = {},
  ) {
    try {
      const effectiveSid = sid ?? session.activeSessionId;
      if (!effectiveSid) return;
      const beforeMsgCount = session.getSessionView(effectiveSid).messages.length;
      const runs = await listAgentRuns(effectiveSid);
      const otherRuns = Object.fromEntries(
        Object.entries(agent.agentRuns).filter(([, run]) => run.sessionId !== effectiveSid),
      );
      agent.agentRuns = {
        ...otherRuns,
        ...Object.fromEntries(runs.map((run) => [run.runId, run])),
      };
      applyAgentRunState(effectiveSid, runs);
      if (options.refreshHistory !== false) {
        await refreshSessionHistory(effectiveSid, "agent-runs");
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
      const events = await listAgentRunEvents(effectiveSid, null);
      const grouped = events.reduce<Record<string, AgentRunLoopEvent[]>>((acc, item) => {
        if (!acc[item.runId]) acc[item.runId] = [];
        acc[item.runId].push(item);
        return acc;
      }, {});
      // 排序口径：同 run 内按 loopIndex 升序（轮次顺序才是重建顺序）
      for (const runEvents of Object.values(grouped)) {
        runEvents.sort((a, b) => (a.loopIndex || 0) - (b.loopIndex || 0));
      }
      const otherEvents = Object.fromEntries(
        Object.entries(agent.agentRunEventsByRun).filter(([, runEvents]) => {
          return runEvents.length > 0 && runEvents[0].sessionId !== effectiveSid;
        }),
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
      const snapshot = await getSessionContextSnapshot(effectiveSid);
      if (snapshot) {
        agent.upsertContextSnapshot(snapshot);
      } else {
        agent.clearContextSnapshot(effectiveSid);
      }
    } catch (err) {
      console.error("加载上下文快照失败:", err);
    }
  }

  return {
    refreshSessionHistory,
    loadSubAgentRunsFromBackend,
    loadSubAgentTraceFromBackend,
    loadPlanDocumentsFromBackend,
    loadTodosFromBackend,
    loadAgentRunsFromBackend,
    loadAgentRunEventsFromBackend,
    loadContextSnapshotFromBackend,
  };
}
