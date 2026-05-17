<!--
# MonitorApp.vue — 独立执行监控窗口

承载独立 Tauri 监控窗口，初始化 Agent 事件监听并复用 AgentPanel 展示执行状态。

## Key Exports
- `MonitorApp`: 监控窗口根组件

## Dependencies
- Internal: `useAgentEvents`, `useAgentStore`, `AgentPanel`
-->
<script setup lang="ts">
import { onBeforeUnmount, onMounted } from "vue";
import { useI18n } from "vue-i18n";
import { invoke } from "@tauri-apps/api/core";
import { getCurrentWindow } from '@tauri-apps/api/window';
import { useAgentEvents } from "./composables/useAgentEvents";
import { useWindow } from "./composables/useWindow";
import { useTheme } from "./composables/useTheme";
import { usePreferences } from "./composables/usePreferences";
import { useAgentStore } from "./stores/agent";
import { useSessionStore } from "./stores/session";
import { normalizeLocale } from "./i18n";
import type { SessionMeta } from "./types";
import AgentPanel from "./components/chat/AgentPanel.vue";

useTheme(); // 初始化主题并监听同步
usePreferences(); // 初始化偏好设置并监听跨窗口同步

const agent = useAgentStore();
const session = useSessionStore();
const { locale } = useI18n();
const {
  initListeners,
  loadSubAgentRunsFromBackend,
  loadSubAgentEventsFromBackend,
  loadPlanDocumentsFromBackend,
  loadAgentRunsFromBackend,
  loadAgentRunEventsFromBackend,
  loadContextSnapshotFromBackend,
} = useAgentEvents();
const { onMonitorSessionChanged, onMonitorLocaleChanged, restoreCurrentWindowState, watchCurrentWindowState } = useWindow();
let unlistenSessionChanged: (() => void) | null = null;
let unlistenLocaleChanged: (() => void) | null = null;
let unwatchWindowState: (() => void) | null = null;

const hydrateMonitorState = async (targetSessionId?: string | null) => {
  const activeSessionId = targetSessionId === undefined
    ? await invoke<string | null>("get_active_session_id")
    : targetSessionId;
  session.activeSessionId = activeSessionId;

  if (!activeSessionId) {
    session.workingDirectory = null;
    session.setSessionUsageTotals(activeSessionId, 0, 0);
    return;
  }

  const meta = await invoke<SessionMeta>("get_session_meta", { id: activeSessionId });
  session.workingDirectory = meta.workingDirectory || null;
  session.setSessionUsageTotals(activeSessionId, meta.totalInputTokens || 0, meta.totalOutputTokens || 0);

  await Promise.all([
    loadSubAgentRunsFromBackend(activeSessionId),
    loadSubAgentEventsFromBackend(activeSessionId),
    loadPlanDocumentsFromBackend(activeSessionId),
    loadAgentRunsFromBackend(activeSessionId, { refreshHistory: false }),
    loadAgentRunEventsFromBackend(activeSessionId),
    loadContextSnapshotFromBackend(activeSessionId),
  ]);
};

onMounted(async () => {
  await restoreCurrentWindowState();
  unwatchWindowState = await watchCurrentWindowState();
  agent.showAgentPanel = true;
  await initListeners();
  await hydrateMonitorState();
  unlistenSessionChanged = await onMonitorSessionChanged((sessionId) => {
    hydrateMonitorState(sessionId);
  });
  unlistenLocaleChanged = await onMonitorLocaleChanged((nextLocale) => {
    locale.value = normalizeLocale(nextLocale);
  });
  // 每次窗口可见时刷新上下文
  const unlistenFocus = await getCurrentWindow().listen('tauri://focus', () => hydrateMonitorState());
  onBeforeUnmount(() => { unlistenFocus(); });
});

onBeforeUnmount(() => {
  unlistenSessionChanged?.();
  unlistenLocaleChanged?.();
  unwatchWindowState?.();
});
</script>

<template>
  <main class="monitor-window">
    <AgentPanel standalone />
  </main>
</template>

<style scoped>
.monitor-window {
  width: 100%;
  height: 100%;
  overflow: hidden;
  background: var(--bg-dark);
  color: var(--text-main);
  font-family: var(--font-sans);
}
</style>
