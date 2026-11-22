<!--
# AgentTurn.vue — 单轮 Agent 对话渲染

渲染一轮对话中的思考块、工具调用、执行日志，支持用户视图与开发者视图两种模式。

注意：**状态标注（notice）不在这里渲染** —— 它已移到 `AgentTurnNotice.vue`，由 `ChatArea`
放在回复气泡**外面**。放气泡内会跟着气泡的 padding 一起缩进，看起来像模型自己说的话。

## Dependencies
- Internal: `../../types`（AgentCurrentTurn / AgentDisplayMode）

## Constraints
- 工具失败不再用中性灰：四态（执行中 / 完成 / 未成功 / 待定）靠同一个全局状态灯区分，
  红只表示「这次调用没成功」，不再等同于致命错误
-->
<script setup lang="ts">
import { computed } from "vue";
import type { AgentCurrentTurn, AgentDisplayMode } from "../../types";
import {
  stripPseudoToolCalls,
  buildDeveloperTimeline,
  describeThinkingStatic,
} from "../../utils/agentTurnRender";
import { toolActionLabel, unwrapDeferredTool } from "../../utils/toolDisplay";
import type { AgentToolCallView } from "../../types";
import { usePreferences } from "../../composables/usePreferences";

const toolDisplayName = (tool: AgentToolCallView) => unwrapDeferredTool(tool).displayName;
import ExecutionPanel from "./ExecutionPanel.vue";
import StreamingMarkdown from "../common/StreamingMarkdown.vue";

const props = defineProps<{
  turn: AgentCurrentTurn;
  displayMode: AgentDisplayMode;
  showStatus: boolean;
  elapsed: number;
  paused: boolean;
}>();

// 「默认展开思考过程」偏好：思考块与「执行过程」面板的展开初值，两种显示模式都生效。
// 必须在 computed 求值时经偏好对象访问（getter），解构成变量会退化成一次性快照。
const uiPrefs = usePreferences();
const expandThinkingDefault = computed(() => uiPrefs.defaultExpandThinking);

// 这里原有「从正文剥离中断标记」的正则与 splitInterruptMarker，2026-09-21 删除。
// 理由（别再把它加回来）：
//   1. 状态标注现在由后端 `notice` 结构化下发，中断标记也不再拼进正文
//      （kind 走 session_messages.interrupt_kind），剥离对新数据永不命中；
//   2. 那条正则过宽 —— "任意含 ⚠ / ✕ 的行"都算标记，而模型正文里写 ⚠ 是常事
//      （比如表格的"注意"列）；且命中后是**截断式**处理（只保留匹配点之前的内容），
//      一处误判就会吃掉整条回复的后续正文（真实事故：一份交接单只剩前一节）。

const rawAssistantText = computed(() =>
  stripPseudoToolCalls(
    props.turn.textBlocks
      .filter((block) => block.kind === "assistant")
      .map((block) => block.content)
      .join(""),
  ),
);

/** 正文原样渲染：不再做任何标记剥离 */
const assistantText = rawAssistantText;

const hasAssistantText = computed(() => assistantText.value.trim().length > 0);
const hasExecution = computed(() => {
  return Boolean(
    props.turn.thinkingBlocks.some((block) => block.content.trim()) ||
      props.turn.toolCalls.length > 0 ||
      props.turn.logs.some((log) => log.content.trim()),
  );
});

const isDeveloperMode = computed(() => props.displayMode === "developer");

const developerTimeline = computed(() =>
  buildDeveloperTimeline(
    {
      textBlocks: props.turn.textBlocks
        .filter((b) => b.kind === "assistant")
        .map((b) => ({
          content: stripPseudoToolCalls(b.content),
          timestamp: b.timestamp,
        }))
        .filter((b) => b.content.trim()),
      thinkingBlocks: props.turn.thinkingBlocks,
      toolCalls: props.turn.toolCalls,
      logs: props.turn.logs,
    },
    props.turn.isRunning,
  ),
);

