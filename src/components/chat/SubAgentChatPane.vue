<!--
# SubAgentChatPane.vue — 子 Agent 历史列表与只读上下文

在主对话区域内展示子 Agent 的任务、模型回复、思考过程、工具调用和工具结果。
每轮实际模型请求保留为可展开的原始快照，便于核对完整上下文和请求参数。

## Key Exports
- `SubAgentChatPane`: 子 Agent 列表选择与逐轮只读轨迹视图

## Dependencies
- Internal: `../../types`, `../common/StreamingMarkdown.vue`
- External: `vue`, `vue-i18n`

## Constraints
- 子 Agent 详情只读，不创建第二套可继续对话的消息流。
- 轨迹由后端按 run 持久化；本组件只负责按轮展示已落库记录。
-->
<script setup lang="ts">
import { computed, nextTick, onMounted, ref, watch } from 'vue';
import { useI18n } from 'vue-i18n';
import type { SubAgentConversationItem, SubAgentTraceEvent } from '../../types';
import StreamingMarkdown from '../common/StreamingMarkdown.vue';

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
const expandedRequestLoops = ref<Set<number>>(new Set());
const expandedToolCalls = ref<Set<string>>(new Set());
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
    if (type === 'tool_result') return asString(block.content);
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

function toolStatusLabel(result: TraceToolResult | null): string {
  if (!result) return t('monitor.subAgentChat.toolRunning');
  return result.isError
    ? t('monitor.subAgentChat.toolFailed')
    : t('monitor.subAgentChat.toolCompleted');
}

function toolCallKey(loopIndex: number, call: TraceToolCall): string {
  return `${loopIndex}:${call.id}`;
}

function updateExpandedSet<T>(target: { value: Set<T> }, key: T, expanded: boolean) {
  const next = new Set(target.value);
  if (expanded) next.add(key);
  else next.delete(key);
  target.value = next;
}

function onRequestToggle(loopIndex: number, event: Event) {
  updateExpandedSet(expandedRequestLoops, loopIndex, (event.currentTarget as HTMLDetailsElement).open);
}

