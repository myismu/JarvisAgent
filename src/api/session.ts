/**
 * # session.ts — 会话、项目与对话主链路领域 API
 *
 * 将会话生命周期、项目 CRUD、历史分页、用量查询及 Agent 运行态查询等常用 IPC 收口为
 * 语义化的强类型函数，避免组件与 Store 各自散落裸 `invoke` 与手写泛型。
 *
 * ## Key Exports
 * - 会话与项目 API：`listSessions` / `createSession` / `switchSession` / `getSessionMeta` / `renameSession` / `deleteSession` / `listProjects` / `openProject` / `renameProject` / `archiveProject` 等
 * - 对话与历史 API：`askJarvis` / `resumeJarvis` / `cancelJarvis` / `recallMessage` / `getSessionMessagesPaged` / `getSessionUsage` / `getSessionContextSnapshot` 等
 * - 运行态与轨迹 API：`listAgentRuns` / `listAgentRunEvents` / `listSubagents` / `listSubagentTraceRuns` / `listSubagentTraceEvents` / `listPlanDocuments` / `getSessionTodos` 等
 *
 * ## Dependencies
 * - Internal: `./commands`, `@/types`
 */
import { invokeTyped } from "./commands";
import type { CommandMap } from "./commands";

export const getActiveSessionId = () => invokeTyped("get_active_session_id");
export const clearActiveSessionId = () => invokeTyped("clear_active_session_id");
export const listSessions = () => invokeTyped("list_sessions");
export const listDeletedSessions = () => invokeTyped("list_deleted_sessions");
export const countDeletedSessions = () => invokeTyped("count_deleted_sessions");
export const createSession = (projectId: string | null) =>
  invokeTyped("create_session", { projectId });
export const switchSession = (id: string) => invokeTyped("switch_session", { id });
export const getSessionMeta = (id: string) => invokeTyped("get_session_meta", { id });
export const deleteSession = (id: string) => invokeTyped("delete_session", { id });
export const renameSession = (id: string, title: string) =>
  invokeTyped("rename_session", { id, title });
export const updateSessionProfile = (id: string, profileId: string) =>
  invokeTyped("update_session_profile", { id, profileId });

export const setSessionWorkMode = (sessionId: string, mode: string) =>
  invokeTyped("set_session_work_mode", { sessionId, mode });
export const setSessionThinkingEnabled = (id: string, enabled: boolean) =>
  invokeTyped("set_session_thinking_enabled", { id, enabled });
export const getSessionContextSnapshot = (sessionId: string) =>
  invokeTyped("get_session_context_snapshot", { sessionId });
export const getSessionUsage = (sessionId: string) =>
  invokeTyped("get_session_usage", { sessionId });
export const getSessionMessagesPaged = (
  sessionId: string,
  beforeSeq: number | null,
  limit = 5,
) => invokeTyped("get_session_messages_paged", { sessionId, beforeSeq, limit });
export const recoverInterruptedSessionMessages = (sessionId: string) =>
  invokeTyped("recover_interrupted_session_messages", { sessionId });

export const askJarvis = (args: CommandMap["ask_jarvis"]["args"]) =>
  invokeTyped("ask_jarvis", args);
export const resumeJarvis = (sessionId: string, reason: string) =>
  invokeTyped("resume_jarvis", { sessionId, reason });
export const cancelJarvis = (sessionId: string) => invokeTyped("cancel_jarvis", { sessionId });
export const recallMessage = (args: CommandMap["recall_message"]["args"]) =>
  invokeTyped("recall_message", args);

export const listAgentRuns = (sessionId: string | null) =>
  invokeTyped("list_agent_runs", { sessionId });
export const listAgentRunEvents = (sessionId: string | null, runId: string | null = null) =>
  invokeTyped("list_agent_run_events", { sessionId, runId });
export const prepareResumeAgentRun = (runId: string) =>
  invokeTyped("prepare_resume_agent_run", { runId });
export const listPlanDocuments = (sessionId: string) =>
  invokeTyped("list_plan_documents", { sessionId });
export const getSessionTodos = (sessionId: string) =>
  invokeTyped("get_session_todos", { sessionId });
export const listSubagents = (sessionId: string | null) =>
  invokeTyped("list_subagents", { sessionId });
export const listSubagentTraceRuns = (sessionId: string) =>
  invokeTyped("list_subagent_trace_runs", { sessionId });
export const listSubagentTraceEvents = (runId: string) =>
  invokeTyped("list_subagent_trace_events", { runId });
export const cancelSubagentRun = (runId: string) =>
  invokeTyped("cancel_subagent_run", { runId });

export const listProjects = () => invokeTyped("list_projects");
export const openProject = (path: string) => invokeTyped("open_project", { path });
export const renameProject = (id: string, name: string) =>
  invokeTyped("rename_project", { id, name });
export const archiveProject = (id: string) => invokeTyped("archive_project", { id });
