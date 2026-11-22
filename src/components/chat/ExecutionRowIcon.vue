<!--
# ExecutionRowIcon.vue — 执行过程行首小图标

纯装饰性图标：14px、线性描边、currentColor，跟随所在行的 muted 灰，
用于快速区分思考 / 搜索 / 读取 / 写入 / 命令 / 计划 / 子代理 / 任务 / 记忆 / 系统。

## Constraints
- 只做视觉识别，不承载状态语义（状态由右侧文字承担）。
- 图标集合刻意保持简单，不引入图标库依赖。
-->
<script lang="ts">
export type ExecutionRowIconKind =
  | "thinking"
  | "search"
  | "file"
  | "edit"
  | "command"
  | "plan"
  | "subagent"
  | "task"
  | "memory"
  | "system"
  | "other"
  /** 行 hover / 展开时替代类型图标的折叠箭头 */
  | "chevron";
</script>

<script setup lang="ts">
defineProps<{ kind: ExecutionRowIconKind }>();
</script>

<template>
  <svg
    class="execution-row-icon"
    viewBox="0 0 24 24"
    width="14"
    height="14"
    fill="none"
    stroke="currentColor"
    stroke-width="1.6"
    stroke-linecap="round"
    stroke-linejoin="round"
    aria-hidden="true"
  >
    <template v-if="kind === 'thinking'">
      <path d="M12 3l1.7 5.3L19 10l-5.3 1.7L12 17l-1.7-5.3L5 10l5.3-1.7z" />
    </template>
    <template v-else-if="kind === 'search'">
      <circle cx="11" cy="11" r="7" />
      <line x1="16.5" y1="16.5" x2="21" y2="21" />
    </template>
    <template v-else-if="kind === 'file'">
      <path d="M7 3h7l4 4v14H7z" />
      <path d="M14 3v5h5" />
      <line x1="10" y1="13" x2="15" y2="13" />
      <line x1="10" y1="17" x2="15" y2="17" />
    </template>
    <template v-else-if="kind === 'edit'">
      <path d="M4 20l4-1L19 8l-3-3L5 16l-1 4z" />
      <line x1="14" y1="5" x2="17" y2="8" />
    </template>
    <template v-else-if="kind === 'command'">
      <polyline points="4 17 10 11 4 5" />
      <line x1="12" y1="19" x2="20" y2="19" />
    </template>
    <template v-else-if="kind === 'plan'">
      <path d="M9 4L3 6v14l6-2 6 2 6-2V4l-6 2-6-2z" />
      <line x1="9" y1="4" x2="9" y2="18" />
      <line x1="15" y1="6" x2="15" y2="20" />
    </template>
    <template v-else-if="kind === 'subagent'">
      <rect x="5" y="8" width="14" height="11" rx="2" />
      <line x1="12" y1="4" x2="12" y2="8" />
      <circle cx="12" cy="3" r="1" />
      <line x1="9" y1="12" x2="9" y2="13.5" />
      <line x1="15" y1="12" x2="15" y2="13.5" />
    </template>
    <template v-else-if="kind === 'task'">
      <rect x="4" y="4" width="16" height="16" rx="2" />
      <polyline points="8 12 11 15 16 9" />
    </template>
    <template v-else-if="kind === 'memory'">
      <ellipse cx="12" cy="5" rx="8" ry="3" />
      <path d="M4 5v14c0 1.7 3.6 3 8 3s8-1.3 8-3V5" />
      <path d="M4 12c0 1.7 3.6 3 8 3s8-1.3 8-3" />
    </template>
    <template v-else-if="kind === 'system'">
      <line x1="4" y1="6" x2="20" y2="6" />
      <line x1="4" y1="12" x2="20" y2="12" />
      <line x1="4" y1="18" x2="20" y2="18" />
      <circle cx="9" cy="6" r="2" />
      <circle cx="15" cy="12" r="2" />
      <circle cx="9" cy="18" r="2" />
    </template>
    <template v-else-if="kind === 'chevron'">
      <polyline points="6 9 12 15 18 9" />
    </template>
    <template v-else>
      <path d="M14.7 6.3a4.5 4.5 0 0 0-6.4 5.6L3 17.2V21h3.8l5.3-5.3a4.5 4.5 0 0 0 5.6-6.4l-2.9 2.9-2.6-2.6 2.5-2.5z" />
    </template>
  </svg>
</template>

<style scoped>
.execution-row-icon {
  flex-shrink: 0;
}
</style>
