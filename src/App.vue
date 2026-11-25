<!--
# App.vue — 应用根组件

组合 TitleBar / Sidebar / ChatArea 的整体布局，管理全局运行状态、主对话与会话级子 Agent 视图切换。

## Key Exports
- `App`: 应用根界面与会话工作区

## Constraints
- 状态灯四态：running=蓝（执行中）、waiting=琥珀（等待授权/方案审批）、finish=绿、error=红；
  interrupted / cancelled / idle 保持中性灰（它们不是“结果”，不应用语义色）
- 子 Agent 列表按当前会话的持久化轨迹索引恢复，详细事件按选中项懒加载
-->
<script setup lang="ts">
import { onMounted, onBeforeUnmount, ref, computed, watch } from "vue";
import { useI18n } from "vue-i18n";
import { listen } from "@tauri-apps/api/event";
import type { UnlistenFn } from "@tauri-apps/api/event";
import { getCurrentWindow } from "@tauri-apps/api/window";
import { useAgentEvents } from "./composables/useAgentEvents";
import { usePreferences } from "./composables/usePreferences";
import { useWindow } from "./composables/useWindow";
import { useTheme } from "./composables/useTheme";
import { useSessionStore } from "./stores/session";
import { useChatStore } from "./stores/chat";
import { useAgentStore } from "./stores/agent";
import { useAppViewStore } from "./stores/appView";
import { usePermissionStore } from "./stores/permission";

import TitleBar from "./components/layout/TitleBar.vue";
import Sidebar from "./components/layout/Sidebar.vue";
import ChatArea from "./components/chat/ChatArea.vue";
import TerminalInput from "./components/chat/TerminalInput.vue";
import PermissionCard from "./components/chat/PermissionCard.vue";
import UserQuestionCard from "./components/chat/UserQuestionCard.vue";
import PlanPreviewPanel from "./components/common/PlanPreviewPanel.vue";
import ToastHost from "./components/common/ToastHost.vue";
import UpdatePrompt from "./components/common/UpdatePrompt.vue";
import SettingsPanel from "./components/settings/SettingsPanel.vue";
import { useUpdater } from "./composables/useUpdater";
// 视图级页面组件：整页接管主内容区（与聊天工作区互斥）
import ExtensionHub from "./components/extension/ExtensionHub.vue";
import ArchiveManager from "./components/archive/ArchiveManager.vue";
import SubAgentChatPane from "./components/chat/SubAgentChatPane.vue";
import type { SubAgentConversationItem } from "./types";

const showSettings = ref(false);
const updater = useUpdater();
let updateCheckTimer: ReturnType<typeof setTimeout> | null = null;
const { t } = useI18n();
const prefs = usePreferences();
/**
 * 侧栏折叠：直接桥接持久化偏好，不用 ref 拷一份。
 *
 * 原先这里是 `ref(prefs.sidebarCollapsed)` —— 一次性快照。偏好是异步加载的，
 * 库里真实值（可能是 true）回来时，这个 ref 早已固定在启动时的默认值 false，
 * 于是界面与磁盘长期不一致：关掉再开，侧栏总是展开的。
 */
const sidebarCollapsed = computed({
  get: () => prefs.sidebarCollapsed,
  set: (val: boolean) => { prefs.setSidebarCollapsed(val); },
});
useTheme(); // 初始化主题

const session = useSessionStore();
const chat = useChatStore();
const agent = useAgentStore();
const appView = useAppViewStore();
const permission = usePermissionStore();
const { initListeners, loadSubAgentRunsFromBackend, loadSubAgentTraceFromBackend } = useAgentEvents();
const {
  openMonitorWindow,
  toggleMonitorWindow,
  onMonitorWindowClosed,
  restoreCurrentWindowState,
  watchCurrentWindowState,
} = useWindow();
const projectName = computed(() => {
  const dir = session.workingDirectory;
  if (!dir) return null;
  const parts = dir.replace(/\\/g, '/').split('/').filter(Boolean);
  return parts[parts.length - 1] || dir;
});

