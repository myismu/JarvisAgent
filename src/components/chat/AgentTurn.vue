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
import { computed, ref, watch } from "vue";
import {
  stripPseudoToolCalls,
  buildDeveloperTimeline,
} from "../../utils/agentTurnRender";
import type {
  AgentCurrentTurn,
  AgentDisplayMode,
} from "../../types";
import { usePreferences } from "../../composables/usePreferences";
import ExecutionPanel from "./ExecutionPanel.vue";
import ExecutionTimeline from "./ExecutionTimeline.vue";
import StreamingMarkdown from "../common/StreamingMarkdown.vue";

const props = defineProps<{
  turn: AgentCurrentTurn;
  displayMode: AgentDisplayMode;
  /** 运行中已用秒数（由 `useRunElapsed` 读秒，父组件传入） */
  elapsed: number;
  /** 本轮状态（RUNNING / CANCELLED / ERROR 等），仅用于折叠头的状态文案 */
  status?: string;
}>();

// 「运行中展开执行过程」偏好（单一开关，轮次结束后一律收起，见 usePreferences 里
// expandExecutionWhileRunning 的注释）。这里的 computed 只是把偏好读成响应式值：
// 必须在 computed 求值时经偏好对象访问（getter），解构成变量会退化成一次性快照。
const uiPrefs = usePreferences();
const expandExecutionWhileRunning = computed(() => uiPrefs.expandExecutionWhileRunning);

// 执行过程折叠的展开状态。
//
// 不用原生 <details> 的理由：它无法对高度做过渡，收起时内容会瞬间塌陷 ——
// 这正是「输出完自动收起」时最后一段视觉跳变的来源。改成受控折叠后，
// 内容容器用 grid-template-rows 0fr → 1fr 过渡，收起是动画而不是塌一下。
//
// 展开规则（与 usePreferences 里 expandExecutionWhileRunning 的注释一致）：
// - 运行中：由开关决定（关掉开关就只剩摘要头一行）；
// - 轮次结束（含历史消息）：无论开关如何都收起；
// - 手动点击可临时切换，但阶段变化（开跑 / 收尾 / 开关变更）会按上面的规则复位，
//   避免上一轮的手动展开态带到下一轮。
const isExecOpen = ref(false);

watch(
  [() => props.turn.isRunning, expandExecutionWhileRunning],
  ([running, expand]) => {
    isExecOpen.value = running && expand;
  },
  { immediate: true },
);

/** 折叠内容区的 id：同一页面有多轮，必须唯一，aria-controls 才不会指向别人的内容 */
const execBodyId = `dev-execution-body-${Math.random().toString(36).slice(2, 9)}`;

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
  // 拆分判据：**只看时间线末尾是不是正文**，与本轮是否运行中无关。
  //
  // 为什么不再用 isRunning：运行中正在流式输出、且位于末尾的那段文字，就是最终回答。
  // 用 isRunning 判断会把它一直关在折叠内（compact 口径：小字号 + muted 灰），
  // 收尾瞬间再搬到折叠外（正文口径），同一段文字换一遍字号/颜色/行高，
  // 而且节点会从一棵子树搬到另一棵子树，表现为「闪一下」。
  //
  // 为什么必须要求「末尾」而不是「最后一个 text」：text 之后若还有 tool / thinking / log，
  // 说明它只是中间叙述，提前提升会让工具调用跑到「最终回答」区域外面
  // （0e11c85 修过这个问题）。被下一轮 loop 接续时，正文自动回落到执行过程内。
  const last = items[items.length - 1];
  if (!last || last.type !== "text") {
    return { execItems: items, finalItems: [] as typeof items };
  }
  return {
    execItems: items.slice(0, -1),
    finalItems: [last],
  };
});
const hasDeveloperSegments = computed(() => developerTimeline.value.length > 0);

/**
 * 本轮执行用时（秒）。
 * - 运行中：父组件用 `useRunElapsed` 读秒（基准是后端 run.startedAt）；
 * - 已结束（含历史消息）：直接取后端在轮次收尾时写入的 `durationMs`。
 *   不再用「首尾条目时间戳相减」推算——历史快照里工具 updatedAt 可能缺失，
 *   推算值会偏小甚至为 0，而后端口径是 run 的 start → finished 真实跨度。
 */
const executionDurationSecs = computed(() => {
  if (props.turn.isRunning) return Math.max(props.elapsed, 0);
  return Math.max(0, Math.round((props.turn.durationMs ?? 0) / 1000));
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
    :class="displayMode"
  >
    <!-- 开发者模式 -->
    <div v-if="isDeveloperMode && hasDeveloperSegments">
      <!-- 执行过程折叠面板放上面：思考块在其内部，面板展开才能看见。
           展开状态由 isExecOpen 控制（规则见 script 里的注释）：
           运行中按开关展开，轮次结束一律收起，历史消息同理。
           不用原生 <details>：它无法做高度过渡，收起会瞬间塌陷。 -->
      <div
        v-if="timelineSplit.execItems.length > 0"
        class="dev-execution-fold"
        :class="{ open: isExecOpen }"
      >
        <button
          type="button"
          class="dev-execution-summary"
          :aria-expanded="isExecOpen"
          :aria-controls="execBodyId"
          @click="isExecOpen = !isExecOpen"
        >
          <span>{{ executionSummaryText }}</span>
          <svg class="dev-execution-chevron" viewBox="0 0 24 24" width="12" height="12" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round" aria-hidden="true">
            <polyline points="6 9 12 15 18 9"></polyline>
          </svg>
        </button>
        <div :id="execBodyId" class="dev-execution-body">
          <div class="dev-execution-body-inner">
            <ExecutionTimeline :items="timelineSplit.execItems" compact />
          </div>
        </div>
      </div>
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
.dev-execution-fold.open {
  background: color-mix(in srgb, var(--glass-bg) calc(var(--agent-message-opacity, 0) * 1%), transparent);
}
.dev-execution-summary {
  display: inline-flex;
  align-items: center;
  gap: 6px;
  /* button 默认样式清零：外观完全由这里定义，不继承浏览器按钮的边框 / 底色 / 字体 */
  padding: 0;
  border: 0;
  background: transparent;
  font-family: inherit;
  font-size: 0.75rem;
  /* 不再加粗：折叠头只是信息标签，层级低于执行过程和最终回答 */
  font-weight: 400;
  color: var(--text-muted);
  cursor: pointer;
  user-select: none;
  /* 下边距也加过渡：否则收起瞬间下方正文会被顶上去一截 */
  transition: margin-bottom 0.18s ease;
}
.dev-execution-chevron {
  flex-shrink: 0;
  opacity: 0.7;
  transition: transform 0.15s ease;
}
.dev-execution-fold.open > .dev-execution-summary .dev-execution-chevron {
  transform: rotate(180deg);
}
.dev-execution-fold.open > .dev-execution-summary {
  margin-bottom: 8px;
}

/* 折叠内容的高度过渡：grid-template-rows 0fr → 1fr。
   不用 max-height：内容可能很长，max-height 需要一个猜出来的上限，
   猜小了截断、猜大了动画速度不对；0fr/1fr 不需要知道具体高度。
   内层必须 overflow: hidden，grid 的行高收缩才能把内容裁掉。 */
.dev-execution-body {
  display: grid;
  grid-template-rows: 0fr;
  transition: grid-template-rows 0.18s ease;
}
.dev-execution-fold.open .dev-execution-body {
  grid-template-rows: 1fr;
}
.dev-execution-body-inner {
  min-height: 0;
  overflow: hidden;
}





</style>
