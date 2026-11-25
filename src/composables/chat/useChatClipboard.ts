/**
 * # useChatClipboard.ts — 聊天区代码块与表格复制交互
 *
 * 封装 Markdown 代码块纯文本复制、HTML 表格转 GFM Markdown + 富文本双格式复制，
 * 以及按钮短暂“已复制”状态反馈。
 *
 * ## Key Exports
 * - `useChatClipboard()`: 提供 `copyCodeBlock()` 与 `copyTable()` 委托处理器
 *
 * ## Dependencies
 * - External: `vue-i18n`
 */
import { useI18n } from "vue-i18n";

async function copyText(text: string, html?: string): Promise<void> {
  if (html && navigator.clipboard?.write && typeof ClipboardItem !== "undefined") {
    try {
      await navigator.clipboard.write([
        new ClipboardItem({
          "text/html": new Blob([html], { type: "text/html" }),
          "text/plain": new Blob([text], { type: "text/plain" }),
        }),
      ]);
      return;
    } catch {
      // 富文本剪贴板不可用时退回纯文本
    }
  }

  if (navigator.clipboard?.writeText) {
    await navigator.clipboard.writeText(text);
    return;
  }

  const textarea = document.createElement("textarea");
  textarea.value = text;
  textarea.setAttribute("readonly", "");
  textarea.style.position = "fixed";
  textarea.style.left = "-9999px";
  textarea.style.opacity = "0";
  document.body.appendChild(textarea);
  textarea.select();
  document.execCommand("copy");
  document.body.removeChild(textarea);
}

function normalizeMarkdownTableCell(cell?: HTMLTableCellElement): string {
  if (!cell) return "";
  return cell.innerText
    .replace(/\s+/g, " ")
    .trim()
    .replace(/\\/g, "\\\\")
    .replace(/\|/g, "\\|");
}

function markdownAlignment(cell?: HTMLTableCellElement): string {
  const align = (cell?.getAttribute("align") || "").toLowerCase();
  if (align === "center") return ":---:";
  if (align === "right") return "---:";
  return "---";
}

function tableToMarkdown(table: HTMLTableElement): string {
  const rows = Array.from(table.rows);
  if (rows.length === 0) return "";

  const headerRow = table.tHead?.rows[0] || rows[0];
  const bodyRows = rows.filter((row) => row !== headerRow);
  const columnCount = Math.max(...rows.map((row) => row.cells.length), 1);
  const readRow = (row: HTMLTableRowElement) =>
    Array.from({ length: columnCount }, (_, index) =>
      normalizeMarkdownTableCell(row.cells[index]),
    );

  const header = readRow(headerRow);
  const separator = Array.from({ length: columnCount }, (_, index) =>
    markdownAlignment(headerRow.cells[index]),
  );
  const body = bodyRows.map((row) => `| ${readRow(row).join(" | ")} |`);

  return [`| ${header.join(" | ")} |`, `| ${separator.join(" | ")} |`, ...body].join("\n");
}

export function useChatClipboard() {
  const { t } = useI18n();

  const showCopiedState = (button: HTMLButtonElement) => {
    const previousText = button.textContent || t("common.copy");
    button.textContent = t("common.copied");
    button.classList.add("copied");
    window.setTimeout(() => {
      button.textContent = previousText;
      button.classList.remove("copied");
    }, 1200);
  };

  const copyCodeBlock = async (button: HTMLButtonElement) => {
    const block = button.closest(".markdown-code-block, .md-code-block");
    const code = block?.querySelector("pre code")?.textContent || "";
    if (!code.trim()) return;
    await copyText(code);
    showCopiedState(button);
  };

  const copyTable = async (button: HTMLButtonElement) => {
    const wrap = button.closest(".markdown-table-wrap, .md-table-wrap");
    const table = wrap?.querySelector("table") as HTMLTableElement | null | undefined;
    if (!table) return;
    await copyText(tableToMarkdown(table), table.outerHTML);
    showCopiedState(button);
  };

  return {
    copyCodeBlock,
    copyTable,
  };
}