const hasUnseenFinish = ref(false);
let unlistenMonitorWindowClosed: (() => void) | null = null;
let unwatchWindowState: (() => void) | null = null;
let unlistenSessionCompacted: UnlistenFn | null = null;
let unlistenSubAgentChatRequested: UnlistenFn | null = null;
const activeConversationTab = ref<'main' | 'subagents'>('main');
const activeSubAgentRunId = ref<string | null>(null);
const selectedSubAgentBySession = ref<Record<string, string>>({});

const subAgentConversationItems = computed<SubAgentConversationItem[]>(() =>
  agent.currentSubAgentTraceRuns.map((summary) => {
    const liveRun = agent.subAgentRuns[summary.runId];
    return {
      ...summary,
      label: liveRun?.label || summary.label || summary.runId,
      status: liveRun?.status ?? 'saved',
      phase: liveRun?.phase ?? 'finalizing',
      loopCount: liveRun?.loopCount ?? summary.loopCount,
      maxLoops: liveRun?.maxLoops ?? 0,
      currentTool: liveRun?.currentTool ?? null,
      inputTokens: liveRun?.inputTokens ?? 0,
      outputTokens: liveRun?.outputTokens ?? 0,
      error: liveRun?.error ?? null,
      summary: liveRun?.summary ?? null,
      promptPreview: liveRun?.promptPreview,
    };
  }),
);

const selectedSubAgent = computed(() =>
  subAgentConversationItems.value.find((item) => item.runId === activeSubAgentRunId.value) ?? null,
);
const activeSubAgentTrace = computed(() =>
  selectedSubAgent.value ? agent.getSubAgentTrace(selectedSubAgent.value.runId) : [],
);

const selectSubAgent = (runId: string) => {
  const item = subAgentConversationItems.value.find((entry) => entry.runId === runId);
  if (!item) return;
  activeConversationTab.value = 'subagents';
  activeSubAgentRunId.value = runId;
  selectedSubAgentBySession.value = {
    ...selectedSubAgentBySession.value,
    [item.sessionId]: runId,
  };
};

const selectSubAgentView = async () => {
  activeConversationTab.value = 'subagents';
  const sessionId = session.activeSessionId;
  const rememberedRunId = sessionId ? selectedSubAgentBySession.value[sessionId] : null;
  const selected = subAgentConversationItems.value.find((item) => item.runId === rememberedRunId)
    ?? subAgentConversationItems.value[0];
  if (selected) selectSubAgent(selected.runId);
};

const openSubAgentChat = async (runId: string, sessionId: string) => {
  await loadSubAgentRunsFromBackend(sessionId);
  if (session.activeSessionId !== sessionId) return;
  await selectSubAgent(runId);
};

const selectMainConversation = () => { activeConversationTab.value = 'main'; };

let subAgentSessionLoadGeneration = 0;
watch(() => session.activeSessionId, async (sessionId) => {
  const generation = ++subAgentSessionLoadGeneration;
  activeSubAgentRunId.value = null;
  if (!sessionId) {
    if (activeConversationTab.value === 'subagents') activeConversationTab.value = 'main';
    return;
  }

  await loadSubAgentRunsFromBackend(sessionId);
  if (generation !== subAgentSessionLoadGeneration || session.activeSessionId !== sessionId) return;

  const entries = subAgentConversationItems.value;
  const rememberedRunId = selectedSubAgentBySession.value[sessionId];
  const selected = entries.find((item) => item.runId === rememberedRunId) ?? entries[0];
  if (!selected) {
    if (activeConversationTab.value === 'subagents') activeConversationTab.value = 'main';
    return;
  }
  activeSubAgentRunId.value = selected.runId;
  selectedSubAgentBySession.value = {
    ...selectedSubAgentBySession.value,
    [sessionId]: selected.runId,
  };
}, { immediate: true, flush: 'post' });

