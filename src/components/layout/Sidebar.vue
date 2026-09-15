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
import { ref, onMounted, onUnmounted, computed } from 'vue';
import type { SessionMeta, ProjectMeta } from '../../types';
import { invoke } from '@tauri-apps/api/core';
import { listen } from '@tauri-apps/api/event';
import { open } from '@tauri-apps/plugin-dialog';
import { useSessionStore } from '../../stores/session';
import { useChatStore } from '../../stores/chat';
import { useAppViewStore } from '../../stores/appView';
import { useAgentEvents } from '../../composables/useAgentEvents';
import { loadSessionThinking } from '../../composables/useThinkingMode';
import { useWindow } from '../../composables/useWindow';
import ConfirmModal from '../common/ConfirmModal.vue';

defineProps<{
  collapsed: boolean;
}>();

const emit = defineEmits<{
  (e: 'open-settings'): void;
}>();

const { t } = useI18n();

const sessionStore = useSessionStore();
const chat = useChatStore();
const appView = useAppViewStore();
const events = useAgentEvents();
const { notifyMonitorSessionChanged } = useWindow();

// 项目管理状态
const projects = ref<ProjectMeta[]>([]);
const collapsedProjects = ref<Set<string>>(new Set());

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

// 按项目分组会话
const standaloneSessions = computed(() =>
  sessions.value.filter(s => !s.projectId)
);

const projectSessions = computed(() => {
  const grouped = new Map<string, { project: ProjectMeta; sessions: SessionMeta[] }>();
  for (const p of projects.value) {
    grouped.set(p.id, { project: p, sessions: [] });
  }
  for (const s of sessions.value) {
    if (s.projectId && grouped.has(s.projectId)) {
      grouped.get(s.projectId)!.sessions.push(s);
    }
  }
  return [...grouped.values()];
});

const toggleProject = (projectId: string) => {
  if (collapsedProjects.value.has(projectId)) {
    collapsedProjects.value.delete(projectId);
  } else {
    collapsedProjects.value.add(projectId);
  }
};

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
  await loadAll();
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

const pickDirectory = async () => {
  try {
    const selected = await open({
      directory: true,
      multiple: false,
      title: t('sidebar.selectWorkspace'),
    });
    if (typeof selected === 'string' && selected.trim()) return selected.trim();
    if (Array.isArray(selected) && typeof selected[0] === 'string' && selected[0].trim()) return selected[0].trim();
    return null;
  } catch (dialogErr) {
    console.error('打开目录选择器失败:', dialogErr);
    showSessionActionMessage(t('sidebar.dialogOpenError'), 'error');
    return null;
  }
};

// 加载会话列表和项目列表
const loadAll = async () => {
  await Promise.all([loadSessions(), loadProjects()]);
};

const loadProjects = async () => {
  try {
    projects.value = await invoke<ProjectMeta[]>('list_projects');
  } catch (err) {
    console.error('加载项目列表失败:', err);
  }
};

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

const prepareNewSession = async (projectId: string | null = null) => {
  try {
    sessionStore.activeSessionId = null;
    await invoke('clear_active_session_id');
    try {
      const config = await invoke<any>('get_config');
      if (config.globalProfileId) {
        config.activeProfileId = config.globalProfileId;
        await invoke('save_config_cmd', { newConfig: config });
      }
    } catch { /* ignore */ }
    sessionStore.pendingProjectId = projectId;
    sessionStore.workingDirectory = null;
    sessionStore.resetSessionView(null);
    sessionStore.setSessionUsageTotals(null, 0, 0);
    chat.resetRenderState();
    chat.triggerRender();
    await notifyMonitorSessionChanged(null);
    requestAnimationFrame(() => chat.forceScrollToBottom());
  } catch (err) {
    console.error('准备新会话失败:', err);
    showSessionActionMessage(t('sidebar.newError', { error: formatErrorMessage(err) }), 'error');
  }
};

