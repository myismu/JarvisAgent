// @vitest-environment happy-dom
/**
 * PlanPreviewPanel.vue 组件测试
 *
 * 重点验证方案审批面板的防回归核心约束：
 * 1. 收到新方案提案时自动展开并展示标题/统计
 * 2. 会话处于 RUNNING（本轮尚未收尾）时隐藏审批按钮，防止用户提前点批准导致消息顺序错乱
 * 3. 点击批准按钮调用 chatStore.resolvePlan('allow', ...) 并触发 continueFromApprovedPlan
 * 4. 点击“拒绝/要求修改”先展开反馈输入框，填完意见后再次提交触发 resolvePlan('reject', feedback)
 */
import { afterEach, describe, expect, it, vi } from "vitest";
import { flushPromises, mount, type VueWrapper } from "@vue/test-utils";
import { nextTick } from "vue";
import PlanPreviewPanel from "../PlanPreviewPanel.vue";
import { usePermissionStore } from "../../../stores/permission";
import { useSessionStore } from "../../../stores/session";
import { useChatStore } from "../../../stores/chat";
import { createTestPlugins } from "../../../test-utils/componentTestHelper";

vi.mock("@tauri-apps/api/core", () => ({
  invoke: vi.fn().mockResolvedValue(undefined),
}));

describe("PlanPreviewPanel.vue", () => {
  const wrappers: VueWrapper[] = [];

  afterEach(() => {
    while (wrappers.length > 0) {
      wrappers.pop()?.unmount();
    }
    vi.clearAllMocks();
  });

  const setupPanel = (sessionId = "s-plan") => {
    const { plugins } = createTestPlugins();
    const sessionStore = useSessionStore();
    const permStore = usePermissionStore();
    const chatStore = useChatStore();

    sessionStore.activeSessionId = sessionId;
    sessionStore.getSessionView(sessionId).status = "IDLE";

    const resolvePlanSpy = vi
      .spyOn(chatStore, "resolvePlan")
      .mockResolvedValue({ planTasksCreated: false } as never);
    const continueSpy = vi
      .spyOn(chatStore, "continueFromApprovedPlan")
      .mockResolvedValue(undefined as never);
    const revisionSpy = vi
      .spyOn(chatStore, "requestPlanRevision")
      .mockResolvedValue(undefined as never);

    const wrapper = mount(PlanPreviewPanel, {
      global: {
        plugins,
        stubs: {
          StreamingMarkdown: {
            props: ["content"],
            template: "<div class='stub-md'>{{ content }}</div>",
          },
        },
      },
    });
    wrappers.push(wrapper);

    return { wrapper, sessionStore, permStore, resolvePlanSpy, continueSpy, revisionSpy };
  };

  it("收到新方案时自动展开；会话 RUNNING 时隐藏审批按钮避免提前点击", async () => {
    const { wrapper, sessionStore, permStore } = setupPanel("s-plan-1");

    // 初始无提案也不显示面板
    expect(wrapper.find(".plan-panel").exists()).toBe(false);

    // 模拟后端推送方案提案（但此时本轮 Agent 仍在 RUNNING 收尾中）
    sessionStore.getSessionView("s-plan-1").status = "RUNNING";
    permStore.planProposals = {
      "s-plan-1": {
        id: "plan-1",
        title: "重构数据库迁移模块",
        content: "# 步骤\n1. 拆目录\n2. 跑测试",
        sessionId: "s-plan-1",
      },
    };
    await nextTick();

    // 面板已展开，但因为本轮未结束，审批按钮必须隐藏，改为显示等待提示
    expect(wrapper.find(".plan-panel").exists()).toBe(true);
    expect(wrapper.find(".plan-title").text()).toBe("重构数据库迁移模块");
    expect(wrapper.find(".plan-btn-approve").exists()).toBe(false);
    expect(wrapper.find(".plan-actions-streaming").exists()).toBe(true);

    // 本轮结束置为 IDLE 后，审批按钮出现
    sessionStore.getSessionView("s-plan-1").status = "IDLE";
    await nextTick();
    expect(wrapper.find(".plan-btn-approve").exists()).toBe(true);
    expect(wrapper.find(".plan-btn-reject").exists()).toBe(true);
  });

  it("点击批准按钮触发 resolvePlan('allow') 与 continueFromApprovedPlan", async () => {
    const { wrapper, permStore, resolvePlanSpy, continueSpy } = setupPanel("s-plan-2");

    permStore.planProposals = {
      "s-plan-2": {
        id: "plan-2",
        title: "接入组件测试",
        content: "## 目标\n补齐三个组件测试",
        sessionId: "s-plan-2",
      },
    };
    await nextTick();

    await wrapper.find(".plan-btn-approve").trigger("click");
    await flushPromises();

    expect(resolvePlanSpy).toHaveBeenCalledTimes(1);
    expect(resolvePlanSpy).toHaveBeenCalledWith("allow", "## 目标\n补齐三个组件测试", undefined);
    expect(continueSpy).toHaveBeenCalledTimes(1);
    expect(continueSpy).toHaveBeenCalledWith("接入组件测试", "## 目标\n补齐三个组件测试", false);
  });

  it("点击拒绝先弹出修改意见输入框，填写意见后提交触发 resolvePlan('reject') 与 requestPlanRevision", async () => {
    const { wrapper, permStore, resolvePlanSpy, revisionSpy } = setupPanel("s-plan-3");

    permStore.planProposals = {
      "s-plan-3": {
        id: "plan-3",
        title: "新增缓存层",
        content: "## 方案\n引入 Redis",
        sessionId: "s-plan-3",
      },
    };
    await nextTick();

    // 第一次点击拒绝：只展开反馈框，不直接发请求
    await wrapper.find(".plan-btn-reject").trigger("click");
    await nextTick();
    expect(wrapper.find(".plan-revision-input").exists()).toBe(true);
    expect(resolvePlanSpy).not.toHaveBeenCalled();

    // 未填意见时提交按钮处于禁用态
    expect(wrapper.find(".plan-btn-reject").attributes("disabled")).toBeDefined();

    // 填写意见后再次点击提交
    await wrapper.find(".plan-revision-input").setValue("桌面端不需要 Redis，请改用本地 SQLite");
    await wrapper.find(".plan-btn-reject").trigger("click");
    await flushPromises();

    expect(resolvePlanSpy).toHaveBeenCalledTimes(1);
    expect(resolvePlanSpy).toHaveBeenCalledWith(
      "reject",
      "桌面端不需要 Redis，请改用本地 SQLite",
      undefined,
    );
    expect(revisionSpy).toHaveBeenCalledTimes(1);
    expect(revisionSpy).toHaveBeenCalledWith("新增缓存层", "桌面端不需要 Redis，请改用本地 SQLite");
  });
});
