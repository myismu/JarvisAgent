<script setup lang="ts">
import { ref, onMounted, nextTick, computed, watch, onUnmounted } from 'vue';
import { useSessionStore } from '../../stores/session';
import { useChatStore } from '../../stores/chat';
import { invoke } from '@tauri-apps/api/core';
import ConfirmModal from '../common/ConfirmModal.vue';
import ThinkingStatus from './ThinkingStatus.vue';
import WelcomeScreen from './WelcomeScreen.vue';

interface CheckpointEntry {
  id: string;
  operations?: Array<unknown>;
}

const session = useSessionStore();
const chat = useChatStore();
const responseAreaRef = ref<HTMLElement | null>(null);

const thinkingElapsed = ref(0);
let thinkingTimer: ReturnType<typeof setInterval> | null = null;

const updateThinkingElapsed = () => {
  const view = session.getSessionView(session.activeSessionId);
  if (view.runStartTime) {
    thinkingElapsed.value = Math.floor((Date.now() - view.runStartTime) / 1000);
  } else {
    thinkingElapsed.value = 0;
  }
};

watch(() => session.isCurrentSessionRunning, (running) => {
  if (running) {
    updateThinkingElapsed();
    thinkingTimer = setInterval(updateThinkingElapsed, 1000);
  } else {
    if (thinkingTimer) { clearInterval(thinkingTimer); thinkingTimer = null; }
    thinkingElapsed.value = 0;
  }
});

watch(() => session.activeSessionId, () => {
  if (session.isCurrentSessionRunning) {
    updateThinkingElapsed();
    if (!thinkingTimer) {
      thinkingTimer = setInterval(updateThinkingElapsed, 1000);
    }
  } else {
    if (thinkingTimer) { clearInterval(thinkingTimer); thinkingTimer = null; }
    thinkingElapsed.value = 0;
  }
});

onUnmounted(() => {
  if (thinkingTimer) clearInterval(thinkingTimer);
});

const rollbackMenu = ref<{
  visible: boolean;
  x: number;
  y: number;
  snapshotId: string | null;
  hasOperations: boolean;
}>({
  visible: false,
  x: 0,
  y: 0,
  snapshotId: null,
  hasOperations: false,
});

const rollbackLoading = ref(false);
const rollbackConfirm = ref<{
  mode: 'both' | 'session';
  snapshotId: string;
  title: string;
  message: string;
  warning?: string;
} | null>(null);

const displayWorkingDir = computed(() => {
  if (!session.workingDirectory) return null;
  const path = session.workingDirectory;
  const parts = path.replace(/\\/g, '/').split('/');
  if (parts.length <= 3) return path;
  return '.../' + parts.slice(-3).join('/');
});

const scrollToBottom = async (force = false) => {
  if (!responseAreaRef.value) return;
  const { scrollTop, scrollHeight, clientHeight } = responseAreaRef.value;
  const isAtBottom = scrollHeight - scrollTop - clientHeight <= 100;

  await nextTick();
  if (responseAreaRef.value && (isAtBottom || force)) {
    responseAreaRef.value.scrollTop = responseAreaRef.value.scrollHeight;
  }
};

const handleContextMenu = (e: MouseEvent) => {
  const target = e.target as HTMLElement;
  const messageEl = target.closest('.chat-message.agent-message');
  if (!messageEl) return;

  const snapshotId = messageEl.getAttribute('data-snapshot-id');
  if (!snapshotId) return;

  e.preventDefault();
  const position = getRollbackMenuPosition(e.clientX, e.clientY, false);
  rollbackMenu.value = {
    visible: true,
    x: position.left,
    y: position.top,
    snapshotId,
    hasOperations: false,
  };
};

// 点击撤回图标按钮
const handleRollbackClick = async (e: MouseEvent) => {
  const target = e.target as HTMLElement;
  const btn = target.closest('.rollback-trigger');
  if (!btn) return;
  const cpId = btn.getAttribute('data-cp-id');
  const fallbackHasOperations = btn.getAttribute('data-has-operations') === 'true';

  if (!cpId) {
    rollbackConfirm.value = {
      mode: 'session',
      snapshotId: '',
      title: '确认撤回',
      message: '此操作没有相关联的文件检查点，仅会撤回此消息记录。确定要撤回并重新编辑吗？',
      warning: '',
    };
    return;
  }

  const hasOperations = await getRollbackHasOperations(cpId, fallbackHasOperations);
  const position = getRollbackMenuPosition(e.clientX, e.clientY, hasOperations);

  rollbackMenu.value = {
    visible: true,
    x: position.left,
    y: position.top,
    snapshotId: cpId,
    hasOperations,
  };
};

