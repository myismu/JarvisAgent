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
import { emit } from '@tauri-apps/api/event';
import { useWindow } from '../../composables/useWindow';
import { useSessionStore } from '../../stores/session';
import { useAgentStore } from '../../stores/agent';
import { usePermissionStore } from '../../stores/permission';
import type { BackgroundTask, PlanDocument, SubAgentRun } from '../../types';
import ContextInspector from './ContextInspector.vue';

const session = useSessionStore();
const agent = useAgentStore();
const permission = usePermissionStore();
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
const currentSubAgents = computed(() => agent.currentSubAgentRuns.slice(0, 4));
const activeSubAgentCount = computed(() => agent.currentSubAgentRuns.filter((run) => run.status === 'running').length);
const recentBackgroundTasks = computed(() => backgroundTasks.value.slice(0, 4));
const runningBackgroundCount = computed(() => backgroundTasks.value.filter((task) => task.status === 'running').length);
const recentPlans = computed(() => permission.currentPlanDocuments.slice(0, 3));

const formatTime = (seconds: number): string => {
  const minutes = Math.floor(seconds / 60);
  const rest = seconds % 60;
  return minutes > 0 ? `${minutes}:${rest.toString().padStart(2, '0')}` : `${rest}s`;
};

const formatDuration = (timestamp?: number | null): string => {
  if (!timestamp) return t('monitor.justNow');
  const seconds = Math.max(0, Math.floor((Date.now() - timestamp) / 1000));
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

const previewText = (value?: string | null, max = 72): string => {
  const text = (value || '').replace(/\s+/g, ' ').trim();
  if (!text) return t('monitor.emptySummary');
  return text.length > max ? `${text.slice(0, max)}...` : text;
};

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

watch(panelVisible, (visible) => {
  if (visible) {
    refreshBackgroundTasks();
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
const subAgentTokenTotal = (run: SubAgentRun): number => run.inputTokens + run.outputTokens;
const backgroundTaskTitle = (task: BackgroundTask): string => task.taskType || task.task_type || task.id;
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
          <section class="monitor-section">
            <div class="monitor-section-head">
              <div>
                <span class="monitor-kicker">Sub Agents</span>
                <strong>{{ t('monitor.subAgents') }}</strong>
              </div>
              <span class="monitor-pill">{{ activeSubAgentCount }}/{{ agent.currentSubAgentRuns.length }}</span>
            </div>
            <div v-if="currentSubAgents.length" class="monitor-list">
              <div
                v-for="run in currentSubAgents"
                :key="run.runId"
                class="monitor-item"
                :class="itemStatusClass(run.status)"
              >
                <div class="monitor-item-main">
                  <span class="status-dot"></span>
                  <strong>{{ run.label || run.runId }}</strong>
                  <span>{{ subAgentStatusLabel(run.status) }}</span>
                </div>
                <p>{{ previewText(run.summary || run.error || run.promptPreview || run.prompt) }}</p>
                <div class="monitor-item-meta">
                  <span>{{ t('monitor.loops', { current: run.loopCount, max: run.maxLoops }) }}</span>
                  <span>{{ formatTokens(subAgentTokenTotal(run)) }} tok</span>
                  <span>{{ t('monitor.ago', { duration: formatDuration(run.updatedAt) }) }}</span>
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
                  <span>{{ task.status }}</span>
                </div>
                <p>{{ previewText(task.command) }}</p>
                <div class="monitor-item-meta">
                  <span v-if="task.port">:{{ task.port }}</span>
                  <span v-if="task.result">{{ previewText(task.result, 42) }}</span>
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
                <p>{{ previewText(plan.content) }}</p>
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
  color: var(--text-muted);
  font-size: 0.66rem;
  line-height: 1.45;
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
