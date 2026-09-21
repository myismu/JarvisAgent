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
import type { BackgroundTask, PlanDocument, SessionContextSnapshot, SubAgentEvent } from '../../types';
import ContextInspector from './ContextInspector.vue';

const session = useSessionStore();
const agent = useAgentStore();
const permission = usePermissionStore();

// ── 权限状态 ──
const permissionAllowanceCount = ref(0)
const permissionAllowances = ref<Array<{ kind: string; scope: string; label: string }>>([])
const permissionPendingCount = ref(0)
const pendingPermissions = ref<Array<{ id: string; message: string; allowSession?: boolean; kind?: string; warning?: string | null }>>([])

/**
 * 已允许清单是否展开。
 *
 * 默认折叠：这块在 `.panel-body` 之外（不随内容滚动），原来固定占 200px 内滚，
 * 在独立监控窗口里长期吃掉约四分之一高度。清单是低频查看项，收起后只留一行计数。
 */
const showAllowances = ref(false)
let permissionPollTimer: ReturnType<typeof setInterval> | null = null

const loadPermissionState = async () => {
  // 切会话时先清空：权限允许是"每个会话各自的内存态"，
  // 不能让上一个会话的清单残留在这里（哪怕这次请求失败也不显示旧数据）
  if (!session.activeSessionId) {
    permissionAllowanceCount.value = 0
    permissionAllowances.value = []
    permissionPendingCount.value = 0
    pendingPermissions.value = []
    return
  }
  try {
    const state = await invoke<any>('get_permission_state', { sessionId: session.activeSessionId })
    permissionAllowanceCount.value = state.allowanceCount ?? 0
    permissionAllowances.value = (state.allowances ?? []) as Array<{ kind: string; scope: string; label: string }>
    permissionPendingCount.value = state.pendingCount ?? 0
    pendingPermissions.value = (state.pending ?? []) as Array<{
      id: string;
      message: string;
      allowSession?: boolean;
      kind?: string;
      warning?: string | null;
    }>
  } catch { /* ignore */ }
}

const resolvePermission = async (id: string, decision: string) => {
  if (!session.activeSessionId) return
  try {
    await invoke('resolve_permission', { id, sessionId: session.activeSessionId, decision, content: null })
    // 后端已决议，本地队列同步出队，避免卡片残留
    permission.removePermission(session.activeSessionId, id)
    await loadPermissionState()
  } catch { /* ignore */ }
}

/** 清空本会话的全部"已允许"（旧的"本会话始终允许一切"总开关已移除，改为按工具+范围记忆） */
const clearSessionAllowances = async () => {
  if (!session.activeSessionId) return
  try {
    await invoke('clear_session_allowances', { sessionId: session.activeSessionId })
    permissionAllowanceCount.value = 0
    permissionAllowances.value = []
  } catch { /* ignore */ }
}

