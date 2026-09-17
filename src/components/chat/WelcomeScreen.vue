<!--
# WelcomeScreen.vue — 空会话欢迎页

反应环标志（双环反向旋转 + 呼吸核心）+ JARVIS 字标（带终端光标闪烁）+ 就绪提示。
仅一次淡入动画；环/核心/光标持续微动效，prefers-reduced-motion 下降级为静态。

## Constraints
- 纯展示，无交互（pointer-events: none）
- 反应环为纯 SVG + CSS 动画，无新依赖
- ① 字标/光标/标志主色参数在此微调
-->
<script setup lang="ts">
import { useI18n } from 'vue-i18n'

const { t } = useI18n()
</script>

<template>
  <div class="welcome-screen">
    <div class="welcome-mark">
      <svg class="ring ring-outer" viewBox="0 0 88 88" fill="none" xmlns="http://www.w3.org/2000/svg" aria-hidden="true">
        <circle cx="44" cy="44" r="42" stroke="var(--accent-blue)" stroke-width="1.5" stroke-dasharray="10 6" opacity="0.5" />
      </svg>
      <svg class="ring ring-inner" viewBox="0 0 88 88" fill="none" xmlns="http://www.w3.org/2000/svg" aria-hidden="true">
        <circle cx="44" cy="44" r="30" stroke="var(--accent-blue)" stroke-width="1.5" stroke-dasharray="52 137" stroke-linecap="round" />
        <circle cx="44" cy="7" r="2.5" fill="var(--accent-blue)" />
      </svg>
      <div class="core">
        <div class="core-dot" />
      </div>
    </div>
    <div class="welcome-title">JARVIS<span class="cursor">_</span></div>
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
  gap: 6px;
  pointer-events: none;
  user-select: none;
  animation: welcomeFadeIn 420ms ease-out;
}

.welcome-mark {
  position: relative;
  width: 88px;
  height: 88px;
  margin-bottom: 10px;
  animation: markFadeIn 520ms ease-out both;
}

.welcome-mark .ring {
  position: absolute;
  inset: 0;
  width: 88px;
  height: 88px;
  display: block;
}

.ring-outer {
  animation: spinCW 14s linear infinite;
}

.ring-inner {
  animation: spinCCW 9s linear infinite;
}

.core {
  position: absolute;
  inset: 0;
  display: flex;
  align-items: center;
  justify-content: center;
}

.core-dot {
  width: 10px;
  height: 10px;
  border-radius: 50%;
  background: var(--accent-blue);
  /* ① 核心呼吸节奏 */
  animation: pulseDot 2.4s ease-in-out infinite;
}

.welcome-title {
  /* ① 字标：更小更疏，更贵气 */
  font-size: 14px;
  font-weight: 600;
  letter-spacing: 7px;
  color: var(--text-main);
}

/* ① 终端光标闪烁节奏 */
.cursor {
  margin-left: 2px;
  color: var(--accent-blue);
  animation: blink 1s step-end infinite;
}

.welcome-text {
  color: var(--text-muted);
  font-size: 13px;
  margin-top: 8px;
}

@keyframes welcomeFadeIn {
  from { opacity: 0; transform: translateY(6px); }
  to { opacity: 1; transform: translateY(0); }
}

@keyframes markFadeIn {
  from { opacity: 0; transform: scale(0.92); }
  to { opacity: 1; transform: scale(1); }
}

@keyframes spinCW { to { transform: rotate(360deg); } }
@keyframes spinCCW { to { transform: rotate(-360deg); } }

@keyframes blink {
  0%, 100% { opacity: 1; }
  50% { opacity: 0; }
}

@keyframes pulseDot {
  0%, 100% { opacity: 1; }
  50% { opacity: 0.35; }
}

@media (prefers-reduced-motion: reduce) {
  .welcome-screen,
  .welcome-mark,
  .ring-outer,
  .ring-inner,
  .core-dot,
  .cursor { animation: none; }
}
</style>
