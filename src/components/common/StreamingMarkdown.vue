<script setup lang="ts">
import { computed, ref, watch } from 'vue';
import { marked } from 'marked';

const props = defineProps<{
  content: string;
}>();

const frozenBoundary = ref(0);
const frozenTokens = ref<any[]>([]);

function findLastSafeBoundary(content: string): number {
  let inCodeBlock = false;
  let lastBoundary = 0;
  let i = 0;

  while (i < content.length) {
    if (
      i <= content.length - 3 &&
      content[i] === '`' &&
      content[i + 1] === '`' &&
      content[i + 2] === '`'
    ) {
      inCodeBlock = !inCodeBlock;
      i += 3;
      continue;
    }

    if (
      !inCodeBlock &&
      i < content.length - 1 &&
      content[i] === '\n' &&
      content[i + 1] === '\n'
    ) {
      lastBoundary = i + 2;
    }

    i++;
  }

  return lastBoundary;
}

watch(
  () => props.content,
  (newContent) => {
    if (!newContent) {
      frozenBoundary.value = 0;
      frozenTokens.value = [];
      return;
    }

    const boundary = findLastSafeBoundary(newContent);

    if (boundary > frozenBoundary.value) {
      frozenBoundary.value = boundary;
      try {
        frozenTokens.value = marked
          .lexer(newContent.substring(0, boundary))
          .filter((t: any) => t.type !== 'space');
      } catch {
        frozenTokens.value = [];
      }
    } else if (frozenBoundary.value > 0 && boundary < frozenBoundary.value) {
      frozenBoundary.value = boundary;
      if (boundary > 0) {
        try {
          frozenTokens.value = marked
            .lexer(newContent.substring(0, boundary))
            .filter((t: any) => t.type !== 'space');
        } catch {
          frozenTokens.value = [];
        }
      } else {
        frozenTokens.value = [];
      }
    }
  },
  { immediate: true },
);

const activeContent = computed(() => {
  return props.content.substring(frozenBoundary.value);
});

const activeTokens = computed(() => {
  if (!activeContent.value) return [];
  try {
    return marked
      .lexer(activeContent.value)
      .filter((t: any) => t.type !== 'space');
  } catch {
    return [];
  }
});

function renderInlineTokens(tokens: any[] | undefined): string {
  if (!tokens || tokens.length === 0) return '';
  try {
    const raw = tokens.map((t: any) => t.raw || '').join('');
    return marked.parseInline(raw) as string;
  } catch {
    return tokens.map((t: any) => t.raw || '').join('');
  }
}

function renderTableCell(cell: any): string {
  return renderInlineTokens(cell.tokens || []);
}
</script>

