<script setup lang="ts">
import type { AgentToolCallView, AgentDisplayMode } from "../../types";
import { renderMarkdown, renderToolStatusIcon } from "../../utils/markdown";
import {
  hasToolDetails,
  isSubAgentToolGroup,
  shouldOpenToolGroup,
  toolActionCountLabel,
  toolActionLabel,
  toolGroupActionLabel,
  toolGroupTitle,
  type ToolCallGroup,
} from "../../utils/toolDisplay";

defineProps<{
  group: ToolCallGroup;
  mode: AgentDisplayMode;
}>();

const markdown = (content?: string) => renderMarkdown(content || "");

const toolKey = (tool: AgentToolCallView, index: number) => `${tool.id}-${index}`;

const technicalOpen = (group: ToolCallGroup) => group.status === "error";
</script>

<template>
  <details
    class="agent-tool-call"
    :class="{
      'agent-tool-group': group.count > 1,
      'agent-subagent-tool': isSubAgentToolGroup(group),
    }"
    :open="shouldOpenToolGroup(group, mode)"
  >
    <summary class="agent-tool-row" :class="group.status">
      <span class="agent-tool-icon" v-html="renderToolStatusIcon(group.status)"></span>
      <span class="agent-tool-title">{{ toolGroupTitle(group) }}</span>
      <span class="agent-tool-summary">{{ toolGroupActionLabel(group) }}</span>
    </summary>

    <div class="agent-tool-action-list">
      <div
        v-for="action in group.actions"
        :key="action.key"
        class="agent-tool-action-row"
        :class="action.status"
      >
        <span class="agent-tool-icon" v-html="renderToolStatusIcon(action.status)"></span>
        <span>{{ toolActionCountLabel(action) }}</span>
        <span class="agent-tool-action-summary">{{ action.summary }}</span>
      </div>
    </div>

    <details class="agent-tool-technical" :open="technicalOpen(group)">
      <summary>技术详情 · {{ group.count }} 步</summary>
      <div class="agent-tool-group-items">
        <div
          v-for="(tool, index) in group.tools"
          :key="toolKey(tool, index)"
          class="agent-tool-child"
          :class="{ 'has-details': hasToolDetails(tool) }"
        >
          <div class="agent-tool-child-row" :class="tool.status">
            <span class="agent-tool-icon" v-html="renderToolStatusIcon(tool.status)"></span>
            <span>{{ toolActionLabel(tool.name, tool.status, tool) }}</span>
            <code>{{ tool.name }}</code>
          </div>
          <div v-if="tool.inputSummary" class="agent-tool-field">
            <span>参数</span>
            <div v-html="markdown(tool.inputSummary)"></div>
          </div>
          <div v-if="tool.outputSummary" class="agent-tool-field">
            <span>输出</span>
            <div v-html="markdown(tool.outputSummary)"></div>
          </div>
          <div v-if="tool.error" class="agent-tool-field error">
            <span>错误</span>
            <div v-html="markdown(tool.error)"></div>
          </div>
          <div
            v-for="(log, logIndex) in tool.logs"
            :key="`${tool.id}_${logIndex}`"
            class="agent-tool-log"
            v-html="markdown(log)"
          ></div>
        </div>
      </div>
    </details>
  </details>
</template>

<style scoped>
.agent-tool-action-list {
  display: flex;
  flex-direction: column;
  gap: 6px;
  margin-top: 8px;
  padding-left: 22px;
}

.agent-tool-title {
  color: var(--text-main);
  font-weight: 650;
}

.agent-tool-summary {
  color: var(--text-muted);
}

.agent-tool-action-row {
  display: inline-flex;
  align-items: center;
  gap: 8px;
  min-height: 22px;
  color: var(--text-muted);
  font-size: 0.82rem;
}

.agent-tool-action-row.completed {
  color: var(--accent-green);
}

.agent-tool-action-row.error {
  color: var(--accent-red);
}

.agent-tool-action-row.running {
  color: var(--accent-yellow);
}

.agent-tool-action-summary {
  color: var(--text-muted);
}

.agent-tool-technical {
  margin-top: 8px;
}

.agent-tool-group-items {
  display: flex;
  flex-direction: column;
  gap: 8px;
  margin-top: 8px;
  padding-left: 22px;
}

.agent-tool-child {
  padding-left: 10px;
}

.agent-tool-child-row {
  display: inline-flex;
  align-items: center;
  gap: 8px;
  min-height: 22px;
  color: var(--text-muted);
  font-size: 0.82rem;
}

.agent-tool-child-row.completed {
  color: var(--accent-green);
}

.agent-tool-child-row.error {
  color: var(--accent-red);
}

.agent-tool-child-row.running {
  color: var(--accent-yellow);
}

.agent-tool-child-row code {
  color: var(--text-main);
  background: transparent;
  padding: 0;
  border: 0;
}

.agent-tool-child .agent-tool-field,
.agent-tool-child .agent-tool-log {
  padding-left: 22px;
}
</style>