watch([activeConversationTab, activeSubAgentRunId], ([tab, runId], [previousTab, previousRunId]) => {
  agent.pinSubAgentTrace(tab === 'subagents' ? runId : null);
  if (previousRunId && (previousRunId !== runId || (previousTab === 'subagents' && tab !== 'subagents'))) {
    agent.clearSubAgentTrace(previousRunId);
  }
  if (tab === 'subagents' && runId && !Object.prototype.hasOwnProperty.call(agent.subAgentTraceByRun, runId)) {
    void loadSubAgentTraceFromBackend(runId);
  }
}, { flush: 'post' });

// 恢复持久化的 Agent 面板可见性
agent.showAgentPanel = prefs.agentPanelVisible;

const toggleAgentMonitor = async () => {
  try {
    agent.showAgentPanel = await toggleMonitorWindow(agent.showAgentPanel);
  } catch (err) {
    console.error('切换监控窗口失败:', err);
    agent.showAgentPanel = false;
  }
};

const rawStatus = computed(() => {
  // 等待决策优先：权限请求与方案审批期间 status 仍是 RUNNING，
  // 按「执行中」显示会让用户以为系统在转，实际卡住的是人。
  if (permission.permissionRequest || permission.userQuestion || permission.planProposal) return 'waiting';
  if (session.isCurrentSessionRunning) return 'running';
  if (session.currentSessionStatus === 'INTERRUPTED') return 'interrupted';
  if (session.currentSessionStatus === 'ERROR') return 'error';
  if (session.currentSessionStatus === 'FINISH') return 'finish';
  return 'idle';
});

const displayStatus = computed(() => {
  if (rawStatus.value === 'finish' && !hasUnseenFinish.value) return 'idle';
  return rawStatus.value;
});

const acknowledgeFinish = () => {
  if (rawStatus.value === 'finish') {
    hasUnseenFinish.value = false;
  }
};

const acknowledgeFinishOnVisible = () => {
  if (!document.hidden) {
    acknowledgeFinish();
  }
};

const addFinishAcknowledgementListeners = () => {
  window.addEventListener('focus', acknowledgeFinish);
  document.addEventListener('visibilitychange', acknowledgeFinishOnVisible);
  document.addEventListener('pointerdown', acknowledgeFinish);
  document.addEventListener('pointermove', acknowledgeFinish, { passive: true });
  document.addEventListener('wheel', acknowledgeFinish, { passive: true });
  document.addEventListener('keydown', acknowledgeFinish);
  document.addEventListener('touchstart', acknowledgeFinish, { passive: true });
};

const removeFinishAcknowledgementListeners = () => {
  window.removeEventListener('focus', acknowledgeFinish);
  document.removeEventListener('visibilitychange', acknowledgeFinishOnVisible);
  document.removeEventListener('pointerdown', acknowledgeFinish);
  document.removeEventListener('pointermove', acknowledgeFinish);
  document.removeEventListener('wheel', acknowledgeFinish);
  document.removeEventListener('keydown', acknowledgeFinish);
  document.removeEventListener('touchstart', acknowledgeFinish);
};

watch([rawStatus, () => session.activeSessionId], ([status, activeSessionId], [prevStatus, prevActiveSessionId]) => {
  const sessionChanged = activeSessionId !== prevActiveSessionId;
  if (status === 'finish' && prevStatus === 'running' && !sessionChanged) {
    hasUnseenFinish.value = true;
    return;
  }
  if (status !== 'finish' || sessionChanged) {
    hasUnseenFinish.value = false;
  }
});

// 侧栏折叠的持久化已由上面的 computed setter 负责，不再需要单独的 watch

// 持久化 Agent 面板可见性
watch(() => agent.showAgentPanel, (val) => {
  prefs.setAgentPanelVisible(val);
});

