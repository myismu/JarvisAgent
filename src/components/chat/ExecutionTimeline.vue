<!--
# ExecutionTimeline.vue — 执行过程时间线

主对话开发者模式（AgentTurn）与子 Agent 轨迹面板共用的执行过程渲染：
按顺序渲染文本、思考、工具调用、执行日志；不包含外层「执行过程」折叠。

## Key Exports
- `ExecutionTimeline`: 接收 `DevTimelineItem[]`，按条目类型渲染

## Constraints
- 不自己排序、不自己推导状态：条目顺序、状态、流式标记由调用方给出。
- `compact` 只影响执行过程的正文密度；最终回答（`compact=false`）沿用正文口径。
-->
<script setup lang="ts">
import { computed } from 'vue';
import type { DevTimelineItem } from '../../utils/agentTurnRender';
import { describeThinkingStatic } from '../../utils/agentTurnRender';
import type { AgentToolCallView } from '../../types';
import { toolActionLabel, unwrapDeferredTool, toolDisplayCategory } from '../../utils/toolDisplay';
import { usePreferences } from '../../composables/usePreferences';
import StreamingMarkdown from '../common/StreamingMarkdown.vue';
import ExecutionRowIcon from './ExecutionRowIcon.vue';
import type { ExecutionRowIconKind } from './ExecutionRowIcon.vue';

defineProps<{
  items: DevTimelineItem[];
  /** true = 执行过程密度（0.8rem 正文、紧凑段落）；false = 最终回答口径 */
  compact?: boolean;
}>();

const uiPrefs = usePreferences();
// 偏好对象是 getter（访问器）集合：必须在 computed 求值时经它访问才是响应式。
const expandThinkingDefault = computed(() => uiPrefs.defaultExpandThinking);
const toolDisplayName = (tool: AgentToolCallView) => unwrapDeferredTool(tool).displayName;

function toolStatusLabel(status: string): string {
  if (status === 'completed') return '完成';
  if (status === 'running') return '执行中';
  if (status === 'error') return '未成功';
  return '';
}

/**
 * 工具状态由右侧文字承担（完成 / 执行中 / 未成功）：
 * 行首不再放四态状态灯，避免每行开头都是一颗彩色圆点、喧宾夺主。
 */

/** 文件类工具里，写入/编辑用"笔"图标，其余（读取/列目录等）用"文档"。 */
const FILE_EDIT_TOOLS = new Set(['writefile', 'editfile', 'editnotebook', 'applypatch']);

/** 行首图标：按条目类型 / 工具分类选择（纯视觉识别，不承载状态）。 */
function rowIconKind(item: DevTimelineItem): ExecutionRowIconKind {
  if (item.type === 'thinking') return 'thinking';
  if (item.type === 'log') return 'command';
  if (item.type !== 'tool') return 'other';
  const { innerName } = unwrapDeferredTool(item.tool);
  const lookup = innerName ?? item.tool.name;
  const category = toolDisplayCategory(lookup);
  if (category === 'file') {
    return FILE_EDIT_TOOLS.has(lookup.toLowerCase()) ? 'edit' : 'file';
  }
  if (category === 'search') return 'search';
  if (category === 'command') return 'command';
  if (category === 'planning') return 'plan';
  if (category === 'subagent') return 'subagent';
  if (category === 'task') return 'task';
  if (category === 'memory') return 'memory';
  if (category === 'system') return 'system';
  return 'other';
}

</script>

