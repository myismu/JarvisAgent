<script setup lang="ts">
import { onMounted, onBeforeUnmount, ref, computed, watch } from "vue";
import { useI18n } from "vue-i18n";
import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import type { UnlistenFn } from "@tauri-apps/api/event";
import { useAgentEvents } from "./composables/useAgentEvents";
import { usePreferences } from "./composables/usePreferences";
import { useWindow } from "./composables/useWindow";
import { useTheme } from "./composables/useTheme";
import { useSessionStore } from "./stores/session";
import { useAgentStore } from "./stores/agent";

import TitleBar from "./components/layout/TitleBar.vue";
import Sidebar from "./components/layout/Sidebar.vue";
import ChatArea from "./components/chat/ChatArea.vue";
import TerminalInput from "./components/chat/TerminalInput.vue";
import PlanPreviewPanel from "./components/common/PlanPreviewPanel.vue";
import SettingsPanel from "./components/settings/SettingsPanel.vue";

const showSettings = ref(false);
const { t } = useI18n();
const prefs = usePreferences();
const sidebarCollapsed = ref(prefs.sidebarCollapsed);
useTheme(); // 初始化主题

const session = useSessionStore();
const agent = useAgentStore();
const { initListeners } = useAgentEvents();
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

// 持久化侧栏折叠状态
watch(sidebarCollapsed, (val) => {
  prefs.setSidebarCollapsed(val);
});

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
      const messages = await invoke<any[]>('get_session_messages', { sessionId: sid });
      session.replaceSessionMessages(sid, messages);
    } catch { /* ignore */ }
  });
  await initListeners();
  if (agent.showAgentPanel) {
    try {
      await openMonitorWindow();
    } catch (err) {
      console.error('恢复监控窗口失败:', err);
      agent.showAgentPanel = false;
    }
  }
});

onBeforeUnmount(() => {
  removeFinishAcknowledgementListeners();
  unlistenMonitorWindowClosed?.();
  unwatchWindowState?.();
  unlistenSessionCompacted?.();
});
</script>

<template>
  <main class="editor-container">
    <div class="editor-window">
      <TitleBar :sidebar-collapsed="sidebarCollapsed" />

      <div class="editor-body">
        <Sidebar :collapsed="sidebarCollapsed" @open-settings="showSettings = true" />

        <div class="main-content">
          <div class="tab-bar">
            <button class="sidebar-toggle" @click="sidebarCollapsed = !sidebarCollapsed" :title="sidebarCollapsed ? t('app.expandSidebar') : t('app.collapseSidebar')">
              <svg viewBox="0 0 24 24" width="16" height="16" stroke="currentColor" stroke-width="2" fill="none" stroke-linecap="round" stroke-linejoin="round">
                <polyline :points="sidebarCollapsed ? '9 18 15 12 9 6' : '15 18 9 12 15 6'"></polyline>
              </svg>
            </button>
            <div v-if="projectName" class="tab-bar-project">
              <svg viewBox="0 0 24 24" width="12" height="12" stroke="currentColor" stroke-width="2" fill="none" stroke-linecap="round" stroke-linejoin="round">
                <path d="M22 19a2 2 0 0 1-2 2H4a2 2 0 0 1-2-2V5a2 2 0 0 1 2-2h5l2 3h9a2 2 0 0 1 2 2z"></path>
              </svg>
              <span>{{ projectName }}</span>
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
          
          <ChatArea />
          <PlanPreviewPanel />

          <div class="floating-terminal-container">
            <TerminalInput />
          </div>
        </div>
      </div>
    </div>

    <SettingsPanel v-model="showSettings" />
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
  backdrop-filter: blur(var(--glass-blur));
  -webkit-backdrop-filter: blur(var(--glass-blur));
  min-width: 0;
  min-height: 0;
  position: relative;
  z-index: 1;
}

.tab-bar {
  height: 38px;
  background: var(--glass-bg);
  backdrop-filter: blur(var(--glass-blur));
  -webkit-backdrop-filter: blur(var(--glass-blur));
  display: flex;
  align-items: center;
  border-bottom: 1px solid var(--glass-border);
  flex-shrink: 0;
  padding: 0 4px;
  position: relative;
}

