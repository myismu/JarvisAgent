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
import type { DevTimelineItem } from '../../utils/agentTurnRender';
import { describeThinkingStatic, summarizeLogLine } from '../../utils/agentTurnRender';

import type { AgentToolCallView } from '../../types';
import { toolActionLabel, unwrapDeferredTool, toolDisplayCategory } from '../../utils/toolDisplay';
import StreamingMarkdown from '../common/StreamingMarkdown.vue';
import ExecutionRowIcon from './ExecutionRowIcon.vue';
import type { ExecutionRowIconKind } from './ExecutionRowIcon.vue';

defineProps<{
  items: DevTimelineItem[];
  /** true = 执行过程密度（0.8rem 正文、紧凑段落）；false = 最终回答口径 */
  compact?: boolean;
  /**
   * 行内自动展开（思考块 / 工具行）。默认 false = 每一行都收起，内容由用户自己点开。
   *
   * 只在子 Agent 轨迹面板（SubAgentChatPane）传 true：那边要看的是现场轨迹，
   * 行内自动展开才看得出“正在想什么 / 正在跑什么”。
   * 主对话（AgentTurn）一律不传：外层折叠由“运行中展开执行过程”开关控制，
   * 行内再自动展开会把几十行内容一次性铺满视图。
   */
  autoExpandRows?: boolean;
}>();

const toolDisplayName = (tool: AgentToolCallView) => unwrapDeferredTool(tool).displayName;

/**
 * 工具行是否有可展开的正文（参数 / 输出 / 错误三者取其一即有）。
 *
 * 为什么需要它：折叠箭头只在「展开后确实有内容」时才该出现。
 * 没有正文的工具行（仍在流式、尚无任何 input）展开后是一片空白，
 * 箭头点下去什么也看不到，反而成了坏提示。因此箭头渲染与 `:open`
 * 都用这个判断，行首的类型图标则始终保留（图标表达的是「这一行是什么」，与能否展开无关）。
 */
