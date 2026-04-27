import { ref, computed, watch } from "vue";
import { listen } from "@tauri-apps/api/event";
import { invoke } from "@tauri-apps/api/core";
import { marked } from "marked";
import type { TodoItem, PermissionRequest, PlanProposal, PlanDocument, JarvisResult, AgentStep, AgentRun, AgentRunEvent, SubAgentRun, SubAgentEvent } from "../types";

interface SessionCleanupPayload {
  deletedSessionId?: string | null;
  activeSessionId?: string | null;
}

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

interface LatestCheckpoint {
  id: string;
  hasOperations: boolean;
}

export interface ChatMessage {
  id: string;
  role: "user" | "assistant";
  content: string;
  thinkingContent?: string;
  snapshotId?: string;
  timestamp: number;
  tokens?: { input: number; output: number };
}

interface SessionViewState {
  status: string;
  messages: ChatMessage[];
  jarvisResponse: string;
  toolBuffer: string;
  contentBuffer: string;
  tempBuffer: string;
  thinkingBuffer: string;
  lastUserMessage: string;
  showRecallEdit: boolean;
  latestCheckpoint: LatestCheckpoint | null;
  agentSteps: AgentStep[];
  currentTurnStepsStart: number;
  hydrated: boolean;
  runStartTime: number | null;
  cancelHandled: boolean;
}