<template>
  <div class="streaming-markdown">
    <!-- 冻结区：已完成的段落，DOM 永远不变 -->
    <template v-for="(token, index) in frozenTokens" :key="`f-${index}`">
      <p
        v-if="token.type === 'paragraph'"
        class="md-paragraph"
        v-html="renderInlineTokens(token.tokens)"
      />
      <h1 v-else-if="token.type === 'heading' && token.depth === 1" class="md-h1" v-html="renderInlineTokens(token.tokens)" />
      <h2 v-else-if="token.type === 'heading' && token.depth === 2" class="md-h2" v-html="renderInlineTokens(token.tokens)" />
      <h3 v-else-if="token.type === 'heading' && token.depth === 3" class="md-h3" v-html="renderInlineTokens(token.tokens)" />
      <h4 v-else-if="token.type === 'heading' && token.depth === 4" class="md-h4" v-html="renderInlineTokens(token.tokens)" />
      <h5 v-else-if="token.type === 'heading' && token.depth === 5" class="md-h5" v-html="renderInlineTokens(token.tokens)" />
      <h6 v-else-if="token.type === 'heading' && token.depth === 6" class="md-h6" v-html="renderInlineTokens(token.tokens)" />
      <div v-else-if="token.type === 'code'" class="md-code-block">
        <div v-if="token.lang" class="md-code-header">
          <span class="md-code-lang">{{ token.lang }}</span>
        </div>
        <pre><code :class="token.lang ? `language-${token.lang}` : ''">{{ token.text }}</code></pre>
      </div>
      <ul v-else-if="token.type === 'list' && !token.ordered" class="md-ul">
        <li
          v-for="(item, i) in token.items"
          :key="`fli-${index}-${i}`"
          class="md-li"
          v-html="renderInlineTokens(item.tokens)"
        />
      </ul>
      <ol v-else-if="token.type === 'list' && token.ordered" class="md-ol" :start="token.start || 1">
        <li
          v-for="(item, i) in token.items"
          :key="`foli-${index}-${i}`"
          class="md-li"
          v-html="renderInlineTokens(item.tokens)"
        />
      </ol>
      <blockquote v-else-if="token.type === 'blockquote'" class="md-blockquote">
        <StreamingMarkdown :content="token.text" />
      </blockquote>
      <div v-else-if="token.type === 'table'" class="md-table-wrap">
        <table class="md-table">
          <thead>
            <tr>
              <th
                v-for="(cell, i) in token.header"
                :key="`fth-${index}-${i}`"
                :align="cell.align || undefined"
                v-html="renderTableCell(cell)"
              />
            </tr>
          </thead>
          <tbody>
            <tr v-for="(row, ri) in token.rows" :key="`ftr-${index}-${ri}`">
              <td
                v-for="(cell, ci) in row"
                :key="`ftd-${index}-${ri}-${ci}`"
                :align="cell.align || undefined"
                v-html="renderTableCell(cell)"
              />
            </tr>
          </tbody>
        </table>
      </div>
      <hr v-else-if="token.type === 'hr'" class="md-hr" />
    </template>

    <!-- 活跃区：最后一个未完成的段落，每次更新重新解析 -->
    <template v-for="(token, index) in activeTokens" :key="`a-${index}`">
      <p
        v-if="token.type === 'paragraph'"
        class="md-paragraph"
        v-html="renderInlineTokens(token.tokens)"
      />
      <h1 v-else-if="token.type === 'heading' && token.depth === 1" class="md-h1" v-html="renderInlineTokens(token.tokens)" />
      <h2 v-else-if="token.type === 'heading' && token.depth === 2" class="md-h2" v-html="renderInlineTokens(token.tokens)" />
      <h3 v-else-if="token.type === 'heading' && token.depth === 3" class="md-h3" v-html="renderInlineTokens(token.tokens)" />
      <h4 v-else-if="token.type === 'heading' && token.depth === 4" class="md-h4" v-html="renderInlineTokens(token.tokens)" />
      <h5 v-else-if="token.type === 'heading' && token.depth === 5" class="md-h5" v-html="renderInlineTokens(token.tokens)" />
      <h6 v-else-if="token.type === 'heading' && token.depth === 6" class="md-h6" v-html="renderInlineTokens(token.tokens)" />
      <div v-else-if="token.type === 'code'" class="md-code-block">
        <div v-if="token.lang" class="md-code-header">
          <span class="md-code-lang">{{ token.lang }}</span>
        </div>
        <pre><code :class="token.lang ? `language-${token.lang}` : ''">{{ token.text }}</code></pre>
      </div>
      <ul v-else-if="token.type === 'list' && !token.ordered" class="md-ul">
        <li
          v-for="(item, i) in token.items"
          :key="`ali-${index}-${i}`"
          class="md-li"
          v-html="renderInlineTokens(item.tokens)"
        />
      </ul>
      <ol v-else-if="token.type === 'list' && token.ordered" class="md-ol" :start="token.start || 1">
        <li
          v-for="(item, i) in token.items"
          :key="`aoli-${index}-${i}`"
          class="md-li"
          v-html="renderInlineTokens(item.tokens)"
        />
      </ol>
      <blockquote v-else-if="token.type === 'blockquote'" class="md-blockquote">
        <StreamingMarkdown :content="token.text" />
      </blockquote>
      <div v-else-if="token.type === 'table'" class="md-table-wrap">
        <table class="md-table">
          <thead>
            <tr>
              <th
                v-for="(cell, i) in token.header"
                :key="`ath-${index}-${i}`"
                :align="cell.align || undefined"
                v-html="renderTableCell(cell)"
              />
            </tr>
          </thead>
          <tbody>
            <tr v-for="(row, ri) in token.rows" :key="`atr-${index}-${ri}`">
              <td
                v-for="(cell, ci) in row"
                :key="`atd-${index}-${ri}-${ci}`"
                :align="cell.align || undefined"
                v-html="renderTableCell(cell)"
              />
            </tr>
          </tbody>
        </table>
      </div>
      <hr v-else-if="token.type === 'hr'" class="md-hr" />
      <span v-else-if="token.type === 'text'" class="md-text">
        {{ token.raw }}
      </span>
      <span v-else class="md-raw">{{ token.raw }}</span>
    </template>
  </div>