function hasToolBody(tool: AgentToolCallView): boolean {
  return Boolean(tool.input || tool.output || tool.error);
}

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
  // 伪任务调用（id 以 `task_` 开头，后端下发的整句中文标题）一律走「任务」图标：
  // 这类调用没有真实工具名可分类，按分类规则只会落到 other，与真正的未知工具混在一起。
  if (item.type === 'tool' && item.tool.id.startsWith('task_')) return 'task';
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
      <!-- 思考块：只在 autoExpandRows 开启且该条仍在流式时自动展开，
           轮次结束后（streaming=false）一律收起。
           不再读全局偏好 expandExecutionWhileRunning：那个开关只管外层「执行过程」面板，
           思考块若也跟随它，主对话一开开关就会把所有思考整段铺开。 -->
      <details v-else-if="item.type === 'thinking'" class="dev-thinking" :class="{ streaming: item.streaming }" :open="autoExpandRows && item.streaming">
        <summary class="dev-thinking-summary"><span class="dev-row-icon-wrap"><ExecutionRowIcon class="dev-row-kind-icon" :kind="rowIconKind(item)" /><ExecutionRowIcon class="dev-row-chevron" kind="chevron" /></span><span class="dev-row-kind">思考</span><span class="dev-row-dot" aria-hidden="true"></span><span class="dev-thinking-label">{{ describeThinkingStatic(item.content) }}</span></summary>
        <div class="dev-thinking-body"><StreamingMarkdown :content="item.content" /></div>
      </details>
      <!-- 工具行同理：autoExpandRows 且正在流式、且确实有可展开正文时才自动展开。
           hasToolBody 同时控制箭头渲染与 :open——没有正文的行展开后是一片空白。 -->
      <details v-else-if="item.type === 'tool'" class="dev-tool" :class="[item.tool.status]" :open="autoExpandRows && item.streaming && hasToolBody(item.tool)">
        <summary class="dev-tool-summary"><span class="dev-row-icon-wrap"><ExecutionRowIcon class="dev-row-kind-icon" :kind="rowIconKind(item)" /><ExecutionRowIcon v-if="hasToolBody(item.tool)" class="dev-row-chevron" kind="chevron" /></span><code class="dev-tool-name" :class="{ 'is-pseudo-task': item.tool.id.startsWith('task_') }">{{ toolDisplayName(item.tool) }}</code><span class="dev-row-dot" aria-hidden="true"></span><span class="dev-tool-action">{{ toolActionLabel(item.tool.name, item.tool.status, item.tool) }}</span><span class="dev-tool-status">{{ toolStatusLabel(item.tool.status) }}</span></summary>
        <div v-if="item.tool.input || item.tool.output || item.tool.error" class="dev-tool-body">
          <div v-if="item.tool.input" class="dev-tool-section"><div class="dev-tool-section-label">参数</div><StreamingMarkdown :content="item.tool.input" /></div>
          <div v-if="item.tool.output && !item.tool.error" class="dev-tool-section"><div class="dev-tool-section-label">输出</div><StreamingMarkdown :content="item.tool.output" /></div>
          <div v-if="item.tool.error" class="dev-tool-section error"><div class="dev-tool-section-label">未成功</div><StreamingMarkdown :content="item.tool.error" /></div>
        </div>
      </details>
      <!-- 日志行：与思考 / 工具行同一套折叠行结构（摘要行 + 缩进内嵌体）。
           原先是一张自带标题栏的终端卡片，与上下折叠行不是同一种视觉语言，
           日志一多就像在主流程里插了几张外来的卡片。
           这里刻意不绑 `:open`：日志恒为默认收起，与普通模式 ExecutionPanel 同一口径。
           不要改成跟随 autoExpandRows 或外层开关——那会把全部日志逐条铺开，
           日志行只提供“摘要 + 点开看全文”。 -->
      <details v-else-if="item.type === 'log'" class="dev-log">
        <summary class="dev-log-summary"><span class="dev-row-icon-wrap"><ExecutionRowIcon class="dev-row-kind-icon" kind="command" /><ExecutionRowIcon class="dev-row-chevron" kind="chevron" /></span><span class="dev-row-kind">输出 #{{ item.loop || 1 }}</span><span class="dev-row-dot" aria-hidden="true"></span><span v-if="summarizeLogLine(item.content).tag" class="dev-log-tag">[{{ summarizeLogLine(item.content).tag }}]</span><span class="dev-log-label">{{ summarizeLogLine(item.content).text }}</span></summary>
        <div class="dev-log-body"><StreamingMarkdown :content="item.content" /></div>
      </details>
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
/* 箭头只在「确实有可展开正文」的工具行渲染（见 hasToolBody）。
   没有箭头的行若仍按 hover 隐去类型图标，图标位会变成一块空白——
   那不是交互提示，是掉了一块。所以工具行的 hover 用 :has(箭头) 限定范围。 */
