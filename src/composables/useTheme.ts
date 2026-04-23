// --- 主题切换 composable ---
// 管理亮色/暗色模式切换，持久化到 localStorage

import { ref, onMounted } from "vue";

// 全局单例状态
const isDark = ref(false);

export function useTheme() {
  // 初始化：从 localStorage 读取用户偏好
  onMounted(() => {
    const saved = localStorage.getItem("darkMode");
    if (saved === "true") {
      isDark.value = true;
      document.body.classList.add("dark-mode");
    }
  });

  // 切换主题
  const toggleTheme = () => {
    isDark.value = !isDark.value;
    if (isDark.value) {
      document.body.classList.add("dark-mode");
    } else {
      document.body.classList.remove("dark-mode");
    }
    localStorage.setItem("darkMode", String(isDark.value));
  };

  return {
    isDark,
    toggleTheme,
  };
}
