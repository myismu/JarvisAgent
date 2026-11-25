<!--
# ConfirmModal.vue — 通用确认弹窗

带标题、说明与可选警告区的模态确认框，供删除等操作二次确认。

## Constraints
- 危险确认按钮用红，警告说明区为中性灰
-->
<script setup lang="ts">
import { nextTick, ref, watch, onBeforeUnmount } from 'vue';
import { useI18n } from 'vue-i18n';

const { t } = useI18n();

const props = withDefaults(defineProps<{
  open: boolean
  title: string
  message: string
  warning?: string
  confirmText?: string
  cancelText?: string
  confirmKind?: 'primary' | 'danger'
  loading?: boolean
}>(), {
  warning: '',
  confirmText: undefined,
  cancelText: undefined,
  confirmKind: 'primary',
  loading: false,
})

const emit = defineEmits<{
  (e: 'confirm'): void
  (e: 'cancel'): void
}>()

const confirmBtnRef = ref<HTMLButtonElement | null>(null);

// 键盘支持：Enter 确认 / ESC 取消。监听必须挂在 window 上——用户可能点过正文文本，
// 焦点不在弹窗容器里，监听容器 keydown 会漏。open 关闭时同步摘除，组件卸载时兜底。
function onKeydown(e: KeyboardEvent) {
  if (props.loading || e.repeat) return;
  const tag = (document.activeElement as HTMLElement | null)?.tagName;
  if (e.key === 'Enter' && tag === 'BUTTON') return; // 焦点已在某个按钮上：交给浏览器原生行为（原生 Enter 触发的是所聚焦的那个按钮，全局拦截会把"聚焦在取消上按 Enter"错变成确认）
  if (tag === 'INPUT' || tag === 'TEXTAREA') return; // 文本输入场景不劫持按键（为弹窗内将来可能的输入框留余地）
  if (e.key === 'Enter') {
    e.preventDefault();
    emit('confirm');
  } else if (e.key === 'Escape') {
    e.preventDefault();
    emit('cancel');
  }
}

watch(() => props.open, (open) => {
  if (open) {
    window.addEventListener('keydown', onKeydown);
    // 焦点管理：把焦点拉进弹窗、落在确认按钮上。否则焦点仍停留在「打开弹窗的那个按钮」
    // （鼠标点击会聚焦它），onKeydown 里"焦点在按钮上放行"的规则会让 Enter 原生触发
    // **背后那个按钮**——表现为把弹窗又打开一次，看起来就是"Enter 没反应"。
    // 聚焦确认按钮后：Enter 走原生 click 即确认（带焦点环提示），Tab 可切到取消，
    // 焦点漂走（点过正文）时仍由全局监听兜底。
    nextTick(() => confirmBtnRef.value?.focus());
  } else {
    window.removeEventListener('keydown', onKeydown);
  }
}, { immediate: true });

onBeforeUnmount(() => {
  window.removeEventListener('keydown', onKeydown);
});
</script>

<template>
  <Teleport to="body">
    <Transition name="confirm-fade">
      <div v-if="open" class="confirm-modal-overlay" @click="!loading && emit('cancel')">
        <div class="confirm-modal" role="dialog" aria-modal="true" :aria-label="title" @click.stop>
          <header class="confirm-header">
            <div class="confirm-header-left">
              <span class="confirm-dot" :class="confirmKind" aria-hidden="true"></span>
              <h3>{{ title }}</h3>
            </div>
            <button
              type="button"
              class="confirm-close-btn"
              :disabled="loading"
              :aria-label="cancelText || t('common.cancel')"
              @click="emit('cancel')"
            >
              <svg viewBox="0 0 24 24" width="14" height="14" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" aria-hidden="true">
                <line x1="18" y1="6" x2="6" y2="18"></line>
                <line x1="6" y1="6" x2="18" y2="18"></line>
              </svg>
            </button>
          </header>

          <div class="confirm-body">
            <slot name="message">
              <p class="confirm-message">{{ message }}</p>
            </slot>
            <p v-if="warning" class="confirm-warning" :class="confirmKind" role="alert">{{ warning }}</p>
          </div>

          <footer class="modal-actions">
            <slot name="actions">
              <button type="button" class="cancel-btn" :disabled="loading" @click="emit('cancel')">
                {{ cancelText || t('common.cancel') }}
              </button>
              <button
                ref="confirmBtnRef"
                type="button"
                class="confirm-btn"
                :class="confirmKind === 'danger' ? 'danger' : 'primary'"
                :disabled="loading"
                @click="emit('confirm')"
              >
                {{ loading ? t('common.processing') : (confirmText || t('common.confirm')) }}
              </button>
            </slot>
          </footer>
        </div>
      </div>
    </Transition>
  </Teleport>
</template>

<style scoped>
.confirm-modal-overlay {
  position: fixed;
  inset: 0;
  background: rgba(0, 0, 0, 0.36);
  backdrop-filter: blur(4px);
  -webkit-backdrop-filter: blur(4px);
  display: flex;
  align-items: center;
  justify-content: center;
  padding: 16px;
  z-index: 10000;
}

