/**
 * # lifecycleEvents.ts — 运行态、用量、子 Agent 与会话缓存失效事件域
 *
 * 注册并处理 Agent 运行态（`agent-run-updated`）、上下文与用量指标
 * （`context-snapshot-updated` / `session-usage-updated`）、子 Agent 节流更新
 * （`subagent-updated` / `subagent-trace-updated`）、检查点与消息缓存失效
 * （`checkpoint-created` / `session-messages-changed` / `active-session-changed`）
 * 以及辅助通知（`todo-update` / `memory-updated` / `background-failed`）。
 *
 * ## Key Exports
 * - `registerLifecycleEvents()`: 注册运行态、指标、通知与会话生命周期事件，返回定时器清理函数
 *
 * ## Dependencies
 * - Internal: `./types`, `@/types`
 * - External: `@tauri-apps/api/core`
 *
 * ## Constraints
 * - 非活跃会话收到 `session-messages-changed` 时只标脏不发 IPC，防止后台会话拖垮前台
 * - `subagent-updated` 采用 2 秒节流批量合并，减轻多子 Agent 心跳渲染压力
 * - 独立监控窗口应传入 `{ syncChatMessages: false }`，避免重复拉取聊天消息历史
 */
import { getSessionMeta } from "../../api";
import type { SubAgentRun, SubAgentTraceRunSummary } from "../../types";
import type {
  AgentEventsContext,
  EventRegisterFn,
  LifecycleEventsOptions,
} from "./types";

