<script setup lang="ts">
import { ref, watch, onUnmounted, computed } from "vue";
import { listen } from "@tauri-apps/api/event";
import { invoke } from "@tauri-apps/api/core";

interface BackgroundTask {
  id: string;
  command: string;
  status: string;
  result: string | null;
  port: number | null;
  task_type: string | null;
}

const props = defineProps<{
  workspacePath?: string;
}>();

const previewUrl = ref<string | null>(null);
const previewMode = ref<"auto" | "dev" | "file">("auto");
const devPort = ref(5173);
const showPreview = ref(false);
const isLoading = ref(false);
const error = ref<string | null>(null);
const backgroundTasks = ref<BackgroundTask[]>([]);
const autoDetectedPort = ref<number | null>(null);
const autoDetectedType = ref<string | null>(null);

const runningFrontendTasks = computed(() => {
  return backgroundTasks.value.filter(
    t => t.status === "running" && t.task_type === "frontend"
  );
});

const activePreviewPort = computed(() => {
  if (previewMode.value === "dev") {
    return devPort.value;
  }
  if (previewMode.value === "auto" && autoDetectedPort.value) {
    return autoDetectedPort.value;
  }
  return devPort.value;
});

const fetchBackgroundTasks = async () => {
  try {
    const tasks = await invoke<BackgroundTask[]>("get_background_tasks");
    backgroundTasks.value = tasks;
    
    if (previewMode.value === "auto") {
      const frontendTask = tasks.find(
        t => t.status === "running" && t.task_type === "frontend" && t.port
      );
      if (frontendTask && frontendTask.port) {
        autoDetectedPort.value = frontendTask.port;
        autoDetectedType.value = frontendTask.task_type;
      }
    }
  } catch (e) {
    console.error("Failed to fetch background tasks:", e);
  }
};

const buildPreviewUrl = () => {
  if (previewMode.value === "file" && props.workspacePath) {
    previewUrl.value = `file:///${props.workspacePath.replace(/\\/g, "/")}/index.html`;
  } else {
    const port = activePreviewPort.value;
    previewUrl.value = `http://localhost:${port}`;
  }
  showPreview.value = true;
  isLoading.value = true;
  error.value = null;
};

const togglePreview = () => {
  if (showPreview.value) {
    showPreview.value = false;
    previewUrl.value = null;
  } else {
    if (previewMode.value === "auto") {
      fetchBackgroundTasks().then(() => buildPreviewUrl());
    } else {
      buildPreviewUrl();
    }
  }
};

const refreshPreview = () => {
  if (previewUrl.value) {
    const current = previewUrl.value;
    previewUrl.value = null;
    setTimeout(() => {
      previewUrl.value = current;
      isLoading.value = true;
    }, 100);
  }
};

const onLoad = () => {
  isLoading.value = false;
};

const onError = () => {
  isLoading.value = false;
  error.value = "预览加载失败，请检查开发服务器是否运行";
};

watch(
  () => props.workspacePath,
  () => {
    if (showPreview.value) {
      buildPreviewUrl();
    }
  }
);

watch(previewMode, () => {
  if (showPreview.value) {
    if (previewMode.value === "auto") {
      fetchBackgroundTasks().then(() => buildPreviewUrl());
    } else {
      buildPreviewUrl();
    }
  }
});

let unlistenSnapshot: (() => void) | null = null;
let taskPollInterval: ReturnType<typeof setInterval> | null = null;

(async () => {
  unlistenSnapshot = await listen("snapshot-created", () => {
    if (showPreview.value && previewMode.value === "file") {
      refreshPreview();
    }
  });
  
  await fetchBackgroundTasks();
  
  taskPollInterval = setInterval(fetchBackgroundTasks, 3000);
})();

onUnmounted(() => {
  if (unlistenSnapshot) unlistenSnapshot();
  if (taskPollInterval) clearInterval(taskPollInterval);
});
</script>

<template>
  <div class="live-preview-container">
    <div class="preview-toolbar">
      <button
        :class="['preview-toggle', { active: showPreview }]"
        @click="togglePreview"
        :title="showPreview ? '关闭预览' : '打开预览'"
      >
        <svg
          viewBox="0 0 24 24"
          width="14"
          height="14"
          stroke="currentColor"
          stroke-width="2"
          fill="none"
          stroke-linecap="round"
          stroke-linejoin="round"
        >
          <path d="M1 12s4-8 11-8 11 8 11 8-4 8-11 8-11-8-11-8z" />
          <circle cx="12" cy="12" r="3" />
        </svg>
        <span>{{ showPreview ? "关闭预览" : "预览" }}</span>
      </button>

      <template v-if="showPreview">
        <div class="mode-switch">
          <button
            :class="['mode-btn', { active: previewMode === 'auto' }]"
            @click="previewMode = 'auto'"
            title="自动检测后台服务"
          >
            自动
          </button>
          <button
            :class="['mode-btn', { active: previewMode === 'dev' }]"
            @click="previewMode = 'dev'"
          >
            Dev Server
          </button>
          <button
            :class="['mode-btn', { active: previewMode === 'file' }]"
            @click="previewMode = 'file'"
          >
            HTML 文件
          </button>
        </div>

        <div v-if="previewMode === 'dev'" class="port-input">
          <input
            v-model.number="devPort"
            type="number"
            min="1"
            max="65535"
            class="port-field"
            @change="buildPreviewUrl()"
          />
        </div>

        <div v-if="previewMode === 'auto'" class="auto-status">
          <span v-if="runningFrontendTasks.length > 0" class="status-badge running">
            {{ runningFrontendTasks.length }} 前端运行中
          </span>
          <span v-else class="status-badge idle">无服务</span>
        </div>

        <button class="refresh-btn" @click="refreshPreview" title="刷新预览">
          <svg
            viewBox="0 0 24 24"
            width="14"
            height="14"
            stroke="currentColor"
            stroke-width="2"
            fill="none"
          >
            <path
              d="M23 4v6h-6M1 20v-6h6M3.51 9a9 9 0 0 1 14.85-3.36L23 10M1 14l4.64 4.36A9 9 0 0 0 20.49 15"
            />
          </svg>
        </button>
      </template>
    </div>

    <div v-if="showPreview" class="preview-frame-wrapper">
      <div v-if="isLoading" class="preview-loading">
        <div class="spinner"></div>
        <span>加载预览中...</span>
      </div>
      <div v-if="error" class="preview-error">{{ error }}</div>
      <iframe
        v-if="previewUrl"
        :src="previewUrl"
        class="preview-frame"
        @load="onLoad"
        @error="onError"
        sandbox="allow-scripts allow-same-origin allow-forms"
      />
    </div>
  </div>
