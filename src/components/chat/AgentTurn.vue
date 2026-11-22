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
import {
  stripPseudoToolCalls,
  buildDeveloperTimeline,
} from "../../utils/agentTurnRender";
import type {
  AgentCurrentTurn,
  AgentDisplayMode,
} from "../../types";
import type { DevTimelineItem } from "../../utils/agentTurnRender";
import { usePreferences } from "../../composables/usePreferences";
import ExecutionPanel from "./ExecutionPanel.vue";
import ExecutionTimeline from "./ExecutionTimeline.vue";
import StreamingMarkdown from "../common/StreamingMarkdown.vue";

const props = defineProps<{
  turn: AgentCurrentTurn;
  displayMode: AgentDisplayMode;
  showStatus: boolean;
  elapsed: number;
  paused: boolean;
  /** 本轮状态（RUNNING / CANCELLED / ERROR 等），仅用于折叠头的状态文案 */
  status?: string;
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

/** 时间线条目的结束时刻：工具的 updatedAt 才是调用结束时间。 */
function timelineTimestamp(item: DevTimelineItem): number {
  return item.type === "tool"
    ? item.tool.updatedAt || item.tool.timestamp
    : item.timestamp;
}

/**
 * 本轮执行用时（秒）。
 * - 运行中：由父组件每秒递增的 `elapsed` 提供；
 * - 已结束（含历史消息）：用本轮第一条与最后一条事件的时间差计算。
 *   `turn.startedAt` 在历史快照里是快照生成时间，不能用，故以条目时间戳为准。
 */
const executionDurationSecs = computed(() => {
  if (props.turn.isRunning) return Math.max(props.elapsed, 0);
  const stamps = developerTimeline.value
    .map(timelineTimestamp)
    .filter((ts) => ts > 0);
  if (!stamps.length) return 0;
  return Math.max(0, Math.round((Math.max(...stamps) - Math.min(...stamps)) / 1000));
});

const executionStateLabel = computed(() => {
  if (props.turn.isRunning) return "执行中";
  const status = (props.status ?? "").toUpperCase();
  if (status === "CANCELLED") return "已停止";
  if (status === "INTERRUPTED") return "已中断";
  if (status === "ERROR" || status === "FAILED") return "执行失败";
  return "已完成";
});

function formatDuration(totalSecs: number): string {
  if (totalSecs < 60) return `${totalSecs} 秒`;
  const minutes = Math.floor(totalSecs / 60);
  const secs = totalSecs % 60;
  return secs > 0 ? `${minutes} 分 ${secs} 秒` : `${minutes} 分`;
}

/** 折叠头文案：已完成 · 用时 33 秒 · 7 步（无时长时省略"用时"段） */
const executionSummaryText = computed(() => {
  const parts = [executionStateLabel.value];
  if (executionDurationSecs.value > 0) {
    parts.push(`用时 ${formatDuration(executionDurationSecs.value)}`);
  }
  parts.push(`${timelineSplit.value.execItems.length} 步`);
  return parts.join(" · ");
});
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
        <summary class="dev-execution-summary">
          <span>{{ executionSummaryText }}</span>
          <svg class="dev-execution-chevron" viewBox="0 0 24 24" width="12" height="12" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round" aria-hidden="true">
            <polyline points="6 9 12 15 18 9"></polyline>
          </svg>
        </summary>
        <ExecutionTimeline :items="timelineSplit.execItems" compact />
      </details>
      <!-- 最后回答放下面 -->
      <ExecutionTimeline v-if="timelineSplit.finalItems.length > 0" :items="timelineSplit.finalItems" />
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

/* 最终回答用主文本色，和执行过程的 muted 灰形成层级差 */
.agent-turn-answer {
  color: var(--text-main);
}

/* ══════════════════════════════════════════════
   开发者模式 — Cursor/Codex 风格
   ══════════════════════════════════════════════ */

/* 执行过程大折叠 */
/* 卡片外观原先由 ChatArea 的 `.response-text details` 统一下发；那条规则已收窄到
   Markdown 内容，折叠面板在这里自己维护外观。 */
.dev-execution-fold {
  margin: 12px 0;
  padding: 8px 12px;
  background: color-mix(in srgb, var(--glass-bg-light) calc(var(--agent-message-opacity, 0) * 1%), transparent);
  border-radius: var(--radius-md);
  transition: background-color var(--transition-fast);
  max-width: 100%;
  overflow: hidden;
}
.dev-execution-fold:hover,
.dev-execution-fold[open] {
  background: color-mix(in srgb, var(--glass-bg) calc(var(--agent-message-opacity, 0) * 1%), transparent);
}
.dev-execution-summary {
  display: inline-flex;
  align-items: center;
  gap: 6px;
  font-size: 0.75rem;
  /* 不再加粗：折叠头只是信息标签，层级低于执行过程和最终回答 */
  font-weight: 400;
  color: var(--text-muted);
  cursor: pointer;
  user-select: none;
  list-style: none;
}
.dev-execution-summary::-webkit-details-marker {
  display: none;
}
.dev-execution-chevron {
  flex-shrink: 0;
  opacity: 0.7;
  transition: transform 0.15s ease;
}
.dev-execution-fold[open] > .dev-execution-summary .dev-execution-chevron {
  transform: rotate(180deg);
}
.dev-execution-fold[open] > .dev-execution-summary {
  margin-bottom: 8px;
}





</style>
