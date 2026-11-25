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
import { ref, onMounted, onUnmounted, computed, watch } from 'vue';
import type { SessionMeta, ProjectMeta } from '../../types';
import { invoke } from '@tauri-apps/api/core';
import { listen } from '@tauri-apps/api/event';
import { open } from '@tauri-apps/plugin-dialog';
import { useSessionStore } from '../../stores/session';
import { useChatStore } from '../../stores/chat';
import { usePermissionStore } from '../../stores/permission';
import { useAppViewStore } from '../../stores/appView';
import { useAgentEvents } from '../../composables/useAgentEvents';
import { loadSessionThinking } from '../../composables/useThinkingMode';
import { useWindow } from '../../composables/useWindow';
import { usePreferences, prefsReady } from '../../composables/usePreferences';
import { showToast } from '../../composables/useToast';
import { formatTokenCount } from '../../utils/tokenDisplay';

defineProps<{
  collapsed: boolean;
}>();

const emit = defineEmits<{
  (e: 'open-settings'): void;
}>();

const { t } = useI18n();

const sessionStore = useSessionStore();
const chat = useChatStore();
// 待决策权限是「会话级」数据，侧边栏要据此把运行点切成琥珀色
const perm = usePermissionStore();
const appView = useAppViewStore();
const events = useAgentEvents();
const { notifyMonitorSessionChanged } = useWindow();

// 项目管理状态
const projects = ref<ProjectMeta[]>([]);
// 折叠状态统一落库到 UiPreferences（data/app-config.json），不再用 localStorage：
// 它确实是“用户的界面偏好”，换机器/清缓存不该丢失。
const prefs = usePreferences();
const collapsedProjects = computed(
  () => new Set(prefs.sidebarCollapsedProjectIds),
);
// 项目区总折叠 / 独立会话区折叠（computed 双向桥接后端偏好，异步加载完成后自动生效）
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
const editingProjectName = ref('');

// 旧版本的折叠状态存在 localStorage，这里做一次性迁移：
// 等后端偏好就绪后再读（两边都异步，早读只能读到默认值），
// 只在后端尚无值时才写入，避免覆盖另一窗口已经迁移过的状态。
const SIDEBAR_COLLAPSED_KEY = 'jarvis_sidebar_collapsed';
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
            saved.projectIds.filter((id: unknown): id is string => typeof id === 'string'),
          );
        }
      }
    } catch {
      // 旧值解析失败就忽略，保持后端状态（默认展开）
    }
    localStorage.removeItem(SIDEBAR_COLLAPSED_KEY);
  },
  { immediate: true },
);

// 会话管理状态
const sessions = ref<SessionMeta[]>([]);
const editingSessionId = ref<string | null>(null);
const editingTitle = ref('');

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
  // 后端偏好是快照式落库：算好新集合整体写回，不原地改 Set
  const next = new Set(prefs.sidebarCollapsedProjectIds);
  if (next.has(projectId)) next.delete(projectId);
  else next.add(projectId);
  prefs.setSidebarCollapsedProjectIds([...next]);
};

const isSessionRunning = (sessionId: string): boolean => {
  return sessionStore.sessionViews[sessionId]?.status === "RUNNING";
};

/**
 * 会话行的状态灯取值。优先级：等待授权 > 执行中 > 异常。
 *
 * 等待授权排在最前：一轮 run 在等用户点「允许」时 status 仍是 RUNNING，
 * 如果按执行中显示，用户看不出「卡住的是我」。
 * 异常只在没有 run 的时候显示：一轮失败后 status 留在 ERROR，下次发送才离开，
 * 正好当「这个会话上次出过错」的残留标记。
 */
const sessionLightStates = computed<Record<string, 'running' | 'waiting' | 'error'>>(() => {
  const states: Record<string, 'running' | 'waiting' | 'error'> = {};
  for (const session of sessions.value) {
    if ((perm.permissionRequests[session.id]?.length ?? 0) > 0) {
      states[session.id] = 'waiting';
      continue;
    }
    const status = sessionStore.sessionViews[session.id]?.status;
    if (status === 'RUNNING') states[session.id] = 'running';
    else if (status === 'ERROR') states[session.id] = 'error';
  }
  return states;
});

