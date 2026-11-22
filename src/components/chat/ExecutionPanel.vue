<!--
# ExecutionPanel.vue — 工具执行日志面板

展示单次工具调用的执行明细（命令、输出、错误），面向开发者视图。

## Constraints
- 状态色：completed=绿，running/error=中性灰
-->
<script setup lang="ts">
import { computed } from "vue";
import { useI18n } from "vue-i18n";
import { usePreferences } from "../../composables/usePreferences";
import type {
  AgentDisplayMode,
  AgentExecutionLog,
  AgentThinkingBlock,
  AgentToolCallView,
} from "../../types";
import { groupAdjacentToolCalls, summarizeToolGroupsForPanel } from "../../utils/toolDisplay";
import { summarizeLogLine, type LogLineSummary } from "../../utils/agentTurnRender";
import ToolCallGroup from "./ToolCallGroup.vue";
import ExecutionRowIcon from "./ExecutionRowIcon.vue";
import StreamingMarkdown from "../common/StreamingMarkdown.vue";

const props = defineProps<{
  mode: AgentDisplayMode;
  running: boolean;
  thinkingBlocks: AgentThinkingBlock[];
  toolCalls: AgentToolCallView[];
  logs: AgentExecutionLog[];
}>();

const { t } = useI18n();

const thinkingItems = computed(() => props.thinkingBlocks.filter((item) => item.content.trim()));
const logItems = computed(() => props.logs.filter((item) => item.content.trim()));
const toolGroups = computed(() => groupAdjacentToolCalls(props.toolCalls));
const hasExecution = computed(
  () => thinkingItems.value.length > 0 || props.toolCalls.length > 0 || logItems.value.length > 0,
);
const isDeveloperMode = computed(() => props.mode === "developer");
// 偏好对象是 getter（访问器）集合：必须在 computed 求值时经它访问才是响应式。
// 解构成局部变量会把 getter 读成挂载时的一次性快照，开关变化后已挂载的面板不再响应。
const uiPrefs = usePreferences();
/**
 * 执行过程面板是否展开：**本轮运行中 且 开关开启 且 已有执行内容**。
 *
 * 原先是 `isDeveloperMode || 偏好`，于是开发者模式下永远展开，开关完全失效；
 * 而口径要求是单一开关管一切：开着就在运行中展开，关掉就只剩摘要头那一行，
 * 轮次结束一律收起（running=false 时这里恒为 false）。
 * 不要改回把 isDeveloperMode 并进来。
 *
 * 第三个条件（已有内容）是 2026-11-23 加的：空窗期（消息已发出、后端还没回包）
 * 面板会渲染但内容为空，展开后就是一个空盒子。此时只留摘要行就够——
 * summaryText 已经写着「处理中」，用户的诉求是看见反向，不是看见一块空白。
 */
const shouldExpand = computed(
  () => props.running && uiPrefs.expandExecutionWhileRunning && hasExecution.value,
);

/**
 * 日志单行摘要（tag + 正文）：模板里 tag 与 text 各取一次，统一入口便于阅读。
 * 与开发者模式共用同一个纯函数，两种视图的摘要口径不会漂移。
 */
function logSummary(log: AgentExecutionLog): LogLineSummary {
  return summarizeLogLine(log.content);
}

const summaryText = computed(() => {
  const state = props.running ? t('execution.running') : t('execution.completed');
  const toolPart =
    props.toolCalls.length > 0
      ? summarizeToolGroupsForPanel(toolGroups.value, props.toolCalls.length)
      : t('execution.noToolActivity');
  const thinkingPart = thinkingItems.value.length > 0 ? ` · ${thinkingItems.value.length} ${t('execution.thinkingSegment')}` : "";
  const logPart = isDeveloperMode.value && logItems.value.length > 0 ? ` · ${logItems.value.length} ${t('execution.logCount')}` : "";
  return `${state} · ${toolPart}${thinkingPart}${logPart}`;
});
</script>

