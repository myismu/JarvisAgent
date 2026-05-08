<!--
# AgentPanel.vue — 开发者监控侧栏

展示上下文 token 监控、子 Agent 运行、后台任务和计划记录，提供紧凑但可读的运行概览。

## Key Exports
- `AgentPanel`: 右侧开发者监控面板组件

## Dependencies
- Internal: `@/stores/agent`, `@/stores/session`, `@/stores/permission`, `ContextInspector`
- External: `@tauri-apps/api/core`
-->
<script setup lang="ts">
import { computed, onUnmounted, ref, watch } from 'vue';
import { useI18n } from 'vue-i18n';
import { invoke } from '@tauri-apps/api/core';
import { getCurrentWindow } from '@tauri-apps/api/window';
import { emit, listen } from '@tauri-apps/api/event';
import { useWindow } from '../../composables/useWindow';
import { useSessionStore } from '../../stores/session';
import { useAgentStore } from '../../stores/agent';
import { usePermissionStore } from '../../stores/permission';
import type { BackgroundTask, PlanDocument, SubAgentEvent } from '../../types';
import ContextInspector from './ContextInspector.vue';

const session = useSessionStore();
const agent = useAgentStore();
const permission = usePermissionStore();

// ── 权限状态 ──
const permissionSessionAllowed = ref(false)
let permissionPollTimer: ReturnType<typeof setInterval> | null = null

const loadPermissionState = async () => {
  if (!session.activeSessionId) return
  try {
    const state = await invoke<any>('get_permission_state', { sessionId: session.activeSessionId })
    permissionSessionAllowed.value = state.sessionAllowed ?? false
  } catch { /* ignore */ }
}

const revokeSessionPermission = async () => {
  if (!session.activeSessionId) return
  try {
    await invoke('revoke_session_permission', { sessionId: session.activeSessionId })
    permissionSessionAllowed.value = false
  } catch { /* ignore */ }
}

watch(() => session.activeSessionId, () => {
  loadPermissionState()
}, { immediate: true })

// 每 2s 轮询权限状态（轻量，无需复杂事件系统）
permissionPollTimer = setInterval(loadPermissionState, 2000)
onUnmounted(() => {
  if (permissionPollTimer) clearInterval(permissionPollTimer)
})
const { persistCurrentWindowState } = useWindow();
const { t } = useI18n();
const props = defineProps<{ standalone?: boolean }>();

const elapsed = ref(0);
const backgroundTasks = ref<BackgroundTask[]>([]);
let timer: ReturnType<typeof setInterval> | null = null;
let backgroundTimer: ReturnType<typeof setInterval> | null = null;

const panelVisible = computed(() => props.standalone || agent.showAgentPanel);
const standalone = computed(() => props.standalone);
const currentContextSnapshot = computed(() => agent.currentContextSnapshot);
const currentSubAgents = computed(() => agent.currentSubAgentRuns.slice(0, 12));
const activeSubAgentCount = computed(() => agent.currentSubAgentRuns.filter((run) => run.status === 'running').length);
const recentBackgroundTasks = computed(() => backgroundTasks.value.slice(0, 4));
const runningBackgroundCount = computed(() => backgroundTasks.value.filter((task) => task.status === 'running').length);
const recentPlans = computed(() => permission.currentPlanDocuments.slice(0, 3));

const expandedSubAgents = ref<Set<string>>(new Set());

const formatTime = (seconds: number): string => {
  const minutes = Math.floor(seconds / 60);
  const rest = seconds % 60;
  return minutes > 0 ? `${minutes}:${rest.toString().padStart(2, '0')}` : `${rest}s`;
};

const formatDuration = (timestamp?: number | null): string => {
  if (!timestamp) return t('monitor.justNow');
  // 后端时间戳为秒级 Unix time，JS Date.now() 为毫秒
  const ts = timestamp < 1e12 ? timestamp * 1000 : timestamp;
  const seconds = Math.max(0, Math.floor((Date.now() - ts) / 1000));
  if (seconds < 60) return `${seconds}s`;
  const minutes = Math.floor(seconds / 60);
  if (minutes < 60) return `${minutes}m`;
  return `${Math.floor(minutes / 60)}h`;
};

