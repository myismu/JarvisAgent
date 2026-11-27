/**
 * # commands.ts — 前端强类型 Tauri IPC 命令契约层
 *
 * 集中定义 Rust 后端 `#[tauri::command]` 的「命令名 -> 入参类型 -> 返回类型」映射表 `CommandMap`，
 * 所有跨端 DTO 直接复用 `src/types`（由 Rust 侧 `ts-rs` 自动生成），并提供 `invokeTyped()` 统一调用入口。
 *
 * ## Key Exports
 * - `CommandMap`: 强类型 IPC 命令契约映射表
 * - `CommandName`: 合法命令名联合类型
 * - `invokeTyped()`: 编译期强校验参数与返回值的 Tauri `invoke` 包装
 *
 * ## Dependencies
 * - Internal: `@/types`
 * - External: `@tauri-apps/api/core`
 *
 * ## Constraints
 * - 底层直调 `@tauri-apps/api/core` 的 `invoke`，确保现有 Vitest `vi.mock("@tauri-apps/api/core")` 透明生效
 * - 无参数命令（`args: undefined`）调用时不传第二个参数，保持与原生 `invoke(cmd)` 行为一致
 */
import { invoke } from "@tauri-apps/api/core";
import type {
  AgentRun,
  AgentRunLoopEvent,
  AppConfig,
  BackgroundTask,
  BatchOpResult,
  JarvisResult,
  PagedSessionMessages,
  PermissionStateView,
  PlanDocument,
  ProjectMeta,
  ResolvePermissionResult,
  ResolveUserQuestionResult,
  ResumeAgentRunPlan,
  SessionContextSnapshot,
  SessionMessage,
  SessionMeta,
  SessionPermissionSettingsView,
  SessionThinkingSnapshot,
  SessionUsageView,
  SubAgentRun,
  SubAgentTraceEvent,
  SubAgentTraceRunSummary,
  TodoGroup,
  UsageBreakdown,
  UserQuestionAnswerItem,
} from "../types";

/**
 * 强类型 Tauri 命令契约表（类似 Java Feign Client 的接口声明）。
 * 每条命令精确约束 `args`（入参对象，无参为 `undefined`）和 `result`（返回值类型）。
 */
export interface CommandMap {
  // === 会话生命周期与元数据 ===
  get_active_session_id: { args: undefined; result: string | null };
  clear_active_session_id: { args: undefined; result: void };
  list_sessions: { args: undefined; result: SessionMeta[] };
  list_deleted_sessions: { args: undefined; result: SessionMeta[] };
  count_deleted_sessions: { args: undefined; result: number };
  create_session: { args: { projectId: string | null }; result: SessionMeta };
  switch_session: { args: { id: string }; result: SessionMeta };
  get_session_meta: { args: { id: string }; result: SessionMeta };
  delete_session: { args: { id: string }; result: void };
  rename_session: { args: { id: string; title: string }; result: SessionMeta };
  restore_session: { args: { id: string }; result: void };
  purge_session: { args: { id: string }; result: void };
  restore_sessions: { args: { ids: string[] }; result: BatchOpResult };
  purge_sessions: { args: { ids: string[] }; result: BatchOpResult };
  update_session_profile: { args: { id: string; profileId: string }; result: void };

  // === 会话模式、深度思考、上下文与用量 ===
  get_session_work_mode: { args: { sessionId: string }; result: string };
  set_session_work_mode: { args: { sessionId: string; mode: string }; result: void };
  get_session_thinking: { args: { id: string }; result: SessionThinkingSnapshot };
  get_pending_thinking_enabled: { args: undefined; result: SessionThinkingSnapshot };
  set_session_thinking_enabled: {
    args: { id: string; enabled: boolean };
    result: SessionThinkingSnapshot;
  };
  get_session_context_snapshot: {
    args: { sessionId: string };
    result: SessionContextSnapshot | null;
  };
  get_session_usage: { args: { sessionId: string }; result: SessionUsageView };
  get_session_usage_breakdown: { args: { sessionId: string }; result: UsageBreakdown };
  compact_conversation: { args: { sessionId: string }; result: string };
  is_session_compacting: { args: { sessionId: string }; result: boolean };

