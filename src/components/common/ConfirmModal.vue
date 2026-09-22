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
    <div v-if="open" class="confirm-modal-overlay" @click="!loading && emit('cancel')">
      <div class="confirm-modal" @click.stop>
        <h3>{{ title }}</h3>
        <slot name="message">
          <p class="confirm-message">{{ message }}</p>
        </slot>
        <p v-if="warning" class="confirm-warning" role="alert">{{ warning }}</p>
        <div class="modal-actions">
          <button class="cancel-btn" :disabled="loading" @click="emit('cancel')">{{ cancelText || t('common.cancel') }}</button>
          <button
            ref="confirmBtnRef"
            class="confirm-btn"
            :class="confirmKind === 'danger' ? 'danger' : 'primary'"
            :disabled="loading"
            @click="emit('confirm')"
          >
            {{ loading ? t('common.processing') : (confirmText || t('common.confirm')) }}
          </button>
        </div>
      </div>
    </div>
  </Teleport>
</template>

<style scoped>
.confirm-modal-overlay {
  position: fixed;
  top: 0;
  left: 0;
  right: 0;
  bottom: 0;
  background: rgba(0, 0, 0, 0.55);
  display: flex;
  align-items: center;
  justify-content: center;
  z-index: 10000;
  backdrop-filter: blur(8px);
  -webkit-backdrop-filter: blur(8px);
}

.confirm-modal {
  background: var(--surface-strong);
  backdrop-filter: blur(var(--glass-blur-heavy));
  -webkit-backdrop-filter: blur(var(--glass-blur-heavy));
  border: 1px solid var(--glass-border);
  border-radius: var(--radius-xl);
  padding: 24px;
  width: min(92vw, 420px);
  box-shadow: var(--glass-shadow);
}

.confirm-modal h3 {
  margin: 0 0 12px;
  font-size: 1.05rem;
  font-weight: 700;
  color: var(--text-main);
}

.confirm-message {
  margin: 0 0 10px;
  color: var(--text-main);
  line-height: 1.7;
  font-size: 0.95rem;
  white-space: pre-wrap;
}

.confirm-warning {
  margin: 0;
  padding: 12px 14px;
  color: var(--text-soft);
  background: var(--glass-bg-light);
  border: 1px solid var(--glass-border);
  border-radius: var(--radius-md);
  font-size: 0.9rem;
  font-weight: 600;
  line-height: 1.6;
  white-space: pre-wrap;
}

.modal-actions {
  display: flex;
  gap: 12px;
  margin-top: 18px;
}

.cancel-btn,
.confirm-btn {
  flex: 1;
  min-height: 44px;
  padding: 10px 16px;
  border-radius: var(--radius-md);
  font-size: 0.92rem;
  font-weight: 600;
  cursor: pointer;
  transition: all var(--transition-fast);
  border: 1px solid transparent;
}

.cancel-btn {
  background: var(--glass-bg-light);
  color: var(--text-main);
  border-color: var(--glass-border);
}

.cancel-btn:hover:not(:disabled) {
  background: var(--glass-bg);
}

.confirm-btn.primary {
  background: var(--accent-blue);
  color: var(--text-inverse);
}

.confirm-btn.primary:hover:not(:disabled) {
  background: var(--accent-blue-hover);
}

.confirm-btn.danger {
  background: var(--surface-danger);
  color: var(--text-on-danger);
  border-color: var(--border-danger);
}

.confirm-btn.danger:hover:not(:disabled) {
  background: var(--surface-danger-hover);
}

.cancel-btn:disabled,
.confirm-btn:disabled {
  opacity: 0.6;
  cursor: not-allowed;
}
</style>
