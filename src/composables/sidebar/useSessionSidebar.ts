/**
 * # useSessionSidebar.ts — 侧边栏项目与会话管理组合式逻辑
 *
 * 承接 Sidebar.vue 的全部非视图编排逻辑：项目与会话列表加载、折叠偏好双向绑定与
 * 旧版 localStorage 一次性迁移、启动会话恢复、切会话与模型预设对齐、项目/会话重命名与归档，
 * 以及 `session-renamed` / `session-updated` 后端事件订阅与卸载清理。
 *
 * ## Key Exports
 * - `useSessionSidebar()`: 返回侧边栏所需的响应式数据、派生分组、状态灯与操作方法
 *
 * ## Dependencies
 * - Internal: `@/api`, `@/stores/*`, `@/composables/*`, `@/utils/*`
 * - External: `@tauri-apps/plugin-dialog`, `vue`, `vue-i18n`
 *
 * ## Constraints
 * - 卸载时通过 `disposed` 守卫防止异步 `listenTyped` 晚到造成监听泄漏
 */
import { computed, onMounted, onUnmounted, ref, watch } from "vue";
import { useI18n } from "vue-i18n";
import { open } from "@tauri-apps/plugin-dialog";
import type { ProjectMeta, SessionMeta } from "../../types";
import {
  archiveProject,
  clearActiveSessionId,
  countDeletedSessions,
  deleteSession as deleteSessionApi,
  getActiveSessionId,
  getConfig,
  getSessionMeta,
  listProjects,
  listSessions,
  listenTyped,
  openProject as openProjectApi,
  renameProject,
  renameSession,
  saveConfig,
  switchSession,
} from "../../api";
import { useSessionStore } from "../../stores/session";
import { useChatStore } from "../../stores/chat";
import { usePermissionStore } from "../../stores/permission";
import { useAppViewStore } from "../../stores/appView";
import { useAgentEvents } from "../useAgentEvents";
import { loadSessionThinking } from "../useThinkingMode";
import { useWindow } from "../useWindow";
import { usePreferences, prefsReady } from "../usePreferences";
import { showToast } from "../useToast";
import { extractCommandErrorMessage } from "../../utils/commandError";
import { formatTokenCount } from "../../utils/tokenDisplay";

const SIDEBAR_COLLAPSED_KEY = "jarvis_sidebar_collapsed";

