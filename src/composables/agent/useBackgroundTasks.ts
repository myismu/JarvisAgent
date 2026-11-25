/**
 * # useBackgroundTasks.ts — 监控面板后台任务轮询与控制
 *
 * 管理当前会话的后台任务列表，通过 Tauri `bg-task-done` 事件实现零延迟刷新，
 * 并在面板可见期间保留 3 秒轮询作为异常兜底；组件卸载时自动清理监听与定时器。
 *
 * ## Key Exports
 * - `useBackgroundTasks()`: 返回任务列表、统计派生值与终止/清理方法
 *
 * ## Dependencies
 * - Internal: `@/stores/session`, `@/types`
 * - External: `@tauri-apps/api/core`, `@tauri-apps/api/event`, `vue`
 */
import { computed, onUnmounted, ref, watch, type Ref } from "vue";
import { invoke } from "@tauri-apps/api/core";
import { listen, type UnlistenFn } from "@tauri-apps/api/event";
import { useSessionStore } from "../../stores/session";
import type { BackgroundTask } from "../../types";

export function useBackgroundTasks(panelVisible: Ref<boolean>) {
  const session = useSessionStore();
  const backgroundTasks = ref<BackgroundTask[]>([]);
  const dismissingTasks = ref<Set<string>>(new Set());
  let backgroundTimer: ReturnType<typeof setInterval> | null = null;
  let unlistenDone: UnlistenFn | null = null;

  const recentBackgroundTasks = computed(() => backgroundTasks.value.slice(0, 4));
  const runningBackgroundCount = computed(
    () => backgroundTasks.value.filter((task) => task.status === "running").length,
  );

  const refreshBackgroundTasks = async () => {
    try {
      const sid = session.activeSessionId;
      backgroundTasks.value = await invoke<BackgroundTask[]>("get_background_tasks", {
        sessionId: sid,
      });
    } catch (err) {
      console.error("加载后台任务失败:", err);
    }
  };

  const dismissTask = async (taskId: string) => {
    dismissingTasks.value.add(taskId);
    try {
      await invoke("dismiss_background_task", { taskId });
      backgroundTasks.value = backgroundTasks.value.filter((t) => t.id !== taskId);
    } catch (err) {
      console.error("清理后台任务失败:", err);
    } finally {
      dismissingTasks.value.delete(taskId);
    }
  };

  const killTask = async (taskId: string) => {
    dismissingTasks.value.add(taskId);
    try {
      await invoke("kill_background_task", { taskId });
      await refreshBackgroundTasks();
    } catch (err) {
      console.error("终止后台任务失败:", err);
    } finally {
      dismissingTasks.value.delete(taskId);
    }
  };

  const clearAllDoneTasks = async () => {
    const sid = session.activeSessionId;
    if (!sid) return;
    try {
      await invoke("clear_session_background_tasks", { sessionId: sid });
      await refreshBackgroundTasks();
    } catch (err) {
      console.error("清理后台任务失败:", err);
    }
  };

  void listen("bg-task-done", () => {
    void refreshBackgroundTasks();
  }).then((fn) => {
    unlistenDone = fn;
  });

  watch(
    panelVisible,
    (visible) => {
      if (visible) {
        void refreshBackgroundTasks();
        if (!backgroundTimer) {
          backgroundTimer = setInterval(refreshBackgroundTasks, 3000);
        }
      } else if (backgroundTimer) {
        clearInterval(backgroundTimer);
        backgroundTimer = null;
      }
    },
    { immediate: true },
  );

  onUnmounted(() => {
    if (backgroundTimer) {
      clearInterval(backgroundTimer);
      backgroundTimer = null;
    }
    if (unlistenDone) {
      unlistenDone();
      unlistenDone = null;
    }
  });

  return {
    backgroundTasks,
    dismissingTasks,
    recentBackgroundTasks,
    runningBackgroundCount,
    refreshBackgroundTasks,
    dismissTask,
    killTask,
    clearAllDoneTasks,
  };
}
