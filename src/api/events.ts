/**
 * # events.ts — 前端强类型 Tauri 事件契约层
 *
 * 集中声明 Rust 后端向前端广播的全部事件名与载荷（Payload）映射表 `EventMap`，
 * 并提供强类型监听封装 `listenTyped()`，消灭散落各处的 `listen<any>` 与 `on<any>`。
 *
 * ## Key Exports
 * - `EventMap`: 事件名 -> 强类型载荷映射表（全部对齐 `src/types/generated/contracts.ts`）
 * - `AgentEventName`: 合法事件名联合类型
 * - `listenTyped()`: 强类型 Tauri `listen` 封装（按事件名自动推导 handler 载荷类型）
 *
 * ## Dependencies
 * - Internal: `@/types`
 * - External: `@tauri-apps/api/event`
 */
import { listen } from "@tauri-apps/api/event";
import type { Event, UnlistenFn } from "@tauri-apps/api/event";
import type {
  AgentReadOnlyChangedEvent,
  AgentRun,
  AgentStep,
  AgentStepEvent,
  AgentWorkModeChangedEvent,
  ApprovalModeChangedEvent,
  BackgroundFailedEvent,
  BgCompactingChangedEvent,
  ChatContentEvent,
  ChatStreamEvent,
  ChatThinkingEvent,
  ChatToolDebugEvent,
  ChatToolStartEvent,
  ChatTurnEndEvent,
  ChatTurnStartEvent,
  CheckpointCreatedEvent,
  McpServerStatusEvent,
  MemoryUpdatedEvent,
  PermissionRequest,
  PermissionResolvedPayload,
  PlanDocument,
  PlanProposal,
  PlanProposalStreamEvent,
  SessionAllowancesChangedEvent,
  SessionContextSnapshot,
  SessionMessagesChangedEvent,
  SessionThinkingModeChangedEvent,
  SessionUsageUpdatedPayload,
  SnapshotCreatedEvent,
  SubAgentRun,
  SubagentTraceUpdatedEvent,
  TodoUpdatePayload,
  UserQuestionRequestEvent,
  UserQuestionResolvedEvent,
} from "../types";

/** 兼容流式事件上可能附带的历史轮次别名与子代理标记 */
interface StreamLoopCompat {
  loop?: number;
  loop_count?: number;
  isSubAgent?: boolean;
}

/** `active-session-changed` 事件载荷（会话删除/归档切走时触发） */
export interface ActiveSessionChangedEvent {
  deletedSessionId?: string | null;
  activeSessionId?: string | null;
}

/**
 * `agent-step` 事件载荷：覆盖 Rust 侧 `AgentStepEvent` 判别联合与前端扩展过程步骤。
 * 使用联合类型后，在 `streamEvents.ts` 中按 `step.type` 判断可自动收窄各分支字段类型。
 */
export type AgentStepEventPayload =
  | (AgentStepEvent & { isSubAgent?: boolean })
  | (Omit<AgentStep, "timestamp"> & { sessionId?: string; isSubAgent?: boolean });

/**
 * 全局跨端事件契约映射表（类似 Java 中集中注册的 EventClass 路由表）。
 * 键为后端 `app.emit("event-name", ...)` 的事件名，值为对应载荷类型。
 */
export interface EventMap {
  // === 对话流式事件域 ===
  "chat-turn-start": ChatTurnStartEvent & StreamLoopCompat;
  "chat-content": ChatContentEvent & StreamLoopCompat;
  "chat-thinking": ChatThinkingEvent & StreamLoopCompat;
  "chat-tool-start": ChatToolStartEvent & StreamLoopCompat;
  "chat-tool-debug": ChatToolDebugEvent & StreamLoopCompat;
  "chat-stream": ChatStreamEvent & StreamLoopCompat;
  "chat-turn-end": ChatTurnEndEvent;
  "agent-step": AgentStepEventPayload;

  // === 权限与方案审批事件域 ===
  "permission-request": PermissionRequest;
  "permission-resolved": PermissionResolvedPayload;
  "user-question-request": UserQuestionRequestEvent;
  "user-question-resolved": UserQuestionResolvedEvent;
  "plan-proposal-stream": Omit<PlanProposalStreamEvent, "sessionId"> & { sessionId?: string };
  "plan-proposal": PlanProposal;
  "plan-document-updated": PlanDocument;
  "approval-mode-changed": ApprovalModeChangedEvent;
  "agent-read-only-changed": AgentReadOnlyChangedEvent;
  "agent-work-mode-changed": AgentWorkModeChangedEvent;
  "session-allowances-changed": SessionAllowancesChangedEvent;

  // === 运行态、指标与会话生命周期事件域 ===
  "todo-update": TodoUpdatePayload;
  "memory-updated": Omit<MemoryUpdatedEvent, "sessionId"> & { sessionId?: string };
  "background-failed": Omit<BackgroundFailedEvent, "sessionId" | "port" | "taskType"> & {
    sessionId?: string | null;
    port?: number | null;
    taskType?: string | null;
  };
  "agent-run-updated": AgentRun;
  "context-snapshot-updated": SessionContextSnapshot;
  "session-usage-updated": SessionUsageUpdatedPayload;
  "subagent-updated": SubAgentRun;
  "subagent-trace-updated": Omit<SubagentTraceUpdatedEvent, "loopIndex"> & { loopIndex?: number };
  "checkpoint-created": Omit<CheckpointCreatedEvent, "message"> & {
    message?: string;
    hasOperations?: boolean;
  };
  "session-messages-changed": SessionMessagesChangedEvent;
  "active-session-changed": ActiveSessionChangedEvent;
  "session-thinking-mode-changed": SessionThinkingModeChangedEvent;
  "bg-compacting-changed": BgCompactingChangedEvent;
  "snapshot-created": SnapshotCreatedEvent;
  "mcp-server-status-changed": McpServerStatusEvent;

  // === 无载荷通知事件 ===
  "session-updated": void;
  "session-renamed": void;
  "config-updated": void;
  "ui-preferences-changed": void;
  "bg-task-done": void;
}

export type AgentEventName = keyof EventMap;

/**
 * 强类型 Tauri 事件监听封装。
 * 传入事件名后，TypeScript 会自动推导 `handler` 的 `event.payload` 类型，禁止手写 `<any>`。
 */
export function listenTyped<K extends AgentEventName>(
  event: K,
  handler: (event: Event<EventMap[K]>) => void | Promise<void>,
): Promise<UnlistenFn> {
  return listen<EventMap[K]>(event, handler);
}
