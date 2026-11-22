<!--
# SubAgentChatPane.vue — 子 Agent 历史列表与只读上下文

在主对话区域内展示子 Agent 的任务、模型回复、思考过程、工具调用和工具结果。
每轮实际模型请求保留为可展开的原始快照，便于核对完整上下文和请求参数。

## Key Exports
- `SubAgentChatPane`: 子 Agent 列表选择与逐轮只读轨迹视图

## Dependencies
- Internal: `../../types`, `../../utils/agentTurnRender`, `./ExecutionTimeline.vue`
- External: `vue`, `vue-i18n`

## Constraints
- 子 Agent 详情只读，不创建第二套可继续对话的消息流。
- 轨迹由后端按 run 持久化；本组件只负责按轮展示已落库记录。
-->
<script setup lang="ts">
import { computed, nextTick, onMounted, ref, watch } from 'vue';
import { useI18n } from 'vue-i18n';
import type { AgentToolCallView, AgentToolStatus, SubAgentConversationItem, SubAgentTraceEvent } from '../../types';
import ExecutionTimeline from './ExecutionTimeline.vue';
import { stripPseudoToolCalls, type DevTimelineItem } from '../../utils/agentTurnRender';
import { isLongUserText } from '../../utils/userMessage';

interface TraceRequest {
  model?: string | null;
  payload: Record<string, unknown>;
}

interface TraceToolCall {
  id: string;
  name: string;
  input: unknown;
}

interface TraceToolResult {
  toolUseId: string;
  tool: string;
  output: string;
  isError: boolean;
}

interface TraceLoop {
  loopIndex: number;
  request: TraceRequest | null;
  response: Record<string, unknown> | null;
  toolResults: TraceToolResult[];
}

/** 渲染视图：在 TraceLoop 上补出主对话同款的执行过程条目。 */
interface LoopView extends TraceLoop {
  /** 本轮响应映射成主对话同款的执行过程条目 */
  timelineItems: DevTimelineItem[];
}

const props = defineProps<{
  runs: SubAgentConversationItem[];
  activeRunId: string | null;
  run: SubAgentConversationItem | null;
  events: SubAgentTraceEvent[];
}>();

const emit = defineEmits<{
  (event: 'select-run', runId: string): void;
}>();

const { t } = useI18n();
const transcriptRef = ref<HTMLElement | null>(null);
// 初始任务气泡的展开态。任务提示词经常是一整段长文本，默认折叠，避免把整轮
// 执行内容顶出首屏；切换子 Agent 时随其他展开态一起重置（见下方 watch）。
const expandedTask = ref(false);
const selectedRun = computed(() => props.run);

function asRecord(value: unknown): Record<string, unknown> | null {
  return value !== null && typeof value === 'object' && !Array.isArray(value)
    ? value as Record<string, unknown>
    : null;
}

function asString(value: unknown): string {
  return typeof value === 'string' ? value : '';
}

function formatJson(value: unknown): string {
  if (value === undefined) return '';
  try {
    return JSON.stringify(value, null, 2);
  } catch {
    return String(value);
  }
}

function contentText(content: unknown): string {
  if (typeof content === 'string') return content;
  if (!Array.isArray(content)) return '';
  return content.map((part) => {
    const block = asRecord(part);
    if (!block) return '';
    const type = asString(block.type);
    if (type === 'text' || type === 'context') return asString(block.text);
    if (type === 'thinking') return asString(block.thinking);
    if (type === 'tool_use' || type === 'tool_call') {
      const name = asString(block.name) || asString(asRecord(block.function)?.name) || 'tool';
      const input = block.input ?? asRecord(block.function)?.arguments ?? null;
      return `工具调用 ${name}\n${formatJson(input)}`;
    }
    if (type === 'tool_result') {
      // tool_result 的 content 可能是字符串或内容块数组；递归取值，取不到再退回 JSON。
      const inner = contentText(block.content).trim();
      return inner || (block.content === undefined ? '' : formatJson(block.content));
    }
    if (type === 'image') return '[图片]';
    return '';
  }).filter(Boolean).join('\n');
}

