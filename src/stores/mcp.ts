//! # stores/mcp.ts — MCP 扩展服务前端状态管理
//!
//! 集中管理已配置 MCP 服务列表、实时运行状态事件监听（`mcp-server-status`），
//! 以及对后端 `command/mcp.rs` 全部命令的调用封装。

import { defineStore } from "pinia";
import { ref } from "vue";
import { invoke } from "@tauri-apps/api/core";
import { listen, type UnlistenFn } from "@tauri-apps/api/event";
import type {
  McpImportItemResult,
  McpPromptItemView,
  McpPromptRenderResult,
  McpResourceItemView,
  McpResourceReadResult,
  McpServerConfig,
  McpServerStatusEvent,
  McpServerSummary,
  McpTestConfigResult,
  McpToolView,
} from "../types/generated/contracts";

export const useMcpStore = defineStore("mcp", () => {
  const servers = ref<McpServerSummary[]>([]);
  const loading = ref(false);
  const error = ref<string | null>(null);
  let unlistenStatus: UnlistenFn | null = null;

  /** 加载全部 MCP 服务摘要 */
  async function loadServers() {
    loading.value = true;
    error.value = null;
    try {
      servers.value = await invoke<McpServerSummary[]>("mcp_list_servers");
    } catch (err) {
      error.value = String(err);
    } finally {
      loading.value = false;
    }
  }

  /** 初始化后端 `mcp-server-status` 事件监听（防重复注册） */
  async function initListener() {
    if (unlistenStatus) return;
    unlistenStatus = await listen<McpServerStatusEvent>(
      "mcp-server-status",
      (event) => {
        const payload = event.payload;
        const target = servers.value.find((s) => s.name === payload.serverName);
        if (target) {
          target.status = payload.status;
          target.toolCount = payload.toolCount;
          target.resourceCount = payload.resourceCount;
          target.promptCount = payload.promptCount;
          target.lastError = payload.lastError;
          if (payload.status === "ready") {
            target.hasCachedCapabilities = true;
          }
        } else {
          // 新增的服务在本地列表里还不存在时，刷新一次列表
          void loadServers();
        }
      },
    );
  }

  /** 卸载事件监听 */
  function disposeListener() {
    if (unlistenStatus) {
      unlistenStatus();
      unlistenStatus = null;
    }
  }

  /** 保存（新增或更新）MCP 服务配置 */
  async function saveServer(
    name: string,
    config: McpServerConfig,
  ): Promise<McpServerSummary> {
    const summary = await invoke<McpServerSummary>("mcp_save_server", {
      name,
      config,
    });
    await loadServers();
    return summary;
  }

  /** 删除 MCP 服务 */
  async function deleteServer(name: string): Promise<void> {
    await invoke("mcp_delete_server", { name });
    servers.value = servers.value.filter((s) => s.name !== name);
  }

  /** 批量导入 `mcpServers` JSON */
  async function importJson(rawJson: string): Promise<McpImportItemResult[]> {
    const report = await invoke<McpImportItemResult[]>("mcp_import_json", {
      rawJson,
    });
    await loadServers();
    return report;
  }

  /** 启停单个服务（立即生效） */
  async function setServerEnabled(name: string, enabled: boolean): Promise<void> {
    await invoke("mcp_set_server_enabled", { name, enabled });
    const target = servers.value.find((s) => s.name === name);
    if (target) {
      target.enabled = enabled;
      target.config.enabled = enabled;
      target.status = enabled ? "idle" : "disabled";
    }
  }

  /** 切换服务信任状态（默认不信任；打开后才享受帮我批准/完全批准自动放行） */
  async function setServerTrusted(name: string, trusted: boolean): Promise<void> {
    await invoke("mcp_set_server_trusted", { name, trusted });
    const target = servers.value.find((s) => s.name === name);
    if (target) {
      target.trusted = trusted;
      target.config.trusted = trusted;
    }
  }

  /** 手动启动或重启服务 */
  async function restartServer(name: string): Promise<McpServerSummary> {
    const summary = await invoke<McpServerSummary>("mcp_restart_server", { name });
    await loadServers();
    return summary;
  }

  /** 手动停止运行中的服务进程，回到待机态（不关闭启用开关） */
  async function stopServer(name: string): Promise<void> {
    await invoke("mcp_stop_server", { name });
    const target = servers.value.find((s) => s.name === name);
    if (target && target.enabled) {
      target.status = "idle";
      target.lastError = undefined;
    }
  }

  /** 不落盘测试连接 */
  async function testConfig(config: McpServerConfig): Promise<McpTestConfigResult> {
    return invoke<McpTestConfigResult>("mcp_test_config", { config });
  }

  /** 获取服务工具列表 */
  async function listTools(serverName?: string): Promise<McpToolView[]> {
    return invoke<McpToolView[]>("mcp_list_tools", {
      serverName: serverName ?? null,
    });
  }

  /** 开关单个 MCP 工具（立即生效） */
  async function setToolEnabled(
    serverName: string,
    toolName: string,
    enabled: boolean,
  ): Promise<void> {
    await invoke("mcp_set_tool_enabled", { serverName, toolName, enabled });
  }

  /** 获取资源列表 */
  async function listResources(serverName?: string): Promise<McpResourceItemView[]> {
    return invoke<McpResourceItemView[]>("mcp_list_resources", {
      serverName: serverName ?? null,
    });
  }

  /** 读取资源内容 */
  async function readResource(
    serverName: string,
    uri: string,
  ): Promise<McpResourceReadResult> {
    return invoke<McpResourceReadResult>("mcp_read_resource", {
      serverName,
      uri,
    });
  }

  /** 获取 Prompt 模板列表 */
  async function listPrompts(serverName?: string): Promise<McpPromptItemView[]> {
    return invoke<McpPromptItemView[]>("mcp_list_prompts", {
      serverName: serverName ?? null,
    });
  }

  /** 渲染 Prompt 模板 */
  async function getPrompt(
    serverName: string,
    promptName: string,
    args?: Record<string, string>,
  ): Promise<McpPromptRenderResult> {
    return invoke<McpPromptRenderResult>("mcp_get_prompt", {
      serverName,
      promptName,
      arguments: args ?? null,
    });
  }

  /** 读取服务日志环形缓冲 */
  async function getServerLogs(name: string): Promise<string[]> {
    return invoke<string[]>("mcp_get_server_logs", { name });
  }

  return {
    servers,
    loading,
    error,
    loadServers,
    initListener,
    disposeListener,
    saveServer,
    deleteServer,
    importJson,
    setServerEnabled,
    setServerTrusted,
    restartServer,
    stopServer,
    testConfig,
    listTools,
    setToolEnabled,
    listResources,
    readResource,
    listPrompts,
    getPrompt,
    getServerLogs,
  };
});
