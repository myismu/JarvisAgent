<script setup lang="ts">
import { computed } from "vue";
import type {
  AgentCurrentTurn,
  AgentDisplayMode,
  AgentExecutionLog,
  AgentTextBlock,
  AgentThinkingBlock,
  AgentToolCallView,
} from "../../types";
import { renderMarkdown, renderToolStatusIcon } from "../../utils/markdown";
import { stripPseudoToolCalls } from "../../utils/agentTurnRender";
import ExecutionPanel from "./ExecutionPanel.vue";
import ThinkingStatus from "./ThinkingStatus.vue";

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
  toolCall?: AgentToolCallView;
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
    segments.push({
      key: `tool-${tool.id}`,
      type: "tool",
      timestamp: tool.timestamp,
      order: index * 4 + 2,
      toolCall: tool,
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

  return segments.sort((a, b) => a.timestamp - b.timestamp || a.order - b.order);
});

const hasDeveloperSegments = computed(() => developerSegments.value.length > 0);

const statusLabel = (status: string) => {
  if (status === "completed") return "调用结果";
  if (status === "error") return "调用失败";
  if (status === "running") return "工具调用中";
  return "等待调用";
};

const hasToolDetails = (tool: AgentToolCallView) => {
  return Boolean(tool.inputSummary || tool.outputSummary || tool.error || tool.logs.length);
};

const isSubAgentTool = (tool: AgentToolCallView) => {
  const name = (tool.name || "").toLowerCase();
  return name === "task" || name === "run_subagent" || name.includes("subagent");
};

const shouldOpenTool = (tool: AgentToolCallView) => {
  return hasToolDetails(tool) && (tool.status === "running" || tool.status === "pending");
};

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

const markdown = (content?: string) => renderMarkdown(content || "");
</script>

<template>
  <div
    class="agent-turn"
    :class="[displayMode, { 'waiting-only': !hasAssistantText && !hasExecution && showStatus }]"
  >
    <div v-if="isDeveloperMode && hasDeveloperSegments" class="agent-developer-timeline">
      <template
        v-for="segment in developerSegments"
        :key="`${segment.key}-${segment.thinkingBlock ? isThinkingOpen(segment.thinkingBlock) : segment.toolCall?.status || 'stable'}`"
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

        <details
          v-else-if="segment.type === 'tool' && segment.toolCall"
          class="agent-tool-call"
          :class="{ 'agent-subagent-tool': isSubAgentTool(segment.toolCall) }"
          :open="shouldOpenTool(segment.toolCall)"
        >
          <summary class="agent-tool-row" :class="segment.toolCall.status">
            <span class="agent-tool-icon" v-html="renderToolStatusIcon(segment.toolCall.status)"></span>
            <code>{{ segment.toolCall.name }}</code>
            <span>{{ statusLabel(segment.toolCall.status) }}</span>
          </summary>
          <div v-if="segment.toolCall.inputSummary" class="agent-tool-field">
            <span>参数</span>
            <div v-html="markdown(segment.toolCall.inputSummary)"></div>
          </div>
          <div v-if="segment.toolCall.outputSummary" class="agent-tool-field">
            <span>结果</span>
            <div v-html="markdown(segment.toolCall.outputSummary)"></div>
          </div>
          <div v-if="segment.toolCall.error" class="agent-tool-field error">
            <span>错误</span>
            <div v-html="markdown(segment.toolCall.error)"></div>
          </div>
          <div
            v-for="(log, index) in segment.toolCall.logs"
            :key="`${segment.toolCall.id}_${index}`"
            class="agent-tool-log"
            v-html="markdown(log)"
          ></div>
        </details>

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
