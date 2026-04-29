<script setup lang="ts">
import { computed, nextTick, onUnmounted, ref, watch } from 'vue';
import { marked } from 'marked';
import { useSessionStore } from '../../stores/session';
import { useChatStore } from '../../stores/chat';
import { useAgentStore } from '../../stores/agent';
import { usePermissionStore } from '../../stores/permission';
import type { AgentStep, PlanDocument, SubAgentEvent, SubAgentRun } from '../../types';

const session = useSessionStore();
const chat = useChatStore();
const agent = useAgentStore();
const perm = usePermissionStore();

type PanelSection = 'subagents' | 'flow' | 'plans';
type StepState = 'pending' | 'running' | 'completed' | 'failed' | 'cancelled';

interface StepView {
  step: AgentStep;
  sourceIndex: number;
  state: StepState;
  detail?: string;
}

const stepsContainer = ref<HTMLElement | null>(null);
const hoveredStep = ref<number | null>(null);
const tooltipStyle = ref({ top: '0px', left: '0px' });
const expandedRunIds = ref<Record<string, boolean>>({});
const expandedPlanIds = ref<Record<string, boolean>>({});
const openSections = ref<Record<PanelSection, boolean>>({
  subagents: true,
  flow: true,
  plans: true,
});

const elapsed = ref(0);
let timer: ReturnType<typeof setInterval> | null = null;

watch(() => session.isCurrentSessionRunning, (running) => {
  if (running) {
    elapsed.value = 0;
    timer = setInterval(() => { elapsed.value++; }, 1000);
  } else if (timer) {
    clearInterval(timer);
    timer = null;
  }
});

onUnmounted(() => {
  if (timer) clearInterval(timer);
});

const formatTime = (s: number): string => {
  const m = Math.floor(s / 60);
  const sec = s % 60;
  return m > 0 ? `${m}:${sec.toString().padStart(2, '0')}` : `${sec}s`;
};

const formatDurationMs = (ms: number): string => {
  return formatTime(Math.max(0, Math.floor(ms / 1000)));
};

const getRunDuration = (run: SubAgentRun): string => {
  elapsed.value;
  const end = run.finishedAt ?? Date.now();
  return formatDurationMs(end - run.startedAt);
};

const getPhaseLabel = (run: SubAgentRun): string => {
  if (run.status === 'completed') return '已完成';
  if (run.status === 'failed') return '失败';
  if (run.status === 'cancelled') return '已取消';
  switch (run.phase) {
    case 'starting': return '启动中';
    case 'waiting_model': return '等待模型';
    case 'streaming': return '接收输出';
    case 'thinking': return '思考中';
    case 'calling_tool': return run.currentTool ? `调用 ${run.currentTool}` : '调用工具';
    case 'processing_tool_result': return '处理结果';
    case 'finalizing': return '收尾中';
    default: return run.phase;
  }
};

const getRunClass = (run: SubAgentRun): string => `subagent-${run.status}`;

const isRunStale = (run: SubAgentRun): boolean => {
  elapsed.value;
  return run.status === 'running' && Date.now() - run.updatedAt > 30000;
};

const isRunExpanded = (run: SubAgentRun): boolean => {
  return expandedRunIds.value[run.runId] ?? run.status === 'running';
};

const toggleRunDetails = (run: SubAgentRun) => {
  expandedRunIds.value = {
    ...expandedRunIds.value,
    [run.runId]: !isRunExpanded(run),
  };
};

watch(
  () => agent.currentSubAgentRuns.map((run) => ({ runId: run.runId, status: run.status })),
  (runs, previousRuns = []) => {
    const previousStatusById = new Map(previousRuns.map((run) => [run.runId, run.status]));
    const activeRunIds = new Set(runs.map((run) => run.runId));
    let changed = false;
    const nextExpanded = { ...expandedRunIds.value };

    for (const run of runs) {
      if (run.status !== 'running' && previousStatusById.get(run.runId) === 'running') {
        nextExpanded[run.runId] = false;
        changed = true;
      }
    }

    for (const runId of Object.keys(nextExpanded)) {
      if (!activeRunIds.has(runId)) {
        delete nextExpanded[runId];
        changed = true;
      }
    }

    if (changed) {
      expandedRunIds.value = nextExpanded;
    }
  },
);

