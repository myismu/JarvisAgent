/**
 * stores/chat.ts 单元测试
 *
 * 覆盖会话视图装载状态机与懒加载分页防重入：
 * 1. ensureSessionViewLoaded：idle 发首屏 IPC、ready 零 IPC、并发调用复用同一在途 Promise
 * 2. markSessionViewStale 标脏后，再次 ensureSessionViewLoaded 重新拉取最新首屏
 * 3. loadSessionMessagesOlder：按 oldestSeq 向前翻页头部插入，hasMore=false 时零 IPC
 */
import { describe, it, expect, beforeEach, vi } from "vitest";
import { createPinia, setActivePinia } from "pinia";

const invokeMock = vi.fn();

vi.mock("@tauri-apps/api/core", () => ({
  invoke: (...args: unknown[]) => invokeMock(...args),
}));

import { useSessionStore } from "../session";
import { useChatStore } from "../chat";

describe("useChatStore hydration & paging", () => {
  beforeEach(() => {
    setActivePinia(createPinia());
    invokeMock.mockReset();
  });

  it("ensureSessionViewLoaded：idle 拉首屏、ready 零 IPC、并发复用在途 Promise", async () => {
    const session = useSessionStore();
    const chat = useChatStore();

    let resolvePaged!: (val: unknown) => void;
    invokeMock.mockImplementation((cmd: string) => {
      if (cmd === "get_session_messages_paged") {
        return new Promise((resolve) => {
          resolvePaged = resolve;
        });
      }
      return Promise.resolve(null);
    });

    // 同一会话并发触发两次加载 → 应合并为一次 IPC
    const p1 = chat.ensureSessionViewLoaded("s1", "mount");
    const p2 = chat.ensureSessionViewLoaded("s1", "switch");
    expect(session.getSessionView("s1").hydration).toBe("loading");

    resolvePaged({
      messages: [
        { id: "u1", role: "user", userContent: "第一问" },
        { id: "a1", role: "assistant", content: "第一答" },
      ],
      hasMore: true,
      oldestSeq: 12,
      newestSeq: 20,
      totalCount: 20,
    });
    await Promise.all([p1, p2]);

    const pagedCalls = invokeMock.mock.calls.filter(([cmd]) => cmd === "get_session_messages_paged");
    expect(pagedCalls).toHaveLength(1);

    const view = session.getSessionView("s1");
    expect(view.hydration).toBe("ready");
    expect(view.messages).toHaveLength(2);
    expect(view.pagedOldestSeq).toBe(12);
    expect(view.pagedHasMore).toBe(true);
    expect(view.loadedNewestSeq).toBe(20);

    // 已是 ready：再次调用应零 IPC 直接返回
    await chat.ensureSessionViewLoaded("s1", "revisit");
    expect(
      invokeMock.mock.calls.filter(([cmd]) => cmd === "get_session_messages_paged"),
    ).toHaveLength(1);
  });

  it("markSessionViewStale 标脏后，再次 ensureSessionViewLoaded 重新拉取最新首屏", async () => {
    const session = useSessionStore();
    const chat = useChatStore();

    let pagedCallCount = 0;
    invokeMock.mockImplementation((cmd: string) => {
      if (cmd === "get_session_messages_paged") {
        pagedCallCount += 1;
        if (pagedCallCount === 1) {
          return Promise.resolve({
            messages: [{ id: "m1", role: "user", userContent: "旧消息" }],
            hasMore: false,
            oldestSeq: 1,
            newestSeq: 1,
            totalCount: 1,
          });
        }
        return Promise.resolve({
          messages: [
            { id: "m1", role: "user", userContent: "旧消息" },
            { id: "m2", role: "assistant", content: "新回复" },
          ],
          hasMore: false,
          oldestSeq: 1,
          newestSeq: 2,
          totalCount: 2,
        });
      }
      return Promise.resolve(null);
    });

    await chat.ensureSessionViewLoaded("s1", "init");
    expect(session.getSessionView("s1").messages).toHaveLength(1);

    chat.markSessionViewStale("s1", "messages-changed:turn-end");
    expect(session.getSessionView("s1").hydration).toBe("stale");

    await chat.ensureSessionViewLoaded("s1", "re-enter");
    expect(session.getSessionView("s1").hydration).toBe("ready");
    expect(session.getSessionView("s1").messages).toHaveLength(2);
  });

  it("loadSessionMessagesOlder：按 oldestSeq 向前翻页头部插入，无更多历史时零 IPC", async () => {
    const session = useSessionStore();
    const chat = useChatStore();

    session.replaceSessionMessages("s1", [{ id: "m3", role: "user", userContent: "最新" }]);
    session.setSessionPaging("s1", 10, true, 15);

    invokeMock.mockResolvedValueOnce({
      messages: [
        { id: "m1", role: "user", userContent: "更早1" },
        { id: "m2", role: "assistant", content: "更早2" },
      ],
      hasMore: false,
      oldestSeq: 1,
      newestSeq: 9,
      totalCount: 15,
    });

    const loaded = await chat.loadSessionMessagesOlder("s1");
    expect(loaded).toBe(true);
    expect(invokeMock).toHaveBeenCalledWith("get_session_messages_paged", {
      sessionId: "s1",
      beforeSeq: 10,
      limit: 5,
    });

    const view = session.getSessionView("s1");
    expect(view.messages.map((m) => m.id)).toEqual(["m1", "m2", "m3"]);
    expect(view.pagedOldestSeq).toBe(1);
    expect(view.pagedHasMore).toBe(false);
    // 向上翻页不得覆盖 loadedNewestSeq（窗口最新端仍为 15）
    expect(view.loadedNewestSeq).toBe(15);

    // hasMore=false 后再次翻页应直接返回 false，不再发 IPC
    const loadedAgain = await chat.loadSessionMessagesOlder("s1");
    expect(loadedAgain).toBe(false);
    expect(invokeMock).toHaveBeenCalledTimes(1);
  });
});