<template>
  <details
    v-if="hasExecution || running"
    class="agent-execution-panel"
    :class="[mode, { running }]"
    :open="shouldExpand"
  >
    <!-- 空窗期：只有摘要行（「处理中」），不展开也不给内容区，
         避免在没有任何执行内容时展开成一个空盒子。 -->
    <summary>
      <svg viewBox="0 0 24 24" width="14" height="14" stroke="currentColor" stroke-width="2" fill="none" stroke-linecap="round" stroke-linejoin="round">
        <circle cx="12" cy="12" r="3"></circle>
        <path d="M12 2v3"></path>
        <path d="M12 19v3"></path>
        <path d="M4.93 4.93l2.12 2.12"></path>
        <path d="M16.95 16.95l2.12 2.12"></path>
        <path d="M2 12h3"></path>
        <path d="M19 12h3"></path>
        <path d="M4.93 19.07l2.12-2.12"></path>
        <path d="M16.95 7.05l2.12-2.12"></path>
      </svg>
      <span>{{ summaryText }}</span>
    </summary>

    <div v-if="toolGroups.length" class="agent-tool-list">
      <ToolCallGroup
        v-for="group in toolGroups"
        :key="`${group.id}-${group.status}-${group.count}`"
        :group="group"
        :mode="mode"
      />
    </div>

    <!-- 思考块刻意不绑 `:open`：即使执行过程面板展开了，思考内容也要用户自己点开。
         外层 :open 已由「运行中 + 开关」控制，若这里再跟随 shouldExpand，
         一开开关所有思考会整段铺开，把摘要行结构冲掉。不要加回来。 -->
    <details
      v-for="block in thinkingItems"
      :key="block.id"
      class="agent-thinking-block"
    >
      <summary>{{ t('execution.thinkingTitle', { loop: block.loop || 1 }) }}</summary>
      <StreamingMarkdown :content="block.content" />
    </details>

    <!-- 日志条目：与开发者模式同一套折叠行（22px 摘要行 + 缩进内嵌体），
         行结构与文案逐字对齐：图标 + 输出 #N + 分隔点 + [标签] + 摘要。
         展开状态刻意不绑 `:open`：日志恒为默认收起，两种视图同一口径。
         「默认展开思考过程」只管思考块——若用它连带展开日志，开开关后
         十条日志会逐条铺开把视图撑满。 -->
    <details
      v-for="log in logItems"
      :key="log.id"
      class="agent-log"
    >
      <summary class="agent-log-summary">
        <span class="agent-log-icon-wrap">
          <ExecutionRowIcon class="agent-log-kind-icon" kind="command" />
          <ExecutionRowIcon class="agent-log-chevron" kind="chevron" />
        </span>
        <span class="agent-log-kind">输出 #{{ log.loop || 1 }}</span>
        <span class="agent-log-dot" aria-hidden="true"></span>
        <span v-if="logSummary(log).tag" class="agent-log-tag">[{{ logSummary(log).tag }}]</span>
        <span class="agent-log-label">{{ logSummary(log).text }}</span>
      </summary>
      <div class="agent-log-body"><StreamingMarkdown :content="log.content" /></div>
    </details>
  </details>
</template>

<style scoped>
.agent-execution-panel {
  margin: 10px 0;
}

.agent-execution-panel > summary {
  display: inline-flex;
  align-items: center;
  gap: 6px;
}

.agent-tool-list {
  display: flex;
  flex-direction: column;
  gap: 6px;
  margin-top: 8px;
}

.agent-tool-call {
  margin: 6px 0;
}

.agent-tool-row {
  display: inline-flex;
  align-items: center;
  gap: 8px;
  min-height: 24px;
  color: var(--text-muted);
  font-size: 0.86rem;
}

.agent-tool-row.completed {
  color: var(--accent-green);
}

.agent-tool-row.error {
  color: var(--text-muted);
}

.agent-tool-row.running {
  color: var(--text-muted);
}

.agent-tool-icon {
  display: inline-flex;
  align-items: center;
}

.agent-tool-row code {
  color: var(--text-main);
  background: transparent;
  padding: 0;
  border: 0;
}

.agent-tool-field {
  margin: 8px 0;
  padding-left: 22px;
}

.agent-tool-field > span {
  display: block;
  margin-bottom: 4px;
  color: var(--text-muted);
  font-size: 0.75rem;
  font-weight: 600;
}

.agent-tool-field.error > span {
  color: var(--text-muted);
}

.agent-tool-log {
  margin: 8px 0;
  padding-left: 22px;
  color: var(--text-muted);
  font-size: 0.86rem;
}

.agent-thinking-block {
  margin: 8px 0;
}

/* 卡片外观原先由 ChatArea 的 `.response-text details` 统一下发；那条规则已收窄到
 * Markdown 内容，组件在这里自己维护，避免与执行时间线的行互相干扰。 */
.agent-execution-panel,
.agent-thinking-block {
  margin: 12px 0;
  padding: 8px 12px;
  background: color-mix(in srgb, var(--glass-bg-light) calc(var(--agent-message-opacity, 0) * 1%), transparent);
  border-radius: var(--radius-md);
  transition: background-color var(--transition-fast);
}
.agent-execution-panel:hover,
.agent-thinking-block:hover,
.agent-execution-panel[open],
.agent-thinking-block[open] {
  background: color-mix(in srgb, var(--glass-bg) calc(var(--agent-message-opacity, 0) * 1%), transparent);
}
.agent-execution-panel > summary,
.agent-thinking-block > summary {
  cursor: pointer;
  font-size: 0.85rem;
  font-weight: 500;
  color: var(--text-muted);
  user-select: none;
  outline: none;
  transition: color var(--transition-fast);
}
.agent-execution-panel > summary:hover,
.agent-thinking-block > summary:hover {
  color: var(--text-main);
}