export async function registerLifecycleEvents(
  on: EventRegisterFn,
  ctx: AgentEventsContext,
  options: LifecycleEventsOptions = {},
): Promise<() => void> {
  const {
    session,
    chat,
    agent,
    perm,
    syncActiveSessionView,
    applyAgentRunState,
    backfillRunningUserMessageId,
    loaders,
  } = ctx;
  const syncChatMessages = options.syncChatMessages ?? true;

  // 统一追踪域内创建的定时器与销毁标记，HMR 或窗口卸载时彻底清理，防止悬挂回调
  let disposed = false;
  let memoryNoticeTimer: ReturnType<typeof setTimeout> | null = null;
  let bgFailedTimer: ReturnType<typeof setTimeout> | null = null;
  let pendingSubAgentRuns: Record<string, SubAgentRun> = {};
  let subAgentFlushTimer: ReturnType<typeof setTimeout> | null = null;
  const pendingMessagesChanged = new Map<
    string,
    { reason: string; count: number; lastSeq: number | null }
  >();

  // todos（按会话隔离，按组存储）
  await on("todo-update", (event) => {
    const { todos, groups, sessionId } = event.payload;
    if (sessionId) {
      // 后端同时给扁平数组和分组；缺 groups 时退化成一个组（兼容旧事件）
      const nextGroups =
        groups ?? (todos.length > 0 ? [{ id: sessionId, items: todos }] : []);
      agent.todoGroupsBySession = { ...agent.todoGroupsBySession, [sessionId]: nextGroups };
    }
    if (syncChatMessages) {
      chat.triggerRender();
    }
  });

  if (syncChatMessages) {
    // memory update notice (transient, not persisted)
    await on("memory-updated", (event) => {
      chat.memoryNotice = event.payload.summary;
      if (memoryNoticeTimer !== null) {
        clearTimeout(memoryNoticeTimer);
      }
      // auto-dismiss after 30 seconds
      memoryNoticeTimer = setTimeout(() => {
        memoryNoticeTimer = null;
        if (!disposed && chat.memoryNotice === event.payload.summary) {
          chat.memoryNotice = null;
        }
      }, 30000);
    });

    // 后台任务失败提醒（transient, not persisted）
    await on("background-failed", (event) => {
      const p = event.payload;
      if (!p) return;
      const sid = p.sessionId ?? session.activeSessionId;
      // 只提醒当前活跃会话的任务；非活跃会话的任务在监控面板可见
      if (!sid || sid !== session.activeSessionId) return;
      const cmd = (p.command || "").slice(0, 60);
      const text = `后台任务 ${p.taskId}（${cmd}）失败：${p.result}。可发消息让 Agent 继续检查。`;
      chat.memoryNotice = text;
      if (bgFailedTimer !== null) {
        clearTimeout(bgFailedTimer);
      }
      // 30 秒自动消失；startsWith 防止误清掉之后的其他提醒
      bgFailedTimer = setTimeout(() => {
        bgFailedTimer = null;
        if (!disposed && chat.memoryNotice?.startsWith(`后台任务 ${p.taskId}`)) {
          chat.memoryNotice = null;
        }
      }, 30000);
    });
  }

  // agent run
  await on("agent-run-updated", (event) => {
    const run = event.payload;
    agent.upsertAgentRun(run);
    // 用户消息 id 随 run 下发：一开跑就回填，撤回按钮不必等整轮结束或刷新
    backfillRunningUserMessageId(run);
    const runs = Object.values(agent.agentRuns).filter(
      (item) => item.sessionId === run.sessionId,
    );
    applyAgentRunState(run.sessionId, runs);
    if (syncChatMessages) {
      syncActiveSessionView(run.sessionId, false);
    }
  });

  // context snapshot
  await on("context-snapshot-updated", (event) => {
    const snapshot = event.payload;
    if (!snapshot?.sessionId) return;
    agent.upsertContextSnapshot(snapshot);
  });

  // 会话累计用量：载荷是绝对值，整值覆盖不累加
  await on("session-usage-updated", (event) => {
    const usage = event.payload;
    if (!usage?.sessionId) return;
    session.setSessionUsageTotals(usage.sessionId, {
      input: usage.inputTokens,
      output: usage.outputTokens,
      cacheHit: usage.cacheHitTokens,
      cacheMiss: usage.cacheMissTokens,
    });
    session.setSessionUsageBreakdown(usage.sessionId, usage.breakdown);
    // 结算本次请求的输出吞吐速度（tok/s）
    session.settleRequestThroughput(usage.sessionId, usage.outputTokens || 0, Date.now());
  });

  // subagent updated（2s 节流批量更新，减轻 5s 心跳 × N 子 Agent 的渲染压力）
  await on("subagent-updated", (event) => {
    const run = event.payload;
    if (!run?.runId || disposed) return;
    pendingSubAgentRuns[run.runId] = run;
    if (!subAgentFlushTimer) {
      subAgentFlushTimer = setTimeout(() => {
        subAgentFlushTimer = null;
        if (disposed) {
          pendingSubAgentRuns = {};
          return;
        }
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
          summariesBySession[updatedRun.sessionId] = nextRuns.sort(
            (a, b) => b.startedAt - a.startedAt,
          );
        }
        agent.subAgentTraceRunsBySession = summariesBySession;
        pendingSubAgentRuns = {};
      }, 2000);
    }
  });

  await on("subagent-trace-updated", (event) => {
    const runId = event.payload?.runId;
    if (runId && Object.prototype.hasOwnProperty.call(agent.subAgentTraceByRun, runId)) {
      void loaders.loadSubAgentTraceFromBackend(runId);
    }
  });

  if (syncChatMessages) {
    // checkpoint created
    await on("checkpoint-created", async (event) => {
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
      // 会话仍在运行时不刷新消息；ERROR 状态不刷新（错误消息仅在客户端 messages 中）
      if (view.status !== "RUNNING" && view.status !== "ERROR") {
        await loaders.refreshSessionHistory(sessionId, "checkpoint");
      }
      syncActiveSessionView(sessionId, false);
    });

    // 会话消息落库变更（事件驱动缓存失效 + 微任务级合并）
    let messagesChangedFlushScheduled = false;

    async function applySessionMessagesChanged(
      sessionId: string,
      batch: { reason: string; count: number; lastSeq: number | null },
    ) {
      if (disposed) return;
      const reason = `messages-changed:${batch.reason}`;
      const view = session.getSessionView(sessionId);
      console.debug(
        `[events] session-messages-changed: session=${sessionId} count=${batch.count} lastSeq=${batch.lastSeq} reason=${reason} status=${view.status} hydration=${view.hydration}`,
      );

      // 分支一：会话正在跑 → 只标脏，不重载（避免和 live currentTurn 重叠）
      if (view.status === "RUNNING") {
        chat.markSessionViewStale(sessionId, reason);
        return;
      }

      // 分支二：活跃会话且不在运行 → 立刻与后端对齐
      if (sessionId === session.activeSessionId) {
        if (view.hydration === "ready") {
          chat.markSessionViewStale(sessionId, reason);
        }
        try {
          await chat.ensureSessionViewLoaded(sessionId, reason);
        } catch (err) {
          console.error("加载会话消息失败:", err);
          session.setSessionLoadError(sessionId, true);
        }
        return;
      }

      // 分支三：非活跃会话 → 只标脏，不发 IPC
      chat.markSessionViewStale(sessionId, reason);
    }

    function scheduleMessagesChangedFlush() {
      if (messagesChangedFlushScheduled || disposed) return;
      messagesChangedFlushScheduled = true;
      queueMicrotask(() => {
        messagesChangedFlushScheduled = false;
        if (disposed || pendingMessagesChanged.size === 0) {
          pendingMessagesChanged.clear();
          return;
        }
        const batches = Array.from(pendingMessagesChanged.entries());
        pendingMessagesChanged.clear();
        for (const [sessionId, batch] of batches) {
          void applySessionMessagesChanged(sessionId, batch);
        }
      });
    }

    await on("session-messages-changed", (event) => {
      const p = event.payload;
      if (!p?.sessionId || disposed) return;
      const existing = pendingMessagesChanged.get(p.sessionId);
      pendingMessagesChanged.set(p.sessionId, {
        reason: p.reason || "unknown",
        count: p.count ?? existing?.count ?? 0,
        lastSeq: p.lastSeq ?? existing?.lastSeq ?? null,
      });
      scheduleMessagesChangedFlush();
    });
  }

  // active session changed
  await on("active-session-changed", async (event) => {
    const deletedSessionId = event.payload?.deletedSessionId ?? null;
    const nextActiveSessionId = event.payload?.activeSessionId ?? null;

    if (deletedSessionId) {
      session.deleteSessionView(deletedSessionId);
      chat.resetRenderState(deletedSessionId);
    }

    session.activeSessionId = nextActiveSessionId;
    if (deletedSessionId) {
      delete perm.planProposals[deletedSessionId];
      delete perm.planDocumentsBySession[deletedSessionId];
      perm.clearPermissions(deletedSessionId);
      agent.agentRuns = Object.fromEntries(
        Object.entries(agent.agentRuns).filter(([, run]) => run.sessionId !== deletedSessionId),
      );
      agent.agentRunEventsByRun = Object.fromEntries(
        Object.entries(agent.agentRunEventsByRun).filter(([, events]) => {
          return events.some((item) => item.sessionId !== deletedSessionId);
        }),
      );
      agent.subAgentRuns = Object.fromEntries(
        Object.entries(agent.subAgentRuns).filter(([, run]) => run.sessionId !== deletedSessionId),
      );
      const remainingTraceRuns = { ...agent.subAgentTraceRunsBySession };
      delete remainingTraceRuns[deletedSessionId];
      agent.subAgentTraceRunsBySession = remainingTraceRuns;
      agent.subAgentTraceByRun = Object.fromEntries(
        Object.entries(agent.subAgentTraceByRun).filter(([, events]) =>
          events.some((item) => item.sessionId !== deletedSessionId),
        ),
      );
      agent.clearContextSnapshot(deletedSessionId);
    }

    try {
      if (nextActiveSessionId) {
        const meta = await getSessionMeta(nextActiveSessionId);
        session.workingDirectory = meta.workingDirectory || null;
        session.setSessionUsageTotals(nextActiveSessionId, {
          input: meta.totalInputTokens || 0,
          output: meta.totalOutputTokens || 0,
          cacheHit: meta.totalCacheHitTokens || 0,
          cacheMiss: meta.totalCacheMissTokens || 0,
        });

        if (syncChatMessages) {
          try {
            await chat.ensureSessionViewLoaded(nextActiveSessionId, "session-fallback");
          } catch (err) {
            console.error("加载会话消息失败:", err);
            session.setSessionLoadError(nextActiveSessionId, true);
          }
        }
        await Promise.all([
          loaders.loadSubAgentRunsFromBackend(nextActiveSessionId),
          loaders.loadPlanDocumentsFromBackend(nextActiveSessionId),
          loaders.loadAgentRunsFromBackend(nextActiveSessionId, { refreshHistory: false }),
          loaders.loadAgentRunEventsFromBackend(nextActiveSessionId),
          loaders.loadContextSnapshotFromBackend(nextActiveSessionId),
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

    if (syncChatMessages) {
      chat.resetRenderState();
      chat.triggerRender();
      chat.forceScrollToBottom();
    }
  });

  // 返回清理回调：用于 HMR 替换或窗口卸载时清掉未触发的节流/自动关闭定时器
  return () => {
    disposed = true;
    if (subAgentFlushTimer !== null) {
      clearTimeout(subAgentFlushTimer);
      subAgentFlushTimer = null;
    }
    if (memoryNoticeTimer !== null) {
      clearTimeout(memoryNoticeTimer);
      memoryNoticeTimer = null;
    }
    if (bgFailedTimer !== null) {
      clearTimeout(bgFailedTimer);
      bgFailedTimer = null;
    }
    pendingSubAgentRuns = {};
    pendingMessagesChanged.clear();
  };
}
