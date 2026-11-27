/**
 * # types.ts — Agent 事件中枢共享上下文与载荷类型定义
 *
 * 为 `agentEvents/` 下各事件域子模块提供统一的依赖上下文（stores、i18n、
 * 辅助调度与加载器），使事件处理逻辑与 Tauri 运行时 `listen()` 解耦，便于单测驱动。
 *
 * ## Key Exports
 * - `EventRegisterFn`: 抽象的事件注册函数签名（生产接 Tauri `listen`，测试可注入内存分发器）
 * - `AgentEventDomain`: 事件域标识（`governance` / `stream` / `lifecycle`）
 * - `InitAgentListenersOptions` / `LifecycleEventsOptions`: 按需订阅与聊天同步开关选项
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
import type { AgentRun, SessionMessagesChangedEvent } from "../../types";
import type { ActiveSessionChangedEvent, AgentEventName, EventMap } from "../../api/events";

/** 事件域枚举：支持主窗口全量订阅与独立监控窗口按需订阅 */
export type AgentEventDomain = "governance" | "stream" | "lifecycle";

/** `initListeners` 初始化选项 */
export interface InitAgentListenersOptions {
  /** 需要注册的事件域，默认全部注册 `["governance", "stream", "lifecycle"]` */
  domains?: AgentEventDomain[];
  /**
   * 是否在生命周期事件中同步主聊天区消息历史与提示条。
   * 主窗口默认 `true`；监控窗口不渲染聊天气泡，传 `false` 可避免重复发分页 IPC。
   */
  syncChatMessages?: boolean;
}

/** 生命周期事件域注册选项 */
export interface LifecycleEventsOptions {
  syncChatMessages?: boolean;
}

/**
 * 强类型事件监听注册函数（类似 Java 的泛型 EventBus.subscribe）。
 * 传入合法事件名 `K` 后，自动根据 `EventMap[K]` 推导回调参数类型，禁止 `on<any>`。
 */
export type EventRegisterFn = <K extends AgentEventName>(
  event: K,
  handler: (event: { payload: EventMap[K] }) => void | Promise<void>,
) => Promise<void>;

export type SessionCleanupPayload = ActiveSessionChangedEvent;

/** 复用 Rust 侧 ts-rs 自动生成的事件契约，消除手工双份维护 */
export type SessionMessagesChangedPayload = SessionMessagesChangedEvent;

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
