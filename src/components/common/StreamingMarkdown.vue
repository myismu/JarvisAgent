<!--
# StreamingMarkdown.vue — 流式 Markdown 增量渲染组件

将 Agent 输出的 Markdown 增量渲染为静态 HTML 块：
- 已跨过安全段落边界的内容仅解析一次并固化至 `shallowRef` 冻结块，配合 `v-memo` 零重算；
- 活跃尾部仅包含当前未闭合的最后一个段落或代码块，单次追加复杂度降至 `O(Δ)`。

## Dependencies
- Internal: `@/utils/streamingMarkdown`

## Constraints
- 内容样式保持中性灰阶；语义色仅用于真实错误内容
- 冻结块与活跃块均通过 `v-html` 渲染，样式统一使用 `:deep(.md-*)` 保持与原有 `(0,2,0)` 特异性一致
-->
<script setup lang="ts">
import { shallowRef, watch } from 'vue';
import {
  advanceSafeBoundary,
  createIncrementalScanState,
  renderMarkdownSliceToBlocks,
  renderMarkdownSliceToHtml,
  type FrozenMarkdownBlock,
} from '../../utils/streamingMarkdown';

const props = defineProps<{
  content: string;
}>();

/** 已冻结的静态块列表（使用 shallowRef 避免 Vue 对只读 HTML 块做深层 Proxy 包装） */
const frozenBlocks = shallowRef<FrozenMarkdownBlock[]>([]);
/** 当前活跃尾部的渲染 HTML（仅包含未跨过安全边界的最后一段） */
const activeHtml = shallowRef('');

// 以下状态均为渲染内部游标，刻意不做成响应式，避免触发多余重绘
let scanState = createIncrementalScanState();
let frozenBoundary = 0;
let lastContent = '';
let nextBlockId = 1;

function resetIncrementalState() {
  scanState = createIncrementalScanState();
  frozenBoundary = 0;
  lastContent = '';
  frozenBlocks.value = [];
  activeHtml.value = '';
}

watch(
  () => props.content,
  (newContent) => {
    if (!newContent) {
      resetIncrementalState();
      return;
    }

    // 内容并非在上一次末尾追加（如切换消息、替换全文或被截断）时重置增量缓存
    if (!newContent.startsWith(lastContent)) {
      scanState = createIncrementalScanState();
      frozenBoundary = 0;
      frozenBlocks.value = [];
    }
    lastContent = newContent;

    // 仅从上次 scannedOffset 起单向增量扫描新增字符（O(Δ)），绝不回扫前文
    const boundary = advanceSafeBoundary(newContent, scanState);

    if (boundary > frozenBoundary) {
      const newlyFrozenSlice = newContent.slice(frozenBoundary, boundary);
      const { blocks, nextId } = renderMarkdownSliceToBlocks(newlyFrozenSlice, nextBlockId);
      nextBlockId = nextId;
      frozenBoundary = boundary;
      if (blocks.length > 0) {
        frozenBlocks.value = [...frozenBlocks.value, ...blocks];
      }
    }

    const activeSlice = newContent.slice(frozenBoundary);
    activeHtml.value = activeSlice ? renderMarkdownSliceToHtml(activeSlice) : '';
  },
  { immediate: true },
);
</script>

<template>
  <div class="streaming-markdown">
    <!-- 冻结区：已完成段落已固化为不可变 HTML 块，v-memo 锁定后流式期间零重绘 -->
    <div
      v-for="block in frozenBlocks"
      :key="block.id"
      v-memo="[block.id]"
      class="md-block-slot"
      v-html="block.html"
    />

    <!-- 活跃区：仅包含当前未闭合的最后一个段落/代码块 -->
    <div v-if="activeHtml" class="md-block-slot md-active-slot" v-html="activeHtml" />
  </div>
</template>

<style scoped>
.streaming-markdown {
  line-height: 1.72;
}

.md-block-slot {
  display: block;
}

:deep(.md-h1) {
  margin: 0 0 14px;
  padding-bottom: 10px;
  color: var(--text-main);
  font-size: 1.32rem;
  font-weight: var(--fw-bold);
  line-height: 1.32;
  border-bottom: 1px solid var(--glass-border-subtle);
}

:deep(.md-h2) {
  margin: 22px 0 10px;
  /* 正文标题统一走主文字色：给标题上蓝色是「AI 模板感」最明显的来源之一，
     Codex / DSH 的 markdown 标题也都只用字号+字重分层级，不上彩色 */
  color: var(--text-main);
  font-size: 1.06rem;
  font-weight: var(--fw-bold);
  line-height: 1.35;
}

:deep(.md-h3) {
  margin: 18px 0 8px;
  color: var(--text-main);
  font-size: 0.96rem;
  font-weight: var(--fw-bold);
  line-height: 1.4;
}

:deep(.md-h4),
:deep(.md-h5),
:deep(.md-h6) {
  margin: 14px 0 6px;
  color: var(--text-main);
  font-weight: 700;
  line-height: 1.4;
}

/* 显式给字号：此前 h4/h5/h6 没写 font-size，会落回浏览器默认的
   1em / 0.83em / 0.67em —— 在 14.25px 正文下 h6 只有 ~9.5px，基本不可读。 */
:deep(.md-h4) { font-size: 0.94rem; }
:deep(.md-h5) { font-size: 0.92rem; }
:deep(.md-h6) { font-size: 0.9rem; }

:deep(.md-paragraph) {
  margin: 9px 0;
  color: var(--text-main);
  font-size: var(--md-body-font-size);
  line-height: 1.72;
}

:deep(.md-ul),
:deep(.md-ol) {
  margin: 9px 0;
  padding-left: 24px;
}

