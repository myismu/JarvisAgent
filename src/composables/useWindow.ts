import { getCurrentWindow } from "@tauri-apps/api/window";

export function useWindow() {
  const closeWindow = async () => {
    const appWindow = getCurrentWindow();
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

  return {
    closeWindow,
    minimizeWindow,
    maximizeWindow,
  };
}
