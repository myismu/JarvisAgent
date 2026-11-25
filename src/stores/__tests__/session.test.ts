/**
 * stores/session.ts 单元测试
 *
 * 覆盖核心不变量：
 * 1. hydration 状态机合法转移与非法转移防护（ready 不可退回 loading，idle/loading 不可置 stale）
 * 2. 多会话视图严格隔离、整批替换复位分页游标、头部插入不触碰尾部
 * 3. 单次请求吞吐量采样（首次 delta 锁起点、增量结算与基线推进、零增量保留旧值）
 */
import { describe, it, expect, beforeEach } from "vitest";
import { createPinia, setActivePinia } from "pinia";
import { useSessionStore } from "../session";

describe("useSessionStore", () => {
  beforeEach(() => {
    setActivePinia(createPinia());
  });

  it("hydration 状态机严格遵守转移约束：ready 不可回退 loading，idle/loading 不可置 stale", () => {
    const session = useSessionStore();
    const view = session.getSessionView("s1");

    // 初始态为 idle，未加载过
    expect(view.hydration).toBe("idle");
    expect(session.hasLoadedSessionView("s1")).toBe(false);

    // idle 态不允许被标脏
    session.markSessionStale("s1", "premature");
    expect(view.hydration).toBe("idle");
    expect(view.staleReason).toBeNull();

    // idle -> loading
    session.markSessionLoading("s1");
    expect(view.hydration).toBe("loading");
    // loading 态同样不允许被标脏
    session.markSessionStale("s1", "while-loading");
    expect(view.hydration).toBe("loading");

    // loading -> ready
    session.markSessionReady("s1");
    expect(view.hydration).toBe("ready");
    expect(session.hasLoadedSessionView("s1")).toBe(true);

    // ready 态调用 markSessionLoading 不回退（后台重拉不闪空白）
    session.markSessionLoading("s1");
    expect(view.hydration).toBe("ready");

    // ready -> stale -> loading -> ready
    session.markSessionStale("s1", "messages-changed:turn-end");
    expect(view.hydration).toBe("stale");
    expect(view.staleReason).toBe("messages-changed:turn-end");
    expect(session.hasLoadedSessionView("s1")).toBe(true);

    session.markSessionLoading("s1");
    expect(view.hydration).toBe("loading");
    session.markSessionReady("s1");
    expect(view.hydration).toBe("ready");
    expect(view.staleReason).toBeNull();
  });

  it("多会话消息与分页游标严格隔离，整批替换复位游标，头部插入保留尾部顺序", () => {
    const session = useSessionStore();

    session.replaceSessionMessages("s1", [
      { id: "m2", role: "user", userContent: "<p>你好</p>" },
      { id: "m3", role: "assistant", content: "世界" },
    ]);
    session.setSessionPaging("s1", 10, true, 25);

    const v1 = session.getSessionView("s1");
    expect(v1.hydration).toBe("ready");
    expect(v1.messages[0].text).toBe("你好");
    expect(v1.pagedOldestSeq).toBe(10);
    expect(v1.pagedHasMore).toBe(true);
    expect(v1.loadedNewestSeq).toBe(25);

    // 另一会话不受影响
    expect(session.getSessionView("s2").messages).toEqual([]);

    // 头部插入更早历史
    session.prependSessionMessages("s1", [{ id: "m1", role: "user", userContent: "早安" }]);
    expect(v1.messages.map((m) => m.id)).toEqual(["m1", "m2", "m3"]);

    // 再次整批替换应复位分页游标
    session.replaceSessionMessages("s1", [{ id: "m9", role: "assistant", content: "最新" }]);
    expect(v1.pagedOldestSeq).toBeNull();
    expect(v1.pagedHasMore).toBe(false);
    expect(v1.loadedNewestSeq).toBeNull();
  });

  it("单次请求吞吐采样：仅首个 delta 记录起点，结算推进基线，零增量保留旧读数", () => {
    const session = useSessionStore();

    // 连续两个 delta 到达，只认第一个起点 1000ms
    session.markRequestFirstDelta("s1", 1000);
    session.markRequestFirstDelta("s1", 1500);
    expect(session.getSessionView("s1").requestFirstDeltaAt).toBe(1000);

    // 2 秒后输出累计到 100 token → 50 tok/s
    session.settleRequestThroughput("s1", 100, 3000);
    const v1 = session.getSessionView("s1");
    expect(v1.lastRequestTokPerSec).toBe(50);
    expect(v1.requestOutputTokensBase).toBe(100);
    expect(v1.requestFirstDeltaAt).toBeNull();

    // 下一次推送输出未增加（delta = 0）→ 保留上次 50 tok/s，不清成 null
    session.markRequestFirstDelta("s1", 4000);
    session.settleRequestThroughput("s1", 100, 5000);
    expect(v1.lastRequestTokPerSec).toBe(50);

    // 第三次推送增量 80 token、耗时 4 秒 → 20 tok/s
    session.markRequestFirstDelta("s1", 6000);
    session.settleRequestThroughput("s1", 180, 10000);
    expect(v1.lastRequestTokPerSec).toBe(20);
  });
});
