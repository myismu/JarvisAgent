<!--
# Sidebar.vue — 应用侧边栏与会话列表

提供导航入口、延迟创建会话、历史会话检索筛选、会话切换/重命名/删除等交互。

## Key Exports
- `Sidebar`: 左侧导航与会话管理组件

## Dependencies
- Internal: `@/stores/session`, `@/stores/chat`, `@/composables/useAgentEvents`
-->
<script setup lang="ts">
import { useI18n } from 'vue-i18n';
import { ref, onMounted, onUnmounted } from 'vue';
import type { SessionMeta } from '../../types';
import { invoke } from '@tauri-apps/api/core';
import { listen } from '@tauri-apps/api/event';
import { open } from '@tauri-apps/plugin-dialog';
import { useSessionStore } from '../../stores/session';
import { useChatStore } from '../../stores/chat';
import { useAgentStore } from '../../stores/agent';
import { useAgentEvents } from '../../composables/useAgentEvents';
import { useWindow } from '../../composables/useWindow';

defineProps<{
  collapsed: boolean;
}>();

const emit = defineEmits<{
  (e: 'open-settings'): void;
}>();

const { t } = useI18n();

const sessionStore = useSessionStore();
const chat = useChatStore();
const agent = useAgentStore();
const events = useAgentEvents();
const { notifyMonitorSessionChanged } = useWindow();

// 会话管理状态
const sessions = ref<SessionMeta[]>([]);
const sessionSearchKeyword = ref('');
const sessionFilterTool = ref('');
const sessionFilterHasTools = ref(false);
const sessionFilterRange = ref<'all' | '24h' | '7d' | '30d'>('all');
const showAdvancedSessionFilters = false;
const showSessionFilters = ref(false);
const sessionActionMessage = ref('');
const editingSessionId = ref<string | null>(null);
const editingTitle = ref('');
const sessionActionMessageKind = ref<'info' | 'error'>('info');
let sessionActionTimer: ReturnType<typeof setTimeout> | null = null;

const isSessionRunning = (sessionId: string): boolean => {
  return sessionStore.sessionViews[sessionId]?.status === "RUNNING";
};

const sessionFilterFromTs = () => {
  const now = Date.now();
  switch (sessionFilterRange.value) {
    case '24h': return now - 24 * 60 * 60 * 1000;
    case '7d': return now - 7 * 24 * 60 * 60 * 1000;
    case '30d': return now - 30 * 24 * 60 * 60 * 1000;
    default: return null;
  }
};

const hasActiveSessionFilters = () => Boolean(
  sessionSearchKeyword.value.trim()
  || sessionFilterTool.value.trim()
  || sessionFilterHasTools.value
  || sessionFilterRange.value !== 'all'
);

const clearSessionFilters = async () => {
  sessionSearchKeyword.value = '';
  sessionFilterTool.value = '';
  sessionFilterHasTools.value = false;
  sessionFilterRange.value = 'all';
  await loadSessions();
};

const formatSessionTime = (timestamp: number) => {
  const date = new Date(timestamp);
  return `${date.getMonth() + 1}/${date.getDate()} ${date.getHours().toString().padStart(2, '0')}:${date.getMinutes().toString().padStart(2, '0')}`;
};

const formatSessionTokens = (session: SessionMeta) => {
  const total = (session.totalInputTokens || 0) + (session.totalOutputTokens || 0);
  if (!total) return '';
  if (total >= 1000) return `${(total / 1000).toFixed(1)}k tok`;
  return `${total} tok`;
};

const showSessionActionMessage = (message: string, kind: 'info' | 'error' = 'info') => {
  sessionActionMessage.value = message;
  sessionActionMessageKind.value = kind;
  if (sessionActionTimer) {
    clearTimeout(sessionActionTimer);
  }
  sessionActionTimer = setTimeout(() => {
    sessionActionMessage.value = '';
    sessionActionTimer = null;
  }, 3500);
};

const formatErrorMessage = (err: unknown) => {
  if (typeof err === 'string') return err;
  if (err instanceof Error) return err.message;
  try {
    return JSON.stringify(err);
  } catch {
    return String(err);
  }
};

