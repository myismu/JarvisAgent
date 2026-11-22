<!--
# ThinkingStatus.vue — 思考中状态行

会话内联的思考状态：旋转 spinner 与计时，支持暂停态展示。

## Constraints
- 默认蓝色旋转器；等待决策态（权限请求 / 方案审批）换成琥珀色呼吸灯 ——
  此时真的没东西在转，拿旋转器表示「等你」是错的信号-->
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
    <span v-if="paused" class="thinking-light state-light is-waiting"></span>
    <span v-else class="thinking-spinner"></span>
    <span class="thinking-timer">{{ formattedTime }}</span>
    <span v-if="paused" class="thinking-paused-label">等待决策</span>
  </div>
</template>

<style scoped>
.thinking-inline-status {
  display: inline-flex;
  align-items: center;
  gap: 6px;
  font-size: 0.8rem;
  color: var(--accent-blue);
  margin-top: 8px;
}
.thinking-inline-status.paused {
  color: var(--text-warning);
}
/* 等待决策用带光晕的呼吸灯（全局 .state-light 的 is-waiting 态）。
   旋转器同时撤掉：此时是「它在等你」，不是「它在转」——
   让一个停住的 spinner 继续占位，反而像界面卡了。 */
.thinking-dot {
  --light-size: 8px;
}
.thinking-paused-label {
  font-size: 0.7333rem;
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
