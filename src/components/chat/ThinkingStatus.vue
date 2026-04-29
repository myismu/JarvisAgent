<script setup lang="ts">
import { computed } from 'vue';

const props = defineProps<{
  running: boolean;
  elapsed: number;
}>();

const formattedTime = computed(() => {
  const m = Math.floor(props.elapsed / 60);
  const s = props.elapsed % 60;
  return m > 0 ? `${m}:${s.toString().padStart(2, '0')}` : `${s}s`;
});
</script>

<template>
  <div v-if="running" class="thinking-inline-status" aria-label="Jarvis is thinking">
    <span class="thinking-spinner"></span>
    <span class="thinking-timer">{{ formattedTime }}</span>
  </div>
</template>

<style scoped>
.thinking-inline-status {
  display: inline-flex;
  align-items: center;
  gap: 6px;
  font-size: 12px;
  color: var(--accent-blue);
}
.thinking-spinner {
  width: 12px;
  height: 12px;
  border: 2px solid rgba(59, 130, 246, 0.2);
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