const requestWorkingDirectory = async () => {
  try {
    const selected = await open({
      directory: true,
      multiple: false,
      title: t('sidebar.selectWorkspace'),
    });

    if (typeof selected === 'string' && selected.trim()) {
      return selected.trim();
    }

    if (Array.isArray(selected) && typeof selected[0] === 'string' && selected[0].trim()) {
      return selected[0].trim();
    }

    return null;
  } catch (dialogErr) {
    console.error('打开目录选择器失败:', dialogErr);
    showSessionActionMessage(t('sidebar.dialogOpenError'), 'error');
    return null;
  }
};

// 加载会话列表
const loadSessions = async () => {
  try {
    sessionStore.setSessionListFilter({
      keyword: sessionSearchKeyword.value || null,
      tool: sessionFilterTool.value || null,
      hasToolCalls: sessionFilterHasTools.value ? true : null,
      fromTs: sessionFilterFromTs(),
    });
    sessions.value = await invoke<SessionMeta[]>('list_sessions', {
      filter: sessionStore.getSessionListFilterPayload(),
    });
  } catch (err) {
    console.error('加载会话列表失败:', err);
  }
};

const startRenameSession = (session: SessionMeta, event: Event) => {
  event.stopPropagation();
  editingSessionId.value = session.id;
  editingTitle.value = session.title;
};

const cancelRenameSession = () => {
  editingSessionId.value = null;
  editingTitle.value = '';
};

const submitRenameSession = async (sessionId: string) => {
  const title = editingTitle.value.trim();
  if (!title) {
    showSessionActionMessage(t('sidebar.renameRequired'), 'error');
    return;
  }
  try {
    await invoke('rename_session', { id: sessionId, title });
    cancelRenameSession();
    await loadSessions();
    showSessionActionMessage(t('sidebar.renamed'));
  } catch (err) {
    console.error('重命名会话失败:', err);
    showSessionActionMessage(t('sidebar.renameError', { error: formatErrorMessage(err) }), 'error');
  }
};

// 创建新会话
const createNewSession = async (withSandbox: boolean = false) => {
  try {
    let sandboxDir: string | null = null;

    if (withSandbox) {
      const selected = await requestWorkingDirectory();
      if (!selected) {
        return;
      }
      sandboxDir = selected;
    }

    sessionStore.activeSessionId = null;
    // 新建会话时切到全局默认模型
    try {
      const config = await invoke<any>('get_config');
      if (config.globalProfileId) {
        config.activeProfileId = config.globalProfileId;
        await invoke('save_config_cmd', { newConfig: config });
      }
    } catch { /* ignore */ }
    sessionStore.pendingWorkingDirectory = sandboxDir;
    sessionStore.workingDirectory = sandboxDir;
    sessionStore.resetSessionView(null);
    sessionStore.setSessionUsageTotals(0, 0);
    chat.resetRenderState();
    chat.triggerRender();
    await notifyMonitorSessionChanged(null);

    requestAnimationFrame(() => chat.forceScrollToBottom());
  } catch (err) {
    console.error('准备新会话失败:', err);
    showSessionActionMessage(t('sidebar.newError', { error: formatErrorMessage(err) }), 'error');
  }
};

// 切换会话
const switchToSession = async (id: string) => {
  if (id === sessionStore.activeSessionId) return;
  try {
    const meta = await invoke<any>('switch_session', { id });
    sessionStore.activeSessionId = id;
    sessionStore.workingDirectory = meta.workingDirectory || null;

    const config = await invoke<any>('get_config');
    if (meta.profileId) {
      config.activeProfileId = meta.profileId;
    } else {
      config.activeProfileId = config.globalProfileId;
    }
    await invoke('save_config_cmd', { newConfig: config });

    sessionStore.setSessionUsageTotals(meta.totalInputTokens || 0, meta.totalOutputTokens || 0);

    // 已渲染过的会话保留原有 UI，不覆盖（特别是正在运行的会话）
    const wasHydrated = sessionStore.hasHydratedSessionView(id);
    if (!wasHydrated) {
      try {
        const messages = await invoke<any[]>('get_session_messages', { sessionId: id });
        sessionStore.replaceSessionMessages(id, messages);
      } catch {
        const history = await invoke<string>('get_session_history', { sessionId: id });
        sessionStore.replaceSessionHistory(id, history || 'Ready for input...');
      }
    }

    await Promise.all([
      events.loadPlanDocumentsFromBackend(id),
      events.loadTodosFromBackend(id),
      events.loadAgentRunsFromBackend(id, { refreshHistory: !wasHydrated }),
      events.loadAgentRunEventsFromBackend(id),
      events.loadSubAgentRunsFromBackend(id),
      events.loadSubAgentEventsFromBackend(id),
      events.loadContextSnapshotFromBackend(id),
    ]);

    // 已渲染过的会话不清 buffer（保留正在运行的 stream 状态）
    if (!wasHydrated) {
      chat.resetRenderState();
    }
    chat.triggerRender();
    await notifyMonitorSessionChanged(id);
    await loadSessions();
    requestAnimationFrame(() => chat.forceScrollToBottom());
  } catch (err) {
    console.error('切换会话失败:', err);
  }
};

