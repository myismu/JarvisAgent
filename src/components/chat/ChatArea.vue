<script setup lang="ts">
import { ref, onMounted, nextTick, computed, watch, onUnmounted } from 'vue';
import { useI18n } from 'vue-i18n';
import { useSessionStore } from '../../stores/session';
import { useChatStore } from '../../stores/chat';
import { usePermissionStore } from '../../stores/permission';
import { usePreferences } from '../../composables/usePreferences';
import { invoke } from '@tauri-apps/api/core';
import ConfirmModal from '../common/ConfirmModal.vue';
import AgentTurn from './AgentTurn.vue';
import TodoPanel from './TodoPanel.vue';
import PermissionCard from './PermissionCard.vue';
import WelcomeScreen from './WelcomeScreen.vue';
import type { PlanDocument } from '../../types';

interface RollbackPreviewFile {
  path: string;
  linesAdded: number;
  linesRemoved: number;
}

interface RollbackPreviewResult {
  targetCheckpointId: string;
  checkpointLabel: string;
  files: RollbackPreviewFile[];
}

interface RollbackRecallResult {
  restoredFiles: string[];
  recalledText: string;
}

interface RollbackPreviewState {
  loading: boolean;
  checkpointId: string;
  files: RollbackPreviewFile[];
  targetCheckpointId: string;
  checkpointLabel: string;
  error: string;
}

const { t } = useI18n();

const session = useSessionStore();
const chat = useChatStore();
const perm = usePermissionStore();
const prefs = usePreferences();
const responseAreaRef = ref<HTMLElement | null>(null);
const shouldFollowStream = ref(prefs.autoScroll);
const currentTurn = computed(() => session.currentSessionView.currentTurn);
const hasCurrentTurnContent = computed(() => {
  const turn = currentTurn.value;
  return Boolean(
    turn.textBlocks.some((block) => block.content.trim()) ||
      turn.thinkingBlocks.some((block) => block.content.trim()) ||
      turn.toolCalls.length > 0 ||
      turn.logs.some((log) => log.content.trim())
  );
});
const showInlineStatus = computed(() => {
  const view = session.currentSessionView;
  return Boolean(
    view.runStartTime &&
    (view.streamActive || (session.isCurrentSessionRunning && !hasCurrentTurnContent.value))
  );
});

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

watch(showInlineStatus, (running) => {
  if (running) {
    updateThinkingElapsed();
    thinkingTimer = setInterval(updateThinkingElapsed, 1000);
  } else {
    if (thinkingTimer) { clearInterval(thinkingTimer); thinkingTimer = null; }
    thinkingElapsed.value = 0;
  }
});