const getVisibleEvents = (run: SubAgentRun): SubAgentEvent[] => {
  const events = agent.getSubAgentEvents(run.runId);
  return isRunExpanded(run) ? events.slice(-12) : events.slice(-3);
};

const formatEventTime = (timestamp: number): string => {
  const date = new Date(timestamp);
  return `${date.getHours().toString().padStart(2, '0')}:${date.getMinutes().toString().padStart(2, '0')}:${date.getSeconds().toString().padStart(2, '0')}`;
};

const getEventLabel = (event: SubAgentEvent): string => {
  switch (event.eventType) {
    case 'start': return '启动';
    case 'phase': return '阶段';
    case 'tool_call': return event.tool || '工具';
    case 'tool_result': return '结果';
    case 'complete': return '完成';
    case 'cancel': return '取消';
    case 'error': return '错误';
    default: return event.eventType;
  }
};

const getEventDetail = (event: SubAgentEvent): string => {
  return event.error || event.outputSummary || event.inputSummary || event.message;
};

const getStepDetail = (step: AgentStep): string => {
  switch (step.type) {
    case 'thinking':
    case 'plan':
      return step.content || '';
    case 'tool_call':
      return step.input_summary || '';
    case 'tool_result':
      return step.output_summary || '';
    case 'tool_error':
      return step.error || '';
    case 'subagent_start':
      return step.task || '';
    default:
      return '';
  }
};

const visibleSteps = computed<StepView[]>(() => {
  const views: StepView[] = [];

  const findOpenStep = (type: AgentStep['type'], tool?: string) => {
    for (let i = views.length - 1; i >= 0; i--) {
      const view = views[i];
      if (view.step.type !== type) continue;
      if (tool && view.step.tool !== tool) continue;
      if (view.state !== 'pending') continue;
      return view;
    }
    return null;
  };

  agent.agentSteps.forEach((step, index) => {
    if (step.type === 'tool_result') {
      const target = findOpenStep('tool_call', step.tool);
      if (target) {
        target.state = 'completed';
        target.detail = step.output_summary || target.detail;
      }
      return;
    }

    if (step.type === 'tool_error') {
      const target = findOpenStep('tool_call', step.tool);
      if (target) {
        target.state = 'failed';
        target.detail = step.error || target.detail;
      } else {
        views.push({ step, sourceIndex: index, state: 'failed', detail: step.error });
      }
      return;
    }

    if (step.type === 'subagent_end') {
      const target = findOpenStep('subagent_start');
      if (target) target.state = 'completed';
      return;
    }

    views.push({
      step,
      sourceIndex: index,
      state: step.type === 'cancelled' ? 'cancelled' : 'pending',
      detail: getStepDetail(step),
    });
  });

  views.forEach((view, index) => {
    if (view.state !== 'pending') return;
    view.state = session.isCurrentSessionRunning && index === views.length - 1 ? 'running' : 'completed';
  });

  return views;
});

watch(() => agent.agentSteps.length, async () => {
  await nextTick();
  if (stepsContainer.value) {
    stepsContainer.value.scrollTop = stepsContainer.value.scrollHeight;
  }
});

const getStepLabel = (step: AgentStep): string => {
  switch (step.type) {
    case 'thinking': return '思考';
    case 'plan': return '计划';
    case 'tool_call': return step.tool || '调用';
    case 'tool_result': return step.tool || '完成';
    case 'tool_error': return '错误';
    case 'subagent_start': return '子代理';
    case 'subagent_end': return '子代理完成';
    case 'retry': return `重试 ${step.attempt || 0}/${step.max || 0}`;
    case 'cancelled': return '已取消';
    default: return step.type;
  }
};

