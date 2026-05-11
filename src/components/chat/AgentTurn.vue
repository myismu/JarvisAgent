<script setup lang="ts">
import { computed } from "vue";
import type { AgentCurrentTurn, AgentDisplayMode } from "../../types";
import { renderMarkdown, renderTokenUsage } from "../../utils/markdown";
import {
  stripPseudoToolCalls,
  buildDeveloperTimeline,
} from "../../utils/agentTurnRender";
import ExecutionPanel from "./ExecutionPanel.vue";
import ThinkingStatus from "./ThinkingStatus.vue";

const props = defineProps<{
  turn: AgentCurrentTurn;
  displayMode: AgentDisplayMode;
  showStatus: boolean;
  elapsed: number;
  paused: boolean;
}>();

const assistantText = computed(() =>
  stripPseudoToolCalls(
    props.turn.textBlocks
      .filter((block) => block.kind === "assistant")
      .map((block) => block.content)
      .join(""),
  ),
);

const renderedAssistantText = computed(() => renderMarkdown(assistantText.value));
const hasAssistantText = computed(() => assistantText.value.trim().length > 0);
const hasExecution = computed(() => {
  return Boolean(
    props.turn.thinkingBlocks.some((block) => block.content.trim()) ||
      props.turn.toolCalls.length > 0 ||
      props.turn.logs.some((log) => log.content.trim()),
  );
});

const tokenUsageHtml = computed(() => {
  if (!props.turn.tokens) return "";
  return renderTokenUsage(
    props.turn.tokens.input,
    props.turn.tokens.output,
    props.turn.tokens.sessionInput,
    props.turn.tokens.sessionOutput,
  );
});

const isDeveloperMode = computed(() => props.displayMode === "developer");

// 统一时间线：直播和历史共用 buildDeveloperTimeline
const developerTimeline = computed(() =>
  buildDeveloperTimeline(
    {
      textBlocks: props.turn.textBlocks.filter((b) => b.kind === "assistant").map((b) => ({
        content: stripPseudoToolCalls(b.content),
        timestamp: b.timestamp,
      })),
      thinkingBlocks: props.turn.thinkingBlocks,
      toolCalls: props.turn.toolCalls,
      logs: props.turn.logs,
    },
    props.turn.isRunning,
  ),
);

