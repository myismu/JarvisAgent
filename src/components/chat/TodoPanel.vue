<!--
# TodoPanel.vue — 会话任务清单面板

显示当前会话的待办事项：进度圆点、当前任务摘要与下拉明细。

## Dependencies
- Internal: `../../stores/agent`

## Constraints
- 状态色：completed=绿，in_progress=中性灰高亮文字
-->
<script setup lang="ts">
import { computed, ref } from 'vue';
import { useAgentStore } from '../../stores/agent';
import type { TodoGroup, TodoItem } from '../../types';

const agent = useAgentStore();
const expanded = ref(false);
// 已完成组默认折叠；单独记录展开状态，避免用户点开后又被收回去
const expandedGroups = ref<Record<string, boolean>>({});

const groups = computed(() => agent.currentTodoGroups);
const todos = computed(() => agent.currentTodos);
const visible = computed(() => todos.value.length > 0);
const done = computed(() => todos.value.filter((t) => t.status === 'completed').length);
const total = computed(() => todos.value.length);
const inProgress = computed(() => todos.value.find((t) => t.status === 'in_progress'));

const groupDoneCount = (group: TodoGroup) =>
  group.items.filter((t) => t.status === 'completed').length;

/** 一组全部完成即视为「已归档」：折叠成一行，避免长会话里清单无限堆叠 */
const isGroupDone = (group: TodoGroup) =>
  group.items.length > 0 && groupDoneCount(group) === group.items.length;

const toggleGroup = (id: string) => {
  expandedGroups.value = { ...expandedGroups.value, [id]: !expandedGroups.value[id] };
};

const todoLabel = (todo: TodoItem): string =>
  todo.status === 'in_progress'
    ? (todo.activeForm || todo.content || todo.text || '')
    : (todo.content || todo.text || '');

/** 调度器回写的子任务进度，如「12/34」 */
const progressText = computed(() => {
  const item = inProgress.value;
  if (!item?.progress || item.progress.total <= 0) return '';
  return `${item.progress.done}/${item.progress.total}`;
});
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
          <span v-if="inProgress" class="todo-current">
            {{ todoLabel(inProgress) }}
            <span v-if="progressText" class="todo-progress">{{ progressText }}</span>
          </span>
          <svg viewBox="0 0 24 24" width="10" height="10" stroke="currentColor" stroke-width="2.5" fill="none" class="todo-chevron" :class="{ open: expanded }">
            <polyline points="9 18 15 12 9 6"></polyline>
          </svg>
        </div>
      </div>
      <Transition name="todo-drop">
        <div v-if="expanded" class="todo-dropdown" @click.stop>
          <div v-for="(group, gi) in groups" :key="group.id" class="todo-group">
            <div
              v-if="groups.length > 1 || isGroupDone(group)"
              class="todo-group-head"
              @click.stop="toggleGroup(group.id)"
            >
              <span class="todo-group-title">清单 {{ gi + 1 }}</span>
              <span class="todo-group-summary">
                {{ isGroupDone(group)
                  ? `已完成 · ${group.items.length} 项`
                  : `${groupDoneCount(group)}/${group.items.length}` }}
              </span>
              <svg
                v-if="isGroupDone(group)"
                viewBox="0 0 24 24"
                width="9"
                height="9"
                stroke="currentColor"
                stroke-width="2.5"
                fill="none"
                class="todo-group-chevron"
                :class="{ open: expandedGroups[group.id] }"
              >
                <polyline points="9 18 15 12 9 6"></polyline>
              </svg>
            </div>
            <template v-if="!isGroupDone(group) || expandedGroups[group.id]">
              <div
                v-for="(todo, ti) in group.items"
                :key="group.id + '-' + ti"
                :class="['todo-item', 'item-' + todo.status]"
              >
                <span class="todo-dot" :class="'dot-' + todo.status"></span>
                <span class="todo-text">{{ todoLabel(todo) }}</span>
                <span v-if="todo.progress && todo.progress.total > 0" class="todo-item-progress">
                  {{ todo.progress.done }}/{{ todo.progress.total }}
                </span>
              </div>
            </template>
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
.dot-in_progress { background: var(--text-muted); }
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
  color: var(--text-main);
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
  border: 1px solid var(--glass-border);
  box-shadow: var(--shadow-lg);
  display: flex;
  flex-direction: column;
  gap: 4px;
  /* 待办条目数不受控（v-for 全量渲染）：封顶 + 内部滚动，
     避免条目一多就把下拉撑得比窗口还高。与 .profile-menu 同一处理口径。 */
  max-height: min(320px, 50vh);
  overflow-y: auto;
  overflow-x: hidden;
  overscroll-behavior: contain;
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
.item-in_progress .todo-text { color: var(--text-main); font-weight: 600; }
.item-completed .todo-text { color: var(--text-muted); text-decoration: line-through; opacity: 0.85; }

/* 调度进度与分组（长会话多轮清单共存） */
.todo-progress {
  color: var(--text-muted);
  font-variant-numeric: tabular-nums;
  margin-left: 4px;
}
.todo-group + .todo-group {
  margin-top: 6px;
  padding-top: 6px;
  border-top: 1px solid var(--glass-border);
}
.todo-group-head {
  display: flex;
  align-items: center;
  gap: 6px;
  padding: 2px 0 4px;
  font-size: 0.68rem;
  color: var(--text-muted);
  cursor: pointer;
  user-select: none;
}
.todo-group-title { font-weight: 600; }
.todo-group-summary { margin-left: auto; font-variant-numeric: tabular-nums; }
.todo-group-chevron { transition: transform 0.2s; flex-shrink: 0; }
.todo-group-chevron.open { transform: rotate(90deg); }
.todo-item-progress {
  margin-left: auto;
  color: var(--text-muted);
  font-variant-numeric: tabular-nums;
}

.todo-fade-enter-active,
.todo-fade-leave-active { transition: opacity 0.2s, transform 0.2s; }
.todo-fade-enter-from,
.todo-fade-leave-to { opacity: 0; transform: translateY(-6px); }
</style>
