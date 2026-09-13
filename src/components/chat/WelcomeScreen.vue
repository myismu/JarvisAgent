<!--
# WelcomeScreen.vue — 空会话欢迎页

极简品牌标识（蓝色描边图形 + JARVIS 字标）与就绪提示，仅一次淡入动画。

## Constraints
- 纯展示，无交互（pointer-events: none）
-->
<script setup lang="ts">
import { useI18n } from 'vue-i18n'

const { t } = useI18n()
</script>

<template>
  <div class="welcome-screen">
    <div class="welcome-mark">
      <svg viewBox="0 0 32 32" fill="none" xmlns="http://www.w3.org/2000/svg" aria-hidden="true">
        <rect x="1.5" y="1.5" width="29" height="29" rx="8"
          stroke="var(--accent-blue)" stroke-width="1.5" />
        <path d="M18.5 7 L11 17.5 H15.5 L13.5 25 L21 14.5 H16.5 Z"
          fill="var(--accent-blue)" />
      </svg>
    </div>
    <div class="welcome-title">JARVIS</div>
    <div class="welcome-text">{{ t('welcome.ready') }}</div>
  </div>
</template>

<style scoped>
.welcome-screen {
  display: flex;
  flex-direction: column;
  align-items: center;
  justify-content: center;
  height: 100%;
  min-height: 400px;
  gap: 10px;
  pointer-events: none;
  user-select: none;
  animation: welcomeFadeIn 400ms ease-out;
}

.welcome-mark svg {
  width: 44px;
  height: 44px;
  display: block;
}

.welcome-title {
  font-size: 15px;
  font-weight: 600;
  letter-spacing: 4px;
  color: var(--text-main);
}

.welcome-text {
  color: var(--text-muted);
  font-size: 13px;
}

@keyframes welcomeFadeIn {
  from { opacity: 0; transform: translateY(6px); }
  to { opacity: 1; transform: translateY(0); }
}

@media (prefers-reduced-motion: reduce) {
  .welcome-screen { animation: none; }
}
</style>
