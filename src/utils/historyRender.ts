import type { AgentDisplayMode } from "../types";
import { renderMarkdown } from "./markdown";
import { extractAgentTurnSnapshot, renderAgentTurnSnapshot } from "./agentTurnRender";

export function renderStoredHistory(
  history: string,
  READY_TEXT: string,
  displayMode: AgentDisplayMode = "user",
) {
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
    rendered += renderHistoryMessageBlock(history.slice(start, end), displayMode);
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

function findMessageContentDivStart(html: string) {
  const divPattern = /<div\b[^>]*\bclass=(["'])(?=[^"']*\bmessage-content\b)[^"']*\1[^>]*>/gi;
  const match = divPattern.exec(html);
  return match?.index ?? -1;
}

function renderHistoryMessageBlock(block: string, displayMode: AgentDisplayMode) {
  const contentStart = findMessageContentDivStart(block);
  if (contentStart === -1) return block;

  const openEnd = block.indexOf(">", contentStart);
  const closeStart = findMatchingDivClose(block, contentStart);
  if (openEnd === -1 || closeStart === -1 || closeStart <= openEnd) {
    return renderMarkdown(block);
  }

  const beforeContent = block.slice(0, openEnd + 1);
  const rawContent = block.slice(openEnd + 1, closeStart);
  const afterContent = block.slice(closeStart);
  const structured = extractAgentTurnSnapshot(rawContent);

  if (structured) {
    return `${beforeContent}${renderAgentTurnSnapshot(structured, displayMode, false)}${afterContent}`;
  }

  return `${beforeContent}${renderMarkdown(rawContent)}${afterContent}`;
}
