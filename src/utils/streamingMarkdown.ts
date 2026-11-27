/**
 * # streamingMarkdown.ts — 流式 Markdown 增量扫描与块级静态 HTML 渲染器
 *
 * 为 `StreamingMarkdown.vue` 提供 `O(Δ)` 增量安全边界扫描状态机与块级 HTML 渲染函数，
 * 确保流式输出进入冻结区后不再回扫前文、不再重复调用 `marked.lexer` / `marked.parseInline`。
 *
 * ## Key Exports
 * - `IncrementalScanState` / `createIncrementalScanState()` / `advanceSafeBoundary()`: 单向增量安全边界状态机
 * - `FrozenMarkdownBlock`: 冻结块不可变数据结构（配合 Vue `shallowRef` + `v-memo` 使用）
 * - `renderMarkdownSliceToBlocks()` / `renderMarkdownSliceToHtml()`: 切片到静态 HTML 块的渲染入口
 *
 * ## Dependencies
 * - Internal: `./markdown`（注册自定义 marked 图片与链接渲染器）
 * - External: `marked`
 *
 * ## Constraints
 * - 输出的 DOM 类名（`.md-paragraph` / `.md-h1`..`.md-h6` / `.md-code-block` / `.md-table-wrap` 等）
 *   必须与 `StreamingMarkdown.vue`、`ChatArea.vue`、`useChatClipboard.ts` 保持完全一致
 * - 原始 HTML token 必须经 HTML 转义后以 `.md-raw` 输出，防范流式文本注入 XSS
 */
import { marked } from "marked";
// 确保全局 marked 实例已挂载自定义图片协议（attachment:// / asset://）渲染器
import "./markdown";

/** 已冻结的静态 Markdown 渲染块（一经生成即不可变） */
export interface FrozenMarkdownBlock {
  id: number;
  html: string;
}

/**
 * 单向增量安全边界扫描状态机。
 *
 * 仅处理已换行结束的完整行（`scannedOffset` 永远停在换行符之后），因此无论流式分片
 * 在任意字符边界切开（甚至逐字符吐出，如 ```` `` ```` + ```` ` ````），状态转移结果都与全量扫描完全一致。
 */
export interface IncrementalScanState {
  /** 已完成扫描的字符偏移（下次追加仅从该偏移起继续扫描） */
  scannedOffset: number;
  /** 扫描至 `scannedOffset` 时是否正处于未闭合围栏代码块内 */
  inCodeFence: boolean;
  /** 当前打开的围栏字符（`` ` `` 或 `~`） */
  fenceChar: string;
  /** 当前打开的围栏长度（>= 3） */
  fenceLen: number;
  /** 最新确认可安全冻结的字符边界（不包含在未闭合代码块内的空行） */
  safeBoundary: number;
}

export function createIncrementalScanState(): IncrementalScanState {
  return {
    scannedOffset: 0,
    inCodeFence: false,
    fenceChar: "",
    fenceLen: 0,
    safeBoundary: 0,
  };
}

/** 解析行首围栏代码块标记（CommonMark：允许 0~3 个前导空格 + 至少 3 个连续 `` ` `` 或 `~`） */
function matchFenceMarker(line: string): { char: string; len: number; rest: string } | null {
  let i = 0;
  while (i < line.length && i < 3 && line[i] === " ") {
    i++;
  }
  if (i >= line.length) return null;
  const ch = line[i];
  if (ch !== "`" && ch !== "~") return null;
  let j = i;
  while (j < line.length && line[j] === ch) {
    j++;
  }
  const len = j - i;
  if (len < 3) return null;
  const rest = line.slice(j);
  // 反引号围栏的 info string 中不允许再出现反引号（CommonMark 规范）
  if (ch === "`" && rest.includes("`")) return null;
  return { char: ch, len, rest };
}

/**
 * 从 `state.scannedOffset` 起增量扫描新到达的完整行，单向推进 `state.safeBoundary`。
 *
 * 仅扫描本轮新增区间（时间复杂度 `O(Δ)`），遇到非代码块内的空白行后即将该行末尾记为安全冻结边界。
 */