const timelineSplit = computed(() => {
  const items = developerTimeline.value;
  // 本轮还在运行中 → 不分离，全部留在执行过程里
  // 只有本轮彻底结束（!isRunning）才把最后一个 text 提升为"最后回答"
  if (props.turn.isRunning) {
    return { execItems: items, finalItems: [] as typeof items };
  }
  let lastTextIdx = -1;
  for (let i = items.length - 1; i >= 0; i--) {
    if (items[i].type === "text") { lastTextIdx = i; break; }
  }
  if (lastTextIdx < 0) return { execItems: items, finalItems: [] as typeof items };
  return {
    execItems: items.filter((_, i) => i !== lastTextIdx),
    finalItems: [items[lastTextIdx]],
  };
});
const hasDeveloperSegments = computed(() => developerTimeline.value.length > 0);

function toolStatusLabel(status: string): string {
  if (status === "completed") return "完成";
  if (status === "running") return "执行中";
  if (status === "error") return "未成功";
  return "";
}

/**
 * 工具调用的状态灯取值。
 *
 * 旧实现里「执行中」与「未成功」都是同一个中性灰点（只有后者不呼吸），
 * 扫一眼分不出来，只能逐行读右侧文字。改为全局状态灯的四态后：
 * 蓝=还在跑、绿=完成、红=这次没成功。红不再等于致命错误 ——
 * 灯只有 8px 且无光晕，不会把常规的失败重试渲染成事故。
 * 未知/待定状态返回空串，落到中性默认色（不为未知状态编造颜色）。
 */
function toolLightClass(status: string): string {
  if (status === "running") return "is-running";
  if (status === "completed") return "is-done";
  if (status === "error") return "is-error";
  return "";
}
</script>

<template>
  <div
    class="agent-turn"
    :class="[displayMode, { 'waiting-only': !hasAssistantText && !hasExecution && showStatus }]"
  >
    <!-- 开发者模式 -->
    <div v-if="isDeveloperMode && hasDeveloperSegments">
      <!-- 执行过程折叠面板放上面：思考块在其内部，偏好要求展开思考时面板必须随之展开，否则看不见 -->
      <details v-if="timelineSplit.execItems.length > 0" class="dev-execution-fold" :open="props.turn.isRunning || expandThinkingDefault">
        <summary class="dev-execution-summary">执行过程（{{ timelineSplit.execItems.length }} 步）</summary>
        <div class="dev-layout">
          <template v-for="(item, i) in timelineSplit.execItems" :key="`exec-${item.type}-${item.timestamp}-${i}`">
            <StreamingMarkdown v-if="item.type === 'text'" class="dev-text" :content="item.content" />
            <details v-else-if="item.type === 'thinking'" class="dev-thinking" :class="{ streaming: item.streaming }" :open="item.streaming || expandThinkingDefault">
              <summary class="dev-thinking-summary"><span class="dev-state-light state-light" :class="item.streaming ? 'is-running' : 'is-done'"></span><span class="dev-thinking-label">{{ describeThinkingStatic(item.content) }}</span></summary>
              <div class="dev-thinking-body"><StreamingMarkdown :content="item.content" /></div>
            </details>
            <details v-else-if="item.type === 'tool'" class="dev-tool" :class="[item.tool.status]" :open="item.streaming">
              <summary class="dev-tool-summary"><span class="dev-state-light state-light" :class="toolLightClass(item.tool.status)"></span><code class="dev-tool-name">{{ toolDisplayName(item.tool) }}</code><span class="dev-tool-action">{{ toolActionLabel(item.tool.name, item.tool.status, item.tool) }}</span><span class="dev-tool-status">{{ toolStatusLabel(item.tool.status) }}</span></summary>
              <div v-if="item.tool.input || item.tool.output || item.tool.error" class="dev-tool-body">
                <div v-if="item.tool.input" class="dev-tool-section"><div class="dev-tool-section-label">参数</div><StreamingMarkdown :content="item.tool.input" /></div>
                <div v-if="item.tool.output && !item.tool.error" class="dev-tool-section"><div class="dev-tool-section-label">输出</div><StreamingMarkdown :content="item.tool.output" /></div>
                <div v-if="item.tool.error" class="dev-tool-section error"><div class="dev-tool-section-label">未成功</div><StreamingMarkdown :content="item.tool.error" /></div>
              </div>
            </details>
            <div v-else-if="item.type === 'log'" class="dev-log"><div class="dev-log-header"><span class="dev-state-light state-light is-done"></span><span class="dev-log-title">输出 #{{ item.loop || 1 }}</span></div><div class="dev-log-body"><StreamingMarkdown :content="item.content" /></div></div>
          </template>
        </div>
      </details>
      <!-- 最后回答放下面 -->
      <template v-for="(item, i) in timelineSplit.finalItems" :key="`final-${item.type}-${item.timestamp}-${i}`">
        <StreamingMarkdown v-if="item.type === 'text'" class="dev-text" :content="item.content" />
        <details v-else-if="item.type === 'thinking'" class="dev-thinking" :class="{ streaming: item.streaming }" :open="item.streaming || expandThinkingDefault">
          <summary class="dev-thinking-summary"><span class="dev-state-light state-light" :class="item.streaming ? 'is-running' : 'is-done'"></span><span class="dev-thinking-label">{{ describeThinkingStatic(item.content) }}</span></summary>
          <div class="dev-thinking-body"><StreamingMarkdown :content="item.content" /></div>
        </details>
        <details v-else-if="item.type === 'tool'" class="dev-tool" :class="[item.tool.status]" :open="item.streaming">
          <summary class="dev-tool-summary"><span class="dev-state-light state-light" :class="toolLightClass(item.tool.status)"></span><code class="dev-tool-name">{{ toolDisplayName(item.tool) }}</code><span class="dev-tool-action">{{ toolActionLabel(item.tool.name, item.tool.status, item.tool) }}</span><span class="dev-tool-status">{{ toolStatusLabel(item.tool.status) }}</span></summary>
          <div v-if="item.tool.input || item.tool.output || item.tool.error" class="dev-tool-body">
            <div v-if="item.tool.input" class="dev-tool-section"><div class="dev-tool-section-label">参数</div><StreamingMarkdown :content="item.tool.input" /></div>
            <div v-if="item.tool.output && !item.tool.error" class="dev-tool-section"><div class="dev-tool-section-label">输出</div><StreamingMarkdown :content="item.tool.output" /></div>
            <div v-if="item.tool.error" class="dev-tool-section error"><div class="dev-tool-section-label">未成功</div><StreamingMarkdown :content="item.tool.error" /></div>
          </div>
        </details>
        <div v-else-if="item.type === 'log'" class="dev-log"><div class="dev-log-header"><span class="dev-state-light state-light is-done"></span><span class="dev-log-title">输出 #{{ item.loop || 1 }}</span></div><div class="dev-log-body"><StreamingMarkdown :content="item.content" /></div></div>
      </template>
    </div>

    <!-- 普通模式 -->
    <template v-else>
      <ExecutionPanel
        :mode="displayMode"
        :running="turn.isRunning"
        :thinking-blocks="turn.thinkingBlocks"
        :tool-calls="turn.toolCalls"
        :logs="turn.logs"
      />
      <StreamingMarkdown v-if="hasAssistantText" class="agent-turn-answer" :content="assistantText" />
    </template>
  </div>
