/**
 * # useAgentEvents.ts — Agent 事件监听与历史恢复入口
 *
 * 装配 `agentEvents/` 下的各事件域处理器（权限审批、流式对话、运行态与生命周期），
 * 管理跨 Vite HMR 生命周期的全局监听器去重与注销，并对外导出后端状态加载方法。
 *
 * ## Key Exports
 * - `useAgentEvents()`: 提供 `initListeners(options?)`、`disposeListeners()` 及 7 个后端运行态/轨迹加载方法
 *
 * ## Dependencies
 * - Internal: `./agentEvents/*`, `@/stores/session`, `@/stores/chat`, `@/stores/agent`, `@/stores/permission`
 * - External: `@tauri-apps/api/event`, `vue-i18n`
 *
 * ## Constraints
 * - `initListeners()` 挂载全局 `window.__jarvisUnlisteners`，跨 HMR 自动注销旧监听器与域内定时器
 * - 注册中途失败会回滚已注册句柄并确保 `__jarvisListenersInitializing` 复位，不会永久锁死
 */
import type { UnlistenFn } from "@tauri-apps/api/event";
import { useI18n } from "vue-i18n";
import { listenTyped } from "../api/events";
import { useSessionStore } from "../stores/session";
import { useChatStore } from "../stores/chat";
import { useAgentStore } from "../stores/agent";
import { usePermissionStore } from "../stores/permission";
import type { AgentRun } from "../types";
import {
  applyAgentRunState as applyRunStateToView,
  backfillRunningUserMessageId as backfillUserMsgId,
} from "./agentEvents/runState";
import { createAgentEventsLoaders } from "./agentEvents/loaders";
import { registerGovernanceEvents } from "./agentEvents/governanceEvents";
import { registerStreamEvents } from "./agentEvents/streamEvents";
import { registerLifecycleEvents } from "./agentEvents/lifecycleEvents";
import type {
  AgentEventDomain,
  AgentEventsContext,
  EventRegisterFn,
  InitAgentListenersOptions,
} from "./agentEvents/types";

const ALL_DOMAINS: AgentEventDomain[] = ["governance", "stream", "lifecycle"];

// 将初始化标志和 unlisten 句柄挂在 window 上，跨 Vite HMR 生命周期持久化
declare global {
  interface Window {
    __jarvisListenersInitialized?: boolean;
    __jarvisListenersInitializing?: boolean;
    __jarvisUnlisteners?: UnlistenFn[];
  }
}

/** 清理当前窗口已注册的全部事件监听器与域内定时器，并复位全局状态标志 */
export function disposeAgentListeners(): void {
  if (typeof window === "undefined") return;
  if (window.__jarvisUnlisteners) {
    for (const unlisten of window.__jarvisUnlisteners) {
      unlisten();
    }
    window.__jarvisUnlisteners = undefined;
  }
  window.__jarvisListenersInitialized = false;
  window.__jarvisListenersInitializing = false;
}

export function useAgentEvents() {
  const { t } = useI18n();
  const session = useSessionStore();
  const chat = useChatStore();
  const agent = useAgentStore();
  const perm = usePermissionStore();
  const dismissedInterruptedRuns = new Set<string>();

  function syncActiveSessionView(sessionId: string | null | undefined, followScroll = false) {
    if (sessionId === session.activeSessionId) {
      chat.triggerRender();
      if (followScroll) chat.followScrollToBottom();
    }
  }

  function applyAgentRunState(sessionId: string, runs: AgentRun[]) {
    applyRunStateToView(session, perm, dismissedInterruptedRuns, sessionId, runs);
  }

  function backfillRunningUserMessageId(run: AgentRun) {
    backfillUserMsgId(session, run);
  }

  const loaders = createAgentEventsLoaders({
    session,
    chat,
    agent,
    perm,
    syncActiveSessionView,
    applyAgentRunState,
  });

  const ctx: AgentEventsContext = {
    t: (key, params) => (params ? t(key, params) : t(key)),
    session,
    chat,
    agent,
    perm,
    dismissedInterruptedRuns,
    syncActiveSessionView,
    applyAgentRunState,
    backfillRunningUserMessageId,
    loaders,
  };

  const initListeners = async (options: InitAgentListenersOptions = {}) => {
    // 跨 HMR 生命周期：如果当前 window 已有监听器，先全部清理再重新注册
    if (window.__jarvisUnlisteners) {
      console.warn("[JarvisAgent] HMR 检测：清理旧事件监听器，重新注册");
      disposeAgentListeners();
    }

    if (window.__jarvisListenersInitializing) {
      console.warn("[JarvisAgent] 事件监听器正在注册，跳过重复注册");
      return;
    }

    if (window.__jarvisListenersInitialized) {
      if (window.__jarvisUnlisteners) {
        console.warn("[JarvisAgent] 事件监听器已全局注册，跳过重复注册");
        return;
      }
      console.warn("[JarvisAgent] 检测到过期监听器标志，重新注册");
      window.__jarvisListenersInitialized = false;
    }

    const enabledDomains = new Set<AgentEventDomain>(options.domains ?? ALL_DOMAINS);
    const syncChatMessages = options.syncChatMessages ?? enabledDomains.has("stream");

    window.__jarvisListenersInitializing = true;
    const unlisteners: UnlistenFn[] = [];
    const on: EventRegisterFn = async (event, handler) => {
      const unlisten = await listenTyped(event, handler);
      unlisteners.push(unlisten);
    };

    try {
      if (enabledDomains.has("governance")) {
        await registerGovernanceEvents(on, ctx);
      }
      if (enabledDomains.has("stream")) {
        await registerStreamEvents(on, ctx);
      }
      if (enabledDomains.has("lifecycle")) {
        const cleanupLifecycle = await registerLifecycleEvents(on, ctx, {
          syncChatMessages,
        });
        unlisteners.push(cleanupLifecycle);
      }

      // 保存所有 unlisten 句柄到 window 级别（跨 HMR 生命周期）
      window.__jarvisUnlisteners = unlisteners;
      window.__jarvisListenersInitialized = true;
    } catch (err) {
      // 注册中途抛错时回滚已注册句柄与定时器，防止半初始化泄漏或永久锁死
      for (const unlisten of unlisteners) {
        try {
          unlisten();
        } catch {
          // 忽略单项回滚异常，确保其余句柄继续释放
        }
      }
      window.__jarvisUnlisteners = undefined;
      window.__jarvisListenersInitialized = false;
      throw err;
    } finally {
      window.__jarvisListenersInitializing = false;
    }

    // Vite HMR 清理：热更新时自动注销旧监听器与定时器
    if (import.meta.hot) {
      import.meta.hot.dispose(() => {
        disposeAgentListeners();
      });
    }
  };

  return {
    initListeners,
    disposeListeners: disposeAgentListeners,
    loadSubAgentRunsFromBackend: loaders.loadSubAgentRunsFromBackend,
    loadSubAgentTraceFromBackend: loaders.loadSubAgentTraceFromBackend,
    loadPlanDocumentsFromBackend: loaders.loadPlanDocumentsFromBackend,
    loadTodosFromBackend: loaders.loadTodosFromBackend,
    loadAgentRunsFromBackend: loaders.loadAgentRunsFromBackend,
    loadAgentRunEventsFromBackend: loaders.loadAgentRunEventsFromBackend,
    loadContextSnapshotFromBackend: loaders.loadContextSnapshotFromBackend,
  };
}