const closeRollbackMenu = () => {
  rollbackMenu.value.visible = false;
};

const getRollbackMenuPosition = (x: number, y: number, hasOperations: boolean) => {
  const menuWidth = 220;
  const menuHeight = hasOperations ? 118 : 74;
  const margin = 12;

  let left = x;
  let top = y;

  if (left + menuWidth > window.innerWidth - margin) {
    left = window.innerWidth - menuWidth - margin;
  }
  if (top + menuHeight > window.innerHeight - margin) {
    top = window.innerHeight - menuHeight - margin;
  }

  left = Math.max(margin, left);
  top = Math.max(margin, top);

  return { left, top };
};

const getRollbackHasOperations = async (checkpointId: string, fallback: boolean) => {
  try {
    const sessionId = session.activeSessionId;
    if (!sessionId) return fallback;

    const checkpoints = await invoke<CheckpointEntry[]>('list_checkpoints', {
      sessionId,
      branchName: null,
    });
    const startIndex = checkpoints.findIndex((checkpoint) => checkpoint.id === checkpointId);
    if (startIndex === -1) return fallback;

    return checkpoints.slice(startIndex).some((checkpoint) => (checkpoint.operations?.length ?? 0) > 0);
  } catch (err) {
    console.error('获取撤回操作信息失败:', err);
    return fallback;
  }
};

const executeRollback = (mode: 'both' | 'session') => {
  if (!rollbackMenu.value.snapshotId) return;

  rollbackConfirm.value = {
    mode,
    snapshotId: rollbackMenu.value.snapshotId,
    title: mode === 'both' ? '确认撤回会话与代码' : '确认撤回会话',
    message:
      mode === 'both'
        ? '确认撤回这条消息以及其后的代码改动吗？'
        : '确认仅撤回这条消息对应的会话内容吗？',
    warning:
      mode === 'both'
        ? '此操作会恢复文件状态，当前未保存修改可能丢失。'
        : '此操作会移除该消息及其后的会话记录。',
  };
};

const confirmRollback = async () => {
  if (!rollbackConfirm.value) return;

  rollbackLoading.value = true;
  try {
    const sessionId = session.activeSessionId;
    if (!sessionId) {
      alert('无法获取当前会话');
      return;
    }

    const recalledText = await invoke<string | null>('recall_last_message', { sessionId });

    if (rollbackConfirm.value.snapshotId) {
      await invoke('rollback_to_checkpoint', {
        sessionId,
        checkpointId: rollbackConfirm.value.snapshotId,
        rollbackFiles: rollbackConfirm.value.mode === 'both',
      });
    }

    rollbackConfirm.value = null;
    closeRollbackMenu();

    if (recalledText) {
      chat.rollbackRecalledMessage = recalledText;
    }

    try {
      const history = await invoke<string>('get_session_history', { sessionId });
      session.replaceSessionHistory(sessionId, history || 'Ready for input...');
      await chat.loadAgentStepsFromBackend(sessionId);
      chat.triggerRender();
    } catch {
      session.resetSessionView(sessionId);
      chat.triggerRender();
    }
  } catch (err) {
    console.error('回滚失败:', err);
    alert(`回滚失败: ${err}`);
  } finally {
    rollbackLoading.value = false;
  }
};

onMounted(() => {
  chat.registerScrollCb(scrollToBottom);
  document.addEventListener('click', (e) => {
    const target = e.target as HTMLElement;
    if (target.closest('.rollback-trigger')) return;
    if (!target.closest('.rollback-menu')) {
      closeRollbackMenu();
    }
  });
});
</script>

