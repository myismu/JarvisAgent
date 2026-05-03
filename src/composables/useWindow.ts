/*
# useWindow.ts — Tauri 窗口控制封装

封装当前窗口基础控制，以及独立监控窗口的创建、定位、聚焦和关闭逻辑。

## Key Exports
- `useWindow`: 返回窗口控制方法集合

## Dependencies
- External: `@tauri-apps/api/window`, `@tauri-apps/api/webviewWindow`, `@tauri-apps/api/dpi`
*/
import { PhysicalPosition, PhysicalSize } from "@tauri-apps/api/dpi";
import { invoke } from "@tauri-apps/api/core";
import { getCurrentWindow, Window } from "@tauri-apps/api/window";
import type { UnlistenFn } from "@tauri-apps/api/event";
import { TauriEvent } from "@tauri-apps/api/event";
import { WebviewWindow } from "@tauri-apps/api/webviewWindow";
import { emit, listen } from "@tauri-apps/api/event";

const MAIN_WINDOW_LABEL = "main";
const MONITOR_WINDOW_LABEL = "monitor";
const MONITOR_WINDOW_WIDTH = 640;
const MONITOR_WINDOW_HEIGHT = 760;
const MONITOR_WINDOW_GAP = 12;
const MONITOR_WINDOW_CLOSED_EVENT = "monitor-window-closed";
const MONITOR_SESSION_CHANGED_EVENT = "monitor-session-changed";

type CustomWindowState = {
  width: number;
  height: number;
  x: number;
  y: number;
  maximized: boolean;
  fullscreen: boolean;
  decorated: boolean;
  updatedAt: number;
};

const readCustomWindowState = async (label: string): Promise<CustomWindowState | null> => {
  return invoke<CustomWindowState | null>("get_custom_window_state", { label });
};

const applyCustomWindowState = async (window: Window | WebviewWindow, state: CustomWindowState | null) => {
  if (!state) return;

  await window.setSize(new PhysicalSize(state.width, state.height));
  await window.setPosition(new PhysicalPosition(state.x, state.y));
};

const captureCustomWindowState = async (window: Window | WebviewWindow): Promise<CustomWindowState> => {
  const [size, position, maximized, fullscreen, decorated] = await Promise.all([
    window.outerSize(),
    window.outerPosition(),
    window.isMaximized(),
    window.isFullscreen(),
    window.isDecorated(),
  ]);

  return {
    width: size.width,
    height: size.height,
    x: position.x,
    y: position.y,
    maximized,
    fullscreen,
    decorated,
    updatedAt: Date.now(),
  };
};

const saveCustomWindowState = async (label: string, window: Window | WebviewWindow) => {
  const state = await captureCustomWindowState(window);
  await invoke("save_custom_window_state", { label, state });
};

const defaultMonitorPosition = async () => {
  const mainWindow = await getMainWindow();
  const mainPosition = await mainWindow.outerPosition();
  const mainSize = await mainWindow.outerSize();

  return new PhysicalPosition(
    mainPosition.x + mainSize.width + MONITOR_WINDOW_GAP,
    mainPosition.y,
  );
};

const monitorUrl = (): string => {
  const isDev = window.location.origin.startsWith("http");
  return isDev ? `${window.location.origin}/monitor.html` : "monitor.html";
};

const getMainWindow = async (): Promise<Window> => {
  return (await Window.getByLabel(MAIN_WINDOW_LABEL)) ?? getCurrentWindow();
};

const getMonitorWindow = async (): Promise<WebviewWindow | null> => {
  return WebviewWindow.getByLabel(MONITOR_WINDOW_LABEL);
};

