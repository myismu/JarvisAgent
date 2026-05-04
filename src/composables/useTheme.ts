// --- 主题切换 composable ---
// 管理亮色/暗色模式切换，持久化到 localStorage

import { ref, onMounted, onUnmounted } from "vue";
import { useWindow } from "./useWindow";

// 全局单例状态
const isDark = ref(false);

// 确保监听器只绑定一次
let isListening = false;

export function useTheme() {
  const { notifyMonitorThemeChanged, onMonitorThemeChanged } = useWindow();

  // 初始化：从 localStorage 读取用户偏好
  onMounted(async () => {
    const saved = localStorage.getItem("darkMode");
    if (saved === "true") {
      isDark.value = true;
      document.body.classList.add("dark-mode");
    }

    if (!isListening) {
      isListening = true;
      const unlisten = await onMonitorThemeChanged((dark) => {
        if (isDark.value !== dark) {
          isDark.value = dark;
          if (dark) {
            document.body.classList.add("dark-mode");
          } else {
            document.body.classList.remove("dark-mode");
          }
          localStorage.setItem("darkMode", String(dark));
        }
      });
      
      onUnmounted(() => {
        unlisten();
        isListening = false;
      });
    }
  });

  // 切换主题
  const toggleTheme = async () => {
    isDark.value = !isDark.value;
    if (isDark.value) {
      document.body.classList.add("dark-mode");
    } else {
      document.body.classList.remove("dark-mode");
    }
    localStorage.setItem("darkMode", String(isDark.value));
    await notifyMonitorThemeChanged(isDark.value);
  };

  return {
    isDark,
    toggleTheme,
  };
}
