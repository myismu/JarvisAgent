import type {
  AgentDisplayMode,
  AgentExecutionLog,
  AgentTextBlock,
  AgentThinkingBlock,
  AgentToolCallView,
  AgentTurnSnapshot,
} from "../types";
import { renderMarkdown, renderTokenUsage, renderToolStatusIcon } from "./markdown";
import {
  groupAdjacentToolCalls,
  hasToolDetails,
  isSubAgentToolGroup,
  shouldOpenToolGroup,
  summarizeToolGroupsForPanel,
  toolActionCountLabel,
  toolActionLabel,
  toolGroupActionLabel,
  toolGroupTitle,
  type ToolCallGroup,
} from "./toolDisplay";

export const PSEUDO_TOOL_CALL_RE = /(?:<tool_call>\s*)?<function=[\s\S]*$/;

export function stripPseudoToolCalls(content: string) {
  return content.replace(PSEUDO_TOOL_CALL_RE, "").trimEnd();
}

function escapeHtml(value: string) {
  return value
    .replace(/&/g, "&amp;")
    .replace(/</g, "&lt;")
    .replace(/>/g, "&gt;")
    .replace(/"/g, "&quot;")
    .replace(/'/g, "&#39;");
}

function describeThinking(content: string) {
  const text = content.replace(/\s+/g, " ").trim();
  const lower = text.toLowerCase();

  if (!text) return "分析当前步骤";
  if (/(方案|计划|审批|plan|proposal)/i.test(text)) return "制定方案与审批策略";
  if (/(工具|调用|tool|function|参数)/i.test(text)) return "选择工具并整理调用参数";
  if (/(文件|目录|代码|实现|修改|file|code|implement)/i.test(text)) return "分析代码与实现路径";
  if (/(错误|失败|修复|bug|error|fix)/i.test(text)) return "定位问题与修复思路";
  if (/(测试|验证|build|check|test)/i.test(text)) return "规划验证与测试方式";
  if (lower.includes("user") || text.includes("用户")) return "理解用户需求与约束";

  const sentence = text.split(/[。.!?？；;]/)[0]?.trim() || text;
  return sentence.length > 28 ? `${sentence.slice(0, 28)}...` : sentence;
}

function renderTextBlocks(blocks: AgentTextBlock[]) {
  const text = stripPseudoToolCalls(
    blocks
      .filter((block) => block.kind === "assistant")
      .map((block) => block.content)
      .join(""),
  );
  return text.trim() ? `<div class="agent-turn-answer">${renderMarkdown(text)}</div>` : "";
}

function renderThinkingBlock(block: AgentThinkingBlock, open: boolean) {
  const content = block.content.trim();
  if (!content) return "";
  const label = block.status === "streaming" ? "思考中" : "思考结果";
  return `<details class="agent-thinking-block" ${open ? "open" : ""}>
<summary>${label} · ${escapeHtml(describeThinking(content))} · 第 ${block.loop || 1} 轮</summary>
${renderMarkdown(content)}
</details>`;
}

function renderToolDetailHtml(tool: AgentToolCallView) {
  return [
    tool.inputSummary
      ? `<div class="agent-tool-field"><span>参数</span>${renderMarkdown(tool.inputSummary)}</div>`
      : "",
    tool.outputSummary
      ? `<div class="agent-tool-field"><span>输出</span>${renderMarkdown(tool.outputSummary)}</div>`
      : "",
    tool.error ? `<div class="agent-tool-field error"><span>错误</span>${renderMarkdown(tool.error)}</div>` : "",
    ...tool.logs.map((log) => `<div class="agent-tool-log">${renderMarkdown(log)}</div>`),
  ].join("");
}

function renderToolRow(tool: AgentToolCallView) {
  return `<span class="agent-tool-child-row ${escapeHtml(tool.status)}">
${renderToolStatusIcon(tool.status)}
<span>${escapeHtml(toolActionLabel(tool.name, tool.status, tool))}</span>
<code>${escapeHtml(tool.name || "")}</code>
</span>`;
}

function renderToolGroupRow(group: ToolCallGroup) {
  return `<span class="agent-tool-row ${escapeHtml(group.status)}">
${renderToolStatusIcon(group.status)}
<span class="agent-tool-title">${escapeHtml(toolGroupTitle(group))}</span>
<span>${escapeHtml(toolGroupActionLabel(group))}</span>
</span>`;
}

function renderToolActionRows(group: ToolCallGroup) {
  return `<div class="agent-tool-action-list">
${group.actions
  .map(
    (action) => `<div class="agent-tool-action-row ${escapeHtml(action.status)}">
${renderToolStatusIcon(action.status)}
<span>${escapeHtml(toolActionCountLabel(action))}</span>
<span class="agent-tool-action-summary">${escapeHtml(action.summary)}</span>
</div>`,
  )
  .join("")}
</div>`;
}

function renderToolGroup(group: ToolCallGroup, mode: AgentDisplayMode) {
  const open = shouldOpenToolGroup(group, mode);
  const groupClass = [
    "agent-tool-call",
    group.count > 1 ? "agent-tool-group" : "",
    isSubAgentToolGroup(group) ? "agent-subagent-tool" : "",
  ]
    .filter(Boolean)
    .join(" ");

  const children = group.tools
    .map((tool) => {
      const detailHtml = renderToolDetailHtml(tool);
      return `<div class="agent-tool-child${hasToolDetails(tool) ? " has-details" : ""}">
${renderToolRow(tool)}
${detailHtml}
</div>`;
    })
    .join("");

  return `<details class="${groupClass}" ${open ? "open" : ""}>
<summary>${renderToolGroupRow(group)}</summary>
${renderToolActionRows(group)}
<details class="agent-tool-technical" ${group.status === "error" ? "open" : ""}>
<summary>技术详情 · ${group.count} 步</summary>
<div class="agent-tool-group-items">
${children}
</div>
</details>
</details>`;
}

function renderExecutionLogs(logs: AgentExecutionLog[], open: boolean) {
  if (!logs.some((log) => log.content.trim())) return "";
  return `<details class="agent-execution-logs" ${open ? "open" : ""}>
<summary>执行日志 · ${logs.length} 条</summary>
${logs.map((log) => `<div class="agent-execution-log">${renderMarkdown(log.content)}</div>`).join("")}
</details>`;
}

function renderExecutionBody(snapshot: AgentTurnSnapshot, mode: AgentDisplayMode) {
  const thinkingOpen = mode === "developer";
  const toolHtml = groupAdjacentToolCalls(snapshot.toolCalls)
    .map((group) => renderToolGroup(group, mode))
    .join("");
  const thinkingHtml = snapshot.thinkingBlocks.map((block) => renderThinkingBlock(block, thinkingOpen)).join("");
  const logsHtml = renderExecutionLogs(snapshot.logs, mode === "developer");
  return [toolHtml, thinkingHtml, logsHtml].filter(Boolean).join("");
}

function renderExecutionPanel(snapshot: AgentTurnSnapshot, mode: AgentDisplayMode, live: boolean) {
  const thinkingCount = snapshot.thinkingBlocks.filter((block) => block.content.trim()).length;
  const toolCount = snapshot.toolCalls.length;
  const toolGroups = groupAdjacentToolCalls(snapshot.toolCalls);
  const logCount = snapshot.logs.filter((log) => log.content.trim()).length;
  if (thinkingCount + toolCount + logCount === 0) return "";

  const state =
    live || snapshot.status === "RUNNING"
      ? "处理中"
      : snapshot.status === "CANCELLED"
        ? "已取消"
        : snapshot.status === "INTERRUPTED"
          ? "已中断"
          : snapshot.status === "ERROR"
            ? "失败"
            : "已完成";
  const summary =
    mode === "developer"
      ? `${state} · ${toolCount > 0 ? summarizeToolGroupsForPanel(toolGroups, toolCount) : "无工具活动"} · ${thinkingCount} 段思考 · ${logCount} 条日志`
      : `${state} · ${toolCount > 0 ? summarizeToolGroupsForPanel(toolGroups, toolCount) : "无工具活动"}${thinkingCount > 0 ? ` · ${thinkingCount} 段思考` : ""}`;

  const body = renderExecutionBody(snapshot, mode);
  return `<details class="agent-execution-panel ${mode}" ${live ? "open" : ""}>
<summary>
  <svg viewBox="0 0 24 24" width="14" height="14" stroke="currentColor" stroke-width="2" fill="none" stroke-linecap="round" stroke-linejoin="round"><circle cx="12" cy="12" r="3"></circle><path d="M12 2v3"></path><path d="M12 19v3"></path><path d="M4.93 4.93l2.12 2.12"></path><path d="M16.95 16.95l2.12 2.12"></path><path d="M2 12h3"></path><path d="M19 12h3"></path><path d="M4.93 19.07l2.12-2.12"></path><path d="M16.95 7.05l2.12-2.12"></path></svg>
  ${escapeHtml(summary)}
</summary>
${body}
</details>`;
}

export function describeThinkingStatic(content: string) {
  const text = content.replace(/\s+/g, " ").trim();
  if (!text) return "分析中...";
  if (/(方案|计划|审批|plan|proposal)/i.test(text)) return "制定方案";
  if (/(工具|调用|tool|function|参数)/i.test(text)) return "选择工具";
  if (/(文件|目录|代码|实现|修改|file|code|implement)/i.test(text)) return "分析代码";
  if (/(错误|失败|修复|bug|error|fix)/i.test(text)) return "定位问题";
  if (/(测试|验证|build|check|test)/i.test(text)) return "规划验证";
  const sentence = text.split(/[。.!?？；;]/)[0]?.trim() || text;
  return sentence.length > 20 ? `${sentence.slice(0, 20)}...` : sentence;
}


export function renderAgentTurnSnapshot(
  snapshot: AgentTurnSnapshot,
  displayMode: AgentDisplayMode,
  live = false,
) {
  const tokenHtml = snapshot.tokens
    ? renderTokenUsage(
        snapshot.tokens.input,
        snapshot.tokens.output,
        snapshot.tokens.sessionInput,
        snapshot.tokens.sessionOutput,
      )
    : "";

  const noticeHtml = snapshot.notice
    ? `<div class="token-usage">${escapeHtml(snapshot.notice)}</div>`
    : "";

  // 开发者模式：所有块按时间戳交错渲染
  if (displayMode === "developer") {
    return renderDeveloperTimeline(snapshot, tokenHtml, noticeHtml, live);
  }

  // 用户模式：execution 面板 + 回答文本
  const answerHtml = snapshot.toolCalls.length > 0 ? "" : renderTextBlocks(snapshot.textBlocks);
  const executionHtml = renderExecutionPanel(snapshot, displayMode, live);

  return `<div class="agent-turn-render ${displayMode}">
${executionHtml}
${answerHtml}
${tokenHtml}
${noticeHtml}
</div>`;
}

export type DevTimelineItem =
  | { type: "text"; timestamp: number; content: string }
  | { type: "thinking"; timestamp: number; content: string; status: string; streaming: boolean }
  | { type: "tool"; timestamp: number; tool: AgentToolCallView; streaming: boolean }
  | { type: "log"; timestamp: number; content: string; loop: number };

/** 从快照数据构建开发者时间线（按时间戳交错排序），直播和历史共用 */
export function buildDeveloperTimeline(
  snapshot: {
    textBlocks: { content: string; timestamp: number }[];
    thinkingBlocks: { content: string; status: string; timestamp: number }[];
    toolCalls: AgentToolCallView[];
    logs: AgentExecutionLog[];
  },
  live: boolean,
): DevTimelineItem[] {
  const timeline: DevTimelineItem[] = [];

  snapshot.textBlocks.forEach((block) => {
    if (!block.content.trim()) return;
    timeline.push({ type: "text", timestamp: block.timestamp, content: block.content.trim() });
  });

  snapshot.thinkingBlocks.forEach((block) => {
    if (!block.content.trim()) return;
    timeline.push({
      type: "thinking",
      timestamp: block.timestamp,
      content: block.content,
      status: block.status,
      streaming: live && block.status === "streaming",
    });
  });

  snapshot.toolCalls.forEach((tool) => {
    timeline.push({
      type: "tool",
      timestamp: tool.timestamp,
      tool,
      streaming: live && (tool.status === "running" || tool.status === "error"),
    });
  });

  snapshot.logs.forEach((log) => {
    if (!log.content.trim()) return;
    timeline.push({
      type: "log",
      timestamp: log.timestamp,
      content: log.content,
      loop: log.loop || 1,
    });
  });

  timeline.sort((a, b) => a.timestamp - b.timestamp);
  return timeline;
}

function splitTimeline(timeline: DevTimelineItem[]) {
  let lastTextIdx = -1;
  for (let i = timeline.length - 1; i >= 0; i--) {
    if (timeline[i].type === "text") {
      lastTextIdx = i;
      break;
    }
  }
  if (lastTextIdx < 0) return { execItems: timeline, finalItems: [] };

  const hasLaterToolOrLog = timeline.slice(lastTextIdx + 1).some((item) => item.type === "tool" || item.type === "log");
  if (hasLaterToolOrLog) return { execItems: timeline, finalItems: [] };

  const execItems: DevTimelineItem[] = [];
  const finalItems: DevTimelineItem[] = [];
  for (let i = 0; i < timeline.length; i++) {
    if (i === lastTextIdx) finalItems.push(timeline[i]);
    else execItems.push(timeline[i]);
  }
  return { execItems, finalItems };
}

function renderDevItemToHtml(item: DevTimelineItem): string {
  switch (item.type) {
    case "text":
      return renderMarkdown(item.content);
    case "thinking": {
      const open = item.streaming;
      return `<details class="dev-thinking${open ? " streaming" : ""}" ${open ? "open" : ""}>
<summary class="dev-thinking-summary">
  <span class="dev-status-dot${open ? " running" : ""}"></span>
  <span class="dev-thinking-label">${escapeHtml(describeThinkingStatic(item.content))}</span>
</summary>
<div class="dev-thinking-body">${renderMarkdown(item.content)}</div>
</details>`;
    }
    case "tool": {
      const tool = item.tool;
      const statusLabel = tool.status === "completed" ? "完成" : tool.status === "running" ? "执行中" : tool.status === "error" ? "失败" : "";
      const label = toolActionLabel(tool.name, tool.status, tool);
      const open = item.streaming;

      const paramsHtml = tool.inputSummary
        ? `<div class="dev-tool-section"><div class="dev-tool-section-label">参数</div><pre class="dev-tool-pre">${escapeHtml(tool.inputSummary)}</pre></div>`
        : "";
      const outputHtml = tool.outputSummary
        ? `<div class="dev-tool-section"><div class="dev-tool-section-label">输出</div><pre class="dev-tool-pre">${escapeHtml(tool.outputSummary)}</pre></div>`
        : "";
      const errorHtml = tool.error
        ? `<div class="dev-tool-section error"><div class="dev-tool-section-label">错误</div><pre class="dev-tool-pre">${escapeHtml(tool.error)}</pre></div>`
        : "";
      const bodyHtml = paramsHtml + outputHtml + errorHtml;

      return `<details class="dev-tool ${escapeHtml(tool.status)}" ${open ? "open" : ""}>
<summary class="dev-tool-summary">
  <span class="dev-status-dot ${escapeHtml(tool.status)}"></span>
  <code class="dev-tool-name">${escapeHtml(tool.name)}</code>
  <span class="dev-tool-action">${escapeHtml(label)}</span>
  <span class="dev-tool-status">${escapeHtml(statusLabel)}</span>
</summary>
${bodyHtml ? `<div class="dev-tool-body">${bodyHtml}</div>` : ""}
</details>`;
    }
    case "log":
      return `<div class="dev-log">
<div class="dev-log-header">
  <span class="dev-log-dot red"></span>
  <span class="dev-log-dot yellow"></span>
  <span class="dev-log-dot green"></span>
  <span class="dev-log-title">输出 #${item.loop || 1}</span>
</div>
<div class="dev-log-body">${renderMarkdown(item.content)}</div>
</div>`;
  }
}

function renderDeveloperTimeline(
  snapshot: AgentTurnSnapshot,
  tokenHtml: string,
  noticeHtml: string,
  live = false,
) {
  const timeline = buildDeveloperTimeline(snapshot, live);
  const { execItems, finalItems } = splitTimeline(timeline);

  const execHtml =
    execItems.length > 0
      ? `<details class="dev-execution-fold">
<summary class="dev-execution-summary">执行过程（${execItems.length} 步）</summary>
<div class="dev-layout">${execItems.map(renderDevItemToHtml).join("")}</div>
</details>`
      : "";

  const finalHtml = finalItems.map(renderDevItemToHtml).join("");

  return `<div class="agent-turn-render ${"developer"}">
${execHtml}
${finalHtml}
${tokenHtml}
${noticeHtml}
</div>`;
}

export function serializeAgentTurnSnapshot(snapshot: AgentTurnSnapshot) {
  const json = JSON.stringify(snapshot).replace(/</g, "\\u003c");
  return `<script type="application/json" class="agent-turn-data">${json}</script>`;
}

export function extractAgentTurnSnapshot(rawContent: string): AgentTurnSnapshot | null {
  const match = rawContent.match(
    /<script\b(?=[^>]*\bclass=(["'])(?=[^"']*\bagent-turn-data\b)[^"']*\1)[^>]*>([\s\S]*?)<\/script>/i,
  );
  if (!match) return null;
  try {
    const parsed = JSON.parse(match[2]);
    if (parsed?.version === 1) {
      return parsed as AgentTurnSnapshot;
    }
  } catch {
    return null;
  }
  return null;
}