export function useSessionSidebar() {
  const { t } = useI18n();
  const sessionStore = useSessionStore();
  const chat = useChatStore();
  // 待决策权限是「会话级」数据，侧边栏要据此把运行点切成琥珀色
  const perm = usePermissionStore();
  const appView = useAppViewStore();
  const events = useAgentEvents();
  const { notifyMonitorSessionChanged } = useWindow();
  const prefs = usePreferences();

  // 项目管理状态
  const projects = ref<ProjectMeta[]>([]);
  // 折叠状态统一落库到 UiPreferences（data/app-config.json），不再用 localStorage
  const collapsedProjects = computed(
    () => new Set(prefs.sidebarCollapsedProjectIds),
  );
  const projectsSectionCollapsed = computed({
    get: () => prefs.sidebarProjectsCollapsed,
    set: (val: boolean) => prefs.setSidebarProjectsCollapsed(val),
  });
  const standaloneCollapsed = computed({
    get: () => prefs.sidebarStandaloneCollapsed,
    set: (val: boolean) => prefs.setSidebarStandaloneCollapsed(val),
  });

  // 项目行内重命名
  const editingProjectId = ref<string | null>(null);
  const editingProjectName = ref("");

  // 旧版 localStorage 折叠状态一次性迁移至后端偏好（只在后端尚无值时写入）
  watch(
    prefsReady,
    (ready) => {
      if (!ready) return;
      const savedRaw = localStorage.getItem(SIDEBAR_COLLAPSED_KEY);
      if (!savedRaw) return;
      try {
        const saved = JSON.parse(savedRaw);
        const hasBackendState =
          prefs.sidebarProjectsCollapsed ||
          prefs.sidebarStandaloneCollapsed ||
          prefs.sidebarCollapsedProjectIds.length > 0;
        if (!hasBackendState) {
          prefs.setSidebarProjectsCollapsed(!!saved.projects);
          prefs.setSidebarStandaloneCollapsed(!!saved.standalone);
          if (Array.isArray(saved.projectIds)) {
            prefs.setSidebarCollapsedProjectIds(
              saved.projectIds.filter((id: unknown): id is string => typeof id === "string"),
            );
          }
        }
      } catch {
        // 旧值解析失败就忽略，保持后端默认展开状态
      }
      localStorage.removeItem(SIDEBAR_COLLAPSED_KEY);
    },
    { immediate: true },
  );

  // 会话管理状态
  const sessions = ref<SessionMeta[]>([]);
  const editingSessionId = ref<string | null>(null);
  const editingTitle = ref("");
  const archivedCount = ref(0);
  const archiving = ref(false);
  const confirmingDeleteProjectId = ref<string | null>(null);
  let deleteProjectConfirmTimer: ReturnType<typeof setTimeout> | null = null;

  // 按项目分组会话
  const standaloneSessions = computed(() =>
    sessions.value.filter((s) => !s.projectId),
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
    const next = new Set(prefs.sidebarCollapsedProjectIds);
    if (next.has(projectId)) next.delete(projectId);
    else next.add(projectId);
    prefs.setSidebarCollapsedProjectIds([...next]);
  };

  // 启动恢复或切会话时自动展开所属项目/独立会话目录，按 activeSessionId 去重避免干预手动折叠
  let lastRevealedSessionId: string | null = null;
  watch(
    [() => sessionStore.activeSessionId, sessions, projects, prefsReady],
    ([activeId, sessionList, projectList, ready]) => {
      if (!ready || !activeId || activeId === lastRevealedSessionId) return;
      const activeSession = sessionList.find((s) => s.id === activeId);
      if (!activeSession) return;
      lastRevealedSessionId = activeId;

      const projectId = activeSession.projectId;
      if (projectId && projectList.some((p) => p.id === projectId)) {
        if (projectsSectionCollapsed.value) {
          projectsSectionCollapsed.value = false;
        }
        if (collapsedProjects.value.has(projectId)) {
          const next = prefs.sidebarCollapsedProjectIds.filter((id) => id !== projectId);
          prefs.setSidebarCollapsedProjectIds(next);
        }
      } else if (standaloneCollapsed.value) {
        standaloneCollapsed.value = false;
      }
    },
    { immediate: true },
  );

  const isSessionRunning = (sessionId: string): boolean => {
    return sessionStore.sessionViews[sessionId]?.status === "RUNNING";
  };

  /** 会话行状态灯优先级：等待授权 > 执行中 > 异常 */
  const sessionLightStates = computed<Record<string, "running" | "waiting" | "error">>(() => {
    const states: Record<string, "running" | "waiting" | "error"> = {};
    for (const session of sessions.value) {
      if (perm.isSessionWaitingForUser(session.id)) {
        states[session.id] = "waiting";
        continue;
      }
      const status = sessionStore.sessionViews[session.id]?.status;
      if (status === "RUNNING") states[session.id] = "running";
      else if (status === "ERROR") states[session.id] = "error";
    }
    return states;
  });

  const formatSessionTime = (timestamp: number) => {
    const date = new Date(timestamp);
    return `${date.getMonth() + 1}/${date.getDate()} ${date.getHours().toString().padStart(2, "0")}:${date.getMinutes().toString().padStart(2, "0")}`;
  };

  const formatSessionTokens = (session: SessionMeta) => {
    const total = (session.totalInputTokens || 0) + (session.totalOutputTokens || 0);
    if (!total) return "";
    return `${formatTokenCount(total)} tok`;
  };

  const showSessionActionMessage = (message: string, kind: "info" | "error" = "info") => {
    showToast(message, kind);
  };

  const formatErrorMessage = (err: unknown) => extractCommandErrorMessage(err);

  const pickDirectory = async () => {
    try {
      const selected = await open({
        directory: true,
        multiple: false,
        title: t("sidebar.selectWorkspace"),
      });
      if (typeof selected === "string" && selected.trim()) return selected.trim();
      if (Array.isArray(selected) && typeof selected[0] === "string" && selected[0].trim()) {
        return selected[0].trim();
      }
      return null;
    } catch (dialogErr) {
      console.error("打开目录选择器失败:", dialogErr);
      showSessionActionMessage(t("sidebar.dialogOpenError"), "error");
      return null;
    }
  };

  const loadProjects = async () => {
    try {
      projects.value = await listProjects();
    } catch (err) {
      console.error("加载项目列表失败:", err);
    }
  };

  const loadArchivedCount = async () => {
    try {
      archivedCount.value = await countDeletedSessions();
    } catch (err) {
      console.error("加载归档会话数量失败:", err);
    }
  };

  const loadSessions = async () => {
    try {
      sessions.value = await listSessions();
    } catch (err) {
      console.error("加载会话列表失败:", err);
    }
  };

  const loadAll = async () => {
    await Promise.all([loadSessions(), loadProjects(), loadArchivedCount()]);
  };

  const openArchive = () => {
    appView.showArchive();
  };

  const openExtensions = () => {
    appView.toggleExtensions();
  };

  const startRenameSession = (session: SessionMeta, event: Event) => {
    event.stopPropagation();
    editingSessionId.value = session.id;
    editingTitle.value = session.title;
  };

  const cancelRenameSession = () => {
    editingSessionId.value = null;
    editingTitle.value = "";
  };

  const submitRenameSession = async (sessionId: string) => {
    const title = editingTitle.value.trim();
    if (!title) {
      showSessionActionMessage(t("sidebar.renameRequired"), "error");
      return;
    }
    try {
      await renameSession(sessionId, title);
      cancelRenameSession();
      await loadSessions();
      showSessionActionMessage(t("sidebar.renamed"));
    } catch (err) {
      console.error("重命名会话失败:", err);
      showSessionActionMessage(t("sidebar.renameError", { error: formatErrorMessage(err) }), "error");
    }
  };

  const startRenameProject = (project: ProjectMeta, event: Event) => {
    event.stopPropagation();
    editingProjectId.value = project.id;
    editingProjectName.value = project.name;
  };

  const cancelRenameProject = () => {
    editingProjectId.value = null;
    editingProjectName.value = "";
  };

  const submitRenameProject = async (projectId: string) => {
    // 去重守卫：enter 与 blur 可能接连触发，取消后也不该再提交
    if (editingProjectId.value !== projectId) return;
    const name = editingProjectName.value.trim();
    if (!name) {
      showSessionActionMessage(t("sidebar.renameRequired"), "error");
      return;
    }
    cancelRenameProject();
    try {
      await renameProject(projectId, name);
      await loadProjects();
      showSessionActionMessage(t("sidebar.renamedProject"));
    } catch (err) {
      console.error("重命名项目失败:", err);
      showSessionActionMessage(
        t("sidebar.renameProjectError", { error: formatErrorMessage(err) }),
        "error",
      );
    }
  };

  const prepareNewSession = async (projectId: string | null = null) => {
    appView.showChat();
    try {
      sessionStore.activeSessionId = null;
      await clearActiveSessionId();
      try {
        const config = await getConfig();
        if (config.globalProfileId) {
          config.activeProfileId = config.globalProfileId;
          await saveConfig(config);
        }
      } catch {
        /* ignore */
      }
      sessionStore.pendingProjectId = projectId;
      sessionStore.workingDirectory = null;
      sessionStore.resetSessionView(null);
      sessionStore.setSessionUsageTotals(null, {});
      chat.resetRenderState();
      chat.triggerRender();
      await notifyMonitorSessionChanged(null);
      requestAnimationFrame(() => chat.forceScrollToBottom());
    } catch (err) {
      console.error("准备新会话失败:", err);
      showSessionActionMessage(t("sidebar.newError", { error: formatErrorMessage(err) }), "error");
    }
  };

  const createNewSession = async () => {
    await prepareNewSession(null);
  };

  const openProject = async () => {
    const dir = await pickDirectory();
    if (!dir) return;
    try {
      const project = await openProjectApi(dir);
      await loadAll();
      await prepareNewSession(project.id);
      sessionStore.workingDirectory = project.path;
    } catch (err) {
      console.error("打开项目失败:", err);
      showSessionActionMessage(t("sidebar.newError", { error: formatErrorMessage(err) }), "error");
    }
  };

  const createProjectSession = async (projectId: string) => {
    const project = projects.value.find((p) => p.id === projectId);
    await prepareNewSession(projectId);
    if (project) sessionStore.workingDirectory = project.path;
  };

  const performDeleteProject = async (projectId: string) => {
    confirmingDeleteProjectId.value = null;
    if (deleteProjectConfirmTimer !== null) {
      clearTimeout(deleteProjectConfirmTimer);
      deleteProjectConfirmTimer = null;
    }
    try {
      await archiveProject(projectId);
      await loadAll();
    } catch (err) {
      console.error("归档项目失败:", err);
      showSessionActionMessage(formatErrorMessage(err), "error");
    }
  };

  const deleteProject = (projectId: string, event: Event) => {
    event.stopPropagation();
    if (confirmingDeleteProjectId.value === projectId) {
      void performDeleteProject(projectId);
    } else {
      confirmingDeleteProjectId.value = projectId;
      if (deleteProjectConfirmTimer !== null) {
        clearTimeout(deleteProjectConfirmTimer);
      }
      deleteProjectConfirmTimer = setTimeout(() => {
        if (confirmingDeleteProjectId.value === projectId) {
          confirmingDeleteProjectId.value = null;
        }
        deleteProjectConfirmTimer = null;
      }, 4000);
    }
  };

  /**
   * 兜底对齐激活模型预设：正常 `switch_session` 已由后端对齐并广播 `config-updated`，
   * 同值一律不写盘，仅在未经过后端对齐的回落路径补齐。
   */
  const syncProfileFromSession = async (meta: {
    profileId?: string | null;
    mainModel?: string | null;
  }) => {
    try {
      const config = await getConfig();
      const target = meta.profileId || config.globalProfileId;
      if (!target || config.activeProfileId === target) return;
      config.activeProfileId = target;
      await saveConfig(config);
    } catch (err) {
      console.warn("对齐会话预设失败:", err);
    }
  };

  const switchToSession = async (id: string) => {
    appView.showChat();
    if (id === sessionStore.activeSessionId) return;
    try {
      const meta = await switchSession(id);
      sessionStore.activeSessionId = id;
      sessionStore.workingDirectory = meta.workingDirectory || null;
      sessionStore.pendingProjectId = null;

      await syncProfileFromSession(meta);
      void loadSessionThinking(id);

      sessionStore.setSessionUsageTotals(id, {
        input: meta.totalInputTokens || 0,
        output: meta.totalOutputTokens || 0,
        cacheHit: meta.totalCacheHitTokens || 0,
        cacheMiss: meta.totalCacheMissTokens || 0,
      });

      await Promise.all([
        events.loadPlanDocumentsFromBackend(id),
        events.loadTodosFromBackend(id),
        events.loadAgentRunsFromBackend(id, { refreshHistory: false }),
        events.loadAgentRunEventsFromBackend(id),
        events.loadSubAgentRunsFromBackend(id),
        events.loadContextSnapshotFromBackend(id),
      ]);

      try {
        await chat.ensureSessionViewLoaded(id, "session-switch");
      } catch (err) {
        console.error("加载会话消息失败:", err);
        sessionStore.setSessionLoadError(id, true);
      }

      chat.triggerRender();
      await notifyMonitorSessionChanged(id);
      await loadSessions();
      requestAnimationFrame(() => chat.forceScrollToBottom());
    } catch (err) {
      console.error("切换会话失败:", err);
    }
  };

  const deleteSession = async (session: SessionMeta, event: Event) => {
    event.stopPropagation();
    if (archiving.value) return;
    if (isSessionRunning(session.id)) {
      showSessionActionMessage(t("sidebar.deleteRunning"), "error");
      return;
    }
    archiving.value = true;
    try {
      await deleteSessionApi(session.id);
      await loadSessions();
      showSessionActionMessage(t("sidebar.archived"));
    } catch (err) {
      console.error("归档会话失败:", err);
      showSessionActionMessage(formatErrorMessage(err), "error");
    } finally {
      archiving.value = false;
    }
  };

  let disposed = false;
  let unlistenRenamed: (() => void) | null = null;
  let unlistenUpdated: (() => void) | null = null;

  onMounted(async () => {
    await loadAll();
    if (disposed) return;

    try {
      const activeId = await getActiveSessionId();
      if (disposed) return;
      if (activeId) {
        try {
          const switchedMeta = await switchSession(activeId);
          sessionStore.activeSessionId = activeId;
          const meta = await getSessionMeta(activeId);
          sessionStore.workingDirectory = meta.workingDirectory || null;
          sessionStore.setSessionUsageTotals(activeId, {
            input: meta.totalInputTokens || 0,
            output: meta.totalOutputTokens || 0,
            cacheHit: meta.totalCacheHitTokens || 0,
            cacheMiss: meta.totalCacheMissTokens || 0,
          });

          await syncProfileFromSession(switchedMeta);
          void loadSessionThinking(activeId);

          try {
            await chat.ensureSessionViewLoaded(activeId, "app-start");
          } catch (err) {
            console.error("加载会话消息失败:", err);
            sessionStore.setSessionLoadError(activeId, true);
          }
        } catch (switchErr) {
          console.error("同步会话状态失败:", switchErr);
          sessionStore.setSessionUsageTotals(null, {});
          if (sessions.value.length > 0) {
            const fallbackId = sessions.value[0].id;
            sessionStore.activeSessionId = fallbackId;
            await syncProfileFromSession(sessions.value[0]);
            void loadSessionThinking(fallbackId);
          }
        }
      } else if (sessions.value.length > 0) {
        sessionStore.activeSessionId = sessions.value[0].id;
        sessionStore.setSessionUsageTotals(sessions.value[0].id, {
          input: sessions.value[0].totalInputTokens || 0,
          output: sessions.value[0].totalOutputTokens || 0,
          cacheHit: sessions.value[0].totalCacheHitTokens || 0,
          cacheMiss: sessions.value[0].totalCacheMissTokens || 0,
        });
        await syncProfileFromSession(sessions.value[0]);
        void loadSessionThinking(sessions.value[0].id);
      }
    } catch {
      sessionStore.setSessionUsageTotals(null, {});
      if (sessions.value.length > 0) {
        const fallbackId = sessions.value[0].id;
        sessionStore.activeSessionId = fallbackId;
        try {
          const meta = await getSessionMeta(fallbackId);
          await syncProfileFromSession(meta);
        } catch {
          /* 取不到就保持现有激活预设 */
        }
        void loadSessionThinking(fallbackId);
      }
    }

    if (disposed) return;
    await Promise.all([
      events.loadPlanDocumentsFromBackend(),
      events.loadTodosFromBackend(),
      events.loadAgentRunsFromBackend(undefined, { refreshHistory: false }),
      events.loadAgentRunEventsFromBackend(),
      events.loadSubAgentRunsFromBackend(),
      events.loadContextSnapshotFromBackend(),
    ]);

    if (disposed) return;
    const stopRenamed = await listenTyped("session-renamed", () => {
      void loadAll();
    });
    if (disposed) {
      stopRenamed();
      return;
    }
    unlistenRenamed = stopRenamed;

    const stopUpdated = await listenTyped("session-updated", () => {
      // 恢复会话可能同时复活归档项目，三数据源同批刷新避免项目分组缺失
      void loadAll();
    });
    if (disposed) {
      stopUpdated();
      return;
    }
    unlistenUpdated = stopUpdated;
  });

  onUnmounted(() => {
    disposed = true;
    if (deleteProjectConfirmTimer !== null) {
      clearTimeout(deleteProjectConfirmTimer);
      deleteProjectConfirmTimer = null;
    }
    if (unlistenRenamed) {
      unlistenRenamed();
      unlistenRenamed = null;
    }
    if (unlistenUpdated) {
      unlistenUpdated();
      unlistenUpdated = null;
    }
  });

  return {
    sessionStore,
    appView,
    projects,
    sessions,
    archivedCount,
    collapsedProjects,
    projectsSectionCollapsed,
    standaloneCollapsed,
    editingProjectId,
    editingProjectName,
    editingSessionId,
    editingTitle,
    standaloneSessions,
    projectSessions,
    sessionLightStates,
    confirmingDeleteProjectId,
    toggleProject,
    formatSessionTime,
    formatSessionTokens,
    openArchive,
    openExtensions,
    startRenameSession,
    cancelRenameSession,
    submitRenameSession,
    startRenameProject,
    cancelRenameProject,
    submitRenameProject,
    createNewSession,
    openProject,
    createProjectSession,
    deleteProject,
    switchToSession,
    deleteSession,
  };
}