</template>

<style scoped>
.agent-turn {
  position: relative;
  width: 100%;
}

.agent-turn.waiting-only {
  min-width: auto;
  min-height: 34px;
  display: inline-flex;
  align-items: center;
  justify-content: flex-start;
}

.agent-turn:not(.developer) .agent-turn-answer {
  padding-bottom: 4px;
}

/* ══════════════════════════════════════════════
   开发者模式 — Cursor/Codex 风格
   ══════════════════════════════════════════════ */

/* 执行过程大折叠 */
.dev-execution-fold {
  margin-bottom: 16px;
  max-width: 100%;
  overflow: hidden;
}
.dev-execution-summary {
  font-size: 0.75rem;
  font-weight: 600;
  color: var(--text-muted);
  cursor: pointer;
  user-select: none;
}
.dev-execution-fold[open] > .dev-execution-summary {
  margin-bottom: 8px;
}

.dev-layout {
  display: flex;
  flex-direction: column;
  /* 2026-11-22 收紧：原先 gap 2px + 各条自身padding，一条约 32px，
     执行过程一长就显得很疏。现改为 gap 0 + 条目 padding 压到 1~2px，
     一条约 24px，行与行贴紧（dsh / deepseek-tui 那种紧凑列表观感）。 */
  gap: 0;
  padding-bottom: 24px;
  margin: 4px 0;
  max-width: 100%;
  overflow: hidden;
}
.dev-execution-fold .dev-layout {
  padding-bottom: 0;
  margin: 0;
}