// 删除会话
const deleteSession = async (id: string, event: Event) => {
  event.stopPropagation();
  if (id === sessionStore.activeSessionId) return;
  if (isSessionRunning(id)) {
    showSessionActionMessage(t('sidebar.deleteRunning'), 'error');
    return;
  }
  try {
    await invoke('delete_session', { id });
    await loadSessions();
  } catch (err) {
    console.error('删除会话失败:', err);
  }
};

let unlistenRenamed: (() => void) | null = null;
let unlistenUpdated: (() => void) | null = null;

onMounted(async () => {
  await loadSessions();

  try {
    const activeId = await invoke<string | null>('get_active_session_id');
    if (activeId) {
      try {
        await invoke('switch_session', { id: activeId });
        sessionStore.activeSessionId = activeId;
        const meta = await invoke<any>('get_session_meta', { id: activeId });
        sessionStore.workingDirectory = meta.workingDirectory || null;
        sessionStore.setSessionUsageTotals(meta.totalInputTokens || 0, meta.totalOutputTokens || 0);

        // 加载会话历史
        try {
          const messages = await invoke<any[]>('get_session_messages', { sessionId: activeId });
          sessionStore.replaceSessionMessages(activeId, messages);
        } catch {
          const history = await invoke<string>('get_session_history', { sessionId: activeId });
          if (history && history.trim()) {
            chat.jarvisResponse = history;
          } else {
            chat.jarvisResponse = 'Ready for input...';
          }
        }
      } catch (switchErr) {
        console.error('同步会话状态失败:', switchErr);
        sessionStore.setSessionUsageTotals(0, 0);
        if (sessions.value.length > 0) {
          sessionStore.activeSessionId = sessions.value[0].id;
        }
      }
    } else if (sessions.value.length > 0) {
      sessionStore.activeSessionId = sessions.value[0].id;
      sessionStore.setSessionUsageTotals(sessions.value[0].totalInputTokens || 0, sessions.value[0].totalOutputTokens || 0);
    }
  } catch (err) {
    sessionStore.setSessionUsageTotals(0, 0);
    if (sessions.value.length > 0) {
      sessionStore.activeSessionId = sessions.value[0].id;
    }
  }

  await Promise.all([
    events.loadPlanDocumentsFromBackend(),
    events.loadTodosFromBackend(),
    events.loadAgentRunsFromBackend(undefined, { refreshHistory: false }),
    events.loadAgentRunEventsFromBackend(),
    events.loadSubAgentRunsFromBackend(),
    events.loadSubAgentEventsFromBackend(),
    events.loadContextSnapshotFromBackend(),
  ]);

  unlistenRenamed = await listen('session-renamed', () => {
    loadSessions();
  });

  unlistenUpdated = await listen('session-updated', () => {
    loadSessions();
  });
});

onUnmounted(() => {
  if (sessionActionTimer) clearTimeout(sessionActionTimer);
  if (unlistenRenamed) unlistenRenamed();
  if (unlistenUpdated) unlistenUpdated();
});
</script>