const formatSessionTime = (timestamp: number) => {
  const date = new Date(timestamp);
  return `${date.getMonth() + 1}/${date.getDate()} ${date.getHours().toString().padStart(2, '0')}:${date.getMinutes().toString().padStart(2, '0')}`;
};

const formatSessionTokens = (session: SessionMeta) => {
  const total = (session.totalInputTokens || 0) + (session.totalOutputTokens || 0);
  if (!total) return '';
  return `${formatTokenCount(total)} tok`;
};

/**
 * 会话操作反馈。
 *
 * 2026-09-21 改成**全局气泡**（`useToast`）：原来的页内提示条会把会话列表挤变形。
 * 保留这个名字与签名，是为了让散落在各处的调用点不必跟着改。
 */
const showSessionActionMessage = (message: string, kind: 'info' | 'error' = 'info') => {
  showToast(message, kind);
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

// 加载会话列表、项目列表与归档计数
const loadAll = async () => {
  await Promise.all([loadSessions(), loadProjects(), loadArchivedCount()]);
};

const loadProjects = async () => {
  try {
    projects.value = await invoke<ProjectMeta[]>('list_projects');
  } catch (err) {
    console.error('加载项目列表失败:', err);
  }
};

/**
 * 归档会话数量（侧边栏只显示一个计数徽标）。
 *
 * 归档列表、恢复与彻底删除都搬到了独立的归档页（`appView.showArchive()`）。
 * 侧边栏不再持有归档列表，只通过 `count_deleted_sessions` 取一个数字：
 * 列表数据只有归档页需要，侧边栏拉全量既浪费 IPC，也让「最近删除」这类
 * 折叠块一直挤占会话列表的空间。
 */
const archivedCount = ref(0);

const loadArchivedCount = async () => {
  try {
    archivedCount.value = await invoke<number>('count_deleted_sessions');
  } catch (err) {
    console.error('加载归档会话数量失败:', err);
  }
};

/**
 * 打开归档页。
 *
 * 与「技能管理」入口同理：只切视图，刻意不动 `sessionStore.activeSessionId` ——
 * 它是消息渲染 / running 状态 / 上下文快照的宿主，置空等于把当前会话卸载掉。
 */
const openArchive = () => {
  appView.showArchive();
};

// 会话列表：后端 `list_sessions` 不收参数，全量拉回后按项目分组展示。
// （曾有一段"高级筛选"链路在这里传 filter，已于 2026-10-07 整体删除 —— 见
//   `core/session/repository.rs::list_sessions` 的说明。）
const loadSessions = async () => {
  try {
    sessions.value = await invoke<SessionMeta[]>('list_sessions');
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

const startRenameProject = (project: ProjectMeta, event: Event) => {
  event.stopPropagation();
  editingProjectId.value = project.id;
  editingProjectName.value = project.name;
};

const cancelRenameProject = () => {
  editingProjectId.value = null;
  editingProjectName.value = '';
};

const submitRenameProject = async (projectId: string) => {
  // 去重守卫：enter 与 blur 可能接连触发，取消后也不该再提交
  if (editingProjectId.value !== projectId) return;
  const name = editingProjectName.value.trim();
  if (!name) {
    showSessionActionMessage(t('sidebar.renameRequired'), 'error');
    return;
  }
  cancelRenameProject();
  try {
    await invoke('rename_project', { id: projectId, name });
    await loadProjects();
    showSessionActionMessage(t('sidebar.renamedProject'));
  } catch (err) {
    console.error('重命名项目失败:', err);
    showSessionActionMessage(t('sidebar.renameProjectError', { error: formatErrorMessage(err) }), 'error');
  }
};

const prepareNewSession = async (projectId: string | null = null) => {
  appView.showChat(); // 新建/打开项目本质是进入聊天视图，技能页下也必须切回
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
    sessionStore.setSessionUsageTotals(null, {});
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
  // 再点一次返回聊天：按钮同时充当“当前视图”指示。
  // 这里刻意不动 sessionStore.activeSessionId —— 它是消息渲染 / running 状态 /
  // 上下文快照的宿主，置空等于把当前会话卸载掉。两个选中态的问题在模板的
  // active 判定条件里解决（会话高亮只在聊天视图下成立），不靠改状态。
  appView.toggleSkillManager();
};

// 在项目下新建对话
const createProjectSession = async (projectId: string) => {
  const project = projects.value.find(p => p.id === projectId);
  await prepareNewSession(projectId);
  if (project) sessionStore.workingDirectory = project.path;
};

// 关闭项目 = 归档项目（可恢复）。
// 名字仍叫 deleteProject 是为了不动模板调用点；语义已由后端 `archive_project` 承担：
// 同一事务里给项目与名下会话打 `deleted_at`，快照与回收站原样保留，归档页可恢复。
const confirmingDeleteProjectId = ref<string | null>(null);

const deleteProject = (projectId: string, event: Event) => {
  event.stopPropagation();
  if (confirmingDeleteProjectId.value === projectId) {
    performDeleteProject(projectId);
  } else {
    // 仍然保留二次点击：虽然可恢复，但误触会让整组会话从列表里消失，代价不小
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
  try {
    await invoke('archive_project', { id: projectId });
    // 活动会话属于该项目时，后端会广播 `active-session-changed`（带 deletedSessionId）
    // 完成「切到 fallback + 对齐预设 + 重载消息视图」，这里不再手动 resetSessionView /
    // chat.resetRenderState —— 那套只清界面不换会话，会留下半状态。
    await loadAll();
  } catch (err) {
    console.error('归档项目失败:', err);
    showSessionActionMessage(formatErrorMessage(err), 'error');
  }
};

/**
 * 对齐「激活预设」与当前会话。
 *
 * **激活预设由后端负责**：`switch_session`（启动恢复也走它）在返回 meta 之前就把
 * `AppConfig.active_profile_id` 按会话的 `profile_id` 对齐并落库（`align_active_profile_to_session`），
 * 对齐值变化时后端自己广播 `config-updated`，输入栏据此重读模型名与能力。
 * 所以正常路径前端**只读不写**，不再 `save_config_cmd` 双写一次——
 * 双写除了多一次写盘 + 一次事件广播，还会与后端的对齐互相覆盖（谁后写谁生效）。
 *
 * 只有本文件的兜底分支（`switch_session` 抛错后回落到首个会话 / 取 active id 失败）
 * 没有经过后端对齐，这里才补一次写盘，保证后端 pipeline 实际使用的预设
 * 与界面显示的预设一致；**同值一律不写**（读回来比较，相同即早退）。
 */
const syncProfileFromSession = async (meta: {
  profileId?: string | null;
  mainModel?: string | null;
}) => {
  try {
    const config = await invoke<any>('get_config');
    const target = meta.profileId || config.globalProfileId;
    if (!target || config.activeProfileId === target) return;
    config.activeProfileId = target;
    await invoke('save_config_cmd', { newConfig: config });
  } catch (err) {
    // 读/写配置失败不阻断切会话：后端对齐仍是主路径，这里只是兜底
    console.warn('对齐会话预设失败:', err);
  }
};

// 切换会话
const switchToSession = async (id: string) => {
  // 视图切换与会话切换正交：点击任何会话都先回到聊天视图，
  // 包括点击当前已激活的会话——此前 showChat 放在早退之后，
  // 从技能页点击当前会话会停在技能页。
  appView.showChat();
  if (id === sessionStore.activeSessionId) return;
  try {
    const meta = await invoke<any>('switch_session', { id });
    sessionStore.activeSessionId = id;
    sessionStore.workingDirectory = meta.workingDirectory || null;
    sessionStore.pendingProjectId = null;

    await syncProfileFromSession(meta);

    // 档位快照的 `resolvedEnabled` 依赖"当前主模型"已确定，
    // 因此在预设落库之后重取一次权威档位，消除与上方 save 的竞态
    //（`activeSessionId` 的 watcher 会先跑一次，这里再对齐一次最终值）。
    void loadSessionThinking(id);

    sessionStore.setSessionUsageTotals(id, {
      input: meta.totalInputTokens || 0,
      output: meta.totalOutputTokens || 0,
      cacheHit: meta.totalCacheHitTokens || 0,
      cacheMiss: meta.totalCacheMissTokens || 0,
    });

    // 切会话只做视角切换，不碰消息和 currentTurn
    // 消息状态由 streaming 事件和 sendToJarvis 收尾自行维护
    await Promise.all([
      events.loadPlanDocumentsFromBackend(id),
      events.loadTodosFromBackend(id),
      events.loadAgentRunsFromBackend(id, { refreshHistory: false }),
      events.loadAgentRunEventsFromBackend(id),
      events.loadSubAgentRunsFromBackend(id),
      events.loadContextSnapshotFromBackend(id),
    ]);

    // 切会话时确保视图已装载：交给 chat store 的状态机裁决——
    // `idle` 拉首屏、`loading` 复用在途请求、`ready` 直接返回、`stale` 重拉。
    // 这里不再自己判 `hasLoadedSessionView`，判据统一收进 `ensureSessionViewLoaded`。
    try {
      await chat.ensureSessionViewLoaded(id, 'session-switch');
    } catch (err) {
      // 无 HTML 兜底（通道已删除）：置标记，界面显示可翻译的提示
      console.error('加载会话消息失败:', err);
      sessionStore.setSessionLoadError(id, true);
    }

    chat.triggerRender();
    await notifyMonitorSessionChanged(id);
    await loadSessions();
    requestAnimationFrame(() => chat.forceScrollToBottom());
  } catch (err) {
    console.error('切换会话失败:', err);
  }
};

// 归档会话：直接执行，不再二次确认。
//
// 归档是可逆的（「归档」区里一键取消），危险度不足以打断用户 —— 原先那个确认框是
// "删除"语义下的产物。真正不可逆的是「归档」区里的「彻底删除」，那一个仍走确认。
//
// 为什么当前会话也可以归档：要求"先切走再归档"会让**只有一个会话时功能完全不可达**，
// 且整理多个会话时被迫来回切换（切换还带改预设、改思考档位的副作用）。
// 归档当前会话后后端会清空 activeSessionId，前端由 active-session-changed 事件
// 回到新对话欢迎页；归档非当前会话时 activeSessionId 保持不变，不会跳走。
const archiving = ref(false);

const deleteSession = async (session: SessionMeta, event: Event) => {
  event.stopPropagation();
  if (archiving.value) return;
  if (isSessionRunning(session.id)) {
    // 正在跑的会话不能归档：run 还在写它的消息与快照
    showSessionActionMessage(t('sidebar.deleteRunning'), 'error');
    return;
  }
  archiving.value = true;
  try {
    await invoke('delete_session', { id: session.id });
    await loadSessions();
    showSessionActionMessage(t('sidebar.archived'));
  } catch (err) {
    console.error('归档会话失败:', err);
    showSessionActionMessage(formatErrorMessage(err), 'error');
  } finally {
    archiving.value = false;
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
        sessionStore.setSessionUsageTotals(activeId, {
          input: meta.totalInputTokens || 0,
          output: meta.totalOutputTokens || 0,
          cacheHit: meta.totalCacheHitTokens || 0,
          cacheMiss: meta.totalCacheMissTokens || 0,
        });

        // 关键：恢复会话时也要把该会话的模型预设同步为激活预设。
        // 漏掉这一步时，后端 pipeline 会继续用 app-config.json 里遗留的
        // activeProfileId（往往是设置里的默认预设），表现为"重启后模型预设不对"。
        await syncProfileFromSession(switchedMeta);
        void loadSessionThinking(activeId);

        // 启动恢复：同上走 `ensureSessionViewLoaded`，状态机自己决定要不要拉首屏
        //（首次启动是 idle → 拉首屏；HMR / 重复挂载时 ready → 零 IPC）
        try {
          await chat.ensureSessionViewLoaded(activeId, 'app-start');
        } catch (err) {
          // 无 HTML 兜底（通道已删除）：置标记，界面显示可翻译的提示
          console.error('加载会话消息失败:', err);
          sessionStore.setSessionLoadError(activeId, true);
        }
      } catch (switchErr) {
        console.error('同步会话状态失败:', switchErr);
        sessionStore.setSessionUsageTotals(null, {});
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
      sessionStore.setSessionUsageTotals(sessions.value[0].id, {
        input: sessions.value[0].totalInputTokens || 0,
        output: sessions.value[0].totalOutputTokens || 0,
        cacheHit: sessions.value[0].totalCacheHitTokens || 0,
        cacheMiss: sessions.value[0].totalCacheMissTokens || 0,
      });
      // 回落到首个会话时同样要对齐预设，否则后端还停在全局默认
      await syncProfileFromSession(sessions.value[0]);
      void loadSessionThinking(sessions.value[0].id);
    }
  } catch (err) {
    sessionStore.setSessionUsageTotals(null, {});
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
    events.loadContextSnapshotFromBackend(),
  ]);

  unlistenRenamed = await listen('session-renamed', () => {
    loadAll();
  });

  unlistenUpdated = await listen('session-updated', () => {
    // 归档（软删除）与恢复都会广播这个事件。这里必须三个数据源同批刷新，不能只拉会话：
    // 恢复一个会话会连带把它所属的归档项目一起复活，项目列表不重拉的话，
    // 该会话虽然有 projectId，但项目不在 projects 里，于是既进不了 projectSessions
    // （只收录 projectId 能在 projects 中找到的会话），也进不了 standaloneSessions
    // （只收录没有 projectId 的会话），最终在侧边栏完全不可见。
    loadAll();
  });
});

onUnmounted(() => {
  // 提示气泡的计时器归 `useToast` 自己管，这里不用清（它挂在模块级队列上）
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
          <button type="button" class="new-session-btn primary-new-btn" @click.stop="createNewSession()">
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
          <button
            type="button"
            class="new-session-btn skill-btn"
            :class="{ active: appView.isSkillManagerView }"
            @click.stop="openSkillManager()"
          >
            <svg viewBox="0 0 24 24" width="16" height="16" stroke="currentColor" stroke-width="2" fill="none">
              <path d="M13 2L3 14h9l-1 8 10-12h-9l1-8z"></path>
            </svg>
            <span>{{ t('sidebar.skillManager') }}</span>
          </button>
          <!-- 归档入口：垃圾桶图标 + 存量徽标。归档页集中承担列表 / 恢复 / 彻底删除，
               所以侧边栏这里不再有折叠块，也不再有任何彻底删除入口 —— 销毁性操作
               必须离开主界面，避免用户在图方便时误点。 -->
          <button
            type="button"
            class="new-session-btn archive-btn"
            :class="{ active: appView.isArchiveView }"
            :title="t('sidebar.recentlyDeleted')"
            @click.stop="openArchive()"
          >
            <svg viewBox="0 0 24 24" width="16" height="16" stroke="currentColor" stroke-width="2" fill="none" stroke-linecap="round" stroke-linejoin="round">
              <rect width="20" height="5" x="2" y="3" rx="1"></rect>
              <path d="M4 8v11a2 2 0 0 0 2 2h12a2 2 0 0 0 2-2V8"></path>
              <path d="M10 12h4"></path>
            </svg>
            <span>{{ t('sidebar.recentlyDeleted') }}</span>
            <span v-if="archivedCount > 0" class="archive-count">{{ archivedCount }}</span>
          </button>
        </div>
        <div v-if="sessions.length === 0 && projects.length === 0" class="session-empty-state">
          {{ t('sidebar.noSessions') }}
        </div>

        <!-- 项目分组 -->
        <!-- 项目总区块：一次折叠全部项目分组（不含独立会话） -->
        <div
          v-if="projects.length > 0"
          class="project-header projects-section-header"
          @click="projectsSectionCollapsed = !projectsSectionCollapsed"
        >
          <svg
            viewBox="0 0 24 24" width="8" height="8" stroke="currentColor" stroke-width="2.5" fill="none"
            class="project-chevron"
            :class="{ collapsed: projectsSectionCollapsed }"
          >
            <polyline points="9 18 15 12 9 6"></polyline>
          </svg>
          <svg viewBox="0 0 24 24" width="12" height="12" stroke="currentColor" stroke-width="2" fill="none" stroke-linecap="round" stroke-linejoin="round" class="project-icon">
            <path d="M22 19a2 2 0 0 1-2 2H4a2 2 0 0 1-2-2V5a2 2 0 0 1 2-2h5l2 3h9a2 2 0 0 1 2 2z"></path>
          </svg>
          <span class="project-name">{{ t('sidebar.projectsSection') }}</span>
          <span class="project-count">{{ projects.length }}</span>
        </div>

        <template v-if="!projectsSectionCollapsed">
        <template v-for="group in projectSessions" :key="group.project.id">
          <div class="project-header project-group-header" @click="toggleProject(group.project.id)">
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
            <input v-if="editingProjectId === group.project.id" v-model="editingProjectName" class="session-title-input" maxlength="40" @click.stop @keydown.enter.prevent="submitRenameProject(group.project.id)" @keydown.esc.prevent="cancelRenameProject" @blur="submitRenameProject(group.project.id)" />
            <span v-else class="project-name">{{ group.project.name }}</span>
            <span class="project-count">{{ group.sessions.length }}</span>
            <button class="project-rename-btn" @click.stop="startRenameProject(group.project, $event)" :title="t('sidebar.renameProject')">
              <svg viewBox="0 0 24 24" width="11" height="11" stroke="currentColor" stroke-width="2" fill="none">
                <path d="M12 20h9"></path>
                <path d="M16.5 3.5a2.12 2.12 0 1 1 3 3L7 19l-4 1 1-4 12.5-12.5z"></path>
              </svg>
            </button>
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
              :title="confirmingDeleteProjectId === group.project.id ? t('common.confirm') : t('sidebar.deleteProjectHint')"
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
              :class="['session-item', { active: appView.isChatView && session.id === sessionStore.activeSessionId }]"
              @click="editingSessionId !== session.id && switchToSession(session.id)"
            >
              <div class="session-main-row">
                <span
                v-if="sessionLightStates[session.id]"
                class="session-state-light state-light"
                :class="`is-${sessionLightStates[session.id]}`"
              ></span>
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
                  <svg viewBox="0 0 24 24" width="11" height="11" stroke="currentColor" stroke-width="2" fill="none" stroke-linecap="round" stroke-linejoin="round">
                    <rect width="20" height="5" x="2" y="3" rx="1"></rect>
                    <path d="M4 8v11a2 2 0 0 0 2 2h12a2 2 0 0 0 2-2V8"></path>
                    <path d="M10 12h4"></path>
                  </svg>
                </button>
              </div>
              <div class="session-meta-row">
                <span>{{ formatSessionTime(session.updatedAt) }}</span>
                <span v-if="formatSessionTokens(session)" class="session-tokens">{{ formatSessionTokens(session) }}</span>
              </div>
            </li>
          </ul>
          </Transition>
        </template>
        </template>

        <!-- 独立会话 -->
        <div class="project-header projects-section-header standalone-header" @click="standaloneCollapsed = !standaloneCollapsed">
          <svg viewBox="0 0 24 24" width="8" height="8" stroke="currentColor" stroke-width="2.5" fill="none" class="project-chevron" :class="{ collapsed: standaloneCollapsed }">
            <polyline points="9 18 15 12 9 6"></polyline>
          </svg>
          <svg viewBox="0 0 24 24" width="13" height="13" stroke="currentColor" stroke-width="2" fill="none" class="project-icon">
            <path d="M21 15a2 2 0 0 1-2 2H7l-4 4V5a2 2 0 0 1 2-2h14a2 2 0 0 1 2 2z"></path>
          </svg>
          <span class="project-name">{{ t('sidebar.standaloneSessions') }}</span>
          <span class="project-count">{{ standaloneSessions.length }}</span>
        </div>
        <ul v-if="!standaloneCollapsed && standaloneSessions.length > 0" class="session-list">
          <li
            v-for="session in standaloneSessions"
            :key="session.id"
            :class="['session-item', { active: appView.isChatView && session.id === sessionStore.activeSessionId }]"
            @click="editingSessionId !== session.id && switchToSession(session.id)"
          >
            <div class="session-main-row">
              <span
                v-if="sessionLightStates[session.id]"
                class="session-state-light state-light"
                :class="`is-${sessionLightStates[session.id]}`"
              ></span>
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
                <svg viewBox="0 0 24 24" width="11" height="11" stroke="currentColor" stroke-width="2" fill="none" stroke-linecap="round" stroke-linejoin="round">
                  <rect width="20" height="5" x="2" y="3" rx="1"></rect>
                  <path d="M4 8v11a2 2 0 0 0 2 2h12a2 2 0 0 0 2-2V8"></path>
                  <path d="M10 12h4"></path>
                </svg>
              </button>
            </div>
            <div class="session-meta-row">
              <span>{{ formatSessionTime(session.updatedAt) }}</span>
              <span v-if="formatSessionTokens(session)" class="session-tokens">{{ formatSessionTokens(session) }}</span>
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
    </div>
  </div>

</template>

<style scoped>
.sidebar {
  width: 250px;
  background: var(--bg-sidebar);
  border-right: 1px solid var(--glass-border-subtle);
  display: flex;
  flex-direction: column;
  overflow: hidden;
  padding: 10px 8px;
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
  color: var(--text-soft);
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
  gap: 2px;
  margin: 2px 4px 12px;
}

.new-session-btn {
  background: transparent;
  border: 1px solid transparent;
  color: var(--text-soft);
  cursor: pointer;
  padding: 7px 10px;
  display: flex;
  align-items: center;
  gap: 9px;
  width: 100%;
  border-radius: var(--radius-md);
  font-size: 0.84rem;
  font-weight: 500;
  transition: background var(--transition-fast), border-color var(--transition-fast), color var(--transition-fast), box-shadow var(--transition-fast);
  -webkit-app-region: no-drag;
  text-align: left;
}

/* 主入口「新建对话」：在浅灰侧栏上以微抬升白卡突出层级，其余导航项保持无边框轻盈感 */
.primary-new-btn {
  background: var(--bg-panel);
  border-color: var(--glass-border-subtle);
  color: var(--text-main);
  font-weight: 600;
  margin-bottom: 6px;
  box-shadow: 0 1px 2px rgba(0, 0, 0, 0.03);
}

.primary-new-btn:hover {
  background: var(--bg-panel);
  border-color: var(--glass-border);
  box-shadow: var(--shadow-sm);
}

.new-session-btn span {
  overflow: hidden;
  text-overflow: ellipsis;
  white-space: nowrap;
}

.new-session-btn:not(.primary-new-btn):hover,
.project-btn:hover {
  background: var(--surface-1);
  border-color: transparent;
  color: var(--text-main);
}

/* 视图入口按钮（技能管理 / 归档）的选中态 = “当前处于该视图”，样式与聊天视图下会话项的选中态同源，
   两者互斥由内容区视图决定：进入技能视图 / 归档页时聊天视图的会话高亮前提不再成立 */
.skill-btn.active,
.archive-btn.active {
  background: var(--bg-panel);
  border-color: var(--glass-border-subtle);
  color: var(--text-main);
  font-weight: 600;
  box-shadow: 0 1px 2px rgba(0, 0, 0, 0.04);
  box-shadow: var(--shadow-sm);
}

/* 归档入口的数量徽标：弱化处理——虚线边 + 弱字色，读作“不活跃存量”，
   不与按钮文字抢视线；数量为 0 时模板直接不渲染 */
.archive-count {
  margin-left: auto;
  flex-shrink: 0;
  padding: 1px 6px;
  font-size: 0.65rem;
  font-family: var(--font-mono);
  border-radius: 10px;
  border: 1px dashed var(--glass-border-subtle);
  color: var(--text-faint);
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
  cursor: pointer;
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
  opacity: 0.85;
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

.project-rename-btn,
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

.project-header:not(.standalone-header):hover .project-rename-btn,
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
  cursor: pointer;
}

/* ── 会话列表层级 ─────────────────────────────────────────────
   三层：分区标题（项目 / 独立会话 / 最近删除）> 项目分组 > 会话。
   靠字号、颜色强度与缩进区分——侧边栏宽度有限，层级不清时用户一眼分不出
   “哪个是可折叠的分区、哪个是项目”。
   分区标题用弱色小字 + 底部分隔线，视觉上“切段”而不是“条目”；
   项目分组保持可点击条目的 hover 反馈，但缩进一级。 */
.project-header.projects-section-header {
  margin: 12px 0 2px;
  padding: 6px 8px 6px 2px;
  border: none;
  border-bottom: 1px solid var(--glass-border-subtle);
  border-radius: 0;
  font-size: 0.72rem;
  font-weight: 600;
  letter-spacing: 0.04em;
  color: var(--text-faint);
}

/* 分区标题 hover 只提亮颜色，不加底色——它不可选中，只是分类标识 */
.project-header.projects-section-header:hover {
  background: transparent;
  border-color: transparent;
  border-bottom-color: var(--glass-border);
  color: var(--text-muted);
}

.project-header.projects-section-header .project-icon {
  width: 11px;
  height: 11px;
  opacity: 0.55;
}

.project-header.project-group-header {
  padding-left: 10px;
  font-size: 0.82rem;
  font-weight: 500;
}

body.dark-mode .project-header.projects-section-header {
  color: rgba(255, 255, 255, 0.42);
}

body.dark-mode .project-header.projects-section-header:hover {
  color: rgba(255, 255, 255, 0.68);
}

/* 项目区总折叠头：一次收起全部项目分组，独立会话不受影响 */
.projects-section-header {
  cursor: pointer;
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
  /* 元信息整行再降一档（faint）：让标题在列表里绝对主导，时间/token 只作余光信息 */
  color: var(--text-faint);
  font-size: 0.62rem;
  font-weight: 400;
  white-space: nowrap;
  overflow: hidden;
}

.session-meta-row span {
  overflow: hidden;
  text-overflow: ellipsis;
}

/* token 读数：等宽数字 + 再弱半档，与文字形的时间区分开 */
.session-meta-row .session-tokens {
  font-family: var(--font-mono);
  opacity: 0.75;
}
.session-item:hover {
  background: var(--glass-bg-light);
  border-color: var(--glass-border-subtle);
  color: var(--text-main);
}
.session-item.active {
  background: var(--glass-bg);
  border-color: var(--glass-border);
  color: var(--text-main);
  font-weight: 600;
  box-shadow: var(--shadow-sm);
}

body.dark-mode .session-item.active {
  color: rgba(255, 255, 255, 0.95);
}

/* 会话行的状态灯：颜色与呼吸节奏全部由全局 .state-light 决定，
   这里只管尺寸与间距。6px 比默认 7px 小一档 —— 它紧排在标题文字前，
   再大会抢视线；侧边栏的信息主体是标题，不是这颗灯。 */
.session-state-light {
  --light-size: 6px;
  margin-right: 4px;
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
  color: var(--text-soft);
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