onMounted(async () => {
  await restoreCurrentWindowState();
  unwatchWindowState = await watchCurrentWindowState();
  addFinishAcknowledgementListeners();
  unlistenMonitorWindowClosed = await onMonitorWindowClosed(() => {
    agent.showAgentPanel = false;
  });
  // 监听监控窗口的上下文压缩完成事件，刷新主窗口聊天历史
  unlistenSessionCompacted = await listen<{ sessionId: string }>('session-compacted', async (event) => {
    const sid = event.payload.sessionId;
    if (!sid || sid !== session.activeSessionId) return;
    try {
      // 压缩会重排历史（生成摘要并截断），属于「数据确实被后端改写」的场景：
      // 走入口三强制重载，分页游标一并重置。
      await chat.reloadSessionView(sid, 'compaction');
    } catch { /* ignore */ }
  });
  await initListeners();
  unlistenSubAgentChatRequested = await listen<{ runId: string; sessionId: string }>("open-subagent-chat", (event) => {
    const { runId, sessionId } = event.payload ?? {};
    if (runId && sessionId) {
      void openSubAgentChat(runId, sessionId).then(() => getCurrentWindow().setFocus()).catch((err) => {
        console.error('打开子 Agent 对话失败:', err);
      });
    }
  });
  if (agent.showAgentPanel) {
    try {
      await openMonitorWindow();
    } catch (err) {
      console.error('恢复监控窗口失败:', err);
      agent.showAgentPanel = false;
    }
  }
  // 启动后延迟检查更新：不阻塞启动；开发模式由 checkForUpdate 内部跳过
  void updater.init();
  updateCheckTimer = setTimeout(() => {
    void updater.checkForUpdate(false);
  }, 5000);
});

onBeforeUnmount(() => {
  if (updateCheckTimer) {
    clearTimeout(updateCheckTimer);
    updateCheckTimer = null;
  }
  removeFinishAcknowledgementListeners();
  unlistenMonitorWindowClosed?.();
  unwatchWindowState?.();
  unlistenSessionCompacted?.();
  unlistenSubAgentChatRequested?.();
});
</script>

