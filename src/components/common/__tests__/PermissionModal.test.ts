// @vitest-environment happy-dom
/**
 * PermissionModal.vue 组件测试
 *
 * 重点演示：
 * 1. Pinia Store 驱动渲染：修改 permissionStore.permissionRequest 后组件自动出现
 * 2. Mock（打桩）：用 vi.spyOn 拦截 chatStore.resolvePermission，不真的调用后端 Tauri 命令
 * 3. 断言（Assertion）：校验点击按钮与全局快捷键（A/S/R/Escape）是否传入了正确决策参数
 */
import { afterEach, describe, expect, it, vi } from "vitest";
import { mount, type VueWrapper } from "@vue/test-utils";
import { nextTick } from "vue";
import PermissionModal from "../PermissionModal.vue";
import { usePermissionStore } from "../../../stores/permission";
import { useSessionStore } from "../../../stores/session";
import { useChatStore } from "../../../stores/chat";
import { createTestPlugins } from "../../../test-utils/componentTestHelper";

// 打桩 Tauri 核心 API，避免测试环境缺少桌面原生层而报错
vi.mock("@tauri-apps/api/core", () => ({
  invoke: vi.fn().mockResolvedValue(undefined),
}));

describe("PermissionModal.vue", () => {
  const wrappers: VueWrapper[] = [];

  afterEach(() => {
    // 卸载每个测试挂载的组件，确保 window 上的 keydown 监听器被正确清理
    while (wrappers.length > 0) {
      wrappers.pop()?.unmount();
    }
    vi.clearAllMocks();
  });

  const setupModal = () => {
    const { plugins } = createTestPlugins();
    const sessionStore = useSessionStore();
    const permStore = usePermissionStore();
    const chatStore = useChatStore();

    // 权限队列按 activeSessionId 索引，因此先设定当前活跃会话
    sessionStore.activeSessionId = "s-perm";

    // Mock 打桩：拦截 resolvePermission，记录调用参数但不触发真实后端通信
    const resolveSpy = vi
      .spyOn(chatStore, "resolvePermission")
      .mockResolvedValue(undefined);

    const wrapper = mount(PermissionModal, {
      global: { plugins },
    });
    wrappers.push(wrapper);

    return { wrapper, permStore, resolveSpy };
  };

  it("解析待审批消息中的反引号命令，并按 allowSession 控制按钮显隐", async () => {
    const { wrapper, permStore } = setupModal();

    // 初始无权限请求时不渲染卡片
    expect(wrapper.find(".perm-card").exists()).toBe(false);

    // 写入一条权限请求（含反引号包裹的命令，且禁止“本会话允许”）
    permStore.enqueuePermission({
      id: "req-1",
      sessionId: "s-perm",
      message: "需要执行命令 `rm -rf build/` 以清理产物",
      allowSession: false,
    });
    await nextTick();

    expect(wrapper.find(".perm-card").exists()).toBe(true);
    expect(wrapper.find(".perm-command-block code").text()).toBe("rm -rf build/");
    // allowSession=false 时不应出现“本会话允许”按钮
    expect(wrapper.find(".perm-allow-session").exists()).toBe(false);
    expect(wrapper.find(".perm-allow").exists()).toBe(true);
    expect(wrapper.find(".perm-reject").exists()).toBe(true);
  });

  it("点击允许一次 / 本会话允许 / 拒绝按钮分别调用对应决策", async () => {
    const { wrapper, permStore, resolveSpy } = setupModal();

    permStore.enqueuePermission({
      id: "req-2",
      sessionId: "s-perm",
      message: "执行构建命令: `pnpm build`",
      allowSession: true,
    });
    await nextTick();

    await wrapper.find(".perm-allow").trigger("click");
    expect(resolveSpy).toHaveBeenLastCalledWith("allow");

    await wrapper.find(".perm-allow-session").trigger("click");
    expect(resolveSpy).toHaveBeenLastCalledWith("allow_session");

    await wrapper.find(".perm-reject").trigger("click");
    expect(resolveSpy).toHaveBeenLastCalledWith("reject");
    expect(resolveSpy).toHaveBeenCalledTimes(3);
  });

  it("全局快捷键 A/S/R/Escape 触发对应审批，但在输入框内打字时不会误触发", async () => {
    const { permStore, resolveSpy } = setupModal();

    permStore.enqueuePermission({
      id: "req-3",
      sessionId: "s-perm",
      message: "执行测试命令: `cargo test`",
      allowSession: true,
    });
    await nextTick();

    // 1. 普通焦点下按快捷键 A / S / Escape
    window.dispatchEvent(new KeyboardEvent("keydown", { key: "a", bubbles: true }));
    expect(resolveSpy).toHaveBeenLastCalledWith("allow");

    window.dispatchEvent(new KeyboardEvent("keydown", { key: "s", bubbles: true }));
    expect(resolveSpy).toHaveBeenLastCalledWith("allow_session");

    window.dispatchEvent(new KeyboardEvent("keydown", { key: "Escape", bubbles: true }));
    expect(resolveSpy).toHaveBeenLastCalledWith("reject");
    expect(resolveSpy).toHaveBeenCalledTimes(3);

    // 2. 当事件来自 <textarea>（用户正在输入框打字）时，按 A 不得误触发审批
    const textarea = document.createElement("textarea");
    document.body.appendChild(textarea);
    textarea.dispatchEvent(new KeyboardEvent("keydown", { key: "a", bubbles: true }));
    document.body.removeChild(textarea);

    expect(resolveSpy).toHaveBeenCalledTimes(3);
  });
});
