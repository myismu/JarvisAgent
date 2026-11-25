/**
 * # runState.ts — Agent 运行态同步与用户消息 ID 回填逻辑
 *
 * 将后端下发或拉取的 `AgentRun` 列表同步为前端 `SessionViewState` 的运行态
 * （RUNNING / INTERRUPTED / IDLE），并在开跑瞬间把真实用户消息 UUID 回填到本地占位消息。
 *
 * ## Key Exports
 * - `applyAgentRunState()`: 同步会话运行态（不推进消息窗口 `hydration`）
 * - `backfillRunningUserMessageId()`: 将 running 态 run 的 `messageId` 回填到尾部占位用户消息
 * - `hydrateCurrentTurnFromRun()`: 用 run 元数据重建当前 turn 的运行态标记
 *
 * ## Dependencies
 * - Internal: `@/stores/session`, `@/stores/permission`, `@/utils/agentTurnState`, `@/types`
 *
 * ## Constraints
 * - `applyAgentRunState` 绝不调用 `markSessionReady`，run 状态与消息窗口加载态相互正交
 */
import type { SessionViewState, useSessionStore } from "../../stores/session";
import type { usePermissionStore } from "../../stores/permission";
import type { AgentRun } from "../../types";
import { beginAgentLoop, resetAgentCurrentTurn } from "../../utils/agentTurnState";

export function hasCurrentTurnContent(view: SessionViewState): boolean {
  const turn = view.currentTurn;
  return Boolean(
    turn.textBlocks.some((block) => block.content.trim()) ||
      turn.thinkingBlocks.some((block) => block.content.trim()) ||
      turn.toolCalls.length > 0 ||
      turn.logs.some((log) => log.content.trim()),
  );
}

export function latestRun(
  runs: AgentRun[],
  predicate: (run: AgentRun) => boolean,
): AgentRun | null {
  return (
    runs
      .filter(predicate)
      .sort((a, b) => (b.startedAt || 0) - (a.startedAt || 0))[0] ?? null
  );
}

/**
 * 用 run 记录重建「当前 turn」的展示态。
 *
 * v15 起 `AgentRun` 不再携带 `live_content / live_thinking / live_tool_buffer`
 * （改由「每轮一行」的 `agent_run_events` 承载）。所以这里不再灌正文/思考，
 * 只恢复运行态标记（开始时间、轮次、是否进行中）。
 */
export function hydrateCurrentTurnFromRun(view: SessionViewState, run: AgentRun): void {
  resetAgentCurrentTurn(view);
  view.currentTurn.startedAt = run.startedAt || Date.now();
  beginAgentLoop(view, run.loopCount || 1);
  view.currentTurn.isRunning = run.status === "running";
  view.currentTurn.revision += 1;
}

/**
 * 把后端 run 记录同步成视图的**运行态**（RUNNING / INTERRUPTED / 收口）。
 *
 * 本函数**不推进 `hydration`**（会话消息窗口的生命周期状态）：
 * run 状态与消息窗口是两个正交的事实源。只有真正把消息写进视图的路径
 * （`replaceSessionMessages` / 事件补齐）才有资格置 `ready`。
 */
export function applyAgentRunState(
  session: ReturnType<typeof useSessionStore>,
  perm: ReturnType<typeof usePermissionStore>,
  dismissedInterruptedRuns: Set<string>,
  sessionId: string,
  runs: AgentRun[],
): void {
  const view = session.getSessionView(sessionId);
  const running = latestRun(
    runs,
    (run) => run.sessionId === sessionId && run.status === "running",
  );
  const interrupted = latestRun(
    runs,
    (run) =>
      run.sessionId === sessionId &&
      run.status === "interrupted" &&
      run.resumable &&
      !dismissedInterruptedRuns.has(run.runId),
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
    // 只在 runId 真的切了且会话尚无持久化消息时才重建（避免与历史消息重叠成两份）
    if (prevRunId && prevRunId !== running.runId && view.messages.length === 0) {
      hydrateCurrentTurnFromRun(view, running);
    }
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
    // 仅当会话尚无已持久化消息时，才用 run 重建当前 turn，防止刷新后重复渲染
    const hasPersistedHistory = view.messages.length > 0;
    if (!hasPersistedHistory && !hasCurrentTurnContent(view)) {
      hydrateCurrentTurnFromRun(view, interrupted);
    }
    view.currentTurn.isRunning = false;
    return;
  }

  view.resumableRunId = null;
  if (view.status === "RUNNING" || view.status === "INTERRUPTED") {
    view.status = "IDLE";
  }
  view.currentTurn.isRunning = false;
  // run 正常收口：清理未走完正式提交流程的流式态方案预览
  perm.clearStreamingPlanProposal(sessionId);
}

/**
 * 把本轮用户消息的数据库 id 回填到本地占位消息。
 *
 * run 记录在开跑时就通过 `agent-run-updated` 带出 `messageId`，比等 `ask_jarvis`
 * 返回更早，且在异常路径上也能到达，确保运行期间和失败后撤回按钮都可用。
 */
export function backfillRunningUserMessageId(
  session: ReturnType<typeof useSessionStore>,
  run: AgentRun,
): void {
  if (run.status !== "running" || !run.messageId) return;
  const view = session.getSessionView(run.sessionId);
  if (view.status !== "RUNNING" || !view.runStartTime) return;
  if ((run.startedAt || 0) < view.runStartTime) return;
  // 只认列表尾部那条占位用户消息（`pending_*`），续跑不追加用户消息须放过
  const last = view.messages[view.messages.length - 1];
  if (!last || last.role !== "user" || last.messageId) return;
  if (!String(last.id).startsWith("pending_")) return;
  last.messageId = run.messageId;
}