<template>
  <main class="editor-container">
    <div class="editor-window">
      <TitleBar :sidebar-collapsed="sidebarCollapsed" @toggle-sidebar="sidebarCollapsed = !sidebarCollapsed" />

      <div class="editor-body">
        <Sidebar :collapsed="sidebarCollapsed" @open-settings="showSettings = true" />

        <div class="main-content">
          <!-- 会话工具栏只属于聊天视图：沙箱徽标/状态灯/监控入口都是会话级内容；
               技能管理页有自己的页头，不再复用这一行 -->
          <div v-if="appView.isChatView" class="tab-bar">
            <div v-if="projectName" class="tab-bar-project sandboxed" :title="t('app.sandboxBoundaryTitle', { dir: session.workingDirectory })">
              <svg viewBox="0 0 24 24" width="12" height="12" stroke="currentColor" stroke-width="2" fill="none" stroke-linecap="round" stroke-linejoin="round">
                <path d="M22 19a2 2 0 0 1-2 2H4a2 2 0 0 1-2-2V5a2 2 0 0 1 2-2h5l2 3h9a2 2 0 0 1 2 2z"></path>
              </svg>
              <span>{{ projectName }}</span>
            </div>
            <!-- 非沙箱会话（未挂项目）：显式标出无边界，避免用户误以为存在目录限制 -->
            <div v-else class="tab-bar-project no-sandbox" :title="t('app.noSandboxTitle')">
              <svg viewBox="0 0 24 24" width="12" height="12" stroke="currentColor" stroke-width="2" fill="none" stroke-linecap="round" stroke-linejoin="round">
                <rect x="3" y="11" width="18" height="11" rx="2" />
                <path d="M7 11V7a5 5 0 0 1 9.9-1" />
              </svg>
              <span>{{ t('app.noSandbox') }}</span>
            </div>
            <div class="tab-bar-center">
              <div class="status-indicator" :class="displayStatus">
                <svg class="status-light" viewBox="0 0 24 24" width="20" height="20">
                  <circle class="status-glow" cx="12" cy="12" r="10" />
                  <circle class="status-core" cx="12" cy="12" r="5" />
                </svg>
              </div>
            </div>
            <button
              class="agent-panel-toggle"
              :class="{ active: agent.showAgentPanel }"
              @click="toggleAgentMonitor"
              :title="agent.showAgentPanel ? t('app.hideAgentPanel') : t('app.showAgentPanel')"
            >
              <svg viewBox="0 0 24 24" width="16" height="16" stroke="currentColor" stroke-width="2" fill="none" stroke-linecap="round" stroke-linejoin="round">
                <polyline points="22 12 18 12"></polyline>
                <polyline points="6 12 2 12"></polyline>
                <polyline points="16 6 18 12 16 18"></polyline>
                <polyline points="8 6 6 12 8 18"></polyline>
              </svg>
            </button>
          </div>

          <template v-if="appView.isExtensionsView">
            <ExtensionHub />
          </template>
          <!-- 归档管理视图：已归档会话列表（恢复 / 彻底删除）。
               与技能管理同级，独占主内容区；聊天区的输入框、权限卡等会话级元素都不渲染 -->
          <template v-else-if="appView.isArchiveView">
            <ArchiveManager />
          </template>
          <template v-else>
            <div class="conversation-workspace">
              <!-- 仅当当前会话存在子 Agent 轨迹时才显示切换页签栏，避免纯主对话时顶部出现多余空行 -->
              <div v-if="subAgentConversationItems.length > 0" class="conversation-tabs" role="tablist" :aria-label="t('monitor.subAgentChat.title')">
                <button
                  class="conversation-tab parent-conversation-tab"
                  :class="{ active: activeConversationTab === 'main' }"
                  role="tab"
                  :aria-selected="activeConversationTab === 'main'"
                  @click="selectMainConversation"
                >
                  <span>{{ t('monitor.subAgentChat.parentConversation') }}</span>
                </button>
                <button
                  v-if="subAgentConversationItems.length"
                  class="conversation-tab parent-conversation-tab"
                  :class="{ active: activeConversationTab === 'subagents' }"
                  role="tab"
                  :aria-selected="activeConversationTab === 'subagents'"
                  @click="selectSubAgentView"
                >
                  <span>{{ t('monitor.subAgentChat.subAgents') }}</span>
                  <span class="conversation-tab-count">{{ t('monitor.subAgentChat.subAgentCount', { count: subAgentConversationItems.length }) }}</span>
                </button>
              </div>
              <div class="conversation-view">
                <section class="primary-chat-pane" v-show="activeConversationTab === 'main'">
                  <ChatArea />
                  <PlanPreviewPanel />
                  <div class="floating-terminal-container">
                    <!-- 权限卡：贴输入框顶部 0px（Codex 风格）。放进本容器后，
                         卡片高度变化由 TerminalInput 对容器的 ResizeObserver 自动测入
                         --input-area-height，聊天流底部留白无需另行补偿 -->
                    <PermissionCard />
                    <UserQuestionCard />
                    <TerminalInput />
                  </div>
                </section>
                <SubAgentChatPane
                  v-if="activeConversationTab === 'subagents'"
                  :runs="subAgentConversationItems"
                  :active-run-id="activeSubAgentRunId"
                  :run="selectedSubAgent"
                  :events="activeSubAgentTrace"
                  @select-run="selectSubAgent"
                />
              </div>
            </div>
          </template>
        </div>
      </div>
    </div>

    <SettingsPanel v-model="showSettings" />

    <!-- 启动时发现新版本的更新提示（状态来自 useUpdater 单例） -->
    <UpdatePrompt />

    <!-- 全局轻提示：挂一次即可（Teleport 到 body，不参与本组件布局） -->
    <ToastHost />
  </main>
</template>

<style scoped>
.editor-container {
  height: 100%;
  width: 100%;
  display: flex;
  background-color: var(--bg-dark);
  color: var(--text-main);
  font-family: var(--font-sans);
  overflow: hidden;
  transition: background-color var(--transition-normal), color var(--transition-normal);
  position: relative;
  z-index: 1;
}