function requestPayload(event: SubAgentTraceEvent): TraceRequest {
  const payload = asRecord(event.payload) ?? {};
  return {
    model: event.model ?? (asString(payload.model) || null),
    payload,
  };
}

function eventPayload(event: SubAgentTraceEvent): Record<string, unknown> {
  return asRecord(event.payload) ?? {};
}

const sortedEvents = computed(() => [...props.events].sort((a, b) =>
  a.loopIndex - b.loopIndex || a.timestamp - b.timestamp
));

const loops = computed<TraceLoop[]>(() => {
  const byLoop = new Map<number, TraceLoop>();
  for (const event of sortedEvents.value) {
    const current = byLoop.get(event.loopIndex) ?? {
      loopIndex: event.loopIndex,
      request: null,
      response: null,
      toolResults: [],
    };
    const payload = eventPayload(event);
    if (event.eventType === 'request') {
      current.request = requestPayload(event);
    } else if (event.eventType === 'response') {
      current.response = payload;
    } else if (event.eventType === 'tool_result') {
      current.toolResults.push({
        toolUseId: asString(payload.toolUseId),
        tool: asString(payload.tool),
        output: asString(payload.output),
        isError: payload.isError === true,
      });
    }
    byLoop.set(event.loopIndex, current);
  }
  return [...byLoop.values()].sort((a, b) => a.loopIndex - b.loopIndex);
});

const loopViews = computed<LoopView[]>(() =>
  loops.value.map((loop) => ({
    ...loop,
    timelineItems: buildLoopTimeline(loop),
  })),
);

/** 所有轮次的执行条目按顺序拼成一条连续时间线，与主对话执行过程一致。 */
const allTimelineItems = computed<DevTimelineItem[]>(() =>
  loopViews.value.flatMap((loop) => loop.timelineItems),
);

/**
 * 与 AgentTurn 相同的拆分口径：
 * - 运行中：全部留在执行过程里；
 * - 已结束：最后一段正文提升为最终回答，用正文口径渲染（颜色更重）。
 */
const timelineSplit = computed(() => {
  const items = allTimelineItems.value;
  if (selectedRun.value?.status === 'running') {
    return { execItems: items, finalItems: [] as DevTimelineItem[] };
  }
  let lastTextIdx = -1;
  for (let i = items.length - 1; i >= 0; i -= 1) {
    if (items[i].type === 'text') {
      lastTextIdx = i;
      break;
    }
  }
  if (lastTextIdx < 0) {
    return { execItems: items, finalItems: [] as DevTimelineItem[] };
  }
  return {
    execItems: items.filter((_, i) => i !== lastTextIdx),
    finalItems: [items[lastTextIdx]],
  };
});

/**
 * 把一轮响应映射成主对话同款的执行时间线条目。
 *
 * 顺序以响应 blocks 的顺序为准（思考 / 正文 / 工具调用），这样文本和工具调用
 * 能像主对话执行过程一样交错出现；工具结果按 tool_use_id 回填到对应工具上。
 * 没有匹配到工具调用的结果（中断 / 旧数据）仍作为工具行保留，避免静默丢失。
 */