const getStepClass = (view: StepView): string => {
  const stateClass = `step-state-${view.state}`;
  switch (view.step.type) {
    case 'thinking': return `step-thinking ${stateClass}`;
    case 'plan': return `step-plan ${stateClass}`;
    case 'tool_call': return `step-tool-call ${stateClass}`;
    case 'tool_result': return `step-tool-result ${stateClass}`;
    case 'tool_error': return `step-tool-error ${stateClass}`;
    case 'subagent_start':
    case 'subagent_end':
      return `step-subagent ${stateClass}`;
    case 'retry': return `step-retry ${stateClass}`;
    case 'cancelled': return `step-cancelled ${stateClass}`;
    default: return stateClass;
  }
};

const isLastStep = (index: number): boolean => index === visibleSteps.value.length - 1;

const togglePlanDocument = (planId: string) => {
  expandedPlanIds.value = {
    ...expandedPlanIds.value,
    [planId]: !expandedPlanIds.value[planId],
  };
};

const isPlanExpanded = (plan: PlanDocument): boolean => {
  return expandedPlanIds.value[plan.id] ?? plan.status === 'pending';
};

const renderPlanContent = (plan: PlanDocument): string => marked.parse(plan.content || '') as string;

const getPlanStatusLabel = (status: string): string => {
  switch (status) {
    case 'pending': return '待审批';
    case 'approved': return '已同意';
    case 'rejected': return '已拒绝';
    default: return status;
  }
};

const getPlanStatusClass = (status: string): string => {
  switch (status) {
    case 'pending': return 'plan-pending';
    case 'approved': return 'plan-approved';
    case 'rejected': return 'plan-rejected';
    default: return '';
  }
};

const toggleSection = (section: PanelSection) => {
  openSections.value = {
    ...openSections.value,
    [section]: !openSections.value[section],
  };
};

const closePanel = () => {
  agent.showAgentPanel = false;
};

const showTooltip = (index: number, event: MouseEvent) => {
  const view = visibleSteps.value[index];
  if (!view) return;
  const detail = view.detail || getStepDetail(view.step);
  if (!detail) return;
  hoveredStep.value = index;
  const rect = (event.currentTarget as HTMLElement).getBoundingClientRect();
  const panelRect = (event.currentTarget as HTMLElement).closest('.agent-panel')?.getBoundingClientRect();
  if (panelRect) {
    tooltipStyle.value = {
      top: `${rect.top - panelRect.top + 20}px`,
      left: `${rect.left - panelRect.left}px`,
    };
  }
};

const hideTooltip = () => {
  hoveredStep.value = null;
};
</script>

