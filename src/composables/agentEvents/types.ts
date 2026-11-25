/**
 * # types.ts — Agent 事件中枢共享上下文与载荷类型定义
 *
 * 为 `agentEvents/` 下各事件域子模块提供统一的依赖上下文（stores、i18n、
 * 辅助调度与加载器），使事件处理逻辑与 Tauri 运行时 `listen()` 解耦，便于单测驱动。
 *
 * ## Key Exports
 * - `EventRegisterFn`: 抽象的事件注册函数签名（生产接 Tauri `listen`，测试可注入内存分发器）
 * - `AgentEventsLoaders`: 后端数据加载器集合
 * - `AgentEventsContext`: 各事件域共享的上下文对象
 * - `SessionMessagesChangedPayload` / `SessionCleanupPayload`: 事件载荷契约
 *
 * ## Dependencies
 * - Internal: `@/stores/session`, `@/stores/chat`, `@/stores/agent`, `@/stores/permission`, `@/types`
 */
import type { useSessionStore } from "../../stores/session";
import type { useChatStore } from "../../stores/chat";
import type { useAgentStore } from "../../stores/agent";
import type { usePermissionStore } from "../../stores/permission";
import type { AgentRun } from "../../types";

/** 抽象事件监听注册函数（类似 Java 的 EventBus.subscribe） */
export type EventRegisterFn = <T>(
  event: string,
  handler: (event: { payload: T }) => void | Promise<void>,
) => Promise<void>;

export interface SessionCleanupPayload {
  deletedSessionId?: string | null;
  activeSessionId?: string | null;
}

export interface SessionMessagesChangedPayload {
  sessionId: string;
  count: number;
  lastSeq: number | null;
  reason: string;
}

export interface AgentEventsLoaders {
  refreshSessionHistory: (sessionId: string, reason: string) => Promise<void>;
  loadSubAgentRunsFromBackend: (sid?: string | null) => Promise<void>;
  loadSubAgentTraceFromBackend: (runId: string) => Promise<void>;
  loadPlanDocumentsFromBackend: (sid?: string | null) => Promise<void>;
  loadTodosFromBackend: (sid?: string | null) => Promise<void>;
  loadAgentRunsFromBackend: (
    sid?: string | null,
    options?: { refreshHistory?: boolean },
  ) => Promise<void>;
  loadAgentRunEventsFromBackend: (sid?: string | null) => Promise<void>;
  loadContextSnapshotFromBackend: (sid?: string | null) => Promise<void>;
}

/** 各事件域子模块共享的依赖上下文 */
export interface AgentEventsContext {
  t: (key: string, params?: Record<string, unknown>) => string;
  session: ReturnType<typeof useSessionStore>;
  chat: ReturnType<typeof useChatStore>;
  agent: ReturnType<typeof useAgentStore>;
  perm: ReturnType<typeof usePermissionStore>;
  /** 已被新运行取代或用户忽略的可恢复中断 runId 集合 */
  dismissedInterruptedRuns: Set<string>;
  syncActiveSessionView: (sessionId: string | null | undefined, followScroll?: boolean) => void;
  applyAgentRunState: (sessionId: string, runs: AgentRun[]) => void;
  backfillRunningUserMessageId: (run: AgentRun) => void;
  loaders: AgentEventsLoaders;
}