export function advanceSafeBoundary(content: string, state: IncrementalScanState): number {
  let pos = state.scannedOffset;
  const len = content.length;

  while (pos < len) {
    const nl = content.indexOf("\n", pos);
    // 最后一行尚未换行，留待下次追加字符后再判定，防止把被切半的围栏标记误判为普通行
    if (nl === -1) break;

    const lineEnd = nl > pos && content[nl - 1] === "\r" ? nl - 1 : nl;
    const line = content.slice(pos, lineEnd);
    const nextPos = nl + 1;

    if (state.inCodeFence) {
      const fence = matchFenceMarker(line);
      // 闭合围栏必须与开启字符一致、长度不少于开启长度且行尾仅允许空白
      if (
        fence &&
        fence.char === state.fenceChar &&
        fence.len >= state.fenceLen &&
        fence.rest.trim().length === 0
      ) {
        state.inCodeFence = false;
        state.fenceChar = "";
        state.fenceLen = 0;
      }
    } else {
      const fence = matchFenceMarker(line);
      if (fence) {
        state.inCodeFence = true;
        state.fenceChar = fence.char;
        state.fenceLen = fence.len;
      } else if (line.trim().length === 0 && pos > 0) {
        // 非代码块内的空行终结前面的段落/列表/表格/引用块，可安全冻结至空行之后
        state.safeBoundary = nextPos;
      }
    }

    pos = nextPos;
  }

  state.scannedOffset = pos;
  return state.safeBoundary;
}