.editor-window {
  width: 100%;
  height: 100%;
  display: flex;
  flex-direction: column;
  overflow: hidden;
}

.editor-body {
  display: flex;
  flex: 1;
  overflow: hidden;
  position: relative;
}

.main-content {
  flex: 1;
  display: flex;
  flex-direction: column;
  background: var(--glass-bg-heavy);
  min-width: 0;
  min-height: 0;
  position: relative;
  z-index: 1;
}

.conversation-workspace {
  flex: 1;
  min-width: 0;
  min-height: 0;
  display: flex;
  flex-direction: column;
  overflow: hidden;
}

.conversation-tabs {
  height: 42px;
  min-height: 42px;
  display: flex;
  align-items: stretch;
  gap: 4px;
  padding: 5px 8px 0;
  overflow-x: auto;
  flex-shrink: 0;
  border-bottom: 1px solid var(--glass-border);
  background: var(--glass-bg);
}

.parent-conversation-tab.active { border-color: var(--glass-border); background: var(--bg-sidebar); color: var(--text-main); }

.conversation-tab {
  height: 36px;
  min-width: 0;
  display: inline-flex;
  align-items: center;
  gap: 8px;
  padding: 0 10px;
  border: 1px solid transparent;
  border-bottom: 0;
  border-radius: 8px 8px 0 0;
  background: transparent;
  color: var(--text-muted);
  cursor: pointer;
  font: inherit;
  font-size: 0.8rem;
  white-space: nowrap;
}

.conversation-tab:hover { color: var(--text-main); background: var(--glass-bg-light); }
.conversation-tab.active { color: var(--text-main); }
.parent-conversation-tab { flex: 0 0 auto; }
.conversation-tab-count {
  margin-left: 2px;
  padding: 1px 7px;
  border: 1px solid var(--glass-border-subtle);
  border-radius: 999px;
  background: var(--glass-bg-light);
  color: var(--text-muted);
  font-size: 0.68rem;
  line-height: 1.45;
}

.conversation-view {
  flex: 1;
  min-width: 0;
  min-height: 0;
  display: flex;
  overflow: hidden;
}

.primary-chat-pane {
  position: relative;
  flex: 1 1 100%;
  min-width: 0;
  min-height: 0;
  display: flex;
  flex-direction: column;
  overflow: hidden;
}

.tab-bar {
  height: 38px;
  background: var(--glass-bg);
  display: flex;
  align-items: center;
  border-bottom: 1px solid var(--glass-border);
  flex-shrink: 0;
  padding: 0 8px;
  position: relative;
}

.tab-bar-project {
  display: inline-flex;
  align-items: center;
  gap: 5px;
  padding: 2px 9px;
  border-radius: 8px;
  font-size: 0.72rem;
  font-weight: 500;
  color: var(--text-muted);
  background: transparent;
  border: 1px solid color-mix(in srgb, var(--text-muted) 20%, transparent);
  user-select: none;
  margin-left: 4px;
}

.tab-bar-project svg {
  flex-shrink: 0;
  opacity: 0.85;
}

/* 会话范围语义色：项目会话 = 绿（有目录边界）；独立对话 = 琥珀（无目录边界）。
   去掉厚重灰底胶囊，改为通透底 + 细腻描边。 */
.tab-bar-project.sandboxed {
  color: var(--accent-green);
  border-color: color-mix(in srgb, var(--accent-green) 28%, transparent);
}

.tab-bar-project.no-sandbox {
  color: var(--text-warning);
  border-style: dashed;
  border-color: color-mix(in srgb, var(--text-warning) 36%, transparent);
}

.status-indicator {
  display: inline-flex;
  align-items: center;
  justify-content: center;
  transition: all var(--transition-fast);
}

.tab-bar-center {
  position: absolute;
  left: 50%;
  transform: translateX(-50%);
  display: flex;
  justify-content: center;
  align-items: center;
}

.status-light {
  flex-shrink: 0;
  overflow: visible;
}

.status-glow {
  display: none;
}