<template>
  <div class="response-area" ref="responseAreaRef" @contextmenu="handleContextMenu" @click="handleRollbackClick">
    <div class="working-dir-indicator" v-if="session.workingDirectory">
      <svg viewBox="0 0 24 24" width="14" height="14" stroke="currentColor" stroke-width="2" fill="none" stroke-linecap="round" stroke-linejoin="round">
        <path d="M22 19a2 2 0 0 1-2 2H4a2 2 0 0 1-2-2V5a2 2 0 0 1 2-2h5l2 3h9a2 2 0 0 1 2 2z"></path>
      </svg>
      <span class="working-dir-label">沙盒</span>
      <span class="working-dir-path" :title="session.workingDirectory || undefined">{{ displayWorkingDir }}</span>
    </div>
    <WelcomeScreen v-if="!chat.parsedHistory || chat.parsedHistory === '<p>Ready for input...</p>\n'" />
    <div class="response-text markdown-body" v-else>
      <div v-html="chat.parsedHistory"></div>
      <div v-if="chat.parsedCurrentTurnHtml" class="chat-message agent-message current-turn-message">
        <div class="message-content current-turn-content">
          <div v-html="chat.parsedCurrentTurnHtml"></div>
          <ThinkingStatus :running="session.isCurrentSessionRunning" :elapsed="thinkingElapsed" />
        </div>
      </div>
    </div>

    <Teleport to="body">
      <div
        v-if="rollbackMenu.visible"
        class="rollback-menu"
        :style="{ left: rollbackMenu.x + 'px', top: rollbackMenu.y + 'px' }"
      >
        <div class="rollback-menu-title">选择撤回方式</div>
        <button v-if="rollbackMenu.hasOperations" class="rollback-menu-item" @click="executeRollback('both')" :disabled="rollbackLoading">
          <svg viewBox="0 0 24 24" width="14" height="14" stroke="currentColor" stroke-width="2" fill="none"><polyline points="1 4 1 10 7 10"></polyline><path d="M3.51 15a9 9 0 1 0 2.13-9.36L1 10"></path></svg>
          会话和代码撤回
        </button>
        <button class="rollback-menu-item" @click="executeRollback('session')" :disabled="rollbackLoading">
          <svg viewBox="0 0 24 24" width="14" height="14" stroke="currentColor" stroke-width="2" fill="none"><path d="M21 15a2 2 0 0 1-2 2H7l-4 4V5a2 2 0 0 1 2-2h14a2 2 0 0 1 2 2z"></path></svg>
          会话撤回
        </button>
      </div>
    </Teleport>

    <ConfirmModal
      :open="!!rollbackConfirm"
      :title="rollbackConfirm?.title || ''"
      :message="rollbackConfirm?.message || ''"
      :warning="rollbackConfirm?.warning || ''"
      confirm-text="确认撤回"
      cancel-text="取消"
      confirm-kind="danger"
      :loading="rollbackLoading"
      @cancel="rollbackConfirm = null"
      @confirm="confirmRollback"
    />
  </div>
</template>

<style scoped>
.response-area {
  flex: 1;
  display: flex;
  flex-direction: column;
  padding: 16px 0;
  overflow-y: auto;
  overflow-x: hidden;
  font-size: 0.95rem;
  line-height: 1.6;
  min-width: 0;
  min-height: 0;
  scroll-behavior: smooth;
}

.welcome-screen {
  flex: 1;
  display: flex;
  flex-direction: column;
  align-items: center;
  justify-content: center;
  color: var(--accent-blue);
  opacity: 0.8;
  padding: 20px;
}

.arc-reactor-container {
  display: flex;
  flex-direction: column;
  align-items: center;
  gap: 20px;
  margin-bottom: 24px;
  filter: drop-shadow(0 0 12px rgba(59, 130, 246, 0.45));
  contain: layout style;
}

.arc-reactor {
  width: 320px;
  height: 320px;
  animation: reactorBoot 2s ease-out forwards, reactorBreath 4s 2s infinite alternate ease-in-out;
  will-change: transform, opacity;
}

.arc-reactor .ring-outer {
  animation: ringRotate 30s linear infinite;
  transform-origin: 200px 200px;
  will-change: transform;
}

.arc-reactor .ring-segments {
  animation: ringRotateReverse 20s linear infinite;
  transform-origin: 200px 200px;
  will-change: transform;
}

.arc-reactor .ring-middle {
  animation: ringRotate 25s linear infinite;
  transform-origin: 200px 200px;
  will-change: transform;
}

.arc-reactor .triangles {
  animation: ringRotateReverse 35s linear infinite;
  transform-origin: 200px 200px;
  will-change: transform;
}

.arc-reactor .ring-inner-outer {
  animation: ringRotate 15s linear infinite;
  transform-origin: 200px 200px;
  will-change: transform;
}