<template>
  <div class="sidebar" :class="{ collapsed }">
    <div v-if="!collapsed" class="sidebar-content">
      <div class="sidebar-main">
      <div class="sidebar-section">
        <div class="sidebar-title">
          <span>{{ t('sidebar.sessions') }}</span>
          <div class="session-btn-group">
            <button type="button" class="new-session-btn" @click.stop="createNewSession(false)" :title="t('sidebar.newSession')">
              <svg viewBox="0 0 24 24" width="14" height="14" stroke="currentColor" stroke-width="2" fill="none">
                <line x1="12" y1="5" x2="12" y2="19"></line>
                <line x1="5" y1="12" x2="19" y2="12"></line>
              </svg>
            </button>
            <button type="button" class="new-session-btn sandbox-btn" @click.stop="createNewSession(true)" :title="t('sidebar.newSandboxSession')">
              <svg viewBox="0 0 24 24" width="14" height="14" stroke="currentColor" stroke-width="2" fill="none" stroke-linecap="round" stroke-linejoin="round">
                <path d="M22 19a2 2 0 0 1-2 2H4a2 2 0 0 1-2-2V5a2 2 0 0 1 2-2h5l2 3h9a2 2 0 0 1 2 2z"></path>
              </svg>
            </button>
          </div>
        </div>
        <div
          v-if="sessionActionMessage"
          class="session-feedback"
          :class="sessionActionMessageKind"
        >
          {{ sessionActionMessage }}
        </div>
        <div v-if="showAdvancedSessionFilters" class="session-filter-toggle-row">
          <button type="button" class="session-filter-toggle" :class="{ active: hasActiveSessionFilters() }" @click="showSessionFilters = !showSessionFilters">
            {{ t('sidebar.advancedFilter') }}<span v-if="hasActiveSessionFilters()">{{ t('sidebar.enabled') }}</span>
          </button>
          <button v-if="hasActiveSessionFilters()" type="button" class="session-filter-clear" @click="clearSessionFilters">{{ t('sidebar.clear') }}</button>
        </div>
        <div v-if="showAdvancedSessionFilters && showSessionFilters" class="session-filters">
          <input
            v-model="sessionSearchKeyword"
            class="session-filter-input"
            :placeholder="t('sidebar.searchSession')"
            @keydown.enter.prevent="loadSessions"
            @blur="loadSessions"
          />
          <input
            v-model="sessionFilterTool"
            class="session-filter-input"
            :placeholder="t('sidebar.filterByTool')"
            @keydown.enter.prevent="loadSessions"
            @blur="loadSessions"
          />
          <select v-model="sessionFilterRange" class="session-filter-input" @change="loadSessions">
            <option value="all">{{ t('sidebar.allTime') }}</option>
            <option value="24h">{{ t('sidebar.last24h') }}</option>
            <option value="7d">{{ t('sidebar.last7d') }}</option>
            <option value="30d">{{ t('sidebar.last30d') }}</option>
          </select>
          <label class="session-filter-check">
            <input v-model="sessionFilterHasTools" type="checkbox" @change="loadSessions" />
            <span>{{ t('sidebar.hasToolCalls') }}</span>
          </label>
        </div>
        <div v-if="sessions.length === 0" class="session-empty-state">
          {{ hasActiveSessionFilters() ? t('sidebar.noMatchedSessions') : t('sidebar.noSessions') }}
        </div>
        <ul v-else class="session-list">
          <li
            v-for="session in sessions"
            :key="session.id"
            :class="['session-item', { active: session.id === sessionStore.activeSessionId }]"
            @click="editingSessionId !== session.id && switchToSession(session.id)"
          >
            <div class="session-main-row">
              <svg viewBox="0 0 24 24" width="13" height="13" stroke="currentColor" stroke-width="2" fill="none" class="session-icon">
                <path d="M21 15a2 2 0 0 1-2 2H7l-4 4V5a2 2 0 0 1 2-2h14a2 2 0 0 1 2 2z"></path>
              </svg>
              <span v-if="isSessionRunning(session.id)" class="session-running-dot"></span>
              <input
                v-if="editingSessionId === session.id"
                v-model="editingTitle"
                class="session-title-input"
                maxlength="30"
                @click.stop
                @keydown.enter.prevent="submitRenameSession(session.id)"
                @keydown.esc.prevent="cancelRenameSession"
                @blur="submitRenameSession(session.id)"
              />
              <span v-else class="session-title">{{ session.title }}</span>
              <span v-if="session.workingDirectory" class="sandbox-badge" :title="t('sidebar.sandboxTitle', { path: session.workingDirectory })">
                <svg viewBox="0 0 24 24" width="10" height="10" stroke="currentColor" stroke-width="2" fill="none" stroke-linecap="round" stroke-linejoin="round">
                  <rect x="3" y="11" width="18" height="11" rx="2" ry="2"></rect>
                  <path d="M7 11V7a5 5 0 0 1 10 0v4"></path>
                </svg>
              </span>
              <span class="session-count" v-if="session.messageCount > 0">{{ session.messageCount }}</span>
              <button
                class="rename-btn"
                @click="startRenameSession(session, $event)"
                :title="t('sidebar.rename')"
              >
                <svg viewBox="0 0 24 24" width="11" height="11" stroke="currentColor" stroke-width="2" fill="none">
                  <path d="M12 20h9"></path>
                  <path d="M16.5 3.5a2.12 2.12 0 1 1 3 3L7 19l-4 1 1-4 12.5-12.5z"></path>
                </svg>
              </button>
              <button
                v-if="session.id !== sessionStore.activeSessionId"
                class="delete-btn"
                @click="deleteSession(session.id, $event)"
                :title="t('sidebar.delete')"
              >
                <svg viewBox="0 0 24 24" width="11" height="11" stroke="currentColor" stroke-width="2" fill="none">
                  <line x1="18" y1="6" x2="6" y2="18"></line>
                  <line x1="6" y1="6" x2="18" y2="18"></line>
                </svg>
              </button>
            </div>
            <div class="session-meta-row">
              <span>{{ formatSessionTime(session.updatedAt) }}</span>
              <span v-if="formatSessionTokens(session)">{{ formatSessionTokens(session) }}</span>
              <span v-if="session.workingDirectory">{{ t('sidebar.sandbox') }}</span>
            </div>
          </li>
        </ul>
      </div>

      <div class="sidebar-section" v-if="agent.currentTodos.length > 0">
        <div class="sidebar-title"><span>TASKS</span></div>
        <ul class="todo-list">
          <li v-for="todo in agent.currentTodos" :key="todo.id" :class="['todo-item', todo.status]">
            <span class="todo-icon">
              <svg v-if="todo.status === 'completed'" viewBox="0 0 24 24" width="14" height="14" stroke="currentColor" stroke-width="2" fill="none" stroke-linecap="round" stroke-linejoin="round">
                <polyline points="20 6 9 17 4 12"></polyline>
              </svg>
              <svg v-else-if="todo.status === 'in_progress'" viewBox="0 0 24 24" width="14" height="14" stroke="currentColor" stroke-width="2" fill="none" stroke-linecap="round" stroke-linejoin="round" class="spin">
                <circle cx="12" cy="12" r="3"></circle>
                <path d="M19.4 15a1.65 1.65 0 0 0 .33 1.82l.06.06a2 2 0 0 1 0 2.83 2 2 0 0 1-2.83 0l-.06-.06a1.65 1.65 0 0 0-1.82-.33 1.65 1.65 0 0 0-1 1.51V21a2 2 0 0 1-2 2 2 2 0 0 1-2-2v-.09A1.65 1.65 0 0 0 9 19.4a1.65 1.65 0 0 0-1.82.33l-.06.06a2 2 0 0 1-2.83 0 2 2 0 0 1 0-2.83l.06-.06a1.65 1.65 0 0 0 .33-1.82 1.65 1.65 0 0 0-1.51-1H3a2 2 0 0 1-2-2 2 2 0 0 1 2-2h.09A1.65 1.65 0 0 0 4.6 9a1.65 1.65 0 0 0-.33-1.82l-.06-.06a2 2 0 0 1 0-2.83 2 2 0 0 1 2.83 0l.06.06a1.65 1.65 0 0 0 1.82.33H9a1.65 1.65 0 0 0 1-1.51V3a2 2 0 0 1 2-2 2 2 0 0 1 2 2v.09a1.65 1.65 0 0 0 1 1.51 1.65 1.65 0 0 0 1.82-.33l.06-.06a2 2 0 0 1 2.83 0 2 2 0 0 1 0 2.83l-.06.06a1.65 1.65 0 0 0-.33 1.82V9a1.65 1.65 0 0 0 1.51 1H21a2 2 0 0 1 2 2 2 2 0 0 1-2 2h-.09a1.65 1.65 0 0 0-1.51 1z"></path>
              </svg>
              <svg v-else viewBox="0 0 24 24" width="14" height="14" stroke="currentColor" stroke-width="2" fill="none" stroke-linecap="round" stroke-linejoin="round">
                <circle cx="12" cy="12" r="10"></circle>
              </svg>
            </span>
            <span class="todo-text">
              {{ todo.status === 'in_progress'
                ? (todo.activeForm || todo.content || todo.text)
                : (todo.content || todo.text) }}
            </span>
          </li>
        </ul>
      </div>

      </div>
      <div class="sidebar-footer">
        <button type="button" class="footer-action" @click="emit('open-settings')" :title="t('sidebar.settingsTitle')">
          <svg viewBox="0 0 24 24" width="15" height="15" stroke="currentColor" stroke-width="2" fill="none" stroke-linecap="round" stroke-linejoin="round">
            <circle cx="12" cy="12" r="3"></circle>
            <path d="M19.4 15a1.65 1.65 0 0 0 .33 1.82l.06.06a2 2 0 0 1 0 2.83 2 2 0 0 1-2.83 0l-.06-.06a1.65 1.65 0 0 0-1.82-.33 1.65 1.65 0 0 0-1 1.51V21a2 2 0 0 1-2 2 2 2 0 0 1-2-2v-.09A1.65 1.65 0 0 0 9 19.4a1.65 1.65 0 0 0-1.82.33l-.06.06a2 2 0 0 1-2.83 0 2 2 0 0 1 0-2.83l.06-.06a1.65 1.65 0 0 0 .33-1.82 1.65 1.65 0 0 0-1.51-1H3a2 2 0 0 1-2-2 2 2 0 0 1 2-2h.09A1.65 1.65 0 0 0 4.6 9a1.65 1.65 0 0 0-.33-1.82l-.06-.06a2 2 0 0 1 0-2.83 2 2 0 0 1 2.83 0l.06.06a1.65 1.65 0 0 0 1.82.33H9a1.65 1.65 0 0 0 1-1.51V3a2 2 0 0 1 2-2 2 2 0 0 1 2 2v.09a1.65 1.65 0 0 0 1 1.51 1.65 1.65 0 0 0 1.82-.33l.06-.06a2 2 0 0 1 2.83 0 2 2 0 0 1 0 2.83l-.06.06a1.65 1.65 0 0 0-.33 1.82V9a1.65 1.65 0 0 0 1.51 1H21a2 2 0 0 1 2 2 2 2 0 0 1-2 2h-.09a1.65 1.65 0 0 0-1.51 1z"></path>
          </svg>
          <span>{{ t('sidebar.settings') }}</span>
        </button>
      </div>

    </div>
  </div>
