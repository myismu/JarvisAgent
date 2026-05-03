<script setup lang="ts">
import { useWindow } from '../../composables/useWindow';

const { closeWindow, minimizeWindow, maximizeWindow } = useWindow();

defineProps<{
  sidebarCollapsed: boolean;
}>()
</script>

<template>
  <div class="editor-header" data-tauri-drag-region>
    <div class="window-controls">
      <span class="control close" @click="closeWindow"></span>
      <span class="control minimize" @click="minimizeWindow"></span>
      <span class="control maximize" @click="maximizeWindow"></span>
    </div>
    <div
      class="window-title"
      :class="{
        'sidebar-open': !sidebarCollapsed
      }"
      data-tauri-drag-region
    >
      <div class="reactor-label">
        <span class="label-char" style="--i:0">J</span><span class="label-dot">.</span>
        <span class="label-char" style="--i:1">A</span><span class="label-dot">.</span>
        <span class="label-char" style="--i:2">R</span><span class="label-dot">.</span>
        <span class="label-char" style="--i:3">V</span><span class="label-dot">.</span>
        <span class="label-char" style="--i:4">I</span><span class="label-dot">.</span>
        <span class="label-char" style="--i:5">S</span>
      </div>
    </div>
    <div class="header-spacer" data-tauri-drag-region></div>
  </div>
</template>

<style scoped>
.editor-header {
  height: var(--header-height);
  background: var(--glass-bg);
  backdrop-filter: blur(var(--glass-blur-heavy));
  -webkit-backdrop-filter: blur(var(--glass-blur-heavy));
  border-bottom: 1px solid var(--glass-border);
  display: flex;
  align-items: center;
  padding: 0 16px;
  user-select: none;
  transition: background var(--transition-normal);
}

.window-controls {
  display: flex;
  gap: 8px;
  align-items: center;
  width: 60px;
}

.control {
  width: 12px;
  height: 12px;
  border-radius: 50%;
  cursor: pointer;
  transition: filter var(--transition-fast), box-shadow var(--transition-fast);
  -webkit-app-region: no-drag;
}
.control:hover { filter: brightness(1.2); box-shadow: 0 0 6px rgba(255, 255, 255, 0.2); }
.control.close { background-color: #ff5f56; }
.control.minimize { background-color: #ffbd2e; }
.control.maximize { background-color: #27c93f; }

.window-title {
  flex: 1;
  position: relative;
  height: 100%;
  font-size: 0.85rem;
  font-weight: 600;
  color: var(--text-muted);
  letter-spacing: 0.02em;
}
.reactor-label {
  position: absolute;
  top: 50%;
  left: calc(50% + var(--sidebar-offset, 0px));
  transform: translate(-50%, -50%);
  display: inline-flex;
  align-items: center;
  gap: 0;
  line-height: 1;
  transition: left 0.25s ease;
}
.window-title.sidebar-open {
  --sidebar-offset: 125px;
}
.label-char {
  display: inline-block;
  animation: charPulse 3s ease-in-out infinite;
  animation-delay: calc(var(--i) * 0.2s);
}
.label-dot {
  display: inline-block;
  opacity: 0.4;
  font-size: 0.7em;
  margin: 0 1px;
}
@keyframes charPulse {
  0%, 100% { opacity: 0.7; }
  50% { opacity: 1; }
}
.header-spacer {
  width: 60px;
  height: 100%;
}
</style>