const formatTokens = (tokens: number): string => {
  if (tokens >= 1_000_000) return `${(tokens / 1_000_000).toFixed(1)}m`;
  if (tokens >= 1000) return `${(tokens / 1000).toFixed(1)}k`;
  return String(tokens);
};

const toggleExpand = (runId: string) => {
  const next = new Set(expandedSubAgents.value);
  next.has(runId) ? next.delete(runId) : next.add(runId);
  expandedSubAgents.value = next;
};

const phaseClass = (phase: string): string => `phase-${phase}`;

const phaseLabel = (phase: string): string => {
  switch (phase) {
    case 'starting': return t('monitor.subAgentPhase.starting');
    case 'waiting_model': return t('monitor.subAgentPhase.waitingModel');
    case 'streaming': return t('monitor.subAgentPhase.streaming');
    case 'thinking': return t('monitor.subAgentPhase.thinking');
    case 'calling_tool': return t('monitor.subAgentPhase.callingTool');
    case 'processing_tool_result': return t('monitor.subAgentPhase.processingToolResult');
    case 'finalizing': return t('monitor.subAgentPhase.finalizing');
    default: return phase;
  }
};

const getToolTimeline = (runId: string): SubAgentEvent[] => {
  return agent.getSubAgentEvents(runId)
    .filter((ev) => ev.eventType === 'tool_call' || ev.eventType === 'tool_result');
};

const copiedTimeline = ref<string | null>(null);

const copyTimeline = async (runId: string) => {
  const events = getToolTimeline(runId);
  const text = events.map((ev) =>
    `L${ev.loopCount} ${ev.eventType === 'tool_call' ? '▸' : '✓'} ${ev.tool}: ${ev.inputSummary || ev.outputSummary || ev.message}`
  ).join('\n');
  try {
    await navigator.clipboard.writeText(text);
    copiedTimeline.value = runId;
    setTimeout(() => { copiedTimeline.value = null; }, 1500);
  } catch { /* ignore */ }
};

const toolEventIcon = (eventType: string): string => {
  return eventType === 'tool_call' ? '▸' : eventType === 'tool_result' ? '✓' : '•';
};

const toolEventClass = (eventType: string): string => `tl-${eventType}`;

const subAgentStatusLabel = (status: string): string => {
  switch (status) {
    case 'running': return t('monitor.subAgentStatus.running');
    case 'completed': return t('monitor.subAgentStatus.completed');
    case 'failed': return t('monitor.subAgentStatus.failed');
    case 'cancelled': return t('monitor.subAgentStatus.cancelled');
    default: return status;
  }
};

const planStatusLabel = (status: string): string => {
  switch (status) {
    case 'pending': return t('monitor.planStatus.pending');
    case 'approved': return t('monitor.planStatus.approved');
    case 'rejected': return t('monitor.planStatus.rejected');
    default: return status;
  }
};

const refreshBackgroundTasks = async () => {
  try {
    backgroundTasks.value = await invoke<BackgroundTask[]>('get_background_tasks');
  } catch (err) {
    console.error('加载后台任务失败:', err);
  }
};

watch(() => session.isCurrentSessionRunning, (running) => {
  if (running) {
    elapsed.value = 0;
    timer = setInterval(() => { elapsed.value++; }, 1000);
  } else if (timer) {
    clearInterval(timer);
    timer = null;
  }
}, { immediate: true });

// 监听后台任务完成事件（Tauri 推送，0 延迟，替代轮询的即时通路）
listen("bg-task-done", () => {
  refreshBackgroundTasks();
});

watch(panelVisible, (visible) => {
  if (visible) {
    refreshBackgroundTasks();
    // 保留 3s 轮询作为兜底（首次加载 + 异常恢复）
    if (!backgroundTimer) {
      backgroundTimer = setInterval(refreshBackgroundTasks, 3000);
    }
  } else if (backgroundTimer) {
    clearInterval(backgroundTimer);
    backgroundTimer = null;
  }
}, { immediate: true });

onUnmounted(() => {
  if (timer) clearInterval(timer);
  if (backgroundTimer) clearInterval(backgroundTimer);
});