<template>
  <Transition name="panel-slide">
    <div
      v-if="agent.showAgentPanel"
      class="agent-panel"
    >
      <div class="panel-header">
        <div class="panel-title">
          <span>监控</span>
          <span v-if="session.isCurrentSessionRunning" class="running-dot"></span>
          <span v-if="session.isCurrentSessionRunning" class="elapsed-time">{{ formatTime(elapsed) }}</span>
          <span v-if="agent.activeSubAgentRuns.length > 0" class="subagent-count">{{ agent.activeSubAgentRuns.length }}</span>
        </div>
        <button class="close-btn" @click="closePanel" title="关闭">
          <svg viewBox="0 0 24 24" width="12" height="12" stroke="currentColor" stroke-width="2" fill="none" stroke-linecap="round" stroke-linejoin="round">
            <line x1="18" y1="6" x2="6" y2="18"></line>
            <line x1="6" y1="6" x2="18" y2="18"></line>
          </svg>
        </button>
      </div>

      <div class="panel-accordion">
        <div
          v-if="agent.currentSubAgentRuns.length === 0 && agent.agentSteps.length === 0 && perm.currentPlanDocuments.length === 0"
          class="panel-empty"
        >
          <div class="panel-empty-icon">
            <svg viewBox="0 0 24 24" width="32" height="32" stroke="currentColor" stroke-width="1.5" fill="none" stroke-linecap="round" stroke-linejoin="round">
              <polyline points="22 12 18 12 15 21 9 3 6 12 2 12"></polyline>
            </svg>
          </div>
          <p class="panel-empty-text">暂无执行数据</p>
          <p class="panel-empty-hint">发送消息后，此处将展示 Agent 执行流程、子 Agent 状态和任务计划</p>
        </div>
        <section class="panel-section" :class="{ open: openSections.subagents }">
          <button class="section-header" type="button" @click="toggleSection('subagents')">
            <svg class="section-chevron" :class="{ open: openSections.subagents }" viewBox="0 0 16 16" width="12" height="12" fill="none">
              <path d="M6 4l4 4-4 4" stroke="currentColor" stroke-width="1.8" stroke-linecap="round" stroke-linejoin="round" />
            </svg>
            <span class="section-title">子 Agent</span>
            <span class="section-count">{{ agent.activeSubAgentRuns.length }}/{{ agent.currentSubAgentRuns.length }}</span>
          </button>
          <div v-if="openSections.subagents" class="section-body">
            <div v-if="agent.currentSubAgentRuns.length > 0" class="subagent-list">
              <div
                v-for="run in agent.currentSubAgentRuns"
                :key="run.runId"
                class="subagent-run"
                :class="[getRunClass(run), { stale: isRunStale(run), expanded: isRunExpanded(run) }]"
                :title="run.promptPreview"
                @click="toggleRunDetails(run)"
              >
                <div class="subagent-main">
                  <span class="subagent-status-dot"></span>
                  <span class="subagent-name">{{ run.label || run.runId }}</span>
                  <button v-if="run.taskId" class="task-link" type="button" title="跳到关联任务" @click.stop="agent.focusTask(run.taskId)">
                    #{{ run.taskId }}
                  </button>
                  <button v-if="run.status === 'running'" class="run-cancel-btn" type="button" title="取消此子 Agent" @click.stop="chat.cancelSubAgentRun(run.runId)">
                    停止
                  </button>
                  <span v-if="!isRunExpanded(run)" class="subagent-compact-phase">{{ getPhaseLabel(run) }}</span>
                  <span class="subagent-time">{{ getRunDuration(run) }}</span>
                </div>
                <div v-if="isRunExpanded(run)" class="subagent-details">
                  <div class="subagent-phase">{{ getPhaseLabel(run) }}</div>
                  <div class="subagent-meta">
                    <span>{{ run.loopCount }}/{{ run.maxLoops }} 轮</span>
                    <span>{{ run.inputTokens + run.outputTokens }} tok</span>
                    <span v-if="run.readOnly">只读</span>
                  </div>
                  <div v-if="run.summary" class="subagent-summary">{{ run.summary }}</div>
                  <div v-if="run.error" class="subagent-error">{{ run.error }}</div>
                  <div v-if="getVisibleEvents(run).length > 0" class="subagent-events">
                    <div
                      v-for="event in getVisibleEvents(run)"
                      :key="event.eventId"
                      class="subagent-event"
                      :class="`event-${event.eventType}`"
                    >
                      <span class="event-time">{{ formatEventTime(event.timestamp) }}</span>
                      <span class="event-label">{{ getEventLabel(event) }}</span>
                      <span class="event-detail">{{ getEventDetail(event) }}</span>
                    </div>
                  </div>
                </div>
              </div>
            </div>
            <div v-else class="section-empty">暂无子 Agent</div>
          </div>
        </section>

        <section class="panel-section" :class="{ open: openSections.flow }">
          <button class="section-header" type="button" @click="toggleSection('flow')">
            <svg class="section-chevron" :class="{ open: openSections.flow }" viewBox="0 0 16 16" width="12" height="12" fill="none">
              <path d="M6 4l4 4-4 4" stroke="currentColor" stroke-width="1.8" stroke-linecap="round" stroke-linejoin="round" />
            </svg>
            <span class="section-title">执行流程</span>
            <span class="section-count">{{ visibleSteps.length }}</span>
          </button>
          <div v-if="openSections.flow" ref="stepsContainer" class="section-body steps-container">
            <div
              v-for="(view, index) in visibleSteps"
              :key="`${view.sourceIndex}-${view.step.type}`"
              class="step-row"
              :class="[getStepClass(view), { active: view.state === 'running' && isLastStep(index) && session.isCurrentSessionRunning }]"
              @mouseenter="showTooltip(index, $event)"
              @mouseleave="hideTooltip"
            >
              <div class="step-rail">
                <div class="step-dot"></div>
                <div v-if="!isLastStep(index)" class="step-line"></div>
              </div>
              <div class="step-body">
                <div class="step-main">
                  <span class="step-label">{{ getStepLabel(view.step) }}</span>
                </div>
              </div>
            </div>
            <div v-if="visibleSteps.length === 0" class="section-empty">暂无执行流程</div>
            <div
              v-if="hoveredStep !== null && visibleSteps[hoveredStep] && (visibleSteps[hoveredStep].detail || getStepDetail(visibleSteps[hoveredStep].step))"
              class="step-tooltip"
              :style="tooltipStyle"
            >
              {{ visibleSteps[hoveredStep].detail || getStepDetail(visibleSteps[hoveredStep].step) }}
            </div>
          </div>
        </section>

        <section class="panel-section" :class="{ open: openSections.plans }">
          <button class="section-header" type="button" @click="toggleSection('plans')">
            <svg class="section-chevron" :class="{ open: openSections.plans }" viewBox="0 0 16 16" width="12" height="12" fill="none">
              <path d="M6 4l4 4-4 4" stroke="currentColor" stroke-width="1.8" stroke-linecap="round" stroke-linejoin="round" />
            </svg>
            <span class="section-title">任务计划</span>
            <span class="section-count">{{ perm.currentPlanDocuments.length }}</span>
          </button>
          <div v-if="openSections.plans" class="section-body">
            <div v-if="perm.currentPlanDocuments.length > 0" class="plan-doc-list">
              <section
                v-for="plan in perm.currentPlanDocuments"
                :key="plan.id"
                class="plan-doc"
                :class="getPlanStatusClass(plan.status)"
              >
                <button class="plan-doc-head" type="button" @click="togglePlanDocument(plan.id)">
                  <span class="plan-doc-dot"></span>
                  <span class="plan-doc-title">{{ plan.title }}</span>
                  <span class="plan-doc-status">{{ getPlanStatusLabel(plan.status) }}</span>
                </button>
                <article v-if="isPlanExpanded(plan)" class="plan-doc-body" v-html="renderPlanContent(plan)"></article>
              </section>
            </div>
            <div v-else class="section-empty">暂无任务计划</div>
          </div>
        </section>
      </div>
    </div>
  </Transition>