</template>

<style scoped>
.live-preview-container {
  display: flex;
  flex-direction: column;
  border-top: 1px solid var(--glass-border);
}

.preview-toolbar {
  display: flex;
  align-items: center;
  gap: 8px;
  padding: 6px 12px;
  background: var(--glass-bg);
  border-bottom: 1px solid var(--glass-border-subtle);
}

.preview-toggle {
  display: flex;
  align-items: center;
  gap: 4px;
  background: var(--glass-bg-light);
  border: 1px solid var(--glass-border-subtle);
  color: var(--text-muted);
  padding: 4px 10px;
  border-radius: var(--radius-md);
  font-size: 0.75rem;
  cursor: pointer;
  transition: all var(--transition-fast);
}

.preview-toggle:hover {
  color: var(--text-main);
  border-color: var(--glass-border);
}

.preview-toggle.active {
  color: var(--accent-blue);
  border-color: rgba(59, 130, 246, 0.3);
  background: rgba(59, 130, 246, 0.08);
}

.mode-switch {
  display: flex;
  gap: 2px;
  background: var(--glass-bg-light);
  border-radius: var(--radius-md);
  padding: 2px;
}

.mode-btn {
  background: transparent;
  border: none;
  color: var(--text-muted);
  padding: 3px 8px;
  border-radius: var(--radius-sm);
  font-size: 0.7rem;
  cursor: pointer;
  transition: all var(--transition-fast);
}

.mode-btn.active {
  background: var(--accent-blue);
  color: white;
}

.port-input {
  display: flex;
  align-items: center;
}

.port-field {
  width: 60px;
  background: var(--glass-bg-light);
  border: 1px solid var(--glass-border-subtle);
  color: var(--text-main);
  padding: 2px 6px;
  border-radius: var(--radius-sm);
  font-size: 0.75rem;
  font-family: var(--font-mono, monospace);
  text-align: center;
}

.auto-status {
  display: flex;
  align-items: center;
}

.status-badge {
  font-size: 0.68rem;
  padding: 2px 6px;
  border-radius: var(--radius-sm);
}

.status-badge.running {
  background: rgba(34, 197, 94, 0.15);
  color: var(--accent-green);
  border: 1px solid rgba(34, 197, 94, 0.3);
}

.status-badge.idle {
  background: rgba(156, 163, 175, 0.15);
  color: var(--text-muted);
  border: 1px solid rgba(156, 163, 175, 0.3);
}

.refresh-btn {
  background: transparent;
  border: none;
  color: var(--text-muted);
  cursor: pointer;
  padding: 4px;
  border-radius: var(--radius-md);
  display: flex;
  align-items: center;
  transition: all var(--transition-fast);
  margin-left: auto;
}

.refresh-btn:hover {
  color: var(--accent-blue);
  background: var(--glass-bg-light);
}

.preview-frame-wrapper {
  position: relative;
  height: 300px;
  background: white;
}

.preview-frame {
  width: 100%;
  height: 100%;
  border: none;
}

.preview-loading {
  position: absolute;
  top: 0;
  left: 0;
  right: 0;
  bottom: 0;
  display: flex;
  flex-direction: column;
  align-items: center;
  justify-content: center;
  gap: 8px;
  background: rgba(0, 0, 0, 0.5);
  color: white;
  font-size: 0.85rem;
  z-index: 1;
}

.spinner {
  width: 24px;
  height: 24px;
  border: 2px solid rgba(255, 255, 255, 0.3);
  border-top-color: white;
  border-radius: 50%;
  animation: spin 0.8s linear infinite;
}

.preview-error {
  position: absolute;
  top: 50%;
  left: 50%;
  transform: translate(-50%, -50%);
  padding: 12px 20px;
  background: rgba(239, 68, 68, 0.1);
  border: 1px solid rgba(239, 68, 68, 0.2);
  color: var(--accent-red);
  border-radius: var(--radius-md);
  font-size: 0.85rem;
  z-index: 2;
}

@keyframes spin {
  100% {
    transform: rotate(360deg);
  }
}
</style>
