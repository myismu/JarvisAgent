import { marked } from "marked";
import { i18n } from "../i18n";

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

/**
 * Markdown 渲染的唯一入口。
 *
 * 这里曾经还导出 `renderToolDetails` / `renderToolStatusIcon` / `renderToolStatusLine` /
 * `renderTokenUsage` 四个「HTML 字符串」工具（供 `utils/agentTurnRender.ts` 的旧渲染路径使用），
 * 那套路径没有调用方，已随死代码清理删除；工具调用与 token 用量现在由 Vue 组件渲染。
 */
export function renderMarkdown(value: string) {
  return marked.parse(value || "") as string;
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