// 新建独立对话
const createNewSession = async () => {
  await prepareNewSession(null);
};

// 打开项目
const openProject = async () => {
  const dir = await pickDirectory();
  if (!dir) return;
  try {
    const project = await invoke<ProjectMeta>('open_project', { path: dir });
    await loadAll();
    // 自动创建该项目下的新会话
    await prepareNewSession(project.id);
    sessionStore.workingDirectory = project.path;
  } catch (err) {
    console.error('打开项目失败:', err);
    showSessionActionMessage(t('sidebar.newError', { error: formatErrorMessage(err) }), 'error');
  }
};

const openSkillManager = () => {
  appView.showSkillManager();
};

// 在项目下新建对话
const createProjectSession = async (projectId: string) => {
  const project = projects.value.find(p => p.id === projectId);
  await prepareNewSession(projectId);
  if (project) sessionStore.workingDirectory = project.path;
};

// 删除项目
const confirmingDeleteProjectId = ref<string | null>(null);

const deleteProject = (projectId: string, event: Event) => {
  event.stopPropagation();
  if (confirmingDeleteProjectId.value === projectId) {
    performDeleteProject(projectId);
  } else {
    confirmingDeleteProjectId.value = projectId;
    setTimeout(() => {
      if (confirmingDeleteProjectId.value === projectId) {
        confirmingDeleteProjectId.value = null;
      }
    }, 4000);
  }
};

const performDeleteProject = async (projectId: string) => {
  confirmingDeleteProjectId.value = null;
  const activeInProject = sessionStore.activeSessionId
    && sessions.value.find(s => s.id === sessionStore.activeSessionId && s.projectId === projectId);
  try {
    await invoke('delete_project', { id: projectId });
    if (activeInProject) {
      sessionStore.activeSessionId = null;
      sessionStore.workingDirectory = null;
      sessionStore.resetSessionView(null);
      sessionStore.setSessionUsageTotals(null, 0, 0);
      chat.resetRenderState();
      chat.triggerRender();
    }
    await loadAll();
  } catch (err) {
    console.error('删除项目失败:', err);
    showSessionActionMessage(formatErrorMessage(err), 'error');
  }
};

/**
 * 把某会话记录的模型预设同步为**激活预设**（并落库）。
 *
 * 模型预设是会话级状态（`sessions.profile_id`），但后端 pipeline 每次读的是
 * `AppConfig.activeProfileId`。因此**任何"让某会话成为当前会话"的路径都必须调用这里**，
 * 否则后端会继续用上一个会话/全局默认的预设。
 *
 * 这条纪律是踩坑总结：启动恢复会话曾经漏掉这一步（只有点击切会话时同步），
 * 结果重启后模型预设回落到设置里的默认值，而会话里并没有消息能暴露这个不一致。
 */
const syncProfileFromSession = async (meta: {
  profileId?: string | null;
  mainModel?: string | null;
}) => {
  const config = await invoke<any>('get_config');
  if (meta.profileId) {
    config.activeProfileId = meta.profileId;
  } else {
    config.activeProfileId = config.globalProfileId;
  }
  await invoke('save_config_cmd', { newConfig: config });
};

