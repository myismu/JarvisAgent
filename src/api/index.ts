/**
 * # index.ts — 前端强类型 IPC 与事件 API 统一出口
 *
 * 汇总导出通用调用原语（`invokeTyped` / `listenTyped`）以及会话、配置、权限领域 API。
 *
 * ## Key Exports
 * - `invokeTyped` / `CommandMap` / `CommandName`
 * - `listenTyped` / `EventMap` / `AgentEventName`
 * - 会话、配置、权限领域 API 函数
 */
export * from "./commands";
export * from "./events";
export * from "./session";
export * from "./config";
export * from "./permission";
