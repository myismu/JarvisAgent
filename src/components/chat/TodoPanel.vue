<!--
# TodoPanel.vue — 任务清单面板

在会话区顶部浮动显示 Agent 的当前任务清单，状态实时更新。
类似 Claude Code 的任务列表 UI。
-->
<script setup lang="ts">
import { computed } from 'vue';
import { useAgentStore } from '../../stores/agent';
import type { TodoItem } from '../../types';

const agent = useAgentStore();

const todos = computed(() => agent.todos);
const visible = computed(() => todos.value.length > 0);

const pendingCount = computed(() => todos.value.filter((t) => t.status === 'pending').length);
const inProgressCount = computed(() => todos.value.filter((t) => t.status === 'in_progress').length);
const completedCount = computed(() => todos.value.filter((t) => t.status === 'completed').length);

const todoLabel = (todo: TodoItem): string => {
  return todo.status === 'in_progress'
    ? (todo.activeForm || todo.content || todo.text || '')
    : (todo.content || todo.text || '');
};

const statusClass = (status: string): string => `todo-${status}`;
</script>

<template>
  <Transition name="todo-panel-fade">
    <div v-if="visible" class="todo-panel">
      <div class="todo-header">
        <svg viewBox="0 0 24 24" width="14" height="14" stroke="currentColor" stroke-width="2" fill="none" stroke-linecap="round" stroke-linejoin="round">
          <path d="M9 11l3 3L22 4"></path>
          <path d="M21 12v7a2 2 0 0 1-2 2H5a2 2 0 0 1-2-2V5a2 2 0 0 1 2-2h11"></path>
        </svg>
        <span class="todo-title">TASKS</span>
        <span class="todo-counts">
          <span class="count-pending">{{ pendingCount }}</span>
          <span class="count-sep">/</span>
          <span class="count-progress">{{ inProgressCount }}</span>
          <span class="count-sep">/</span>
          <span class="count-done">{{ completedCount }}</span>
        </span>
      </div>
      <ul class="todo-items">
        <li v-for="todo in todos" :key="todo.id" :class="['todo-item', statusClass(todo.status)]">
          <span class="todo-dot"></span>
          <span class="todo-text">{{ todoLabel(todo) }}</span>
        </li>
      </ul>
    </div>
  </Transition>
</template>

<style scoped>
.todo-panel {
  margin: 0 8px 8px;
  padding: 8px 10px;
  background: color-mix(in srgb, var(--surface-strong) 50%, transparent);
  border: 1px solid color-mix(in srgb, var(--border-color) 60%, transparent);
  border-radius: 10px;
  max-width: 420px;
}

.todo-header {
  display: flex;
  align-items: center;
  gap: 6px;
  margin-bottom: 6px;
  color: var(--text-muted);
}

.todo-title {
  font-size: 0.6rem;
  font-weight: 850;
  letter-spacing: 0.08em;
  text-transform: uppercase;
}

.todo-counts {
  margin-left: auto;
  font-size: 0.55rem;
  font-weight: 800;
  font-variant-numeric: tabular-nums;
}

.count-pending { color: var(--text-muted); }
.count-progress { color: var(--accent-yellow); }
.count-done { color: var(--accent-green); }
.count-sep { color: var(--border-color); margin: 0 1px; }

.todo-items {
  list-style: none;
  margin: 0;
  padding: 0;
  display: flex;
  flex-direction: column;
  gap: 3px;
}

.todo-item {
  display: flex;
  align-items: center;
  gap: 6px;
  padding: 2px 4px;
  border-radius: 4px;
  font-size: 0.7rem;
  line-height: 1.35;
}

.todo-dot {
  width: 6px;
  height: 6px;
  flex-shrink: 0;
  border-radius: 999px;
}

.todo-pending .todo-dot {
  background: var(--text-muted);
  opacity: 0.5;
}

.todo-in_progress .todo-dot {
  background: var(--accent-yellow);
  box-shadow: 0 0 6px var(--accent-yellow);
}

.todo-completed .todo-dot {
  background: var(--accent-green);
}

.todo-pending .todo-text {
  color: var(--text-muted);
  opacity: 0.7;
}

.todo-in_progress .todo-text {
  color: var(--accent-yellow);
  font-weight: 650;
}

.todo-completed .todo-text {
  color: var(--accent-green);
  text-decoration: line-through;
  opacity: 0.7;
}

.todo-panel-fade-enter-active,
.todo-panel-fade-leave-active {
  transition: opacity 200ms ease, transform 200ms ease;
}

.todo-panel-fade-enter-from,
.todo-panel-fade-leave-to {
  opacity: 0;
  transform: translateY(-8px);
}
</style>