/* 执行过程里的正文段落收紧（2026-11-22）。
 *
 * 真正的空白大头在这里，不在条目自身的 padding：`item.type === 'text'` 的项
 * 渲染成 StreamingMarkdown，其 `.md-paragraph` 带 `margin: 9px 0`，而 dev-layout
 * 是 flex 容器 —— **子元素 margin 不会折叠**，于是每个正文段落实打实吃掉 18px。
 *
 * 为什么必须把特异性顶到这个高度：`ChatArea.vue` 曾有一条
 * `.compact-mode .response-text p { margin-bottom: 0.35em }`（(0,3,1)），把本条
 * 规则的 margin-bottom 反向覆盖成 4.9875px —— devtools 里看到 margin-top 已归零、
 * margin-bottom 却仍是 0.35em，就是它。紧凑模式已于 2026-11-22 整体删除（开关、
 * 状态、文案、样式规则都不在了），但这条特异性要求**仍要保留**：响应区内任何
 * 针对 `p` 的规则都可能再次出现，一旦压低一级就会被默默吃掉。
 *
 * line-height / font-size 也一起对齐到摘要行（2026-11-22 再收一档）：
 * 段落正文原为 14.25px × 1.72 = 24.5px，而思考/工具摘要行是 12px —— 两种字号
 * 混排时不管怎么压 margin 都「看着不齐」（dsh 那种密度的来源是所有行同一字号、
 * 同一行高）。这里统一到 0.8rem(=12px) × 1.45 ≈ 17.4px，与摘要行的 1.4 同档。
 *
 * 选择器补上 `.dev-layout` 一档 → (0,4,0)：只带 `.dev-execution-fold` 前缀时
 * 仅 (0,3,0)，在「一个 class + 一个 p 元素」这类规则（(0,1,1) 及以上）面前
 * 会输得很冤。
 */
.dev-execution-fold .dev-layout .dev-text :deep(.md-paragraph) {
  margin: 0;
  line-height: 1.45;
  font-size: 0.8rem;
}
.dev-execution-fold .dev-text :deep(.md-ul),
.dev-execution-fold .dev-text :deep(.md-ol) {
  margin: 0;
}

/* 段落以外的块（列表 / 引用 / 标题）继承 `.streaming-markdown` 容器的 1.72，
 * 单靠上面那条只能压住 p。这里统一到 1.45，避免列表行比正文段落松一档。 */
.dev-execution-fold .dev-text :deep(.streaming-markdown) {
  line-height: 1.45;
}

/* 思考摘要行 / 工具行同样收紧行高：它们没写 line-height，会继承全局 1.72，
 * 12px 字号被拉到 ~20.6px。1.4 下约 16.8px，与上面的正文档位一致。 */
.dev-execution-fold .dev-layout .dev-thinking-summary,
.dev-execution-fold .dev-layout .dev-tool-summary {
  line-height: 1.4;
}

/* 状态圆点：配色与呼吸节奏全部由全局 .state-light 的四态决定，这里只定尺寸。
   8px 是开发者视图的基准 —— 与旁边等宽工具名（0.82rem）视觉齐平。 */
.dev-state-light {
  --light-size: 8px;
}

/* 思考块 */
.dev-thinking {
  /* 上下 padding 归零（2026-11-22）：先收到 2px 仍觉疏，索性只留文字行高，
     行与行贴紧。基线仍与工具行一致（工具行上下也是 0）。 */
  padding: 0;
  max-width: 100%;
  overflow: hidden;
}
.dev-thinking-summary {
  display: inline-flex;
  align-items: center;
  gap: 8px;
  cursor: pointer;
  user-select: none;
  list-style: none;
  color: var(--text-muted);
  font-size: 0.8rem;
  padding: 0;
}
.dev-thinking-summary::-webkit-details-marker {
  display: none;
}
.dev-thinking-label {
  color: var(--text-soft);
}
.dev-thinking-body {
  /* 展开态也保持密（2026-11-22）：8px/10-14px → 4px/6-10px，展开一块思考
     不再把执行过程撑出一段明显的空白带。 */
  margin-top: 4px;
  padding: 6px 10px;
  border-left: 2px solid var(--glass-border-subtle);
  background: var(--msg-block-tint);
  font-size: 0.82rem;
  color: var(--text-muted);
  line-height: 1.6;
  max-width: 100%;
  overflow: hidden;
}

