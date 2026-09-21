<!--
# ToastHost.vue — 全局轻提示的渲染宿主

把 `useToast` 的队列渲染成顶部居中的悬浮气泡（`Teleport` 到 body，浮在所有界面之上），
淡入 → 停留 → 淡出，点一下立即关掉。全应用只需要挂一次（见 `App.vue`）。

## 视觉规则（与项目玻璃体系的关系）
- 底子是 `--glass-bg-heavy` / `--glass-border` / `--glass-blur`，与 `ConfirmModal` 同一套；
- **但海拔比面板高一档**：`--glass-shadow` 是 6% 黑（浅色下几乎不可见），面板够用，
  而通知层要飘在任何背景之上都能读出来，所以额外叠一层更实的投影；
- 图标定语气色（蓝=完成 / 红=失败），底部细线是**倒计时**（按 `duration` 缩到 0），
  让"还有多久消失"可见，而不是突然不见。

## Key Exports
- `ToastHost`: 渲染气泡队列的组件（无 props / 无事件）

## Dependencies
- Internal: `../../composables/useToast`
- External: `vue`（`Teleport` / `TransitionGroup`）

## Constraints
- 必须挂在 App 根组件里，且只挂一次
- `info` 一档目前承载的全是"操作已完成"类反馈（已重命名 / 已恢复 / 已删除），
  所以用绿色对勾；将来若出现真正中性的提示，另开一档而不是改这一档的色
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
        :role="item.kind === 'error' ? 'alert' : 'status'"
        :title="item.message"
        @click="dismissToast(item.id)"
      >
        <!-- 两个独立 svg 各自 v-if：不在 svg 内部用 <template v-if>（那样子节点会丢 SVG 命名空间） -->
        <span class="toast-icon" aria-hidden="true">
          <svg
            v-if="item.kind === 'error'"
            viewBox="0 0 24 24"
            width="16"
            height="16"
            fill="none"
            stroke="currentColor"
            stroke-width="2"
            stroke-linecap="round"
            stroke-linejoin="round"
          >
            <circle cx="12" cy="12" r="9.5" />
            <line x1="12" y1="7.5" x2="12" y2="13" />
            <line x1="12" y1="16.5" x2="12.01" y2="16.5" />
          </svg>
          <svg
            v-else
            viewBox="0 0 24 24"
            width="16"
            height="16"
            fill="none"
            stroke="currentColor"
            stroke-width="2"
            stroke-linecap="round"
            stroke-linejoin="round"
          >
            <circle cx="12" cy="12" r="9.5" />
            <polyline points="8 12.2 11 15.2 16 9.6" />
          </svg>
        </span>
        <span class="toast-text">{{ item.message }}</span>
        <!-- 倒计时细线：时长由 JS 侧给（info 3.5s / error 5s），这里只负责画 -->
        <span
          class="toast-progress"
          :style="{ animationDuration: `${item.duration}ms` }"
          aria-hidden="true"
        />
      </div>
    </TransitionGroup>
  </Teleport>
</template>

<style scoped>
.toast-host {
  position: fixed;
  top: 14px;
  left: 0;
  right: 0;
  z-index: 2000;
  display: flex;
  flex-direction: column;
  align-items: center;
  gap: 8px;
  /* 容器不吃点击：气泡飘着的时候，下面的界面照常能操作 */
  pointer-events: none;
}

.toast-item {
  position: relative;
  overflow: hidden;
  pointer-events: auto;
  display: flex;
  align-items: flex-start;
  gap: 9px;
  max-width: min(420px, calc(100vw - 32px));
  padding: 10px 14px 11px 12px;
  border-radius: var(--radius-lg);
  font-size: 13px;
  line-height: 1.5;
  color: var(--text-main);
  background: var(--glass-bg-heavy);
  border: 1px solid var(--glass-border);
  backdrop-filter: blur(var(--glass-blur-heavy));
  -webkit-backdrop-filter: blur(var(--glass-blur-heavy));
  /* 海拔比面板高一档：基础玻璃阴影 + 一层更实的投影 */
  box-shadow: var(--glass-shadow), 0 10px 30px rgba(0, 0, 0, 0.12);
  cursor: pointer;
  user-select: none;
}

/* info 一档目前全是"操作已完成"（见文件头说明）→ 绿勾；失败 → 红圈感叹号 */
.toast-item.info .toast-icon {
  color: var(--accent-green);
}
.toast-item.error .toast-icon {
  color: var(--accent-red);
}
.toast-item.error {
  border-color: color-mix(in srgb, var(--accent-red) 34%, var(--glass-border));
}

.toast-icon {
  flex-shrink: 0;
  display: inline-flex;
  margin-top: 1px;
}
.toast-text {
  flex: 1;
  min-width: 0;
  word-break: break-word;
}

/* 倒计时：左对齐缩到 0，行高 2px，不抢文字的视觉权重 */
.toast-progress {
  position: absolute;
  left: 0;
  right: 0;
  bottom: 0;
  height: 2px;
  transform-origin: left center;
  opacity: 0.45;
  animation-name: toast-countdown;
  animation-timing-function: linear;
  animation-fill-mode: forwards;
}
.toast-item.info .toast-progress {
  background: var(--accent-green);
}
.toast-item.error .toast-progress {
  background: var(--accent-red);
}
@keyframes toast-countdown {
  from {
    transform: scaleX(1);
  }
  to {
    transform: scaleX(0);
  }
}

/* 入场：轻微下移 + 缩放（有"落下来"的实感，不是硬闪）；
   出场：更快、幅度更小（消失不需要被注意） */
.toast-enter-active {
  transition:
    opacity 0.2s ease,
    transform 0.22s cubic-bezier(0.22, 1, 0.36, 1);
}
.toast-leave-active {
  transition:
    opacity 0.15s ease,
    transform 0.15s ease;
  /* 脱离文档流：后面的气泡平滑补位，否则会跳一下 */
  position: absolute;
}
.toast-enter-from {
  opacity: 0;
  transform: translateY(-8px) scale(0.97);
}
.toast-leave-to {
  opacity: 0;
  transform: translateY(-4px) scale(0.98);
}
</style>