</template>

<style scoped>
.streaming-markdown {
  line-height: 1.72;
}

.md-h1 {
  margin: 0 0 14px;
  padding-bottom: 10px;
  color: var(--text-main);
  font-size: 1.32rem;
  font-weight: 760;
  line-height: 1.32;
  border-bottom: 1px solid var(--glass-border-subtle);
}

.md-h2 {
  margin: 22px 0 10px;
  color: var(--accent-blue);
  font-size: 1.06rem;
  font-weight: 740;
  line-height: 1.35;
}

.md-h3 {
  margin: 18px 0 8px;
  color: var(--text-main);
  font-size: 0.96rem;
  font-weight: 720;
  line-height: 1.4;
}

.md-h4,
.md-h5,
.md-h6 {
  margin: 14px 0 6px;
  color: var(--text-main);
  font-weight: 700;
  line-height: 1.4;
}

.md-paragraph {
  margin: 9px 0;
  color: var(--text-main);
  font-size: 0.9rem;
  line-height: 1.72;
}

.md-ul,
.md-ol {
  margin: 9px 0;
  padding-left: 24px;
}

.md-li {
  margin: 5px 0;
  color: var(--text-main);
  line-height: 1.68;
}

.md-li::marker {
  color: var(--accent-blue);
}

.md-blockquote {
  margin: 12px 0;
  padding: 10px 14px;
  color: var(--text-muted);
  border-left: 3px solid var(--accent-blue);
  border-radius: 0 var(--radius-md) var(--radius-md) 0;
  background: color-mix(in srgb, var(--accent-blue) 10%, transparent);
}

.md-blockquote :deep(p) {
  margin: 0;
}

.md-code-block {
  margin: 12px 0;
  border: 1px solid var(--glass-border-subtle);
  border-radius: var(--radius-md);
  background: color-mix(in srgb, var(--bg-dark) 80%, var(--bg-sidebar));
  box-shadow: inset 0 1px 3px rgba(0, 0, 0, 0.1);
  overflow: hidden;
}

.md-code-header {
  display: flex;
  align-items: center;
  justify-content: space-between;
  padding: 8px 12px;
  background: color-mix(in srgb, var(--surface-strong) 30%, transparent);
  border-bottom: 1px solid var(--glass-border-subtle);
}

.md-code-lang {
  font-size: 0.78rem;
  font-weight: 600;
  color: var(--text-muted);
  font-family: var(--font-mono);
}

.md-code-block pre {
  margin: 0;
  padding: 14px 16px;
  overflow-x: auto;
  background: transparent;
}

.md-code-block code {
  font-family: var(--font-mono);
  font-size: 0.85em;
  line-height: 1.6;
  color: var(--text-main);
  background: transparent;
  padding: 0;
  border: none;
}

.md-table-wrap {
  margin: 12px 0;
  overflow-x: auto;
  border: 1px solid var(--glass-border);
  border-radius: var(--radius-md);
}

.md-table {
  width: 100%;
  border-collapse: separate;
  border-spacing: 0;
}

.md-table th,
.md-table td {
  padding: 9px 12px;
  text-align: left;
  border-bottom: 1px solid var(--glass-border-subtle);
  color: var(--text-main);
  font-size: 0.9rem;
}

.md-table th {
  font-weight: 730;
  background: color-mix(in srgb, var(--bg-panel) 50%, transparent);
}

.md-table tr:last-child td {
  border-bottom: 0;
}

.md-hr {
  margin: 16px 0;
  border: 0;
  border-top: 1px solid var(--glass-border-subtle);
}

.md-text {
  white-space: pre-wrap;
}

.md-raw {
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
  font-weight: 760;
}

.streaming-markdown :deep(em) {
  font-style: italic;
}

.streaming-markdown :deep(code) {
  padding: 2px 6px;
  color: var(--text-main);
  font-family: var(--font-mono);
  font-size: 0.85em;
  border: 1px solid var(--glass-border-subtle);
  border-radius: 5px;
  background: color-mix(in srgb, var(--text-muted) 15%, transparent);
}

.streaming-markdown :deep(del) {
  text-decoration: line-through;
  color: var(--text-muted);
}
</style>