const closePanel = async () => {
  agent.showAgentPanel = false;
  if (props.standalone) {
    await persistCurrentWindowState();
    await emit('monitor-window-closed');
    await getCurrentWindow().close();
  }
};

const itemStatusClass = (status: string): string => `status-${status}`;
const planStatusClass = (plan: PlanDocument): string => `status-${plan.status}`;
const backgroundTaskTitle = (task: BackgroundTask): string => task.task_type || task.taskType || task.id;
const backgroundStatusLabel = (status: string): string => {
  switch (status) {
    case 'running': return t('monitor.subAgentStatus.running');
    case 'completed': return t('monitor.subAgentStatus.completed');
    case 'failed':
    case 'error': return t('monitor.subAgentStatus.failed');
    default: return status;
  }
};
</script>

<template>
  <Transition name="panel-slide">
    <aside v-if="panelVisible" class="agent-panel" :class="{ standalone }">
      <div class="panel-header" data-tauri-drag-region>
        <div class="panel-title" data-tauri-drag-region>
          <span>{{ t('monitor.title') }}</span>
          <span v-if="session.isCurrentSessionRunning" class="running-dot"></span>
          <span v-if="session.isCurrentSessionRunning" class="elapsed-time">{{ formatTime(elapsed) }}</span>
        </div>
        <button class="close-btn" type="button" :aria-label="t('monitor.close')" @click="closePanel">
          <svg viewBox="0 0 24 24" width="12" height="12" stroke="currentColor" stroke-width="2" fill="none" stroke-linecap="round" stroke-linejoin="round">
            <line x1="18" y1="6" x2="6" y2="18"></line>
            <line x1="6" y1="6" x2="18" y2="18"></line>
          </svg>
        </button>
      </div>

      <Transition name="fade">
        <div v-if="permissionSessionAllowed" class="perm-session-bar">
          <svg viewBox="0 0 24 24" width="14" height="14" stroke="currentColor" stroke-width="2" fill="none" stroke-linecap="round"><path d="M10.29 3.86L1.82 18a2 2 0 0 0 1.71 3h16.94a2 2 0 0 0 1.71-3L13.71 3.86a2 2 0 0 0-3.42 0z"/><line x1="12" y1="9" x2="12" y2="13"/><line x1="12" y1="17" x2="12.01" y2="17"/></svg>
          <span>{{ t('permission.sessionAllowedHint') }}</span>
          <button class="perm-revoke-btn" @click="revokeSessionPermission">{{ t('permission.revoke') }}</button>
        </div>
      </Transition>

      <div class="panel-body">
        <section class="monitor-section context-section">
          <div class="monitor-section-head">
            <div>
              <span class="monitor-kicker">Context</span>
              <strong>{{ t('monitor.contextBudget') }}</strong>
            </div>
          </div>
          <ContextInspector :snapshot="currentContextSnapshot" />
        </section>

        <div class="monitor-grid">
          <section class="monitor-section subagents-section">
            <div class="monitor-section-head">
              <div>
                <span class="monitor-kicker">Sub Agents</span>
                <strong>{{ t('monitor.subAgents') }}</strong>
              </div>
              <span class="monitor-pill">{{ activeSubAgentCount }}/{{ agent.currentSubAgentRuns.length }}</span>
            </div>
            <div v-if="currentSubAgents.length" class="subagent-list">
              <div
                v-for="run in currentSubAgents"
                :key="run.runId"
                class="subagent-card"
                :class="[itemStatusClass(run.status), { expanded: expandedSubAgents.has(run.runId) }]"
              >
                <!-- 卡片头部（始终可见） -->
                <div class="subagent-header" @click="toggleExpand(run.runId)">
                  <span class="status-dot"></span>
                  <strong>{{ run.label || run.runId }}</strong>
                  <span class="agent-type-badge" :title="`Agent: ${run.agentType}`">{{ run.agentType }}</span>
                  <span v-if="run.readOnly" class="readonly-badge" :title="t('monitor.readOnly')">R</span>
                  <span class="phase-badge" :class="phaseClass(run.phase)">{{ phaseLabel(run.phase) }}</span>
                  <span class="status-label">{{ subAgentStatusLabel(run.status) }}</span>
                  <span class="loop-badge">{{ run.loopCount }}/{{ run.maxLoops }}</span>
                  <span class="expand-arrow">{{ expandedSubAgents.has(run.runId) ? '▾' : '▸' }}</span>
                </div>

                <!-- 详情区域（点击展开） -->
                <div v-if="expandedSubAgents.has(run.runId)" class="subagent-detail">
                  <!-- 当前工具 -->
                  <div v-if="run.currentTool" class="current-tool-bar">
                    <span class="ct-label">{{ t('monitor.currentTool') }}:</span>
                    <span class="ct-name">{{ run.currentTool }}</span>
                    <span v-if="run.currentToolInput" class="ct-input">{{ run.currentToolInput }}</span>
                  </div>

                  <!-- 工具调用时间线 -->
                  <div v-if="getToolTimeline(run.runId).length" class="tool-timeline">
                    <div class="timeline-header">
                      <span>{{ t('monitor.toolTimeline') }}</span>
                      <button
                        class="tl-copy-btn"
                        :class="{ copied: copiedTimeline === run.runId }"
                        @click.stop="copyTimeline(run.runId)"
                      >
                        {{ copiedTimeline === run.runId ? '已复制' : '复制全部' }}
                      </button>
                    </div>
                    <div
                      v-for="event in getToolTimeline(run.runId)"
                      :key="event.eventId"
                      class="timeline-item"
                      :class="toolEventClass(event.eventType)"
                    >
                      <span class="tl-loop">L{{ event.loopCount }}</span>
                      <span class="tl-icon">{{ toolEventIcon(event.eventType) }}</span>
                      <span class="tl-tool">{{ event.tool }}</span>
                      <span class="tl-summary">{{ event.inputSummary || event.outputSummary || event.message }}</span>
                    </div>
                  </div>
                  <div v-else class="tool-timeline-empty">{{ t('monitor.noToolEvents') }}</div>

                  <!-- Token 明细 -->
                  <div class="detail-tokens">
                    <span>输入 {{ formatTokens(run.inputTokens) }}</span>
                    <span class="sep">·</span>
                    <span>输出 {{ formatTokens(run.outputTokens) }}</span>
                    <span class="sep">·</span>
                    <span>总计 {{ formatTokens(run.inputTokens + run.outputTokens) }}</span>
                  </div>

                  <!-- 时间信息 -->
                  <div class="detail-time">
                    <span>{{ t('monitor.ago', { duration: formatDuration(run.startedAt) }) }}前启动</span>
                    <span class="sep">·</span>
                    <span>{{ t('monitor.updatedAgo', { duration: formatDuration(run.updatedAt) }) }}</span>
                    <template v-if="run.finishedAt">
                      <span class="sep">·</span>
                      <span>耗时 {{ formatDuration(run.startedAt ? Math.max(0, run.finishedAt - run.startedAt) : null) }}</span>
                    </template>
                  </div>

                  <!-- 错误详情 -->
                  <div v-if="run.error" class="detail-error">
                    <div class="error-label">{{ t('monitor.errorDetail') }}</div>
                    <pre>{{ run.error }}</pre>
                  </div>

                  <!-- 任务描述预览 -->
                  <div class="detail-prompt">
                    <div class="prompt-label">{{ t('monitor.taskPrompt') }}</div>
                    <p>{{ run.summary || run.prompt || run.promptPreview || t('monitor.emptySummary') }}</p>
                  </div>
                </div>
              </div>
            </div>
            <div v-else class="monitor-empty">{{ t('monitor.noSubAgents') }}</div>
          </section>

          <section class="monitor-section">
            <div class="monitor-section-head">
              <div>
                <span class="monitor-kicker">Background</span>
                <strong>{{ t('monitor.backgroundTasks') }}</strong>
              </div>
              <span class="monitor-pill">{{ runningBackgroundCount }}/{{ backgroundTasks.length }}</span>
            </div>
            <div v-if="recentBackgroundTasks.length" class="monitor-list">
              <div
                v-for="task in recentBackgroundTasks"
                :key="task.id"
                class="monitor-item"
                :class="itemStatusClass(task.status)"
              >
                <div class="monitor-item-main">
                  <span class="status-dot"></span>
                  <strong>{{ backgroundTaskTitle(task) }}</strong>
                  <span>{{ backgroundStatusLabel(task.status) }}</span>
                </div>
                <p>{{ task.command }}</p>
                <div class="monitor-item-meta">
                  <span v-if="task.port">端口 {{ task.port }}</span>
                  <span v-if="task.result">{{ task.result }}</span>
                </div>
              </div>
            </div>
            <div v-else class="monitor-empty">{{ t('monitor.noBackgroundTasks') }}</div>
          </section>

          <section class="monitor-section plans-section">
            <div class="monitor-section-head">
              <div>
                <span class="monitor-kicker">Plans</span>
                <strong>{{ t('monitor.plans') }}</strong>
              </div>
              <span class="monitor-pill">{{ permission.currentPlanDocuments.length }}</span>
            </div>
            <div v-if="recentPlans.length" class="monitor-list">
              <div
                v-for="plan in recentPlans"
                :key="plan.id"
                class="monitor-item"
                :class="planStatusClass(plan)"
              >
                <div class="monitor-item-main">
                  <span class="status-dot"></span>
                  <strong>{{ plan.title }}</strong>
                  <span>{{ planStatusLabel(plan.status) }}</span>
                </div>
                <p>{{ plan.content }}</p>
                <div class="monitor-item-meta">
                  <span>{{ t('monitor.updatedAgo', { duration: formatDuration(plan.updatedAt) }) }}</span>
                </div>
              </div>
            </div>
            <div v-else class="monitor-empty">{{ t('monitor.noPlans') }}</div>
          </section>
        </div>
      </div>
    </aside>
  </Transition>