// 最后一个 text 块始终在折叠外，其余全进折叠
const timelineSplit = computed(() => {
  const items = developerTimeline.value;
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
</script>

<template>
  <div
    class="agent-turn"
    :class="[displayMode, { 'waiting-only': !hasAssistantText && !hasExecution && showStatus }]"
  >
    <!-- 开发者模式 -->
    <div v-if="isDeveloperMode && hasDeveloperSegments">
      <!-- 执行过程大折叠：直播时展开，完成后折叠 -->
      <details
        v-if="timelineSplit.execItems.length > 0"
        class="dev-execution-fold"
        :open="props.turn.isRunning"
      >
        <summary class="dev-execution-summary">执行过程（{{ timelineSplit.execItems.length }} 步）</summary>
        <div class="dev-layout">
          <div
            v-for="(item, i) in timelineSplit.execItems"
            :key="`exec-${item.type}-${item.timestamp}-${i}`"
            v-html="item.html"
          ></div>
        </div>
      </details>
      <!-- 最终总结 -->
      <div
        v-for="(item, i) in timelineSplit.finalItems"
        :key="`final-${item.timestamp}-${i}`"
        v-html="item.html"
      ></div>
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
      <div v-if="hasAssistantText" class="agent-turn-answer" v-html="renderedAssistantText"></div>
    </template>

    <div v-if="tokenUsageHtml" class="agent-turn-tokens" v-html="tokenUsageHtml"></div>

    <ThinkingStatus :running="showStatus" :elapsed="elapsed" :paused="paused" />
  </div>
</template>

<style scoped>
.agent-turn {
  position: relative;
  width: 100%;
}

.agent-turn-tokens {
  margin-top: 16px;
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
  padding-bottom: 24px;
}

.agent-turn > :deep(.thinking-inline-status) {
  position: absolute;
  bottom: 0;
  right: 0;
  z-index: 1;
}

.agent-turn.waiting-only > :deep(.thinking-inline-status) {
  position: static;
}

/* ══════════════════════════════════════════════
   开发者模式 — Cursor/Codex 风格
   ══════════════════════════════════════════════ */

/* 执行过程大折叠 */
.dev-execution-fold {
  margin-bottom: 16px;
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
  gap: 2px;
  padding-bottom: 24px;
}
.dev-execution-fold .dev-layout {
  padding-bottom: 0;
}

/* 状态圆点 */
.dev-status-dot {
  width: 8px;
  height: 8px;
  border-radius: 50%;
  flex-shrink: 0;
  background: var(--accent-green);
  transition: background 0.2s;
}
.dev-status-dot.running {
  background: var(--accent-yellow);
  animation: dev-pulse 1.5s ease-in-out infinite;
}

/* 思考块 */
.dev-thinking {
  padding: 6px 0;
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
  padding: 2px 0;
}
.dev-thinking-summary::-webkit-details-marker {
  display: none;
}
.dev-thinking-label {
  color: var(--text-muted);
}
.dev-thinking.streaming .dev-thinking-label {
  color: var(--accent-yellow);
}
.dev-thinking-body {
  margin-top: 8px;
  padding: 10px 14px;
  border-left: 2px solid var(--glass-border-subtle);
  font-size: 0.82rem;
  color: var(--text-muted);
  line-height: 1.6;
}
.dev-thinking.streaming .dev-thinking-body {
  border-left-color: var(--accent-yellow);
}

/* 工具调用 */
.dev-tool {
  border-radius: 6px;
  transition: background 0.15s;
}
.dev-tool[open] {
  background: color-mix(in srgb, var(--surface-strong) calc(30 * var(--agent-message-opacity) / 100), transparent);
}
.dev-tool-summary {
  display: flex;
  align-items: center;
  gap: 8px;
  padding: 5px 8px;
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
  color: var(--accent-yellow);
}
.dev-tool.error .dev-tool-status {
  color: var(--accent-red);
}
.dev-tool.error .dev-tool-name {
  color: var(--accent-red);
}

/* 工具详情 */
.dev-tool-body {
  padding: 0 8px 8px 24px;
}
.dev-tool-section {
  margin-top: 8px;
}
.dev-tool-section-label {
  font-size: 0.7rem;
  font-weight: 600;
  color: var(--text-muted);
  text-transform: uppercase;
  letter-spacing: 0.5px;
  margin-bottom: 4px;
}
.dev-tool-section.error .dev-tool-section-label {
  color: var(--accent-red);
}
.dev-tool-pre {
  font-family: var(--font-mono);
  font-size: 0.78rem;
  line-height: 1.5;
  background: color-mix(in srgb, var(--surface-strong) calc(50 * var(--agent-message-opacity) / 100), transparent);
  border: 1px solid var(--glass-border-subtle);
  border-radius: 4px;
  padding: 8px 10px;
  overflow-x: auto;
  white-space: pre-wrap;
  word-break: break-word;
  max-height: 240px;
  overflow-y: auto;
  color: var(--text-main);
  margin: 0;
}
.dev-tool-section.error .dev-tool-pre {
  border-color: color-mix(in srgb, var(--accent-red) 30%, transparent);
  background: color-mix(in srgb, var(--accent-red) calc(5 * var(--agent-message-opacity) / 100), transparent);
}

/* 执行日志终端 */
.dev-log {
  background: rgba(15, 23, 42, calc(var(--agent-message-opacity) / 100));
  border-radius: 8px;
  border: 1px solid rgba(255, 255, 255, 0.08);
  overflow: hidden;
  margin: 8px 0;
}
.dev-log-header {
  background: rgba(255, 255, 255, calc(0.04 * var(--agent-message-opacity) / 100));
  padding: 6px 10px;
  display: flex;
  align-items: center;
  gap: 6px;
  border-bottom: 1px solid rgba(255, 255, 255, 0.06);
}
.dev-log-dot {
  width: 8px;
  height: 8px;
  border-radius: 50%;
}
.dev-log-dot.red   { background: #ef4444; }
.dev-log-dot.yellow { background: #f59e0b; }
.dev-log-dot.green  { background: #10b981; }
.dev-log-title {
  font-family: var(--font-mono);
  font-size: 0.68rem;
  color: rgba(255, 255, 255, 0.35);
  letter-spacing: 1px;
}
.dev-log-body {
  padding: 8px 12px;
  font-family: var(--font-mono);
  font-size: 0.78rem;
  color: #e2e8f0;
  max-height: 160px;
  overflow-y: auto;
  line-height: 1.5;
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

@keyframes dev-pulse {
  0%, 100% { opacity: 1; }
  50% { opacity: 0.4; }
}
</style>
