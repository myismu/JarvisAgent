<script setup lang="ts">
withDefaults(defineProps<{
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
  confirmText: '确认',
  cancelText: '取消',
  confirmKind: 'primary',
  loading: false,
})

const emit = defineEmits<{
  (e: 'confirm'): void
  (e: 'cancel'): void
}>()
</script>

<template>
  <Teleport to="body">
    <div v-if="open" class="confirm-modal-overlay" @click="!loading && emit('cancel')">
      <div class="confirm-modal" @click.stop>
        <h3>{{ title }}</h3>
        <p class="confirm-message">{{ message }}</p>
        <p v-if="warning" class="confirm-warning" role="alert">{{ warning }}</p>
        <div class="modal-actions">
          <button class="cancel-btn" :disabled="loading" @click="emit('cancel')">{{ cancelText }}</button>
          <button
            class="confirm-btn"
            :class="confirmKind === 'danger' ? 'danger' : 'primary'"
            :disabled="loading"
            @click="emit('confirm')"
          >
            {{ loading ? '处理中...' : confirmText }}
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
  color: var(--text-warning);
  background: var(--surface-warning);
  border: 1px solid var(--border-warning);
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
