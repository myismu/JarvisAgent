<script setup lang="ts">
import { onMounted, ref } from "vue";
import { useJarvis } from "./composables/useJarvis";

import TitleBar from "./components/layout/TitleBar.vue";
import Sidebar from "./components/layout/Sidebar.vue";
import ChatArea from "./components/chat/ChatArea.vue";
import TerminalInput from "./components/chat/TerminalInput.vue";
import PermissionModal from "./components/common/PermissionModal.vue";
import PlanPreviewPanel from "./components/common/PlanPreviewPanel.vue";
import SettingsPanel from "./components/settings/SettingsPanel.vue";

const showSettings = ref(false);

const { initListeners, systemStatus } = useJarvis();

onMounted(async () => {
  await initListeners();
});
</script>

<template>
  <main class="editor-container">
    <div class="editor-window">
      <TitleBar @open-settings="showSettings = true" />

      <div class="editor-body">
        <Sidebar />

        <div class="main-content">
          <div class="tab-bar">
            <div class="tab active">
              <span class="tab-icon">
                <svg viewBox="0 0 24 24" width="14" height="14" stroke="currentColor" stroke-width="2" fill="none" stroke-linecap="round" stroke-linejoin="round">
                  <path d="M13 2L3 14h9l-1 8 10-12h-9l1-8z"></path>
                </svg>
              </span>
              output.log
            </div>
            <div class="status-indicator" :class="systemStatus.toLowerCase()">
              Status: {{ systemStatus }}
            </div>
          </div>
          
          <ChatArea />
          <TerminalInput />
        </div>
      </div>
    </div>

    <PermissionModal />
    <PlanPreviewPanel />
    <SettingsPanel v-model="showSettings" />
  </main>
</template>

<!-- 组件局部样式：仅包含布局相关的样式，全局变量和重置已迁移到 global.css -->
<style scoped>
.editor-container {
  height: 100%;
  width: 100%;
  display: flex;
  background-color: var(--bg-panel);
  color: var(--text-main);
  font-family: var(--font-mono);
  overflow: hidden;
  transition: all 0.3s ease;
}

.editor-window {
  width: 100%;
  height: 100%;
  background-color: var(--bg-panel);
  display: flex;
  flex-direction: column;
  overflow: hidden;
}

.editor-body {
  display: flex;
  flex: 1;
  overflow: hidden;
}

.main-content {
  flex: 1;
  display: flex;
  flex-direction: column;
  background-color: var(--bg-panel);
  min-width: 0;
  min-height: 0;
}

.tab-bar {
  height: 35px;
  background-color: var(--bg-sidebar);
  display: flex;
  justify-content: space-between;
  align-items: center;
  border-bottom: 1px solid var(--border-color);
  flex-shrink: 0;
}

.tab {
  background-color: var(--bg-panel);
  border-top: 1px solid var(--accent-blue);
  border-right: 1px solid var(--border-color);
  padding: 0 15px;
  height: 100%;
  display: flex;
  align-items: center;
  font-size: 0.85rem;
  color: var(--text-main);
}

.tab-icon {
  margin-right: 6px;
  color: var(--accent-blue);
  display: inline-flex;
  align-items: center;
}

.status-indicator {
  font-size: 0.75rem;
  padding: 0 15px;
  color: var(--text-muted);
}
.status-indicator.finish { color: var(--accent-green); }
.status-indicator.error { color: var(--accent-red); }
.status-indicator.running { color: var(--accent-yellow); }
.status-indicator.cancelled { color: var(--accent-red); opacity: 0.7; }
</style>