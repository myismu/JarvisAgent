/**
 * api/ 强类型 IPC 与事件封装层单元测试
 *
 * 验证 invokeTyped / listenTyped 以及领域 API 封装的参数透传与返回值行为。
 */
import { describe, it, expect, beforeEach, vi } from "vitest";

const invokeMock = vi.fn();
const listenMock = vi.fn();

vi.mock("@tauri-apps/api/core", () => ({
  invoke: (...args: unknown[]) => invokeMock(...args),
}));

vi.mock("@tauri-apps/api/event", () => ({
  listen: (...args: unknown[]) => listenMock(...args),
}));

import {
  getConfig,
  getPermissionState,
  getSessionMessagesPaged,
  invokeTyped,
  listSessions,
  listenTyped,
} from "../index";

describe("src/api 强类型 IPC 与事件契约层", () => {
  beforeEach(() => {
    invokeMock.mockReset();
    listenMock.mockReset();
  });

  it("invokeTyped：无参数命令不传第二个参数，有参数命令精确透传参数", async () => {
    invokeMock.mockResolvedValueOnce([]);
    const sessions = await listSessions();
    expect(sessions).toEqual([]);
    expect(invokeMock).toHaveBeenCalledWith("list_sessions");

    invokeMock.mockResolvedValueOnce({ activeProfileId: "p1", globalProfileId: "p1", profiles: [] });
    const cfg = await getConfig();
    expect(cfg.activeProfileId).toBe("p1");
    expect(invokeMock).toHaveBeenCalledWith("get_config");

    invokeMock.mockResolvedValueOnce({
      messages: [],
      hasMore: false,
      oldestSeq: null,
      newestSeq: null,
      totalCount: 0,
    });
    await getSessionMessagesPaged("s1", 10, 5);
    expect(invokeMock).toHaveBeenCalledWith("get_session_messages_paged", {
      sessionId: "s1",
      beforeSeq: 10,
      limit: 5,
    });

    invokeMock.mockResolvedValueOnce({
      allowanceCount: 1,
      allowances: [{ kind: "tool", scope: "Bash", label: "Bash" }],
      pendingCount: 0,
      pending: [],
    });
    const perm = await getPermissionState("s1");
    expect(perm.allowanceCount).toBe(1);
    expect(invokeMock).toHaveBeenCalledWith("get_permission_state", { sessionId: "s1" });

    invokeMock.mockResolvedValueOnce("s1");
    const activeId = await invokeTyped("get_active_session_id");
    expect(activeId).toBe("s1");
    expect(invokeMock).toHaveBeenLastCalledWith("get_active_session_id");
  });

  it("listenTyped：透传事件名与回调并返回 unlisten 句柄", async () => {
    const unlisten = vi.fn();
    listenMock.mockResolvedValueOnce(unlisten);
    const handler = vi.fn();

    const stop = await listenTyped("chat-content", handler);
    expect(listenMock).toHaveBeenCalledWith("chat-content", handler);
    expect(stop).toBe(unlisten);
  });
});