function buildLoopTimeline(loop: TraceLoop): DevTimelineItem[] {
  const response = loop.response;
  if (!response) return [];
  const items: DevTimelineItem[] = [];
  const usedResults = new Set<TraceToolResult>();

  const pushThinking = (content: string, timestamp: number) => {
    const text = content.trim();
    if (text) items.push({ type: 'thinking', content: text, status: 'completed', streaming: false, timestamp });
  };
  const pushText = (content: string, timestamp: number) => {
    const text = stripPseudoToolCalls(content).trim();
    if (text) items.push({ type: 'text', content: text, timestamp });
  };
  const pushTool = (call: TraceToolCall, callIndex: number, timestamp: number) => {
    const result = toolResultFor(loop, call, callIndex);
    if (result) usedResults.add(result);
    const status: AgentToolStatus = result ? (result.isError ? 'error' : 'completed') : 'running';
    const tool: AgentToolCallView = {
      id: call.id,
      loop: loop.loopIndex,
      name: call.name,
      status,
      input: call.input === undefined || call.input === null
        ? undefined
        : (typeof call.input === 'string' ? call.input : formatJson(call.input)),
      output: result && !result.isError ? result.output : undefined,
      error: result && result.isError ? result.output : undefined,
      logs: [],
      timestamp,
      updatedAt: timestamp,
    };
    items.push({ type: 'tool', tool, streaming: false, timestamp });
  };

  const blocks = Array.isArray(response.blocks) ? response.blocks : [];
  if (blocks.length > 0) {
    let toolIndex = 0;
    blocks.forEach((value, index) => {
      const block = asRecord(value);
      if (!block) return;
      const type = asString(block.type);
      if (type === 'text') {
        pushText(asString(block.text), index);
      } else if (type === 'thinking') {
        pushThinking(asString(block.thinking), index);
      } else if (type === 'tool_use' || type === 'tool_call') {
        const functionCall = asRecord(block.function);
        pushTool({
          id: asString(block.id) || asString(block.tool_use_id) || `tool-${toolIndex}`,
          name: asString(block.name) || asString(functionCall?.name) || 'Tool',
          input: block.input ?? functionCall?.arguments ?? null,
        }, toolIndex, index);
        toolIndex += 1;
      }
    });
  } else {
    pushThinking(responseThinking(response), 0);
    pushText(responseText(response), 1);
    toolCalls(response).forEach((call, index) => pushTool(call, index, index + 2));
  }

  let orphanIndex = 0;
  for (const result of loop.toolResults) {
    if (usedResults.has(result)) continue;
    const timestamp = blocks.length + orphanIndex;
    const tool: AgentToolCallView = {
      id: `orphan-${loop.loopIndex}-${orphanIndex}`,
      loop: loop.loopIndex,
      name: result.tool || 'Tool',
      status: result.isError ? 'error' : 'completed',
      output: result.isError ? undefined : result.output,
      error: result.isError ? result.output : undefined,
      logs: [],
      timestamp,
      updatedAt: timestamp,
    };
    items.push({ type: 'tool', tool, streaming: false, timestamp });
    orphanIndex += 1;
  }

  return items;
}

const initialTask = computed(() => {
  const firstRequest = loops.value.find((loop) => loop.request)?.request?.payload;
  const messages = firstRequest?.messages;
  if (Array.isArray(messages)) {
    const initialUserMessage = messages.find((value) => asString(asRecord(value)?.role) === 'user');
    const text = contentText(asRecord(initialUserMessage)?.content).trim();
    if (text) return text;
  }
  return selectedRun.value?.promptPreview || '';
});

// 与主对话共用同一套长消息判定（utils/userMessage）：两处阈值不会再漂移。
const taskIsLong = computed(() => isLongUserText(initialTask.value));

function responseText(response: Record<string, unknown> | null): string {
  if (!response) return '';
  const direct = asString(response.text).trim();
  if (direct) return direct;
  if (!Array.isArray(response.blocks)) return '';
  return response.blocks.map((value) => {
    const block = asRecord(value);
    return block && block.type === 'text' ? asString(block.text) : '';
  }).filter(Boolean).join('\n');
}

function responseThinking(response: Record<string, unknown> | null): string {
  if (!response) return '';
  const direct = asString(response.thinking).trim();
  if (direct) return direct;
  if (!Array.isArray(response.blocks)) return '';
  return response.blocks.map((value) => {
    const block = asRecord(value);
    return block && block.type === 'thinking' ? asString(block.thinking) : '';
  }).filter(Boolean).join('\n');
}

const toolCallCache = new WeakMap<Record<string, unknown>, TraceToolCall[]>();

function toolCalls(response: Record<string, unknown> | null): TraceToolCall[] {
  if (!response || !Array.isArray(response.blocks)) return [];
  const cached = toolCallCache.get(response);
  if (cached) return cached;
  const parsed = response.blocks.flatMap((value, index) => {
    const block = asRecord(value);
    if (!block || (block.type !== 'tool_use' && block.type !== 'tool_call')) return [];
    const functionCall = asRecord(block.function);
    return [{
      id: asString(block.id) || asString(block.tool_use_id) || `tool-${index}`,
      name: asString(block.name) || asString(functionCall?.name) || 'Tool',
      input: block.input ?? functionCall?.arguments ?? null,
    }];
  });
  toolCallCache.set(response, parsed);
  return parsed;
}

