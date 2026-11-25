import { describe, it, expect, beforeEach, vi } from "vitest";
import { setActivePinia, createPinia } from "pinia";

const invokeMock = vi.fn();
const listenMock = vi.fn();

vi.mock("@tauri-apps/api/core", () => ({
  invoke: (...args: unknown[]) => invokeMock(...args),
}));

vi.mock("@tauri-apps/api/event", () => ({
  listen: (...args: unknown[]) => listenMock(...args),
}));

import { useAppViewStore } from "../appView";
import { useMcpStore } from "../mcp";

describe("appView store extensions view", () => {
  beforeEach(() => {
    setActivePinia(createPinia());
  });

  it("toggles between chat and extensions view", () => {
    const view = useAppViewStore();
    expect(view.isChatView).toBe(true);
    expect(view.isExtensionsView).toBe(false);

    view.toggleExtensions();
    expect(view.activeView).toBe("extensions");
    expect(view.isExtensionsView).toBe(true);

    view.toggleExtensions();
    expect(view.activeView).toBe("chat");
  });
});

describe("mcp store", () => {
  beforeEach(() => {
    setActivePinia(createPinia());
    invokeMock.mockReset();
    listenMock.mockReset();
  });

  it("loads servers and applies status event updates", async () => {
    let statusCallback: ((event: { payload: unknown }) => void) | null = null;
    listenMock.mockImplementation((_event: string, cb: (e: { payload: unknown }) => void) => {
      statusCallback = cb;
      return Promise.resolve(() => {});
    });

    invokeMock.mockResolvedValueOnce([
      {
        name: "fs",
        transport: "stdio",
        status: "idle",
        enabled: true,
        trusted: false,
        autoStart: false,
        toolCount: 0,
        resourceCount: 0,
        promptCount: 0,
        hasCachedCapabilities: false,
        config: {
          command: "npx",
          args: [],
          env: {},
          headers: {},
          enabled: true,
          trusted: false,
          autoStart: false,
          isolation: "global",
          startupTimeoutSec: 60,
          callTimeoutSec: 60,
          idleTimeoutMin: 30,
          disabledTools: [],
        },
      },
    ]);

    const store = useMcpStore();
    await store.loadServers();
    await store.initListener();

    expect(store.servers).toHaveLength(1);
    expect(store.servers[0].status).toBe("idle");

    statusCallback!({
      payload: {
        serverName: "fs",
        status: "ready",
        toolCount: 3,
        resourceCount: 1,
        promptCount: 2,
      },
    });

    expect(store.servers[0].status).toBe("ready");
    expect(store.servers[0].toolCount).toBe(3);
    expect(store.servers[0].hasCachedCapabilities).toBe(true);
  });
});
