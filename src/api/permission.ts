/**
 * # permission.ts — 权限控制与会话授权领域 API
 *
 * 封装权限待办状态读取、权限/方案审批决议、权限档位与只读模式设置、
 * 以及会话级“已允许”清单的撤销与清空操作。
 *
 * ## Key Exports
 * - `getPermissionState()` / `resolvePermission()`
 * - `setSessionApprovalMode()` / `setAgentReadOnly()`
 * - `clearSessionAllowances()` / `revokeSessionAllowance()`
 *
 * ## Dependencies
 * - Internal: `./commands`
 */
import { invokeTyped } from "./commands";
import type { CommandMap } from "./commands";

export const getPermissionState = (sessionId: string) =>
  invokeTyped("get_permission_state", { sessionId });

export const resolvePermission = (args: CommandMap["resolve_permission"]["args"]) =>
  invokeTyped("resolve_permission", args);

export const setSessionApprovalMode = (sessionId: string, mode: string) =>
  invokeTyped("set_session_approval_mode", { sessionId, mode });

export const setAgentReadOnly = (sessionId: string, enabled: boolean) =>
  invokeTyped("set_agent_read_only", { sessionId, enabled });

export const clearSessionAllowances = (sessionId: string) =>
  invokeTyped("clear_session_allowances", { sessionId });

export const revokeSessionAllowance = (sessionId: string, kind: string, scope: string) =>
  invokeTyped("revoke_session_allowance", { sessionId, kind, scope });