function toolResultFor(loop: TraceLoop, call: TraceToolCall, callIndex: number): TraceToolResult | null {
  return loop.toolResults.find((result) => result.toolUseId && result.toolUseId === call.id)
    ?? loop.toolResults.find((result, index) => index === callIndex && result.tool === call.name)
    ?? null;
}

function statusLabel(run: SubAgentConversationItem | null): string {
  if (!run) return t('monitor.subAgentChat.saved');
  if (run.status === 'saved') return t('monitor.subAgentChat.saved');
  switch (run.status) {
    case 'running': return t('monitor.subAgentStatus.running');
    case 'completed': return t('monitor.subAgentStatus.completed');
    case 'failed': return t('monitor.subAgentStatus.failed');
    case 'cancelled': return t('monitor.subAgentStatus.cancelled');
    default: return run.status;
  }
}

function phaseLabel(phase: string): string {
  const localizedPhase = ({
    waiting_model: 'waitingModel',
    calling_tool: 'callingTool',
    processing_tool_result: 'processingToolResult',
  } as Record<string, string>)[phase] ?? phase;
  const key = `monitor.subAgentPhase.${localizedPhase}`;
  const translated = t(key);
  return translated === key ? phase : translated;
}

function formatTokens(value: number): string {
  if (value >= 1_000_000) return `${(value / 1_000_000).toFixed(1)}m`;
  if (value >= 1000) return `${(value / 1000).toFixed(1)}k`;
  return String(value);
}

async function scrollToLatestIfFollowing() {
  const element = transcriptRef.value;
  if (!element) return;
  const wasFollowing = element.scrollHeight - element.scrollTop - element.clientHeight < 120;
  await nextTick();
  if (wasFollowing) element.scrollTop = element.scrollHeight;
}

watch(() => [props.activeRunId, props.events.length], () => {
  void scrollToLatestIfFollowing();
}, { flush: 'post' });

watch(() => props.activeRunId, () => {
  expandedTask.value = false;
});

onMounted(async () => {
  await nextTick();
  if (transcriptRef.value) transcriptRef.value.scrollTop = transcriptRef.value.scrollHeight;
});
</script>