</template>

<style scoped>
.sidebar {
  width: 250px;
  background: var(--glass-bg);
  backdrop-filter: blur(var(--glass-blur));
  -webkit-backdrop-filter: blur(var(--glass-blur));
  border-right: 1px solid var(--glass-border);
  display: flex;
  flex-direction: column;
  overflow: hidden;
  padding: 12px 8px;
  transition: width 0.25s ease, padding 0.25s ease;
  flex-shrink: 0;
}

.sidebar.collapsed {
  width: 0;
  padding: 0;
  border-right: none;
  overflow: hidden;
}

.sidebar-content {
  min-width: 234px;
  height: 100%;
  min-height: 0;
  display: flex;
  flex-direction: column;
}

.sidebar-main {
  flex: 1;
  min-height: 0;
  overflow-y: auto;
  overflow-x: hidden;
  padding-right: 2px;
}

.sidebar-main::-webkit-scrollbar {
  width: 6px;
}

.sidebar-main::-webkit-scrollbar-thumb {
  background: var(--glass-border);
  border-radius: 999px;
}

.sidebar-footer {
  display: flex;
  align-items: center;
  gap: 6px;
  flex-shrink: 0;
  padding: 10px 4px 0;
  border-top: 1px solid var(--glass-border-subtle);
}

