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
  toolGroupTitle,
  type ToolCallGroup,
} from "../../utils/toolDisplay";
import { renderTokenUsage, renderToolStatusIcon } from "../../utils/markdown";
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

const tokenUsageHtml = computed(() => {
  if (!props.turn.tokens) return "";
  return renderTokenUsage(
    props.turn.tokens.input,
    props.turn.tokens.output,
    props.turn.tokens.sessionInput,
    props.turn.tokens.sessionOutput,
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

</script>

<template>
  <div
    class="agent-turn"
    :class="[displayMode, { 'waiting-only': !hasAssistantText && !hasExecution && showStatus }]"
  >
    <!-- 开发者模式：Cursor 风格侧边轨迹 -->
    <div v-if="isDeveloperMode && hasDeveloperSegments" class="agent-developer-layout">
      <div class="technical-trace">
        <template
          v-for="segment in developerSegments"
          :key="`${segment.key}-${segment.thinkingBlock ? isThinkingOpen(segment.thinkingBlock) : segment.toolGroup?.status || 'stable'}`"
        >
          <!-- 思考过程：胶囊化 -->
          <div
            v-if="segment.type === 'thinking' && segment.thinkingBlock"
            class="trace-capsule thinking-capsule"
            :class="{ streaming: isThinkingOpen(segment.thinkingBlock) }"
            :title="segment.thinkingBlock.content"
          >
            <div class="capsule-icon">
              <svg viewBox="0 0 24 24" width="12" height="12" stroke="currentColor" stroke-width="2.5" fill="none"><path d="M12 2v4M12 18v4M4.93 4.93l2.83 2.83M16.24 16.24l2.83 2.83M2 12h4M18 12h4M4.93 19.07l2.83-2.83M16.24 7.76l2.83-2.83"/></svg>
            </div>
            <span class="capsule-label">{{ describeThinking(segment.thinkingBlock.content) }}</span>
          </div>

          <!-- 工具调用：胶囊化 -->
          <div
            v-else-if="segment.type === 'tool' && segment.toolGroup"
            class="trace-capsule tool-capsule"
            :class="segment.toolGroup.status"
          >
            <div class="capsule-icon" v-html="renderToolStatusIcon(segment.toolGroup.status)"></div>
            <span class="capsule-label">{{ toolGroupTitle(segment.toolGroup) }}</span>
            <span v-if="segment.toolGroup.count > 1" class="capsule-badge">{{ segment.toolGroup.count }}</span>
          </div>

          <!-- 执行日志：微型终端流 -->
          <div
            v-else-if="segment.type === 'log' && segment.log"
            class="mini-terminal-log"
          >
            <div class="terminal-header">
              <span class="terminal-dot red"></span>
              <span class="terminal-dot yellow"></span>
              <span class="terminal-dot green"></span>
              <span class="terminal-title">LOG #{{ segment.log.loop || 1 }}</span>
            </div>
            <div class="terminal-content" v-html="segment.html"></div>
          </div>

          <!-- 文本段落：主干内容 -->
          <div
            v-else-if="segment.type === 'text'"
            class="agent-turn-answer trace-main-text"
            v-html="segment.html"
          ></div>
        </template>
      </div>
    </div>

    <!-- 普通模式：清爽面板 -->
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

    <ThinkingStatus :running="showStatus" :elapsed="elapsed" />
  </div>
</template>

<style scoped>
.agent-turn {
  position: relative;
  width: 100%;
}

.agent-turn-tokens {
  margin-top: 12px;
  width: 100%;
}

.agent-turn.waiting-only {
  min-width: auto;
  min-height: 34px;
  display: inline-flex;
  align-items: center;
  justify-content: flex-start;
}

/* 开发者模式布局 */
.agent-developer-layout {
  display: flex;
  flex-direction: column;
  gap: 16px;
  padding-bottom: 24px;
}

.technical-trace {
  display: flex;
  flex-direction: column;
  gap: 12px;
  position: relative;
}

/* 技术胶囊 (Thinking/Tools) */
.trace-capsule {
  display: inline-flex;
  align-items: center;
  gap: 8px;
  padding: 4px 12px;
  background: var(--glass-bg-light);
  border: 1px solid var(--glass-border-subtle);
  border-radius: 20px;
  font-size: 0.75rem;
  font-weight: 550;
  color: var(--text-muted);
  width: fit-content;
  max-width: 100%;
  transition: all var(--transition-fast);
  cursor: help;
  user-select: none;
}

.trace-capsule:hover {
  background: var(--glass-bg);
  border-color: var(--accent-blue);
  color: var(--text-main);
  transform: translateX(4px);
}

.capsule-icon {
  display: flex;
  align-items: center;
  justify-content: center;
  flex-shrink: 0;
}

.capsule-label {
  white-space: nowrap;
  overflow: hidden;
  text-overflow: ellipsis;
}

/* 思考胶囊特有样式 */
.thinking-capsule.streaming .capsule-icon svg {
  animation: spin 2s linear infinite;
}

.thinking-capsule.streaming {
  border-color: var(--accent-yellow);
  color: var(--accent-yellow);
  background: rgba(245, 158, 11, 0.05);
}

/* 工具胶囊状态样式 */
.tool-capsule.completed {
  border-color: rgba(16, 185, 129, 0.2);
  color: var(--accent-green);
}

.tool-capsule.error {
  border-color: rgba(239, 68, 68, 0.2);
  color: var(--accent-red);
}

.tool-capsule.running {
  border-color: rgba(245, 158, 11, 0.2);
  color: var(--accent-yellow);
}

.capsule-badge {
  background: var(--glass-border);
  color: var(--text-muted);
  padding: 0 5px;
  border-radius: 4px;
  font-size: 0.65rem;
  font-family: var(--font-mono);
}

/* 微型终端日志 */
.mini-terminal-log {
  background: #0f172a; /* 深色终端背景 */
  border-radius: var(--radius-md);
  border: 1px solid rgba(255, 255, 255, 0.1);
  overflow: hidden;
  max-width: 600px;
  box-shadow: 0 4px 12px rgba(0, 0, 0, 0.2);
  margin: 4px 0;
}

.terminal-header {
  background: rgba(255, 255, 255, 0.05);
  padding: 4px 10px;
  display: flex;
  align-items: center;
  gap: 6px;
  border-bottom: 1px solid rgba(255, 255, 255, 0.05);
}

.terminal-dot {
  width: 6px;
  height: 6px;
  border-radius: 50%;
}
.terminal-dot.red { background: #ef4444; }
.terminal-dot.yellow { background: #f59e0b; }
.terminal-dot.green { background: #10b981; }

.terminal-title {
  font-family: var(--font-mono);
  font-size: 0.65rem;
  color: rgba(255, 255, 255, 0.4);
  letter-spacing: 1px;
}

.terminal-content {
  padding: 8px 12px;
  font-family: var(--font-mono);
  font-size: 0.8rem;
  color: #e2e8f0;
  max-height: 120px;
  overflow-y: auto;
  line-height: 1.5;
}

.terminal-content :deep(pre), 
.terminal-content :deep(code) {
  background: transparent;
  padding: 0;
  border: none;
  color: inherit;
  font-size: inherit;
}

/* 主文本段落 */
.trace-main-text {
  padding: 4px 0;
}

.agent-turn:not(.developer) .agent-turn-answer {
  padding-bottom: 24px;
}

@keyframes spin {
  from { transform: rotate(0deg); }
  to { transform: rotate(360deg); }
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
</style>