.confirm-modal {
  background: var(--surface-strong, #fff);
  border: 1px solid var(--glass-border, rgba(0, 0, 0, 0.09));
  border-radius: 16px;
  width: min(92vw, 448px);
  box-shadow: 0 18px 48px rgba(0, 0, 0, 0.16), 0 2px 8px rgba(0, 0, 0, 0.06);
  overflow: hidden;
  color: var(--text-main);
}

.confirm-header {
  display: flex;
  align-items: center;
  justify-content: space-between;
  gap: 12px;
  padding: 16px 18px 12px;
}

.confirm-header-left {
  display: flex;
  align-items: center;
  gap: 8px;
  min-width: 0;
}

.confirm-dot {
  flex-shrink: 0;
  width: 7px;
  height: 7px;
  border-radius: 50%;
  background: var(--text-main);
  opacity: 0.72;
}

.confirm-dot.danger {
  background: var(--accent-red);
  opacity: 0.9;
}

.confirm-header h3 {
  margin: 0;
  font-size: 0.94rem;
  font-weight: 600;
  letter-spacing: -0.01em;
  color: var(--text-main);
  line-height: 1.35;
}

.confirm-close-btn {
  display: inline-flex;
  align-items: center;
  justify-content: center;
  width: 26px;
  height: 26px;
  padding: 0;
  border: none;
  border-radius: 7px;
  background: transparent;
  color: var(--text-muted);
  cursor: pointer;
  flex-shrink: 0;
  transition: background 0.14s ease, color 0.14s ease;
}

.confirm-close-btn:hover:not(:disabled) {
  background: color-mix(in srgb, var(--text-main) 6%, transparent);
  color: var(--text-main);
}

.confirm-close-btn:disabled {
  opacity: 0.4;
  cursor: not-allowed;
}

.confirm-body {
  display: flex;
  flex-direction: column;
  gap: 12px;
  padding: 0 18px 16px;
}

.confirm-message {
  margin: 0;
  color: var(--text-soft);
  line-height: 1.65;
  font-size: 0.86rem;
  white-space: pre-wrap;
}

/* 轻量警示提示条：左侧细强调线 + 极淡底色，避免大灰盒叠加 */
.confirm-warning {
  margin: 0;
  padding: 8px 11px;
  border-radius: 8px;
  border-left: 2.5px solid color-mix(in srgb, var(--text-muted) 45%, transparent);
  background: color-mix(in srgb, var(--text-main) 3%, transparent);
  color: var(--text-muted);
  font-size: 0.8rem;
  font-weight: 500;
  line-height: 1.55;
  white-space: pre-wrap;
}

.confirm-warning.danger {
  border-left-color: color-mix(in srgb, var(--accent-red) 70%, transparent);
  background: color-mix(in srgb, var(--accent-red) 5%, transparent);
  color: color-mix(in srgb, var(--accent-red) 82%, var(--text-main));
}

.modal-actions {
  display: flex;
  align-items: center;
  justify-content: flex-end;
  gap: 8px;
  padding: 12px 18px;
  border-top: 1px solid color-mix(in srgb, var(--text-main) 7%, transparent);
  background: color-mix(in srgb, var(--text-main) 1.5%, transparent);
}

.cancel-btn,
.confirm-btn {
  display: inline-flex;
  align-items: center;
  justify-content: center;
  height: 32px;
  padding: 0 14px;
  border-radius: 8px;
  font-size: 0.82rem;
  font-weight: 500;
  cursor: pointer;
  white-space: nowrap;
  transition: background 0.14s ease, border-color 0.14s ease, color 0.14s ease, opacity 0.14s ease;
}

.cancel-btn {
  background: transparent;
  color: var(--text-main);
  border: 1px solid color-mix(in srgb, var(--text-main) 14%, transparent);
}

.cancel-btn:hover:not(:disabled) {
  background: color-mix(in srgb, var(--text-main) 5%, transparent);
  border-color: color-mix(in srgb, var(--text-main) 22%, transparent);
}

.confirm-btn.primary {
  background: var(--btn-primary-bg);
  color: var(--btn-primary-fg);
  border: 1px solid transparent;
}

.confirm-btn.primary:hover:not(:disabled) {
  background: var(--btn-primary-bg-hover);
}

.confirm-btn.danger {
  background: var(--surface-danger);
  color: var(--text-on-danger);
  border: 1px solid transparent;
}

.confirm-btn.danger:hover:not(:disabled) {
  background: var(--surface-danger-hover);
}

.cancel-btn:disabled,
.confirm-btn:disabled {
  opacity: 0.45;
  cursor: not-allowed;
}

.confirm-fade-enter-active,
.confirm-fade-leave-active {
  transition: opacity 0.16s ease;
}

.confirm-fade-enter-active .confirm-modal,
.confirm-fade-leave-active .confirm-modal {
  transition: transform 0.16s ease, opacity 0.16s ease;
}

.confirm-fade-enter-from,
.confirm-fade-leave-to {
  opacity: 0;
}

.confirm-fade-enter-from .confirm-modal,
.confirm-fade-leave-to .confirm-modal {
  transform: translateY(6px) scale(0.985);
  opacity: 0;
}
</style>