.footer-action {
  min-width: 0;
  height: 32px;
  flex: 1;
  display: inline-flex;
  align-items: center;
  justify-content: center;
  gap: 8px;
  border-radius: var(--radius-md);
  border: 1px solid transparent;
  background: transparent;
  color: var(--text-muted);
  cursor: pointer;
  font-size: 0.78rem;
  font-weight: 500;
  transition: all var(--transition-fast);
  -webkit-app-region: no-drag;
}

.footer-action.icon-only {
  flex: 0 0 34px;
}

.footer-action:hover {
  color: var(--text-main);
  background: var(--glass-bg-light);
  border-color: var(--glass-border-subtle);
}

.sidebar-section {
  display: flex;
  flex-direction: column;
  margin-bottom: 16px;
}

.sidebar-section + .sidebar-section {
  border-top: 1px solid var(--glass-border-subtle);
  padding-top: 16px;
}

.sidebar-title {
  font-size: 0.75rem;
  font-weight: 600;
  color: var(--text-muted);
  padding: 4px 12px 8px;
  letter-spacing: 0.05em;
  display: flex;
  justify-content: space-between;
  align-items: center;
  flex-shrink: 0;
  text-transform: uppercase;
}

.session-btn-group {
  display: flex;
  gap: 2px;
}