// 切换会话
const switchToSession = async (id: string) => {
  if (id === sessionStore.activeSessionId) return;
  try {
    appView.showChat(); // 切换会话时返回聊天视图
    const meta = await invoke<any>('switch_session', { id });
    sessionStore.activeSessionId = id;
    sessionStore.workingDirectory = meta.workingDirectory || null;
    sessionStore.pendingProjectId = null;

    await syncProfileFromSession(meta);

    // 档位快照里的 profileResolvedDefault 依赖"激活预设"已切换完成，
    // 因此在 profile 落库之后重取一次权威档位，消除与上方 save 的竞态
    //（`activeSessionId` 的 watcher 会先跑一次，这里再对齐一次最终值）。
    void loadSessionThinking(id);

    sessionStore.setSessionUsageTotals(id, meta.totalInputTokens || 0, meta.totalOutputTokens || 0);

    // 切会话只做视角切换，不碰消息和 currentTurn
    // 消息状态由 streaming 事件和 sendToJarvis 收尾自行维护
    await Promise.all([
      events.loadPlanDocumentsFromBackend(id),
      events.loadTodosFromBackend(id),
      events.loadAgentRunsFromBackend(id, { refreshHistory: false }),
      events.loadAgentRunEventsFromBackend(id),
      events.loadSubAgentRunsFromBackend(id),
      events.loadSubAgentEventsFromBackend(id),
      events.loadContextSnapshotFromBackend(id),
    ]);

    const view = sessionStore.getSessionView(id);
    // 首次加载（view 新创建、无消息、无 live turn）时从 DB 初始化
    if (!view.hydrated) {
      try {
        const messages = await invoke<any[]>('get_session_messages', { sessionId: id });
        sessionStore.replaceSessionMessages(id, messages);
      } catch {
        const history = await invoke<string>('get_session_history', { sessionId: id });
        sessionStore.replaceSessionHistory(id, history || 'Ready for input...');
      }
    }

    chat.triggerRender();
    await notifyMonitorSessionChanged(id);
    await loadSessions();
    requestAnimationFrame(() => chat.forceScrollToBottom());
  } catch (err) {
    console.error('切换会话失败:', err);
  }
};

// 删除会话：先进入确认态，确认后才真正执行（硬删除、不可撤销）
//
// 为什么当前会话也可以删：要求"先切走再删"会让**只有一个会话时删除功能完全不可达**，
// 且整理多个会话时被迫来回切换（切换还带改预设、改思考档位的副作用）。
// 安全性改由**显式确认**承担：删除当前会话时文案会额外说明将切换到哪个会话。
const pendingDeleteSession = ref<{ id: string; title: string; isActive: boolean } | null>(null);
const deleteSessionLoading = ref(false);

const pendingDeleteMessage = computed(() => {
  const pending = pendingDeleteSession.value;
  if (!pending) return '';
  if (pending.isActive) {
    const next = sessions.value.find((s) => s.id !== pending.id);
    return t('sidebar.deleteSessionConfirmActive', { next: next?.title || t('sidebar.newSession') });
  }
  return t('sidebar.deleteSessionConfirm', { name: pending.title });
});

const deleteSession = (session: SessionMeta, event: Event) => {
  event.stopPropagation();
  if (isSessionRunning(session.id)) {
    showSessionActionMessage(t('sidebar.deleteRunning'), 'error');
    return;
  }
  pendingDeleteSession.value = {
    id: session.id,
    title: session.title,
    isActive: session.id === sessionStore.activeSessionId,
  };
};

const cancelDeleteSession = () => {
  if (deleteSessionLoading.value) return;
  pendingDeleteSession.value = null;
};

const confirmDeleteSession = async () => {
  const pending = pendingDeleteSession.value;
  if (!pending || deleteSessionLoading.value) return;
  deleteSessionLoading.value = true;
  try {
    // 后端会把 activeSessionId 回落到另一个会话（并同步该会话的模型预设），
    // 前端由 active-session-changed 事件切过去，这里不需要额外处理
    await invoke('delete_session', { id: pending.id });
    await loadSessions();
  } catch (err) {
    console.error('删除会话失败:', err);
    showSessionActionMessage(formatErrorMessage(err), 'error');
  } finally {
    deleteSessionLoading.value = false;
    pendingDeleteSession.value = null;
  }
};

let unlistenRenamed: (() => void) | null = null;
let unlistenUpdated: (() => void) | null = null;