  // === 对话主流程、历史分页与撤回 ===
  ask_jarvis: {
    args: {
      sessionId: string;
      msg: string;
      thinkingOverride: boolean | null;
      imageBase64List: string[] | null;
      agentDisplayMode: string;
      reflectionMode: string;
      displayMsg: string | null;
    };
    result: JarvisResult;
  };
  resume_jarvis: { args: { sessionId: string; reason: string }; result: JarvisResult };
  cancel_jarvis: { args: { sessionId: string }; result: void };
  recall_message: {
    args: {
      sessionId: string;
      messageId: string | null;
      userMessageIndex?: number | null;
      pruneMetadataCutoff: number | null;
    };
    result: string;
  };
  get_session_messages: { args: { sessionId: string }; result: SessionMessage[] };
  get_session_messages_paged: {
    args: { sessionId: string; beforeSeq: number | null; limit: number };
    result: PagedSessionMessages;
  };
  recover_interrupted_session_messages: { args: { sessionId: string }; result: boolean };

  // === Agent 运行态、计划、待办与子代理 ===
  list_agent_runs: { args: { sessionId: string | null }; result: AgentRun[] };
  list_agent_run_events: {
    args: { sessionId: string | null; runId: string | null };
    result: AgentRunLoopEvent[];
  };
  prepare_resume_agent_run: { args: { runId: string }; result: ResumeAgentRunPlan };
  list_plan_documents: { args: { sessionId: string }; result: PlanDocument[] };
  get_session_todos: { args: { sessionId: string }; result: TodoGroup[] };
  list_subagents: { args: { sessionId: string | null }; result: SubAgentRun[] };
  list_subagent_trace_runs: { args: { sessionId: string }; result: SubAgentTraceRunSummary[] };
  list_subagent_trace_events: { args: { runId: string }; result: SubAgentTraceEvent[] };
  cancel_subagent_run: { args: { runId: string }; result: SubAgentRun };
  get_background_tasks: { args: { sessionId: string | null }; result: BackgroundTask[] };
  dismiss_background_task: { args: { taskId: string }; result: boolean };
  kill_background_task: { args: { taskId: string }; result: boolean };
  clear_session_background_tasks: { args: { sessionId: string }; result: number };

  // === 项目管理 ===
  list_projects: { args: undefined; result: ProjectMeta[] };
  open_project: { args: { path: string }; result: ProjectMeta };
  rename_project: { args: { id: string; name: string }; result: ProjectMeta };
  archive_project: { args: { id: string }; result: number };
  get_workspace_dir: { args: { sessionId: string }; result: string | null };

  // === 权限控制与用户咨询 ===
  get_permission_state: { args: { sessionId: string }; result: PermissionStateView };
  get_session_permission_settings: {
    args: { sessionId: string };
    result: SessionPermissionSettingsView;
  };
  resolve_permission: {
    args: {
      id: string;
      sessionId: string | null;
      decision: string;
      content: string | null;
    };
    result: ResolvePermissionResult;
  };
  set_session_approval_mode: { args: { sessionId: string; mode: string }; result: void };
  set_agent_read_only: { args: { sessionId: string; enabled: boolean }; result: void };
  clear_session_allowances: { args: { sessionId: string }; result: void };
  revoke_session_allowance: {
    args: { sessionId: string; kind: string; scope: string };
    result: void;
  };
  resolve_user_question: {
    args: {
      id: string;
      sessionId: string;
      status: string;
      answers?: UserQuestionAnswerItem[] | null;
    };
    result: ResolveUserQuestionResult;
  };

  // === 应用配置 ===
  get_config: { args: undefined; result: AppConfig };
  save_config_cmd: { args: { newConfig: AppConfig }; result: void };
}

export type CommandName = keyof CommandMap;

type VoidArgCommands = {
  [K in CommandName]: CommandMap[K]["args"] extends undefined ? K : never;
}[CommandName];

type RequiredArgCommands = Exclude<CommandName, VoidArgCommands>;

/**
 * 强类型 IPC 调用入口：根据命令名自动校验入参结构并推导返回值类型。
 */
export function invokeTyped<K extends VoidArgCommands>(
  cmd: K,
): Promise<CommandMap[K]["result"]>;
export function invokeTyped<K extends RequiredArgCommands>(
  cmd: K,
  args: CommandMap[K]["args"],
): Promise<CommandMap[K]["result"]>;
export function invokeTyped<K extends CommandName>(
  cmd: K,
  args?: CommandMap[K]["args"],
): Promise<CommandMap[K]["result"]> {
  if (args === undefined) {
    return invoke<CommandMap[K]["result"]>(cmd);
  }
  return invoke<CommandMap[K]["result"]>(cmd, args as Record<string, unknown>);
}
