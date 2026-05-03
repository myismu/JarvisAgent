<script setup lang="ts">
import { computed } from "vue";
import type {
  AgentDisplayMode,
  AgentExecutionLog,
  AgentThinkingBlock,
  AgentToolCallView,
} from "../../types";
import { renderMarkdown } from "../../utils/markdown";
import { groupAdjacentToolCalls, summarizeToolGroupsForPanel } from "../../utils/toolDisplay";
import ToolCallGroup from "./ToolCallGroup.vue";

const props = defineProps<{
  mode: AgentDisplayMode;
  running: boolean;
  thinkingBlocks: AgentThinkingBlock[];
  toolCalls: AgentToolCallView[];
  logs: AgentExecutionLog[];
}>();

const thinkingItems = computed(() => props.thinkingBlocks.filter((item) => item.content.trim()));
const logItems = computed(() => props.logs.filter((item) => item.content.trim()));
const toolGroups = computed(() => groupAdjacentToolCalls(props.toolCalls));
const hasExecution = computed(
  () => thinkingItems.value.length > 0 || props.toolCalls.length > 0 || logItems.value.length > 0,
);
const isDeveloperMode = computed(() => props.mode === "developer");

const summaryText = computed(() => {
  const state = props.running ? "处理中" : "已完成";
  const toolPart =
    props.toolCalls.length > 0
      ? summarizeToolGroupsForPanel(toolGroups.value, props.toolCalls.length)
      : "无工具活动";
  const thinkingPart = thinkingItems.value.length > 0 ? ` · ${thinkingItems.value.length} 段思考` : "";
  const logPart = props.mode === "developer" && logItems.value.length > 0 ? ` · ${logItems.value.length} 条日志` : "";
  return `${state} · ${toolPart}${thinkingPart}${logPart}`;
});

const markdown = (content?: string) => renderMarkdown(content || "");
</script>

<template>
  <details
    v-if="hasExecution"
    class="agent-execution-panel"
    :class="[mode, { running }]"
    :open="isDeveloperMode"
  >
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

    <details
      v-for="block in thinkingItems"
      :key="block.id"
      class="agent-thinking-block"
      :open="isDeveloperMode"
    >
      <summary>思考过程 · 第 {{ block.loop || 1 }} 轮</summary>
      <div v-html="markdown(block.content)"></div>
    </details>

    <details v-if="logItems.length" class="agent-execution-logs" :open="isDeveloperMode">
      <summary>执行日志 · {{ logItems.length }} 条</summary>
      <div
        v-for="log in logItems"
        :key="log.id"
        class="agent-execution-log"
        v-html="markdown(log.content)"
      ></div>
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
  color: var(--accent-red);
}

.agent-tool-row.running {
  color: var(--accent-yellow);
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
  color: var(--accent-red);
}

.agent-tool-log,
.agent-execution-log {
  margin: 8px 0;
  padding-left: 22px;
  color: var(--text-muted);
  font-size: 0.86rem;
}

.agent-thinking-block,
.agent-execution-logs {
  margin: 8px 0;
}
</style>