/** 撤销单条"已允许" */
const revokeAllowance = async (allowance: { kind: string; scope: string }) => {
  if (!session.activeSessionId) return
  try {
    await invoke('revoke_session_allowance', {
      sessionId: session.activeSessionId,
      kind: allowance.kind,
      scope: allowance.scope,
    })
    await loadPermissionState()
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
const dismissingTasks = ref<Set<string>>(new Set());
let timer: ReturnType<typeof setInterval> | null = null;
let backgroundTimer: ReturnType<typeof setInterval> | null = null;

const panelVisible = computed(() => props.standalone || agent.showAgentPanel);
const standalone = computed(() => props.standalone);
const currentContextSnapshot = computed(() => agent.currentContextSnapshot);

const refreshContextSnapshot = async () => {
  if (!session.activeSessionId) return;
  try {
    const snapshot = await invoke<SessionContextSnapshot | null>('get_session_context_snapshot', {
      sessionId: session.activeSessionId,
    });
    agent.upsertContextSnapshot(snapshot);
  } catch { /* ignore */ }
};
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

const formatCountdown = (run: { status: string; startedAt: number; timeoutSecs: number }): string => {
  if (run.status !== 'running') return '';
  if (!run.startedAt || !run.timeoutSecs) return '';
  void elapsed.value; // 每 1s 刷新，利用现有 timer
  const deadline = run.startedAt + run.timeoutSecs * 1000;
  const remaining = Math.max(0, Math.floor((deadline - Date.now()) / 1000));
  if (!isFinite(remaining)) return '';
  if (remaining > 3600) return `${Math.floor(remaining / 3600)}h`;
  return `${Math.floor(remaining / 60)}:${(remaining % 60).toString().padStart(2, '0')}`;
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

/**
 * 收起行上的「工具调用次数」。
 *
 * 只数 `tool_call`：一次调用会产生 `tool_call` + `tool_result` 两条事件，
 * 直接取时间线长度会把次数翻倍。数据来自内存中的子代理事件（与展开后的时间线同源）。
 */
const toolCallCount = (runId: string): number =>
  agent.getSubAgentEvents(runId).filter((ev) => ev.eventType === 'tool_call').length;

const copiedTimeline = ref<string | null>(null);

const copyTimeline = async (runId: string) => {
  const events = getToolTimeline(runId);
  const text = events.map((ev) =>
    `L${ev.loopCount} ${ev.eventType === 'tool_call' ? '▸' : '✓'} ${ev.tool}: ${ev.input || ev.output || ev.message}`
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

const agentRoleLabel = (agentRole: string): string => {
  const key = `monitor.subAgentType.${agentRole}`;
  const translated = t(key);
  return translated === key ? agentRole : translated;
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
    const sid = session.activeSessionId;
    backgroundTasks.value = await invoke<BackgroundTask[]>('get_background_tasks', { sessionId: sid });
  } catch (err) {
    console.error('加载后台任务失败:', err);
  }
};

const dismissTask = async (taskId: string) => {
  dismissingTasks.value.add(taskId);
  try {
    await invoke('dismiss_background_task', { taskId });
    backgroundTasks.value = backgroundTasks.value.filter((t) => t.id !== taskId);
  } catch (err) {
    console.error('清理后台任务失败:', err);
  } finally {
    dismissingTasks.value.delete(taskId);
  }
};

const killTask = async (taskId: string) => {
  dismissingTasks.value.add(taskId);
  try {
    await invoke('kill_background_task', { taskId });
    await refreshBackgroundTasks();
  } catch (err) {
    console.error('终止后台任务失败:', err);
  } finally {
    dismissingTasks.value.delete(taskId);
  }
};

const clearAllDoneTasks = async () => {
  const sid = session.activeSessionId;
  if (!sid) return;
  try {
    await invoke('clear_session_background_tasks', { sessionId: sid });
    await refreshBackgroundTasks();
  } catch (err) {
    console.error('清理后台任务失败:', err);
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
const backgroundTaskTitle = (task: BackgroundTask): string => {
  const cmd = task.command || "";
  if (cmd.length <= 50) return cmd;
  return cmd.substring(0, 50) + "...";
};
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

      <div class="perm-status-bar" :class="{ allowed: permissionPendingCount === 0, pending: permissionPendingCount > 0 }">
        <svg viewBox="0 0 24 24" width="14" height="14" stroke="currentColor" stroke-width="2" fill="none" stroke-linecap="round">
          <path d="M12 22s8-4 8-10V5l-8-3-8 3v7c0 6 8 10 8 10z"/>
        </svg>
        <span v-if="permissionPendingCount > 0">{{ t('permission.pendingRequests', { count: permissionPendingCount }) }}</span>
        <span v-else-if="permissionAllowanceCount > 0">{{ t('permission.allowancesHint', { count: permissionAllowanceCount }) }}</span>
        <span v-else>{{ t('permission.normalMode') }}</span>
        <!-- 折叠开关：不加文字文案（箭头 + 数字 title 够用），避免为一条 UI 文案去动 locale -->
        <button
          v-if="permissionAllowances.length > 0"
          class="perm-allow-toggle"
          :aria-expanded="showAllowances"
          :title="`${permissionAllowances.length} 项`"
          @click="showAllowances = !showAllowances"
        >
          <span class="perm-chev">{{ showAllowances ? '▾' : '▸' }}</span>
        </button>
        <button v-if="permissionAllowanceCount > 0" class="perm-revoke-btn" @click="clearSessionAllowances">{{ t('permission.revokeAll') }}</button>
      </div>

      <!-- 本会话已允许的范围：紧凑行式，一行一条，悬停行显撤销。
           清单可能很长（覆盖/命令各一条 easily 20+），max-height 内部滚动，
           不再把下方的 CONTEXT 等监控区挤走 -->
      <div v-if="permissionAllowances.length > 0" v-show="showAllowances" class="perm-allow-list">
        <div v-for="allowance in permissionAllowances" :key="allowance.kind + '|' + allowance.scope" class="perm-allow-row">
          <span class="perm-allow-label" :title="allowance.label">{{ allowance.label }}</span>
          <button class="perm-allow-revoke" @click="revokeAllowance(allowance)">{{ t('permission.revoke') }}</button>
        </div>
      </div>

      <!-- 权限请求卡片列表 -->
      <div v-if="permissionPendingCount > 0 && pendingPermissions.length > 0" class="perm-cards">
        <div v-for="req in pendingPermissions" :key="req.id" class="perm-card-inline" :class="{ 'perm-card-warning': !!req.warning }">
          <p class="perm-card-msg">{{ req.message }}</p>
          <div class="perm-card-actions">
            <button class="perm-card-btn reject" @click="resolvePermission(req.id, 'reject')">{{ t('permission.reject') }}</button>
            <button class="perm-card-btn allow" @click="resolvePermission(req.id, 'allow')">{{ t('permission.allowOnce') }}</button>
            <button v-if="req.allowSession !== false" class="perm-card-btn session" @click="resolvePermission(req.id, 'allow_session')">{{ t('permission.allowSession') }}</button>
          </div>
        </div>
      </div>

      <div class="panel-body">
        <section class="monitor-section context-section">
          <div class="monitor-section-head">
            <div>
              <span class="monitor-kicker">Context</span>
              <strong>{{ t('monitor.contextBudget') }}</strong>
            </div>
          </div>
          <ContextInspector :snapshot="currentContextSnapshot" :session-id="session.activeSessionId" @compacted="refreshContextSnapshot" />
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
                  <span class="agent-type-badge" :title="`Agent: ${run.agentRole}`">{{ agentRoleLabel(run.agentRole) }}</span>
                  <span v-if="run.readOnly" class="readonly-badge" :title="t('monitor.readOnly')">R</span>
                  <span v-if="run.status === 'running'" class="phase-badge" :class="phaseClass(run.phase)">{{ phaseLabel(run.phase) }}</span>
                  <span class="status-label">{{ subAgentStatusLabel(run.status) }}</span>
                  <span class="loop-badge">{{ run.loopCount }}/{{ run.maxLoops }}</span>
                  <span
                    v-if="toolCallCount(run.runId)"
                    class="tool-count-badge"
                    :title="t('monitor.toolTimeline')"
                  >{{ t('monitor.toolCalls', { count: toolCallCount(run.runId) }) }}</span>
                  <span v-if="run.status === 'running'" class="countdown-badge">&#9201; {{ formatCountdown(run) }}</span>
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
                      <span class="tl-summary">{{ event.input || event.output || event.message }}</span>
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
                  <button
                    v-if="task.status === 'running'"
                    class="kill-task-btn"
                    :disabled="dismissingTasks.has(task.id)"
                    @click="killTask(task.id)"
                    :title="t('monitor.killTask')"
                  >&#9209;</button>
                  <button
                    class="dismiss-task-btn"
                    :disabled="dismissingTasks.has(task.id)"
                    @click="dismissTask(task.id)"
                    :title="t('monitor.dismissTask')"
                  >&times;</button>
                </div>
                <div class="monitor-item-meta">
                  <span v-if="task.task_type || task.taskType">{{ task.task_type || task.taskType }}</span>
                  <span v-if="task.port">:{{ task.port }}</span>
                  <span v-if="task.result">{{ task.result }}</span>
                </div>
              </div>
              <button
                class="clear-all-tasks-btn"
                @click="clearAllDoneTasks"
              >{{ t('monitor.clearDoneTasks') }}</button>
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
/* 为什么这里没有断点：面板宽度和**视口宽度无关** —— 内嵌时视口 1600 而面板只有 620，
   独立窗口时视口 640 而面板占满 640，用 @media 两种场景都不生效。
   改由内层网格的 auto-fit 自己决定落几列（见 .monitor-grid），
   比 @container 更稳：容器查询是较新的 at-rule，要依赖 scoped 插件对它的处理。 */
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
  box-shadow: var(--shadow-lg);
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
  background: var(--text-muted);
  animation: runningDotPulse 1.5s ease-in-out infinite;
}

@keyframes runningDotPulse {
  0%, 100% { opacity: 0.45; }
  50% { opacity: 1; }
}

.elapsed-time {
  color: var(--text-muted);
  font-size: 0.78rem;
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
.perm-status-bar {
  display: flex;
  align-items: center;
  gap: 8px;
  padding: 8px 14px;
  margin: 0 12px;
  border-radius: 8px;
  background: var(--glass-bg-light);
  border: 1px solid var(--border-color);
  color: var(--text-muted);
  font-size: 0.78rem;
  transition: all 0.15s;
}
.perm-status-bar.allowed {
  background: var(--glass-bg-light);
  border-color: var(--border-color);
  color: var(--text-soft);
}
.perm-status-bar.pending {
  background: color-mix(in srgb, var(--accent-blue) 8%, transparent);
  border-color: color-mix(in srgb, var(--accent-blue) 20%, transparent);
  color: var(--accent-blue);
}
.perm-status-bar svg { flex-shrink: 0; }
/* 已允许清单的折叠开关：只用一个箭头（条数放在 title 里），
   不为一条 UI 文案去动 locale 文件（那两个文件正被别处改动，避免互相覆盖） */
.perm-allow-toggle {
  flex-shrink: 0;
  padding: 2px 5px;
  border: 0;
  border-radius: 5px;
  background: transparent;
  color: var(--text-muted);
  cursor: pointer;
  transition: color 0.15s, background 0.15s;
}

.perm-allow-toggle:hover {
  color: var(--text-main);
  background: var(--glass-bg-light);
}

.perm-chev {
  display: inline-block;
  font-size: 0.72rem;
  line-height: 1;
}

.perm-revoke-btn {
  margin-left: auto;
  padding: 3px 10px;
  border-radius: 6px;
  border: 1px solid var(--border-color);
  background: transparent;
  color: var(--text-soft);
  font-size: 0.78rem;
  font-weight: 600;
  cursor: pointer;
  transition: all 0.15s;
}
.perm-revoke-btn:hover {
  background: var(--glass-bg-light);
  border-color: var(--text-muted);
}

/* 权限请求卡片列表（监控窗口内联展示） */
.perm-cards {
  padding: 0 12px;
  display: flex;
  flex-direction: column;
  gap: 8px;
}

/* 已允许清单（紧凑行式）：一行一条 + 悬停显撤销 + 内部滚动 */
.perm-allow-list {
  margin: 8px 12px 0;
  max-height: 200px;
  overflow-y: auto;
  border-top: 1px solid var(--glass-border-subtle);
  border-bottom: 1px solid var(--glass-border-subtle);
}

.perm-allow-row {
  display: flex;
  align-items: center;
  gap: 8px;
  padding: 4px 8px;
}

.perm-allow-row + .perm-allow-row {
  border-top: 1px solid color-mix(in srgb, var(--glass-border-subtle) 55%, transparent);
}

.perm-allow-row:hover {
  background: var(--glass-bg-light);
}

.perm-allow-label {
  flex: 1;
  min-width: 0;
  font-size: 0.78rem;
  line-height: 1.7;
  color: var(--text-soft);
  white-space: nowrap;
  overflow: hidden;
  text-overflow: ellipsis;
}

/* 撤销是低频动作：悬停行才浮现，列表常时只留内容本身 */
.perm-allow-revoke {
  flex: none;
  padding: 1px 6px;
  border: none;
  border-radius: 4px;
  background: transparent;
  color: var(--text-muted);
  font-size: 0.72rem;
  cursor: pointer;
  opacity: 0;
  transition: opacity 0.12s, color 0.12s, background 0.12s;
}

.perm-allow-row:hover .perm-allow-revoke {
  opacity: 1;
}

.perm-allow-revoke:hover {
  color: var(--accent-red);
  background: color-mix(in srgb, var(--accent-red) 8%, transparent);
}

.perm-card-inline {
  padding: 12px 14px;
  border: 1px solid color-mix(in srgb, var(--accent-blue) 20%, transparent);
  border-radius: 10px;
  background: color-mix(in srgb, var(--accent-blue) 6%, transparent);
}

/* 危险卡弱分级：只把左边框换成琥珀（2px），底色不动——一条边的信号量，够认出不吵 */
.perm-card-inline.perm-card-warning {
  border-left: 2px solid var(--border-warning);
}

.perm-card-msg {
  margin: 0 0 10px;
  color: var(--text-soft);
  font-size: 0.78rem;
  line-height: 1.55;
  word-break: break-word;
}

.perm-card-actions {
  display: flex;
  gap: 8px;
}

.perm-card-btn {
  padding: 5px 12px;
  border-radius: 6px;
  border: 1px solid var(--glass-border);
  background: var(--glass-bg-light);
  font-size: 0.78rem;
  font-weight: 600;
  cursor: pointer;
  transition: all 0.12s;
}
.perm-card-btn:hover {
  transform: translateY(-1px);
  box-shadow: 0 2px 8px rgba(0, 0, 0, 0.12);
}
.perm-card-btn.reject { color: var(--accent-red); border-color: color-mix(in srgb, var(--accent-red) 25%, transparent); background: color-mix(in srgb, var(--accent-red) 6%, transparent); }
.perm-card-btn.reject:hover { background: color-mix(in srgb, var(--accent-red) 14%, transparent); }
.perm-card-btn.allow { color: var(--accent-blue); border-color: color-mix(in srgb, var(--accent-blue) 25%, transparent); background: color-mix(in srgb, var(--accent-blue) 6%, transparent); }
.perm-card-btn.allow:hover { background: color-mix(in srgb, var(--accent-blue) 14%, transparent); }
.perm-card-btn.session { color: var(--text-soft); border-color: var(--border-color); background: transparent; }
.perm-card-btn.session:hover { background: var(--glass-bg-light); }

.panel-body {
  flex: 1;
  min-height: 0;
  padding: 12px;
  overflow: auto;
}

.monitor-section {
  min-width: 0;
  border: 1px solid var(--border-color);
  border-radius: 16px;
  background: color-mix(in srgb, var(--glass-bg) 82%, transparent);
  box-shadow: 0 10px 28px rgba(0, 0, 0, 0.1);
  padding: 12px;
}

.context-section {
  margin-bottom: 12px;
}

/* 单列纵向流。
   这三个 section 装的都是"列表 + 长文本"（子 Agent 时间线、后台命令与报错、计划正文），
   全宽才是它们需要的形状。
   用两列网格时的问题：Sub Agents 与 Plans 被 `grid-column: 1/-1` 强制跨列，
   单独的 Background 留在左列，**右列就空出一大片**（截图里的空白正是这么来的）。
   面板宽度本来就和视口无关，与其在窄面板里硬凑列数，不如老实单列。 */
.monitor-grid {
  display: flex;
  flex-direction: column;
  gap: 12px;
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
  font-size: 0.66rem;
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
  font-size: 0.72rem;
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
  font-size: 0.78rem;
  font-weight: 800;
  text-overflow: ellipsis;
  white-space: nowrap;
}

.monitor-item-main span:last-child {
  flex-shrink: 0;
  color: var(--text-muted);
  font-size: 0.72rem;
  font-weight: 750;
}

/* 失败/出错状态用红字：只靠左侧那个小圆点区分度太弱，
   一列任务扫下来很容易漏掉失败项 */
.monitor-item.status-failed .monitor-item-main span:last-child,
.monitor-item.status-error .monitor-item-main span:last-child {
  color: var(--accent-red);
}

.dismiss-task-btn {
  flex-shrink: 0;
  margin-left: auto;
  width: 18px;
  height: 18px;
  padding: 0;
  border: none;
  border-radius: 50%;
  background: transparent;
  color: var(--text-muted);
  font-size: 0.8rem;
  line-height: 1;
  cursor: pointer;
  opacity: 0;
  transition: opacity 0.15s, background 0.15s, color 0.15s;
}

.monitor-item:hover .dismiss-task-btn {
  opacity: 1;
}

.dismiss-task-btn:hover {
  background: color-mix(in srgb, var(--text-muted) 16%, transparent);
  color: var(--text-main);
}

.dismiss-task-btn:disabled,
.kill-task-btn:disabled {
  opacity: 0.3;
  cursor: not-allowed;
}

.kill-task-btn {
  flex-shrink: 0;
  margin-left: 2px;
  width: 18px;
  height: 18px;
  padding: 0;
  border: none;
  border-radius: 50%;
  background: transparent;
  color: var(--accent-red);
  font-size: 0.78rem;
  line-height: 1;
  cursor: pointer;
  opacity: 0;
  transition: opacity 0.15s, background 0.15s;
}

.monitor-item:hover .kill-task-btn {
  opacity: 1;
}

.kill-task-btn:hover {
  background: color-mix(in srgb, var(--accent-red) 16%, transparent);
}

.clear-all-tasks-btn {
  margin-top: 6px;
  padding: 4px 0;
  width: 100%;
  border: none;
  background: transparent;
  color: var(--text-muted);
  font-size: 0.72rem;
  cursor: pointer;
  transition: color 0.15s;
}

.clear-all-tasks-btn:hover {
  color: var(--accent-red);
}

/* 结果/报错文本：原来放开到 160px（约 9 行），一个任务卡就占掉四分之一面板高度。
   收到 72px（约 4 行）并保留内部滚动 —— 长报错仍可看全，但不再主导版面 */
.monitor-item p {
  margin: 6px 0 0;
  max-height: 72px;
  overflow: auto;
  color: var(--text-muted);
  font-size: 0.72rem;
  line-height: 1.45;
  white-space: pre-wrap;
  word-break: break-word;
}

/* 计划正文是这一块的主要内容，不该跟后台任务的报错摘要同一个高度上限 */
.plans-section .monitor-item p {
  max-height: 200px;
}

.monitor-item-meta {
  display: flex;
  flex-wrap: wrap;
  gap: 6px;
  margin-top: 7px;
  color: var(--text-muted);
  font-size: 0.72rem;
  font-variant-numeric: tabular-nums;
}

.monitor-item-meta span {
  max-height: 140px;
  overflow: auto;
  word-break: break-all;
  white-space: pre-wrap;
}

/* 子 Agent 列表不再设 max-height / 内部滚动（2026-09-19）：
   - 列表本来只显示前 12 张卡（currentSubAgents = slice(0, 12)），收起态全长 ~430px，
     与旧上限 480px 几乎相同 —— 内滚在现实中极少触发，却让面板常驻"滚动套滚动"
     （外层 .panel-body 一根、这里一根），滚轮悬停位置决定滚哪层，手感割裂；
   - 主体内容属于阅读流，交给面板整体滚动；展开的详情（.tool-timeline）才是局部焦点，
     由它自己内滚，见下方注释。 */
.subagent-list {
  display: flex;
  flex-direction: column;
  gap: 8px;
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
  font-size: 0.78rem;
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
  font-size: 0.66rem;
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
  background: color-mix(in srgb, var(--text-muted) 20%, transparent);
  color: var(--text-muted);
  font-size: 0.66rem;
  font-weight: 900;
}

.phase-badge {
  flex-shrink: 0;
  padding: 1px 5px;
  border-radius: 4px;
  font-size: 0.66rem;
  font-weight: 750;
}

.phase-starting,
.phase-waiting_model,
.phase-streaming,
.phase-thinking,
.phase-calling_tool,
.phase-processing_tool_result,
.phase-finalizing {
  background: color-mix(in srgb, var(--text-muted) 15%, transparent);
  color: var(--text-soft);
}

.status-label {
  flex-shrink: 0;
  color: var(--text-muted);
  font-size: 0.72rem;
  font-weight: 700;
}

.loop-badge {
  flex-shrink: 0;
  color: var(--text-muted);
  font-size: 0.72rem;
  font-weight: 700;
  font-variant-numeric: tabular-nums;
}

/* 收起行上的工具调用计数：与 loop-badge 同族 —— 都是元信息，不带状态语义 */
.tool-count-badge {
  flex-shrink: 0;
  color: var(--text-muted);
  font-size: 0.72rem;
  font-weight: 700;
}

.countdown-badge {
  flex-shrink: 0;
  color: var(--accent-blue);
  font-size: 0.72rem;
  font-weight: 700;
  font-variant-numeric: tabular-nums;
}

.expand-arrow {
  flex-shrink: 0;
  width: 16px;
  text-align: center;
  color: var(--text-muted);
  font-size: 0.66rem;
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
  font-size: 0.72rem;
  border-bottom: 1px solid color-mix(in srgb, var(--border-color) 30%, transparent);
}

.ct-label {
  color: var(--text-muted);
  font-weight: 700;
  flex-shrink: 0;
}

.ct-name {
  color: var(--text-main);
  font-weight: 800;
  font-family: ui-monospace, 'Cascadia Code', monospace;
}

.ct-input {
  color: var(--text-muted);
  font-size: 0.72rem;
  overflow: hidden;
  text-overflow: ellipsis;
  white-space: nowrap;
}

/* 工具时间线：保留 max-height 内滚（与 .subagent-list 的取舍相反）。
   它位于"展开的详情卡"内部 —— 一个跑了几十轮的子 Agent 会产出上百行记录，
   不设上限的话点开一张卡就吞掉整个面板，其他区块全被挤走。
   展开的详情是"局部放大镜"，不该绑架全局导航。 */
.tool-timeline {
  max-height: 200px;
  overflow-y: auto;
  margin-top: 8px;
  padding: 4px 0;
}

.tool-timeline-empty {
  padding: 8px 0;
  color: var(--text-muted);
  font-size: 0.72rem;
}

.timeline-header {
  display: flex;
  align-items: center;
  justify-content: space-between;
  color: var(--text-muted);
  font-size: 0.66rem;
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
  font-size: 0.66rem;
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
  font-size: 0.72rem;
  font-family: ui-monospace, 'Cascadia Code', monospace;
}

.tl-loop {
  flex-shrink: 0;
  width: 26px;
  color: var(--text-muted);
  font-size: 0.66rem;
}

.tl-icon {
  flex-shrink: 0;
  width: 12px;
  text-align: center;
  font-size: 0.66rem;
}

.tl-tool_call .tl-icon { color: var(--text-muted); }
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
  font-size: 0.66rem;
}

/* Token 明细 */
.detail-tokens {
  display: flex;
  align-items: center;
  gap: 4px;
  padding: 7px 0;
  color: var(--text-muted);
  font-size: 0.72rem;
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
  font-size: 0.66rem;
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
  font-size: 0.72rem;
  font-weight: 800;
  margin-bottom: 4px;
}

.detail-error pre {
  margin: 0;
  color: var(--accent-red);
  font-size: 0.66rem;
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
  font-size: 0.66rem;
  font-weight: 800;
  margin-bottom: 2px;
}

.detail-prompt p {
  margin: 0;
  color: var(--text-muted);
  font-size: 0.72rem;
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
  background: var(--text-muted);
  animation: runningDotPulse 1.5s ease-in-out infinite;
}

.status-completed .status-dot,
.status-approved .status-dot {
  background: var(--accent-green);
}

.status-failed .status-dot,
.status-error .status-dot,
.status-rejected .status-dot,
.status-cancelled .status-dot {
  background: var(--accent-red);
}

/* 空态与 section 标题之间不再留一整行空白：标题带的 margin-bottom 已经够了 */
.monitor-empty {
  padding: 2px 2px 2px;
  color: var(--text-muted);
  font-size: 0.72rem;
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

/* 原来的 `@media (max-width: 1180px)` 已删除：它挂在视口上，而面板宽度与视口无关
   （内嵌时视口 1600 永不触发，于是 620 宽的面板里硬塞两列，每列只剩 ~283px）。
   列数现在由 .monitor-grid 的 auto-fit 决定，不需要断点。 */

/* 这一条**保留视口断点**（与上面那条不同）：它调的是面板相对窗口的边距，
   语义本来就是"窗口窄 → 面板贴边"，视口才是正确的判据。
   （standalone 时 `.agent-panel.standalone` 的 width:100% 优先级更高，不受影响） */
@media (max-width: 920px) {
  .agent-panel {
    right: 8px;
    width: calc(100% - 16px);
  }
}
</style>
