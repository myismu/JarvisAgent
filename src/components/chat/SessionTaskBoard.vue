<!--
# SessionTaskBoard.vue — 会话任务清单面板

类似 Claude Code 的任务面板，显示 CreateTask 创建的持久任务及其执行状态。
实时监听 agent-step 事件更新。
-->
<script setup lang="ts">
import { ref, computed, watch, onMounted, onBeforeUnmount } from 'vue';
import { invoke } from '@tauri-apps/api/core';
import { listen } from '@tauri-apps/api/event';
import type { UnlistenFn } from '@tauri-apps/api/event';
import { useSessionStore } from '../../stores/session';

interface TaskItem {
  id: number;
  subject: string;
  description: string;
  status: 'pending' | 'in_progress' | 'completed';
  blockedBy: number[];
  blocks: number[];
  activeForm?: string | null;
}

const session = useSessionStore();
const tasks = ref<TaskItem[]>([]);
const collapsed = ref(false);
let unlistenStep: UnlistenFn | null = null;

const visible = computed(() => tasks.value.length > 0);

const completedCount = computed(() => tasks.value.filter((t) => t.status === 'completed').length);

const progressPercent = computed(() => {
  if (!tasks.value.length) return 0;
  return Math.round((completedCount.value / tasks.value.length) * 100);
});

const statusClass = (status: string): string => `task-${status}`;

const taskLabel = (task: TaskItem): string => {
  if (task.status === 'in_progress') return task.activeForm || task.subject;
  return task.subject;
};

const loadTasks = async () => {
  const sid = session.activeSessionId;
  if (!sid) { tasks.value = []; return; }
  try {
    tasks.value = await invoke<TaskItem[]>('get_session_tasks', { sessionId: sid });
  } catch {
    tasks.value = [];
  }
};

watch(() => session.activeSessionId, () => { loadTasks(); });

onMounted(async () => {
  await loadTasks();

  unlistenStep = await listen<any>('agent-step', (event) => {
    if (event.payload?.isSubAgent) return;
    const sid = event.payload?.sessionId;
    if (sid && sid !== session.activeSessionId) return;

    const { type, taskId, status, subject } = event.payload;
    if (!taskId) return;

    const task = tasks.value.find((t) => t.id === taskId);
    if (!task) {
      if (type === 'task_scheduled') {
        tasks.value.push({
          id: taskId,
          subject: subject || `Task #${taskId}`,
          description: '',
          status: 'in_progress',
          blockedBy: [],
          blocks: [],
          activeForm: subject || undefined,
        });
      }
      return;
    }

    if (type === 'task_scheduled') {
      task.status = 'in_progress';
      if (subject) task.activeForm = subject;
    } else if (type === 'task_completed') {
      task.status = status === '完成' ? 'completed' : 'in_progress';
      if (task.status === 'completed') task.activeForm = null;
    }
  });
});

onBeforeUnmount(() => {
  unlistenStep?.();
});
</script>

<template>
  <div v-if="visible" class="session-task-board" :class="{ collapsed }">
    <button class="task-board-header" @click="collapsed = !collapsed">
      <svg viewBox="0 0 24 24" width="14" height="14" stroke="currentColor" stroke-width="2" fill="none" stroke-linecap="round" stroke-linejoin="round">
        <path d="M9 11l3 3L22 4"></path>
        <path d="M21 12v7a2 2 0 0 1-2 2H5a2 2 0 0 1-2-2V5a2 2 0 0 1 2-2h11"></path>
      </svg>
      <span class="task-board-title">TASKS</span>
      <span class="task-board-progress">{{ progressPercent }}%</span>
      <span class="task-board-counts">
        {{ completedCount }}/{{ tasks.length }}
      </span>
      <svg class="task-board-chevron" viewBox="0 0 24 24" width="12" height="12" stroke="currentColor" stroke-width="2" fill="none"><polyline points="6 9 12 15 18 9"></polyline></svg>
    </button>

    <div v-if="!collapsed" class="task-board-body">
      <!-- 进度条 -->
      <div class="task-progress-bar">
        <div class="task-progress-fill" :style="{ width: progressPercent + '%' }"></div>
      </div>

      <div
        v-for="task in tasks"
        :key="task.id"
        class="task-board-item"
        :class="statusClass(task.status)"
      >
        <span class="task-status-icon">
          <svg v-if="task.status === 'completed'" viewBox="0 0 24 24" width="12" height="12" stroke="currentColor" stroke-width="2" fill="none"><polyline points="20 6 9 17 4 12"></polyline></svg>
          <svg v-else-if="task.status === 'in_progress'" class="task-spinner" viewBox="0 0 24 24" width="12" height="12" fill="none" stroke="currentColor" stroke-width="2"><circle cx="12" cy="12" r="10"></circle><path d="M12 2a10 10 0 0 1 10 10"></path></svg>
          <span v-else class="task-pending-dot"></span>
        </span>
        <span class="task-item-label">{{ taskLabel(task) }}</span>
        <span v-if="task.blockedBy.length" class="task-dep-badge" :title="'依赖: ' + task.blockedBy.join(', ')">
          {{ task.blockedBy.length }}
        </span>
      </div>
    </div>
  </div>
