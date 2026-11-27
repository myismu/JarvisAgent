/**
 * # config.ts — 应用配置领域 API
 *
 * 封装模型配置与预设的强类型读写 IPC，避免在组件与 Store 中直接 `invoke<any>('get_config')`。
 *
 * ## Key Exports
 * - `getConfig()`: 读取完整 `AppConfig`
 * - `saveConfig()`: 保存完整 `AppConfig`
 *
 * ## Dependencies
 * - Internal: `./commands`, `@/types`
 */
import type { AppConfig } from "../types";
import { invokeTyped } from "./commands";

export const getConfig = () => invokeTyped("get_config");
export const saveConfig = (newConfig: AppConfig) =>
  invokeTyped("save_config_cmd", { newConfig });