</template>

<style scoped>
.agent-panel {
  position: absolute;
  top: 46px;
  right: 12px;
  width: min(620px, calc(100% - 24px));
  max-height: min(720px, calc(100% - 58px));
  background-color: var(--bg-sidebar);
  border: 1px solid var(--border-color);
  border-radius: 18px;
  display: flex;
  flex-direction: column;
  overflow: hidden;
  z-index: 25;
  box-shadow: 0 24px 64px rgba(0, 0, 0, 0.24);
  will-change: transform, opacity;
}

.agent-panel.standalone {
  position: static;
  width: 100%;
  height: 100%;
  max-height: none;
  border: 0;
  border-radius: 0;
  box-shadow: none;
}

.panel-header {
  height: 44px;
  padding: 0 14px;
  border-bottom: 1px solid var(--border-color);
  display: flex;
  align-items: center;
  justify-content: space-between;
  flex-shrink: 0;
}

.panel-title {
  min-width: 0;
  display: inline-flex;
  align-items: center;
  gap: 8px;
  color: var(--text-main);
  font-size: 0.82rem;
  font-weight: 850;
}

.running-dot {
  width: 7px;
  height: 7px;
  border-radius: 999px;
  background: var(--accent-green);
  box-shadow: 0 0 10px var(--accent-green);
}