.arc-reactor .hex-ring {
  animation: ringRotateReverse 18s linear infinite;
  transform-origin: 200px 200px;
  will-change: transform;
}

.arc-reactor .ring-inner {
  animation: ringRotate 12s linear infinite;
  transform-origin: 200px 200px;
  will-change: transform;
}

.arc-reactor .ring-core-outer {
  animation: ringRotateReverse 8s linear infinite;
  transform-origin: 200px 200px;
  will-change: transform;
}

.arc-reactor .ring-core-segments {
  animation: ringRotate 6s linear infinite;
  transform-origin: 200px 200px;
  will-change: transform;
}

.arc-reactor .core-pulse {
  animation: corePulse 2s infinite ease-in-out;
  transform-origin: 200px 200px;
  will-change: transform;
}

.arc-reactor .scan-line {
  animation: scanRotate 3s linear infinite;
  transform-origin: 200px 200px;
  will-change: transform;
}

.arc-reactor .hud-data {
  animation: hudFlicker 5s infinite;
}

.reactor-label {
  display: flex;
  align-items: center;
  gap: 2px;
  font-family: var(--font-mono);
  font-size: 1.6rem;
  letter-spacing: 0.15em;
  font-weight: 600;
  color: var(--accent-blue);
  text-shadow: 0 0 10px rgba(59, 130, 246, 0.5), 0 0 20px rgba(59, 130, 246, 0.3);
}

.label-char {
  display: inline-block;
  animation: charGlow 3s infinite ease-in-out;
  animation-delay: calc(var(--i) * 0.2s);
}

.label-dot {
  color: #60a5fa;
  opacity: 0.6;
  animation: dotPulse 2s infinite ease-in-out;
}

@keyframes reactorBoot {
  0% {
    opacity: 0;
    transform: scale(0.5);
    filter: brightness(3) blur(10px);
  }
  50% {
    opacity: 0.8;
    filter: brightness(1.5) blur(2px);
  }
  100% {
    opacity: 1;
    transform: scale(1);
    filter: none;
  }
}

@keyframes reactorBreath {
  0% { opacity: 0.88; }
  100% { opacity: 1; }
}

@keyframes ringRotate {
  from { transform: rotate(0deg); }
  to { transform: rotate(360deg); }
}

@keyframes ringRotateReverse {
  from { transform: rotate(360deg); }
  to { transform: rotate(0deg); }
}

@keyframes corePulse {
  0%, 100% { transform: scale(1); opacity: 0.9; }
  50% { transform: scale(1.08); opacity: 1; }
}

@keyframes scanRotate {
  from { transform: rotate(0deg); }
  to { transform: rotate(360deg); }
}

@keyframes hudFlicker {
  0%, 95%, 100% { opacity: 0.3; }
  96% { opacity: 0.1; }
  97% { opacity: 0.35; }
  98% { opacity: 0.15; }
}

@keyframes charGlow {
  0%, 100% {
    text-shadow: 0 0 8px rgba(59, 130, 246, 0.4), 0 0 16px rgba(59, 130, 246, 0.2);
    opacity: 0.85;
  }
  50% {
    text-shadow: 0 0 14px rgba(59, 130, 246, 0.7), 0 0 28px rgba(59, 130, 246, 0.4);
    opacity: 1;
  }
}

@keyframes dotPulse {
  0%, 100% { opacity: 0.4; }
  50% { opacity: 0.8; }
}

.welcome-text {
  font-size: 1.2rem;
  letter-spacing: 0.1em;
  font-weight: 500;
  color: var(--text-muted);
}

.working-dir-indicator {
  position: sticky;
  top: 0;
  z-index: 10;
  display: flex;
  align-items: center;
  gap: 6px;
  padding: 8px 12px;
  margin: 0 16px 8px;
  background: var(--glass-bg);
  backdrop-filter: blur(12px);
  -webkit-backdrop-filter: blur(12px);
  border: 1px solid var(--glass-border-subtle);
  border-radius: var(--radius-md);
  font-size: 0.8rem;
  color: var(--text-muted);
  box-shadow: var(--shadow-sm);
}

.working-dir-indicator svg {
  flex-shrink: 0;
  color: var(--accent-green);
}

.working-dir-indicator .working-dir-label {
  color: var(--accent-green);
  font-weight: 600;
  font-size: 0.75rem;
  text-transform: uppercase;
  letter-spacing: 0.05em;
}

