<!--
# ToastHost.vue — 全局轻提示的渲染宿主

把 `useToast` 的队列渲染成顶部居中的悬浮气泡（`Teleport` 到 body，浮在所有界面之上），
出现时淡入、到点自动消失、点一下立即关掉。全应用只需要挂一次（见 `App.vue`）。

选顶部居中而不是右下角：右下角会压住聊天输入框，左侧会压住侧边栏，顶部中间是最空的
位置 —— 而且它浮在内容之上，**不参与任何布局**（页内提示条会把列表挤变形，见 `useToast`
的说明）。

## Key Exports
- `ToastHost`: 渲染气泡队列的组件（无 props / 无事件）

## Dependencies
- Internal: `../../composables/useToast`
- External: `vue`（`Teleport` / `TransitionGroup`）

## Constraints
- 必须挂在 App 根组件里，且只挂一次
-->
<script setup lang="ts">
import { dismissToast, toasts } from '../../composables/useToast';
</script>

<template>
  <Teleport to="body">
    <TransitionGroup name="toast" tag="div" class="toast-host">
      <div
        v-for="item in toasts"
        :key="item.id"
        class="toast-item"
        :class="item.kind"
        role="status"
        :title="item.message"
        @click="dismissToast(item.id)"
      >
        {{ item.message }}
      </div>
    </TransitionGroup>
  </Teleport>
</template>

<style scoped>
.toast-host {
  position: fixed;
  top: 16px;
  left: 0;
  right: 0;
  z-index: 2000;
  display: flex;
  flex-direction: column;
  align-items: center;
  gap: 8px;
  /* 容器不吃点击：只有气泡本身可点，下面的界面照常可操作 */
  pointer-events: none;
}
.toast-item {
  pointer-events: auto;
  max-width: min(420px, calc(100vw - 32px));
  padding: 8px 14px;
  border-radius: var(--radius-md);
  font-size: 13px;
  line-height: 1.45;
  color: var(--text-main);
  background: var(--glass-bg-heavy);
  border: 1px solid var(--glass-border);
  box-shadow: 0 6px 20px rgba(0, 0, 0, 0.22);
  backdrop-filter: blur(var(--glass-blur));
  cursor: pointer;
  word-break: break-word;
}
.toast-item.error {
  border-color: var(--accent-red);
  color: var(--accent-red);
}
.toast-enter-active,
.toast-leave-active {
  transition: opacity 0.18s ease, transform 0.18s ease;
}
.toast-enter-from,
.toast-leave-to {
  opacity: 0;
  transform: translateY(-6px);
}
/* 离开时脱离文档流，后面的气泡平滑补位（否则会跳一下） */
.toast-leave-active {
  position: absolute;
}
</style>