function convertBackendStep(step: BackendAgentStep): AgentStep {
  return {
    type: step.type as AgentStep["type"],
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

// Configure marked
marked.setOptions({
  breaks: true,
  gfm: true,
});

const READY_TEXT = "Ready for input...";
const DEFAULT_SESSION_KEY = "__default__";

function createEmptySessionView(initialHistory = READY_TEXT, hydrated = false): SessionViewState {
  return {
    status: "IDLE",
    messages: [],
    jarvisResponse: initialHistory,
    toolBuffer: "",
    contentBuffer: "",
    tempBuffer: "",
    thinkingBuffer: "",
    lastUserMessage: "",
    showRecallEdit: false,
    latestCheckpoint: null,
    agentSteps: [],
    currentTurnStepsStart: 0,
    hydrated,
    runStartTime: null,
    cancelHandled: false,
  };
}

function getSessionKey(sessionId: string | null | undefined) {
  return sessionId ?? DEFAULT_SESSION_KEY;
}

// Global State
const sessionViews = ref<Record<string, SessionViewState>>({
  [DEFAULT_SESSION_KEY]: createEmptySessionView(READY_TEXT, true),
});
const todos = ref<TodoItem[]>([]);
const permissionRequests = ref<Record<string, PermissionRequest>>({});
const planProposals = ref<Record<string, PlanProposal>>({});
const planDocumentsBySession = ref<Record<string, PlanDocument[]>>({});
const agentRuns = ref<Record<string, AgentRun>>({});
const agentRunEventsByRun = ref<Record<string, AgentRunEvent[]>>({});
const subAgentRuns = ref<Record<string, SubAgentRun>>({});
const subAgentEventsByRun = ref<Record<string, SubAgentEvent[]>>({});
const focusedTaskId = ref<number | null>(null);

const totalInputTokens = ref(0);
const totalOutputTokens = ref(0);

const showAgentPanel = ref(false);
const agentPanelUserPref = ref<boolean | null>(null);

try {
  const stored = localStorage.getItem("jarvis-agent-panel-open");
  if (stored !== null) {
    agentPanelUserPref.value = stored === "true";
    showAgentPanel.value = agentPanelUserPref.value;
  }
} catch {}

watch(showAgentPanel, (val) => {
  agentPanelUserPref.value = val;
  try { localStorage.setItem("jarvis-agent-panel-open", String(val)); } catch {}
});
const activeSessionId = ref<string | null>(null);

const workingDirectory = ref<string | null>(null);

const rollbackRecalledMessage = ref("");

function getSessionView(sessionId: string | null | undefined, initialHistory = READY_TEXT) {
  const key = getSessionKey(sessionId);
  if (!sessionViews.value[key]) {
    sessionViews.value[key] = createEmptySessionView(initialHistory, false);
  }
  return sessionViews.value[key];
}

function hasHydratedSessionView(sessionId: string | null | undefined) {
  const key = getSessionKey(sessionId);
  return Boolean(sessionViews.value[key]?.hydrated);
}

function resetSessionView(sessionId: string | null | undefined, initialHistory = READY_TEXT) {
  const key = getSessionKey(sessionId);
  sessionViews.value[key] = createEmptySessionView(initialHistory, true);
}

function deleteSessionView(sessionId: string | null | undefined) {
  const key = getSessionKey(sessionId);
  delete sessionViews.value[key];
}

function clearSessionBuffers(sessionId: string | null | undefined) {
  const view = getSessionView(sessionId);
  view.toolBuffer = "";
  view.contentBuffer = "";
  view.tempBuffer = "";
  view.thinkingBuffer = "";
}

function replaceSessionHistory(sessionId: string | null | undefined, history: string) {
  const view = getSessionView(sessionId);
  view.jarvisResponse = history && history.trim() ? history : READY_TEXT;
  view.hydrated = true;
}

function appendSessionHistory(sessionId: string | null | undefined, html: string) {
  const view = getSessionView(sessionId);
  if (view.jarvisResponse === READY_TEXT) {
    view.jarvisResponse = "";
  }
  view.jarvisResponse += html;
  view.hydrated = true;
}

function escapeHtml(value: string) {
  return value
    .replace(/&/g, "&amp;")
    .replace(/</g, "&lt;")
    .replace(/>/g, "&gt;")
    .replace(/"/g, "&quot;")
    .replace(/'/g, "&#39;");
}

function renderMarkdown(value: string) {
  return marked.parse(value || "") as string;
}

function renderTokenUsage(
  inputTokens: number,
  outputTokens: number,
  sessionInputTokens?: number,
  sessionOutputTokens?: number,
) {
  const sessionPart =
    sessionInputTokens !== undefined && sessionOutputTokens !== undefined
      ? ` &nbsp;&nbsp;|&nbsp;&nbsp; <b>会话总计</b>: 输入 ${sessionInputTokens || 0} / 输出 ${sessionOutputTokens || 0} Token`
      : "";
  return `<div class="token-usage"><b>本次消耗</b>: 输入 ${inputTokens || 0} / 输出 ${outputTokens || 0} Token${sessionPart}</div>`;
}

const toolDetailsIcon = `<svg viewBox="0 0 24 24" width="14" height="14" stroke="currentColor" stroke-width="2" fill="none" stroke-linecap="round" stroke-linejoin="round" style="vertical-align: text-bottom; margin-right: 4px;"><circle cx="12" cy="12" r="3"></circle><path d="M12 2v3"></path><path d="M12 19v3"></path><path d="M4.93 4.93l2.12 2.12"></path><path d="M16.95 16.95l2.12 2.12"></path><path d="M2 12h3"></path><path d="M19 12h3"></path><path d="M4.93 19.07l2.12-2.12"></path><path d="M16.95 7.05l2.12-2.12"></path></svg>`;

function renderToolDetails(content: string, mode: "live" | "done", open = false) {
  const summary =
    mode === "live"
      ? "贾维斯正在思考与执行操作... (点击查看详情)"
      : "贾维斯已完成思考与操作 (点击查看完整决策链)";
  const body = content.trim() ? renderMarkdown(content) : "";
  return `\n\n<details ${open ? "open" : ""}>\n<summary>${toolDetailsIcon}${summary}</summary>\n\n${body}\n\n</details>\n\n`;
}

function renderToolStatusIcon(status: string) {
  if (status === "completed") {
    return `<svg class="tool-status-icon completed" viewBox="0 0 24 24" width="14" height="14" fill="none" stroke="currentColor" stroke-width="2.2" stroke-linecap="round" stroke-linejoin="round"><polyline points="20 6 9 17 4 12"></polyline></svg>`;
  }
  if (status === "error") {
    return `<svg class="tool-status-icon error" viewBox="0 0 24 24" width="14" height="14" fill="none" stroke="currentColor" stroke-width="2.2" stroke-linecap="round" stroke-linejoin="round"><circle cx="12" cy="12" r="9"></circle><line x1="12" y1="8" x2="12" y2="12"></line><line x1="12" y1="16" x2="12.01" y2="16"></line></svg>`;
  }
  return `<svg class="tool-status-icon running" viewBox="0 0 24 24" width="14" height="14" fill="none" stroke="currentColor" stroke-width="2.2" stroke-linecap="round"><circle class="tool-status-track" cx="12" cy="12" r="9"></circle><path class="tool-status-head" d="M21 12a9 9 0 0 0-9-9"></path></svg>`;
}

function renderToolStatusLine(toolCallId: string, tool: string, status: string) {
  const safeId = escapeHtml(toolCallId);
  const safeTool = escapeHtml(tool);
  const title = status === "completed" ? "工具执行完成" : status === "error" ? "工具执行失败" : "工具执行中";
  return `<div class="tool-status-line ${escapeHtml(status)}" data-tool-call-id="${safeId}" title="${title}">${renderToolStatusIcon(status)}<code>${safeTool}</code></div>`;
}

function upsertToolStatusLine(view: SessionViewState, toolCallId: string, tool: string, status: string) {
  const html = renderToolStatusLine(toolCallId, tool, status);
  const marker = `data-tool-call-id="${escapeHtml(toolCallId)}"`;
  const markerIndex = view.toolBuffer.indexOf(marker);
  if (markerIndex === -1) {
    view.toolBuffer += `${html}\n`;
    return;
  }

  const start = view.toolBuffer.lastIndexOf('<div class="tool-status-line', markerIndex);
  const end = view.toolBuffer.indexOf("</div>", markerIndex);
  if (start === -1 || end === -1) {
    view.toolBuffer += `${html}\n`;
    return;
  }
  view.toolBuffer = `${view.toolBuffer.slice(0, start)}${html}${view.toolBuffer.slice(end + "</div>".length)}`;
}

function replaceSessionAgentSteps(sessionId: string | null | undefined, steps: AgentStep[]) {
  const view = getSessionView(sessionId);
  view.agentSteps = steps;
  view.currentTurnStepsStart = steps.length;
  view.hydrated = true;
  if (steps.length > 0 && agentPanelUserPref.value !== false) {
    showAgentPanel.value = true;
  }
}

function removeTrailingUserMessageFromView(sessionId: string | null | undefined = activeSessionId.value) {
  const view = getSessionView(sessionId);
  const lastUserIdx = view.jarvisResponse.lastIndexOf('<div class="chat-message user-message"');
  if (lastUserIdx !== -1) {
    const lastAgentIdx = view.jarvisResponse.lastIndexOf('<div class="chat-message agent-message"');
    if (lastAgentIdx < lastUserIdx) {
      view.jarvisResponse = view.jarvisResponse.substring(0, lastUserIdx);
    }
  }
}

const currentSessionView = computed(() => getSessionView(activeSessionId.value));

const jarvisResponse = computed({
  get: () => currentSessionView.value.jarvisResponse,
  set: (value: string) => {
    const view = getSessionView(activeSessionId.value);
    view.jarvisResponse = value;
    view.hydrated = true;
  },
});

const messages = computed({
  get: () => currentSessionView.value.messages,
  set: (value: ChatMessage[]) => {
    const view = getSessionView(activeSessionId.value);
    view.messages = value;
    view.hydrated = true;
  },
});

const toolBuffer = computed({
  get: () => currentSessionView.value.toolBuffer,
  set: (value: string) => {
    const view = getSessionView(activeSessionId.value);
    view.toolBuffer = value;
    view.hydrated = true;
  },
});

const contentBuffer = computed({
  get: () => currentSessionView.value.contentBuffer,
  set: (value: string) => {
    const view = getSessionView(activeSessionId.value);
    view.contentBuffer = value;
    view.hydrated = true;
  },
});

const tempBuffer = computed({
  get: () => currentSessionView.value.tempBuffer,
  set: (value: string) => {
    const view = getSessionView(activeSessionId.value);
    view.tempBuffer = value;
    view.hydrated = true;
  },
});

const thinkingBuffer = computed({
  get: () => currentSessionView.value.thinkingBuffer,
  set: (value: string) => {
    const view = getSessionView(activeSessionId.value);
    view.thinkingBuffer = value;
    view.hydrated = true;
  },
});

const lastUserMessage = computed({
  get: () => currentSessionView.value.lastUserMessage,
  set: (value: string) => {
    const view = getSessionView(activeSessionId.value);
    view.lastUserMessage = value;
    view.hydrated = true;
  },
});

const showRecallEdit = computed({
  get: () => currentSessionView.value.showRecallEdit,
  set: (value: boolean) => {
    const view = getSessionView(activeSessionId.value);
    view.showRecallEdit = value;
    view.hydrated = true;
  },
});

const latestCheckpoint = computed({
  get: () => currentSessionView.value.latestCheckpoint,
  set: (value: LatestCheckpoint | null) => {
    const view = getSessionView(activeSessionId.value);
    view.latestCheckpoint = value;
    view.hydrated = true;
  },
});

const agentSteps = computed({
  get: () => currentSessionView.value.agentSteps,
  set: (value: AgentStep[]) => {
    const view = getSessionView(activeSessionId.value);
    view.agentSteps = value;
    view.hydrated = true;
  },
});

const currentSessionStatus = computed(() => {
  return currentSessionView.value.status;
});

const isCurrentSessionRunning = computed(() => {
  return currentSessionView.value.status === "RUNNING";
});

const isAnySessionRunning = computed(() => {
  return Object.values(sessionViews.value).some((v) => v.status === "RUNNING");
});

const currentSubAgentRuns = computed(() => {
  const sessionId = activeSessionId.value;
  if (!sessionId) return [];
  return Object.values(subAgentRuns.value)
    .filter((run) => run.sessionId === sessionId)
    .sort((a, b) => a.startedAt - b.startedAt);
});

const activeSubAgentRuns = computed(() => {
  return currentSubAgentRuns.value.filter((run) => run.status === "running");
});

const getSubAgentEvents = (runId: string): SubAgentEvent[] => {
  return subAgentEventsByRun.value[runId] ?? [];
};

const focusTask = (taskId: number | null | undefined) => {
  if (taskId === null || taskId === undefined) return;
  focusedTaskId.value = taskId;
  window.dispatchEvent(new CustomEvent("subagent-task-focus", { detail: { taskId } }));
};

const permissionRequest = computed(() => {
  if (!activeSessionId.value) return null;
  return permissionRequests.value[activeSessionId.value] ?? null;
});

const planProposal = computed(() => {
  if (!activeSessionId.value) return null;
  return planProposals.value[activeSessionId.value] ?? null;
});

const currentPlanDocuments = computed(() => {
  if (!activeSessionId.value) return [];
  return planDocumentsBySession.value[activeSessionId.value] ?? [];
});

const currentAgentRuns = computed(() => {
  const sessionId = activeSessionId.value;
  if (!sessionId) return [];
  return Object.values(agentRuns.value)
    .filter((run) => run.sessionId === sessionId)
    .sort((a, b) => b.startedAt - a.startedAt);
});

const interruptedAgentRuns = computed(() => {
  return currentAgentRuns.value.filter((run) => run.status === "interrupted" && run.resumable);
});

const getAgentRunEvents = (runId: string): AgentRunEvent[] => {
  return agentRunEventsByRun.value[runId] ?? [];
};

function findMatchingDivClose(html: string, divStart: number) {
  const openEnd = html.indexOf(">", divStart);
  if (openEnd === -1) return -1;

  const lower = html.toLowerCase();
  let depth = 1;
  let cursor = openEnd + 1;

  while (cursor < html.length) {
    const nextOpen = lower.indexOf("<div", cursor);
    const nextClose = lower.indexOf("</div>", cursor);
    if (nextClose === -1) return -1;

    if (nextOpen !== -1 && nextOpen < nextClose) {
      depth += 1;
      cursor = nextOpen + 4;
      continue;
    }

    depth -= 1;
    if (depth === 0) return nextClose;
    cursor = nextClose + "</div>".length;
  }

  return -1;
}

function renderHistoryMessageBlock(block: string) {
  const contentStart = block.indexOf('<div class="message-content"');
  if (contentStart === -1) return block;

  const openEnd = block.indexOf(">", contentStart);
  const closeStart = findMatchingDivClose(block, contentStart);
  if (openEnd === -1 || closeStart === -1 || closeStart <= openEnd) {
    return renderMarkdown(block);
  }

  const beforeContent = block.slice(0, openEnd + 1);
  const rawContent = block.slice(openEnd + 1, closeStart);
  const afterContent = block.slice(closeStart);

  return `${beforeContent}${renderMarkdown(rawContent)}${afterContent}`;
}

function renderStoredHistory(history: string) {
  const trimmed = history.trim();
  if (!trimmed) return "";
  if (trimmed === READY_TEXT) return renderMarkdown(history);

  const messageMarker = '<div class="chat-message';
  if (!history.includes(messageMarker)) {
    return renderMarkdown(history);
  }

  let rendered = "";
  let cursor = 0;
  while (cursor < history.length) {
    const start = history.indexOf(messageMarker, cursor);
    if (start === -1) {
      rendered += history.slice(cursor);
      break;
    }

    rendered += history.slice(cursor, start);
    const next = history.indexOf(messageMarker, start + messageMarker.length);
    const end = next === -1 ? history.length : next;
    rendered += renderHistoryMessageBlock(history.slice(start, end));
    cursor = end;
  }

  return rendered;
}

// Computed for rendering
const parsedHistory = computed(() => {
  if (messages.value.length === 0) {
    return renderStoredHistory(jarvisResponse.value);
  }
  return messages.value.map(msg => {
    const roleClass = msg.role === "user" ? "user-message" : "agent-message";
    let content = renderMarkdown(msg.content);
    if (msg.thinkingContent) {
      content = renderToolDetails(msg.thinkingContent, "done") + content;
    }
    let tokenInfo = "";
    if (msg.tokens) {
      tokenInfo = `\n\n${renderTokenUsage(msg.tokens.input, msg.tokens.output)}`;
    }
    return `<div class="chat-message ${roleClass}" data-msg-id="${msg.id}" data-snapshot-id="${msg.snapshotId || ""}"><div class="message-content">\n\n${content}${tokenInfo}\n\n</div></div>`;
  }).join("\n\n");
});

// Throttled rendering for current turn
const parsedCurrentTurnHtml = ref("");
let throttlePending = false;

function flushCurrentTurnRender() {
  const view = getSessionView(activeSessionId.value);
  let html = "";
  const liveToolBuffer = (view.toolBuffer || view.thinkingBuffer)
    ? `${view.thinkingBuffer}${view.toolBuffer}`
    : "";
  if (view.status === "RUNNING" || liveToolBuffer) {
    html += renderToolDetails(liveToolBuffer, "live", true);
  }
  const responseContent = `${view.contentBuffer}${view.tempBuffer}`;
  if (responseContent) {
    html += renderMarkdown(responseContent);
  }
  parsedCurrentTurnHtml.value = html;
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
export function forceScrollToBottom() {
  scrollToBottomCb?.(true);
}

function setSessionUsageTotals(inputTokens: number, outputTokens: number) {
  totalInputTokens.value = inputTokens || 0;
  totalOutputTokens.value = outputTokens || 0;
}

function syncActiveSessionView(sessionId: string | null | undefined, scroll = false) {
  if (sessionId === activeSessionId.value) {
    triggerRender();
    if (scroll) {
      scrollToBottomCb?.();
    }
  }
}

function upsertPlanDocument(document: PlanDocument, fallbackSessionId: string | null | undefined = activeSessionId.value) {
  const sessionId = document.sessionId || fallbackSessionId;
  if (!sessionId) return;
  const existing = planDocumentsBySession.value[sessionId] ?? [];
  const next = [
    document,
    ...existing.filter((item) => item.id !== document.id),
  ].sort((a, b) => b.updatedAt - a.updatedAt);
  planDocumentsBySession.value = {
    ...planDocumentsBySession.value,
    [sessionId]: next,
  };
}

function upsertAgentRun(run: AgentRun) {
  if (!run?.runId) return;
  agentRuns.value = {
    ...agentRuns.value,
    [run.runId]: run,
  };
}

export function useJarvis() {
  const initListeners = async () => {
    await listen<TodoItem[]>("todo-update", (event) => {
      todos.value = event.payload;
      scrollToBottomCb?.();
    });

    await listen<PermissionRequest>("permission-request", (event) => {
      const sid = event.payload.sessionId ?? activeSessionId.value;
      if (sid) {
        permissionRequests.value[sid] = event.payload;
      }
    });

    await listen<PlanProposal>("plan-proposal", (event) => {
      const sid = event.payload.sessionId ?? activeSessionId.value;
      if (sid) {
        planProposals.value[sid] = event.payload;
        upsertPlanDocument({
          id: event.payload.id,
          sessionId: sid,
          title: event.payload.title,
          content: event.payload.content,
          status: "pending",
          path: null,
          createdAt: Date.now() / 1000,
          updatedAt: Date.now() / 1000,
          decidedAt: null,
        }, sid);
        if (sid === activeSessionId.value && agentPanelUserPref.value !== false) {
          showAgentPanel.value = true;
        }
      }
    });

    await listen<PlanDocument>("plan-document-updated", (event) => {
      upsertPlanDocument(event.payload);
      if (event.payload.sessionId === activeSessionId.value && agentPanelUserPref.value !== false) {
        showAgentPanel.value = true;
      }
    });

    await listen<AgentRun>("agent-run-updated", (event) => {
      const run = event.payload;
      upsertAgentRun(run);
    });

    await listen<AgentRunEvent>("agent-run-event", (event) => {
      const item = event.payload;
      if (!item?.runId) return;
      const events = [...(agentRunEventsByRun.value[item.runId] ?? []), item]
        .sort((a, b) => a.timestamp - b.timestamp)
        .slice(-500);
      agentRunEventsByRun.value = {
        ...agentRunEventsByRun.value,
        [item.runId]: events,
      };
    });

    await listen<any>("chat-turn-start", (event) => {
      const sessionId = event.payload?.sessionId ?? activeSessionId.value;
      if (!sessionId) return;
      const view = getSessionView(sessionId);
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

    await listen<any>("chat-content", (event) => {
      const sessionId = event.payload?.sessionId ?? activeSessionId.value;
      if (!sessionId) return;
      const { content } = event.payload;
      const view = getSessionView(sessionId);
      view.tempBuffer += content;
      view.hydrated = true;
      syncActiveSessionView(sessionId, true);
    });

    await listen<any>("chat-thinking", (event) => {
      const sessionId = event.payload?.sessionId ?? activeSessionId.value;
      if (!sessionId) return;
      const { content } = event.payload;
      const view = getSessionView(sessionId);
      view.thinkingBuffer += content;
      view.hydrated = true;
      syncActiveSessionView(sessionId, true);
    });

    await listen<any>("chat-tool-start", (event) => {
      const sessionId = event.payload?.sessionId ?? activeSessionId.value;
      if (!sessionId) return;
      const view = getSessionView(sessionId);
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

    await listen<any>("chat-tool-debug", (event) => {
      const sessionId = event.payload?.sessionId ?? activeSessionId.value;
      if (!sessionId) return;
      const view = getSessionView(sessionId);
      const { content, kind, toolCallId, tool, status } = event.payload;
      if (kind === "tool_status" && toolCallId && tool && status) {
        upsertToolStatusLine(view, String(toolCallId), String(tool), String(status));
      } else if (content) {
        view.toolBuffer += content;
      }
      view.hydrated = true;
      syncActiveSessionView(sessionId, true);
    });

    await listen<any>("chat-stream", (event) => {
      const sessionId = event.payload?.sessionId ?? activeSessionId.value;
      if (!sessionId) return;
      const { content } = event.payload;
      const view = getSessionView(sessionId);
      view.toolBuffer += content;
      view.hydrated = true;
      syncActiveSessionView(sessionId, true);
    });

    await listen<any>("chat-turn-end", (event) => {
      const sessionId = event.payload?.sessionId ?? activeSessionId.value;
      if (!sessionId) return;
      const { has_tool } = event.payload;
      const view = getSessionView(sessionId);
      const thought = view.thinkingBuffer.trim();
      if (thought) {
        view.toolBuffer += `${thought}\n\n`;
        const summary = thought.length > 100 ? thought.substring(0, 100) + "..." : thought;
        view.toolBuffer += has_tool
          ? `> 继续计划: ${summary}\n\n`
          : `> 思考摘要: ${summary}\n\n`;
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

    await listen<any>("agent-step", (event) => {
      const sessionId = event.payload?.sessionId ?? activeSessionId.value;
      if (!sessionId) return;
      const step = event.payload as Omit<AgentStep, "timestamp">;
      const view = getSessionView(sessionId);
      view.agentSteps.push({ ...step, timestamp: Date.now() });
      view.hydrated = true;
      if (sessionId === activeSessionId.value && agentPanelUserPref.value !== false) {
        showAgentPanel.value = true;
      }
    });

    await listen<SubAgentRun>("subagent-updated", (event) => {
      const run = event.payload;
      if (!run?.runId) return;
      subAgentRuns.value = {
        ...subAgentRuns.value,
        [run.runId]: run,
      };
      if (run.sessionId === activeSessionId.value && agentPanelUserPref.value !== false) {
        showAgentPanel.value = true;
      }
    });

    await listen<SubAgentEvent>("subagent-event", (event) => {
      const item = event.payload;
      if (!item?.runId) return;
      const events = [...(subAgentEventsByRun.value[item.runId] ?? []), item]
        .sort((a, b) => a.timestamp - b.timestamp)
        .slice(-300);
      subAgentEventsByRun.value = {
        ...subAgentEventsByRun.value,
        [item.runId]: events,
      };
    });

    await listen<any>("checkpoint-created", (event) => {
      const sessionId = event.payload?.sessionId ?? activeSessionId.value;
      if (!sessionId) return;
      if (event.payload?.checkpointId) {
        const view = getSessionView(sessionId);
        view.latestCheckpoint = {
          id: event.payload.checkpointId,
          hasOperations: event.payload.hasOperations === true,
        };
        view.hydrated = true;
      }
    });

    await listen<SessionCleanupPayload>("active-session-changed", async (event) => {
      const deletedSessionId = event.payload?.deletedSessionId ?? null;
      const nextActiveSessionId = event.payload?.activeSessionId ?? null;

      if (deletedSessionId) {
        deleteSessionView(deletedSessionId);

      }

      activeSessionId.value = nextActiveSessionId;
      if (deletedSessionId) {
        delete planProposals.value[deletedSessionId];
        delete planDocumentsBySession.value[deletedSessionId];
        delete permissionRequests.value[deletedSessionId];
        agentRuns.value = Object.fromEntries(
          Object.entries(agentRuns.value).filter(([, run]) => run.sessionId !== deletedSessionId)
        );
        agentRunEventsByRun.value = Object.fromEntries(
          Object.entries(agentRunEventsByRun.value).filter(([, events]) => {
            return events.some((item) => item.sessionId !== deletedSessionId);
          })
        );
        subAgentRuns.value = Object.fromEntries(
          Object.entries(subAgentRuns.value).filter(([, run]) => run.sessionId !== deletedSessionId)
        );
        subAgentEventsByRun.value = Object.fromEntries(
          Object.entries(subAgentEventsByRun.value).filter(([, events]) => {
            return events.some((item) => item.sessionId !== deletedSessionId);
          })
        );
      }

      try {
        if (nextActiveSessionId) {
          const meta = await invoke<any>("get_session_meta", { id: nextActiveSessionId });
          workingDirectory.value = meta.workingDirectory || null;
          setSessionUsageTotals(meta.totalInputTokens || 0, meta.totalOutputTokens || 0);

          if (!hasHydratedSessionView(nextActiveSessionId)) {
            const history = await invoke<string>("get_session_history", { sessionId: nextActiveSessionId });
            replaceSessionHistory(nextActiveSessionId, history);
            await loadAgentStepsFromBackend(nextActiveSessionId);
          }
          await loadSubAgentRunsFromBackend(nextActiveSessionId);
          await loadSubAgentEventsFromBackend(nextActiveSessionId);
          await loadPlanDocumentsFromBackend(nextActiveSessionId);
          await loadAgentRunsFromBackend(nextActiveSessionId);
          await loadAgentRunEventsFromBackend(nextActiveSessionId);
        } else {
          workingDirectory.value = null;
          setSessionUsageTotals(0, 0);
          resetSessionView(null, READY_TEXT);
        }
      } catch (err) {
        console.error("同步清理后的会话失败:", err);
        workingDirectory.value = null;
        setSessionUsageTotals(0, 0);
        if (nextActiveSessionId) {
          resetSessionView(nextActiveSessionId, READY_TEXT);
        } else {
          resetSessionView(null, READY_TEXT);
        }
      }

      triggerRender();
      scrollToBottomCb?.();
    });
  };

  const resolvePermission = async (decision: string) => {
    if (permissionRequest.value) {
      const sid = permissionRequest.value.sessionId ?? activeSessionId.value;
      await invoke("resolve_permission", {
        id: permissionRequest.value.id,
        sessionId: sid,
        decision,
      });
      if (sid) {
        delete permissionRequests.value[sid];
      }
    }
  };

  const resolvePlan = async (decision: string, modifiedContent?: string) => {
    if (planProposal.value) {
      const sid = planProposal.value.sessionId ?? activeSessionId.value;
      await invoke("resolve_permission", {
        id: planProposal.value.id,
        sessionId: sid,
        decision,
        content: modifiedContent ?? null,
      });
      if (sid) {
        delete planProposals.value[sid];
      }
    }
  };

  const updatePlanProposalContent = (newContent: string) => {
    const sid = activeSessionId.value;
    if (sid && planProposals.value[sid]) {
      planProposals.value[sid] = { ...planProposals.value[sid], content: newContent };
    }
  };

  const sendToJarvis = async (msg: string, thinkingOverride?: boolean, imageBase64List?: string[]) => {
    if (!msg && (!imageBase64List || imageBase64List.length === 0)) return;
    if (!activeSessionId.value) return;

    const sessionIdAtStart = activeSessionId.value;
    const requestView = getSessionView(sessionIdAtStart);

    // 重置本轮 checkpoint 跟踪
    requestView.latestCheckpoint = null;
    requestView.showRecallEdit = false;
    requestView.currentTurnStepsStart = requestView.agentSteps.length;
    requestView.hydrated = true;
    requestView.status = "RUNNING";
    requestView.runStartTime = Date.now();
    requestView.cancelHandled = false;
    clearSessionBuffers(sessionIdAtStart);
    if (agentPanelUserPref.value !== false) {
      showAgentPanel.value = true;
    }

    let displayMsg = msg;
    if (imageBase64List && imageBase64List.length > 0) {
      const imageHtml = imageBase64List.map(b64 =>
        `<img src="${b64}" style="max-width: 200px; max-height: 200px; border-radius: 8px; margin: 4px 4px 4px 0; display: inline-block; vertical-align: middle;" alt="鐢ㄦ埛鍙戦€佺殑鍥剧墖" />`
      ).join("");
      displayMsg = imageHtml + (msg ? `\n\n${msg}` : "");
    }

    appendSessionHistory(
      sessionIdAtStart,
      `<div class="chat-message user-message" style="position: relative;"><div class="message-content">\n\n${displayMsg}\n\n</div></div>\n\n`
    );

    requestView.lastUserMessage = msg;
    if (sessionIdAtStart === activeSessionId.value) {
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

      const sessionSwitched = sessionIdAtStart !== activeSessionId.value;
      if (!sessionSwitched) {
        setSessionUsageTotals(res.session_input_tokens || 0, res.session_output_tokens || 0);
      }
      requestView.lastUserMessage = msg;

      const checkpoint = requestView.latestCheckpoint as LatestCheckpoint | null;
      const cpId = checkpoint?.id || "";
      const hasOperations = checkpoint?.hasOperations || false;
      const btnTitle = cpId ? "撤回此消息及操作" : "撤回此消息";
      const btnHtml = `<button class="rollback-trigger" data-cp-id="${cpId}" data-has-operations="${hasOperations}" title="${btnTitle}"></button>`;
      const lastIdx = requestView.jarvisResponse.lastIndexOf("</div></div>\n\n");
      if (lastIdx !== -1) {
        requestView.jarvisResponse = requestView.jarvisResponse.slice(0, lastIdx) + btnHtml + requestView.jarvisResponse.slice(lastIdx);
      }
      requestView.latestCheckpoint = null;

      if (res.status === "CANCELLED") {
        if (!requestView.cancelHandled) {
          const hasPartialContent = requestView.contentBuffer || requestView.toolBuffer || requestView.tempBuffer;
          if (hasPartialContent) {
            let partialResponse = `<div class="chat-message agent-message"><div class="message-content">\n\n`;
            if (requestView.toolBuffer) {
              partialResponse += renderToolDetails(requestView.toolBuffer, "done");
            }
            partialResponse += requestView.contentBuffer + requestView.tempBuffer;
            partialResponse += `\n\n<div class="token-usage">用户已取消执行，以上为部分结果</div>\n\n`;
            partialResponse += `\n\n</div></div>\n\n`;
            appendSessionHistory(sessionIdAtStart, partialResponse);
          } else if (res.content && res.content !== "用户已取消执行。") {
            appendSessionHistory(
              sessionIdAtStart,
              `<div class="chat-message agent-message"><div class="message-content">\n\n${res.content}\n\n</div></div>\n\n`
            );
          }
          clearSessionBuffers(sessionIdAtStart);
          removeTrailingUserMessageFromView(sessionIdAtStart);
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
        clearSessionBuffers(sessionIdAtStart);
        appendSessionHistory(
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

      appendSessionHistory(sessionIdAtStart, agentResponse);
      clearSessionBuffers(sessionIdAtStart);
      requestView.showRecallEdit = true;

      requestView.status = res.status;
      if (!sessionSwitched) {
        triggerRender();
        scrollToBottomCb?.();
      }
      await saveAgentStepsToBackend(sessionIdAtStart);
    } catch (err) {
      clearSessionBuffers(sessionIdAtStart);
      
      const btnHtml = `<button class="rollback-trigger" data-cp-id="" data-has-operations="false" title="撤回此消息"></button>`;
      const lastIdx = requestView.jarvisResponse.lastIndexOf("</div></div>\n\n");
      if (lastIdx !== -1) {
        requestView.jarvisResponse = requestView.jarvisResponse.slice(0, lastIdx) + btnHtml + requestView.jarvisResponse.slice(lastIdx);
      }

      appendSessionHistory(sessionIdAtStart, `\n\n**Error:** ${err}`);
      requestView.showRecallEdit = true;

      requestView.status = "ERROR";
      if (sessionIdAtStart === activeSessionId.value) {
        triggerRender();
      }
      await saveAgentStepsToBackend(sessionIdAtStart);
    }
  };

  const cancelJarvis = async (): Promise<string> => {
    const runningSessionId = activeSessionId.value;
    if (!runningSessionId) {
      return "";
    }
    const view = getSessionView(runningSessionId);
    const messageToRestore = view.lastUserMessage;

    if (view.status !== "RUNNING") {
      return messageToRestore;
    }
    view.cancelHandled = false;
    if (runningSessionId) {
      delete permissionRequests.value[runningSessionId];
      delete planProposals.value[runningSessionId];
    }
    try {
      await invoke("cancel_jarvis", { sessionId: runningSessionId });
    } catch (err) {
      console.error("閸欐牗绉锋径杈Е:", err);
    }
    return messageToRestore;

  };

  const recallAndEdit = async (): Promise<string> => {
    try {
      const recalledText = await invoke<string>("recall_last_message", {
        sessionId: activeSessionId.value,
      });
      const view = getSessionView(activeSessionId.value);

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
      await saveAgentStepsToBackend(activeSessionId.value);
      triggerRender();

      return recalledText || "";
    } catch (err) {
      console.error("鎾ゅ洖澶辫触:", err);
      return "";
    }
  };

  const dismissRecallEdit = () => {
    showRecallEdit.value = false;
  };

  const cancelSubAgentRun = async (runId: string) => {
    try {
      const run = await invoke<SubAgentRun>("cancel_subagent_run", { runId });
      subAgentRuns.value = {
        ...subAgentRuns.value,
        [run.runId]: run,
      };
    } catch (err) {
      console.error("鍙栨秷瀛?Agent 澶辫触:", err);
    }
  };

  const resumeAgentRun = async (runId: string) => {
    try {
      const plan = await invoke<{ sessionId: string; prompt: string }>("prepare_resume_agent_run", { runId });
      if (plan.sessionId !== activeSessionId.value) {
        console.warn("鎭㈠鎵ц鐨勪細璇濅笉鏄綋鍓嶄細璇?", plan.sessionId);
        return;
      }
      await sendToJarvis(plan.prompt);
    } catch (err) {
      console.error("鎭㈠鎵ц澶辫触:", err);
    }
  };

  const saveAgentStepsToBackend = async (sessionId: string | null | undefined = activeSessionId.value) => {
    if (!sessionId) return;
    try {
      const steps = getSessionView(sessionId).agentSteps.map(convertFrontendStep);
      await invoke("save_agent_steps", { steps, sessionId });
    } catch (err) {
      console.error("淇濆瓨鎵ц娴佺▼澶辫触:", err);
    }
  };

  const loadAgentStepsFromBackend = async (sessionId: string | null | undefined = activeSessionId.value) => {
    if (!sessionId) return;
    try {
      const steps = await invoke<BackendAgentStep[]>("get_agent_steps", { sessionId });
      replaceSessionAgentSteps(sessionId, steps.map(convertBackendStep));
    } catch (err) {
      console.error("鍔犺浇鎵ц娴佺▼澶辫触:", err);
      replaceSessionAgentSteps(sessionId, []);
    }
  };

  const loadSubAgentRunsFromBackend = async (sessionId: string | null | undefined = activeSessionId.value) => {
    if (!sessionId) return;
    try {
      const runs = await invoke<SubAgentRun[]>("list_subagents", { sessionId });
      const otherRuns = Object.fromEntries(
        Object.entries(subAgentRuns.value).filter(([, run]) => run.sessionId !== sessionId)
      );
      subAgentRuns.value = {
        ...otherRuns,
        ...Object.fromEntries(runs.map((run) => [run.runId, run])),
      };
    } catch (err) {
      console.error("閸旂姾娴囩€涙劒鍞悶鍡欐磧閹貉呭Ц閹礁銇戠拹:", err);
    }
  };

  const loadSubAgentEventsFromBackend = async (sessionId: string | null | undefined = activeSessionId.value) => {
    if (!sessionId) return;
    try {
      const events = await invoke<SubAgentEvent[]>("list_subagent_events", { sessionId, runId: null });
      const grouped = events.reduce<Record<string, SubAgentEvent[]>>((acc, item) => {
        if (!acc[item.runId]) acc[item.runId] = [];
        acc[item.runId].push(item);
        return acc;
      }, {});
      for (const runEvents of Object.values(grouped)) {
        runEvents.sort((a, b) => a.timestamp - b.timestamp);
      }
      const otherEvents = Object.fromEntries(
        Object.entries(subAgentEventsByRun.value).filter(([, runEvents]) => {
          return runEvents.length > 0 && runEvents[0].sessionId !== sessionId;
        })
      );
      subAgentEventsByRun.value = {
        ...otherEvents,
        ...grouped,
      };
    } catch (err) {
      console.error("鍔犺浇瀛?Agent 浜嬩欢鍘嗗彶澶辫触:", err);
    }
  };

  const loadPlanDocumentsFromBackend = async (sessionId: string | null | undefined = activeSessionId.value) => {
    if (!sessionId) return;
    try {
      const documents = await invoke<PlanDocument[]>("list_plan_documents", { sessionId });
      planDocumentsBySession.value = {
        ...planDocumentsBySession.value,
        [sessionId]: documents,
      };
    } catch (err) {
      console.error("鍔犺浇璁″垝鏂囨。澶辫触:", err);
      planDocumentsBySession.value = {
        ...planDocumentsBySession.value,
        [sessionId]: [],
      };
    }
  };

  const loadAgentRunsFromBackend = async (sessionId: string | null | undefined = activeSessionId.value) => {
    if (!sessionId) return;
    try {
      const runs = await invoke<AgentRun[]>("list_agent_runs", { sessionId });
      const otherRuns = Object.fromEntries(
        Object.entries(agentRuns.value).filter(([, run]) => run.sessionId !== sessionId)
      );
      agentRuns.value = {
        ...otherRuns,
        ...Object.fromEntries(runs.map((run) => [run.runId, run])),
      };
    } catch (err) {
      console.error("鍔犺浇涓?Agent 鎵ц璁板綍澶辫触:", err);
    }
  };

  const loadAgentRunEventsFromBackend = async (sessionId: string | null | undefined = activeSessionId.value) => {
    if (!sessionId) return;
    try {
      const events = await invoke<AgentRunEvent[]>("list_agent_run_events", { sessionId, runId: null });
      const grouped = events.reduce<Record<string, AgentRunEvent[]>>((acc, item) => {
        if (!acc[item.runId]) acc[item.runId] = [];
        acc[item.runId].push(item);
        return acc;
      }, {});
      for (const runEvents of Object.values(grouped)) {
        runEvents.sort((a, b) => a.timestamp - b.timestamp);
      }
      const otherEvents = Object.fromEntries(
        Object.entries(agentRunEventsByRun.value).filter(([, runEvents]) => {
          return runEvents.length > 0 && runEvents[0].sessionId !== sessionId;
        })
      );
      agentRunEventsByRun.value = {
        ...otherEvents,
        ...grouped,
      };
    } catch (err) {
      console.error("鍔犺浇涓?Agent 浜嬩欢鍘嗗彶澶辫触:", err);
    }
  };

  return {
    jarvisResponse,
    messages,
    toolBuffer,
    contentBuffer,
    tempBuffer,
    isCurrentSessionRunning,
    isAnySessionRunning,
    currentSessionStatus,
    thinkingBuffer,
    latestCheckpoint,
    todos,
    permissionRequest,
    planProposal,
    currentPlanDocuments,
    totalInputTokens,
    totalOutputTokens,
    setSessionUsageTotals,
    parsedHistory,
    parsedCurrentTurnHtml,
    lastUserMessage,
    showRecallEdit,
    agentSteps,
    currentAgentRuns,
    interruptedAgentRuns,
    agentRunEventsByRun,
    getAgentRunEvents,
    currentSubAgentRuns,
    activeSubAgentRuns,
    subAgentEventsByRun,
    getSubAgentEvents,
    focusedTaskId,
    focusTask,
    showAgentPanel,
    activeSessionId,
    workingDirectory,
    rollbackRecalledMessage,
    initListeners,
    resolvePermission,
    resolvePlan,
    updatePlanProposalContent,
    sendToJarvis,
    cancelJarvis,
    cancelSubAgentRun,
    resumeAgentRun,
    recallAndEdit,
    dismissRecallEdit,
    saveAgentStepsToBackend,
    loadAgentStepsFromBackend,
    loadAgentRunsFromBackend,
    loadAgentRunEventsFromBackend,
    loadSubAgentRunsFromBackend,
    loadSubAgentEventsFromBackend,
    loadPlanDocumentsFromBackend,
    hasHydratedSessionView,
    replaceSessionHistory,
    resetSessionView,
    deleteSessionView,
    sessionViews,
    getSessionView,
  };
}