/* 工具调用 */
/* 工具块：展开态给一层淡底，走「消息背景透明度」统一控制（调到 0 即消失）。
   hover 的浅底是"可折叠"的交互提示而非静态配色，故不跟这个变量走。 */
.dev-tool {
  border-radius: 6px;
  transition: background 0.15s;
  max-width: 100%;
  overflow: hidden;
}
.dev-tool[open] {
  background: var(--msg-block-tint);
}
.dev-tool-summary {
  display: flex;
  align-items: center;
  gap: 8px;
  /* 上下 5px → 2px → 0（2026-11-22）：只保留左右 8px 用于 hover 底色的横向留白。
     可点区域变矮，但这里是桌面端列表，不靠触控热区。 */
  padding: 0 8px;
  cursor: pointer;
  user-select: none;
  list-style: none;
  border-radius: 6px;
  font-size: 0.8rem;
  transition: background 0.15s;
}
.dev-tool-summary::-webkit-details-marker {
  display: none;
}
.dev-tool-summary:hover {
  background: color-mix(in srgb, var(--surface-strong) calc(20 * var(--agent-message-opacity) / 100), transparent);
}
.dev-tool-name {
  font-family: var(--font-mono);
  font-size: 0.78rem;
  font-weight: 600;
  color: var(--text-main);
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
  color: var(--accent-green);
}
.dev-tool.running .dev-tool-status {
  color: var(--text-muted);
}
.dev-tool.error .dev-tool-status {
  color: var(--text-muted);
}
.dev-tool.error .dev-tool-name {
  color: var(--text-soft);
}

/* 工具详情 */
.dev-tool-body {
  padding: 0 8px 8px 24px;
  max-width: 100%;
  overflow: hidden;
}
.dev-tool-section {
  margin-top: 8px;
  max-width: 100%;
  overflow: hidden;
}
/* 参数 / 输出：容器内的内容块，比外层再深一档（tint-strong），同样受透明度控制 */
.dev-tool-section :deep(.streaming-markdown) {
  font-family: var(--font-mono);
  /* 跟代码口径（--code-font-size）而不是正文口径：工具参数/输出本质是代码与日志。
     同时改写 --md-body-font-size —— 否则容器内的段落仍会走 StreamingMarkdown
     的正文口径，出现「段落 14.25px + 裸文本 12.8px」两种字号混排。 */
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
  color: var(--text-main);
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
/* 注：这里原有一条 5% 底色的"错误态"规则，已删除 —— 它的 calc 缺百分比单位，
   在 color-mix 里无效、从未生效过；且工具失败本就靠中性灰而非底色区分
   （见文件头 Constraints：工具失败为常规事件，红只留给致命错误）。 */
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
  /* 同 .dev-tool-section：执行日志跟代码字号，
     并把 markdown 正文口径一起带过来，避免容器内字号混排 */
  font-size: var(--code-font-size);
  --md-body-font-size: var(--code-font-size);
  color: var(--text-main);
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
  padding: 8px 0;
  line-height: 1.75;
}

/* 状态灯的 breathing 已上提到 global.css（.state-light），
   局部不再保留 agent-pulse：两处同名 keyframes 各自漂移过一次，只留一份。 */
.dev-thinking.streaming .dev-thinking-label {
  color: var(--text-soft);
}
.dev-thinking.streaming .dev-thinking-body {
  border-left-color: var(--glass-border);
}
.dev-tool-pre {
  font-family: var(--font-mono);
  /* 跟代码口径：工具参数/输出块本质是代码与日志，不是正文 */
  font-size: var(--code-font-size);
  line-height: 1.5;
  background: transparent;
  border: none;
  border-radius: 4px;
  padding: 8px 10px;
  max-width: 100%;
  overflow-x: auto;
  white-space: pre-wrap;
  word-break: break-word;
  max-height: 240px;
  overflow-y: auto;
  color: var(--text-main);
  margin: 0;
}
.dev-tool-section.error .dev-tool-pre {
  border-color: var(--glass-border-subtle);
  background: color-mix(in srgb, var(--text-muted) calc(5 * var(--agent-message-opacity) / 100), transparent);
}
.dev-log-body :deep(pre),
.dev-log-body :deep(code) {
  background: transparent;
  padding: 0;
  border: none;
  color: inherit;
  font-size: inherit;
}
</style>
