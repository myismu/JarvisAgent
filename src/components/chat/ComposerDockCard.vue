<!--
# ComposerDockCard.vue — 输入区上方停靠交互卡统一骨架（Template Method 模板组件）

为 `UserQuestionCard`（运行期咨询）与 `PermissionCard`（权限审批）提供统一的卡片外壳、
与主输入框等宽的布局、极简标题栏（圆点 + 标题 + 计数 + 折叠按钮）、底部分割操作区与公共按钮样式。

## Key Exports
- 默认导出组件：`ComposerDockCard`
- Props：`visible` / `title` / `counter` / `tone` / `collapsible` / `collapsed` / `ariaLabel`
- Slots：默认插槽（主体内容）、`footer-left`（左侧次要操作）、`footer-right`（右侧主操作）

## Dependencies
- External: `vue`
-->
<script setup lang="ts">
const props = withDefaults(
  defineProps<{
    visible: boolean;
    title: string;
    counter?: string;
    tone?: 'neutral' | 'warning';
    collapsible?: boolean;
    collapsed?: boolean;
    ariaLabel?: string;
  }>(),
  {
    counter: '',
    tone: 'neutral',
    collapsible: true,
    collapsed: false,
    ariaLabel: '',
  },
);

const emit = defineEmits<{
  (e: 'update:collapsed', value: boolean): void;
}>();

function toggleCollapse() {
  if (!props.collapsible) return;
  emit('update:collapsed', !props.collapsed);
}
</script>

<template>
  <Transition name="dock-slide">
    <section
      v-if="visible"
      class="dock-card"
      :class="{ collapsed: collapsible && collapsed }"
      :aria-label="ariaLabel || title"
    >
      <header
        class="dock-header"
        :class="{ clickable: collapsible }"
        @click="toggleCollapse"
      >
        <div class="dock-header-left">
          <span class="dock-dot" :class="tone" aria-hidden="true"></span>
          <span class="dock-title">{{ title }}</span>
        </div>
        <div class="dock-header-right">
          <span v-if="counter" class="dock-counter">{{ counter }}</span>
          <button
            v-if="collapsible"
            type="button"
            class="dock-collapse-btn"
            :aria-expanded="!collapsed"
            :title="collapsed ? '展开' : '收起'"
            @click.stop="toggleCollapse"
          >
            <svg
              class="dock-chevron"
              :class="{ rotated: collapsed }"
              viewBox="0 0 24 24"
              width="14"
              height="14"
              fill="none"
              stroke="currentColor"
              stroke-width="2"
              stroke-linecap="round"
              stroke-linejoin="round"
              aria-hidden="true"
            >
              <polyline points="6 9 12 15 18 9"></polyline>
            </svg>
          </button>
        </div>
      </header>

      <div v-if="!collapsible || !collapsed" class="dock-collapsible">
        <div class="dock-body">
          <slot />
        </div>

        <footer v-if="$slots['footer-left'] || $slots['footer-right']" class="dock-footer">
          <div class="dock-footer-left">
            <slot name="footer-left" />
          </div>
          <div class="dock-footer-right">
            <slot name="footer-right" />
          </div>
        </footer>
      </div>
    </section>
  </Transition>
</template>

