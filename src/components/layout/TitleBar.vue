<script setup lang="ts">
import { useWindow } from '../../composables/useWindow';
import { useTheme } from '../../composables/useTheme';

const { closeWindow, minimizeWindow, maximizeWindow } = useWindow();
const { isDark, toggleTheme } = useTheme();

const emit = defineEmits<{
  (e: 'open-settings'): void
}>()
</script>

<template>
  <div class="editor-header" data-tauri-drag-region>
    <div class="window-controls">
      <span class="control close" @click="closeWindow"></span>
      <span class="control minimize" @click="minimizeWindow"></span>
      <span class="control maximize" @click="maximizeWindow"></span>
    </div>
    <div class="window-title" data-tauri-drag-region>Jarvis Agent Workspace</div>
    <div class="header-actions">
      <!-- 设置按钮 -->
      <button class="theme-toggle" @click="emit('open-settings')" title="系统设置">
        <svg viewBox="0 0 24 24" width="14" height="14" stroke="currentColor" stroke-width="2" fill="none" stroke-linecap="round" stroke-linejoin="round">
          <circle cx="12" cy="12" r="3"></circle>
          <path d="M19.4 15a1.65 1.65 0 0 0 .33 1.82l.06.06a2 2 0 0 1 0 2.83 2 2 0 0 1-2.83 0l-.06-.06a1.65 1.65 0 0 0-1.82-.33 1.65 1.65 0 0 0-1 1.51V21a2 2 0 0 1-2 2 2 2 0 0 1-2-2v-.09A1.65 1.65 0 0 0 9 19.4a1.65 1.65 0 0 0-1.82.33l-.06.06a2 2 0 0 1-2.83 0 2 2 0 0 1 0-2.83l.06-.06a1.65 1.65 0 0 0 .33-1.82 1.65 1.65 0 0 0-1.51-1H3a2 2 0 0 1-2-2 2 2 0 0 1 2-2h.09A1.65 1.65 0 0 0 4.6 9a1.65 1.65 0 0 0-.33-1.82l-.06-.06a2 2 0 0 1 0-2.83 2 2 0 0 1 2.83 0l.06.06a1.65 1.65 0 0 0 1.82.33H9a1.65 1.65 0 0 0 1-1.51V3a2 2 0 0 1 2-2 2 2 0 0 1 2 2v.09a1.65 1.65 0 0 0 1 1.51 1.65 1.65 0 0 0 1.82-.33l.06-.06a2 2 0 0 1 2.83 0 2 2 0 0 1 0 2.83l-.06.06a1.65 1.65 0 0 0-.33 1.82V9a1.65 1.65 0 0 0 1.51 1H21a2 2 0 0 1 2 2 2 2 0 0 1-2 2h-.09a1.65 1.65 0 0 0-1.51 1z"></path>
        </svg>
      </button>

      <!-- 主题切换按钮 -->
      <button class="theme-toggle" @click="toggleTheme" :title="isDark ? '切换到亮色模式' : '切换到暗色模式'">
        <!-- 太阳图标（暗色模式下显示，点击切换到亮色） -->
        <svg v-if="isDark" viewBox="0 0 24 24" width="14" height="14" stroke="currentColor" stroke-width="2" fill="none" stroke-linecap="round" stroke-linejoin="round">
          <circle cx="12" cy="12" r="5"></circle>
          <line x1="12" y1="1" x2="12" y2="3"></line>
          <line x1="12" y1="21" x2="12" y2="23"></line>
          <line x1="4.22" y1="4.22" x2="5.64" y2="5.64"></line>
          <line x1="18.36" y1="18.36" x2="19.78" y2="19.78"></line>
          <line x1="1" y1="12" x2="3" y2="12"></line>
          <line x1="21" y1="12" x2="23" y2="12"></line>
          <line x1="4.22" y1="19.78" x2="5.64" y2="18.36"></line>
          <line x1="18.36" y1="5.64" x2="19.78" y2="4.22"></line>
        </svg>
        <!-- 月亮图标（亮色模式下显示，点击切换到暗色） -->
        <svg v-else viewBox="0 0 24 24" width="14" height="14" stroke="currentColor" stroke-width="2" fill="none" stroke-linecap="round" stroke-linejoin="round">
          <path d="M21 12.79A9 9 0 1 1 11.21 3 7 7 0 0 0 21 12.79z"></path>
        </svg>
      </button>
    </div>
  </div>
</template>

<style scoped>
.editor-header {
  height: 35px;
  background-color: var(--bg-sidebar);
  border-bottom: 1px solid var(--border-color);
  display: flex;
  align-items: center;
  padding: 0 15px;
  user-select: none;
}

.window-controls {
  display: flex;
  gap: 8px;
}

.control {
  width: 12px;
  height: 12px;
  border-radius: 50%;
  cursor: pointer;
}
.control.close { background-color: #ff5f56; }
.control.minimize { background-color: #ffbd2e; }
.control.maximize { background-color: #27c93f; }

.window-title {
  flex: 1;
  text-align: center;
  font-size: 0.85rem;
  color: var(--text-muted);
}

.header-actions {
  display: flex;
  align-items: center;
  gap: 8px;
  -webkit-app-region: no-drag;
}

.theme-toggle {
  background: none;
  border: 1px solid var(--border-color);
  border-radius: 4px;
  color: var(--text-muted);
  cursor: pointer;
  padding: 3px 6px;
  display: inline-flex;
  align-items: center;
  transition: all var(--transition-fast);
}
.theme-toggle:hover {
  color: var(--accent-blue);
  border-color: var(--accent-blue);
  background: rgba(0, 102, 204, 0.08);
}
</style>