.dev-tool-summary:has(.dev-row-chevron):hover .dev-row-kind-icon,
.dev-thinking-summary:hover .dev-row-kind-icon,
.dev-log-summary:hover .dev-row-kind-icon {
  opacity: 0;
}
.dev-tool-summary:has(.dev-row-chevron):hover .dev-row-chevron,
.dev-thinking-summary:hover .dev-row-chevron,
.dev-log-summary:hover .dev-row-chevron {
  opacity: 1;
}
/* 展开后箭头常驻并转向，收起且未 hover 时回到类型图标 */
.dev-tool[open] .dev-row-kind-icon,
.dev-thinking[open] .dev-row-kind-icon,
.dev-log[open] .dev-row-kind-icon {
  opacity: 0;
}
.dev-tool[open] .dev-row-chevron,
.dev-thinking[open] .dev-row-chevron,
.dev-log[open] .dev-row-chevron {
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
/* 伪任务行（后端下发的 `task_*` 调用）的名字是整句中文标题，不是工具标识符：
   等宽字体下汉字间距与西文宽度不一致，整句会显得松散且与动作文字脱节，
   所以这类行改用正文字体。不要改回等宽——它描述的是“做什么”，不是“用什么工具”。 */
.dev-tool-name.is-pseudo-task {
  font-family: inherit;
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
/* 参数 / 输出：直接作为 `.dev-tool-body` 面板内的文字流，不再单独成块（见下方注释） */
.dev-tool-section :deep(.streaming-markdown) {
  font-family: var(--font-mono);
  /* 跟代码口径（--code-font-size）而不是正文口径：工具参数/输出本质是代码与日志。 */
  font-size: var(--code-font-size);
  --md-body-font-size: var(--code-font-size);
  line-height: 1.5;
  /* 2026-11 去掉这一层的底色与描边：外层 `.dev-tool-body` 已经是“缩进 + 细边 + 淡底”
     的内嵌面板，参数/输出再套一层 tint-strong + 边框 + 圆角就是框套框，
     同一块内容被画了两遍边界，层级反而变模糊。现在只靠 section 标签（参数 / 输出）分隔。 */
  background: transparent;
  border: 0;
  border-radius: 0;
  padding: 0;
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

/* 执行日志：与思考行完全同构的折叠行（摘要行 22px + 缩进内嵌体）
 *
 * 原来是一张独立的“终端卡片”（标题栏 + 深底 + 边框 + 8px 外边距）：
 * 它与思考 / 工具行不属于同一种视觉语言，在一条流里就像插进来几张外来卡片；
 * 而且卡片内再叠后端下发的 markdown 引用块，就成了「卡片 > 引用块」的框套框。
 * 现在摘要行只占一行（输出 #N · 首行摘要），完整内容展开看。 */
.dev-log {
  padding: 0;
  max-width: 100%;
  overflow: hidden;
}
/* 尺寸口径（22px 行高 / gap 6px / 左右内边距 8px / 0.8rem）逐项对齐 `.dev-thinking-summary`：
   三类行在同一条流里必须共用同一套行高与首字竖线，否则混排时看着忽紧忽松。 */
.dev-log-summary {
  display: flex;
  align-items: center;
  min-height: 22px;
  gap: 6px;
  padding: 0 8px;
  font-size: 0.8rem;
  line-height: 1.4;
  cursor: pointer;
  user-select: none;
  list-style: none;
  color: var(--text-muted);
}
.dev-log-summary::-webkit-details-marker {
  display: none;
}
/* 行首方括号标签（等待确认 / 调度启动 / OK / FAIL ...）单独成一个 token，并保留方括号：
   它表达的是「这条日志属于哪类运行状态」，与后面的正文不是同一层级。
   summarizeLogLine 已把标签从正文里剥离，这里必须把它渲染回来，否则
   「[调度启动] 1 个就绪任务」会退化成「1 个就绪任务」，状态语义直接消失。
   不加底色：执行过程整列刻意不用色块，只靠灰度与字重分层。 */
.dev-log-tag {
  flex-shrink: 0;
  color: var(--text-muted);
  font-weight: 500;
}
/* 日志首行常常很长（后端把整段提示塞进一行）：摘要行只给一行，超出用省略号，
   完整内容由展开后的正文承担。不要改成换行——一换行整列节奏就散了。 */
.dev-log-label {
  overflow: hidden;
  text-overflow: ellipsis;
  white-space: nowrap;
  color: var(--text-muted);
}
/* 展开体复刻 `.dev-thinking-body`：同一套缩进面板语言（细边 + 左强调线 + 淡底），
   日志与思考都是“过程数据”，不该有两种容器外观。
   上下 2px、左 12px 与思考体一致，保持整列紧凑。 */
.dev-log-body {
  margin: 2px 0 2px 12px;
  padding: 6px 10px;
  border: 1px solid var(--glass-border-subtle);
  border-left: 2px solid var(--glass-border);
  border-radius: 0 6px 6px 0;
  background: var(--msg-block-tint);
  font-family: var(--font-mono);
  font-size: var(--code-font-size);
  --md-body-font-size: var(--code-font-size);
  /* 过程日志同样退后一档 */
  color: var(--text-soft);
  max-height: 240px;
  overflow-y: auto;
  line-height: 1.5;
  max-width: 100%;
  overflow-x: hidden;
}
/* 日志内容常是后端下发的 markdown 引用块（如 `> **[等待确认]** ...`）。
   引用块自带 12px 外边距、左侧线、灰底与内边距；叠在已有一层边界的折叠体里，
   就形成“行内面板 > 引用块”的框套框——这正是之前那块内容看起来没有设计感的直接原因。
   在日志体内部把引用块外观清零，只保留文字流。不要恢复这些属性。 */
.dev-log-body :deep(.md-blockquote) {
  margin: 0;
  padding: 0;
  border: 0;
  background: transparent;
  color: inherit;
  font-size: inherit;
  line-height: inherit;
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