:deep(.md-li) {
  margin: 5px 0;
  color: var(--text-main);
  /* 显式写死，不再靠从 .message-content 继承 —— 继承会让列表和段落差 0.75px */
  font-size: var(--md-body-font-size);
  line-height: 1.68;
}

:deep(.md-li::marker) {
  color: var(--text-muted);
}

:deep(.md-blockquote) {
  margin: 12px 0;
  padding: 10px 14px;
  color: var(--text-muted);
  font-size: var(--md-body-font-size);
  border: 1px solid color-mix(in srgb, var(--text-muted) 18%, transparent);
  border-left: 3px solid color-mix(in srgb, var(--text-muted) 42%, transparent);
  border-radius: var(--radius-md);
  background: var(--msg-block-tint);
}

/* 引用块（含加粗标题的提示块） */
:deep(.md-blockquote:has(strong)) {
  border-left-color: color-mix(in srgb, var(--text-main) 55%, transparent);
  background: var(--msg-block-tint);
  color: var(--text-soft);
}

:deep(.md-blockquote p) {
  margin: 0;
}

:deep(.md-code-block) {
  margin: 12px 0;
  border: 1px solid var(--glass-border-subtle);
  border-radius: var(--radius-md);
  background: color-mix(in srgb, var(--bg-dark) 80%, var(--bg-sidebar));
  box-shadow: inset 0 1px 3px rgba(0, 0, 0, 0.1);
  overflow: hidden;
}

:deep(.md-code-header) {
  display: flex;
  align-items: center;
  justify-content: space-between;
  padding: 8px 12px;
  background: color-mix(in srgb, var(--surface-strong) 30%, transparent);
  border-bottom: 1px solid var(--glass-border-subtle);
  /* 头部按代码字号排版，语言标签随之缩放（它用 em 相对这里）。
     不这么做的话：界面字号 19 + 代码「小」时，标签 14px 会比代码 11px 还大。
     ChatArea 里另有一条同值声明供聊天区使用（那边特异性更高）。 */
  font-size: var(--code-font-size);
}

:deep(.md-code-lang) {
  font-size: 0.88em;
  font-weight: 600;
  color: var(--text-muted);
  font-family: var(--font-mono);
}

:deep(.md-code-block pre) {
  margin: 0;
  padding: 14px 16px;
  overflow-x: auto;
  background: transparent;
}

:deep(.md-code-block code) {
  font-family: var(--font-mono);
  /* --code-font-size 是正文的固定倍率（见 global.css）；0.85em 仅作变量缺失时的回退 */
  font-size: var(--code-font-size, 0.85em);
  line-height: 1.6;
  color: var(--text-main);
  background: transparent;
  padding: 0;
  border: none;
}

:deep(.md-table-wrap) {
  margin: 12px 0;
  overflow-x: auto;
  border: 1px solid var(--glass-border);
  border-radius: var(--radius-md);
}

:deep(.md-table) {
  width: 100%;
  border-collapse: separate;
  border-spacing: 0;
}

:deep(.md-table th),
:deep(.md-table td) {
  padding: 9px 12px;
  text-align: left;
  border-bottom: 1px solid var(--glass-border-subtle);
  color: var(--text-main);
  font-size: var(--md-body-font-size);
}

:deep(.md-table th) {
  font-weight: var(--fw-bold);
  background: color-mix(in srgb, var(--bg-panel) 50%, transparent);
}

:deep(.md-table tr:last-child td) {
  border-bottom: 0;
}

:deep(.md-hr) {
  margin: 16px 0;
  border: 0;
  border-top: 1px solid var(--glass-border-subtle);
}

:deep(.md-text) {
  white-space: pre-wrap;
}

:deep(.md-raw) {
  white-space: pre-wrap;
  color: var(--text-muted);
}

.streaming-markdown :deep(a) {
  color: var(--accent-blue);
  text-decoration: none;
}

.streaming-markdown :deep(a:hover) {
  text-decoration: underline;
}

.streaming-markdown :deep(strong) {
  color: var(--text-main);
  font-weight: var(--fw-bold);
}

.streaming-markdown :deep(em) {
  font-style: italic;
}

/* 行内代码：只标记正文里的 `标识符`。父元素不是 pre 才算行内（代码块内 code 的父元素是 pre，
   不吃这套胶囊样式）。
   设计取舍（原样式的问题已核实）：
     · 去边框 —— 原先 border(#ececec) 与底色(≈#e1e1e1) 同属一个灰阶，边框既不分隔也不强调，
       只是把块的边缘磨厚一圈，形成"双层包裹"。
     · 圆角 4px —— 元素高约 20px，5px 已占行高四分之一，读作按钮/标签；4px 才是文字标记的比例。
     · 内边距用 em —— 原先固定 2px 6px，界面字号放大时胶囊不跟着长，会撑乱行距。
     · 底色走 --code-inline-bg —— 冷调定值，不再从文字色混出来（浅/深主题各一套）。
   此规则是**唯一一份**行内代码样式；ChatArea / PlanPreviewPanel 里原来的两份拷贝已删，
   正文全部经本组件渲染，不需要特异性更高的覆盖层。 */
.streaming-markdown :deep(:not(pre) > code) {
  padding: 0.1em 0.35em;
  color: var(--text-main);
  font-family: var(--font-mono);
  font-size: 0.88em;
  letter-spacing: -0.01em;
  border: 0;
  border-radius: 4px;
  background: var(--code-inline-bg);
}

.streaming-markdown :deep(del) {
  text-decoration: line-through;
  color: var(--text-muted);
}
</style>