.elapsed-time {
  color: var(--text-muted);
  font-size: 0.7rem;
  font-weight: 700;
  font-variant-numeric: tabular-nums;
}

.close-btn {
  width: 28px;
  height: 28px;
  border: 0;
  border-radius: 8px;
  display: inline-flex;
  align-items: center;
  justify-content: center;
  color: var(--text-muted);
  background: transparent;
  cursor: pointer;
  -webkit-app-region: no-drag;
}

.close-btn:hover {
  color: var(--text-main);
  background: var(--glass-bg-light);
}

/* 权限状态条 */
.perm-session-bar {
  display: flex;
  align-items: center;
  gap: 8px;
  padding: 8px 14px;
  margin: 0 12px;
  border-radius: 8px;
  background: rgba(245, 158, 11, 0.08);
  border: 1px solid rgba(245, 158, 11, 0.2);
  color: var(--accent-yellow);
  font-size: 0.78rem;
}
.perm-session-bar svg { flex-shrink: 0; }
.perm-revoke-btn {
  margin-left: auto;
  padding: 3px 10px;
  border-radius: 6px;
  border: 1px solid rgba(245, 158, 11, 0.3);
  background: transparent;
  color: var(--accent-yellow);
  font-size: 0.72rem;
  font-weight: 600;
  cursor: pointer;
  transition: all 0.15s;
}
.perm-revoke-btn:hover {
  background: rgba(245, 158, 11, 0.15);
  border-color: var(--accent-yellow);
}

