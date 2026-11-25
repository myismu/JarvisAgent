// @vitest-environment happy-dom
/**
 * SettingsPanel.vue 组件测试
 *
 * 重点验证设置面板的高频交互与配置保存链路：
 * 1. modelValue=false 时不渲染弹窗；modelValue=true 时渲染五页签导航，点击关闭触发 update:modelValue(false)
 * 2. 切到“预设”页签加载预设列表，点击“+”新增预设并触发保存调用 save_config_cmd
 */
import { afterEach, describe, expect, it, vi } from "vitest";
import { flushPromises, mount, type VueWrapper } from "@vue/test-utils";
import { nextTick } from "vue";
import SettingsPanel from "../SettingsPanel.vue";
import { createTestPlugins } from "../../../test-utils/componentTestHelper";

const invokeMock = vi.fn(async (cmd: string) => {
  if (cmd === "get_config") {
    return {
      activeProfileId: "p1",
      globalProfileId: "p1",
      profiles: [
        {
          id: "p1",
          name: "主力预设",
          config: {
            apiKey: "sk-test",
            baseUrl: "https://api.example.com/v1",
            mainModel: "gpt-4o",
            utilityModel: "gpt-4o-mini",
            apiFormat: "openai",
            reflectionMode: "smart",
          },
        },
      ],
    };
  }
  if (cmd === "list_models") return [];
  if (cmd === "get_model_capabilities") {
    return { supportsThinking: true, supportsVision: true, thinkingForced: false };
  }
  if (cmd === "save_config_cmd") return null;
  return null;
});

vi.mock("@tauri-apps/api/core", () => ({
  invoke: (...args: unknown[]) => invokeMock(...(args as [string])),
}));

vi.mock("@tauri-apps/api/event", () => ({
  listen: vi.fn().mockResolvedValue(() => {}),
}));

describe("SettingsPanel.vue", () => {
  const wrappers: VueWrapper[] = [];

  afterEach(() => {
    while (wrappers.length > 0) {
      wrappers.pop()?.unmount();
    }
    invokeMock.mockClear();
  });

  const mountSettings = async (modelValue: boolean) => {
    const { plugins } = createTestPlugins();
    const wrapper = mount(SettingsPanel, {
      props: { modelValue },
      global: {
        plugins,
        stubs: {
          GeneralTab: true,
          PromptsTab: true,
          ToolsPanel: true,
          AboutTab: true,
          ConfirmModal: true,
        },
      },
    });
    wrappers.push(wrapper);
    await flushPromises();
    await nextTick();
    return wrapper;
  };

  it("modelValue=false 时不渲染；打开后展示五页签，点击关闭抛出 update:modelValue(false)", async () => {
    const hidden = await mountSettings(false);
    expect(hidden.find(".settings-overlay").exists()).toBe(false);

    const wrapper = await mountSettings(true);
    expect(wrapper.find(".settings-overlay").exists()).toBe(true);
    // 五页签：常规 / 预设 / 提示词 / 工具 / 关于
    expect(wrapper.findAll(".sidebar-nav .nav-item")).toHaveLength(5);

    await wrapper.find(".close-btn").trigger("click");
    expect(wrapper.emitted("update:modelValue")).toEqual([[false]]);
  });

  it("切换到预设页签后可新增预设并调用 save_config_cmd 持久化", async () => {
    // 模拟真实 App.vue 生命周期：先挂载（关闭态）再切换为打开态，触发配置加载
    const wrapper = await mountSettings(false);
    await wrapper.setProps({ modelValue: true });
    await flushPromises();
    await nextTick();

    // 点击第二个导航项（预设）
    const navItems = wrapper.findAll(".sidebar-nav .nav-item");
    await navItems[1].trigger("click");
    await flushPromises();
    await nextTick();

    // 初始加载 1 个预设
    expect(wrapper.findAll(".profile-list .profile-item")).toHaveLength(1);

    // 点击“复制预设”按钮，列表应变为 2 个预设，且自动调用 save_config_cmd 持久化
    await wrapper.find(".copy-btn").trigger("click");
    await flushPromises();
    await nextTick();
    expect(wrapper.findAll(".profile-list .profile-item")).toHaveLength(2);

    // 也可继续点击保存按钮持久化当前编辑内容
    await wrapper.find(".save-btn").trigger("click");
    await flushPromises();

    const saveCalls = invokeMock.mock.calls.filter(([cmd]) => cmd === "save_config_cmd");
    expect(saveCalls.length).toBeGreaterThanOrEqual(1);
  });
});
