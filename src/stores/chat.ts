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

export const useChatStore = defineStore("chat", () => {
  let throttlePending = false;
  let lastRenderTime = 0;
  let scrollToBottomCb: ((force?: boolean) => void) | null = null;
  let sendGeneration = 0;

  const rollbackRecalledMessage = ref("");

  const jarvisResponse = computed({
    get: () => {
      const session = useSessionStore();
      return session.currentSessionView.jarvisResponse;
    },
    set: (value: string) => {
      const session = useSessionStore();
      const view = session.getSessionView(session.activeSessionId);
      view.jarvisResponse = value;
      view.hydrated = true;
    },
  });

  const toolBuffer = computed({
    get: () => {
      const session = useSessionStore();
      return session.currentSessionView.toolBuffer;
    },
    set: (value: string) => {
      const session = useSessionStore();
      const view = session.getSessionView(session.activeSessionId);
      view.toolBuffer = value;
      view.hydrated = true;
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
      view.hydrated = true;
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
      view.hydrated = true;
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
      view.hydrated = true;
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
      view.hydrated = true;
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
      view.hydrated = true;
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
      view.hydrated = true;
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
      view.hydrated = true;
    },
  });

  function resetRenderState() {
    throttlePending = false;
    lastRenderTime = 0;
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

  function flushCurrentTurnRender() {
    renderTick.value++;
    throttlePending = false;
  }

  function triggerRender() {
    if (throttlePending) return;
    const now = performance.now();
    const elapsed = now - lastRenderTime;
    // 节流到 ~30fps，减少 markdown 解析次数
    const MIN_INTERVAL = 33;
    if (elapsed < MIN_INTERVAL) {
      throttlePending = true;
      setTimeout(() => {
        lastRenderTime = performance.now();
        flushCurrentTurnRender();
      }, MIN_INTERVAL - elapsed);
    } else {
      throttlePending = true;
      lastRenderTime = now;
      requestAnimationFrame(flushCurrentTurnRender);
    }
  }

  // 渲染 tick，用于触发流式滚动
  const renderTick = ref(0);

  async function resolvePermission(decision: string) {
    const perm = usePermissionStore();
    const session = useSessionStore();
    if (perm.permissionRequest) {
      const sid = perm.permissionRequests[session.activeSessionId!]?.sessionId ?? session.activeSessionId;
      await invoke("resolve_permission", {
        id: perm.permissionRequests[session.activeSessionId!].id,
        sessionId: sid,
        decision,
      });
      if (sid) {
        delete perm.permissionRequests[sid];
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
    return sendToJarvis(
      `用户已同意方案「${title}」。请按照上文中的方案内容立即开始执行。`,
      undefined,
      undefined,
      false,
      true, // skipRunningCheck — 方案审批续跑是新的用户轮次，不应取消前一轮
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

  async function ensureActiveSessionForSend() {
    const session = useSessionStore();
    if (session.activeSessionId) {
      return session.activeSessionId;
    }

    const meta = await invoke<any>("create_session", {
      workingDirectory: session.pendingWorkingDirectory,
    });
    session.activeSessionId = meta.id;
    session.workingDirectory = meta.workingDirectory || null;
    session.pendingWorkingDirectory = null;
    session.resetSessionView(meta.id);
    session.setSessionUsageTotals(meta.totalInputTokens || 0, meta.totalOutputTokens || 0);

    // 记录新建会话使用的模型，后续切回来时自动恢复
    const config = await invoke<any>("get_config");
    if (config.activeProfileId) {
      await invoke("update_session_profile", { id: meta.id, profileId: config.activeProfileId });
    }
    return meta.id as string;
  }

  async function sendToJarvis(msg: string, thinkingOverride?: boolean, imageBase64List?: string[], resumeOnly = false, skipRunningCheck = false) {
    const session = useSessionStore();

    if (!msg && (!imageBase64List || imageBase64List.length === 0)) return;

    if (!skipRunningCheck) {
      if (session.runningSessionId && session.runningSessionId === session.activeSessionId) {
        const runningView = session.getSessionView(session.runningSessionId);
        if (runningView.status === "RUNNING") {
          await cancelJarvis();
        } else {
          session.runningSessionId = null;
          runningView.runStartTime = null;
          runningView.streamActive = false;
          runningView.activeRunId = null;
        }
      }
    }

    const myGeneration = ++sendGeneration;

    const sessionIdAtStart = await ensureActiveSessionForSend();
    const requestView = session.getSessionView(sessionIdAtStart);

    try {
      const recovered = await invoke<boolean>("recover_interrupted_session_messages", {
        sessionId: sessionIdAtStart,
      });
      if (recovered) {
        try {
          const messages = await invoke<any[]>("get_session_messages", { sessionId: sessionIdAtStart });
          session.replaceSessionMessages(sessionIdAtStart, messages);
        } catch {
          const history = await invoke<string>("get_session_history", { sessionId: sessionIdAtStart });
          session.replaceSessionHistory(sessionIdAtStart, history);
        }
        session.clearSessionBuffers(sessionIdAtStart);
        resetRenderState();
      }
    } catch (err) {
      console.warn("恢复中断消息失败:", err);
    }

    requestView.latestCheckpoint = null;
    requestView.showRecallEdit = false;
    requestView.currentTurnStepsStart = requestView.agentSteps.length;
    requestView.hydrated = true;
    requestView.status = "RUNNING";
    session.runningSessionId = sessionIdAtStart;
    requestView.activeRunId = null;
    requestView.resumableRunId = null;
    requestView.runStartTime = Date.now();
    requestView.streamActive = false;
    requestView.cancelHandled = false;
    session.clearSessionBuffers(sessionIdAtStart);
    resetRenderState();

    if (!resumeOnly) {
      let displayMsg = msg;
      const userImages = imageBase64List && imageBase64List.length > 0 ? [...imageBase64List] : null;
      if (userImages) {
        const imageHtml = userImages
          .map(
            (b64) =>
              `<img src="${b64}" style="max-width: 200px; max-height: 200px; border-radius: 8px; margin: 4px 4px 4px 0; display: inline-block; vertical-align: middle;" alt="用户发送的图片" />`
          )
          .join("");
        displayMsg = imageHtml + (msg ? `\n\n${msg}` : "");
      }

      // 长消息自动折叠：超过6行或500字符时折叠
      const COLLAPSE_LINE_THRESHOLD = 6;
      const COLLAPSE_CHAR_THRESHOLD = 500;
      const plainText = msg.replace(/<[^>]*>/g, '').replace(/\n{3,}/g, '\n\n');
      const lineCount = plainText.split('\n').length;
      const shouldCollapse = lineCount > COLLAPSE_LINE_THRESHOLD || plainText.length > COLLAPSE_CHAR_THRESHOLD;
      const userMsgHtml = shouldCollapse
        ? `<div class="chat-message user-message" style="position: relative;"><div class="message-content"><div class="user-msg-collapsed" data-collapsed="true"><div class="user-msg-preview">\n\n${displayMsg}\n\n</div><div class="user-msg-fade"></div></div><button class="user-msg-toggle" onclick="this.previousElementSibling.dataset.collapsed=this.previousElementSibling.dataset.collapsed==='true'?'false':'true';this.textContent=this.previousElementSibling.dataset.collapsed==='true'?'展开全部':'收起'">展开全部</button></div></div>\n\n`
        : `<div class="chat-message user-message" style="position: relative;"><div class="message-content">\n\n${displayMsg}\n\n</div></div>\n\n`;
      session.appendSessionHistory(
        sessionIdAtStart,
        userMsgHtml
      );

      // 同时添加到结构化消息数组（供 Vue 组件渲染）
      session.appendSessionMessage(sessionIdAtStart, {
        role: "user",
        id: `user_${Date.now()}`,
        text: msg || "",
        images: userImages,
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
          });

      const sessionSwitched = sessionIdAtStart !== session.activeSessionId;
      if (!sessionSwitched) {
        session.setSessionUsageTotals(res.session_input_tokens || 0, res.session_output_tokens || 0);
      }
      requestView.lastUserMessage = resumeOnly ? "" : msg;

      if (res.status === "CANCELLED") {
        if (!requestView.cancelHandled) {
          const cancellationFallback = res.content && res.content !== "用户已取消执行。" ? res.content : "";
          const { finalContent, finalToolBuffer } = buildFinalResponseParts(requestView, cancellationFallback);
          const hasPartialContent = finalContent || finalToolBuffer;
          if (hasPartialContent) {
            const canceledSnapshot = buildAgentTurnSnapshot(requestView.currentTurn, finalContent, finalToolBuffer, undefined, "CANCELLED");
            canceledSnapshot.notice = "用户已取消执行，以上为部分结果";
            session.appendSessionMessage(sessionIdAtStart, { role: "agent", id: `agent_${Date.now()}`, snapshot: canceledSnapshot });
          } else if (res.content && res.content !== "用户已取消执行。") {
            const canceledSnapshot = buildAgentTurnSnapshot(requestView.currentTurn, stripPseudoToolCalls(res.content), "", undefined, "CANCELLED");
            canceledSnapshot.notice = "用户已取消执行，以上为部分结果";
            session.appendSessionMessage(sessionIdAtStart, { role: "agent", id: `agent_${Date.now()}`, snapshot: canceledSnapshot });
          }
          requestView.latestCheckpoint = null;
          session.clearSessionBuffers(sessionIdAtStart);
          resetRenderState();
          requestView.lastUserMessage = resumeOnly ? "" : msg;
          requestView.showRecallEdit = !resumeOnly;
          requestView.hydrated = true;
        }
        if (myGeneration === sendGeneration) {
          requestView.runStartTime = null;
          requestView.streamActive = false;
          requestView.status = "IDLE";
          session.runningSessionId = null;
          requestView.activeRunId = null;
        }
        requestView.cancelHandled = false;
        if (!sessionSwitched && myGeneration === sendGeneration) {
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
          {
            input: res.input_tokens || 0,
            output: res.output_tokens || 0,
            sessionInput: res.session_input_tokens || 0,
            sessionOutput: res.session_output_tokens || 0,
          },
          res.status,
        );
        requestView.latestCheckpoint = null;
        session.clearSessionBuffers(sessionIdAtStart);
        session.appendSessionMessage(sessionIdAtStart, { role: "agent", id: `agent_${Date.now()}`, snapshot: clarificationSnapshot });
        resetRenderState();
        if (myGeneration === sendGeneration) {
          requestView.streamActive = false;
          requestView.status = "IDLE";
          session.runningSessionId = null;
          requestView.activeRunId = null;
        }
        if (!sessionSwitched && myGeneration === sendGeneration) {
          triggerRender();
          scrollToBottomCb?.();
        }
        // steps persist removed — session_messages is the source of truth
        return;
      }

      const { finalContent, finalToolBuffer } = buildFinalResponseParts(requestView, res.content);
      const inputTokens = res.input_tokens ?? (res as any).inputTokens ?? 0;
      const outputTokens = res.output_tokens ?? (res as any).outputTokens ?? 0;
      const sessionInputTokens = res.session_input_tokens ?? (res as any).sessionInputTokens ?? 0;
      const sessionOutputTokens = res.session_output_tokens ?? (res as any).sessionOutputTokens ?? 0;
      
      // 更新当前 turn 的 tokens 状态，供 Live 组件渲染
      requestView.currentTurn.tokens = {
        input: inputTokens,
        output: outputTokens,
        sessionInput: sessionInputTokens,
        sessionOutput: sessionOutputTokens,
      };

      // 先清空 live 缓冲区，避免 AgentTurn 与追加到历史的同一段内容同时渲染
      session.clearSessionBuffers(sessionIdAtStart);

      if (myGeneration === sendGeneration) {
        requestView.status = "IDLE";
        session.runningSessionId = null;
        requestView.activeRunId = null;
        requestView.resumableRunId = null;
        requestView.streamActive = false;
        requestView.runStartTime = null;
        requestView.latestCheckpoint = null;
      }

      // 同时添加到结构化消息数组（供 Vue 组件渲染）
      const snapshot = buildAgentTurnSnapshot(requestView.currentTurn, finalContent, finalToolBuffer, undefined, res.status);
      session.appendSessionMessage(sessionIdAtStart, {
        role: "agent",
        id: `agent_${Date.now()}`,
        snapshot,
      });

      resetRenderState();

      if (!sessionSwitched && myGeneration === sendGeneration) {
        triggerRender();
        scrollToBottomCb?.();
      }
      // agent_steps persist removed
      const sessionAfterSave = useSessionStore();
      if (sessionIdAtStart === sessionAfterSave.activeSessionId) {
        sessionAfterSave.setSessionUsageTotals(sessionInputTokens, sessionOutputTokens);
      }
      // agent 完成后主动拉取最新上下文快照，使监控面板的 Session Messages 包含完整最后一轮回复
      refreshContextSnapshot(sessionIdAtStart);
    } catch (err) {
      session.clearSessionBuffers(sessionIdAtStart);
      resetRenderState();

      console.error("[chat] ask_jarvis 失败，原始错误:", err);
      const errMsg = extractErrorMessage(err);
      console.error("[chat] 格式化后:", errMsg);
      const escapedErr = errMsg.replace(/&/g, "&amp;").replace(/</g, "&lt;").replace(/>/g, "&gt;").replace(/"/g, "&quot;");
      const errorHtml = `<div class="chat-message agent-message"><div class="message-content"><div class="agent-error-banner">${escapedErr}</div></div></div>\n\n`;
      session.appendSessionHistory(sessionIdAtStart, errorHtml);
      if (myGeneration === sendGeneration) {
        requestView.showRecallEdit = !resumeOnly;
        requestView.status = "ERROR";
        session.runningSessionId = null;
        requestView.activeRunId = null;
        requestView.streamActive = false;
      }
      if (sessionIdAtStart === session.activeSessionId && myGeneration === sendGeneration) {
        triggerRender();
      }
    }
  }

  async function cancelJarvis(): Promise<void> {
    const session = useSessionStore();
    const perm = usePermissionStore();
    const runningSessionId = session.activeSessionId;
    if (!runningSessionId) return;

    const view = session.getSessionView(runningSessionId);

    if (view.status !== "RUNNING") return;
    view.cancelHandled = false;
    if (runningSessionId) {
      delete perm.permissionRequests[runningSessionId];
      delete perm.planProposals[runningSessionId];
    }
    try {
      await invoke("cancel_jarvis", { sessionId: runningSessionId });
    } catch (err) {
      console.error("取消执行失败:", err);
    }
    session.runningSessionId = null;
  }

  async function recallAndEdit(): Promise<string> {
    const session = useSessionStore();
    try {
      const recalledText = await invoke<string>("recall_last_message", {
        sessionId: session.activeSessionId,
      });
      const view = session.getSessionView(session.activeSessionId);

      // 从 messages 数组中移除最后一条 agent 消息和用户消息
      while (view.messages.length > 0) {
        const last = view.messages[view.messages.length - 1];
        if (last.role === "agent") {
          view.messages.pop();
        } else {
          break;
        }
      }
      if (view.messages.length > 0 && view.messages[view.messages.length - 1].role === "user") {
        view.messages.pop();
      }

      // 仍保留 jarvisResponse 操作以兼容后端持久化
      const lastUserIdx = view.jarvisResponse.lastIndexOf('<div class="chat-message user-message"');
      if (lastUserIdx !== -1) {
        view.jarvisResponse = view.jarvisResponse.substring(0, lastUserIdx);
      }

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
        const messages = await invoke<any[]>("get_session_messages", { sessionId: plan.sessionId });
        session.replaceSessionMessages(plan.sessionId, messages);
      } catch {
        const history = await invoke<string>("get_session_history", { sessionId: plan.sessionId });
        session.replaceSessionHistory(plan.sessionId, history);
      }
      session.clearSessionBuffers(plan.sessionId);
      resetRenderState();
      triggerRender();
      await sendToJarvis(plan.prompt);
    } catch (err) {
      console.error("恢复执行失败:", err);
    }
  }

  return {
    renderTick,
    rollbackRecalledMessage,
    jarvisResponse,
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
  };
});