onMounted(async () => {
  await loadAll();

  try {
    const activeId = await invoke<string | null>('get_active_session_id');
    if (activeId) {
      try {
        const switchedMeta = await invoke<any>('switch_session', { id: activeId });
        sessionStore.activeSessionId = activeId;
        const meta = await invoke<any>('get_session_meta', { id: activeId });
        sessionStore.workingDirectory = meta.workingDirectory || null;
        sessionStore.setSessionUsageTotals(activeId, meta.totalInputTokens || 0, meta.totalOutputTokens || 0);

        // 关键：恢复会话时也要把该会话的模型预设同步为激活预设。
        // 漏掉这一步时，后端 pipeline 会继续用 app-config.json 里遗留的
        // activeProfileId（往往是设置里的默认预设），表现为"重启后模型预设不对"。
        await syncProfileFromSession(switchedMeta);
        void loadSessionThinking(activeId);

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
        sessionStore.setSessionUsageTotals(null, 0, 0);
        if (sessions.value.length > 0) {
          const fallbackId = sessions.value[0].id;
          sessionStore.activeSessionId = fallbackId;
          // 回落也要对齐预设，否则后端停在遗留的 activeProfileId（同上）
          await syncProfileFromSession(sessions.value[0]);
          void loadSessionThinking(fallbackId);
        }
      }
    } else if (sessions.value.length > 0) {
      sessionStore.activeSessionId = sessions.value[0].id;
      sessionStore.setSessionUsageTotals(sessions.value[0].id, sessions.value[0].totalInputTokens || 0, sessions.value[0].totalOutputTokens || 0);
      // 回落到首个会话时同样要对齐预设，否则后端还停在全局默认
      await syncProfileFromSession(sessions.value[0]);
      void loadSessionThinking(sessions.value[0].id);
    }
  } catch (err) {
    sessionStore.setSessionUsageTotals(null, 0, 0);
    if (sessions.value.length > 0) {
      const fallbackId = sessions.value[0].id;
      sessionStore.activeSessionId = fallbackId;
      // 最外层兜底路径同样不能漏掉预设对齐（用 meta 兜底取 profileId）
      try {
        const meta = await invoke<any>('get_session_meta', { id: fallbackId });
        await syncProfileFromSession(meta);
      } catch { /* 取不到就保持现有激活预设 */ }
      void loadSessionThinking(fallbackId);
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
    loadAll();
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
        <div class="session-btn-group">
          <button type="button" class="new-session-btn" @click.stop="createNewSession()">
            <svg viewBox="0 0 24 24" width="16" height="16" stroke="currentColor" stroke-width="2" fill="none">
              <line x1="12" y1="5" x2="12" y2="19"></line>
              <line x1="5" y1="12" x2="19" y2="12"></line>
            </svg>
            <span>{{ t('sidebar.newSession') }}</span>
          </button>
          <button type="button" class="new-session-btn project-btn" @click.stop="openProject()">
            <svg viewBox="0 0 24 24" width="16" height="16" stroke="currentColor" stroke-width="2" fill="none" stroke-linecap="round" stroke-linejoin="round">
              <path d="M22 19a2 2 0 0 1-2 2H4a2 2 0 0 1-2-2V5a2 2 0 0 1 2-2h5l2 3h9a2 2 0 0 1 2 2z"></path>
            </svg>
            <span>{{ t('sidebar.openProject') }}</span>
          </button>
          <button type="button" class="new-session-btn skill-btn" @click.stop="openSkillManager()">
            <svg viewBox="0 0 24 24" width="16" height="16" stroke="currentColor" stroke-width="2" fill="none">
              <path d="M13 2L3 14h9l-1 8 10-12h-9l1-8z"></path>
            </svg>
            <span>{{ t('sidebar.skillManager') }}</span>
          </button>
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
        <div v-if="sessions.length === 0 && projects.length === 0" class="session-empty-state">
          {{ hasActiveSessionFilters() ? t('sidebar.noMatchedSessions') : t('sidebar.noSessions') }}
        </div>

        <!-- 项目分组 -->
        <template v-for="group in projectSessions" :key="group.project.id">
          <div class="project-header" @click="toggleProject(group.project.id)">
            <svg
              viewBox="0 0 24 24" width="8" height="8" stroke="currentColor" stroke-width="2.5" fill="none"
              class="project-chevron"
              :class="{ collapsed: collapsedProjects.has(group.project.id) }"
            >
              <polyline points="9 18 15 12 9 6"></polyline>
            </svg>
            <svg viewBox="0 0 24 24" width="12" height="12" stroke="currentColor" stroke-width="2" fill="none" stroke-linecap="round" stroke-linejoin="round" class="project-icon">
              <path d="M22 19a2 2 0 0 1-2 2H4a2 2 0 0 1-2-2V5a2 2 0 0 1 2-2h5l2 3h9a2 2 0 0 1 2 2z"></path>
            </svg>
            <span class="project-name">{{ group.project.name }}</span>
            <span class="project-count">{{ group.sessions.length }}</span>
            <button class="project-new-btn" @click.stop="createProjectSession(group.project.id)" :title="t('sidebar.newProjectSession')">
              <svg viewBox="0 0 24 24" width="12" height="12" stroke="currentColor" stroke-width="2" fill="none">
                <line x1="12" y1="5" x2="12" y2="19"></line>
                <line x1="5" y1="12" x2="19" y2="12"></line>
              </svg>
            </button>
            <button
              class="project-delete-btn"
              :class="{ confirming: confirmingDeleteProjectId === group.project.id }"
              @click="deleteProject(group.project.id, $event)"
              :title="confirmingDeleteProjectId === group.project.id ? t('common.confirm') : t('sidebar.deleteProject')"
            >
              <svg viewBox="0 0 24 24" width="11" height="11" stroke="currentColor" stroke-width="2" fill="none">
                <line x1="18" y1="6" x2="6" y2="18"></line>
                <line x1="6" y1="6" x2="18" y2="18"></line>
              </svg>
              <span v-if="confirmingDeleteProjectId === group.project.id" class="confirm-label">{{ t('common.confirm') }}</span>
            </button>
          </div>
          <Transition name="project-expand">
            <ul v-if="!collapsedProjects.has(group.project.id)" class="session-list project-session-list">
              <li
                v-for="session in group.sessions"
              :key="session.id"
              :class="['session-item', { active: session.id === sessionStore.activeSessionId }]"
              @click="editingSessionId !== session.id && switchToSession(session.id)"
            >
              <div class="session-main-row">
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
                                <button class="rename-btn" @click="startRenameSession(session, $event)" :title="t('sidebar.rename')">
                  <svg viewBox="0 0 24 24" width="11" height="11" stroke="currentColor" stroke-width="2" fill="none">
                    <path d="M12 20h9"></path>
                    <path d="M16.5 3.5a2.12 2.12 0 1 1 3 3L7 19l-4 1 1-4 12.5-12.5z"></path>
                  </svg>
                </button>
                <button
                  class="delete-btn"
                  @click="deleteSession(session, $event)"
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
              </div>
            </li>
          </ul>
          </Transition>
        </template>

        <!-- 独立会话 -->
        <div class="project-header standalone-header">
          <svg viewBox="0 0 24 24" width="13" height="13" stroke="currentColor" stroke-width="2" fill="none" class="project-icon">
            <path d="M21 15a2 2 0 0 1-2 2H7l-4 4V5a2 2 0 0 1 2-2h14a2 2 0 0 1 2 2z"></path>
          </svg>
          <span class="project-name">{{ t('sidebar.standaloneSessions') }}</span>
          <span class="project-count">{{ standaloneSessions.length }}</span>
        </div>
        <ul v-if="standaloneSessions.length > 0" class="session-list">
          <li
            v-for="session in standaloneSessions"
            :key="session.id"
            :class="['session-item', { active: session.id === sessionStore.activeSessionId }]"
            @click="editingSessionId !== session.id && switchToSession(session.id)"
          >
            <div class="session-main-row">
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
                            <button class="rename-btn" @click="startRenameSession(session, $event)" :title="t('sidebar.rename')">
                <svg viewBox="0 0 24 24" width="11" height="11" stroke="currentColor" stroke-width="2" fill="none">
                  <path d="M12 20h9"></path>
                  <path d="M16.5 3.5a2.12 2.12 0 1 1 3 3L7 19l-4 1 1-4 12.5-12.5z"></path>
                </svg>
              </button>
              <button
                class="delete-btn"
                @click="deleteSession(session, $event)"
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
            </div>
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

      <!-- 删除会话确认：硬删除不可撤销，用显式确认替代"隐藏按钮"式的隐式防护 -->
      <ConfirmModal
        :open="pendingDeleteSession !== null"
        :title="t('sidebar.delete')"
        :message="pendingDeleteMessage"
        :confirm-text="t('sidebar.delete')"
        confirm-kind="danger"
        :loading="deleteSessionLoading"
        @confirm="confirmDeleteSession"
        @cancel="cancelDeleteSession"
      />
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
  flex-direction: column;
  gap: 8px;
  margin: 12px 12px;
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
  border-color: color-mix(in srgb, var(--accent-blue) 20%, transparent);
}
.session-feedback.error {
  color: var(--accent-red);
  border-color: color-mix(in srgb, var(--accent-red) 20%, transparent);
}

.new-session-btn {
  background: transparent;
  border: 1px dashed var(--glass-border-subtle);
  color: var(--text-main);
  cursor: pointer;
  padding: 10px 12px;
  display: flex;
  align-items: center;
  gap: 8px;
  width: 100%;
  border-radius: var(--radius-md);
  font-size: 0.85rem;
  font-weight: 500;
  transition: all var(--transition-fast);
  -webkit-app-region: no-drag;
  text-align: left;
}

.new-session-btn span {
  overflow: hidden;
  text-overflow: ellipsis;
  white-space: nowrap;
}
.new-session-btn:hover,
.project-btn:hover {
  background: var(--glass-bg-light);
  border: 1px solid var(--glass-border);
  box-shadow: 0 4px 12px rgba(0, 0, 0, 0.05);
}

.project-header {
  display: flex;
  align-items: center;
  gap: 4px;
  padding: 8px 12px 8px 0;
  margin: 6px 0 2px;
  border-radius: var(--radius-md);
  cursor: pointer;
  color: var(--text-muted);
  font-size: 0.85rem;
  font-weight: 600;
  transition: all var(--transition-fast);
  border: 1px solid transparent;
}

body.dark-mode .project-header {
  color: rgba(255, 255, 255, 0.7);
}

.standalone-header {
  cursor: default;
  font-weight: 400 !important;
}

.project-header:not(.standalone-header):hover {
  background: var(--glass-bg-light);
  border-color: var(--glass-border-subtle);
  color: var(--text-main);
}

.project-chevron {
  flex-shrink: 0;
  transition: transform 0.2s ease;
}

.project-chevron.collapsed {
  transform: rotate(0deg);
}

.project-chevron:not(.collapsed) {
  transform: rotate(90deg);
}

.project-icon {
  flex-shrink: 0;
  opacity: 0.7;
}

.project-name {
  flex: 1;
  overflow: hidden;
  text-overflow: ellipsis;
  white-space: nowrap;
}

.project-count {
  font-size: 0.65rem;
  background: var(--glass-bg-light);
  color: var(--text-muted);
  padding: 1px 6px;
  border-radius: 10px;
  border: 1px solid var(--glass-border-subtle);
}

.project-new-btn,
.project-delete-btn {
  display: none;
  background: var(--glass-bg-light);
  border: 1px solid var(--glass-border-subtle);
  color: var(--text-muted);
  cursor: pointer;
  padding: 2px;
  border-radius: var(--radius-md);
  align-items: center;
  justify-content: center;
  transition: all var(--transition-fast);
}

.project-header:not(.standalone-header):hover .project-new-btn,
.project-header:not(.standalone-header):hover .project-delete-btn {
  display: inline-flex;
}

.project-delete-btn.confirming {
  display: inline-flex;
  color: var(--accent-red);
  background: color-mix(in srgb, var(--accent-red) 12%, transparent);
  border-color: color-mix(in srgb, var(--accent-red) 25%, transparent);
  gap: 3px;
  padding: 1px 6px;
  border-radius: var(--radius-md);
  font-size: 0.65rem;
  font-weight: 600;
  line-height: 1;
  height: 17px;
  box-sizing: border-box;
}

.project-delete-btn.confirming:hover {
  background: color-mix(in srgb, var(--accent-red) 20%, transparent);
  border-color: color-mix(in srgb, var(--accent-red) 35%, transparent);
}

.confirm-label {
  white-space: nowrap;
  line-height: 1;
}

.project-new-btn:hover,
.project-delete-btn:not(.confirming):hover {
  color: var(--text-main);
  background: color-mix(in srgb, var(--text-muted) 14%, transparent);
  border-color: color-mix(in srgb, var(--text-muted) 22%, transparent);
}

.project-session-list {
  margin-left: 20px;
  border-left: 1px solid var(--glass-border-subtle);
  padding-left: 12px;
}

.project-expand-enter-active,
.project-expand-leave-active {
  transition: opacity 0.3s ease, transform 0.3s ease;
  transform-origin: top;
}
.project-expand-enter-from,
.project-expand-leave-to {
  opacity: 0;
  transform: translateY(-10px);
}

.project-new-session-item {
  padding: 4px 12px;
}

.project-new-session-btn {
  display: flex;
  align-items: center;
  gap: 8px;
  width: 100%;
  padding: 6px 0;
  border: 1px dashed transparent;
  border-radius: var(--radius-md);
  background: transparent;
  color: var(--text-muted);
  cursor: pointer;
  font-size: 0.78rem;
  transition: all var(--transition-fast);
}

.project-new-session-btn:hover {
  color: var(--text-main);
  border-color: var(--glass-border-subtle);
}

.standalone-header {
  cursor: default;
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
  padding: 0 0 0 12px;
  margin: 0;
  display: flex;
  flex-direction: column;
  gap: 2px;
}

.session-item {
  padding: 6px 14px 6px 26px;
  font-size: 0.78rem;
  font-weight: 400;
  border-radius: var(--radius-md);
  display: flex;
  flex-direction: column;
  gap: 2px;
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
  min-height: 24px;
}

.session-meta-row {
  display: flex;
  gap: 8px;
  padding-left: 0;
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
  color: #0f172a;
  font-weight: 600;
  box-shadow: var(--shadow-sm);
}

body.dark-mode .session-item.active {
  color: rgba(255, 255, 255, 0.95);
}

.session-running-dot {
  width: 6px;
  height: 6px;
  border-radius: 50%;
  background: var(--text-muted);
  flex-shrink: 0;
  margin-right: 4px;
  animation: sessionDotBlink 1.5s ease-in-out infinite;
}

@keyframes sessionDotBlink {
  0%, 100% { opacity: 0.35; }
  50% { opacity: 1; }
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
  background: color-mix(in srgb, var(--accent-red) 15%, transparent);
  border-color: color-mix(in srgb, var(--accent-red) 30%, transparent);
}
.rename-btn:hover {
  color: var(--accent-blue);
  background: color-mix(in srgb, var(--accent-blue) 15%, transparent);
  border-color: color-mix(in srgb, var(--accent-blue) 30%, transparent);
}

@keyframes spin {
  100% { transform: rotate(360deg); }
}
.spin {
  animation: spin 2s linear infinite;
}
</style>
