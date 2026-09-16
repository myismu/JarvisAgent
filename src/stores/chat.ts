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

/**
 * 从正文中剥离"运行被打断"的标记，改为气泡下方的小字说明。
 *
 * 后端在中断收尾时会把标记追加进 `res.content`（如 `> ⚠️ **[回复被中断]** …`、
 * `> ✕ **用户已取消执行…**`）。它属于**状态标注**而非模型正文，若留在正文里
 * 会挤进回复气泡内部，既突兀又与"已保留的部分结果"重复。
 * 这里把它取出来交给 `snapshot.notice`，由渲染层放在气泡下方。
 */
function splitInterruptedNotice(content: string): { content: string; notice?: string } {
  const match = content.match(/\n*>?\s*[⚠✕][^\n]*/);
  if (!match || match.index === undefined) return { content };
  const notice = match[0]
    .replace(/^[\s>]+/, "")
    .replace(/\*\*/g, "")
    .replace(/[⚠️✕]/g, (m) => (m === "✕" ? "✕" : "⚠"))
    .trim();
  const cleaned = content.slice(0, match.index).trimEnd();
  return { content: cleaned, notice: notice || undefined };
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
  let scrollToBottomCb: ((force?: boolean) => void) | null = null;
  const sendGeneration: Record<string, number> = {};

  const rollbackRecalledMessage = ref("");
  const memoryNotice = ref<string | null>(null);

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

  function resetRenderState(sessionId?: string | null) {
    const session = useSessionStore();
    const view = session.getSessionView(sessionId ?? session.activeSessionId);
    view.throttling = false;
    view.lastRenderAt = 0;
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

  function triggerRender() {
    const session = useSessionStore();
    const view = session.getSessionView(session.activeSessionId);
    if (view.throttling) return;
    const now = performance.now();
    const elapsed = now - view.lastRenderAt;
    const MIN_INTERVAL = 33;
    if (elapsed < MIN_INTERVAL) {
      view.throttling = true;
      setTimeout(() => {
        const view2 = session.getSessionView(session.activeSessionId);
        view2.lastRenderAt = performance.now();
        renderTick.value++;
        view2.throttling = false;
      }, MIN_INTERVAL - elapsed);
    } else {
      view.throttling = true;
      view.lastRenderAt = now;
      requestAnimationFrame(() => {
        renderTick.value++;
        view.throttling = false;
      });
    }
  }

  // 渲染 tick，用于触发流式滚动
  const renderTick = ref(0);

  /**
   * 提交权限决策。
   * @param decision allow | allow_session | reject
   * @param feedback 仅 reject 时使用：用户的拒绝说明，会回灌给模型，让它别换个写法重试
   */
  async function resolvePermission(decision: string, feedback?: string) {
    const perm = usePermissionStore();
    const session = useSessionStore();
    const request = perm.permissionRequest;
    if (request) {
      const reqId = request.id;
      const sid = request.sessionId ?? session.activeSessionId;
      const trimmed = feedback?.trim() ?? "";
      const result = await invoke<{ needsResume?: boolean; resumeWith?: string }>("resolve_permission", {
        id: reqId,
        sessionId: sid,
        decision,
        content: trimmed ? trimmed : null,
      });
      if (sid) perm.removePermission(sid, reqId);
      // 循环上限超时后续跑：用 resume_jarvis 续跑，不注入新的用户消息
      if (result?.needsResume && result?.resumeWith && sid) {
        const session = useSessionStore();
        const requestView = session.getSessionView(sid);
        requestView.status = "RUNNING";
        requestView.streamActive = false;
        const res = await invoke<JarvisResult>("resume_jarvis", {
          sessionId: sid,
          reason: result.resumeWith,
        });
        if (res.status !== "PAUSED_LOOP_LIMIT") {
          const { content: resumeContent, notice: resumeNotice } = splitInterruptedNotice(
            stripPseudoToolCalls(res.content),
          );
          const snapshot = buildAgentTurnSnapshot(
            requestView.currentTurn,
            resumeContent,
            "",
            undefined,
            res.status,
            resumeNotice,
          );
          session.appendSessionMessage(sid, { role: "agent", id: `agent_${Date.now()}`, snapshot });
          session.clearSessionBuffers(sid);
          resetRenderState(sid);
        }
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
    const fullMsg = [
      `用户已同意方案「${title}」。严格按以下步骤执行，禁止自己直接执行任务：`,
      `1. 用 ExecuteTool 执行 SwitchWorkMode(mode="edit", reason="方案已审批通过，切回编辑模式执行")`,
      `2. 用 ExecuteTool 执行 CreateTask，批量创建方案中所有任务（传 tasks 数组，含 depends_on 依赖关系）`,
      `3. 用 ExecuteTool 执行 RunSubagentsSequentially，启动调度器由子 Agent 执行任务`,
      `4. 调度完成后读取报告，向用户汇报结果`,
    ].join('\n');
    return sendToJarvis(
      fullMsg,
      undefined,
      undefined,
      false,
      true, // skipRunningCheck — 方案审批续跑是新的用户轮次，不应取消前一轮
      `已同意方案「${title}」`, // 简短展示消息
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
      projectId: session.pendingProjectId,
    });
    session.activeSessionId = meta.id;
    session.workingDirectory = meta.workingDirectory || null;
    session.pendingProjectId = null;
    session.resetSessionView(meta.id);
    session.setSessionUsageTotals(meta.id, {
      input: meta.totalInputTokens || 0,
      output: meta.totalOutputTokens || 0,
      cacheHit: meta.totalCacheHitTokens || 0,
      cacheMiss: meta.totalCacheMissTokens || 0,
    });

    // 记录新建会话使用的模型，后续切回来时自动恢复
    const config = await invoke<any>("get_config");
    if (config.activeProfileId) {
      await invoke("update_session_profile", { id: meta.id, profileId: config.activeProfileId });
    }
    return meta.id as string;
  }

  /**
   * @param thinkingOverride 单轮思考档位覆盖：
   *   - `null` / `undefined`（**正常路径**）：后端按会话档位（`sessions.thinking_mode`）
   *     + 预设默认 + 模型能力自行裁决，前端不参与决策；
   *   - `true` / `false`：仅供程序化调用或测试强制指定单轮值。
   */
  async function sendToJarvis(msg: string, thinkingOverride?: boolean | null, imageBase64List?: string[], resumeOnly = false, skipRunningCheck = false, uiDisplayMsg?: string) {
    const session = useSessionStore();

    if (!msg && (!imageBase64List || imageBase64List.length === 0)) return;

    if (!skipRunningCheck) {
      const currentView = session.getSessionView(session.activeSessionId);
      if (currentView.status === 'RUNNING') {
        await cancelJarvis(session.activeSessionId ?? undefined);
      }
    }

    const sessionIdAtStart = await ensureActiveSessionForSend();
    sendGeneration[sessionIdAtStart] = (sendGeneration[sessionIdAtStart] || 0) + 1;
    const myGeneration = sendGeneration[sessionIdAtStart];
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
        resetRenderState(sessionIdAtStart);
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
    resetRenderState(sessionIdAtStart);

    if (!resumeOnly) {
      let displayMsg = uiDisplayMsg ?? msg;
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
        text: (uiDisplayMsg ?? msg) || "",
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
            displayMsg: uiDisplayMsg ?? null,
          });

      // 用后端返回的 user_message_id 更新前端用户消息，使撤回按钮立即可见
      if (res.user_message_id && !resumeOnly) {
        const msgs = requestView.messages;
        for (let i = msgs.length - 1; i >= 0; i--) {
          if (msgs[i].role === "user" && !msgs[i].messageId) {
            msgs[i].messageId = res.user_message_id;
            break;
          }
        }
      }

      const sessionSwitched = sessionIdAtStart !== session.activeSessionId;
      if (!sessionSwitched) {
        session.setSessionUsageTotals(sessionIdAtStart, {
          input: res.session_input_tokens || 0,
          output: res.session_output_tokens || 0,
          cacheHit: res.session_cache_hit_tokens || 0,
          cacheMiss: res.session_cache_miss_tokens || 0,
        });
      }
      requestView.lastUserMessage = resumeOnly ? "" : msg;

      if (res.status === "CANCELLED") {
        if (!requestView.cancelHandled) {
          const cancellationFallback = res.content && res.content !== "用户已取消执行。" ? res.content : "";
          // 同样先剥离中断/取消标记：后端把它追加在 content 末尾，
          // 若留在正文里会和下面设置的 notice 重复成两行。
          const { content: cleanedFallback } = splitInterruptedNotice(
            stripPseudoToolCalls(cancellationFallback),
          );
          const { finalContent, finalToolBuffer } = buildFinalResponseParts(requestView, cleanedFallback);
          const hasPartialContent = finalContent || finalToolBuffer;
          if (hasPartialContent || cleanedFallback.trim()) {
            const canceledSnapshot = buildAgentTurnSnapshot(
              requestView.currentTurn,
              finalContent || cleanedFallback,
              finalToolBuffer,
              undefined,
              "CANCELLED",
              "用户已取消执行，以上为部分结果",
            );
            session.appendSessionMessage(sessionIdAtStart, { role: "agent", id: `agent_${Date.now()}`, snapshot: canceledSnapshot });
          }
          requestView.latestCheckpoint = null;
          session.clearSessionBuffers(sessionIdAtStart);
          resetRenderState(sessionIdAtStart);
          requestView.lastUserMessage = resumeOnly ? "" : msg;
          requestView.showRecallEdit = !resumeOnly;
          requestView.hydrated = true;
        }
        if (myGeneration === sendGeneration[sessionIdAtStart]) {
          requestView.runStartTime = null;
          requestView.streamActive = false;
          requestView.status = "IDLE";
          requestView.activeRunId = null;
        }
        requestView.cancelHandled = false;
        if (!sessionSwitched && myGeneration === sendGeneration[sessionIdAtStart]) {
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
        resetRenderState(sessionIdAtStart);
        if (myGeneration === sendGeneration[sessionIdAtStart]) {
          requestView.streamActive = false;
          requestView.status = "IDLE";
          requestView.activeRunId = null;
        }
        if (!sessionSwitched && myGeneration === sendGeneration[sessionIdAtStart]) {
          triggerRender();
          scrollToBottomCb?.();
        }
        // steps persist removed — session_messages is the source of truth
        return;
      }

      const { finalContent: rawFinalContent, finalToolBuffer: streamedToolBuffer } =
        buildFinalResponseParts(requestView, res.content);
      // 状态标注优先取后端结构化字段 `notice`；正则剥离作为兜底
      // （兼容历史数据里仍把标记写在正文中的情况）。
      const { content: strippedContent, notice: strippedNotice } =
        splitInterruptedNotice(rawFinalContent);
      const finalContent = strippedContent;
      const interruptedNotice = (res as any).notice || strippedNotice;
      // break_loop 时后端通过 tool_execution_summary 传递工具结果，补充到 toolBuffer
      const finalToolBuffer = streamedToolBuffer || (res as any).toolExecutionSummary || "";
      const inputTokens = res.input_tokens ?? (res as any).inputTokens ?? 0;
      const outputTokens = res.output_tokens ?? (res as any).outputTokens ?? 0;
      const sessionInputTokens = res.session_input_tokens ?? (res as any).sessionInputTokens ?? 0;
      const sessionOutputTokens = res.session_output_tokens ?? (res as any).sessionOutputTokens ?? 0;
      // 会话累计缓存命中 / 未命中：后端从 sessions 表读回，0 表示该会话从未上报过缓存字段
      const sessionCacheHitTokens = res.session_cache_hit_tokens ?? (res as any).sessionCacheHitTokens ?? 0;
      const sessionCacheMissTokens = res.session_cache_miss_tokens ?? (res as any).sessionCacheMissTokens ?? 0;
      
      // 更新当前 turn 的 tokens 状态，供 Live 组件渲染
      requestView.currentTurn.tokens = {
        input: inputTokens,
        output: outputTokens,
        sessionInput: sessionInputTokens,
        sessionOutput: sessionOutputTokens,
      };

      // 先拍快照（保留执行过程），再清空 live 缓冲区
      const snapshot = buildAgentTurnSnapshot(
        requestView.currentTurn,
        finalContent,
        finalToolBuffer,
        undefined,
        res.status,
        interruptedNotice,
      );
      session.clearSessionBuffers(sessionIdAtStart);

      if (myGeneration === sendGeneration[sessionIdAtStart]) {
        requestView.status = "IDLE";
        requestView.activeRunId = null;
        requestView.resumableRunId = null;
        requestView.streamActive = false;
        requestView.runStartTime = null;
        requestView.latestCheckpoint = null;
      }
      session.appendSessionMessage(sessionIdAtStart, {
        role: "agent",
        id: `agent_${Date.now()}`,
        snapshot,
      });

      resetRenderState(sessionIdAtStart);

      if (!sessionSwitched && myGeneration === sendGeneration[sessionIdAtStart]) {
        triggerRender();
        scrollToBottomCb?.();
      }
      // agent_steps persist removed
      const sessionAfterSave = useSessionStore();
      if (sessionIdAtStart === sessionAfterSave.activeSessionId) {
        sessionAfterSave.setSessionUsageTotals(sessionIdAtStart, {
          input: sessionInputTokens,
          output: sessionOutputTokens,
          cacheHit: sessionCacheHitTokens,
          cacheMiss: sessionCacheMissTokens,
        });
      }
      // agent 完成后主动拉取最新上下文快照，使监控面板的 Session Messages 包含完整最后一轮回复
      refreshContextSnapshot(sessionIdAtStart);
    } catch (err) {
      session.clearSessionBuffers(sessionIdAtStart);
      resetRenderState(sessionIdAtStart);

      console.error("[chat] ask_jarvis 失败，原始错误:", err);
      const errMsg = extractErrorMessage(err) || "未知错误";
      console.error("[chat] 格式化后:", errMsg);

      // 构建错误快照，追加到 chat.messages（而非遗留字段 jarvisResponse）
      const snapshot = buildAgentTurnSnapshot(
        requestView.currentTurn,
        `> ✕ **${errMsg}**`,
        "",
        undefined,
        "ERROR",
      );
      session.appendSessionMessage(sessionIdAtStart, {
        role: "agent",
        id: `agent_${Date.now()}`,
        snapshot,
      });

      if (myGeneration === sendGeneration[sessionIdAtStart]) {
        requestView.showRecallEdit = !resumeOnly;
        requestView.status = "ERROR";
        requestView.activeRunId = null;
        requestView.streamActive = false;
        requestView.runStartTime = null;
      }
      if (sessionIdAtStart === session.activeSessionId) {
        triggerRender();
        scrollToBottomCb?.();
      }
    }
  }

  async function cancelJarvis(preferSessionId?: string): Promise<void> {
    const session = useSessionStore();
    const perm = usePermissionStore();
    // 优先取消指定会话，否则找到第一个正在运行的
    const runningSid = (preferSessionId && session.sessionViews[preferSessionId]?.status === 'RUNNING')
      ? preferSessionId
      : Object.keys(session.sessionViews).find(
          k => session.sessionViews[k].status === 'RUNNING' && k !== '__default__'
        );
    if (!runningSid) return;
    const view = session.getSessionView(runningSid);
    view.cancelHandled = false;
    perm.clearPermissions(runningSid);
    delete perm.planProposals[runningSid];
    try {
      await invoke("cancel_jarvis", { sessionId: runningSid });
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
      resetRenderState(plan.sessionId);
      triggerRender();
      await sendToJarvis(plan.prompt);
    } catch (err) {
      console.error("恢复执行失败:", err);
    }
  }

  return {
    renderTick,
    rollbackRecalledMessage,
    memoryNotice,
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
