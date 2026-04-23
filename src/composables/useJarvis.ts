import { ref, computed } from "vue";
import { listen } from "@tauri-apps/api/event";
import { invoke } from "@tauri-apps/api/core";
import { marked } from "marked";
import type { TodoItem, PermissionRequest, PlanProposal, JarvisResult } from "../types";

// Configure marked
marked.setOptions({
  breaks: true,
  gfm: true,
});

// Global State
const jarvisResponse = ref("Ready for input...");
const toolBuffer = ref("");
const contentBuffer = ref("");
const tempBuffer = ref("");

const systemStatus = ref("IDLE");
const todos = ref<TodoItem[]>([]);
const permissionRequest = ref<PermissionRequest | null>(null);
const planProposal = ref<PlanProposal | null>(null);

const totalInputTokens = ref(0);
const totalOutputTokens = ref(0);

// Computed for rendering
const parsedHistory = computed(() => {
  return marked.parse(jarvisResponse.value);
});

// Throttled rendering for current turn
const parsedCurrentTurnHtml = ref("");
let throttlePending = false;

function flushCurrentTurnRender() {
  let html = "";
  if (toolBuffer.value) {
    html += `\n\n<details open>\n<summary><svg viewBox="0 0 24 24" width="14" height="14" stroke="currentColor" stroke-width="2" fill="none" stroke-linecap="round" stroke-linejoin="round" style="vertical-align: text-bottom; margin-right: 4px;"><circle cx="12" cy="12" r="3"></circle><path d="M19.4 15a1.65 1.65 0 0 0 .33 1.82l.06.06a2 2 0 0 1 0 2.83 2 2 0 0 1-2.83 0l-.06-.06a1.65 1.65 0 0 0-1.82-.33 1.65 1.65 0 0 0-1 1.51V21a2 2 0 0 1-2 2 2 2 0 0 1-2-2v-.09A1.65 1.65 0 0 0 9 19.4a1.65 1.65 0 0 0-1.82.33l-.06.06a2 2 0 0 1-2.83 0 2 2 0 0 1 0-2.83l.06-.06a1.65 1.65 0 0 0 .33-1.82 1.65 1.65 0 0 0-1.51-1H3a2 2 0 0 1-2-2 2 2 0 0 1 2-2h.09A1.65 1.65 0 0 0 4.6 9a1.65 1.65 0 0 0-.33-1.82l-.06-.06a2 2 0 0 1 0-2.83 2 2 0 0 1 2.83 0l.06.06a1.65 1.65 0 0 0 1.82.33H9a1.65 1.65 0 0 0 1-1.51V3a2 2 0 0 1 2-2 2 2 0 0 1 2 2v.09a1.65 1.65 0 0 0 1 1.51 1.65 1.65 0 0 0 1.82-.33l.06-.06a2 2 0 0 1 2.83 0 2 2 0 0 1 0 2.83l-.06.06a1.65 1.65 0 0 0-.33 1.82V9a1.65 1.65 0 0 0 1.51 1H21a2 2 0 0 1 2 2 2 2 0 0 1-2 2h-.09a1.65 1.65 0 0 0-1.51 1z"></path></svg> 贾维斯正在思考与执行操作... (点击查看详情)</summary>\n\n${toolBuffer.value}\n\n</details>\n\n`;
  }
  html += contentBuffer.value;
  html += tempBuffer.value;
  parsedCurrentTurnHtml.value = html ? (marked.parse(html) as string) : "";
  throttlePending = false;
}

export function triggerRender() {
  if (!throttlePending) {
    throttlePending = true;
    requestAnimationFrame(flushCurrentTurnRender);
  }
}

// Global scroll callback function registration
let scrollToBottomCb: ((force?: boolean) => void) | null = null;
export function registerScrollCb(cb: (force?: boolean) => void) {
  scrollToBottomCb = cb;
}

