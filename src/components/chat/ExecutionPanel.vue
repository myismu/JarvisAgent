<script setup lang="ts">
import { computed } from "vue";
import type {
  AgentDisplayMode,
  AgentExecutionLog,
  AgentThinkingBlock,
  AgentToolCallView,
} from "../../types";
import { renderMarkdown, renderToolStatusIcon } from "../../utils/markdown";

const props = defineProps<{
  mode: AgentDisplayMode;
  running: boolean;
  thinkingBlocks: AgentThinkingBlock[];
  toolCalls: AgentToolCallView[];
  logs: AgentExecutionLog[];
}>();

const thinkingItems = computed(() => props.thinkingBlocks.filter((item) => item.content.trim()));
const logItems = computed(() => props.logs.filter((item) => item.content.trim()));
const hasExecution = computed(
  () => thinkingItems.value.length > 0 || props.toolCalls.length > 0 || logItems.value.length > 0,
);
const isDeveloperMode = computed(() => props.mode === "developer");

const summaryText = computed(() => {
  const state = props.running ? "处理中" : "已完成";
  const toolPart = props.toolCalls.length > 0 ? `${props.toolCalls.length} 个工具` : "无工具调用";
  const thinkingPart = thinkingItems.value.length > 0 ? ` · ${thinkingItems.value.length} 段思考` : "";
  const logPart = props.mode === "developer" && logItems.value.length > 0 ? ` · ${logItems.value.length} 条日志` : "";
  return `${state} · ${toolPart}${thinkingPart}${logPart}`;
});

const statusLabel = (status: string) => {
  if (status === "completed") return "已完成";
  if (status === "error") return "失败";
  if (status === "running") return "执行中";
  return "等待中";
};

const hasToolDetails = (tool: AgentToolCallView) => {
  return Boolean(tool.inputSummary || tool.outputSummary || tool.error || tool.logs.length);
};

const isSubAgentTool = (tool: AgentToolCallView) => {
  const name = (tool.name || "").toLowerCase();
  return name === "task" || name === "run_subagent" || name.includes("subagent");
};

const shouldOpenTool = (tool: AgentToolCallView) => {
  return isDeveloperMode.value && hasToolDetails(tool) && (tool.status === "running" || tool.status === "pending");
};

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

    <div v-if="toolCalls.length" class="agent-tool-list">
      <details
        v-for="tool in toolCalls"
        :key="tool.id"
        class="agent-tool-call"
        :class="{ 'agent-subagent-tool': isSubAgentTool(tool) }"
        :open="shouldOpenTool(tool)"
      >
        <summary class="agent-tool-row" :class="tool.status">
          <span class="agent-tool-icon" v-html="renderToolStatusIcon(tool.status)"></span>
          <code>{{ tool.name }}</code>
          <span>{{ statusLabel(tool.status) }}</span>
        </summary>
        <div v-if="tool.inputSummary" class="agent-tool-field">
          <span>参数</span>
          <div v-html="markdown(tool.inputSummary)"></div>
        </div>
        <div v-if="tool.outputSummary" class="agent-tool-field">
          <span>结果</span>
          <div v-html="markdown(tool.outputSummary)"></div>
        </div>
        <div v-if="tool.error" class="agent-tool-field error">
          <span>错误</span>
          <div v-html="markdown(tool.error)"></div>
        </div>
        <div v-for="(log, index) in tool.logs" :key="`${tool.id}_${index}`" class="agent-tool-log" v-html="markdown(log)"></div>
      </details>
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