export function useWindow() {
  const closeWindow = async () => {
    const appWindow = getCurrentWindow();
    if (appWindow.label === MAIN_WINDOW_LABEL) {
      await closeMonitorWindow();
    }
    await appWindow.close();
  };

  const minimizeWindow = async () => {
    const appWindow = getCurrentWindow();
    await appWindow.minimize();
  };

  const maximizeWindow = async () => {
    const appWindow = getCurrentWindow();
    await appWindow.toggleMaximize();
  };

  const positionMonitorNextToMain = async (monitorWindow: WebviewWindow) => {
    const savedState = await readCustomWindowState(MONITOR_WINDOW_LABEL);
    if (savedState) {
      await applyCustomWindowState(monitorWindow, savedState);
      return;
    }

    await monitorWindow.setPosition(await defaultMonitorPosition());
  };

  const focusMonitorWindow = async (): Promise<WebviewWindow | null> => {
    const monitorWindow = await getMonitorWindow();
    if (!monitorWindow) return null;

    await monitorWindow.show();
    if (await monitorWindow.isMinimized()) {
      await monitorWindow.unminimize();
    }
    await monitorWindow.setFocus();
    return monitorWindow;
  };

  const openMonitorWindow = async (): Promise<WebviewWindow> => {
    const existingWindow = await focusMonitorWindow();
    if (existingWindow) return existingWindow;

    const savedState = await readCustomWindowState(MONITOR_WINDOW_LABEL);
    const monitorWindow = new WebviewWindow(MONITOR_WINDOW_LABEL, {
      url: monitorUrl(),
      title: "执行监控",
      width: savedState?.width ?? MONITOR_WINDOW_WIDTH,
      height: savedState?.height ?? MONITOR_WINDOW_HEIGHT,
      x: savedState?.x,
      y: savedState?.y,
      minWidth: 520,
      minHeight: 520,
      maximized: savedState?.maximized,
      fullscreen: savedState?.fullscreen,
      decorations: savedState?.decorated ?? false,
      transparent: true,
      resizable: true,
      visible: false,
    });

    await new Promise<void>((resolve, reject) => {
      monitorWindow.once("tauri://created", () => resolve());
      monitorWindow.once("tauri://error", (event) => reject(event.payload));
    });

    await positionMonitorNextToMain(monitorWindow);
    await monitorWindow.show();
    await monitorWindow.setFocus();
    return monitorWindow;
  };

  const closeMonitorWindow = async () => {
    const monitorWindow = await getMonitorWindow();
    if (monitorWindow) {
      await saveCustomWindowState(MONITOR_WINDOW_LABEL, monitorWindow);
    }
    await emit(MONITOR_WINDOW_CLOSED_EVENT);
    await monitorWindow?.close();
  };

  const onMonitorWindowClosed = async (handler: () => void) => {
    return listen(MONITOR_WINDOW_CLOSED_EVENT, handler);
  };

  const notifyMonitorSessionChanged = async (sessionId: string | null) => {
    await emit(MONITOR_SESSION_CHANGED_EVENT, { sessionId });
  };

  const onMonitorSessionChanged = async (handler: (sessionId: string | null) => void) => {
    return listen<{ sessionId: string | null }>(MONITOR_SESSION_CHANGED_EVENT, (event) => {
      handler(event.payload.sessionId);
    });
  };

  const toggleMonitorWindow = async (visible: boolean): Promise<boolean> => {
    if (visible) {
      await closeMonitorWindow();
      return false;
    }

    await openMonitorWindow();
    return true;
  };

  const resetWindowStates = async () => {
    await invoke("clear_custom_window_states");

    const mainWindow = await getMainWindow();
    await mainWindow.setSize(new PhysicalSize(1600, 1000));
    await mainWindow.setPosition(new PhysicalPosition(80, 60));

    const monitorWindow = await getMonitorWindow();
    if (monitorWindow) {
      await monitorWindow.setSize(new PhysicalSize(MONITOR_WINDOW_WIDTH, MONITOR_WINDOW_HEIGHT));
      await monitorWindow.setPosition(await defaultMonitorPosition());
    }
  };

  const restoreCurrentWindowState = async () => {
    const currentWindow = getCurrentWindow();
    await applyCustomWindowState(currentWindow, await readCustomWindowState(currentWindow.label));
  };

  const persistCurrentWindowState = async () => {
    const currentWindow = getCurrentWindow();
    await saveCustomWindowState(currentWindow.label, currentWindow);
  };

  const watchCurrentWindowState = async () => {
    const currentWindow = getCurrentWindow();
    let saveTimer: ReturnType<typeof window.setTimeout> | null = null;
    let lastSave: Promise<void> = Promise.resolve();
    let closingMainWindow = false;
    const scheduleSave = () => {
      if (saveTimer !== null) {
        window.clearTimeout(saveTimer);
      }
      saveTimer = window.setTimeout(() => {
        lastSave = persistCurrentWindowState();
      }, 300);
    };

    const unlisteners: UnlistenFn[] = [];
    unlisteners.push(await currentWindow.onMoved(scheduleSave));
    unlisteners.push(await currentWindow.onResized(scheduleSave));

    if (currentWindow.label === MAIN_WINDOW_LABEL) {
      unlisteners.push(await currentWindow.onCloseRequested(async (event) => {
        if (closingMainWindow) return;

        event.preventDefault();
        closingMainWindow = true;
        await lastSave;
        await closeMonitorWindow();
        await currentWindow.close();
      }));
      unlisteners.push(await listen(TauriEvent.WINDOW_DESTROYED, async () => {
        await closeMonitorWindow();
      }, { target: { kind: "Window", label: MAIN_WINDOW_LABEL } }));
    }

    return () => {
      if (saveTimer !== null) {
        window.clearTimeout(saveTimer);
      }
      unlisteners.forEach((unlisten) => unlisten());
    };
  };

  return {
    closeWindow,
    minimizeWindow,
    maximizeWindow,
    openMonitorWindow,
    closeMonitorWindow,
    focusMonitorWindow,
    positionMonitorNextToMain,
    onMonitorWindowClosed,
    notifyMonitorSessionChanged,
    onMonitorSessionChanged,
    toggleMonitorWindow,
    resetWindowStates,
    restoreCurrentWindowState,
    persistCurrentWindowState,
    watchCurrentWindowState,
  };
}