function onToolToggle(loopIndex: number, call: TraceToolCall, event: Event) {
  updateExpandedSet(expandedToolCalls, toolCallKey(loopIndex, call), (event.currentTarget as HTMLDetailsElement).open);
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
  expandedRequestLoops.value = new Set();
  expandedToolCalls.value = new Set();
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
          <div class="user-bubble">{{ initialTask }}</div>
        </div>

        <article v-for="loop in loops" :key="loop.loopIndex" class="transcript-loop">
          <div class="loop-divider"><span>{{ t('monitor.subAgentChat.loop', { count: loop.loopIndex }) }}</span></div>

          <details v-if="loop.request" class="request-snapshot" :open="expandedRequestLoops.has(loop.loopIndex)" @toggle="onRequestToggle(loop.loopIndex, $event)">
            <summary>
              <span>{{ t('monitor.subAgentChat.requestContext') }}</span>
              <span v-if="loop.request.model" class="request-model">{{ loop.request.model }}</span>
            </summary>
            <pre v-if="expandedRequestLoops.has(loop.loopIndex)">{{ formatJson(loop.request.payload) }}</pre>
          </details>

          <div v-if="responseThinking(loop.response)" class="assistant-thinking">
            <details>
              <summary>{{ t('monitor.subAgentChat.thinking') }}</summary>
              <div class="thinking-content"><StreamingMarkdown :content="responseThinking(loop.response)" /></div>
            </details>
          </div>

          <div v-if="responseText(loop.response)" class="transcript-message assistant-message">
            <div class="message-role">{{ loop.request?.model || t('monitor.subAgentChat.title') }}</div>
            <div class="assistant-content"><StreamingMarkdown :content="responseText(loop.response)" /></div>
          </div>

          <div v-for="(call, callIndex) in toolCalls(loop.response)" :key="call.id" class="tool-observation">
            <details
              class="tool-call-card"
              :open="expandedToolCalls.has(toolCallKey(loop.loopIndex, call))"
              @toggle="onToolToggle(loop.loopIndex, call, $event)"
            >
              <summary>
                <code>{{ call.name }}</code>
                <span class="tool-summary-label">
                  {{ toolStatusLabel(toolResultFor(loop, call, callIndex)) }}
                </span>
              </summary>
              <div v-if="expandedToolCalls.has(toolCallKey(loop.loopIndex, call))" class="tool-detail">
                <div class="tool-detail-section">
                  <span>{{ t('monitor.subAgentChat.parameters') }}</span>
                  <pre>{{ formatJson(call.input) }}</pre>
                </div>
                <div v-if="toolResultFor(loop, call, callIndex)" class="tool-detail-section" :class="{ error: toolResultFor(loop, call, callIndex)?.isError }">
                  <span>{{ t('monitor.subAgentChat.result') }}</span>
                  <pre>{{ toolResultFor(loop, call, callIndex)?.output }}</pre>
                </div>
              </div>
            </details>
          </div>

          <div
            v-for="(result, index) in loop.toolResults.filter((item) => !toolCalls(loop.response).some((call, callIndex) => toolResultFor(loop, call, callIndex) === item))"
            :key="`${result.toolUseId}-${index}`"
            class="orphan-tool-result"
          >
            <strong>{{ result.tool }}</strong>
            <span>{{ result.isError ? t('monitor.subAgentChat.toolFailed') : t('monitor.subAgentChat.toolResult') }}</span>
            <pre>{{ result.output }}</pre>
          </div>
        </article>

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
.assistant-content { max-width: 100%; color: var(--text-main); line-height: 1.65; overflow-wrap: anywhere; }
.assistant-content :deep(p:last-child) { margin-bottom: 0; }

.transcript-loop { display: flex; flex-direction: column; gap: 12px; margin-bottom: 20px; }
.loop-divider { display: flex; align-items: center; gap: 10px; color: var(--text-muted); font-size: 0.7rem; }
.loop-divider::before,
.loop-divider::after { content: ''; height: 1px; flex: 1; background: var(--glass-border-subtle); }

.request-snapshot { border: 1px solid var(--glass-border-subtle); border-radius: 9px; background: color-mix(in srgb, var(--surface-strong) 35%, transparent); }
.request-snapshot summary,
.tool-call-card > summary,
.assistant-thinking summary { display: flex; align-items: center; gap: 8px; padding: 9px 11px; color: var(--text-soft); cursor: pointer; list-style: none; font-size: 0.75rem; }
.request-snapshot summary::-webkit-details-marker,
.tool-call-card > summary::-webkit-details-marker,
.assistant-thinking summary::-webkit-details-marker { display: none; }
.request-snapshot summary::before,
.tool-call-card > summary::before,
.assistant-thinking summary::before { content: '▸'; color: var(--text-muted); transition: transform 120ms ease; }
.request-snapshot[open] > summary::before,
.tool-call-card[open] > summary::before,
.assistant-thinking details[open] summary::before { transform: rotate(90deg); }
.request-model { margin-left: auto; color: var(--text-muted); font-size: 0.7rem; }
.request-snapshot pre,
.tool-detail pre,
.orphan-tool-result pre,
.run-error pre { max-height: 320px; overflow: auto; margin: 0; padding: 11px 13px; border-top: 1px solid var(--glass-border-subtle); color: var(--text-soft); background: color-mix(in srgb, var(--bg-dark) 55%, transparent); font: 0.7rem/1.55 var(--font-mono); white-space: pre-wrap; overflow-wrap: anywhere; }

.assistant-thinking details { border-left: 2px solid color-mix(in srgb, var(--accent-blue) 45%, transparent); }
.assistant-thinking summary { padding: 5px 9px; color: var(--text-muted); }
.thinking-content { padding: 5px 12px 10px; color: var(--text-soft); font-size: 0.82rem; }
.thinking-content :deep(p:last-child) { margin-bottom: 0; }

.tool-observation { margin-left: 12px; border: 1px solid var(--glass-border-subtle); border-radius: 9px; background: color-mix(in srgb, var(--surface-strong) 30%, transparent); }
.tool-call-card > summary { min-height: 38px; }
.tool-call-card > summary code { color: var(--text-main); font: 600 0.76rem var(--font-mono); }
.tool-summary-label { margin-left: auto; color: var(--text-muted); font-size: 0.7rem; }
.tool-detail { border-top: 1px solid var(--glass-border-subtle); }
.tool-detail-section > span { display: block; padding: 8px 11px 0; color: var(--text-muted); font-size: 0.68rem; font-weight: 700; }
.tool-detail-section.error > span { color: var(--accent-red); }
.tool-detail-section pre { border-top: 0; }
.orphan-tool-result { display: flex; flex-direction: column; gap: 6px; margin-left: 12px; border: 1px solid var(--glass-border-subtle); border-radius: 9px; overflow: hidden; }
.orphan-tool-result strong { padding: 9px 11px 0; color: var(--text-main); font: 600 0.76rem var(--font-mono); }
.orphan-tool-result > span { padding: 0 11px; color: var(--text-muted); font-size: 0.7rem; }

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
