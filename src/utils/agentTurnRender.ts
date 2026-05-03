import type {
  AgentCurrentTurn,
  AgentDisplayMode,
  AgentExecutionLog,
  AgentTextBlock,
  AgentThinkingBlock,
  AgentToolCallView,
  AgentTurnSnapshot,
} from "../types";
import { renderMarkdown, renderTokenUsage, renderToolStatusIcon } from "./markdown";
import {
  canMergeToolGroups,
  createToolCallGroup,
  groupAdjacentToolCalls,
  hasToolDetails,
  isSubAgentToolGroup,
  mergeToolGroups,
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

function snapshotFromTurn(turn: AgentCurrentTurn): AgentTurnSnapshot {
  return {
    version: 1,
    status: turn.isRunning ? "RUNNING" : "FINISH",
    textBlocks: turn.textBlocks,
    thinkingBlocks: turn.thinkingBlocks,
    toolCalls: turn.toolCalls,
    logs: turn.logs,
    createdAt: turn.startedAt ?? Date.now(),
  };
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

function renderTextBlock(block: AgentTextBlock) {
  const content = stripPseudoToolCalls(block.content);
  return content.trim()
    ? `<div class="agent-turn-answer agent-developer-text">${renderMarkdown(content)}</div>`
    : "";
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

function renderExecutionLog(log: AgentExecutionLog) {
  if (!log.content.trim()) return "";
  return `<details class="agent-execution-logs">
<summary>执行日志 · 第 ${log.loop || 1} 轮</summary>
<div class="agent-execution-log">${renderMarkdown(log.content)}</div>
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
  return `<details class="agent-execution-panel ${mode}" ${mode === "developer" || live ? "open" : ""}>
<summary>
  <svg viewBox="0 0 24 24" width="14" height="14" stroke="currentColor" stroke-width="2" fill="none" stroke-linecap="round" stroke-linejoin="round"><circle cx="12" cy="12" r="3"></circle><path d="M12 2v3"></path><path d="M12 19v3"></path><path d="M4.93 4.93l2.12 2.12"></path><path d="M16.95 16.95l2.12 2.12"></path><path d="M2 12h3"></path><path d="M19 12h3"></path><path d="M4.93 19.07l2.12-2.12"></path><path d="M16.95 7.05l2.12-2.12"></path></svg>
  ${escapeHtml(summary)}
</summary>
${body}
</details>`;
}

function renderDeveloperTimeline(snapshot: AgentTurnSnapshot) {
  const segments: Array<{
    timestamp: number;
    order: number;
    html?: string;
    group?: ToolCallGroup;
    type: "html" | "tool";
  }> = [];

  snapshot.textBlocks.forEach((block, index) => {
    if (block.kind !== "assistant") return;
    const html = renderTextBlock(block);
    if (!html) return;
    segments.push({ timestamp: block.timestamp, order: index * 4, html, type: "html" });
  });

  snapshot.thinkingBlocks.forEach((block, index) => {
    const html = renderThinkingBlock(block, block.status === "streaming");
    if (!html) return;
    segments.push({ timestamp: block.timestamp, order: index * 4 + 1, html, type: "html" });
  });

  snapshot.toolCalls.forEach((tool, index) => {
    const group = createToolCallGroup([tool]);
    segments.push({ timestamp: group.timestamp, order: index * 4 + 2, group, type: "tool" });
  });

  snapshot.logs.forEach((log, index) => {
    const html = renderExecutionLog(log);
    if (!html) return;
    segments.push({ timestamp: log.timestamp, order: index * 4 + 3, html, type: "html" });
  });

  const merged: typeof segments = [];
  segments
    .sort((a, b) => a.timestamp - b.timestamp || a.order - b.order)
    .forEach((segment) => {
      const previous = merged[merged.length - 1];
      if (
        segment.type === "tool" &&
        segment.group &&
        previous?.type === "tool" &&
        previous.group &&
        canMergeToolGroups(previous.group, segment.group)
      ) {
        previous.group = mergeToolGroups(previous.group, segment.group);
        previous.timestamp = previous.group.timestamp;
        return;
      }
      merged.push(segment);
    });

  const body = merged
    .map((segment) => (segment.type === "tool" && segment.group ? renderToolGroup(segment.group, "developer") : segment.html || ""))
    .join("");

  return body ? `<div class="agent-developer-timeline">${body}</div>` : "";
}

export function renderAgentTurnSnapshot(
  snapshot: AgentTurnSnapshot,
  displayMode: AgentDisplayMode,
  live = false,
) {
  const answerHtml = displayMode === "developer" ? "" : renderTextBlocks(snapshot.textBlocks);
  const executionHtml =
    displayMode === "developer"
      ? renderDeveloperTimeline(snapshot)
      : renderExecutionPanel(snapshot, displayMode, live);
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
  return `<div class="agent-turn-render ${displayMode}">
${executionHtml}
${answerHtml}
${tokenHtml}
${noticeHtml}
</div>`;
}

export function renderAgentCurrentTurn(
  turn: AgentCurrentTurn,
  displayMode: AgentDisplayMode,
) {
  return renderAgentTurnSnapshot(snapshotFromTurn(turn), displayMode, turn.isRunning);
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
