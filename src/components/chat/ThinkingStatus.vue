<!--
# ThinkingStatus.vue — 思考中状态行

会话内联的思考状态：旋转 spinner 与计时，支持暂停态展示。

## Constraints
- 全部使用中性灰阶，不用彩色
-->
<script setup lang="ts">
import { computed } from 'vue';

const props = defineProps<{
  running: boolean;
  elapsed: number;
  paused: boolean;
}>();

const formattedTime = computed(() => {
  const m = Math.floor(props.elapsed / 60);
  const s = props.elapsed % 60;
  return m > 0 ? `${m}:${s.toString().padStart(2, '0')}` : `${s}s`;
});
</script>

<template>
  <div v-if="running" class="thinking-inline-status" :class="{ paused }" aria-label="Jarvis is thinking">
    <span class="thinking-spinner"></span>
    <span class="thinking-timer">{{ formattedTime }}</span>
    <span v-if="paused" class="thinking-paused-label">等待决策</span>
  </div>
</template>

<style scoped>
.thinking-inline-status {
  display: inline-flex;
  align-items: center;
  gap: 6px;
  font-size: 12px;
  color: var(--accent-blue);
  margin-top: 8px;
}
.thinking-inline-status.paused {
  color: var(--text-muted);
}
.thinking-inline-status.paused .thinking-spinner {
  border-color: var(--glass-border);
  border-top-color: var(--text-muted);
  animation-play-state: paused;
}
.thinking-paused-label {
  font-size: 11px;
  opacity: 0.8;
}
.thinking-spinner {
  width: 12px;
  height: 12px;
  border: 2px solid color-mix(in srgb, var(--accent-blue) 20%, transparent);
  border-top-color: var(--accent-blue);
  border-radius: 50%;
  animation: spin 1s linear infinite;
}
.thinking-timer {
  font-variant-numeric: tabular-nums;
}
@keyframes spin {
  to { transform: rotate(360deg); }
}
@keyframes fadeIn {
  from { opacity: 0; }
  to { opacity: 1; }
}
</style>