.working-dir-indicator .working-dir-path {
  font-family: var(--font-mono);
  color: var(--text-main);
  overflow: hidden;
  text-overflow: ellipsis;
  white-space: nowrap;
  max-width: 300px;
}

.response-text {
  flex: 1;
  padding: 0 16px;
  display: flex;
  flex-direction: column;
  gap: 20px;
}

:deep(.chat-message) {
  display: flex;
  width: 100%;
  margin-bottom: 16px;
  animation: slideIn var(--transition-normal) forwards;
}

@keyframes slideIn {
  from { opacity: 0; transform: translateY(8px) scale(0.98); }
  to { opacity: 1; transform: translateY(0) scale(1); }
}

:deep(.user-message) {
  justify-content: flex-end;
  align-items: flex-start;
  position: relative;
  gap: 8px;
}

:deep(.agent-message) {
  justify-content: flex-start;
}

:deep(.message-content) {
  max-width: 85%;
  padding: 12px 18px;
  border-radius: var(--radius-xl);
  font-size: 0.95rem;
  line-height: 1.6;
  letter-spacing: 0.01em;
  transition: transform var(--transition-fast), box-shadow var(--transition-fast);
  word-wrap: break-word;
}

:deep(.message-content:hover) {
  transform: translateY(-1px);
}

:deep(.user-message .message-content) {
  background: var(--glass-bg-heavy);
  backdrop-filter: blur(var(--glass-blur));
  -webkit-backdrop-filter: blur(var(--glass-blur));
  border: 1px solid var(--glass-border);
  color: var(--text-main);
  border-bottom-right-radius: 4px;
  box-shadow: var(--shadow-sm);
}

:deep(.user-message .message-content:hover) {
  box-shadow: var(--shadow-md);
  border-color: var(--accent-blue);
}

:deep(.agent-message .message-content) {
  background: var(--glass-bg);
  backdrop-filter: blur(var(--glass-blur));
  -webkit-backdrop-filter: blur(var(--glass-blur));
  border: 1px solid var(--glass-border);
  border-bottom-left-radius: 4px;
  box-shadow: var(--shadow-sm);
}

:deep(.agent-message .message-content:hover) {
  box-shadow: var(--shadow-md);
  border-color: var(--glass-border);
}

:deep(.user-message .message-content p) {
  margin: 0;
}
:deep(.user-message .message-content a) {
  color: var(--accent-blue);
  text-decoration: underline;
  text-underline-offset: 3px;
}
:deep(.user-message .message-content a:hover) {
  color: var(--accent-blue-hover);
}

.response-text :deep(p) {
  margin-top: 0;
  margin-bottom: 0.75em;
}

.response-text :deep(details) {
  margin: 12px 0;
  padding: 8px 12px;
  background: var(--glass-bg-light);
  backdrop-filter: blur(8px);
  -webkit-backdrop-filter: blur(8px);
  border-left: 3px solid var(--glass-border);
  border-radius: var(--radius-md);
  transition: all var(--transition-fast);
}

.response-text :deep(details:hover) {
  background: var(--glass-bg);
  border-left-color: var(--accent-blue);
}

.response-text :deep(summary) {
  cursor: pointer;
  font-size: 0.85rem;
  font-weight: 500;
  color: var(--text-muted);
  user-select: none;
  outline: none;
  transition: color var(--transition-fast);
}

.response-text :deep(summary:hover) {
  color: var(--accent-blue);
}

.response-text :deep(details[open]) {
  background: var(--glass-bg);
  border-left-color: var(--accent-blue);
}

.current-turn-content {
  position: relative;
  min-width: min(560px, 85vw);
}

.current-turn-content :deep(details:first-child) {
  margin-top: 0;
}

.current-turn-content :deep(summary) {
  padding-right: 78px;
}

.current-turn-content > :deep(.thinking-inline-status) {
  position: absolute;
  top: 12px;
  right: 14px;
  z-index: 1;
}

.response-text :deep(strong) {
  color: var(--accent-blue);
  font-weight: 600;
}

.response-text :deep(code) {
  background: var(--glass-bg-light);
  padding: 0.2em 0.4em;
  border-radius: 4px;
  font-family: var(--font-mono);
  font-size: 0.85em;
  color: var(--accent-red);
  border: 1px solid var(--glass-border-subtle);
}