.session-feedback {
  margin: 0 12px 8px;
  padding: 8px 10px;
  border-radius: var(--radius-md);
  font-size: 0.75rem;
  line-height: 1.4;
  border: 1px solid transparent;
  background: var(--glass-bg-light);
  backdrop-filter: blur(8px);
  -webkit-backdrop-filter: blur(8px);
}
.session-feedback.info {
  color: var(--accent-blue);
  border-color: rgba(59, 130, 246, 0.2);
}
.session-feedback.error {
  color: var(--accent-red);
  border-color: rgba(239, 68, 68, 0.2);
}

.new-session-btn {
  background: var(--glass-bg-light);
  border: 1px solid var(--glass-border-subtle);
  color: var(--text-muted);
  cursor: pointer;
  padding: 4px;
  display: inline-flex;
  align-items: center;
  justify-content: center;
  border-radius: var(--radius-md);
  transition: all var(--transition-fast);
  -webkit-app-region: no-drag;
  backdrop-filter: blur(8px);
  -webkit-backdrop-filter: blur(8px);
}
.new-session-btn:hover {
  color: var(--accent-blue);
  background: var(--glass-bg);
  border-color: var(--glass-border);
  transform: scale(1.05);
}
.sandbox-btn:hover {
  color: var(--accent-green);
  background: rgba(34, 197, 94, 0.1);
  border-color: rgba(34, 197, 94, 0.2);
}