<template>
  <div class="execution-timeline" :class="{ compact }">
    <template v-for="(item, i) in items" :key="`${item.type}-${item.timestamp}-${i}`">
      <StreamingMarkdown v-if="item.type === 'text'" class="dev-text" :content="item.content" />
      <details v-else-if="item.type === 'thinking'" class="dev-thinking" :class="{ streaming: item.streaming }" :open="item.streaming || expandThinkingDefault">
        <summary class="dev-thinking-summary"><span class="dev-row-icon-wrap"><ExecutionRowIcon class="dev-row-kind-icon" :kind="rowIconKind(item)" /><ExecutionRowIcon class="dev-row-chevron" kind="chevron" /></span><span class="dev-row-kind">思考</span><span class="dev-row-dot" aria-hidden="true"></span><span class="dev-thinking-label">{{ describeThinkingStatic(item.content) }}</span></summary>
        <div class="dev-thinking-body"><StreamingMarkdown :content="item.content" /></div>
      </details>
      <details v-else-if="item.type === 'tool'" class="dev-tool" :class="[item.tool.status]" :open="item.streaming">
        <summary class="dev-tool-summary"><span class="dev-row-icon-wrap"><ExecutionRowIcon class="dev-row-kind-icon" :kind="rowIconKind(item)" /><ExecutionRowIcon class="dev-row-chevron" kind="chevron" /></span><code class="dev-tool-name">{{ toolDisplayName(item.tool) }}</code><span class="dev-row-dot" aria-hidden="true"></span><span class="dev-tool-action">{{ toolActionLabel(item.tool.name, item.tool.status, item.tool) }}</span><span class="dev-tool-status">{{ toolStatusLabel(item.tool.status) }}</span></summary>
        <div v-if="item.tool.input || item.tool.output || item.tool.error" class="dev-tool-body">
          <div v-if="item.tool.input" class="dev-tool-section"><div class="dev-tool-section-label">参数</div><StreamingMarkdown :content="item.tool.input" /></div>
          <div v-if="item.tool.output && !item.tool.error" class="dev-tool-section"><div class="dev-tool-section-label">输出</div><StreamingMarkdown :content="item.tool.output" /></div>
          <div v-if="item.tool.error" class="dev-tool-section error"><div class="dev-tool-section-label">未成功</div><StreamingMarkdown :content="item.tool.error" /></div>
        </div>
      </details>
      <div v-else-if="item.type === 'log'" class="dev-log"><div class="dev-log-header"><span class="dev-log-title">输出 #{{ item.loop || 1 }}</span></div><div class="dev-log-body"><StreamingMarkdown :content="item.content" /></div></div>
    </template>
  </div>
</template>

<style scoped>
.execution-timeline {
  display: flex;
  flex-direction: column;
  /* 2026-11-22 收紧：原先 gap 2px + 各条自身 padding，一条约 32px，
     执行过程一长就显得很疏。现改为 gap 0 + 条目 padding 压到 1~2px，
     一条约 24px，行与行贴紧（dsh / deepseek-tui 那种紧凑列表观感）。 */
  gap: 0;
  max-width: 100%;
  overflow: hidden;
}

/* 执行过程整列用 muted 灰，和最终回答（主文本色）形成层级差：
   dsh 的做法就是过程信息整体退后一档，正文回答保持最重的颜色。 */
.execution-timeline.compact {
  color: var(--text-muted);
}

/* 执行过程里的正文段落收紧（2026-11-22）。
 *
 * 真正的空白大头在这里，不在条目自身的 padding：`item.type === 'text'` 的项
 * 渲染成 StreamingMarkdown，其 `.md-paragraph` 带 `margin: 9px 0`，而时间线
 * 是 flex 容器 —— **子元素 margin 不会折叠**，于是每个正文段落实打实吃掉 18px。
 *
 * line-height / font-size 也一起对齐到摘要行（2026-11-22 再收一档）：
 * 段落正文原为 14.25px × 1.72 = 24.5px，而思考/工具摘要行是 12px —— 两种字号
 * 混排时不管怎么压 margin 都「看着不齐」（dsh 那种密度的来源是所有行同一字号、
 * 同一行高）。这里统一到 0.8rem(=12px) × 1.45 ≈ 17.4px，与摘要行的 1.4 同档。
 *
 * 只有 compact（执行过程）才收；最终回答仍用正文口径。
 */
.execution-timeline.compact .dev-text :deep(.md-paragraph) {
  margin: 0;
  line-height: 1.45;
  font-size: 0.8rem;
}
.execution-timeline.compact .dev-text :deep(.md-ul),
.execution-timeline.compact .dev-text :deep(.md-ol) {
  margin: 0;
}

/* 段落以外的块（列表 / 引用 / 标题）继承 `.streaming-markdown` 容器的 1.72，
 * 单靠上面那条只能压住 p。这里统一到 1.45，避免列表行比正文段落松一档。 */
.execution-timeline.compact .dev-text :deep(.streaming-markdown) {
  line-height: 1.45;
  /* 过程里的叙述文字同样退到 muted，与最终回答的主文本色区分开 */
  color: var(--text-muted);
}

/* 思考摘要行 / 工具行同样收紧行高：它们没写 line-height，会继承全局 1.72，
 * 12px 字号被拉到 ~20.6px。1.4 下约 16.8px，与上面的正文档位一致。 */
.execution-timeline.compact .dev-thinking-summary,
.execution-timeline.compact .dev-tool-summary {
  line-height: 1.4;
}