.panel-body {
  flex: 1;
  min-height: 0;
  padding: 16px;
  overflow: auto;
}

.monitor-section {
  min-width: 0;
  border: 1px solid var(--border-color);
  border-radius: 16px;
  background: color-mix(in srgb, var(--glass-bg) 82%, transparent);
  box-shadow: 0 10px 28px rgba(0, 0, 0, 0.1);
  padding: 14px;
}

.context-section {
  margin-bottom: 12px;
}

.monitor-grid {
  display: grid;
  grid-template-columns: repeat(2, minmax(0, 1fr));
  gap: 12px;
}

.plans-section {
  grid-column: 1 / -1;
}

.monitor-section-head {
  display: flex;
  align-items: flex-start;
  justify-content: space-between;
  gap: 10px;
  margin-bottom: 10px;
}

.monitor-section-head > div {
  min-width: 0;
  display: flex;
  flex-direction: column;
  gap: 2px;
}

.monitor-kicker {
  color: var(--text-muted);
  font-size: 0.58rem;
  font-weight: 850;
  letter-spacing: 0.08em;
  text-transform: uppercase;
}

.monitor-section-head strong {
  color: var(--text-main);
  font-size: 0.78rem;
  font-weight: 850;
}

.monitor-pill {
  padding: 3px 7px;
  border-radius: 999px;
  color: var(--text-muted);
  background: var(--glass-bg-light);
  font-size: 0.62rem;
  font-weight: 800;
  font-variant-numeric: tabular-nums;
}

.monitor-list {
  display: flex;
  flex-direction: column;
  gap: 8px;
}

.monitor-item {
  min-width: 0;
  padding: 9px;
  border: 1px solid color-mix(in srgb, var(--border-color) 80%, transparent);
  border-radius: 10px;
  background: color-mix(in srgb, var(--surface-strong) 35%, transparent);
}

.monitor-item-main {
  min-width: 0;
  display: flex;
  align-items: center;
  gap: 7px;
}

.monitor-item-main strong {
  min-width: 0;
  flex: 1;
  overflow: hidden;
  color: var(--text-main);
  font-size: 0.72rem;
  font-weight: 800;
  text-overflow: ellipsis;
  white-space: nowrap;
}

.monitor-item-main span:last-child {
  flex-shrink: 0;
  color: var(--text-muted);
  font-size: 0.6rem;
  font-weight: 750;
}

.monitor-item p {
  margin: 6px 0 0;
  max-height: 160px;
  overflow: auto;
  color: var(--text-muted);
  font-size: 0.66rem;
  line-height: 1.45;
  white-space: pre-wrap;
  word-break: break-word;
}

.monitor-item-meta {
  display: flex;
  flex-wrap: wrap;
  gap: 6px;
  margin-top: 7px;
  color: var(--text-muted);
  font-size: 0.6rem;
  font-variant-numeric: tabular-nums;
}

.monitor-item-meta span {
  max-height: 140px;
  overflow: auto;
  word-break: break-all;
  white-space: pre-wrap;
}

.monitor-section.subagents-section {
  grid-column: 1 / -1;
}

.subagent-list {
  display: flex;
  flex-direction: column;
  gap: 8px;
  max-height: 480px;
  overflow-y: auto;
}

.subagent-card {
  min-width: 0;
  border: 1px solid color-mix(in srgb, var(--border-color) 80%, transparent);
  border-radius: 10px;
  background: color-mix(in srgb, var(--surface-strong) 35%, transparent);
  transition: border-color 120ms ease;
}

.subagent-card.expanded {
  border-color: var(--border-color);
}

.subagent-header {
  min-width: 0;
  display: flex;
  align-items: center;
  gap: 6px;
  padding: 8px 10px;
  cursor: pointer;
  user-select: none;
}

.subagent-header:hover {
  background: color-mix(in srgb, var(--glass-bg-light) 60%, transparent);
}