<template>
  <aside class="subagent-chat-pane">
    <nav v-if="runs.length" class="subagent-run-selector" role="tablist" :aria-label="t('monitor.subAgentChat.subAgents')">
      <button
        v-for="item in runs"
        :key="item.runId"
        class="subagent-run-option"
        :class="{ active: item.runId === activeRunId }"
        role="tab"
        :aria-selected="item.runId === activeRunId"
        :title="item.label || item.runId"
        @click="emit('select-run', item.runId)"
      >
        <strong class="run-option-title">{{ item.label || item.runId }}</strong>
        <span class="run-option-meta">
          <span class="run-option-status" :class="item.status">{{ statusLabel(item) }}</span>
          <span aria-hidden="true">·</span>
          <span>{{ t('monitor.subAgentChat.loopCount', { count: item.loopCount }) }}</span>
        </span>
      </button>
    </nav>
    <template v-if="selectedRun">
      <section class="subagent-chat-detail">
      <header class="subagent-chat-header">
        <div class="subagent-chat-heading">
          <div class="heading-copy">
            <strong>{{ selectedRun.label || activeRunId }}</strong>
            <span>
              {{ statusLabel(selectedRun) }}
              <template v-if="selectedRun?.status === 'running'">
                · {{ phaseLabel(selectedRun.phase) }} · {{ t('monitor.subAgentChat.loopProgress', { count: selectedRun.loopCount, max: selectedRun.maxLoops }) }}
                <template v-if="selectedRun.currentTool"> · {{ t('monitor.currentTool') }}: {{ selectedRun.currentTool }}</template>
              </template>
              <template v-else-if="selectedRun"> · {{ t('monitor.subAgentChat.loopCount', { count: loops.length }) }}</template>
              <template v-if="selectedRun && selectedRun.status !== 'saved'"> · {{ t('monitor.subAgentChat.tokenCount', { count: formatTokens(selectedRun.inputTokens + selectedRun.outputTokens) }) }}</template>
            </span>
          </div>
        </div>
      </header>

      <div class="subagent-transcript" ref="transcriptRef">
        <div class="subagent-transcript-content">
        <div v-if="initialTask" class="transcript-message user-message">
          <div class="message-role">{{ t('monitor.subAgentChat.task') }}</div>
          <div class="user-bubble" :class="{ collapsed: taskIsLong && !expandedTask }">{{ initialTask }}</div>
          <button
            v-if="taskIsLong"
            class="user-msg-toggle"
            @click="expandedTask = !expandedTask"
          >
            {{ expandedTask ? t('chat.collapse') : t('chat.expandAll') }}
          </button>
        </div>

        <!-- 执行过程与主对话同款：一条连续时间线，不套折叠、不按轮次分组。 -->
        <ExecutionTimeline v-if="timelineSplit.execItems.length" :items="timelineSplit.execItems" compact />
        <!-- 最终回答用正文口径（不压缩），与执行过程形成层级差 -->
        <ExecutionTimeline v-if="timelineSplit.finalItems.length" :items="timelineSplit.finalItems" />

        <div v-if="!loops.length" class="transcript-empty">
          <span v-if="selectedRun?.status === 'running'">{{ t('monitor.subAgentChat.loading') }}</span>
          <span v-else>{{ t('monitor.subAgentChat.empty') }}</span>
        </div>

        <div v-if="selectedRun?.error" class="run-error">
          <strong>{{ t('monitor.subAgentChat.runFailed') }}</strong>
          <pre>{{ selectedRun.error }}</pre>
        </div>
        </div>
      </div>
      </section>
    </template>
    <div v-else class="transcript-empty">{{ t('monitor.subAgentChat.empty') }}</div>
  </aside>
</template>

<style scoped>
.subagent-chat-pane {
  flex: 1 1 100%;
  min-width: 0;
  min-height: 0;
  display: flex;
  flex-direction: row;
  overflow: hidden;
  background: var(--glass-bg-heavy);
}

.subagent-run-selector {
  flex: 0 0 260px;
  width: 260px;
  min-width: 0;
  display: flex;
  flex-direction: column;
  align-items: stretch;
  gap: 2px;
  padding: 10px 8px;
  overflow-x: hidden;
  overflow-y: auto;
  border-right: 1px solid var(--glass-border-subtle);
  background: color-mix(in srgb, var(--glass-bg) 55%, transparent);
}

.subagent-run-option {
  flex: 0 0 auto;
  width: 100%;
  min-width: 0;
  display: flex;
  flex-direction: column;
  align-items: flex-start;
  gap: 3px;
  padding: 8px 10px;
  border: 1px solid transparent;
  border-radius: var(--radius-md);
  background: transparent;
  color: var(--text-muted);
  cursor: pointer;
  text-align: left;
  transition: background var(--transition-fast), border-color var(--transition-fast), color var(--transition-fast);
}

.subagent-run-option:hover { border-color: var(--glass-border-subtle); background: var(--glass-bg-light); color: var(--text-main); }
.subagent-run-option.active { border-color: var(--glass-border); background: var(--glass-bg); color: var(--text-main); font-weight: 600; box-shadow: var(--shadow-sm); }
.run-option-title { display: block; width: 100%; overflow: hidden; color: inherit; font-size: 0.78rem; text-overflow: ellipsis; white-space: nowrap; }
.run-option-meta { display: flex; align-items: center; gap: 6px; max-width: 100%; color: var(--text-muted); font-size: 0.64rem; font-weight: 400; white-space: nowrap; }
.run-option-status.running { color: var(--accent-green); }
.run-option-status.failed { color: var(--accent-red); }
.run-option-status.completed { color: var(--accent-blue); }

.subagent-chat-detail { flex: 1; min-width: 0; min-height: 0; display: flex; flex-direction: column; overflow: hidden; }

.subagent-chat-header {
  min-height: 58px;
  display: flex;
  align-items: center;
  justify-content: center;
  gap: 12px;
  padding: 9px 0;
  border-bottom: 1px solid var(--glass-border-subtle);
  flex-shrink: 0;
}