/* 行首结构：标签 · 内容。
 * 状态不再用彩色圆点（每行开头一颗绿/蓝/红点太抢眼），改由右侧文字承担；
 * 行首只保留一个 3px 灰点做「标签 · 内容」的分隔。 */
.dev-row-kind {
  color: var(--text-muted);
  font-size: 0.78rem;
}
.dev-row-dot {
  width: 3px;
  height: 3px;
  border-radius: 50%;
  background: var(--text-muted);
  opacity: 0.55;
  flex-shrink: 0;
}

/* 行首图标 ↔ 折叠箭头：固定 14px 方框内交叉淡入。
 * 默认显示类型图标；hover 时替换为折叠箭头；展开后箭头旋转 180°。
 * 整行不再有 hover 灰底，交互提示全部集中在这一个小小的图标上。 */
.dev-row-icon-wrap {
  position: relative;
  display: inline-flex;
  width: 14px;
  height: 14px;
  flex-shrink: 0;
}
.dev-row-kind-icon,
.dev-row-chevron {
  position: absolute;
  inset: 0;
  transition: opacity 0.15s ease, transform 0.18s ease;
}
.dev-row-kind-icon {
  opacity: 0.75;
}
.dev-row-chevron {
  opacity: 0;
}
.dev-tool-summary:hover .dev-row-kind-icon,
.dev-thinking-summary:hover .dev-row-kind-icon {
  opacity: 0;
}
.dev-tool-summary:hover .dev-row-chevron,
.dev-thinking-summary:hover .dev-row-chevron {
  opacity: 1;
}
/* 展开后箭头常驻并转向，收起且未 hover 时回到类型图标 */
.dev-tool[open] .dev-row-kind-icon,
.dev-thinking[open] .dev-row-kind-icon {
  opacity: 0;
}
.dev-tool[open] .dev-row-chevron,
.dev-thinking[open] .dev-row-chevron {
  opacity: 1;
  transform: rotate(180deg);
}

/* 思考块 */
.dev-thinking {
  padding: 0;
  max-width: 100%;
  overflow: hidden;
}
.dev-thinking-summary {
  /* display 用 flex 而不是 inline-flex：inline-flex 是**行内盒**，会被父块
     1.6 行高的 strut 撑到 24px，而工具行是块级 flex 只有 16.8px——
     两种行高混排，列表看着忽紧忽松。改成块级后统一由 min-height 决定行高节奏。 */
  display: flex;
  align-items: center;
  min-height: 22px;
  gap: 6px;
  cursor: pointer;
  user-select: none;
  list-style: none;
  color: var(--text-muted);
  font-size: 0.8rem;
  /* 与工具行同一水平内边距：两类行的首字对齐在同一条竖线上 */
  padding: 0 8px;
}
.dev-thinking-summary::-webkit-details-marker {
  display: none;
}
.dev-thinking-label {
  color: var(--text-muted);
}
.dev-thinking-body {
  /* 展开态：缩进的内嵌面板（淡底 + 细边 + 左强调线），
     与行首摘要行形成清晰层级；上下只留 2px，保持整列紧凑节奏，
     不因为展开一块内容就把行距撑回去。 */
  margin: 2px 0 2px 12px;
  padding: 6px 10px;
  border: 1px solid var(--glass-border-subtle);
  border-left: 2px solid var(--glass-border);
  border-radius: 0 6px 6px 0;
  background: var(--msg-block-tint);
  font-size: 0.82rem;
  color: var(--text-muted);
  line-height: 1.6;
  max-width: 100%;
  overflow: hidden;
}

/* 工具调用 */
/* 工具块：不再用底色表达 hover / 展开，改用行首图标切换成折叠箭头（见下方）。 */
.dev-tool {
  border-radius: 6px;
  max-width: 100%;
  overflow: hidden;
}
.dev-tool-summary {
  display: flex;
  align-items: center;
  min-height: 22px;
  gap: 6px;
  padding: 0 8px;
  cursor: pointer;
  user-select: none;
  list-style: none;
  border-radius: 6px;
  font-size: 0.8rem;
}
.dev-tool-summary::-webkit-details-marker {
  display: none;
}
.dev-tool-name {
  font-family: var(--font-mono);
  font-size: 0.78rem;
  /* 与整列同一灰度、不再加粗：工具名只是标签，不该比动作和回答更抢眼 */
  font-weight: 500;
  color: var(--text-muted);
  background: transparent;
  padding: 0;
  border: 0;
}
.dev-tool-action {
  color: var(--text-muted);
  font-size: 0.78rem;
  overflow: hidden;
  text-overflow: ellipsis;
  white-space: nowrap;
}
.dev-tool-status {
  margin-left: auto;
  font-size: 0.7rem;
  flex-shrink: 0;
}
.dev-tool.completed .dev-tool-status {
  color: var(--text-muted);
}
.dev-tool.running .dev-tool-status {
  color: var(--accent-blue);
}
.dev-tool.error .dev-tool-status {
  color: var(--accent-red);
}
.dev-tool.error .dev-tool-name {
  color: var(--text-muted);
}

