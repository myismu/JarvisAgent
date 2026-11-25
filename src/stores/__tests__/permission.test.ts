/**
 * stores/permission.ts 单元测试
 *
 * 验证多会话并发权限审批与方案审批的状态隔离：
 * 1. 同会话多个权限请求按 FIFO 排队、去重，不互相覆盖
 * 2. 切换会话时仅展示当前会话的队头请求，跨会话不串台
 * 3. 流式方案预览（plan_stream_*）可被清理，而正式方案（plan_*）不受影响
 */
import { describe, it, expect, beforeEach } from "vitest";
import { createPinia, setActivePinia } from "pinia";
import { usePermissionStore } from "../permission";
import { useSessionStore } from "../session";
import type { PermissionRequest, PlanDocument } from "../../types";

function makeRequest(id: string, sessionId: string): PermissionRequest {
  return {
    id,
    message: `执行命令 ${id}`,
    sessionId,
    kind: "tool",
  };
}

describe("usePermissionStore", () => {
  beforeEach(() => {
    // 每个用例初始化全新 Pinia 实例，保证测试间零状态残留
    setActivePinia(createPinia());
  });

  it("同会话多条权限请求按到达顺序排队且按 id 去重，跨会话严格隔离", () => {
    const session = useSessionStore();
    const perm = usePermissionStore();

    session.activeSessionId = "s1";
    perm.enqueuePermission(makeRequest("req_1", "s1"));
    perm.enqueuePermission(makeRequest("req_2", "s1"));
    // 重复 id 不应重复入队
    perm.enqueuePermission(makeRequest("req_1", "s1"));
    // 另一会话的请求入队
    perm.enqueuePermission(makeRequest("req_b1", "s2"));

    expect(perm.permissionQueue.map((r) => r.id)).toEqual(["req_1", "req_2"]);
    expect(perm.permissionRequest?.id).toBe("req_1");

    // 切到 s2，只看到 s2 的请求
    session.activeSessionId = "s2";
    expect(perm.permissionQueue.map((r) => r.id)).toEqual(["req_b1"]);
    expect(perm.permissionRequest?.id).toBe("req_b1");

    // 移除 s1 队头，s1 的队头顺延为 req_2，不影响 s2
    perm.removePermission("s1", "req_1");
    expect(perm.permissionRequest?.id).toBe("req_b1");

    session.activeSessionId = "s1";
    expect(perm.permissionRequest?.id).toBe("req_2");

    // 清空 s1
    perm.clearPermissions("s1");
    expect(perm.permissionQueue).toEqual([]);
    expect(perm.permissionRequest).toBeNull();
  });

  it("clearStreamingPlanProposal 只清流式占位，保留正式提交的方案", () => {
    const session = useSessionStore();
    const perm = usePermissionStore();
    session.activeSessionId = "s1";

    // 流式生成中
    perm.updatePlanProposalStreamingContent("s1", "步骤1");
    perm.updatePlanProposalStreamingContent("s1", "\n步骤2");
    expect(perm.planProposal?.content).toBe("步骤1\n步骤2");
    expect(perm.planProposal?.id?.startsWith("plan_stream_")).toBe(true);

    // 清理流式占位
    perm.clearStreamingPlanProposal("s1");
    expect(perm.planProposal).toBeNull();

    // 正式方案不应被 clearStreamingPlanProposal 误清
    perm.finalizePlanProposal("s1", {
      id: "plan_official_1",
      title: "正式方案",
      content: "详细计划",
      sessionId: "s1",
    });
    perm.clearStreamingPlanProposal("s1");
    expect(perm.planProposal?.id).toBe("plan_official_1");
  });

  it("upsertPlanDocument 按 updatedAt 倒序排列并覆盖同 id 文档", () => {
    const session = useSessionStore();
    const perm = usePermissionStore();
    session.activeSessionId = "s1";

    const doc1: PlanDocument = {
      id: "d1",
      sessionId: "s1",
      title: "旧版",
      content: "v1",
      status: "draft",
      createdAt: 100,
      updatedAt: 100,
    };
    const doc2: PlanDocument = {
      id: "d2",
      sessionId: "s1",
      title: "较新",
      content: "v1",
      status: "draft",
      createdAt: 200,
      updatedAt: 200,
    };
    perm.upsertPlanDocument(doc1);
    perm.upsertPlanDocument(doc2);
    expect(perm.currentPlanDocuments.map((d) => d.id)).toEqual(["d2", "d1"]);

    // 更新 d1 时间戳后应排到最前
    perm.upsertPlanDocument({ ...doc1, title: "新版", updatedAt: 300 });
    expect(perm.currentPlanDocuments.map((d) => d.id)).toEqual(["d1", "d2"]);
    expect(perm.currentPlanDocuments[0].title).toBe("新版");
  });
});