.subagent-chat-heading { width: min(85%, 960px); min-width: 0; display: flex; align-items: center; }
.heading-copy { min-width: 0; display: flex; flex-direction: column; gap: 3px; }
.heading-copy strong { overflow: hidden; color: var(--text-main); font-size: 0.88rem; text-overflow: ellipsis; white-space: nowrap; }
.heading-copy > span { color: var(--text-muted); font-size: 0.73rem; }
.subagent-transcript {
  flex: 1;
  min-height: 0;
  overflow: auto;
  padding: 22px 0 36px;
  scroll-behavior: auto;
}

.subagent-transcript-content {
  width: min(85%, 960px);
  margin: 0 auto;
  display: flex;
  flex-direction: column;
  gap: 16px;
}

.transcript-message { display: flex; flex-direction: column; gap: 7px; margin: 0 0 20px; }
.message-role { color: var(--text-muted); font-size: 0.72rem; font-weight: 700; }
.user-message { align-items: flex-end; }
.user-bubble { max-width: 85%; padding: 14px 22px; border-radius: var(--radius-xl); border-bottom-right-radius: 4px; background: rgba(var(--msg-bg-rgb), calc(var(--user-message-opacity) / 100)); color: var(--text-main); font-size: 0.95rem; line-height: 1.6; white-space: pre-wrap; overflow-wrap: anywhere; }
/* 长任务折叠：与主对话一致的 180px 视窗 + 底部渐隐。
   渐隐终点必须用气泡自身底色（同一个 --msg-bg-rgb 与不透明度），不能用面板底色：
   气泡半透明时两者不是一个颜色，收尾处会露出一条明显的色带。 */
.user-bubble.collapsed { position: relative; max-height: 180px; overflow: hidden; }
.user-bubble.collapsed::after {
  content: '';
  position: absolute;
  right: 0;
  bottom: 0;
  left: 0;
  height: 48px;
  background: linear-gradient(transparent, rgba(var(--msg-bg-rgb), calc(var(--user-message-opacity) / 100)));
  pointer-events: none;
}
/* 展开/收起按钮与主对话同款（ChatArea 的 .user-msg-toggle）；
   气泡列是右对齐，按钮作为列内子项自然落在右侧。 */
.user-msg-toggle {
  margin-top: 6px;
  padding: 2px 0;
  border: 0;
  background: none;
  color: var(--accent-blue);
  font-family: inherit;
  font-size: 0.8rem;
  cursor: pointer;
}
.user-msg-toggle:hover { color: var(--accent-blue-hover); text-decoration: underline; }

.run-error pre { max-height: 320px; overflow: auto; margin: 0; padding: 11px 13px; border-top: 1px solid var(--glass-border-subtle); color: var(--text-soft); background: color-mix(in srgb, var(--bg-dark) 55%, transparent); font: 0.7rem/1.55 var(--font-mono); white-space: pre-wrap; overflow-wrap: anywhere; }

.transcript-empty { display: grid; min-height: 180px; place-items: center; color: var(--text-muted); font-size: 0.82rem; text-align: center; }
.run-error { margin-top: 18px; border: 1px solid color-mix(in srgb, var(--accent-red) 35%, transparent); border-radius: 9px; overflow: hidden; }
.run-error strong { display: block; padding: 9px 12px; color: var(--accent-red); font-size: 0.76rem; }

@media (max-width: 900px) {
  .subagent-run-selector { flex-basis: 220px; width: 220px; }
  .subagent-chat-heading,
  .subagent-transcript-content { width: min(90%, 960px); }
  .subagent-transcript { padding-top: 16px; }
}

@media (max-width: 640px) {
  .subagent-chat-pane { flex-direction: column; }
  .subagent-run-selector {
    flex: 0 0 160px;
    width: 100%;
    border-right: 0;
    border-bottom: 1px solid var(--glass-border-subtle);
  }
  .subagent-chat-heading,
  .subagent-transcript-content { width: calc(100% - 28px); }
  .user-bubble { max-width: 92%; padding: 12px 16px; }
}
</style>
