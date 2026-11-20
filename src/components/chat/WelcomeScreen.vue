<!--
# WelcomeScreen.vue — 空会话欢迎页

反应环标志（双环反向旋转 + 呼吸核心）+ JARVIS 字标（带终端光标闪烁）+ 就绪提示。
仅一次淡入动画；环/核心/光标持续微动效，prefers-reduced-motion 下降级为静态。

## Constraints
- 整体无交互（pointer-events: none），仅快捷指令按钮自身恢复可点
- 反应环为纯 SVG + CSS 动画，无新依赖
- ① 字标/光标/标志主色参数在此微调
-->
<script setup lang="ts">
import { computed, onBeforeUnmount, onMounted, ref } from 'vue'
import { useI18n } from 'vue-i18n'
import { useSessionStore } from '../../stores/session'

const { t, tm } = useI18n()
const session = useSessionStore()

/** 快捷指令：渐隐轮换展示，点击即填入输入框（草稿由 TerminalInput 消费后清空） */
const hints = computed(() => tm('welcome.hints') as string[])
const hintIndex = ref(0)
const hintPaused = ref(false)
let hintTimer: ReturnType<typeof setInterval> | null = null

onMounted(() => {
  hintTimer = setInterval(() => {
    if (hintPaused.value || hints.value.length < 2) return
    hintIndex.value = (hintIndex.value + 1) % hints.value.length
  }, 4500)
})

onBeforeUnmount(() => {
  if (hintTimer) clearInterval(hintTimer)
})

function useHint() {
  const hint = hints.value[hintIndex.value]
  if (hint) session.inputDraft = hint
}
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
    <button
      v-if="hints.length"
      class="welcome-hint"
      type="button"
      :title="t('welcome.hintPrefix')"
      @click="useHint"
      @mouseenter="hintPaused = true"
      @mouseleave="hintPaused = false"
    >
      <span class="welcome-hint-prefix">{{ t('welcome.hintPrefix') }}</span>
      <Transition name="hint-fade" mode="out-in">
        <span :key="hintIndex" class="welcome-hint-text">{{ hints[hintIndex] }}</span>
      </Transition>
    </button>
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
  font-size: 0.9333rem;
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
  font-size: 0.8667rem;
  margin-top: 8px;
}

/* 快捷指令：比就绪提示再弱一档，hover 时才"浮"出来邀请点击。
   容器是 pointer-events:none，这里单独恢复。 */
.welcome-hint {
  pointer-events: auto;
  display: inline-flex;
  align-items: baseline;
  gap: 6px;
  margin-top: 14px;
  padding: 4px 12px;
  background: transparent;
  border: 1px solid transparent;
  border-radius: var(--radius-md);
  color: var(--text-faint);
  font-size: 0.8rem;
  font-family: inherit;
  cursor: pointer;
  transition: all var(--transition-fast);
}

.welcome-hint:hover {
  background: var(--glass-bg-light);
  border-color: var(--glass-border-subtle);
  color: var(--text-muted);
}

.welcome-hint-prefix {
  flex-shrink: 0;
  color: var(--accent-blue);
  opacity: 0.75;
}

.welcome-hint-prefix::after {
  content: "：";
}

.welcome-hint-text {
  overflow: hidden;
  text-overflow: ellipsis;
  white-space: nowrap;
  max-width: 320px;
}

.hint-fade-enter-active,
.hint-fade-leave-active {
  transition: opacity 280ms ease, transform 280ms ease;
}

.hint-fade-enter-from {
  opacity: 0;
  transform: translateY(4px);
}

.hint-fade-leave-to {
  opacity: 0;
  transform: translateY(-4px);
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