<style scoped>
.dock-card {
  width: 100%;
  max-width: min(85%, 960px); /* 与 TerminalInput(.chat-input-wrapper) 完全等宽 */
  margin: 0 auto 10px;
  box-sizing: border-box;
  padding: 12px 16px 14px;
  border-radius: 16px;
  background: var(--surface-strong, #fff);
  border: 1px solid var(--glass-border, rgba(0, 0, 0, 0.09));
  box-shadow: var(--shadow-md, 0 6px 20px rgba(0, 0, 0, 0.06));
  color: var(--text-main);
  transition: padding 0.16s ease, box-shadow 0.16s ease;
}

.dock-card.collapsed {
  padding: 10px 16px;
}

.dock-header {
  display: flex;
  align-items: center;
  justify-content: space-between;
  gap: 10px;
  user-select: none;
}

.dock-header.clickable {
  cursor: pointer;
}

.dock-header-left {
  display: flex;
  align-items: center;
  gap: 8px;
  min-width: 0;
}

.dock-dot {
  flex-shrink: 0;
  width: 6px;
  height: 6px;
  border-radius: 50%;
  background: var(--text-main);
  opacity: 0.72;
}

.dock-dot.warning {
  background: var(--text-warning, #d97706);
  opacity: 0.9;
}

.dock-title {
  font-size: 0.84rem;
  font-weight: 600;
  letter-spacing: -0.01em;
  white-space: nowrap;
  overflow: hidden;
  text-overflow: ellipsis;
}

.dock-header-right {
  display: flex;
  align-items: center;
  gap: 6px;
  flex-shrink: 0;
}

.dock-counter {
  font-size: 0.74rem;
  font-weight: 500;
  color: var(--text-muted);
  font-variant-numeric: tabular-nums;
}

.dock-collapse-btn {
  display: inline-flex;
  align-items: center;
  justify-content: center;
  width: 24px;
  height: 24px;
  padding: 0;
  border: none;
  border-radius: 7px;
  background: transparent;
  color: var(--text-muted);
  cursor: pointer;
  transition: background 0.15s ease, color 0.15s ease;
}

.dock-collapse-btn:hover {
  background: color-mix(in srgb, var(--text-main) 6%, transparent);
  color: var(--text-main);
}

.dock-chevron {
  transition: transform 0.18s ease;
}

.dock-chevron.rotated {
  transform: rotate(180deg);
}

.dock-collapsible {
  margin-top: 10px;
}

.dock-footer {
  display: flex;
  align-items: center;
  justify-content: space-between;
  gap: 8px;
  padding-top: 8px;
  border-top: 1px solid var(--glass-border-subtle, rgba(0, 0, 0, 0.06));
}

.dock-footer-left,
.dock-footer-right {
  display: flex;
  align-items: center;
  gap: 8px;
}

/* ── 子组件共享的基础交互控件样式（通过 :deep 统一下发，消除多处复制） ── */
.dock-card :deep(.dock-btn-ghost) {
  display: inline-flex;
  align-items: center;
  gap: 5px;
  padding: 5px 6px;
  border: none;
  border-radius: 7px;
  background: transparent;
  color: var(--text-muted);
  font-size: 0.76rem;
  font-weight: 500;
  cursor: pointer;
  transition: color 0.15s ease, background 0.15s ease;
}

.dock-card :deep(.dock-btn-ghost:hover:not(:disabled)) {
  color: var(--text-main);
  background: color-mix(in srgb, var(--text-main) 4%, transparent);
}

.dock-card :deep(.dock-btn-ghost:disabled) {
  opacity: 0.45;
  cursor: not-allowed;
}

.dock-card :deep(.dock-btn) {
  display: inline-flex;
  align-items: center;
  gap: 5px;
  padding: 6px 14px;
  border-radius: 8px;
  font-size: 0.78rem;
  font-weight: 500;
  cursor: pointer;
  transition: background 0.15s ease, border-color 0.15s ease, opacity 0.15s ease;
}

.dock-card :deep(.dock-btn:disabled) {
  opacity: 0.4;
  cursor: not-allowed;
}

.dock-card :deep(.dock-btn-secondary) {
  border: 1px solid var(--glass-border, rgba(0, 0, 0, 0.14));
  background: var(--surface-strong, #fff);
  color: var(--text-main);
}

.dock-card :deep(.dock-btn-secondary:hover:not(:disabled)) {
  border-color: color-mix(in srgb, var(--text-main) 35%, transparent);
  background: color-mix(in srgb, var(--text-main) 3%, transparent);
}

.dock-card :deep(.dock-btn-primary) {
  border: 1px solid var(--btn-primary-bg, var(--text-main));
  background: var(--btn-primary-bg, var(--text-main));
  color: var(--btn-primary-fg, #fff);
}

.dock-card :deep(.dock-btn-primary:hover:not(:disabled)) {
  opacity: 0.9;
}

.dock-card :deep(.dock-kbd) {
  font-size: 0.64rem;
  opacity: 0.65;
  font-family: var(--font-mono);
}

.dock-slide-enter-active {
  transition: all 0.18s ease-out;
}
.dock-slide-leave-active {
  transition: all 0.14s ease-in;
}
.dock-slide-enter-from {
  opacity: 0;
  transform: translateY(8px);
}
.dock-slide-leave-to {
  opacity: 0;
  transform: translateY(4px);
}
</style>