/* ══════════════════════════════════
   执行日志行 — 与开发者模式 ExecutionTimeline 同一套视觉语言
   ══════════════════════════════════ */

/* 摘要行：22px 行高、gap 6px、左右 8px，与工具组行、思考摘要行同节奏。
   普通模式不显示「输出 #N」轮号：轮号是调试信息，用户视图只要状态词 + 一句话。 */
.agent-log-summary {
  display: flex;
  align-items: center;
  min-height: 22px;
  gap: 6px;
  padding: 0 8px;
  /* 0.8rem 直接对齐开发者模式的摘要行（而不是普通模式面板的 0.85rem）：
     日志行是两种视图共用的同一行，字号不一致就会“看着像但不是”。 */
  font-size: 0.8rem;
  line-height: 1.4;
  color: var(--text-muted);
  cursor: pointer;
  user-select: none;
  list-style: none;
}
.agent-log-summary::-webkit-details-marker {
  display: none;
}

/* 行首图标 ↔ 折叠箭头：固定 14px 方框内交叉淡入，交互提示只集中在这一个图标上 */
.agent-log-icon-wrap {
  position: relative;
  display: inline-flex;
  width: 14px;
  height: 14px;
  flex-shrink: 0;
}
.agent-log-kind-icon,
.agent-log-chevron {
  position: absolute;
  inset: 0;
  transition: opacity 0.15s ease, transform 0.18s ease;
}
.agent-log-kind-icon {
  opacity: 0.75;
}
.agent-log-chevron {
  opacity: 0;
}
.agent-log-summary:hover .agent-log-kind-icon {
  opacity: 0;
}
.agent-log-summary:hover .agent-log-chevron {
  opacity: 1;
}
.agent-log[open] .agent-log-kind-icon {
  opacity: 0;
}
.agent-log[open] .agent-log-chevron {
  opacity: 1;
  transform: rotate(180deg);
}

/* 「输出 #N」标签与分隔点：与 ExecutionTimeline 的 .dev-row-kind / .dev-row-dot 同口径。
   状态的彩色不上场——整列只靠灰度分层，圆点只做「标签 · 内容」的视觉分隔。 */
.agent-log-kind {
  color: var(--text-muted);
  font-size: 0.78rem;
}
.agent-log-dot {
  width: 3px;
  height: 3px;
  border-radius: 50%;
  background: var(--text-muted);
  opacity: 0.55;
  flex-shrink: 0;
}

/* 状态词（等待确认 / 调度启动 / OK / FAIL）：summarizeLogLine 已把它从正文剥离，
   这里必须渲染回来，否则「[调度启动] 1 个就绪任务」会退化成「1 个就绪任务」。 */
.agent-log-tag {
  flex-shrink: 0;
  color: var(--text-muted);
  font-weight: 500;
}
.agent-log-label {
  overflow: hidden;
  text-overflow: ellipsis;
  white-space: nowrap;
}

/* 展开体：缩进内嵌面板（淡底 + 细边 + 左强调线），与思考体同一套容器语言 */
.agent-log-body {
  margin: 2px 0 2px 12px;
  padding: 6px 10px;
  border: 1px solid var(--glass-border-subtle);
  border-left: 2px solid var(--glass-border);
  border-radius: 0 6px 6px 0;
  background: var(--msg-block-tint);
  font-family: var(--font-mono);
  font-size: var(--code-font-size);
  --md-body-font-size: var(--code-font-size);
  color: var(--text-soft);
  line-height: 1.5;
  max-height: 240px;
  overflow-y: auto;
  max-width: 100%;
  overflow-x: hidden;
}
/* 日志内容常是后端下发的 markdown 引用块：把引用块外观清零，
   否则「行内面板 > 引用块」会形成框套框。与 ExecutionTimeline 口径一致。 */
.agent-log-body :deep(.md-blockquote) {
  margin: 0;
  padding: 0;
  border: 0;
  background: transparent;
  color: inherit;
  font-size: inherit;
  line-height: inherit;
}
.agent-log-body :deep(pre),
.agent-log-body :deep(code) {
  background: transparent;
  padding: 0;
  border: none;
  color: inherit;
  font-size: inherit;
}
</style>
