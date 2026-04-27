<script setup lang="ts">
import { computed, onMounted, onUnmounted } from 'vue';
import { useJarvis } from '../../composables/useJarvis';

const { permissionRequest, resolvePermission } = useJarvis();

// 智能解析消息，提取原因和命令内容
const parsedData = computed(() => {
  if (!permissionRequest.value) return { reason: '', command: '' };
  
  const msg = permissionRequest.value.message;
  let reason = msg;
  let command = '';
  
  // 模式 1: 匹配反引号内的命令 (Markdown 风格)
  const codeMatch = msg.match(/`([^`]+)`/);
  if (codeMatch) {
    reason = msg.replace(codeMatch[0], '').trim();
    command = codeMatch[1].trim();
  } else {
    // 模式 2: 匹配冒号后的长内容 (兼容中英文冒号)
    const colonMatch = msg.match(/[:：]/);
    if (colonMatch && colonMatch.index !== undefined) {
      const colonIndex = colonMatch.index;
      const potentialCommand = msg.substring(colonIndex + 1).trim();
      // 只有当冒号后内容较长时才视为命令块
      if (potentialCommand.length > 5) {
        reason = msg.substring(0, colonIndex + 1).trim();
        command = potentialCommand;
      }
    } else if (msg.length > 150) {
      reason = "请求执行以下复杂系统指令：";
      command = msg;
    }
  }

  // 精简过长的命令，确保用户能理解且不会破坏界面
  const MAX_CMD_LENGTH = 500;
  if (command.length > MAX_CMD_LENGTH) {
    command = command.substring(0, 250) + '\n\n... [指令过长已精简，中间内容省略] ...\n\n' + command.substring(command.length - 200);
  }
  
  // 如果 reason 仍然很长，也进行截断，避免撑破界面
  if (reason.length > 150) {
    reason = reason.substring(0, 150) + '...';
  }

  return { reason, command };
});

// 快捷键支持: A-Allow, S-Session, R-Reject
const handleKeydown = (e: KeyboardEvent) => {
  if (!permissionRequest.value) return;
  
  const key = e.key.toLowerCase();
  if (key === 'a') {
    e.preventDefault();
    resolvePermission('allow_once');
  } else if (key === 's') {
    e.preventDefault();
    resolvePermission('allow_session');
  } else if (key === 'r' || key === 'escape') {
    e.preventDefault();
    resolvePermission('reject');
  }
};

onMounted(() => window.addEventListener('keydown', handleKeydown, true));
onUnmounted(() => window.removeEventListener('keydown', handleKeydown, true));
</script>

<template>
  <Transition name="fade">
    <div v-if="permissionRequest" class="modal-overlay">
      <div class="modal-content glass-panel-heavy">
        <!-- 顶部的红色警示光晕 -->
        <div class="modal-glow"></div>
        
        <div class="modal-header">
          <div class="header-icon">
            <svg viewBox="0 0 24 24" width="24" height="24" stroke="currentColor" stroke-width="2.5" fill="none" stroke-linecap="round" stroke-linejoin="round">
              <path d="M10.29 3.86L1.82 18a2 2 0 0 0 1.71 3h16.94a2 2 0 0 0 1.71-3L13.71 3.86a2 2 0 0 0-3.42 0z"></path>
              <line x1="12" y1="9" x2="12" y2="13"></line>
              <line x1="12" y1="17" x2="12.01" y2="17"></line>
            </svg>
          </div>
          <div class="header-text">
            <h3>安全确认 / SECURITY REQUEST</h3>
            <p>Jarvis 正在请求敏感权限</p>
          </div>
        </div>

        <div class="modal-body">
          <p class="reason-text">{{ parsedData.reason }}</p>
          
          <div v-if="parsedData.command" class="command-container">
            <div class="container-label">待执行的详细指令:</div>
            <pre class="command-block"><code>{{ parsedData.command }}</code></pre>
          </div>
          
          <div class="modal-actions">
            <button @click="resolvePermission('reject')" class="cmd-button danger" title="快捷键: R 或 Esc">
              <span class="key-hint">R</span> 拒绝
            </button>
            <div class="allow-group">
              <button @click="resolvePermission('allow_once')" class="cmd-button safe" title="快捷键: A">
                <span class="key-hint">A</span> 允许一次
              </button>
              <button @click="resolvePermission('allow_session')" class="cmd-button warn" title="快捷键: S">
                <span class="key-hint">S</span> 本次会话始终允许
              </button>
            </div>
          </div>
        </div>
      </div>
    </div>
  </Transition>
</template>

<style scoped>
.modal-overlay {
  position: absolute;
  top: 0; left: 0; right: 0; bottom: 0;
  background: rgba(0, 0, 0, 0.45);
  display: flex;
  align-items: center;
  justify-content: center;
  z-index: 1000;
  backdrop-filter: blur(8px);
}

.modal-content {
  position: relative;
  width: 520px;
  max-width: 90vw;
  border-radius: var(--radius-xl);
  border: 1px solid rgba(239, 68, 68, 0.25);
  overflow: hidden;
  box-shadow: 0 20px 50px rgba(0, 0, 0, 0.3);
  animation: modal-bounce 0.4s cubic-bezier(0.34, 1.56, 0.64, 1);
}

.modal-glow {
  position: absolute;
  top: -40px; left: 0; right: 0;
  height: 80px;
  background: radial-gradient(circle, rgba(239, 68, 68, 0.2) 0%, transparent 70%);
  pointer-events: none;
  z-index: 0;
}

.modal-header {
  display: flex;
  align-items: center;
  padding: 24px 24px 16px;
  gap: 16px;
  position: relative;
  z-index: 1;
}

.header-icon {
  width: 44px;
  height: 44px;
  background: rgba(239, 68, 68, 0.15);
  border-radius: 12px;
  display: flex;
  align-items: center;
  justify-content: center;
  color: var(--accent-red);
  box-shadow: 0 0 15px rgba(239, 68, 68, 0.1);
}

.header-text h3 {
  margin: 0;
  font-size: 1.05rem;
  font-weight: 700;
  letter-spacing: 0.3px;
  color: var(--text-main);
}

.header-text p {
  margin: 2px 0 0;
  font-size: 0.8rem;
  color: var(--text-muted);
}

.modal-body {
  padding: 0 24px 24px;
  position: relative;
  z-index: 1;
}

.reason-text {
  font-size: 0.92rem;
  margin-bottom: 18px;
  line-height: 1.6;
  color: var(--text-main);
  word-break: break-word;
}

.command-container {
  margin-bottom: 24px;
}

.container-label {
  font-size: 0.7rem;
  font-weight: 700;
  text-transform: uppercase;
  margin-bottom: 8px;
  color: var(--text-muted);
  letter-spacing: 1px;
}

.command-block {
  background: rgba(0, 0, 0, 0.25);
  border: 1px solid var(--glass-border);
  border-radius: var(--radius-md);
  padding: 14px;
  max-height: 160px;
  overflow-y: auto;
  margin: 0;
  box-shadow: inset 0 2px 8px rgba(0, 0, 0, 0.2);
}

.command-block code {
  font-family: var(--font-mono);
  font-size: 0.82rem;
  color: var(--text-soft);
  white-space: pre-wrap;
  word-break: break-all;
  line-height: 1.5;
}

.modal-actions {
  display: flex;
  justify-content: space-between;
  align-items: center;
  gap: 12px;
  margin-top: 10px;
}

.allow-group {
  display: flex;
  gap: 12px;
}

.cmd-button {
  display: flex;
  align-items: center;
  gap: 8px;
  padding: 9px 16px;
  border-radius: var(--radius-md);
  border: 1px solid var(--glass-border);
  background: var(--glass-bg-light);
  color: var(--text-main);
  font-size: 0.85rem;
  font-weight: 600;
  cursor: pointer;
  transition: all 0.2s cubic-bezier(0.4, 0, 0.2, 1);
  backdrop-filter: blur(4px);
}

.key-hint {
  font-size: 0.65rem;
  background: rgba(255, 255, 255, 0.08);
  padding: 1px 5px;
  border-radius: 4px;
  border: 1px solid rgba(255, 255, 255, 0.15);
  min-width: 1.4em;
  text-align: center;
  font-weight: 400;
}

.cmd-button:hover {
  transform: translateY(-1.5px);
  background: var(--glass-bg);
  box-shadow: 0 6px 15px rgba(0, 0, 0, 0.15);
  border-color: var(--glass-border-heavy);
}

.cmd-button:active {
  transform: translateY(0);
}

.cmd-button.danger {
  border-color: rgba(239, 68, 68, 0.2);
  color: var(--accent-red);
}
.cmd-button.danger:hover {
  background: rgba(239, 68, 68, 0.12);
  border-color: rgba(239, 68, 68, 0.5);
}

.cmd-button.safe {
  border-color: rgba(59, 130, 246, 0.2);
  color: var(--accent-blue);
}
.cmd-button.safe:hover {
  background: rgba(59, 130, 246, 0.12);
  border-color: rgba(59, 130, 246, 0.5);
}

.cmd-button.warn {
  border-color: rgba(245, 158, 11, 0.2);
  color: var(--accent-yellow);
}
.cmd-button.warn:hover {
  background: rgba(245, 158, 11, 0.12);
  border-color: rgba(245, 158, 11, 0.5);
}

/* Animations */
@keyframes modal-bounce {
  from { transform: scale(0.92) translateY(15px); opacity: 0; }
  to { transform: scale(1) translateY(0); opacity: 1; }
}

.fade-enter-active, .fade-leave-active {
  transition: opacity 0.3s ease;
}
.fade-enter-from, .fade-leave-to {
  opacity: 0;
}

/* 滚动条美化 */
.command-block::-webkit-scrollbar {
  width: 4px;
}
.command-block::-webkit-scrollbar-thumb {
  background: rgba(255, 255, 255, 0.1);
  border-radius: 10px;
}
.command-block::-webkit-scrollbar-thumb:hover {
  background: rgba(255, 255, 255, 0.2);
}
</style>
