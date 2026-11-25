/**
 * # streamEvents.ts — 对话流式输出与 Agent 执行步骤事件域
 *
 * 注册并处理主对话轮次流式事件（`chat-turn-start` / `chat-content` / `chat-thinking` /
 * `chat-tool-start` / `chat-tool-debug` / `chat-stream` / `chat-turn-end`）及 `agent-step`
 * 过程性状态标注（等待提示、重试进度、按来源清除、反思审查）。
 *
 * ## Key Exports
 * - `registerStreamEvents()`: 注册流式对话与步骤域全部事件监听
 *
 * ## Dependencies
 * - Internal: `./types`, `@/utils/agentTurnState`, `@/types`
 *
 * ## Constraints
 * - 子代理事件（`isSubAgent: true`）严禁注入主聊天缓冲区
 * - 过程性 `notice` 写入时必须携带 `noticeKind`，供 `notice_clear` 精确清除
 */
import type { AgentStep } from "../../types";
import {
  appendAgentExecutionLog,
  appendAgentText,
  appendAgentThinking,
  applyAgentStepToCurrentTurn,
  beginAgentLoop,
  finishAgentLoop,
  markAgentToolActivity,
  upsertAgentToolCall,
} from "../../utils/agentTurnState";
import type { AgentEventsContext, EventRegisterFn } from "./types";

function payloadLoop(payload: any): number | null {
  const raw = payload?.loopCount ?? payload?.loop_count ?? payload?.loop;
  const value = Number(raw);
  return Number.isFinite(value) && value > 0 ? value : null;
}

function commitTempBuffer(view: { contentBuffer: string; tempBuffer: string }): void {
  if (!view.tempBuffer) return;
  view.contentBuffer += view.tempBuffer;
  view.tempBuffer = "";
}

function commitThinkingBuffer(
  view: { thinkingBuffer: string; toolBuffer: string; agentSteps: AgentStep[] },
  type: AgentStep["type"] = "thinking",
): void {
  const thought = view.thinkingBuffer.trim();
  if (!thought) return;
  view.toolBuffer += `${thought}\n\n`;
  view.agentSteps.push({ type, content: thought, timestamp: Date.now() });
}

export async function registerStreamEvents(
  on: EventRegisterFn,
  ctx: AgentEventsContext,
): Promise<void> {
  const { t, session, dismissedInterruptedRuns, syncActiveSessionView } = ctx;

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
    // 概览栏的「步」= 本轮 agent 跑到了第几次 loop 迭代：这里记下实时值
    if (startedLoop) session.noteLoopStep(sessionId, startedLoop);
    view.streamActive = true;
    session.markSessionReady(sessionId);
    syncActiveSessionView(sessionId, true);
  });

  // chat content — 流式片段必须逐段追加，不能按文本内容去重
  await on<any>("chat-content", (event) => {
    const sessionId = event.payload?.sessionId ?? session.activeSessionId;
    if (!sessionId) return;
    const { content } = event.payload;
    if (!content) return;
    // 主 Agent 正文片段 = 本次请求生成的开始（子代理的内容不算）
    if (!event.payload?.isSubAgent) session.markRequestFirstDelta(sessionId, Date.now());
    const view = session.getSessionView(sessionId);
    view.tempBuffer += content;
    appendAgentText(view, content, "assistant", payloadLoop(event.payload));
    view.streamActive = true;
    session.markSessionReady(sessionId);
    syncActiveSessionView(sessionId, true);
  });

  // chat thinking — 同样保留所有片段，避免重复词或重复标点被误删
  await on<any>("chat-thinking", (event) => {
    if (event.payload?.isSubAgent) return; // 子代理事件不注入主聊天
    const sessionId = event.payload?.sessionId ?? session.activeSessionId;
    if (!sessionId) return;
    const { content } = event.payload;
    if (!content) return;
    // 思考片段同样是本次请求的输出，也要算吞吐起点
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
      upsertAgentToolCall(
        view,
        String(toolCallId),
        String(tool),
        String(status),
        payloadLoop(event.payload),
      );
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

    // 过程性状态标注：写入时必须带 `noticeKind`，供 `notice_clear` 按来源精确清除
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
    // 过程态标注的清除信号：只清同来源的标注，不误伤仍在有效的反思提示
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
    // 反思审查判定：只报 not_ok（需要改进），判 ok 静默不打扰
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
}
