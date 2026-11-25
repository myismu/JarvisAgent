// @vitest-environment happy-dom
/**
 * UserQuestionCard.vue 组件测试（AskUser 运行期咨询卡片）
 *
 * 验证核心交互：
 * 1. 无挂起咨询时不渲染；入队后渲染标题、问题与选项
 * 2. 单选与“其他”自填互斥，提交后通过 resolve_user_question 回传结构化答案并关闭卡片
 * 3. 多题 + 多选支持切换题签并合并选中项与自定义补充文本
 * 4. 点击“跳过”回传 skipped；切换会话时按 sessionId 隔离显示
 */
import { afterEach, describe, expect, it, vi } from "vitest";
import { flushPromises, mount, type VueWrapper } from "@vue/test-utils";
import { nextTick } from "vue";
import UserQuestionCard from "../UserQuestionCard.vue";
import { usePermissionStore } from "../../../stores/permission";
import { useSessionStore } from "../../../stores/session";
import { createTestPlugins } from "../../../test-utils/componentTestHelper";

const invokeMock = vi.fn().mockResolvedValue({ resolved: true, status: "answered" });

vi.mock("@tauri-apps/api/core", () => ({
  invoke: (...args: unknown[]) => invokeMock(...args),
}));

describe("UserQuestionCard.vue", () => {
  const wrappers: VueWrapper[] = [];

  afterEach(() => {
    while (wrappers.length > 0) {
      wrappers.pop()?.unmount();
    }
    invokeMock.mockClear();
  });

  const setupCard = (sessionId = "s-uq") => {
    const { plugins } = createTestPlugins();
    const sessionStore = useSessionStore();
    const permStore = usePermissionStore();

    sessionStore.activeSessionId = sessionId;

    const wrapper = mount(UserQuestionCard, {
      global: { plugins },
    });
    wrappers.push(wrapper);

    return { wrapper, sessionStore, permStore };
  };

  it("单选模式：选择选项后提交触发 resolve_user_question 并移除队列条目", async () => {
    const { wrapper, permStore } = setupCard("s-1");
    expect(wrapper.find(".dock-card").exists()).toBe(false);

    permStore.enqueueUserQuestion({
      id: "uq_1",
      sessionId: "s-1",
      title: "确认存储方案",
      questions: [
        {
          id: "q1",
          question: "缓存希望放在哪里？",
          multiSelect: false,
          allowCustom: true,
          options: [
            { label: "SQLite", description: "复用现有本地库", recommended: true },
            { label: "内存 Map", description: "重启即清", recommended: false },
          ],
        },
      ],
    });
    await nextTick();

    expect(wrapper.find(".dock-card").exists()).toBe(true);
    expect(wrapper.find(".dock-title").text()).toBe("确认存储方案");
    expect(wrapper.find(".dock-counter").text()).toBe("0/1");
    expect(wrapper.find(".dock-btn-primary").attributes("disabled")).toBeDefined();

    // 点击折叠按钮收起内容，再点击展开
    await wrapper.find(".dock-collapse-btn").trigger("click");
    expect(wrapper.find(".dock-collapsible").exists()).toBe(false);
    await wrapper.find(".dock-collapse-btn").trigger("click");
    expect(wrapper.find(".dock-collapsible").exists()).toBe(true);

    // 选择第一项 SQLite
    await wrapper.findAll("button.uq-option")[0].trigger("click");
    expect(wrapper.find(".dock-counter").text()).toBe("1/1");
    expect(wrapper.find(".dock-btn-primary").attributes("disabled")).toBeUndefined();

    // 点击提交
    await wrapper.find(".dock-btn-primary").trigger("click");
    await flushPromises();

    expect(invokeMock).toHaveBeenCalledWith("resolve_user_question", {
      requestId: "uq_1",
      sessionId: "s-1",
      decision: "answered",
      answers: [{ questionId: "q1", selectedOptions: ["SQLite"] }],
    });
    expect(permStore.userQuestion).toBeNull();
  });

  it("多题 + 多选 + 其他自填：支持题签切换并正确回传多选与自定义补充", async () => {
    const { wrapper, permStore } = setupCard("s-2");

    permStore.enqueueUserQuestion({
      id: "uq_2",
      sessionId: "s-2",
      title: "方案细节确认",
      questions: [
        {
          id: "q_feat",
          header: "功能范围",
          question: "本期需要包含哪些能力？",
          multiSelect: true,
          allowCustom: true,
          options: [
            { label: "单选", recommended: true },
            { label: "多选", recommended: true },
          ],
        },
        {
          id: "q_pos",
          header: "展示位置",
          question: "卡片放哪里？",
          multiSelect: false,
          allowCustom: true,
          options: [
            { label: "输入区上方", recommended: true },
            { label: "居中弹窗", recommended: false },
          ],
        },
      ],
    });
    await nextTick();

    // 第一题尚未答完全部题目时，不显示「提交回复」，只引导「下一题」
    expect(wrapper.find(".dock-btn-primary").exists()).toBe(false);

    // 第一题多选两项 + 自填补充
    const firstOpts = wrapper.findAll("button.uq-option");
    await firstOpts[0].trigger("click");
    await firstOpts[1].trigger("click");
    await wrapper.find(".uq-custom-input").setValue("需要支持跳过按钮");

    // 切到第二题（最后一题）后才显示「提交回复」并直接填“其他”自填
    await wrapper.findAll(".uq-tab")[1].trigger("click");
    await nextTick();
    expect(wrapper.find(".dock-btn-primary").exists()).toBe(true);
    await wrapper.find(".uq-custom-input").setValue("右侧抽屉");

    await wrapper.find(".dock-btn-primary").trigger("click");
    await flushPromises();

    expect(invokeMock).toHaveBeenCalledWith("resolve_user_question", {
      requestId: "uq_2",
      sessionId: "s-2",
      decision: "answered",
      answers: [
        {
          questionId: "q_feat",
          selectedOptions: ["单选", "多选"],
          customText: "需要支持跳过按钮",
        },
        {
          questionId: "q_pos",
          selectedOptions: [],
          customText: "右侧抽屉",
        },
      ],
    });
  });

  it("点击跳过按钮回传 skipped；切换会话时按 sessionId 隔离展示与等待态", async () => {
    const { wrapper, sessionStore, permStore } = setupCard("s-a");

    permStore.enqueueUserQuestion({
      id: "uq_a",
      sessionId: "s-a",
      questions: [
        {
          id: "q1",
          question: "需要拆分组件吗？",
          multiSelect: false,
          allowCustom: true,
          options: [
            { label: "拆分", recommended: true },
            { label: "暂不拆分", recommended: false },
          ],
        },
      ],
    });
    await nextTick();

    expect(permStore.isSessionWaitingForUser("s-a")).toBe(true);
    expect(permStore.isSessionWaitingForUser("s-b")).toBe(false);

    // 切到会话 B，不应看到会话 A 的咨询卡片
    sessionStore.activeSessionId = "s-b";
    await nextTick();
    expect(wrapper.find(".dock-card").exists()).toBe(false);

    // 切回会话 A 并点击跳过
    sessionStore.activeSessionId = "s-a";
    await nextTick();
    await wrapper.find(".dock-btn-ghost").trigger("click");
    await flushPromises();

    expect(invokeMock).toHaveBeenCalledWith("resolve_user_question", {
      requestId: "uq_a",
      sessionId: "s-a",
      decision: "skipped",
      answers: null,
    });
    expect(permStore.isSessionWaitingForUser("s-a")).toBe(false);
  });
});