</template>

<style scoped>
.agent-panel {
  width: 260px;
  min-width: 220px;
  background-color: var(--bg-sidebar);
  border-left: 1px solid var(--border-color);
  display: flex;
  flex-direction: column;
  overflow: hidden;
  flex-shrink: 0;
}

.panel-empty {
  display: flex;
  flex-direction: column;
  align-items: center;
  justify-content: center;
  padding: 40px 20px;
  text-align: center;
  gap: 8px;
}

.panel-empty-icon {
  color: var(--text-muted);
  opacity: 0.4;
  margin-bottom: 4px;
}

.panel-empty-text {
  font-size: 0.9rem;
  font-weight: 500;
  color: var(--text-muted);
  margin: 0;
}

.panel-empty-hint {
  font-size: 0.8rem;
  color: var(--text-muted);
  opacity: 0.6;
  margin: 0;
  line-height: 1.5;
}

.panel-header {
  height: 38px;
  display: flex;
  justify-content: space-between;
  align-items: center;
  padding: 0 10px 0 12px;
  border-bottom: 1px solid var(--border-color);
  flex-shrink: 0;
}

.panel-title {
  display: flex;
  align-items: center;
  gap: 6px;
  min-width: 0;
  color: var(--text-muted);
  font-size: 0.72rem;
  font-weight: 700;
  letter-spacing: 0;
}