.response-text :deep(pre) {
  background: var(--bg-dark);
  padding: 12px;
  border-radius: var(--radius-md);
  overflow-x: auto;
  border: 1px solid var(--glass-border);
  margin-bottom: 0.75em;
  box-shadow: inset 0 2px 4px rgba(0,0,0,0.05);
}

.response-text :deep(pre code) {
  background-color: transparent;
  padding: 0;
  color: inherit;
  font-size: 0.85rem;
  border: none;
}

.response-text :deep(ul), .response-text :deep(ol) {
  padding-left: 1.5em;
  margin-bottom: 0.75em;
}

.response-text :deep(li) {
  margin-bottom: 0.25em;
}

.rollback-menu {
  position: fixed;
  z-index: 10000;
  background: var(--glass-bg-heavy);
  backdrop-filter: blur(var(--glass-blur-heavy));
  -webkit-backdrop-filter: blur(var(--glass-blur-heavy));
  border: 1px solid var(--glass-border);
  border-radius: var(--radius-lg);
  box-shadow: var(--shadow-lg);
  padding: 8px;
  min-width: 180px;
  animation: popIn 0.15s ease-out;
}

@keyframes popIn {
  from { opacity: 0; transform: scale(0.95); }
  to { opacity: 1; transform: scale(1); }
}

.rollback-menu-title {
  font-size: 0.75rem;
  font-weight: 600;
  color: var(--text-muted);
  padding: 6px 10px;
  border-bottom: 1px solid var(--glass-border-subtle);
  margin-bottom: 4px;
  text-transform: uppercase;
  letter-spacing: 0.05em;
}

.rollback-menu-item {
  display: flex;
  align-items: center;
  gap: 8px;
  width: 100%;
  padding: 8px 12px;
  background: transparent;
  border: none;
  border-radius: var(--radius-md);
  color: var(--text-main);
  font-size: 0.85rem;
  cursor: pointer;
  transition: all var(--transition-fast);
  text-align: left;
}

.rollback-menu-item:hover:not(:disabled) {
  background: var(--glass-bg-light);
}

.rollback-menu-item:disabled {
  opacity: 0.5;
  cursor: not-allowed;
}

.rollback-menu-item.cancel {
  margin-top: 4px;
  border-top: 1px solid var(--glass-border-subtle);
  color: var(--text-muted);
}

.rollback-menu-item.cancel:hover {
  color: var(--accent-red);
  background: rgba(239, 68, 68, 0.1);
}

/* 撤回触发按钮：位于用户消息气泡左侧外部 */
:deep(.rollback-trigger) {
  position: static;
  order: -1;
  align-self: flex-start;
  flex: 0 0 auto;
  width: 24px;
  height: 24px;
  border-radius: 6px;
  border: 1px solid transparent;
  background: transparent;
  backdrop-filter: blur(8px);
  -webkit-backdrop-filter: blur(8px);
  cursor: pointer;
  padding: 0;
  margin-top: 8px;
  display: flex;
  align-items: center;
  justify-content: center;
  transition: opacity 0.15s ease, transform 0.15s ease, border-color 0.15s ease, background 0.15s ease, box-shadow 0.15s ease;
  z-index: 5;
  -webkit-app-region: no-drag;
  opacity: 0.2;
  pointer-events: auto;
}
:deep(.rollback-trigger)::after {
  content: "↩";
  font-size: 13px;
  line-height: 1;
  color: color-mix(in srgb, var(--text-muted) 78%, transparent);
}
:deep(.user-message:hover .rollback-trigger) {
  opacity: 0.75;
  border-color: var(--glass-border-subtle);
  background: var(--glass-bg);
}
:deep(.rollback-trigger:hover) {
  opacity: 1 !important;
  border-color: var(--accent-blue) !important;
  background: rgba(59, 130, 246, 0.12);
  box-shadow: 0 0 0 1px rgba(59, 130, 246, 0.12);
  transform: scale(1.1);
}
:deep(.rollback-trigger:hover)::after {
  color: var(--accent-blue);
}

.thinking-timer {
  font-family: var(--font-mono);
  font-size: 0.8rem;
  font-weight: 600;
  color: var(--accent-yellow);
  opacity: 1 !important;
  animation: none !important;
  font-variant-numeric: tabular-nums;
  letter-spacing: 0.02em;
}
</style>