.status-core {
  fill: currentColor;
  opacity: 0.9;
}

.status-indicator.finish { color: var(--accent-green); }

.status-indicator.error { color: var(--accent-red); }

/* 等待决策：节奏压到 1.4s、幅度更大 —— 「它在等你」需要比「它在干活」更抓眼 */
.status-indicator.waiting { color: var(--text-warning); }
.status-indicator.waiting .status-glow {
  display: block;
  fill: currentColor;
  transform-box: fill-box;
  transform-origin: center;
  animation: statusGlowWide 1.4s ease-out infinite;
}
.status-indicator.waiting .status-core { animation: statusBreathe 1.4s ease-in-out infinite; }

/* 执行中：蓝点 + 2s 呼吸外圈。用蓝不用绿 —— 绿色已被「完成」占用 */
.status-indicator.running { color: var(--accent-blue); }
.status-indicator.running .status-glow {
  display: block;
  fill: currentColor;
  transform-box: fill-box;
  transform-origin: center;
  animation: statusGlow 2s ease-out infinite;
}
.status-indicator.running .status-core { animation: statusBreathe 2s ease-in-out infinite; }

/* 中断 / 取消 / 空闲：不是「结果」，保持中性灰，不抢语义色 */
.status-indicator.interrupted { color: var(--text-muted); }
.status-indicator.interrupted .status-core { opacity: 0.85; }

.status-indicator.cancelled { color: var(--text-muted); }
.status-indicator.cancelled .status-core { opacity: 0.85; }

.status-indicator.idle { color: var(--text-muted); }
.status-indicator.idle .status-core { opacity: 0.4; }

.agent-panel-toggle {
  background: transparent;
  border: 1px solid transparent;
  color: var(--text-muted);
  cursor: pointer;
  padding: 4px 8px;
  margin-left: auto;
  display: inline-flex;
  align-items: center;
  justify-content: center;
  border-radius: var(--radius-md);
  transition: all var(--transition-fast);
  -webkit-app-region: no-drag;
}
.agent-panel-toggle:hover {
  color: var(--accent-blue);
  background: var(--glass-bg-light);
  border-color: var(--glass-border-subtle);
}
.agent-panel-toggle.active {
  color: var(--accent-blue);
  background: color-mix(in srgb, var(--accent-blue) 8%, transparent);
  border-color: color-mix(in srgb, var(--accent-blue) 20%, transparent);
}

.floating-terminal-container {
  position: absolute;
  bottom: 0;
  left: 0;
  right: 0;
  z-index: 100;
  padding: 0 40px 32px;
  pointer-events: none;
  display: flex;
  /* column：权限卡堆叠在输入框正上方（0px 间隔，Codex 风格），整体随输入框浮动 */
  flex-direction: column;
  align-items: center;
}

.floating-terminal-container > * {
  pointer-events: auto;
  width: 100%;
}

/* 内核呼吸：只改透明度不改尺寸，避免 5px 的实心点边绿发虚 */
@keyframes statusBreathe {
  0%, 100% { opacity: 0.9; }
  50% { opacity: 0.45; }
}

/* 外圈扩散：从紧贴内核（r=5）扩到整个 viewBox 直径（r=10）后淡出。
   两态共用同一套动作，只是时长不同 —— 差异本身已经在传递语义，
   不需要再为等待态写一个形状不同的动画。 */
@keyframes statusGlow {
  0% { transform: scale(0.55); opacity: 0.3; }
  70%, 100% { transform: scale(1.05); opacity: 0; }
}

@keyframes statusGlowWide {
  0% { transform: scale(0.5); opacity: 0.4; }
  60%, 100% { transform: scale(1.12); opacity: 0; }
}

/* 系统开了「减少动态效果」：四态靠颜色区分已经足够，光晕直接收起 */
@media (prefers-reduced-motion: reduce) {
  .status-indicator .status-core,
  .status-indicator .status-glow {
    animation: none;
  }
  .status-indicator .status-glow {
    display: none;
  }
}
</style>
