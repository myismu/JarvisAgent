<script setup lang="ts">
import { computed, ref } from 'vue';
import { useAgentStore } from '../../stores/agent';
import type { TodoItem } from '../../types';

const agent = useAgentStore();
const expanded = ref(false);

const todos = computed(() => agent.currentTodos);
const visible = computed(() => todos.value.length > 0);
const done = computed(() => todos.value.filter((t) => t.status === 'completed').length);
const total = computed(() => todos.value.length);
const inProgress = computed(() => todos.value.find((t) => t.status === 'in_progress'));

const todoLabel = (todo: TodoItem): string =>
  todo.status === 'in_progress'
    ? (todo.activeForm || todo.content || todo.text || '')
    : (todo.content || todo.text || '');
</script>

<template>
  <Transition name="todo-fade">
    <div v-if="visible" class="todo-bar">
      <div class="todo-bar-inner" @click="expanded = !expanded">
        <div class="todo-bar-main">
          <span class="todo-dots">
            <span v-for="t in todos" :key="t.id" class="todo-dot" :class="'dot-' + t.status"></span>
          </span>
          <span class="todo-count">{{ done }}/{{ total }}</span>
          <span v-if="inProgress" class="todo-current">{{ todoLabel(inProgress) }}</span>
          <svg viewBox="0 0 24 24" width="10" height="10" stroke="currentColor" stroke-width="2.5" fill="none" class="todo-chevron" :class="{ open: expanded }">
            <polyline points="9 18 15 12 9 6"></polyline>
          </svg>
        </div>
      </div>
      <Transition name="todo-drop">
        <div v-if="expanded" class="todo-dropdown" @click.stop>
          <div v-for="todo in todos" :key="todo.id" :class="['todo-item', 'item-' + todo.status]">
            <span class="todo-dot" :class="'dot-' + todo.status"></span>
            <span class="todo-text">{{ todoLabel(todo) }}</span>
          </div>
        </div>
      </Transition>
    </div>
  </Transition>
</template>

<style scoped>
.todo-bar {
  position: sticky;
  top: 8px;
  z-index: 10;
  align-self: flex-start;
  margin: 0 0 12px 40px;
  max-width: 320px;
}
.todo-bar-inner {
  padding: 6px 14px;
  border-radius: 20px;
  background: var(--glass-bg-heavy);
  backdrop-filter: blur(12px);
  -webkit-backdrop-filter: blur(12px);
  border: 1px solid var(--glass-border);
  box-shadow: var(--shadow-sm);
  cursor: pointer;
  user-select: none;
  transition: all var(--transition-fast);
  position: relative;
}
.todo-bar-inner:hover {
  border-color: var(--glass-border);
  background: var(--glass-bg-heavy);
}

.todo-bar-main {
  display: flex;
  align-items: center;
  gap: 8px;
  font-size: 0.72rem;
  color: var(--text-muted);
}

.todo-dots {
  display: flex;
  gap: 3px;
}
.todo-dot {
  width: 7px;
  height: 7px;
  border-radius: 50%;
  flex-shrink: 0;
}
.dot-pending { background: var(--text-muted); opacity: 0.35; }
.dot-in_progress { background: var(--accent-yellow); box-shadow: 0 0 4px var(--accent-yellow); }
.dot-completed { background: var(--accent-green); }

.todo-count {
  font-weight: 700;
  font-variant-numeric: tabular-nums;
  color: var(--text-main);
}

.todo-current {
  flex: 1;
  overflow: hidden;
  text-overflow: ellipsis;
  white-space: nowrap;
  color: var(--accent-yellow);
}

.todo-chevron {
  flex-shrink: 0;
  transition: transform 0.2s;
}
.todo-chevron.open { transform: rotate(90deg); }

.todo-dropdown {
  position: absolute;
  top: 100%;
  left: 0;
  margin-top: 4px;
  min-width: 220px;
  padding: 8px 12px;
  border-radius: 12px;
  background: var(--glass-bg-heavy);
  backdrop-filter: blur(16px);
  -webkit-backdrop-filter: blur(16px);
  border: 1px solid var(--glass-border);
  box-shadow: var(--shadow-lg);
  display: flex;
  flex-direction: column;
  gap: 4px;
}

.todo-drop-enter-active,
.todo-drop-leave-active { transition: opacity 0.15s, transform 0.15s; }
.todo-drop-enter-from,
.todo-drop-leave-to { opacity: 0; transform: translateY(-4px); }
.todo-item {
  display: flex;
  align-items: center;
  gap: 8px;
  font-size: 0.7rem;
  padding: 2px 0;
}
.item-pending .todo-text { color: var(--text-muted); opacity: 0.6; }
.item-in_progress .todo-text { color: var(--accent-yellow); font-weight: 600; }
.item-completed .todo-text { color: var(--text-muted); text-decoration: line-through; opacity: 0.5; }

.todo-fade-enter-active,
.todo-fade-leave-active { transition: opacity 0.2s, transform 0.2s; }
.todo-fade-enter-from,
.todo-fade-leave-to { opacity: 0; transform: translateY(-6px); }
</style>
