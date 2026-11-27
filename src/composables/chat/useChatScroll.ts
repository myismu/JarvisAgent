/**
 * # useChatScroll.ts — 聊天区滚动锚定机、单帧合批吸底与上滑懒加载
 *
 * 集中管理 `ChatArea.vue` 的滚动容器状态：
 * - 切会话锚定期（`initialAnchoring`）：渲染分帧稳定后再显现，直接定位在最新消息；
 * - 流式跟随单帧合批调度（rAF）：将 `renderTick`、`currentTurn.revision` 与 store 滚动回调
 *   合并为单次绘制前回调，遵守 `prefs.autoScroll && atBottom` 总闸；
 * - 顶部懒加载与双 rAF 高度补偿：头部插入历史消息后视口不跳动。
 *
 * ## Key Exports
 * - `useChatScroll()`: 返回滚动容器 ref、锚定状态、懒加载提示与滚动操作函数
 *
 * ## Dependencies
 * - Internal: `@/stores/session`, `@/stores/chat`, `@/composables/usePreferences`
 * - External: `vue`, `vue-i18n`
 *
 * ## Constraints
 * - 必须在组件 setup 期间调用，内部在 `onMounted` / `onUnmounted` 自动注册与释放滚动回调和 rAF
 */
import { computed, nextTick, onMounted, onUnmounted, ref, watch, type ComputedRef } from "vue";
import { useI18n } from "vue-i18n";
import { useSessionStore } from "../../stores/session";
import { useChatStore } from "../../stores/chat";
import { usePreferences } from "../usePreferences";

interface UseChatScrollOptions {
  hasCurrentTurnContent: ComputedRef<boolean>;
  currentTurnRevision: ComputedRef<number>;
  onSessionChange?: () => void;
}