.sandbox-badge {
  display: inline-flex;
  align-items: center;
  margin-left: 4px;
  color: var(--accent-green, #22c55e);
  opacity: 0.7;
  flex-shrink: 0;
}

.session-filter-toggle-row {
  display: flex;
  align-items: center;
  gap: 6px;
  margin: 0 12px 8px;
}

.session-filter-toggle,
.session-filter-clear {
  height: 24px;
  padding: 0 8px;
  border: 1px solid var(--glass-border-subtle);
  border-radius: var(--radius-md);
  background: var(--glass-bg-light);
  color: var(--text-muted);
  font-size: 0.7rem;
  cursor: pointer;
}

.session-filter-toggle {
  flex: 1;
  display: flex;
  align-items: center;
  justify-content: space-between;
}

.session-filter-toggle.active {
  color: var(--accent-blue);
  border-color: var(--accent-blue);
}

.session-filter-clear:hover,
.session-filter-toggle:hover {
  color: var(--text-main);
}

.session-filters {
  display: flex;
  flex-direction: column;
  gap: 6px;
  margin: 0 12px 8px;
  padding: 8px;
  border: 1px solid var(--glass-border-subtle);
  border-radius: var(--radius-md);
  background: var(--glass-bg);
}

.session-empty-state {
  margin: 4px 12px 8px;
  padding: 10px;
  color: var(--text-muted);
  border: 1px dashed var(--glass-border-subtle);
  border-radius: var(--radius-md);
  font-size: 0.74rem;
  text-align: center;
}

.session-filter-input {
  width: 100%;
  border: 1px solid var(--glass-border-subtle);
  background: var(--glass-bg-light);
  color: var(--text-main);
  border-radius: var(--radius-md);
  padding: 6px 8px;
  font-size: 0.75rem;
  outline: none;
}

.session-filter-input:focus {
  border-color: var(--accent-blue);
}

.session-filter-check {
  display: inline-flex;
  align-items: center;
  gap: 6px;
  color: var(--text-muted);
  font-size: 0.72rem;
}

.session-list {
  list-style: none;
  padding: 0;
  margin: 0;
  display: flex;
  flex-direction: column;
  gap: 2px;
}

.session-item {
  padding: 8px 12px;
  font-size: 0.85rem;
  font-weight: 500;
  border-radius: var(--radius-md);
  display: flex;
  flex-direction: column;
  gap: 4px;
  align-items: stretch;
  cursor: pointer;
  color: var(--text-muted);
  transition: all var(--transition-fast);
  position: relative;
  background: transparent;
  border: 1px solid transparent;
}

.session-main-row {
  display: flex;
  align-items: center;
  min-width: 0;
}

.session-meta-row {
  display: flex;
  gap: 8px;
  padding-left: 23px;
  color: var(--text-muted);
  font-size: 0.62rem;
  font-weight: 400;
  white-space: nowrap;
  overflow: hidden;
}

.session-meta-row span {
  overflow: hidden;
  text-overflow: ellipsis;
}
.session-item:hover {
  background: var(--glass-bg-light);
  border-color: var(--glass-border-subtle);
  color: var(--text-main);
}
.session-item.active {
  background: var(--glass-bg);
  border-color: var(--glass-border);
  color: var(--accent-blue);
  font-weight: 600;
  box-shadow: var(--shadow-sm);
}

.session-icon {
  margin-right: 10px;
  flex-shrink: 0;
  opacity: 0.7;
}
.session-item.active .session-icon {
  opacity: 1;
}

.session-running-dot {
  width: 6px;
  height: 6px;
  border-radius: 50%;
  background: var(--accent-yellow);
  flex-shrink: 0;
  margin-right: 4px;
  animation: sessionDotBlink 1.5s ease-in-out infinite;
  box-shadow: 0 0 4px rgba(245, 158, 11, 0.5);
}

@keyframes sessionDotBlink {
  0%, 100% { opacity: 0.3; box-shadow: 0 0 2px rgba(245, 158, 11, 0.3); }
  50% { opacity: 1; box-shadow: 0 0 8px rgba(245, 158, 11, 0.6); }
}

.session-title {
  flex: 1;
  overflow: hidden;
  text-overflow: ellipsis;
  white-space: nowrap;
}

.session-title-input {
  flex: 1;
  min-width: 0;
  background: var(--glass-bg-light);
  border: 1px solid var(--glass-border);
  color: var(--text-main);
  border-radius: var(--radius-sm);
  padding: 4px 8px;
  font-size: 0.82rem;
}

.session-title-input:focus {
  outline: none;
  border-color: var(--accent-blue);
}

.session-count {
  font-size: 0.7rem;
  font-weight: 600;
  background: var(--glass-bg-light);
  color: var(--text-muted);
  padding: 2px 6px;
  border-radius: 12px;
  margin-left: 6px;
  flex-shrink: 0;
  border: 1px solid var(--glass-border-subtle);
}

.delete-btn {
  display: none;
  background: var(--glass-bg-light);
  border: 1px solid var(--glass-border-subtle);
  color: var(--text-muted);
  cursor: pointer;
  padding: 4px;
  margin-left: 4px;
  border-radius: var(--radius-md);
  align-items: center;
  justify-content: center;
  -webkit-app-region: no-drag;
  transition: all var(--transition-fast);
}
.rename-btn {
  display: none;
  background: var(--glass-bg-light);
  border: 1px solid var(--glass-border-subtle);
  color: var(--text-muted);
  cursor: pointer;
  padding: 4px;
  margin-left: 4px;
  border-radius: var(--radius-md);
  align-items: center;
  justify-content: center;
  -webkit-app-region: no-drag;
  transition: all var(--transition-fast);
}
.session-item:hover .delete-btn,
.session-item:hover .rename-btn {
  display: inline-flex;
  animation: fadeIn var(--transition-fast);
}
@keyframes fadeIn {
  from { opacity: 0; transform: scale(0.9); }
  to { opacity: 1; transform: scale(1); }
}
.delete-btn:hover {
  color: var(--accent-red);
  background: rgba(239, 68, 68, 0.15);
  border-color: rgba(239, 68, 68, 0.3);
}
.rename-btn:hover {
  color: var(--accent-blue);
  background: rgba(59, 130, 246, 0.15);
  border-color: rgba(59, 130, 246, 0.3);
}

.todo-list {
  list-style: none;
  padding: 0;
  margin: 0;
  display: flex;
  flex-direction: column;
  gap: 2px;
}

.todo-item {
  padding: 8px 12px;
  font-size: 0.85rem;
  border-radius: var(--radius-md);
  display: flex;
  align-items: flex-start;
  cursor: default;
  transition: all var(--transition-fast);
  background: transparent;
  border: 1px solid transparent;
}
.todo-item:hover {
  background: var(--glass-bg-light);
  border-color: var(--glass-border-subtle);
}

.todo-icon {
  margin-top: 2px;
  margin-right: 10px;
  font-size: 0.9rem;
  display: inline-flex;
  align-items: center;
  justify-content: center;
  flex-shrink: 0;
}

.todo-item.pending { color: var(--text-muted); }
.todo-item.in_progress { color: var(--accent-yellow); font-weight: 500; }
.todo-item.completed { color: var(--accent-green); opacity: 0.7; text-decoration: line-through; }

@keyframes spin {
  100% { transform: rotate(360deg); }
}
.spin {
  animation: spin 2s linear infinite;
}
</style>