export function useJarvis() {
  const initListeners = async () => {
    await listen<TodoItem[]>("todo-update", (event) => {
      todos.value = event.payload;
      scrollToBottomCb?.();
    });

    await listen<PermissionRequest>("permission-request", (event) => {
      permissionRequest.value = event.payload;
    });

    // 监听方案审批事件
    await listen<PlanProposal>("plan-proposal", (event) => {
      planProposal.value = event.payload;
    });

    await listen("chat-turn-start", () => {
      tempBuffer.value = "";
      triggerRender();
    });

    await listen<any>("chat-content", (event) => {
      const { content } = event.payload;
      tempBuffer.value += content;
      triggerRender();
      scrollToBottomCb?.();
    });

    await listen("chat-tool-start", () => {
      let thought = tempBuffer.value.trim();
      if (thought) {
        toolBuffer.value += `\n> <svg viewBox="0 0 24 24" width="14" height="14" stroke="currentColor" stroke-width="2" fill="none" stroke-linecap="round" stroke-linejoin="round" style="vertical-align: text-bottom; margin-right: 4px;"><path d="M21 15a2 2 0 0 1-2 2H7l-4 4V5a2 2 0 0 1 2-2h14a2 2 0 0 1 2 2z"></path></svg> 思考与计划: ${thought}\n\n`;
      }
      tempBuffer.value = "";
      triggerRender();
    });

    await listen<any>("chat-tool-debug", (event) => {
      const { content } = event.payload;
      toolBuffer.value += content;
      triggerRender();
      scrollToBottomCb?.();
    });

    await listen<any>("chat-stream", (event) => {
      const { content } = event.payload;
      toolBuffer.value += content;
      triggerRender();
      scrollToBottomCb?.();
    });

    await listen<any>("chat-turn-end", (event) => {
      const { has_tool } = event.payload;
      if (has_tool) {
        let thought = tempBuffer.value.trim();
        if (thought) {
          toolBuffer.value += `\n> <svg viewBox="0 0 24 24" width="14" height="14" stroke="currentColor" stroke-width="2" fill="none" stroke-linecap="round" stroke-linejoin="round" style="vertical-align: text-bottom; margin-right: 4px;"><path d="M21 15a2 2 0 0 1-2 2H7l-4 4V5a2 2 0 0 1 2-2h14a2 2 0 0 1 2 2z"></path></svg> 继续计划: ${thought}\n\n`;
        }
      } else {
        contentBuffer.value += tempBuffer.value;
      }
      tempBuffer.value = "";
      triggerRender();
      scrollToBottomCb?.();
    });
  };

  const resolvePermission = async (decision: string) => {
    if (permissionRequest.value) {
      await invoke("resolve_permission", {
        id: permissionRequest.value.id,
        decision,
      });
      permissionRequest.value = null;
    }
  };

  // 方案审批决策（复用后端 resolve_permission command，因为共用 PendingPermissions）
  const resolvePlan = async (decision: string) => {
    if (planProposal.value) {
      await invoke("resolve_permission", {
        id: planProposal.value.id,
        decision,
      });
      planProposal.value = null;
    }
  };

  const sendToJarvis = async (msg: string) => {
    if (!msg) return;

    // 标记为执行中，前端可以显示停止按钮
    systemStatus.value = "RUNNING";

    if (jarvisResponse.value === "Ready for input...") {
      jarvisResponse.value = "";
    }

    jarvisResponse.value += `<div class="chat-message user-message"><div class="message-content">\n\n${msg}\n\n</div></div>\n\n`;

    try {
      const res = await invoke<JarvisResult>("ask_jarvis", { msg });

      totalInputTokens.value += res.input_tokens || 0;
      totalOutputTokens.value += res.output_tokens || 0;

      let agentResponse = `<div class="chat-message agent-message"><div class="message-content">\n\n`;
      if (toolBuffer.value) {
        agentResponse += `\n\n<details>\n<summary><svg viewBox="0 0 24 24" width="14" height="14" stroke="currentColor" stroke-width="2" fill="none" stroke-linecap="round" stroke-linejoin="round" style="vertical-align: text-bottom; margin-right: 4px;"><circle cx="12" cy="12" r="3"></circle><path d="M19.4 15a1.65 1.65 0 0 0 .33 1.82l.06.06a2 2 0 0 1 0 2.83 2 2 0 0 1-2.83 0l-.06-.06a1.65 1.65 0 0 0-1.82-.33 1.65 1.65 0 0 0-1 1.51V21a2 2 0 0 1-2 2 2 2 0 0 1-2-2v-.09A1.65 1.65 0 0 0 9 19.4a1.65 1.65 0 0 0-1.82.33l-.06.06a2 2 0 0 1-2.83 0 2 2 0 0 1 0-2.83l.06-.06a1.65 1.65 0 0 0 .33-1.82 1.65 1.65 0 0 0-1.51-1H3a2 2 0 0 1-2-2 2 2 0 0 1 2-2h.09A1.65 1.65 0 0 0 4.6 9a1.65 1.65 0 0 0-.33-1.82l-.06-.06a2 2 0 0 1 0-2.83 2 2 0 0 1 2.83 0l.06.06a1.65 1.65 0 0 0 1.82.33H9a1.65 1.65 0 0 0 1-1.51V3a2 2 0 0 1 2-2 2 2 0 0 1 2 2v.09a1.65 1.65 0 0 0 1 1.51 1.65 1.65 0 0 0 1.82-.33l.06-.06a2 2 0 0 1 2.83 0 2 2 0 0 1 0 2.83l-.06.06a1.65 1.65 0 0 0-.33 1.82V9a1.65 1.65 0 0 0 1.51 1H21a2 2 0 0 1 2 2 2 2 0 0 1-2 2h-.09a1.65 1.65 0 0 0-1.51 1z"></path></svg> 贾维斯已完成思考与操作 (点击查看完整决策链)</summary>\n\n${toolBuffer.value}\n\n</details>\n\n`;
      }
      agentResponse += contentBuffer.value;

      agentResponse += `\n\n<div style="font-size: 0.8em; color: gray; margin-top: 10px; border-top: 1px dashed #ccc; padding-top: 5px;"><b>本次消耗</b>: 输入 ${res.input_tokens || 0} / 输出 ${res.output_tokens || 0} Token &nbsp;&nbsp;|&nbsp;&nbsp; <b>会话总计</b>: 输入 ${totalInputTokens.value} / 输出 ${totalOutputTokens.value} Token</div>\n\n`;
      agentResponse += `\n\n</div></div>\n\n`;
      jarvisResponse.value += agentResponse;

      contentBuffer.value = "";
      toolBuffer.value = "";
      triggerRender(); // 立即刷新，清除 parsedCurrentTurnHtml 中的残留内容

      systemStatus.value = res.status;
      scrollToBottomCb?.();
    } catch (err) {
      jarvisResponse.value += "\n\n**Error:** " + err;
      systemStatus.value = "ERROR";
    }
  };

  // 取消当前执行的 Agent
  const cancelJarvis = async () => {
    try {
      await invoke("cancel_jarvis");
    } catch (err) {
      console.error("取消失败:", err);
    }
  };

  return {
    jarvisResponse,
    toolBuffer,
    contentBuffer,
    tempBuffer,
    systemStatus,
    todos,
    permissionRequest,
    planProposal,
    totalInputTokens,
    totalOutputTokens,
    parsedHistory,
    parsedCurrentTurnHtml,
    initListeners,
    resolvePermission,
    resolvePlan,
    sendToJarvis,
    cancelJarvis,
  };
}