export function useChatScroll(options: UseChatScrollOptions) {
  const { hasCurrentTurnContent, currentTurnRevision, onSessionChange } = options;
  const { t } = useI18n();
  const session = useSessionStore();
  const chat = useChatStore();
  const prefs = usePreferences();

  const responseAreaRef = ref<HTMLElement | null>(null);
  /** 视口是否贴底：滚动事件与程序化落底共同维护的统一事实源 */
  const atBottom = ref(true);
  /** 流式跟随总闸 = 用户开启自动滚动 且 当前视口处于底部 */
  const shouldFollowStream = computed(() => prefs.autoScroll && atBottom.value);
  /** 懒加载锚定期标志：头部插入与高度补偿期间屏蔽重复触发 */
  const historyAnchoring = ref(false);
  /** 切会话锚定期：列表不可见但保留布局，稳定落底后一次性显现 */
  const initialAnchoring = ref(false);

  const pendingInitialScrollSessionKey = ref<string | null>(null);
  const currentSessionKey = computed(() => session.activeSessionId || "__default__");

  let followScrollRafId: number | null = null;

  const isResponseAtBottom = () => {
    if (!responseAreaRef.value) return false;
    const { scrollTop, scrollHeight, clientHeight } = responseAreaRef.value;
    return scrollHeight - scrollTop - clientHeight <= 35;
  };

  const setResponseScrollToBottom = () => {
    if (!responseAreaRef.value) return;
    responseAreaRef.value.scrollTop = responseAreaRef.value.scrollHeight;
  };

  const forceScrollToBottomAfterRender = async () => {
    // 只恢复「贴底」事实，不越过自动跟随总闸；开跑与切会话定位属导航行为始终落底
    atBottom.value = true;

    await nextTick();
    setResponseScrollToBottom();

    // 持续跟随到内容高度连续 2 帧稳定才算落底（防止分帧撑高导致停在半腰）
    let lastHeight = -1;
    let stableFrames = 0;
    let totalFrames = 0;
    await new Promise<void>((resolve) => {
      const tick = () => {
        if (!responseAreaRef.value) return resolve();
        setResponseScrollToBottom();
        const h = responseAreaRef.value.scrollHeight;
        stableFrames = h === lastHeight ? stableFrames + 1 : 0;
        lastHeight = h;
        totalFrames += 1;
        if (stableFrames >= 2 || totalFrames > 60) return resolve();
        requestAnimationFrame(tick);
      };
      requestAnimationFrame(tick);
    });
  };

  /** 单帧合批流式跟随调度器（rAF）：同一帧内多次触发仅测量/滚动一次 */
  const scheduleFollowScroll = () => {
    if (followScrollRafId !== null) return;
    followScrollRafId = requestAnimationFrame(() => {
      followScrollRafId = null;
      if (!responseAreaRef.value) return;
      if (shouldFollowStream.value) {
        responseAreaRef.value.scrollTop = responseAreaRef.value.scrollHeight;
        return;
      }
      // 内容增长顶离底部时不会触发 scroll 事件，必须重算贴底状态以便显现回底按钮
      atBottom.value = isResponseAtBottom();
    });
  };

  const scrollToBottom = async (force = false) => {
    if (force) {
      if (followScrollRafId !== null) {
        cancelAnimationFrame(followScrollRafId);
        followScrollRafId = null;
      }
      await forceScrollToBottomAfterRender();
      return;
    }
    scheduleFollowScroll();
  };

  /** 上滑接近顶部时加载更早历史，并通过双 rAF 补偿 scrollHeight 差值保持视口静止 */
  const maybeLoadOlderMessages = async () => {
    const el = responseAreaRef.value;
    if (!el) return;
    const view = session.currentSessionView;
    if (!view.pagedHasMore || view.pagedLoadingOlder || historyAnchoring.value) return;
    if (el.scrollTop > 60) return;
    const sid = session.activeSessionId;
    if (!sid) return;
    const prevHeight = el.scrollHeight;
    const prevTop = el.scrollTop;
    historyAnchoring.value = true;
    const loaded = await chat.loadSessionMessagesOlder(sid);
    if (!loaded) {
      historyAnchoring.value = false;
      return;
    }
    await nextTick();
    await new Promise<void>((resolve) =>
      requestAnimationFrame(() => requestAnimationFrame(() => resolve())),
    );
    if (responseAreaRef.value) {
      responseAreaRef.value.scrollTop =
        prevTop + (responseAreaRef.value.scrollHeight - prevHeight);
    }
    historyAnchoring.value = false;
  };

  const handleResponseScroll = () => {
    atBottom.value = isResponseAtBottom();
    void maybeLoadOlderMessages();
  };

  const showScrollToBottom = computed(() => {
    return !atBottom.value && (chat.messages.length > 0 || hasCurrentTurnContent.value);
  });

  const showHistoryLoader = computed(() => {
    const v = session.currentSessionView;
    return v.pagedHasMore || v.pagedLoadingOlder || v.pagedOldestSeq !== null;
  });

  const historyLoaderText = computed(() => {
    const v = session.currentSessionView;
    if (v.pagedLoadingOlder) return t("chat.loadingOlderMessages");
    if (v.pagedHasMore) return t("chat.scrollForOlderMessages");
    return v.pagedOldestSeq !== null ? t("chat.allMessagesLoaded") : "";
  });

  watch(
    () => session.isCurrentSessionRunning,
    (running) => {
      if (running) {
        void scrollToBottom(true);
      }
    },
  );

  watch(
    currentSessionKey,
    (key) => {
      pendingInitialScrollSessionKey.value = key;
      initialAnchoring.value = true;
      onSessionChange?.();
    },
    { immediate: true },
  );

  watch(
    () => [
      currentSessionKey.value,
      session.currentSessionView.hydration,
      chat.messages.length,
      hasCurrentTurnContent.value,
    ],
    async ([key, hydration, msgCount, hasTurnContent]) => {
      if (pendingInitialScrollSessionKey.value !== key) return;
      const loaded = hydration === "ready" || hydration === "stale";
      if (!loaded && !msgCount && !hasTurnContent) return;

      pendingInitialScrollSessionKey.value = null;
      await forceScrollToBottomAfterRender();
      initialAnchoring.value = false;
    },
    { immediate: true, flush: "post" },
  );

  watch(
    () => [chat.renderTick, currentTurnRevision.value],
    () => {
      scheduleFollowScroll();
    },
  );

  onMounted(() => {
    chat.registerScrollCb(scrollToBottom);
  });

  onUnmounted(() => {
    chat.unregisterScrollCb(scrollToBottom);
    if (followScrollRafId !== null) {
      cancelAnimationFrame(followScrollRafId);
      followScrollRafId = null;
    }
  });

  return {
    responseAreaRef,
    atBottom,
    initialAnchoring,
    showScrollToBottom,
    showHistoryLoader,
    historyLoaderText,
    scrollToBottom,
    handleResponseScroll,
  };
}
