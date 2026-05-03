<script setup lang="ts">
import { computed } from "vue";
import type {
  AgentCurrentTurn,
  AgentDisplayMode,
  AgentExecutionLog,
  AgentTextBlock,
  AgentThinkingBlock,
} from "../../types";
import { renderMarkdown } from "../../utils/markdown";
import { stripPseudoToolCalls } from "../../utils/agentTurnRender";
import {
  canMergeToolGroups,
  createToolCallGroup,
  mergeToolGroups,
  type ToolCallGroup,
} from "../../utils/toolDisplay";
import ExecutionPanel from "./ExecutionPanel.vue";
import ThinkingStatus from "./ThinkingStatus.vue";
import ToolCallGroupView from "./ToolCallGroup.vue";

const props = defineProps<{
  turn: AgentCurrentTurn;
  displayMode: AgentDisplayMode;
  showStatus: boolean;
  elapsed: number;
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

interface DeveloperSegment {
  key: string;
  type: "text" | "thinking" | "tool" | "log";
  timestamp: number;
  order: number;
  textBlock?: AgentTextBlock;
  thinkingBlock?: AgentThinkingBlock;
  toolGroup?: ToolCallGroup;
  log?: AgentExecutionLog;
  html?: string;
}

const isDeveloperMode = computed(() => props.displayMode === "developer");

const developerSegments = computed<DeveloperSegment[]>(() => {
  const segments: DeveloperSegment[] = [];

  props.turn.textBlocks.forEach((block, index) => {
    if (block.kind !== "assistant") return;
    const content = stripPseudoToolCalls(block.content);
    if (!content.trim()) return;
    segments.push({
      key: `text-${block.id}`,
      type: "text",
      timestamp: block.timestamp,
      order: index * 4,
      textBlock: block,
      html: renderMarkdown(content),
    });
  });

  props.turn.thinkingBlocks.forEach((block, index) => {
    if (!block.content.trim()) return;
    segments.push({
      key: `thinking-${block.id}`,
    type: "thinking",
    timestamp: block.timestamp,
      order: index * 4 + 1,
      thinkingBlock: block,
      html: renderMarkdown(block.content),
    });
  });

  props.turn.toolCalls.forEach((tool, index) => {
    const group = createToolCallGroup([tool]);
    segments.push({
      key: `tool-${group.id}`,
      type: "tool",
      timestamp: group.timestamp,
      order: index * 4 + 2,
      toolGroup: group,
    });
  });

  props.turn.logs.forEach((log, index) => {
    if (!log.content.trim()) return;
    segments.push({
      key: `log-${log.id}`,
      type: "log",
      timestamp: log.timestamp,
      order: index * 4 + 3,
      log,
      html: renderMarkdown(log.content),
    });
  });

  const merged: DeveloperSegment[] = [];
  segments
    .sort((a, b) => a.timestamp - b.timestamp || a.order - b.order)
    .forEach((segment) => {
      const previous = merged[merged.length - 1];
      if (
        segment.type === "tool" &&
        segment.toolGroup &&
        previous?.type === "tool" &&
        previous.toolGroup &&
        canMergeToolGroups(previous.toolGroup, segment.toolGroup)
      ) {
        const group = mergeToolGroups(previous.toolGroup, segment.toolGroup);
        previous.key = `tool-${group.id}`;
        previous.timestamp = group.timestamp;
        previous.toolGroup = group;
        return;
      }

      merged.push(segment);
    });

  return merged;
});

const hasDeveloperSegments = computed(() => developerSegments.value.length > 0);

const isThinkingOpen = (block: AgentThinkingBlock) => {
  return block.status === "streaming" && props.turn.activeThinkingBlockId === block.id;
};

const describeThinking = (content: string) => {
  const text = content.replace(/\s+/g, " ").trim();
  const lower = text.toLowerCase();

  if (!text) return "分析当前步骤";
  if (/(方案|计划|审批|plan|proposal)/i.test(text)) return "制定方案与审批策略";
  if (/(工具|调用|tool|function|参数)/i.test(text)) return "选择工具并整理调用参数";
  if (/(文件|目录|代码|实现|修改|file|code|implement)/i.test(text)) return "分析代码与实现路径";
  if (/(错误|失败|修复|bug|error|fix)/i.test(text)) return "定位问题与修复思路";
  if (/(测试|验证|build|check|test)/i.test(text)) return "规划验证与测试方式";
  if (lower.includes("user") || text.includes("用户")) return "理解用户需求与约束";

  const sentence = text.split(/[。.!?？；;]/)[0]?.trim() || text;
  return sentence.length > 28 ? `${sentence.slice(0, 28)}...` : sentence;
};

const thinkingLabel = (block: AgentThinkingBlock) => {
  const state = isThinkingOpen(block) ? "思考中" : "思考结果";
  return `${state} · ${describeThinking(block.content)}`;
};
</script>

<template>
  <div
    class="agent-turn"
    :class="[displayMode, { 'waiting-only': !hasAssistantText && !hasExecution && showStatus }]"
  >
    <div v-if="isDeveloperMode && hasDeveloperSegments" class="agent-developer-timeline">
      <template
        v-for="segment in developerSegments"
        :key="`${segment.key}-${segment.thinkingBlock ? isThinkingOpen(segment.thinkingBlock) : segment.toolGroup?.status || 'stable'}`"
      >
        <div
          v-if="segment.type === 'text'"
          class="agent-turn-answer agent-developer-text"
          v-html="segment.html"
        ></div>

        <details
          v-else-if="segment.type === 'thinking' && segment.thinkingBlock"
          class="agent-thinking-block"
          :open="isThinkingOpen(segment.thinkingBlock)"
        >
          <summary>{{ thinkingLabel(segment.thinkingBlock) }} · 第 {{ segment.thinkingBlock.loop || 1 }} 轮</summary>
          <div v-html="segment.html"></div>
        </details>

        <ToolCallGroupView
          v-else-if="segment.type === 'tool' && segment.toolGroup"
          :group="segment.toolGroup"
          mode="developer"
        />

        <details
          v-else-if="segment.type === 'log' && segment.log"
          class="agent-execution-logs"
        >
          <summary>执行日志 · 第 {{ segment.log.loop || 1 }} 轮</summary>
          <div class="agent-execution-log" v-html="segment.html"></div>
        </details>
      </template>
    </div>

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
    <ThinkingStatus :running="showStatus" :elapsed="elapsed" />
  </div>
</template>

<style scoped>
.agent-turn {
  position: relative;
  min-width: min(560px, 85vw);
}

.agent-turn.waiting-only {
  min-width: auto;
  min-height: 34px;
  display: inline-flex;
  align-items: center;
  justify-content: flex-start;
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

.agent-turn:not(.developer) .agent-turn-answer {
  padding-bottom: 24px;
}

.agent-developer-timeline {
  padding-bottom: 24px;
}

.agent-developer-text + .agent-thinking-block,
.agent-developer-text + .agent-tool-call,
.agent-thinking-block + .agent-tool-call,
.agent-tool-call + .agent-developer-text,
.agent-execution-logs + .agent-developer-text {
  margin-top: 10px;
}

.agent-turn.waiting-only .agent-turn-answer {
  padding-right: 0;
}

.agent-turn :deep(details:first-child) {
  margin-top: 0;
}
</style>