.running-dot {
  width: 6px;
  height: 6px;
  border-radius: 50%;
  background: var(--accent-yellow);
  animation: dotBreathe 1.5s ease-in-out infinite;
}

.elapsed-time {
  color: var(--accent-yellow);
  font-size: 0.68rem;
  font-variant-numeric: tabular-nums;
}

.subagent-count,
.section-count {
  min-width: 16px;
  height: 16px;
  padding: 0 5px;
  border-radius: 8px;
  display: inline-flex;
  align-items: center;
  justify-content: center;
  color: var(--text-muted);
  background: var(--glass-bg-light);
  font-size: 0.62rem;
  font-weight: 700;
  font-variant-numeric: tabular-nums;
}

.close-btn {
  width: 24px;
  height: 24px;
  display: inline-flex;
  align-items: center;
  justify-content: center;
  border: 0;
  border-radius: 4px;
  color: var(--text-muted);
  background: transparent;
  cursor: pointer;
  opacity: 0.62;
}

.close-btn:hover {
  color: var(--text-main);
  background: var(--glass-bg-light);
  opacity: 1;
}

.panel-accordion {
  flex: 1;
  min-height: 0;
  display: flex;
  flex-direction: column;
}

.panel-section {
  min-height: 0;
  display: flex;
  flex-direction: column;
  border-bottom: 1px solid var(--border-color);
  flex: 0 0 auto;
}

.panel-section.open {
  flex: 1 1 0;
}

.section-header {
  height: 28px;
  display: grid;
  grid-template-columns: 14px minmax(0, 1fr) auto;
  align-items: center;
  gap: 5px;
  padding: 0 8px;
  color: var(--text-muted);
  border: 0;
  background: transparent;
  cursor: pointer;
  text-align: left;
}

.section-header:hover {
  color: var(--text-main);
  background: var(--glass-bg-light);
}

.section-chevron {
  transform: rotate(0deg);
  transition: transform var(--transition-fast);
}

.section-chevron.open {
  transform: rotate(90deg);
}

.section-title {
  min-width: 0;
  overflow: hidden;
  text-overflow: ellipsis;
  white-space: nowrap;
  font-size: 0.68rem;
  font-weight: 700;
}

.section-body {
  flex: 1;
  min-height: 0;
  overflow-y: auto;
  padding: 8px 10px 10px;
}

.section-empty {
  padding: 8px 2px;
  color: var(--text-muted);
  font-size: 0.68rem;
}

.subagent-list,
.plan-doc-list {
  display: flex;
  flex-direction: column;
  gap: 6px;
}

.subagent-run,
.plan-doc {
  border: 1px solid var(--border-color);
  border-radius: 6px;
  background: var(--glass-bg);
  overflow: hidden;
}

.subagent-run {
  padding: 7px 8px;
  cursor: pointer;
  transition: border-color var(--transition-fast), background-color var(--transition-fast);
}

.subagent-run:hover {
  border-color: rgba(139, 92, 246, 0.35);
  background: var(--glass-bg-light);
}

.subagent-main {
  display: flex;
  align-items: center;
  gap: 6px;
  min-width: 0;
}

.subagent-status-dot,
.plan-doc-dot {
  width: 7px;
  height: 7px;
  border-radius: 50%;
  flex-shrink: 0;
  background: var(--text-muted);
}

.subagent-name {
  flex: 1;
  min-width: 0;
  color: var(--text-main);
  font-size: 0.72rem;
  font-weight: 600;
  white-space: nowrap;
  overflow: hidden;
  text-overflow: ellipsis;
}

.subagent-time {
  color: var(--text-muted);
  font-size: 0.66rem;
  font-variant-numeric: tabular-nums;
  flex-shrink: 0;
}

.subagent-compact-phase {
  max-width: 86px;
  color: var(--text-muted);
  font-size: 0.64rem;
  white-space: nowrap;
  overflow: hidden;
  text-overflow: ellipsis;
  flex-shrink: 0;
}

