import { marked } from "marked";
import type { AgentToolStatus } from "../types";
import { i18n } from "../i18n";
import { toolActionLabel, toolCategoryLabel } from "./toolDisplay";

const t = i18n.global.t;

marked.use({
  renderer: {
    code(token: any) {
      const language = String(token.lang || "").match(/\S+/)?.[0] || "";
      const languageClass = language ? ` class="language-${escapeHtmlForAttr(language)}"` : "";
      const languageLabel = language || "code";
      const code = escapeHtml(String(token.text || "").replace(/\n$/, ""));

      return `<div class="markdown-code-block">
<div class="markdown-code-header">
<span class="markdown-code-language">${escapeHtml(languageLabel)}</span>
<button type="button" class="markdown-copy-btn code-copy-btn" title="${escapeHtmlForAttr(t('execution.copyCode'))}" aria-label="${escapeHtmlForAttr(t('execution.copyCode'))}">${escapeHtml(t('common.copy'))}</button>
</div>
<pre><code${languageClass}>${code}</code></pre>
</div>
`;
    },
    table(this: any, token: any) {
      const renderCell = (cell: any) => {
        const tag = cell.header ? "th" : "td";
        const align = cell.align ? ` align="${escapeHtmlForAttr(String(cell.align))}"` : "";
        const content = this.parser.parseInline(cell.tokens || []);
        return `<${tag}${align}>${content}</${tag}>
`;
      };
      const header = token.header.map(renderCell).join("");
      const rows = token.rows
        .map((row: any[]) => `<tr>
${row.map(renderCell).join("")}</tr>
`)
        .join("");
      const body = rows ? `<tbody>
${rows}</tbody>
` : "";

      return `<div class="markdown-table-wrap">
<div class="markdown-table-header">
<span>${escapeHtml(t('execution.table'))}</span>
<button type="button" class="markdown-copy-btn table-copy-btn" title="${escapeHtmlForAttr(t('execution.copyTable'))}" aria-label="${escapeHtmlForAttr(t('execution.copyTable'))}">${escapeHtml(t('common.copy'))}</button>
</div>
<div class="markdown-table-scroll">
<table>
<thead>
<tr>
${header}</tr>
</thead>
${body}</table>
</div>
</div>
`;
    },
  },
});

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
      ? t('execution.toolDetailsSummary')
      : t('execution.toolDetailsSummaryDone');
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
  const safeStatus = normalizeToolStatus(status);
  const title = safeStatus === "completed" ? t('execution.completed') : safeStatus === "error" ? t('execution.error') : t('execution.running');
  const safeCategory = escapeHtml(toolCategoryLabel(tool));
  const safeAction = escapeHtml(toolActionLabel(tool, safeStatus));
  return `<div class="tool-status-line ${escapeHtmlForAttr(safeStatus)}" data-tool-call-id="${safeId}" title="${title}">${renderToolStatusIcon(safeStatus)}<span class="tool-status-title">${safeCategory}</span><span>${safeAction}</span></div>`;
}

function normalizeToolStatus(status: string): AgentToolStatus {
  if (status === "pending" || status === "running" || status === "completed" || status === "error") {
    return status;
  }
  return "running";
}

function escapeHtmlForAttr(value: string) {
  return value
    .replace(/&/g, "&amp;")
    .replace(/</g, "&lt;")
    .replace(/>/g, "&gt;")
    .replace(/"/g, "&quot;");
}

function escapeHtml(value: string) {
  return value
    .replace(/&/g, "&amp;")
    .replace(/</g, "&lt;")
    .replace(/>/g, "&gt;")
    .replace(/"/g, "&quot;")
    .replace(/'/g, "&#39;");
}

export function renderTokenUsage(
  inputTokens: number,
  outputTokens: number,
  sessionInputTokens?: number,
  sessionOutputTokens?: number,
) {
  const sessionPart =
    sessionInputTokens !== undefined && sessionOutputTokens !== undefined
      ? ` &nbsp;&nbsp;|&nbsp;&nbsp; <b>${escapeHtml(t('execution.sessionUsage'))}</b>: ${escapeHtml(t('execution.input'))} ${sessionInputTokens || 0} / ${escapeHtml(t('execution.outputToken'))} ${sessionOutputTokens || 0} Token`
      : "";
  return `<div class="token-usage"><b>${escapeHtml(t('execution.tokenUsage'))}</b>: ${escapeHtml(t('execution.input'))} ${inputTokens || 0} / ${escapeHtml(t('execution.outputToken'))} ${outputTokens || 0} Token${sessionPart}</div>`;
}
