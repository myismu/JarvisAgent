import { marked } from "marked";

marked.setOptions({
  breaks: true,
  gfm: true,
});

export function renderMarkdown(value: string) {
  return marked.parse(value || "") as string;
}

const toolDetailsIcon = `<svg viewBox="0 0 24 24" width="14" height="14" stroke="currentColor" stroke-width="2" fill="none" stroke-linecap="round" stroke-linejoin="round" style="vertical-align: text-bottom; margin-right: 4px;"><circle cx="12" cy="12" r="3"></circle><path d="M12 2v3"></path><path d="M12 19v3"></path><path d="M4.93 4.93l2.12 2.12"></path><path d="M16.95 16.95l2.12 2.12"></path><path d="M2 12h3"></path><path d="M19 12h3"></path><path d="M4.93 19.07l2.12-2.12"></path><path d="M16.95 7.05l2.12-2.12"></path></svg>`;

export function renderToolDetails(content: string, mode: "live" | "done", open = false) {
  const summary =
    mode === "live"
      ? "贾维斯正在思考与执行操作... (点击查看详情)"
      : "贾维斯已完成思考与操作 (点击查看完整决策链)";
  const body = content.trim() ? renderMarkdown(content) : "";
  return `\n\n<details ${open ? "open" : ""}>\n<summary>${toolDetailsIcon}${summary}</summary>\n\n${body}\n\n</details>\n\n`;
}

export function renderToolStatusIcon(status: string) {
  if (status === "completed") {
    return `<svg class="tool-status-icon completed" viewBox="0 0 24 24" width="14" height="14" fill="none" stroke="currentColor" stroke-width="2.2" stroke-linecap="round" stroke-linejoin="round"><polyline points="20 6 9 17 4 12"></polyline></svg>`;
  }
  if (status === "error") {
    return `<svg class="tool-status-icon error" viewBox="0 0 24 24" width="14" height="14" fill="none" stroke="currentColor" stroke-width="2.2" stroke-linecap="round" stroke-linejoin="round"><circle cx="12" cy="12" r="9"></circle><line x1="12" y1="8" x2="12" y2="12"></line><line x1="12" y1="16" x2="12.01" y2="16"></line></svg>`;
  }
  return `<svg class="tool-status-icon running" viewBox="0 0 24 24" width="14" height="14" fill="none" stroke="currentColor" stroke-width="2.2" stroke-linecap="round"><circle class="tool-status-track" cx="12" cy="12" r="9"></circle><path class="tool-status-head" d="M21 12a9 9 0 0 0-9-9"></path></svg>`;
}

export function renderToolStatusLine(toolCallId: string, tool: string, status: string) {
  const safeId = escapeHtmlForAttr(toolCallId);
  const safeTool = escapeHtmlForAttr(tool);
  const title = status === "completed" ? "工具执行完成" : status === "error" ? "工具执行失败" : "工具执行中";
  return `<div class="tool-status-line ${escapeHtmlForAttr(status)}" data-tool-call-id="${safeId}" title="${title}">${renderToolStatusIcon(status)}<code>${safeTool}</code></div>`;
}

function escapeHtmlForAttr(value: string) {
  return value
    .replace(/&/g, "&amp;")
    .replace(/</g, "&lt;")
    .replace(/>/g, "&gt;")
    .replace(/"/g, "&quot;");
}

export function renderTokenUsage(
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

export function renderStoredHistory(history: string, READY_TEXT: string) {
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