/* 工具详情 */
.dev-tool-body {
  /* 与思考体同一套内嵌面板语言：缩进 + 细边 + 圆角；
     上下同样只留 2px，内部的参数/输出代码块用 tint-strong 深一档区分层级。 */
  margin: 2px 0 2px 12px;
  padding: 6px 8px;
  border: 1px solid var(--glass-border-subtle);
  border-radius: 6px;
  background: var(--msg-block-tint);
  max-width: 100%;
  overflow: hidden;
}
.dev-tool-section {
  margin-top: 4px;
  max-width: 100%;
  overflow: hidden;
}
/* 参数 / 输出：容器内的内容块，比外层再深一档（tint-strong），同样受透明度控制 */
.dev-tool-section :deep(.streaming-markdown) {
  font-family: var(--font-mono);
  /* 跟代码口径（--code-font-size）而不是正文口径：工具参数/输出本质是代码与日志。 */
  font-size: var(--code-font-size);
  --md-body-font-size: var(--code-font-size);
  line-height: 1.5;
  background: var(--msg-block-tint-strong);
  border: 1px solid var(--glass-border-subtle);
  border-radius: 4px;
  padding: 8px 10px;
  overflow: hidden;
  max-height: 240px;
  overflow-y: auto;
  /* 过程内容比最终回答退后一档（soft），正文回答保持 main */
  color: var(--text-soft);
  word-break: break-all;
  overflow-wrap: break-word;
}
.dev-tool-section :deep(.streaming-markdown *) {
  max-width: 100%;
  word-break: break-all;
  overflow-wrap: break-word;
  white-space: pre-wrap;
}
.dev-tool-section :deep(.streaming-markdown pre),
.dev-tool-section :deep(.streaming-markdown code) {
  white-space: pre-wrap;
  word-break: break-all;
  overflow-wrap: break-word;
}
.dev-tool-section-label {
  font-size: 0.7rem;
  font-weight: 600;
  color: var(--text-soft);
  text-transform: uppercase;
  letter-spacing: 0.5px;
  margin-bottom: 4px;
}
.dev-tool-section.error .dev-tool-section-label {
  color: var(--text-soft);
}

/* 执行日志终端 */
.dev-log {
  background: var(--msg-block-tint);
  border-radius: 8px;
  border: 1px solid var(--glass-border-subtle);
  overflow: hidden;
  margin: 8px 0;
  max-width: 100%;
}
.dev-log-header {
  background: var(--msg-block-tint-strong);
  padding: 6px 10px;
  display: flex;
  align-items: center;
  gap: 6px;
  border-bottom: 1px solid var(--glass-border-subtle);
}
.dev-log-title {
  font-family: var(--font-mono);
  font-size: 0.68rem;
  color: var(--text-soft);
  letter-spacing: 1px;
}
.dev-log-body {
  padding: 8px 12px;
  font-family: var(--font-mono);
  font-size: var(--code-font-size);
  --md-body-font-size: var(--code-font-size);
  /* 过程日志同样退后一档 */
  color: var(--text-soft);
  max-height: 160px;
  overflow-y: auto;
  line-height: 1.5;
  max-width: 100%;
  overflow-x: hidden;
}
.dev-log-body :deep(pre),
.dev-log-body :deep(code) {
  background: transparent;
  padding: 0;
  border: none;
  color: inherit;
  font-size: inherit;
}

/* 文本回答 */
.dev-text {
  /* 执行过程里的正文与思考/工具行同属一条流：8px 上下留白会把它撑成
     两倍行距的"段落"，与 dsh 那种整列同节奏的观感相反 */
  padding: 4px 0;
  line-height: 1.75;
}

/* 状态灯的 breathing 在 global.css（.state-light），局部不保留动画。 */
.dev-thinking.streaming .dev-thinking-label {
  color: var(--text-soft);
}
.dev-thinking.streaming .dev-thinking-body {
  border-left-color: var(--glass-border);
}
</style>