.task-link,
.run-cancel-btn {
  height: 18px;
  padding: 0 5px;
  color: var(--text-muted);
  border: 1px solid var(--border-color);
  border-radius: 4px;
  background: transparent;
  font-size: 0.6rem;
  line-height: 16px;
  cursor: pointer;
  flex-shrink: 0;
}

.task-link:hover {
  color: var(--accent-blue);
  border-color: rgba(59, 130, 246, 0.45);
  background: rgba(59, 130, 246, 0.08);
}

.run-cancel-btn:hover {
  color: var(--accent-red);
  border-color: rgba(239, 68, 68, 0.45);
  background: rgba(239, 68, 68, 0.08);
}

.subagent-phase {
  margin-top: 5px;
  color: #a78bfa;
  font-size: 0.68rem;
  white-space: nowrap;
  overflow: hidden;
  text-overflow: ellipsis;
}

.subagent-meta {
  display: flex;
  gap: 8px;
  margin-top: 5px;
  color: var(--text-muted);
  font-size: 0.62rem;
  font-variant-numeric: tabular-nums;
  white-space: nowrap;
  overflow: hidden;
}

.subagent-summary,
.subagent-error {
  margin-top: 6px;
  font-size: 0.64rem;
  line-height: 1.35;
  word-break: break-word;
}

.subagent-summary {
  color: var(--text-main);
  display: -webkit-box;
  -webkit-line-clamp: 2;
  -webkit-box-orient: vertical;
  overflow: hidden;
}

.subagent-error {
  color: var(--accent-red);
}

.subagent-events {
  margin-top: 7px;
  padding-top: 7px;
  border-top: 1px solid var(--border-color);
  display: flex;
  flex-direction: column;
  gap: 4px;
}

.subagent-event {
  display: grid;
  grid-template-columns: 44px 38px minmax(0, 1fr);
  align-items: baseline;
  gap: 5px;
  min-width: 0;
  font-size: 0.61rem;
  line-height: 1.35;
}

.event-time {
  color: var(--text-muted);
  font-variant-numeric: tabular-nums;
}

.event-label {
  color: #a78bfa;
  font-weight: 600;
  white-space: nowrap;
  overflow: hidden;
  text-overflow: ellipsis;
}

.event-detail {
  color: var(--text-muted);
  white-space: nowrap;
  overflow: hidden;
  text-overflow: ellipsis;
}

.event-error .event-label,
.event-error .event-detail {
  color: var(--accent-red);
}

.event-cancel .event-label,
.event-cancel .event-detail,
.event-tool_call .event-label {
  color: var(--accent-yellow);
}

.event-tool_result .event-label,
.event-complete .event-label {
  color: var(--accent-green);
}

.subagent-running .subagent-status-dot {
  background: #a78bfa;
  animation: dotBreathe 1.5s ease-in-out infinite;
}

.subagent-completed .subagent-status-dot {
  background: var(--accent-green);
}

.subagent-failed .subagent-status-dot {
  background: var(--accent-red);
}

.subagent-cancelled .subagent-status-dot,
.subagent-run.stale .subagent-status-dot {
  background: var(--text-muted);
}

.subagent-run.stale .subagent-phase {
  color: var(--accent-yellow);
}

.steps-container {
  position: relative;
  padding: 8px 0 10px;
}

.step-row {
  display: flex;
  align-items: stretch;
  min-height: 28px;
}

.step-rail {
  width: 20px;
  display: flex;
  flex-direction: column;
  align-items: center;
  flex-shrink: 0;
}

.step-dot {
  width: 6px;
  height: 6px;
  border-radius: 50%;
  background: var(--text-muted);
  margin-top: 7px;
  flex-shrink: 0;
  opacity: 0.5;
}

.step-line {
  width: 1px;
  flex: 1;
  background: var(--border-color);
  opacity: 0.4;
}

.step-body {
  flex: 1;
  min-width: 0;
  padding: 2px 10px 6px 0;
}

.step-main {
  display: flex;
  align-items: center;
  height: 20px;
}

.step-label {
  color: var(--text-muted);
  font-size: 0.72rem;
  font-weight: 500;
  white-space: nowrap;
  overflow: hidden;
  text-overflow: ellipsis;
}