.subagent-header strong {
  min-width: 0;
  flex: 1;
  overflow: hidden;
  color: var(--text-main);
  font-size: 0.7rem;
  font-weight: 800;
  text-overflow: ellipsis;
  white-space: nowrap;
}

.agent-type-badge {
  flex-shrink: 0;
  padding: 1px 5px;
  border-radius: 4px;
  background: var(--glass-bg-light);
  color: var(--text-muted);
  font-size: 0.55rem;
  font-weight: 750;
  text-transform: uppercase;
}

.readonly-badge {
  flex-shrink: 0;
  width: 14px;
  height: 14px;
  display: inline-flex;
  align-items: center;
  justify-content: center;
  border-radius: 3px;
  background: color-mix(in srgb, var(--accent-yellow) 30%, transparent);
  color: var(--accent-yellow);
  font-size: 0.5rem;
  font-weight: 900;
}

.phase-badge {
  flex-shrink: 0;
  padding: 1px 5px;
  border-radius: 4px;
  font-size: 0.55rem;
  font-weight: 750;
}

.phase-starting { background: color-mix(in srgb, var(--text-muted) 20%, transparent); color: var(--text-muted); }
.phase-waiting_model { background: color-mix(in srgb, var(--accent-blue) 20%, transparent); color: var(--accent-blue); }
.phase-streaming { background: color-mix(in srgb, var(--accent-green) 20%, transparent); color: var(--accent-green); }
.phase-thinking { background: color-mix(in srgb, var(--accent-purple) 20%, transparent); color: var(--accent-purple); }
.phase-calling_tool { background: color-mix(in srgb, var(--accent-yellow) 20%, transparent); color: var(--accent-yellow); }
.phase-processing_tool_result { background: color-mix(in srgb, var(--accent-orange) 20%, transparent); color: var(--accent-orange); }
.phase-finalizing { background: color-mix(in srgb, var(--accent-blue) 20%, transparent); color: var(--accent-blue); }

.status-label {
  flex-shrink: 0;
  color: var(--text-muted);
  font-size: 0.6rem;
  font-weight: 700;
}

.loop-badge {
  flex-shrink: 0;
  color: var(--text-muted);
  font-size: 0.6rem;
  font-weight: 700;
  font-variant-numeric: tabular-nums;
}

.expand-arrow {
  flex-shrink: 0;
  width: 16px;
  text-align: center;
  color: var(--text-muted);
  font-size: 0.55rem;
  transition: transform 120ms ease;
}

/* 详情区域 */
.subagent-detail {
  padding: 0 10px 10px;
  border-top: 1px solid color-mix(in srgb, var(--border-color) 50%, transparent);
}

.current-tool-bar {
  display: flex;
  align-items: center;
  gap: 6px;
  padding: 8px 0;
  font-size: 0.64rem;
  border-bottom: 1px solid color-mix(in srgb, var(--border-color) 30%, transparent);
}

.ct-label {
  color: var(--text-muted);
  font-weight: 700;
  flex-shrink: 0;
}

.ct-name {
  color: var(--accent-yellow);
  font-weight: 800;
  font-family: ui-monospace, 'Cascadia Code', monospace;
}

.ct-input {
  color: var(--text-muted);
  font-size: 0.6rem;
  overflow: hidden;
  text-overflow: ellipsis;
  white-space: nowrap;
}

/* 工具时间线 */
.tool-timeline {
  max-height: 200px;
  overflow-y: auto;
  margin-top: 8px;
  padding: 4px 0;
}

.tool-timeline-empty {
  padding: 8px 0;
  color: var(--text-muted);
  font-size: 0.6rem;
}

.timeline-header {
  display: flex;
  align-items: center;
  justify-content: space-between;
  color: var(--text-muted);
  font-size: 0.58rem;
  font-weight: 800;
  letter-spacing: 0.06em;
  text-transform: uppercase;
  margin-bottom: 6px;
}

.tl-copy-btn {
  padding: 1px 5px;
  border: 1px solid var(--border-color);
  border-radius: 3px;
  background: var(--glass-bg);
  color: var(--text-muted);
  font-size: 0.5rem;
  font-weight: 700;
  text-transform: none;
  letter-spacing: 0;
  cursor: pointer;
}

