import { marked } from "marked";
import { convertFileSrc } from "@tauri-apps/api/core";
import { i18n } from "../i18n";

const t = i18n.global.t;

/** 判断是否为允许自动转换为 Tauri asset 协议的本地图片绝对路径 */
const LOCAL_IMAGE_EXT_RE = /\.(png|jpe?g|webp|gif|svg|bmp|ico)(\?.*)?$/i;

export function resolveMarkdownImageSrc(rawHref: string): string {
  const href = rawHref.trim();
  if (!href) return "";
  // 会话附件统一协议：attachment://<filename> -> Tauri 自定义协议 jarvis-attachment 直读 SQLite
  const attachmentMatch = href.match(/^attachment:\/\/([^/?#]+)$/i);
  if (attachmentMatch) {
    try {
      return convertFileSrc(attachmentMatch[1], "jarvis-attachment");
    } catch {
      return href;
    }
  }
  // 远程链接、data URI、已转换的 asset URL 直接透传
  if (/^(https?:|data:|asset:|blob:)/i.test(href)) {
    return href;
  }
  let localPath = href;
  if (/^file:\/\//i.test(localPath)) {
    try {
      localPath = decodeURIComponent(localPath.replace(/^file:\/\/\/?/i, ""));
      // Windows 盘符前多余的斜杠处理：`/E:/foo` -> `E:/foo`
      if (/^\/[a-zA-Z]:/.test(localPath)) {
        localPath = localPath.slice(1);
      }
    } catch {
      return href;
    }
  }
  // 仅对本地绝对路径（Windows 盘符或 Unix 根路径）且为图片后缀进行转换
  const isAbsPath = /^[a-zA-Z]:[\\/]/.test(localPath) || localPath.startsWith("/");
  if (isAbsPath && LOCAL_IMAGE_EXT_RE.test(localPath)) {
    try {
      return convertFileSrc(localPath);
    } catch {
      return href;
    }
  }
  return href;
}

marked.use({
  renderer: {
    image(token: any) {
      const rawHref = String(token.href || "");
      const src = resolveMarkdownImageSrc(rawHref);
      const alt = escapeHtmlForAttr(String(token.text || ""));
      const titleAttr = token.title ? ` title="${escapeHtmlForAttr(String(token.title))}"` : "";
      if (!src) return alt;
      // 使用 span 而非 figure：StreamingMarkdown.vue 在 <p> 内调用 marked.parseInline，
      // 块级 <figure> 会破坏 <p> 结构；inline-block 的 span 既合法又美观。
      return `<span class="markdown-image-wrap"><img class="markdown-image" src="${escapeHtmlForAttr(src)}" alt="${alt}"${titleAttr} /></span>`;
    },
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
