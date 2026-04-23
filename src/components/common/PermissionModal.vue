<script setup lang="ts">
import { useJarvis } from '../../composables/useJarvis';

const { permissionRequest, resolvePermission } = useJarvis();
</script>

<template>
  <div v-if="permissionRequest" class="modal-overlay">
    <div class="modal-content">
      <div class="modal-header">
        <svg viewBox="0 0 24 24" width="16" height="16" stroke="currentColor" stroke-width="2" fill="none" stroke-linecap="round" stroke-linejoin="round" style="vertical-align: text-bottom; margin-right: 6px;"><path d="M10.29 3.86L1.82 18a2 2 0 0 0 1.71 3h16.94a2 2 0 0 0 1.71-3L13.71 3.86a2 2 0 0 0-3.42 0z"></path><line x1="12" y1="9" x2="12" y2="13"></line><line x1="12" y1="17" x2="12.01" y2="17"></line></svg>
        SECURITY_ALERT
      </div>
      <div class="modal-body">
        <p class="modal-message">{{ permissionRequest.message }}</p>
        <div class="modal-actions">
          <button @click="resolvePermission('allow_once')" class="cmd-button safe">[A] ALLOW_ONCE</button>
          <button @click="resolvePermission('allow_session')" class="cmd-button warn">[S] ALLOW_SESSION</button>
          <button @click="resolvePermission('reject')" class="cmd-button danger">[R] REJECT</button>
        </div>
      </div>
    </div>
  </div>
</template>

<style scoped>
.modal-overlay {
  position: absolute;
  top: 0; left: 0; right: 0; bottom: 0;
  background: rgba(255, 255, 255, 0.6);
  display: flex;
  align-items: center;
  justify-content: center;
  z-index: 100;
  backdrop-filter: blur(4px);
}

.modal-content {
  background-color: var(--bg-panel);
  border: none;
  border-radius: 8px;
  width: 500px;
  box-shadow: 0 10px 40px rgba(0, 0, 0, 0.1);
  overflow: hidden;
}

.modal-header {
  background-color: var(--accent-red);
  color: #ffffff;
  padding: 10px 15px;
  font-weight: bold;
  font-size: 0.85rem;
  display: flex;
  align-items: center;
}

.modal-body {
  padding: 20px;
}

.modal-message {
  color: var(--text-main);
  margin-top: 0;
  margin-bottom: 25px;
  line-height: 1.5;
  white-space: pre-wrap;
}

.modal-actions {
  display: flex;
  gap: 10px;
  justify-content: flex-end;
}

.cmd-button {
  background: transparent;
  border: 1px solid var(--border-color);
  border-radius: 4px;
  color: var(--text-main);
  padding: 6px 12px;
  font-family: inherit;
  font-size: 0.85rem;
  cursor: pointer;
  transition: all 0.2s;
}

.cmd-button:hover { background-color: var(--bg-sidebar); }
.cmd-button.safe { color: var(--accent-blue); border-color: rgba(0, 102, 204, 0.2); }
.cmd-button.safe:hover { background-color: rgba(0, 102, 204, 0.05); }
.cmd-button.warn { color: var(--accent-yellow); border-color: rgba(176, 136, 0, 0.2); }
.cmd-button.warn:hover { background-color: rgba(176, 136, 0, 0.05); }
.cmd-button.danger { color: var(--accent-red); border-color: rgba(215, 58, 73, 0.2); }
.cmd-button.danger:hover { background-color: rgba(215, 58, 73, 0.05); }
</style>