.sidebar-toggle {
  background: transparent;
  border: 1px solid transparent;
  color: var(--text-muted);
  cursor: pointer;
  padding: 4px 8px;
  display: inline-flex;
  align-items: center;
  justify-content: center;
  border-radius: var(--radius-md);
  transition: all var(--transition-fast);
  -webkit-app-region: no-drag;
}
.sidebar-toggle:hover {
  color: var(--accent-blue);
  background: var(--glass-bg-light);
  border-color: var(--glass-border-subtle);
}

.tab-bar-project {
  display: inline-flex;
  align-items: center;
  gap: 6px;
  padding: 3px 10px;
  border-radius: 20px;
  font-size: 0.72rem;
  color: var(--text-muted);
  background: var(--glass-bg-light);
  border: 1px solid var(--glass-border-subtle);
  user-select: none;
  margin-left: 4px;
}

.tab-bar-project svg {
  flex-shrink: 0;
  opacity: 0.5;
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
  filter: drop-shadow(0 0 2px currentColor);
  transition: filter var(--transition-fast);
}

.status-glow {
  fill: currentColor;
  opacity: 0.2;
  animation: breathe 3s ease-in-out infinite;
}

.status-core {
  fill: currentColor;
  opacity: 0.9;
  animation: breatheCore 3s ease-in-out infinite;
}

.status-indicator.finish { color: var(--accent-green); }
.status-indicator.finish .status-glow { animation: breatheGreen 3s ease-in-out infinite; }
.status-indicator.finish .status-core { animation: breatheCoreGreen 3s ease-in-out infinite; }
.status-indicator.finish .status-light { filter: drop-shadow(0 0 4px rgba(16, 185, 129, 0.5)); }

.status-indicator.error { color: var(--accent-red); }
.status-indicator.error .status-glow { animation: breatheRed 2s ease-in-out infinite; }
.status-indicator.error .status-core { animation: breatheCoreRed 2s ease-in-out infinite; }
.status-indicator.error .status-light { filter: drop-shadow(0 0 4px rgba(239, 68, 68, 0.5)); }

.status-indicator.running { color: var(--accent-yellow); }
.status-indicator.running .status-glow { animation: breatheYellow 1.5s ease-in-out infinite; }
.status-indicator.running .status-core { animation: breatheCoreYellow 1.5s ease-in-out infinite; }
.status-indicator.running .status-light { filter: drop-shadow(0 0 4px rgba(245, 158, 11, 0.5)); }

.status-indicator.interrupted { color: var(--accent-yellow); }
.status-indicator.interrupted .status-glow { animation: none; opacity: 0.2; }
.status-indicator.interrupted .status-core { animation: none; opacity: 0.65; }
.status-indicator.interrupted .status-light { filter: drop-shadow(0 0 4px rgba(245, 158, 11, 0.35)); }

.status-indicator.cancelled { color: var(--text-muted); }
.status-indicator.cancelled .status-glow { animation: none; opacity: 0.15; }
.status-indicator.cancelled .status-core { animation: none; opacity: 0.5; }
.status-indicator.cancelled .status-light { filter: none; }

.status-indicator.idle { color: var(--text-muted); }
.status-indicator.idle .status-glow { animation: none; opacity: 0.12; }
.status-indicator.idle .status-core { animation: none; opacity: 0.4; }
.status-indicator.idle .status-light { filter: none; }

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
  background: rgba(59, 130, 246, 0.08);
  border-color: rgba(59, 130, 246, 0.2);
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
  justify-content: center;
}

.floating-terminal-container > * {
  pointer-events: auto;
  width: 100%;
}

@keyframes breatheGreen {
  0%, 100% { opacity: 0.15; r: 10; }
  50% { opacity: 0.4; r: 11; }
}
@keyframes breatheCoreGreen {
  0%, 100% { opacity: 0.7; }
  50% { opacity: 1; }
}

@keyframes breatheYellow {
  0%, 100% { opacity: 0.2; r: 10; }
  50% { opacity: 0.55; r: 11.5; }
}
@keyframes breatheCoreYellow {
  0%, 100% { opacity: 0.75; }
  50% { opacity: 1; }
}

@keyframes breatheRed {
  0%, 100% { opacity: 0.2; r: 10; }
  50% { opacity: 0.5; r: 11; }
}
@keyframes breatheCoreRed {
  0%, 100% { opacity: 0.8; }
  50% { opacity: 1; }
}

@keyframes breathe {
  0%, 100% { opacity: 0.12; r: 10; }
  50% { opacity: 0.3; r: 11; }
}
@keyframes breatheCore {
  0%, 100% { opacity: 0.5; }
  50% { opacity: 0.8; }
}
</style>
