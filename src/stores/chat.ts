import { defineStore } from "pinia";
import { ref, computed } from "vue";
import { invoke } from "@tauri-apps/api/core";
import type { JarvisResult, AgentStep } from "../types";
import { useSessionStore } from "./session";
import { useAgentStore } from "./agent";
import { usePermissionStore } from "./permission";
import { renderMarkdown, renderToolDetails, renderTokenUsage, renderToolStatusLine, renderStoredHistory } from "../utils/markdown";

interface BackendAgentStep {
  type: string;
  tool?: string;
  input_summary?: string;
  output_summary?: string;
  error?: string;
  task?: string;
  attempt?: number;
  max?: number;
  content?: string;
  timestamp: number;
}

function convertFrontendStep(step: AgentStep): BackendAgentStep {
  return {
    type: step.type,
    tool: step.tool,
    input_summary: step.input_summary,
    output_summary: step.output_summary,
    error: step.error,
    task: step.task,
    attempt: step.attempt,
    max: step.max,
    content: step.content,
    timestamp: step.timestamp,
  };
}

export const useChatStore = defineStore("chat", () => {
  const parsedCurrentTurnHtml = ref("");
  let throttlePending = false;
  let lastRenderTime = 0;
  let scrollToBottomCb: ((force?: boolean) => void) | null = null;

  // 增量渲染状态——只渲染新到达的文本，避免每次全量 markdown 解析
  let renderedContentStableLen = 0;
  let cachedContentHtml = "";
  let renderedToolStableLen = 0;
  let cachedToolHtml = "";

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
    renderedContentStableLen = 0;
    cachedContentHtml = "";
    renderedToolStableLen = 0;
    cachedToolHtml = "";
    toolStatusMap.value = new Map();
    parsedCurrentTurnHtml.value = "";
  }

  function registerScrollCb(cb: (force?: boolean) => void) {
    scrollToBottomCb = cb;
  }

  function forceScrollToBottom() {
    scrollToBottomCb?.(true);
  }

  function flushCurrentTurnRender() {
    const session = useSessionStore();
    const view = session.getSessionView(session.activeSessionId);
    let html = "";

    // === 增量渲染正文内容 (contentBuffer + tempBuffer) ===
    const fullContent = `${view.contentBuffer}${view.tempBuffer}`;
    // 防御：如果内容长度回退（缓冲区被外部清空），重置增量状态
    if (fullContent.length < renderedContentStableLen) {
      renderedContentStableLen = 0;
      cachedContentHtml = "";
    }
    if (fullContent.length > 0) {
      // 找到最后一个完整行（以 \n 结尾的行是"稳定的"，不会因为后续字符改变渲染结果）
      const lastNewline = fullContent.lastIndexOf("\n");
      const stableLen = lastNewline >= 0 ? lastNewline + 1 : 0;

      if (stableLen > renderedContentStableLen) {
        // 有新完成的完整行 → 只渲染新增的稳定部分
        const newStablePart = fullContent.slice(renderedContentStableLen, stableLen);
        cachedContentHtml += renderMarkdown(newStablePart);
        renderedContentStableLen = stableLen;
      }

      html += cachedContentHtml;
      // 最后一行（可能不完整）总是重新渲染
      const tail = fullContent.slice(renderedContentStableLen);
      if (tail) {
        html += renderMarkdown(tail);
      }
    } else {
      // 缓冲区被清空（新回合开始）
      renderedContentStableLen = 0;
      cachedContentHtml = "";
    }

    // === 增量渲染工具/思考缓冲区 ===
    const toolStatusHtml = renderToolStatusLines();
    const liveToolBuffer = `${toolStatusHtml}${view.thinkingBuffer}${view.toolBuffer}`;
    if (liveToolBuffer.length < renderedToolStableLen) {
      renderedToolStableLen = 0;
      cachedToolHtml = "";
    }
    if (liveToolBuffer || view.status === "RUNNING") {
      const lastNewline = liveToolBuffer.lastIndexOf("\n");
      const stableLen = lastNewline >= 0 ? lastNewline + 1 : 0;

      if (stableLen > renderedToolStableLen) {
        const newStablePart = liveToolBuffer.slice(renderedToolStableLen, stableLen);
        cachedToolHtml += newStablePart;
        renderedToolStableLen = stableLen;
      }

      const fullToolContent = cachedToolHtml + liveToolBuffer.slice(renderedToolStableLen);
      html += renderToolDetails(fullToolContent, "live", true);
    } else {
      renderedToolStableLen = 0;
      cachedToolHtml = "";
    }

    parsedCurrentTurnHtml.value = html;
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

  // 结构化工具状态——用 Map 替代 HTML 字符串拼接，消除 indexOf 操作
  const toolStatusMap = ref<Map<string, { tool: string; status: string }>>(new Map());

  function upsertToolStatusLine(_view: any, toolCallId: string, tool: string, status: string) {
    const next = new Map(toolStatusMap.value);
    next.set(toolCallId, { tool, status });
    toolStatusMap.value = next;
  }

  function renderToolStatusLines(): string {
    if (toolStatusMap.value.size === 0) return "";
    let html = "";
    toolStatusMap.value.forEach((item, toolCallId) => {
      html += renderToolStatusLine(toolCallId, item.tool, item.status);
    });
    return html;
  }

  const parsedHistory = computed(() => {
    const session = useSessionStore();
    const view = session.currentSessionView;
    if (view.messages.length === 0) {
      return renderStoredHistory(view.jarvisResponse, session.READY_TEXT);
    }
    return view.messages
      .map((msg) => {
        const roleClass = msg.role === "user" ? "user-message" : "agent-message";
        let content = renderMarkdown(msg.content);
        if (msg.thinkingContent) {
          content = content + renderToolDetails(msg.thinkingContent, "done");
        }
        let tokenInfo = "";
        if (msg.tokens) {
          tokenInfo = `\n\n${renderTokenUsage(msg.tokens.input, msg.tokens.output)}`;
        }
        return `<div class="chat-message ${roleClass}" data-msg-id="${msg.id}" data-snapshot-id="${msg.snapshotId || ""}"><div class="message-content">\n\n${content}${tokenInfo}\n\n</div></div>`;
      })
      .join("\n\n");
  });

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

  async function resolvePlan(decision: string, modifiedContent?: string) {
    const perm = usePermissionStore();
    const session = useSessionStore();
    if (perm.planProposal) {
      const sid = perm.planProposals[session.activeSessionId!]?.sessionId ?? session.activeSessionId;
      await invoke("resolve_permission", {
        id: perm.planProposals[session.activeSessionId!].id,
        sessionId: sid,
        decision,
        content: modifiedContent ?? null,
      });
      if (sid) {
        delete perm.planProposals[sid];
      }
    }
  }

  async function saveAgentStepsToBackend(sessionId?: string | null) {
    const session = useSessionStore();
    const sid = sessionId ?? session.activeSessionId;
    if (!sid) return;
    try {
      const view = session.getSessionView(sid);
      const steps = view.agentSteps.map(convertFrontendStep);
      await invoke("save_agent_steps", { steps, sessionId: sid });
    } catch (err) {
      console.error("保存执行流程失败:", err);
    }
  }

  async function loadAgentStepsFromBackend(sessionId?: string | null) {
    const session = useSessionStore();
    const sid = sessionId ?? session.activeSessionId;
    if (!sid) return;
    try {
      const steps = await invoke<BackendAgentStep[]>("get_agent_steps", { sessionId: sid });
      const view = session.getSessionView(sid);
      view.agentSteps = steps.map((s) => ({
        type: s.type as AgentStep["type"],
        tool: s.tool,
        input_summary: s.input_summary,
        output_summary: s.output_summary,
        error: s.error,
        task: s.task,
        attempt: s.attempt,
        max: s.max,
        content: s.content,
        timestamp: s.timestamp,
      }));
      view.currentTurnStepsStart = view.agentSteps.length;
      view.hydrated = true;
    } catch (err) {
      console.error("加载执行流程失败:", err);
      const view = session.getSessionView(sid);
      view.agentSteps = [];
    }
  }

  async function sendToJarvis(msg: string, thinkingOverride?: boolean, imageBase64List?: string[]) {
    const session = useSessionStore();
    const agent = useAgentStore();

    if (!msg && (!imageBase64List || imageBase64List.length === 0)) return;
    if (!session.activeSessionId) return;

    const sessionIdAtStart = session.activeSessionId;
    const requestView = session.getSessionView(sessionIdAtStart);

    requestView.latestCheckpoint = null;
    requestView.showRecallEdit = false;
    requestView.currentTurnStepsStart = requestView.agentSteps.length;
    requestView.hydrated = true;
    requestView.status = "RUNNING";
    requestView.runStartTime = Date.now();
    requestView.cancelHandled = false;
    session.clearSessionBuffers(sessionIdAtStart);
    resetRenderState();
    agent.showAgentPanel = true;

    let displayMsg = msg;
    if (imageBase64List && imageBase64List.length > 0) {
      const imageHtml = imageBase64List
        .map(
          (b64) =>
            `<img src="${b64}" style="max-width: 200px; max-height: 200px; border-radius: 8px; margin: 4px 4px 4px 0; display: inline-block; vertical-align: middle;" alt="用户发送的图片" />`
        )
        .join("");
      displayMsg = imageHtml + (msg ? `\n\n${msg}` : "");
    }

    session.appendSessionHistory(
      sessionIdAtStart,
      `<div class="chat-message user-message" style="position: relative;"><div class="message-content">\n\n${displayMsg}\n\n</div></div>\n\n`
    );

    requestView.lastUserMessage = msg;
    if (sessionIdAtStart === session.activeSessionId) {
      triggerRender();
      scrollToBottomCb?.(true);
    }

    try {
      const res = await invoke<JarvisResult>("ask_jarvis", {
        sessionId: sessionIdAtStart,
        msg,
        thinkingOverride: thinkingOverride ?? null,
        imageBase64List: imageBase64List ?? null,
      });

      const sessionSwitched = sessionIdAtStart !== session.activeSessionId;
      if (!sessionSwitched) {
        session.setSessionUsageTotals(res.session_input_tokens || 0, res.session_output_tokens || 0);
      }
      requestView.lastUserMessage = msg;

      const checkpoint = requestView.latestCheckpoint as any;
      const cpId = checkpoint?.id || "";
      const hasOperations = checkpoint?.hasOperations || false;
      const btnTitle = cpId ? "撤回此消息及操作" : "撤回此消息";
      const btnHtml = `<button class="rollback-trigger" data-cp-id="${cpId}" data-has-operations="${hasOperations}" title="${btnTitle}"></button>`;
      const lastIdx = requestView.jarvisResponse.lastIndexOf("</div></div>\n\n");
      if (lastIdx !== -1) {
        requestView.jarvisResponse =
          requestView.jarvisResponse.slice(0, lastIdx) + btnHtml + requestView.jarvisResponse.slice(lastIdx);
      }
      requestView.latestCheckpoint = null;

      if (res.status === "CANCELLED") {
        if (!requestView.cancelHandled) {
          const hasPartialContent =
            requestView.contentBuffer || requestView.toolBuffer || requestView.tempBuffer;
          if (hasPartialContent) {
            let partialResponse = `<div class="chat-message agent-message"><div class="message-content">\n\n`;
            if (requestView.toolBuffer) {
              partialResponse += renderToolDetails(requestView.toolBuffer, "done");
            }
            partialResponse += requestView.contentBuffer + requestView.tempBuffer;
            partialResponse += `\n\n<div class="token-usage">用户已取消执行，以上为部分结果</div>\n\n`;
            partialResponse += `\n\n</div></div>\n\n`;
            session.appendSessionHistory(sessionIdAtStart, partialResponse);
          } else if (res.content && res.content !== "用户已取消执行。") {
            session.appendSessionHistory(
              sessionIdAtStart,
              `<div class="chat-message agent-message"><div class="message-content">\n\n${res.content}\n\n</div></div>\n\n`
            );
          }
          session.clearSessionBuffers(sessionIdAtStart);
          session.removeTrailingUserMessageFromView(sessionIdAtStart);
          requestView.lastUserMessage = msg;
          requestView.showRecallEdit = true;
          requestView.hydrated = true;
        }
        requestView.runStartTime = null;
        requestView.status = "IDLE";
        requestView.cancelHandled = false;
        if (!sessionSwitched) {
          triggerRender();
          scrollToBottomCb?.();
        }
        await saveAgentStepsToBackend(sessionIdAtStart);
        return;
      }

      if (res.status === "CLARIFICATION_NEEDED") {
        session.clearSessionBuffers(sessionIdAtStart);
        session.appendSessionHistory(
          sessionIdAtStart,
          `<div class="chat-message agent-message"><div class="message-content">\n\n${res.content}\n\n${renderTokenUsage(res.input_tokens || 0, res.output_tokens || 0, res.session_input_tokens || 0, res.session_output_tokens || 0)}\n\n</div></div>\n\n`
        );
        requestView.showRecallEdit = true;
        requestView.status = "IDLE";
        if (!sessionSwitched) {
          triggerRender();
          scrollToBottomCb?.();
        }
        await saveAgentStepsToBackend(sessionIdAtStart);
        return;
      }

      let agentResponse = `<div class="chat-message agent-message"><div class="message-content">\n\n`;
      if (requestView.toolBuffer) {
        agentResponse += renderToolDetails(requestView.toolBuffer, "done");
      }
      agentResponse += requestView.contentBuffer;
      agentResponse += `\n\n${renderTokenUsage(res.input_tokens || 0, res.output_tokens || 0, res.session_input_tokens || 0, res.session_output_tokens || 0)}\n\n`;
      agentResponse += `\n\n</div></div>\n\n`;

      session.appendSessionHistory(sessionIdAtStart, agentResponse);
      session.clearSessionBuffers(sessionIdAtStart);
      requestView.showRecallEdit = true;
      requestView.status = res.status;

      if (!sessionSwitched) {
        triggerRender();
        scrollToBottomCb?.();
      }
      await saveAgentStepsToBackend(sessionIdAtStart);
    } catch (err) {
      session.clearSessionBuffers(sessionIdAtStart);

      const btnHtml = `<button class="rollback-trigger" data-cp-id="" data-has-operations="false" title="撤回此消息"></button>`;
      const lastErrIdx = requestView.jarvisResponse.lastIndexOf("</div></div>\n\n");
      if (lastErrIdx !== -1) {
        requestView.jarvisResponse =
          requestView.jarvisResponse.slice(0, lastErrIdx) + btnHtml + requestView.jarvisResponse.slice(lastErrIdx);
      }

      session.appendSessionHistory(sessionIdAtStart, `\n\n**Error:** ${err}`);
      requestView.showRecallEdit = true;
      requestView.status = "ERROR";
      if (sessionIdAtStart === session.activeSessionId) {
        triggerRender();
      }
      await saveAgentStepsToBackend(sessionIdAtStart);
    }
  }

  async function cancelJarvis(): Promise<string> {
    const session = useSessionStore();
    const perm = usePermissionStore();
    const runningSessionId = session.activeSessionId;
    if (!runningSessionId) return "";

    const view = session.getSessionView(runningSessionId);
    const messageToRestore = view.lastUserMessage;

    if (view.status !== "RUNNING") return messageToRestore;
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
    return messageToRestore;
  }

  async function recallAndEdit(): Promise<string> {
    const session = useSessionStore();
    try {
      const recalledText = await invoke<string>("recall_last_message", {
        sessionId: session.activeSessionId,
      });
      const view = session.getSessionView(session.activeSessionId);

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
      await saveAgentStepsToBackend(session.activeSessionId);
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
      await sendToJarvis(plan.prompt);
    } catch (err) {
      console.error("恢复执行失败:", err);
    }
  }

  return {
    parsedCurrentTurnHtml,
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
    parsedHistory,
    resetRenderState,
    registerScrollCb,
    forceScrollToBottom,
    triggerRender,
    upsertToolStatusLine,
    resolvePermission,
    resolvePlan,
    sendToJarvis,
    cancelJarvis,
    recallAndEdit,
    dismissRecallEdit,
    cancelSubAgentRun,
    resumeAgentRun,
    saveAgentStepsToBackend,
    loadAgentStepsFromBackend,
  };
});
