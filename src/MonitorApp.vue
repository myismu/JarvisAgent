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
  disposeListeners,
  loadSubAgentRunsFromBackend,
  loadPlanDocumentsFromBackend,
  loadAgentRunsFromBackend,
  loadAgentRunEventsFromBackend,
  loadContextSnapshotFromBackend,
} = useAgentEvents();
const { onMonitorSessionChanged, onMonitorLocaleChanged, restoreCurrentWindowState, watchCurrentWindowState } = useWindow();
let unlistenSessionChanged: (() => void) | null = null;
let unlistenLocaleChanged: (() => void) | null = null;
let unwatchWindowState: (() => void) | null = null;
let unlistenFocus: (() => void) | null = null;
// 卸载竞态标记：组件可能在异步注册完成前已卸载，await 返回后需立即释放刚拿到的监听句柄
let isUnmounted = false;

const hydrateMonitorState = async (targetSessionId?: string | null) => {
  const activeSessionId = targetSessionId === undefined
    ? await invoke<string | null>("get_active_session_id")
    : targetSessionId;
  if (isUnmounted) return;
  session.activeSessionId = activeSessionId;

  if (!activeSessionId) {
    session.workingDirectory = null;
    session.setSessionUsageTotals(activeSessionId, {});
    return;
  }

  const meta = await invoke<SessionMeta>("get_session_meta", { id: activeSessionId });
  if (isUnmounted) return;
  session.workingDirectory = meta.workingDirectory || null;
  session.setSessionUsageTotals(activeSessionId, {
    input: meta.totalInputTokens || 0,
    output: meta.totalOutputTokens || 0,
    cacheHit: meta.totalCacheHitTokens || 0,
    cacheMiss: meta.totalCacheMissTokens || 0,
  });

  await Promise.all([
    loadSubAgentRunsFromBackend(activeSessionId),
    loadPlanDocumentsFromBackend(activeSessionId),
    loadAgentRunsFromBackend(activeSessionId, { refreshHistory: false }),
    loadAgentRunEventsFromBackend(activeSessionId),
    loadContextSnapshotFromBackend(activeSessionId),
  ]);
};

onMounted(async () => {
  await restoreCurrentWindowState();
  if (isUnmounted) return;

  const stopWatch = await watchCurrentWindowState();
  if (isUnmounted) {
    stopWatch();
    return;
  }
  unwatchWindowState = stopWatch;

  agent.showAgentPanel = true;
  // 监控窗口只展示运行态、子 Agent、权限与上下文，不订阅聊天流也不重复拉取聊天消息历史
  await initListeners({
    domains: ["governance", "lifecycle"],
    syncChatMessages: false,
  });
  if (isUnmounted) {
    disposeListeners();
    return;
  }

  await hydrateMonitorState();
  if (isUnmounted) return;

  const stopSessionChanged = await onMonitorSessionChanged((sessionId) => {
    void hydrateMonitorState(sessionId);
  });
  if (isUnmounted) {
    stopSessionChanged();
    return;
  }
  unlistenSessionChanged = stopSessionChanged;

  const stopLocaleChanged = await onMonitorLocaleChanged((nextLocale) => {
    locale.value = normalizeLocale(nextLocale);
  });
  if (isUnmounted) {
    stopLocaleChanged();
    return;
  }
  unlistenLocaleChanged = stopLocaleChanged;

  // 每次窗口可见时刷新上下文（必须在同步顶层 onBeforeUnmount 中释放，避免 async 后挂钩失效）
  const stopFocus = await getCurrentWindow().listen('tauri://focus', () => {
    void hydrateMonitorState();
  });
  if (isUnmounted) {
    stopFocus();
    return;
  }
  unlistenFocus = stopFocus;
});

onBeforeUnmount(() => {
  isUnmounted = true;
  unlistenFocus?.();
  unlistenFocus = null;
  unlistenSessionChanged?.();
  unlistenSessionChanged = null;
  unlistenLocaleChanged?.();
  unlistenLocaleChanged = null;
  unwatchWindowState?.();
  unwatchWindowState = null;
  disposeListeners();
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