watch(() => session.activeSessionId, () => {
  if (showInlineStatus.value) {
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
  rollbackMode: 'both' | 'session';
  userMessageIndex: number | null;
  messageId: string | null;
  fallbackSnapshotId: string;
}>({
  visible: false,
  x: 0,
  y: 0,
  snapshotId: null,
  rollbackMode: 'session',
  userMessageIndex: null,
  fallbackSnapshotId: '',
  messageId: null,
});

const rollbackConfirm = ref<{
  mode: 'both' | 'session';
  snapshotId: string;
  fallbackSnapshotId: string;
  userMessageIndex: number | null;
  messageId: string | null;
  title: string;
  message: string;
  files: RollbackPreviewFile[];
  warning?: string;
} | null>(null);

const rollbackPreview = ref<RollbackPreviewState>({
  loading: false,
  checkpointId: '',
  files: [],
  targetCheckpointId: '',
  checkpointLabel: '',
  error: '',
});
const rollbackLoading = ref(false);
const rollbackError = ref('');
function ensureRollbackButtons() {
  const root = responseAreaRef.value?.querySelector('.history-html');
  if (!root) return;

  root.querySelectorAll('.user-message').forEach((messageEl, index) => {
    const existingButtons = Array.from(messageEl.querySelectorAll('.rollback-trigger'));
    existingButtons.slice(1).forEach((button) => button.remove());

    const contentEl = messageEl.querySelector('.message-content');
    if (contentEl && !contentEl.getAttribute('data-user-message-index')) {
      contentEl.setAttribute('data-user-message-index', String(index));
    }

    // 从后端渲染的 message-content 属性中读取回滚信息
    const rollbackCheckpointId = contentEl?.getAttribute('data-rollback-checkpoint-id') || '';

    if (existingButtons.length > 0) {
      // 已有按钮时同步最新的回滚属性（后端刷新历史后属性可能更新）
      const existingBtn = existingButtons[0] as HTMLElement;
      existingBtn.setAttribute('data-cp-id', rollbackCheckpointId);
      return;
    }

    const button = document.createElement('button');
    button.className = 'rollback-trigger';
    button.setAttribute('data-cp-id', rollbackCheckpointId);
    button.setAttribute('data-latest-snapshot-id', '');
    button.setAttribute('title', t('rollback.trigger'));
    messageEl.appendChild(button);
  });
}

watch(
  () => chat.parsedHistory,
  async () => {
    await nextTick();
    ensureRollbackButtons();
  },
  { immediate: true, flush: 'post' }
);

const displayWorkingDir = computed(() => {
  if (!session.workingDirectory) return null;
  const path = session.workingDirectory;
  const parts = path.replace(/\\/g, '/').split('/');
  if (parts.length <= 3) return path;
  return '.../' + parts.slice(-3).join('/');
});

const isResponseAtBottom = () => {
  if (!responseAreaRef.value) return false;
  const { scrollTop, scrollHeight, clientHeight } = responseAreaRef.value;
  return scrollHeight - scrollTop - clientHeight <= 35;
};

const setResponseScrollToBottom = () => {
  if (!responseAreaRef.value) return;
  responseAreaRef.value.scrollTop = responseAreaRef.value.scrollHeight;
};

const forceScrollToBottomAfterRender = async () => {
  shouldFollowStream.value = true;

  await nextTick();
  setResponseScrollToBottom();

  requestAnimationFrame(() => {
    setResponseScrollToBottom();
    requestAnimationFrame(setResponseScrollToBottom);
  });
};

const scrollToBottom = async (force = false) => {
  if (force) {
    await forceScrollToBottomAfterRender();
    return;
  }

  const shouldScroll = force || shouldFollowStream.value || isResponseAtBottom();

  await nextTick();
  if (responseAreaRef.value && shouldScroll) {
    responseAreaRef.value.scrollTop = responseAreaRef.value.scrollHeight;
  }
};

const handleResponseScroll = () => {
  shouldFollowStream.value = Boolean(isResponseAtBottom());
};

const showScrollToBottom = computed(() => {
  return !shouldFollowStream.value && (chat.parsedHistory || hasCurrentTurnContent.value);
});

watch(() => session.isCurrentSessionRunning, (running) => {
  if (running) {
    scrollToBottom(true);
  }
});

const pendingInitialScrollSessionKey = ref<string | null>(null);
const currentSessionKey = computed(() => session.activeSessionId || '__default__');

watch(currentSessionKey, (key) => {
  pendingInitialScrollSessionKey.value = key;
}, { immediate: true });

watch(
  () => [
    currentSessionKey.value,
    session.currentSessionView.hydrated,
    chat.parsedHistory,
    hasCurrentTurnContent.value,
  ],
  async ([key, hydrated, history, hasTurnContent]) => {
    if (pendingInitialScrollSessionKey.value !== key) return;
    if (!hydrated && !history && !hasTurnContent) return;

    pendingInitialScrollSessionKey.value = null;
    await forceScrollToBottomAfterRender();
  },
  { immediate: true, flush: 'post' }
);

watch(() => [chat.parsedCurrentTurnHtml, currentTurn.value.revision], () => {
  if (shouldFollowStream.value) {
    scrollToBottom();
  }
});

const handleContextMenu = (e: MouseEvent) => {
  const target = e.target as HTMLElement;
  const messageEl = target.closest('.chat-message.agent-message');
  if (!messageEl) return;

  const snapshotId = messageEl.getAttribute('data-snapshot-id');
  if (!snapshotId) return;

  e.preventDefault();
  const position = getRollbackMenuPosition(e.clientX, e.clientY, 'session');
  rollbackMenu.value = {
    visible: true,
    x: position.left,
    y: position.top,
    snapshotId,
    rollbackMode: 'session',
    userMessageIndex: null,
    messageId: null,
    fallbackSnapshotId: '',
  };
};

// 点击撤回图标按钮
const handleRollbackClick = async (e: MouseEvent) => {
  const target = e.target as HTMLElement;
  const btn = target.closest('.rollback-trigger');
  if (!btn) return;
  const userMessageEl = btn.closest('.user-message');
  const contentEl = userMessageEl?.querySelector('.message-content');
  const userMessageIndexAttr = contentEl?.getAttribute('data-user-message-index');
  const messageId = contentEl?.getAttribute('data-message-id') || null;
  const userMessageIndex = userMessageIndexAttr ? Number(userMessageIndexAttr) : null;
  const rollbackTarget = Number.isInteger(userMessageIndex) ? userMessageIndex : null;
  const rollbackMode = contentEl?.getAttribute('data-rollback-mode') === 'both' ? 'both' : 'session';
  const rollbackCheckpointId = contentEl?.getAttribute('data-rollback-checkpoint-id') || '';
  const position = getRollbackMenuPosition(e.clientX, e.clientY, rollbackMode);

  rollbackMenu.value = {
    visible: true,
    x: position.left,
    y: position.top,
    snapshotId: rollbackCheckpointId,
    rollbackMode,
    userMessageIndex: rollbackTarget,
    messageId,
    fallbackSnapshotId: '',
  };
};

const copyText = async (text: string, html?: string) => {
  if (html && navigator.clipboard?.write && typeof ClipboardItem !== 'undefined') {
    try {
      await navigator.clipboard.write([
        new ClipboardItem({
          'text/html': new Blob([html], { type: 'text/html' }),
          'text/plain': new Blob([text], { type: 'text/plain' }),
        }),
      ]);
      return;
    } catch {
      // Fall back to plain text below when rich clipboard writes are unavailable.
    }
  }

  if (navigator.clipboard?.writeText) {
    await navigator.clipboard.writeText(text);
    return;
  }

  const textarea = document.createElement('textarea');
  textarea.value = text;
  textarea.setAttribute('readonly', '');
  textarea.style.position = 'fixed';
  textarea.style.left = '-9999px';
  textarea.style.opacity = '0';
  document.body.appendChild(textarea);
  textarea.select();
  document.execCommand('copy');
  document.body.removeChild(textarea);
};

const showCopiedState = (button: HTMLButtonElement) => {
  const previousText = button.textContent || t('common.copy');
  button.textContent = t('common.copied');
  button.classList.add('copied');
  window.setTimeout(() => {
    button.textContent = previousText;
    button.classList.remove('copied');
  }, 1200);
};

const copyCodeBlock = async (button: HTMLButtonElement) => {
  const block = button.closest('.markdown-code-block');
  const code = block?.querySelector('pre code')?.textContent || '';
  if (!code.trim()) return;
  await copyText(code);
  showCopiedState(button);
};

const normalizeMarkdownTableCell = (cell?: HTMLTableCellElement) => {
  if (!cell) return '';
  return cell.innerText
    .replace(/\s+/g, ' ')
    .trim()
    .replace(/\\/g, '\\\\')
    .replace(/\|/g, '\\|');
};

const markdownAlignment = (cell?: HTMLTableCellElement) => {
  const align = (cell?.getAttribute('align') || '').toLowerCase();
  if (align === 'center') return ':---:';
  if (align === 'right') return '---:';
  return '---';
};

const tableToMarkdown = (table: HTMLTableElement) => {
  const rows = Array.from(table.rows);
  if (rows.length === 0) return '';

  const headerRow = table.tHead?.rows[0] || rows[0];
  const bodyRows = rows.filter((row) => row !== headerRow);
  const columnCount = Math.max(...rows.map((row) => row.cells.length), 1);
  const readRow = (row: HTMLTableRowElement) =>
    Array.from({ length: columnCount }, (_, index) => normalizeMarkdownTableCell(row.cells[index]));

  const header = readRow(headerRow);
  const separator = Array.from({ length: columnCount }, (_, index) =>
    markdownAlignment(headerRow.cells[index])
  );
  const body = bodyRows.map((row) => `| ${readRow(row).join(' | ')} |`);

  return [`| ${header.join(' | ')} |`, `| ${separator.join(' | ')} |`, ...body].join('\n');
};

const copyTable = async (button: HTMLButtonElement) => {
  const wrap = button.closest('.markdown-table-wrap');
  const table = wrap?.querySelector('table');
  if (!table) return;
  await copyText(tableToMarkdown(table), table.outerHTML);
  showCopiedState(button);
};

const handleResponseClick = (e: MouseEvent) => {
  const target = e.target as HTMLElement;
  const codeCopyButton = target.closest<HTMLButtonElement>('.code-copy-btn');
  if (codeCopyButton) {
    e.preventDefault();
    e.stopPropagation();
    copyCodeBlock(codeCopyButton);
    return;
  }

  const tableCopyButton = target.closest<HTMLButtonElement>('.table-copy-btn');
  if (tableCopyButton) {
    e.preventDefault();
    e.stopPropagation();
    copyTable(tableCopyButton);
    return;
  }

  handleRollbackClick(e);
};

const closeRollbackMenu = () => {
  rollbackMenu.value.visible = false;
};

const getRollbackMenuPosition = (x: number, y: number, rollbackMode: 'both' | 'session') => {
  const menuWidth = 220;
  const menuHeight = rollbackMode === 'both' ? 118 : 74;
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

const executeRollback = async (mode: 'both' | 'session') => {
  rollbackError.value = '';
  rollbackPreview.value.error = '';
  if (mode === 'both') {
    rollbackLoading.value = true;
    rollbackPreview.value.loading = true;
    try {
      const sessionId = session.activeSessionId;
      if (!sessionId) {
        alert(t('rollback.noSession'));
        return;
      }
      const result = await invoke<RollbackPreviewResult>('preview_rollback_to_checkpoint_with_recall', {
        sessionId,
        checkpointId: rollbackMenu.value.snapshotId || rollbackMenu.value.fallbackSnapshotId || '',
        messageId: rollbackMenu.value.messageId,
        userMessageIndex: rollbackMenu.value.userMessageIndex,
      });
      rollbackPreview.value = {
        loading: false,
        checkpointId: result.targetCheckpointId,
        files: result.files,
        targetCheckpointId: result.targetCheckpointId,
        checkpointLabel: result.checkpointLabel,
        error: '',
      };
    } catch (err) {
      const message = normalizeRollbackError(err);
      rollbackPreview.value.loading = false;
      rollbackPreview.value.error = message;
      rollbackError.value = message;
      return;
    } finally {
      rollbackPreview.value.loading = false;
      rollbackLoading.value = false;
    }
  }

  const previewMessage = mode === 'both'
    ? t('rollback.confirmBothMessage')
    : t('rollback.confirmSessionMessage');

  rollbackConfirm.value = {
    mode,
    snapshotId: rollbackMenu.value.snapshotId || '',
    fallbackSnapshotId: rollbackMenu.value.fallbackSnapshotId,
    userMessageIndex: rollbackMenu.value.userMessageIndex,
    messageId: rollbackMenu.value.messageId,
    title: mode === 'both' ? t('rollback.confirmBothTitle') : t('rollback.confirmSessionTitle'),
    message: previewMessage,
    files: mode === 'both' ? rollbackPreview.value.files : [],
    warning:
      mode === 'both'
        ? t('rollback.warningBoth')
        : t('rollback.warningSession'),
  };
};

const normalizeRollbackError = (err: unknown) => {
  const raw = typeof err === 'string' ? err : err instanceof Error ? err.message : String(err || t('common.unknownError'));
  if (raw.includes(t('rollback.historyKept'))) {
    return raw;
  }
  return t('rollback.historyKeptSuffix', { error: raw });
};

const confirmRollback = async () => {
  if (!rollbackConfirm.value) return;

  rollbackLoading.value = true;
  rollbackError.value = '';
  try {
    const sessionId = session.activeSessionId;
    if (!sessionId) {
      alert(t('rollback.noSession'));
      return;
    }

    let recalledText: string | null = null;
    const rollbackUserMessageIndex = rollbackConfirm.value.userMessageIndex;
    const rollbackMessageId = rollbackConfirm.value.messageId;
    if (rollbackConfirm.value.snapshotId || rollbackConfirm.value.fallbackSnapshotId || rollbackConfirm.value.mode === 'both') {
      // 有 checkpointId 或需要回滚代码（后端会按传入快照恢复文件）
      const result = await invoke<RollbackRecallResult>('rollback_to_checkpoint_with_recall', {
        sessionId,
        checkpointId: rollbackConfirm.value.snapshotId || rollbackConfirm.value.fallbackSnapshotId || '',
        rollbackFiles: rollbackConfirm.value.mode === 'both',
        messageId: rollbackMessageId,
        userMessageIndex: rollbackUserMessageIndex,
      });
      recalledText = result.recalledText;
    } else if (rollbackMessageId || rollbackUserMessageIndex !== null) {
      recalledText = await invoke<string | null>('recall_message', {
        sessionId,
        messageId: rollbackMessageId,
        userMessageIndex: rollbackUserMessageIndex,
      });
    } else {
      recalledText = await invoke<string | null>('recall_last_message', { sessionId });
    }

    rollbackConfirm.value = null;
    rollbackPreview.value = {
      loading: false,
      checkpointId: '',
      files: [],
      targetCheckpointId: '',
      checkpointLabel: '',
      error: '',
    };
    closeRollbackMenu();

    if (recalledText) {
      chat.rollbackRecalledMessage = recalledText;
    }

    try {
      const history = await invoke<string>('get_session_history', { sessionId });
      session.replaceSessionHistory(sessionId, history || 'Ready for input...');
      const planDocuments = await invoke<PlanDocument[]>('list_plan_documents', { sessionId });
      perm.planDocumentsBySession = {
        ...perm.planDocumentsBySession,
        [sessionId]: planDocuments,
      };
      delete perm.planProposals[sessionId];
      chat.triggerRender();
    } catch {
      session.resetSessionView(sessionId);
      chat.triggerRender();
    }
  } catch (err) {
    const message = normalizeRollbackError(err);
    console.error('回滚失败:', err);
    rollbackError.value = message;
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
  <div class="response-area" ref="responseAreaRef" @scroll="handleResponseScroll" @contextmenu="handleContextMenu" @click="handleResponseClick">
    <div class="working-dir-indicator" v-if="session.workingDirectory">
      <svg viewBox="0 0 24 24" width="14" height="14" stroke="currentColor" stroke-width="2" fill="none" stroke-linecap="round" stroke-linejoin="round">
        <path d="M22 19a2 2 0 0 1-2 2H4a2 2 0 0 1-2-2V5a2 2 0 0 1 2-2h5l2 3h9a2 2 0 0 1 2 2z"></path>
      </svg>
      <span class="working-dir-label">{{ t('rollback.sandbox') }}</span>
      <span class="working-dir-path" :title="session.workingDirectory || undefined">{{ displayWorkingDir }}</span>
    </div>
    <TodoPanel />
    <WelcomeScreen v-if="!chat.parsedHistory || chat.parsedHistory === '<p>Ready for input...</p>\n'" />
    <div class="response-text markdown-body" v-else>
      <div class="history-html" v-html="chat.parsedHistory"></div>
      <div v-if="hasCurrentTurnContent || showInlineStatus" class="chat-message agent-message current-turn-message">
        <div
          class="message-content current-turn-content"
          :class="{ 'waiting-only': !hasCurrentTurnContent && showInlineStatus }"
        >
          <AgentTurn
            :turn="currentTurn"
            :display-mode="prefs.agentAudience.value"
            :show-status="showInlineStatus"
            :elapsed="thinkingElapsed"
          />
        </div>
      </div>
      <PermissionCard />
    </div>

    <Teleport to="body">
      <div
        v-if="rollbackMenu.visible"
        class="rollback-menu"
        :style="{ left: rollbackMenu.x + 'px', top: rollbackMenu.y + 'px' }"
      >
        <div class="rollback-menu-title">{{ t('rollback.selectMode') }}</div>
        <button v-if="rollbackMenu.rollbackMode === 'both'" class="rollback-menu-item" @click="executeRollback('both')" :disabled="rollbackLoading">
          <svg viewBox="0 0 24 24" width="14" height="14" stroke="currentColor" stroke-width="2" fill="none"><polyline points="1 4 1 10 7 10"></polyline><path d="M3.51 15a9 9 0 1 0 2.13-9.36L1 10"></path></svg>
          {{ t('rollback.sessionAndCode') }}
        </button>
        <button class="rollback-menu-item" @click="executeRollback('session')" :disabled="rollbackLoading">
          <svg viewBox="0 0 24 24" width="14" height="14" stroke="currentColor" stroke-width="2" fill="none"><path d="M21 15a2 2 0 0 1-2 2H7l-4 4V5a2 2 0 0 1 2-2h14a2 2 0 0 1 2 2z"></path></svg>
          {{ t('rollback.sessionOnly') }}
        </button>
      </div>
    </Teleport>

    <ConfirmModal
      :open="!!rollbackConfirm"
      :title="rollbackConfirm?.title || ''"
      :message="rollbackConfirm?.message || ''"
      :warning="rollbackError || rollbackConfirm?.warning || ''"
      :confirm-text="t('rollback.confirm')"
      :cancel-text="t('common.cancel')"
      confirm-kind="danger"
      :loading="rollbackLoading"
      @cancel="rollbackConfirm = null"
      @confirm="confirmRollback"
    >
      <template v-if="rollbackConfirm?.mode === 'both'" #message>
        <div class="rollback-preview-modal">
          <p class="rollback-preview-message">{{ rollbackConfirm.message }}</p>
          <div v-if="rollbackConfirm.files.length > 0" class="rollback-preview-files">
            <div class="rollback-preview-summary">{{ t('rollback.previewSummary', { count: rollbackConfirm.files.length }) }}</div>
            <div class="rollback-preview-file-list">
              <div v-for="file in rollbackConfirm.files" :key="file.path" class="rollback-preview-file">
                <span class="rollback-preview-path" :title="file.path">{{ file.path }}</span>
                <span class="rollback-preview-stats">
                  <span class="rollback-preview-added">+{{ file.linesAdded }}</span>
                  <span class="rollback-preview-removed">-{{ file.linesRemoved }}</span>
                </span>
              </div>
            </div>
          </div>
          <p v-else class="rollback-preview-empty">{{ t('rollback.previewEmpty') }}</p>
        </div>
      </template>
    </ConfirmModal>

    <Transition name="scroll-btn">
      <button
        v-if="showScrollToBottom"
        class="scroll-to-bottom-btn"
        @click="scrollToBottom(true)"
        :title="t('rollback.scrollToBottom')"
      >
        <svg viewBox="0 0 24 24" width="16" height="16" stroke="currentColor" stroke-width="2" fill="none" stroke-linecap="round" stroke-linejoin="round">
          <polyline points="6 9 12 15 18 9"></polyline>
        </svg>
      </button>
    </Transition>
  </div>
</template>

<style scoped>
.response-area {
  flex: 1;
  display: flex;
  flex-direction: column;
  padding: 16px 0 200px; /* Increased bottom padding for floating input */
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
  padding: 0 40px; /* 与输入框左右间距一致 */
  display: flex;
  flex-direction: column;
  gap: 24px;
}

:deep(.chat-message) {
  display: flex;
  width: 100%;
  margin-bottom: 20px;
  animation: slideIn var(--transition-normal) forwards;
}

.history-html :deep(.chat-message) {
  animation: none;
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
  max-width: 94%; /* 允许会话内容占据更多宽度，匹配全宽输入框 */
  padding: 14px 22px;
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

/* 长消息折叠 */
:deep(.user-msg-collapsed) {
  position: relative;
  max-height: 180px;
  overflow: hidden;
  transition: max-height 0.3s ease;
}
:deep(.user-msg-collapsed[data-collapsed="false"]) {
  max-height: none;
}
:deep(.user-msg-collapsed[data-collapsed="true"] .user-msg-fade) {
  display: block;
}
:deep(.user-msg-collapsed[data-collapsed="false"] .user-msg-fade) {
  display: none;
}
:deep(.user-msg-fade) {
  position: absolute;
  bottom: 0;
  left: 0;
  right: 0;
  height: 48px;
  background: linear-gradient(transparent, var(--glass-bg-heavy));
  pointer-events: none;
}
:deep(.user-msg-toggle) {
  display: block;
  margin-top: 6px;
  padding: 2px 0;
  background: none;
  border: none;
  color: var(--accent-blue);
  font-size: 0.8rem;
  cursor: pointer;
  font-family: inherit;
}
:deep(.user-msg-toggle:hover) {
  color: var(--accent-blue-hover);
  text-decoration: underline;
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
  border-radius: var(--radius-md);
  transition: background-color var(--transition-fast);
}

.response-text :deep(details:hover) {
  background: var(--glass-bg);
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
  color: var(--text-main);
}

.response-text :deep(details[open]) {
  background: var(--glass-bg);
}

.current-turn-content,
.response-text :deep(.current-turn-content) {
  position: relative;
  min-width: min(560px, 85vw);
}

.current-turn-content.waiting-only {
  min-width: auto;
  min-height: 34px;
  display: inline-flex;
  align-items: center;
  justify-content: flex-start;
}

.current-turn-content :deep(details:first-child),
.response-text :deep(.current-turn-content details:first-child) {
  margin-top: 0;
}

.current-turn-content :deep(summary),
.response-text :deep(.current-turn-content summary) {
  padding-right: 0;
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

.response-text :deep(.markdown-code-block),
.response-text :deep(.markdown-table-wrap) {
  margin: 12px 0 16px;
  overflow: hidden;
  border: 1px solid color-mix(in srgb, var(--text-muted) 18%, transparent);
  border-radius: var(--radius-md);
  background: color-mix(in srgb, var(--surface-strong) 72%, var(--glass-bg-heavy));
  box-shadow: var(--shadow-sm);
}

.response-text :deep(.markdown-code-header),
.response-text :deep(.markdown-table-header) {
  min-height: 34px;
  padding: 6px 8px 6px 12px;
  display: flex;
  align-items: center;
  justify-content: space-between;
  gap: 12px;
  border-bottom: 1px solid color-mix(in srgb, var(--text-muted) 14%, transparent);
  background: color-mix(in srgb, var(--glass-bg-heavy) 72%, transparent);
  color: var(--text-muted);
  font-size: 0.76rem;
  font-weight: 650;
}

.response-text :deep(.markdown-code-language),
.response-text :deep(.markdown-table-header span) {
  min-width: 0;
  overflow: hidden;
  text-overflow: ellipsis;
  white-space: nowrap;
}

.response-text :deep(.markdown-copy-btn) {
  height: 24px;
  min-width: 48px;
  padding: 0 9px;
  display: inline-flex;
  align-items: center;
  justify-content: center;
  border: 1px solid color-mix(in srgb, var(--text-muted) 18%, transparent);
  border-radius: 6px;
  background: color-mix(in srgb, var(--surface-strong) 82%, transparent);
  color: var(--text-muted);
  font: inherit;
  font-size: 0.74rem;
  font-weight: 650;
  cursor: pointer;
  transition: color var(--transition-fast), border-color var(--transition-fast), background var(--transition-fast);
}

.response-text :deep(.markdown-copy-btn:hover) {
  color: var(--accent-blue);
  border-color: color-mix(in srgb, var(--accent-blue) 38%, transparent);
  background: color-mix(in srgb, var(--accent-blue) 9%, var(--surface-strong));
}

.response-text :deep(.markdown-copy-btn.copied) {
  color: var(--accent-green);
  border-color: color-mix(in srgb, var(--accent-green) 42%, transparent);
  background: color-mix(in srgb, var(--accent-green) 10%, var(--surface-strong));
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

.response-text :deep(.markdown-code-block pre) {
  margin: 0;
  padding: 14px 16px;
  border: 0;
  border-radius: 0;
  background: color-mix(in srgb, var(--bg-dark) 82%, var(--surface-strong));
  box-shadow: none;
}

.response-text :deep(pre code) {
  background-color: transparent;
  padding: 0;
  color: inherit;
  font-size: var(--code-font-size);
  border: none;
}

.response-text :deep(.markdown-table-scroll) {
  overflow-x: auto;
}

.response-text :deep(table) {
  width: 100%;
  border-collapse: separate;
  border-spacing: 0;
  font-size: 0.88rem;
}

.response-text :deep(th),
.response-text :deep(td) {
  padding: 9px 12px;
  text-align: left;
  vertical-align: top;
  border-right: 1px solid color-mix(in srgb, var(--text-muted) 13%, transparent);
  border-bottom: 1px solid color-mix(in srgb, var(--text-muted) 13%, transparent);
}

.response-text :deep(th:last-child),
.response-text :deep(td:last-child) {
  border-right: 0;
}

.response-text :deep(tr:last-child td) {
  border-bottom: 0;
}

.response-text :deep(th) {
  color: var(--text-main);
  font-weight: 700;
  background: color-mix(in srgb, var(--accent-blue) 8%, transparent);
}

.response-text :deep(td code) {
  white-space: nowrap;
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

.rollback-preview-modal {
  display: flex;
  flex-direction: column;
  gap: 12px;
}

.rollback-preview-message,
.rollback-preview-empty {
  margin: 0;
  color: var(--text-main);
  line-height: 1.7;
  font-size: 0.95rem;
}

.rollback-preview-empty {
  color: var(--text-muted);
}

.rollback-preview-files {
  border: 1px solid var(--glass-border-subtle);
  border-radius: var(--radius-md);
  background: var(--glass-bg-light);
  overflow: hidden;
}

.rollback-preview-summary {
  padding: 9px 12px;
  color: var(--text-muted);
  font-size: 0.82rem;
  font-weight: 600;
  border-bottom: 1px solid var(--glass-border-subtle);
}

.rollback-preview-file-list {
  max-height: 220px;
  overflow-y: auto;
}

.rollback-preview-file {
  display: flex;
  align-items: center;
  justify-content: space-between;
  gap: 12px;
  padding: 8px 12px;
  font-family: var(--font-mono);
  font-size: 0.82rem;
  border-bottom: 1px solid var(--glass-border-subtle);
}

.rollback-preview-file:last-child {
  border-bottom: none;
}

.rollback-preview-path {
  min-width: 0;
  overflow: hidden;
  text-overflow: ellipsis;
  white-space: nowrap;
  color: var(--text-main);
}

.rollback-preview-stats {
  display: inline-flex;
  gap: 8px;
  flex-shrink: 0;
  font-weight: 700;
}

.rollback-preview-added {
  color: var(--accent-green);
}

.rollback-preview-removed {
  color: var(--accent-red);
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

/* 滚动到底部按钮 */
.scroll-to-bottom-btn {
  position: absolute;
  bottom: calc(160px); /* 响应式定位：140px 是 response-area 的底部 padding，20px 是额外间距 */
  left: 50%;
  transform: translateX(-50%);
  z-index: 10;
  width: 36px;
  height: 36px;
  border-radius: 50%;
  border: 1px solid var(--glass-border);
  background: var(--glass-bg-heavy);
  backdrop-filter: blur(12px);
  -webkit-backdrop-filter: blur(12px);
  color: var(--accent-blue);
  cursor: pointer;
  display: flex;
  align-items: center;
  justify-content: center;
  box-shadow: var(--shadow-md);
  transition: all var(--transition-fast);
}
.scroll-to-bottom-btn:hover {
  background: var(--glass-bg);
  border-color: var(--accent-blue);
  box-shadow: 0 0 0 2px rgba(59, 130, 246, 0.2);
  transform: translateX(-50%) scale(1.08);
}
.scroll-to-bottom-btn:active {
  transform: translateX(-50%) scale(0.95);
}

.scroll-btn-enter-active,
.scroll-btn-leave-active {
  transition: opacity 0.2s ease, transform 0.2s ease;
}
.scroll-btn-enter-from,
.scroll-btn-leave-to {
  opacity: 0;
  transform: translateX(-50%) translateY(8px);
}
</style>
