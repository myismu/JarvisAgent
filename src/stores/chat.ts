import { defineStore } from "pinia";
import { ref, computed } from "vue";
import { invoke } from "@tauri-apps/api/core";
import type { JarvisResult, AgentStep } from "../types";
import { useSessionStore } from "./session";
import type { SessionViewState } from "./session";
import { useAgentStore } from "./agent";
import { usePermissionStore } from "./permission";
import { usePreferences } from "../composables/usePreferences";
import { renderMarkdown, renderToolDetails, renderTokenUsage, renderToolStatusLine } from "../utils/markdown";
import { renderStoredHistory } from "../utils/historyRender";
import { buildAgentTurnSnapshot } from "../utils/agentTurnState";
import {
  renderAgentTurnSnapshot,
  serializeAgentTurnSnapshot,
  stripPseudoToolCalls,
} from "../utils/agentTurnRender";

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

const ASSISTANT_MESSAGE_CONTENT_CLASS = "message-content current-turn-content";

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

  function buildStructuredAgentResponseHtml(
    view: SessionViewState,
    finalContent: string,
    finalToolBuffer: string,
    status: string,
    tokens?: { input: number; output: number; sessionInput?: number; sessionOutput?: number },
    notice?: string,
  ) {
    const prefs = usePreferences();
    const snapshot = buildAgentTurnSnapshot(view.currentTurn, finalContent, finalToolBuffer, tokens, status);
    if (notice) {
      snapshot.notice = notice;
    }
    // 渲染快照 HTML
    const rendered = renderAgentTurnSnapshot(snapshot, prefs.agentDisplayMode.value, false);
    
    // 如果快照里有 tokens 但渲染结果里没包含 token-usage 类（可能被 renderAgentTurnSnapshot 内部逻辑跳过），我们强制补上
    let finalHtml = rendered;
    if (tokens && (tokens.input > 0 || tokens.output > 0) && !rendered.includes('token-usage')) {
       finalHtml += renderTokenUsage(tokens.input, tokens.output, tokens.sessionInput, tokens.sessionOutput);
    }

    return `<div class="chat-message agent-message"><div class="${ASSISTANT_MESSAGE_CONTENT_CLASS}">\n\n${serializeAgentTurnSnapshot(snapshot)}\n${finalHtml}\n\n</div></div>\n\n`;
  }

  function resetRenderState() {
    renderedContentStableLen = 0;
    cachedContentHtml = "";
    toolStatusMap.value = new Map();
    parsedCurrentTurnHtml.value = "";
    // 重置节流状态，确保切换会话后 triggerRender 不会被跳过
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
    const session = useSessionStore();
    const view = session.getSessionView(session.activeSessionId);
    let html = "";

    // === 先渲染工具/思考缓冲区（在上方，与最终组装顺序一致） ===
    const liveToolBuffer = `${renderToolStatusLines()}${view.toolBuffer}${view.thinkingBuffer}`;
    if (liveToolBuffer.trim()) {
      // 工具状态行、思考提交会改变内容前缀，不能使用按长度切片的增量缓存。
      html += renderToolDetails(liveToolBuffer, view.streamActive ? "live" : "done", view.streamActive);
    }

    // === 再渲染正文内容（在下方） ===
    const fullContent = stripPseudoToolCalls(`${view.contentBuffer}${view.tempBuffer}`);
    if (fullContent.length < renderedContentStableLen) {
      renderedContentStableLen = 0;
      cachedContentHtml = "";
    }
    if (fullContent.length > 0) {
      const lastNewline = fullContent.lastIndexOf("\n");
      const stableLen = lastNewline >= 0 ? lastNewline + 1 : 0;

      if (stableLen > renderedContentStableLen) {
        const newStablePart = fullContent.slice(renderedContentStableLen, stableLen);
        cachedContentHtml += renderMarkdown(newStablePart);
        renderedContentStableLen = stableLen;
      }

      html += cachedContentHtml;
      const tail = fullContent.slice(renderedContentStableLen);
      if (tail) {
        html += renderMarkdown(tail);
      }
    } else {
      renderedContentStableLen = 0;
      cachedContentHtml = "";
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
    const prefs = usePreferences();
    const view = session.currentSessionView;
    if (view.messages.length === 0) {
      return renderStoredHistory(view.jarvisResponse, session.READY_TEXT, prefs.agentDisplayMode.value);
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

    const config = await invoke<any>("get_config");
    if (config.globalProfileId) {
      config.activeProfileId = config.globalProfileId;
      await invoke("save_config_cmd", { newConfig: config });
      await invoke("update_session_profile", { id: meta.id, profileId: config.globalProfileId });
    }
    return meta.id as string;
  }

  async function sendToJarvis(msg: string, thinkingOverride?: boolean, imageBase64List?: string[]) {
    const session = useSessionStore();

    if (!msg && (!imageBase64List || imageBase64List.length === 0)) return;
    const sessionIdAtStart = await ensureActiveSessionForSend();
    const requestView = session.getSessionView(sessionIdAtStart);

    try {
      const recovered = await invoke<boolean>("recover_interrupted_session_messages", {
        sessionId: sessionIdAtStart,
      });
      if (recovered) {
        const history = await invoke<string>("get_session_history", { sessionId: sessionIdAtStart });
        session.replaceSessionHistory(sessionIdAtStart, history);
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
    requestView.activeRunId = null;
    requestView.resumableRunId = null;
    requestView.runStartTime = Date.now();
    requestView.streamActive = false;
    requestView.cancelHandled = false;
    session.clearSessionBuffers(sessionIdAtStart);
    resetRenderState();

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
        agentDisplayMode: usePreferences().agentDisplayMode.value,
      });

      const sessionSwitched = sessionIdAtStart !== session.activeSessionId;
      if (!sessionSwitched) {
        session.setSessionUsageTotals(res.session_input_tokens || 0, res.session_output_tokens || 0);
      }
      requestView.lastUserMessage = msg;

      if (res.status === "CANCELLED") {
        if (!requestView.cancelHandled) {
          const cancellationFallback = res.content && res.content !== "用户已取消执行。" ? res.content : "";
          const { finalContent, finalToolBuffer } = buildFinalResponseParts(requestView, cancellationFallback);
          const hasPartialContent = finalContent || finalToolBuffer;
          if (hasPartialContent) {
            const partialResponse = buildStructuredAgentResponseHtml(
              requestView,
              finalContent,
              finalToolBuffer,
              "CANCELLED",
              undefined,
              "用户已取消执行，以上为部分结果",
            );
            session.appendSessionHistory(sessionIdAtStart, partialResponse);
            parsedCurrentTurnHtml.value = "";
          } else if (res.content && res.content !== "用户已取消执行。") {
            const partialResponse = buildStructuredAgentResponseHtml(
              requestView,
              stripPseudoToolCalls(res.content),
              "",
              "CANCELLED",
              undefined,
              "用户已取消执行，以上为部分结果",
            );
            session.appendSessionHistory(sessionIdAtStart, partialResponse);
            parsedCurrentTurnHtml.value = "";
          }
          requestView.latestCheckpoint = null;
          session.clearSessionBuffers(sessionIdAtStart);
          resetRenderState();
          requestView.lastUserMessage = msg;
          requestView.showRecallEdit = true;
          requestView.hydrated = true;
        }
        requestView.runStartTime = null;
        requestView.streamActive = false;
        requestView.status = "IDLE";
        requestView.activeRunId = null;
        requestView.cancelHandled = false;
        if (!sessionSwitched) {
          triggerRender();
          scrollToBottomCb?.();
        }
        await saveAgentStepsToBackend(sessionIdAtStart);
        return;
      }

      if (res.status === "CLARIFICATION_NEEDED") {
        const clarificationResponse = buildStructuredAgentResponseHtml(
          requestView,
          stripPseudoToolCalls(res.content || ""),
          "",
          res.status,
          {
            input: res.input_tokens || 0,
            output: res.output_tokens || 0,
            sessionInput: res.session_input_tokens || 0,
            sessionOutput: res.session_output_tokens || 0,
          },
        );
        requestView.latestCheckpoint = null;
        session.clearSessionBuffers(sessionIdAtStart);
        session.appendSessionHistory(sessionIdAtStart, clarificationResponse);
        resetRenderState();
        requestView.streamActive = false;
        requestView.status = "IDLE";
        requestView.activeRunId = null;
        if (!sessionSwitched) {
          triggerRender();
          scrollToBottomCb?.();
        }
        await saveAgentStepsToBackend(sessionIdAtStart);
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

      const agentResponse = buildStructuredAgentResponseHtml(
        requestView,
        finalContent,
        finalToolBuffer,
        res.status,
        {
          input: inputTokens,
          output: outputTokens,
          sessionInput: sessionInputTokens,
          sessionOutput: sessionOutputTokens,
        },
      );

      // 先重置状态和清除缓冲区，确保“实时”渲染区域在历史记录更新前消失
      requestView.status = res.status;
      requestView.activeRunId = null;
      requestView.resumableRunId = null;
      requestView.streamActive = false;
      requestView.runStartTime = null;
      
      session.clearSessionBuffers(sessionIdAtStart);
      resetRenderState();

      // 然后将最终结果存入历史
      requestView.latestCheckpoint = null;
      session.appendSessionHistory(sessionIdAtStart, agentResponse);
      
      if (!sessionSwitched) {
        triggerRender();
        scrollToBottomCb?.();
      }
      await saveAgentStepsToBackend(sessionIdAtStart);
      const sessionAfterSave = useSessionStore();
      if (sessionIdAtStart === sessionAfterSave.activeSessionId) {
        sessionAfterSave.setSessionUsageTotals(sessionInputTokens, sessionOutputTokens);
      }
    } catch (err) {
      session.clearSessionBuffers(sessionIdAtStart);
      resetRenderState();

      session.appendSessionHistory(sessionIdAtStart, `\n\n**Error:** ${err}`);
      requestView.showRecallEdit = true;
      requestView.status = "ERROR";
      requestView.activeRunId = null;
      requestView.streamActive = false;
      if (sessionIdAtStart === session.activeSessionId) {
        triggerRender();
      }
      await saveAgentStepsToBackend(sessionIdAtStart);
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
      const history = await invoke<string>("get_session_history", { sessionId: plan.sessionId });
      session.replaceSessionHistory(plan.sessionId, history);
      session.clearSessionBuffers(plan.sessionId);
      resetRenderState();
      triggerRender();
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
    followScrollToBottom,
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