.step-tooltip {
  position: absolute;
  z-index: 100;
  max-width: 220px;
  padding: 8px 10px;
  color: var(--text-main);
  background: var(--bg-dark);
  border: 1px solid var(--border-color);
  border-radius: 6px;
  box-shadow: 0 4px 12px rgba(0, 0, 0, 0.3);
  font-size: 0.68rem;
  line-height: 1.4;
  white-space: pre-wrap;
  word-break: break-all;
  pointer-events: none;
}

.step-state-running .step-dot { background: var(--accent-yellow); opacity: 1; }
.step-state-running .step-label { color: var(--text-main); font-weight: 600; }
.step-state-completed .step-dot { background: var(--accent-green); opacity: 1; }
.step-state-completed .step-label { color: var(--text-muted); }
.step-state-failed .step-dot { background: var(--accent-red); opacity: 1; }
.step-state-failed .step-label { color: var(--accent-red); }
.step-state-cancelled .step-dot { background: var(--text-muted); opacity: 0.5; }
.step-state-cancelled .step-label { color: var(--text-muted); opacity: 0.6; }

.step-row.active .step-dot {
  width: 8px;
  height: 8px;
  margin-top: 6px;
  border: 1.5px solid transparent;
  border-top-color: var(--accent-yellow);
  border-right-color: var(--accent-yellow);
  border-radius: 50%;
  background: transparent;
  opacity: 1;
  animation: spin 1.2s linear infinite;
}

.plan-doc-head {
  width: 100%;
  min-height: 30px;
  display: grid;
  grid-template-columns: 8px minmax(0, 1fr) auto;
  align-items: center;
  gap: 7px;
  padding: 6px 8px;
  color: var(--text-main);
  border: 0;
  background: transparent;
  cursor: pointer;
  text-align: left;
}

.plan-doc-head:hover {
  background: var(--glass-bg-light);
}

.plan-doc-title {
  min-width: 0;
  font-size: 0.72rem;
  font-weight: 600;
  white-space: nowrap;
  overflow: hidden;
  text-overflow: ellipsis;
}

.plan-doc-status {
  color: var(--text-muted);
  font-size: 0.62rem;
  white-space: nowrap;
}

.plan-doc-body {
  max-height: 240px;
  overflow-y: auto;
  padding: 8px 9px 10px;
  border-top: 1px solid var(--border-color);
  color: var(--text-main);
  font-size: 0.68rem;
  line-height: 1.55;
}

.plan-doc-body :deep(h1),
.plan-doc-body :deep(h2),
.plan-doc-body :deep(h3) {
  margin: 8px 0 5px;
  font-size: 0.76rem;
  line-height: 1.35;
}

.plan-doc-body :deep(p) {
  margin: 6px 0;
}

.plan-doc-body :deep(ul),
.plan-doc-body :deep(ol) {
  margin: 6px 0;
  padding-left: 18px;
}

.plan-doc-body :deep(code) {
  font-family: var(--font-mono);
  font-size: 0.92em;
}

.plan-pending .plan-doc-dot {
  background: var(--accent-yellow);
  animation: dotBreathe 1.5s ease-in-out infinite;
}

.plan-approved .plan-doc-dot {
  background: var(--accent-green);
}

.plan-rejected .plan-doc-dot {
  background: var(--accent-red);
}

@keyframes dotBreathe {
  0%, 100% { opacity: 0.4; box-shadow: 0 0 0 0 rgba(245, 158, 11, 0.4); }
  50% { opacity: 1; box-shadow: 0 0 6px 2px rgba(245, 158, 11, 0.3); }
}

@keyframes spin {
  to { transform: rotate(360deg); }
}

.panel-slide-enter-active,
.panel-slide-leave-active {
  transition: all var(--transition-normal);
}

.panel-slide-enter-from,
.panel-slide-leave-to {
  width: 0;
  min-width: 0;
  opacity: 0;
  padding: 0;
  overflow: hidden;
}
</style>
