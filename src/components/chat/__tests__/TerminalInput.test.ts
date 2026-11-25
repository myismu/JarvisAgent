// @vitest-environment happy-dom
/**
 * TerminalInput.vue 组件测试
 *
 * 重点演示复杂交互组件的测试方法：
 * 1. Mock Tauri 原生模块（@tauri-apps/api/core、event、window），让桌面组件可以在无原生壳子的测试环境里挂载
 * 2. 用 vi.spyOn 拦截 chatStore.sendToJarvis / cancelJarvis，聚焦验证“用户输入 → 发送/停止/草稿切换”行为
 * 3. 使用 flushPromises + nextTick 等待 Vue 响应式更新与异步挂载完成，再做断言
 */
import { afterEach, describe, expect, it, vi } from "vitest";
import { flushPromises, mount, type VueWrapper } from "@vue/test-utils";
import { nextTick } from "vue";
import TerminalInput from "../TerminalInput.vue";
import { useSessionStore } from "../../../stores/session";
import { useChatStore } from "../../../stores/chat";
import { createTestPlugins } from "../../../test-utils/componentTestHelper";

// ── 1. Mock 打桩：屏蔽 Tauri 桌面原生依赖 ──
vi.mock("@tauri-apps/api/core", () => ({
  invoke: vi.fn(async (cmd: string) => {
    if (cmd === "get_config") {
      return {
        activeProfileId: "p1",
        globalProfileId: "p1",
        profiles: [
          {
            id: "p1",
            name: "默认预设",
            config: {
              apiKey: "test-key",
              baseUrl: "https://example.com",
              mainModel: "gpt-4o",
              utilityModel: "gpt-4o-mini",
              apiFormat: "openai",
            },
          },
        ],
      };
    }
    if (cmd === "get_session_work_mode") {
      return "edit";
    }
    if (cmd === "get_session_permission_settings") {
      return { approvalMode: "ask", readOnly: false };
    }
    if (cmd === "get_compacting_status") return false;
    if (cmd === "get_model_capabilities") {
      return { supportsThinking: true, supportsVision: true, thinkingForced: false };
    }
    return null;
  }),
}));

vi.mock("@tauri-apps/api/event", () => ({
  listen: vi.fn().mockResolvedValue(() => {}),
}));

vi.mock("@tauri-apps/api/window", () => ({
  getCurrentWindow: () => ({
    onDragDropEvent: vi.fn().mockResolvedValue(() => {}),
  }),
}));

describe("TerminalInput.vue", () => {
  const wrappers: VueWrapper[] = [];

  afterEach(() => {
    while (wrappers.length > 0) {
      wrappers.pop()?.unmount();
    }
    vi.clearAllMocks();
  });

  const setupInput = async (initialSessionId = "s-1") => {
    const { plugins } = createTestPlugins();
    const sessionStore = useSessionStore();
    const chatStore = useChatStore();

    sessionStore.activeSessionId = initialSessionId;
    sessionStore.getSessionView(initialSessionId).status = "IDLE";

    const sendSpy = vi
      .spyOn(chatStore, "sendToJarvis")
      .mockResolvedValue({ sent: true } as never);
    const cancelSpy = vi
      .spyOn(chatStore, "cancelJarvis")
      .mockResolvedValue(true);

    const wrapper = mount(TerminalInput, {
      global: {
        plugins,
        stubs: {
          ConfirmModal: true,
          CacheHitTooltip: true,
        },
      },
    });
    wrappers.push(wrapper);

    // 等待 onMounted 里的异步配置加载完成
    await flushPromises();
    await nextTick();

    return { wrapper, sessionStore, chatStore, sendSpy, cancelSpy };
  };

  it("输入有效文本后点击发送触发 sendToJarvis 并清空输入框；纯空白不发送", async () => {
    const { wrapper, sendSpy } = await setupInput();
    const textarea = wrapper.find("textarea.editor-input");

    // 1. 纯空白不应触发发送
    await textarea.setValue("   \n  ");
    await wrapper.find("button.send-btn").trigger("click");
    expect(sendSpy).not.toHaveBeenCalled();

    // 2. 有效文本点击发送后触发调用并清空输入框
    await textarea.setValue("请帮我检查这段 Rust 代码");
    await wrapper.find("button.send-btn").trigger("click");

    expect(sendSpy).toHaveBeenCalledTimes(1);
    expect(sendSpy).toHaveBeenCalledWith("请帮我检查这段 Rust 代码", null, []);
    expect((textarea.element as HTMLTextAreaElement).value).toBe("");
  });

  it("按 Enter 直接发送，按 Shift+Enter 换行不发送", async () => {
    const { wrapper, sendSpy } = await setupInput();
    const textarea = wrapper.find("textarea.editor-input");

    await textarea.setValue("第一行");
    await textarea.trigger("keydown", { key: "Enter", shiftKey: true });
    expect(sendSpy).not.toHaveBeenCalled();

    await textarea.trigger("keydown", { key: "Enter", shiftKey: false });
    expect(sendSpy).toHaveBeenCalledTimes(1);
    expect(sendSpy).toHaveBeenCalledWith("第一行", null, []);
  });

  it("会话处于 RUNNING 状态时切换为停止按钮，点击后调用 cancelJarvis 且回车不重复发送", async () => {
    const { wrapper, sessionStore, sendSpy, cancelSpy } = await setupInput("s-run");
    sessionStore.getSessionView("s-run").status = "RUNNING";
    await nextTick();

    // 运行态下按钮应呈现 stop-state
    const stopBtn = wrapper.find("button.send-btn.stop-state");
    expect(stopBtn.exists()).toBe(true);

    // 运行态按 Enter 不应发送新消息
    const textarea = wrapper.find("textarea.editor-input");
    await textarea.setValue("运行中误按回车");
    await textarea.trigger("keydown", { key: "Enter", shiftKey: false });
    expect(sendSpy).not.toHaveBeenCalled();

    // 点击停止按钮触发 cancelJarvis
    await stopBtn.trigger("click");
    expect(cancelSpy).toHaveBeenCalledTimes(1);
    expect(cancelSpy).toHaveBeenCalledWith("s-run");
  });

  it("切换会话时按会话 ID 独立保存并恢复未发送草稿", async () => {
    const { wrapper, sessionStore } = await setupInput("s-a");
    const textarea = wrapper.find("textarea.editor-input");

    // 在会话 A 输入草稿
    await textarea.setValue("会话 A 的未发送草稿");
    expect((textarea.element as HTMLTextAreaElement).value).toBe("会话 A 的未发送草稿");

    // 切换到会话 B，输入框应清空
    sessionStore.activeSessionId = "s-b";
    await nextTick();
    expect((textarea.element as HTMLTextAreaElement).value).toBe("");

    // 在会话 B 输入草稿
    await textarea.setValue("会话 B 的草稿");

    // 切回会话 A，应恢复会话 A 的草稿
    sessionStore.activeSessionId = "s-a";
    await nextTick();
    expect((textarea.element as HTMLTextAreaElement).value).toBe("会话 A 的未发送草稿");
  });
});
