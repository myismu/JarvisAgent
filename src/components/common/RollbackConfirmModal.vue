<script setup lang="ts">
defineProps<{
  visible: boolean;
  title?: string;
  message: string;
  details?: string[];
  loading?: boolean;
}>();

const emit = defineEmits<{
  (e: 'confirm'): void;
  (e: 'cancel'): void;
}>();
</script>

<template>
  <Teleport to="body">
    <div v-if="visible" class="modal-overlay" @click.self="emit('cancel')">
      <div class="modal-content rollback-modal">
        <h3 class="modal-title">{{ title || '确认回滚' }}</h3>
        <p class="rollback-message">{{ message }}</p>
        <div v-if="details && details.length > 0" class="rollback-details">
          <p class="details-label">操作详情：</p>
          <ul>
            <li v-for="(detail, i) in details" :key="i">{{ detail }}</li>
          </ul>
        </div>
        <div class="modal-actions">
          <button class="cmd-button safe" @click="emit('confirm')" :disabled="loading">
            {{ loading ? '执行中...' : '确认回滚' }}
          </button>
          <button class="cmd-button" @click="emit('cancel')" :disabled="loading">取消</button>
        </div>
      </div>
    </div>
  </Teleport>
</template>

<style scoped>
.modal-overlay {
  position: fixed;
  top: 0;
  left: 0;
  right: 0;
  bottom: 0;
  background: rgba(0, 0, 0, 0.5);
  display: flex;
  justify-content: center;
  align-items: center;
  z-index: 1000;
}
.modal-content {
  background: var(--glass-bg-heavy);
  backdrop-filter: blur(var(--glass-blur));
  border: 1px solid var(--glass-border);
  border-radius: var(--radius-lg);
  padding: var(--space-lg);
  max-width: 480px;
  width: 90%;
}
.modal-title {
  margin: 0 0 var(--space-md);
  color: var(--text-main);
  font-size: 16px;
}
.rollback-message {
  color: var(--text-muted);
  margin-bottom: var(--space-md);
  line-height: 1.5;
}
.rollback-details {
  background: var(--glass-bg-light);
  border-radius: var(--radius-md);
  padding: var(--space-sm) var(--space-md);
  margin-bottom: var(--space-md);
  max-height: 200px;
  overflow-y: auto;
}
.details-label {
  color: var(--text-muted);
  font-size: 12px;
  margin-bottom: var(--space-xs);
}
.rollback-details ul {
  margin: 0;
  padding-left: var(--space-md);
  font-size: 13px;
  color: var(--text-main);
}
.modal-actions {
  display: flex;
  gap: var(--space-sm);
  justify-content: flex-end;
}
.cmd-button {
  padding: 6px 16px;
  border-radius: var(--radius-md);
  border: 1px solid var(--glass-border);
  background: var(--glass-bg);
  color: var(--text-main);
  cursor: pointer;
  font-size: 13px;
  transition: all var(--transition-fast);
}
.cmd-button:hover:not(:disabled) {
  background: var(--glass-bg-light);
}
.cmd-button:disabled {
  opacity: 0.5;
  cursor: not-allowed;
}
.cmd-button.safe {
  background: var(--accent-blue);
  border-color: var(--accent-blue);
  color: white;
}
.cmd-button.safe:hover:not(:disabled) {
  opacity: 0.9;
}
</style>
