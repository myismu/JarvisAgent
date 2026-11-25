// @vitest-environment happy-dom
/**
 * RollbackConfirmModal.vue 组件测试
 *
 * 演示最基础的组件测试三段式结构（AAA 模式）：
 * 1. Arrange（准备）：通过 mount() 挂载组件并传入 props
 * 2. Act（操作）：通过 trigger('click') 模拟用户点击按钮或遮罩
 * 3. Assert（断言）：通过 expect() 检查 DOM 渲染内容与 emit 抛出的事件
 */
import { describe, expect, it } from "vitest";
import { mount } from "@vue/test-utils";
import RollbackConfirmModal from "../RollbackConfirmModal.vue";
import { createTestPlugins } from "../../../test-utils/componentTestHelper";

describe("RollbackConfirmModal.vue", () => {
  // 因为组件用了 <Teleport to="body">，测试中通过 stubs: { Teleport: true }
  // 把弹窗保留在当前测试容器内，便于直接用 wrapper.find() 查找 DOM 节点。
  const mountModal = (props: {
    visible: boolean;
    title?: string;
    message: string;
    details?: string[];
    loading?: boolean;
  }) => {
    const { plugins } = createTestPlugins();
    return mount(RollbackConfirmModal, {
      props,
      global: {
        plugins,
        stubs: {
          Teleport: true,
        },
      },
    });
  };

  it("visible=false 时不渲染弹窗内容", () => {
    const wrapper = mountModal({
      visible: false,
      message: "确认回滚当前文件吗？",
    });

    // 断言：遮罩层不存在，弹窗处于隐藏状态
    expect(wrapper.find(".modal-overlay").exists()).toBe(false);
  });

  it("visible=true 时正确渲染标题、正文与变更详情列表", () => {
    const wrapper = mountModal({
      visible: true,
      title: "回滚到检查点 #3",
      message: "将还原以下文件改动：",
      details: ["src/App.vue（修改）", "src/utils/foo.ts（删除）"],
    });

    expect(wrapper.find(".modal-title").text()).toBe("回滚到检查点 #3");
    expect(wrapper.find(".rollback-message").text()).toBe("将还原以下文件改动：");

    const items = wrapper.findAll(".rollback-details li");
    expect(items).toHaveLength(2);
    expect(items[0].text()).toBe("src/App.vue（修改）");
    expect(items[1].text()).toBe("src/utils/foo.ts（删除）");
  });

  it("点击确认、取消与遮罩层分别抛出 confirm / cancel 事件", async () => {
    const wrapper = mountModal({
      visible: true,
      message: "确认回滚？",
    });

    // 1. 点击确认按钮
    await wrapper.find("button.cmd-button.safe").trigger("click");
    expect(wrapper.emitted("confirm")).toHaveLength(1);

    // 2. 点击取消按钮
    const buttons = wrapper.findAll("button.cmd-button");
    await buttons[1].trigger("click");
    expect(wrapper.emitted("cancel")).toHaveLength(1);

    // 3. 点击遮罩层自身也触发取消
    await wrapper.find(".modal-overlay").trigger("click");
    expect(wrapper.emitted("cancel")).toHaveLength(2);
  });

  it("loading=true 时禁用所有操作按钮，防止用户重复提交回滚", () => {
    const wrapper = mountModal({
      visible: true,
      message: "正在回滚中...",
      loading: true,
    });

    const buttons = wrapper.findAll("button.cmd-button");
    expect(buttons).toHaveLength(2);
    expect(buttons[0].attributes("disabled")).toBeDefined();
    expect(buttons[1].attributes("disabled")).toBeDefined();
  });
});