function escapeHtml(value: string): string {
  return value
    .replace(/&/g, "&amp;")
    .replace(/</g, "&lt;")
    .replace(/>/g, "&gt;")
    .replace(/"/g, "&quot;")
    .replace(/'/g, "&#39;");
}

function escapeAttr(value: string): string {
  return value
    .replace(/&/g, "&amp;")
    .replace(/</g, "&lt;")
    .replace(/>/g, "&gt;")
    .replace(/"/g, "&quot;");
}

interface MarkedTokenLike {
  type?: string;
  raw?: string;
  text?: string;
  depth?: number;
  lang?: string;
  ordered?: boolean;
  start?: number | string;
  tokens?: MarkedTokenLike[];
  items?: Array<{ tokens?: MarkedTokenLike[]; text?: string }>;
  header?: Array<{ tokens?: MarkedTokenLike[]; align?: string | null }>;
  rows?: Array<Array<{ tokens?: MarkedTokenLike[]; align?: string | null }>>;
}

function renderInlineTokens(tokens: MarkedTokenLike[] | undefined): string {
  if (!tokens || tokens.length === 0) return "";
  const raw = tokens.map((token) => token.raw || "").join("");
  try {
    return marked.parseInline(raw) as string;
  } catch {
    return escapeHtml(raw);
  }
}

function renderListItem(item: { tokens?: MarkedTokenLike[]; text?: string }): string {
  const tokens = item.tokens;
  if (!tokens || tokens.length === 0) {
    return item.text ? (marked.parseInline(item.text) as string) : "";
  }
  // 常见单行列表项走纯行内解析；若含嵌套子列表或代码块则逐 token 渲染，避免丢失层级
  const hasBlockChild = tokens.some(
    (t) => t.type === "list" || t.type === "code" || t.type === "blockquote" || t.type === "table",
  );
  if (!hasBlockChild) {
    return renderInlineTokens(tokens);
  }
  return tokens
    .map((child) => {
      if (child.type === "text") {
        return child.tokens ? renderInlineTokens(child.tokens) : (marked.parseInline(child.text || child.raw || "") as string);
      }
      if (child.type === "space") return "";
      return renderTokenToHtml(child, false);
    })
    .join("");
}

function renderTokenToHtml(token: MarkedTokenLike, allowRawFallback: boolean): string {
  switch (token.type) {
    case "space":
      return "";
    case "paragraph":
      return `<p class="md-paragraph">${renderInlineTokens(token.tokens)}</p>`;
    case "heading": {
      const depth = Math.min(6, Math.max(1, Number(token.depth) || 1));
      return `<h${depth} class="md-h${depth}">${renderInlineTokens(token.tokens)}</h${depth}>`;
    }
    case "code": {
      const lang = String(token.lang || "").trim();
      const langClass = lang ? ` class="language-${escapeAttr(lang)}"` : "";
      const header = lang
        ? `<div class="md-code-header"><span class="md-code-lang">${escapeHtml(lang)}</span></div>`
        : "";
      return `<div class="md-code-block">${header}<pre><code${langClass}>${escapeHtml(String(token.text ?? ""))}</code></pre></div>`;
    }
    case "list": {
      const itemsHtml = (token.items || [])
        .map((item) => `<li class="md-li">${renderListItem(item)}</li>`)
        .join("");
      if (token.ordered) {
        const startNum = Number(token.start);
        const startAttr = Number.isFinite(startNum) && startNum > 0 ? ` start="${startNum}"` : ' start="1"';
        return `<ol class="md-ol"${startAttr}>${itemsHtml}</ol>`;
      }
      return `<ul class="md-ul">${itemsHtml}</ul>`;
    }
    case "blockquote": {
      const inner = token.tokens
        ? token.tokens
            .filter((child) => child.type !== "space")
            .map((child) => renderTokenToHtml(child, true))
            .join("")
        : renderMarkdownSliceToHtml(String(token.text || ""));
      return `<blockquote class="md-blockquote">${inner}</blockquote>`;
    }
    case "table": {
      const headCells = (token.header || [])
        .map((cell) => {
          const alignAttr = cell.align ? ` align="${escapeAttr(String(cell.align))}"` : "";
          return `<th${alignAttr}>${renderInlineTokens(cell.tokens)}</th>`;
        })
        .join("");
      const bodyRows = (token.rows || [])
        .map((row) => {
          const cells = row
            .map((cell) => {
              const alignAttr = cell.align ? ` align="${escapeAttr(String(cell.align))}"` : "";
              return `<td${alignAttr}>${renderInlineTokens(cell.tokens)}</td>`;
            })
            .join("");
          return `<tr>${cells}</tr>`;
        })
        .join("");
      return `<div class="md-table-wrap"><table class="md-table"><thead><tr>${headCells}</tr></thead><tbody>${bodyRows}</tbody></table></div>`;
    }
    case "hr":
      return '<hr class="md-hr" />';
    case "text":
      return allowRawFallback
        ? `<span class="md-text">${escapeHtml(String(token.raw ?? token.text ?? ""))}</span>`
        : "";
    default:
      return allowRawFallback
        ? `<span class="md-raw">${escapeHtml(String(token.raw ?? ""))}</span>`
        : "";
  }
}

/**
 * 将新增冻结切片一次性解析并固化为静态 HTML 块数组。
 * 每个顶层 Markdown 块分配递增的稳定 `id`，配合 `v-memo="[block.id]"` 跳过后续 diff。
 */
export function renderMarkdownSliceToBlocks(
  slice: string,
  startId: number,
): { blocks: FrozenMarkdownBlock[]; nextId: number } {
  if (!slice) return { blocks: [], nextId: startId };
  let tokens: MarkedTokenLike[];
  try {
    tokens = (marked.lexer(slice) as MarkedTokenLike[]).filter((t) => t.type !== "space");
  } catch {
    return {
      blocks: [{ id: startId, html: `<span class="md-raw">${escapeHtml(slice)}</span>` }],
      nextId: startId + 1,
    };
  }
  const blocks: FrozenMarkdownBlock[] = [];
  let nextId = startId;
  for (const token of tokens) {
    const html = renderTokenToHtml(token, false);
    if (!html) continue;
    blocks.push({ id: nextId++, html });
  }
  return { blocks, nextId };
}

/** 将当前活跃尾部（未冻结的最后一个段落/代码块）渲染为静态 HTML 字符串 */
export function renderMarkdownSliceToHtml(slice: string): string {
  if (!slice) return "";
  try {
    const tokens = (marked.lexer(slice) as MarkedTokenLike[]).filter((t) => t.type !== "space");
    return tokens.map((token) => renderTokenToHtml(token, true)).join("");
  } catch {
    return `<span class="md-raw">${escapeHtml(slice)}</span>`;
  }
}