</template>

<style scoped>
.session-task-board {
  margin: 0 8px 8px;
  background: var(--surface-strong);
  border: 1px solid var(--glass-border-subtle);
  border-radius: 10px;
  overflow: hidden;
  max-width: 420px;
}

.session-task-board.collapsed .task-board-chevron {
  transform: rotate(-90deg);
}

.task-board-header {
  display: flex;
  align-items: center;
  gap: 6px;
  width: 100%;
  padding: 8px 10px;
  border: none;
  background: transparent;
  color: var(--text-muted);
  cursor: pointer;
  font-size: 0.65rem;
  font-weight: 850;
  letter-spacing: 0.06em;
  text-transform: uppercase;
}

.task-board-title {
  color: var(--text-muted);
}

.task-board-progress {
  margin-left: auto;
  color: var(--accent-blue);
  font-variant-numeric: tabular-nums;
}

.task-board-counts {
  color: var(--text-muted);
  font-variant-numeric: tabular-nums;
  opacity: 0.6;
}

.task-board-chevron {
  flex-shrink: 0;
  opacity: 0.5;
  transition: transform 0.2s;
}

.task-board-body {
  padding: 0 10px 8px;
}

.task-progress-bar {
  height: 3px;
  background: var(--glass-bg-light);
  border-radius: 2px;
  margin-bottom: 8px;
  overflow: hidden;
}

.task-progress-fill {
  height: 100%;
  background: var(--accent-blue);
  border-radius: 2px;
  transition: width 0.3s ease;
}

.task-board-item {
  display: flex;
  align-items: center;
  gap: 8px;
  padding: 4px 6px;
  border-radius: 4px;
  font-size: 0.7rem;
  line-height: 1.35;
}

.task-board-item.task-pending {
  color: var(--text-muted);
  opacity: 0.65;
}

.task-board-item.task-in_progress {
  color: var(--text-main);
  background: color-mix(in srgb, var(--accent-blue) 6%, transparent);
}

.task-board-item.task-completed {
  color: var(--accent-green);
}

.task-status-icon {
  flex-shrink: 0;
  width: 16px;
  height: 16px;
  display: flex;
  align-items: center;
  justify-content: center;
}

.task-spinner {
  animation: task-spin 1.5s linear infinite;
  color: var(--accent-blue);
}

@keyframes task-spin {
  to { transform: rotate(360deg); }
}

.task-pending-dot {
  width: 6px;
  height: 6px;
  border-radius: 50%;
  background: var(--text-muted);
  opacity: 0.4;
}

.task-item-label {
  flex: 1;
  min-width: 0;
  overflow: hidden;
  text-overflow: ellipsis;
  white-space: nowrap;
}

.task-dep-badge {
  flex-shrink: 0;
  padding: 0 4px;
  border-radius: 3px;
  background: color-mix(in srgb, var(--text-muted) 12%, transparent);
  color: var(--text-muted);
  font-size: 0.55rem;
  font-weight: 700;
}
</style>