.tl-copy-btn:hover {
  color: var(--text-main);
  border-color: var(--text-muted);
}

.tl-copy-btn.copied {
  color: var(--accent-green);
  border-color: var(--accent-green);
}

.timeline-item {
  display: flex;
  align-items: center;
  gap: 5px;
  padding: 2px 0;
  font-size: 0.6rem;
  font-family: ui-monospace, 'Cascadia Code', monospace;
}

.tl-loop {
  flex-shrink: 0;
  width: 26px;
  color: var(--text-muted);
  font-size: 0.55rem;
}

.tl-icon {
  flex-shrink: 0;
  width: 12px;
  text-align: center;
  font-size: 0.5rem;
}

.tl-tool_call .tl-icon { color: var(--accent-yellow); }
.tl-tool_result .tl-icon { color: var(--accent-green); }

.tl-tool {
  flex-shrink: 0;
  color: var(--text-main);
  font-weight: 700;
}

.tl-summary {
  flex: 1;
  min-width: 0;
  overflow: hidden;
  text-overflow: ellipsis;
  white-space: nowrap;
  color: var(--text-muted);
  font-size: 0.55rem;
}

/* Token 明细 */
.detail-tokens {
  display: flex;
  align-items: center;
  gap: 4px;
  padding: 7px 0;
  color: var(--text-muted);
  font-size: 0.6rem;
  font-variant-numeric: tabular-nums;
  border-top: 1px solid color-mix(in srgb, var(--border-color) 30%, transparent);
}

.sep {
  color: var(--border-color);
}

/* 时间信息 */
.detail-time {
  display: flex;
  align-items: center;
  gap: 4px;
  padding-bottom: 6px;
  color: var(--text-muted);
  font-size: 0.58rem;
}

/* 错误详情 */
.detail-error {
  padding: 8px;
  border-radius: 6px;
  background: color-mix(in srgb, var(--accent-red) 8%, transparent);
  border: 1px solid color-mix(in srgb, var(--accent-red) 25%, transparent);
}

.error-label {
  color: var(--accent-red);
  font-size: 0.6rem;
  font-weight: 800;
  margin-bottom: 4px;
}

.detail-error pre {
  margin: 0;
  color: var(--accent-red);
  font-size: 0.58rem;
  font-family: ui-monospace, 'Cascadia Code', monospace;
  white-space: pre-wrap;
  word-break: break-all;
}

/* 任务描述 */
.detail-prompt {
  padding-top: 6px;
  border-top: 1px solid color-mix(in srgb, var(--border-color) 30%, transparent);
}

.prompt-label {
  color: var(--text-muted);
  font-size: 0.58rem;
  font-weight: 800;
  margin-bottom: 2px;
}

.detail-prompt p {
  margin: 0;
  color: var(--text-muted);
  font-size: 0.62rem;
  line-height: 1.45;
}

.status-dot {
  width: 7px;
  height: 7px;
  flex-shrink: 0;
  border-radius: 999px;
  background: var(--text-muted);
}

.status-running .status-dot,
.status-pending .status-dot {
  background: var(--accent-yellow);
  box-shadow: 0 0 10px var(--accent-yellow);
}

.status-completed .status-dot,
.status-approved .status-dot {
  background: var(--accent-green);
}

.status-failed .status-dot,
.status-rejected .status-dot,
.status-cancelled .status-dot {
  background: var(--accent-red);
}

.monitor-empty {
  padding: 12px 2px 4px;
  color: var(--text-muted);
  font-size: 0.66rem;
}

.panel-slide-enter-active,
.panel-slide-leave-active {
  transition: opacity 180ms ease, transform 180ms ease;
}

.panel-slide-enter-from,
.panel-slide-leave-to {
  opacity: 0;
  transform: translate(8px, -8px) scale(0.98);
}

@media (max-width: 1180px) {
  .monitor-grid {
    grid-template-columns: 1fr;
  }
}

@media (max-width: 920px) {
  .agent-panel {
    right: 8px;
    width: calc(100% - 16px);
  }
}
</style>
