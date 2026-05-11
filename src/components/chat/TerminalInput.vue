<script setup lang="ts">
import { computed, ref, onMounted, nextTick, onUnmounted, onBeforeUnmount, watch } from 'vue';
import { useI18n } from 'vue-i18n';
import { getCurrentWindow } from '@tauri-apps/api/window';
import { useSessionStore } from '../../stores/session';
import { useChatStore } from '../../stores/chat';
import { usePreferences } from '../../composables/usePreferences';
import { invoke } from '@tauri-apps/api/core';
import { listen, UnlistenFn } from '@tauri-apps/api/event';
import { readFile } from '@tauri-apps/plugin-fs';
import type { AgentWorkMode } from '../../types';

const { t } = useI18n();

const userInput = ref("");
const isDragging = ref(false);
const inputRef = ref<HTMLTextAreaElement | null>(null);
const mediaFiles = ref<{path: string, type: 'image' | 'video', url: string, base64: string}[]>([]);
const showVisionWarning = ref(false);

const session = useSessionStore();
const chat = useChatStore();
const uiPrefs = usePreferences();

const isRunning = computed(() =>
  session.runningSessionId != null && session.runningSessionId === session.activeSessionId
);

const sessionTokenTotal = computed(() => (session.totalInputTokens || 0) + (session.totalOutputTokens || 0));

const formatToken = (n: number): string => {
  if (n >= 1_000_000) return `${(n / 1_000_000).toFixed(1)}M`;
  if (n >= 1_000) return `${(n / 1000).toFixed(1)}k`;
  return `${n}`;
};

// WorkMode 状态，与 usePreferences 双向同步 + 监听后端切换
const showWorkModeMenu = ref(false);
const currentWorkMode = ref<AgentWorkMode>(uiPrefs.agentWorkMode.value);

watch(() => uiPrefs.agentWorkMode.value, (val) => {
  currentWorkMode.value = val;
});

let unlistenDragDrop: (() => void) | null = null;
let unlistenConfig: UnlistenFn | null = null;
let unlistenWorkMode: UnlistenFn | null = null;

const appConfig = ref<any>(null);
const agentModel = computed(() => appConfig.value?.mainModel || '—');
const showProfileMenu = ref(false);
const isThinkingActive = ref(false);
const canModelThink = ref(true);
const canModelVision = ref(true);

const imageCompressConfig = ref({ maxWidth: 1920, maxHeight: 1080, quality: 0.8 });
const showInterruptedResumeHint = computed(() => {
  const view = session.currentSessionView;
  return !isRunning && (view.status === "INTERRUPTED" || Boolean(view.resumableRunId));
});

const loadImageCompressConfig = async () => {
  try {
    const cfg = await invoke<{ maxWidth: number; maxHeight: number; quality: number }>('get_image_compress_config');
    imageCompressConfig.value = cfg;
  } catch (e) {
    console.error('Failed to load image compress config:', e);
  }
};

const compressImage = async (fileData: Uint8Array, mimeType: string): Promise<{ base64: string; mimeType: string }> => {
  const { maxWidth, maxHeight, quality } = imageCompressConfig.value;
  const blob = new Blob([new Uint8Array(fileData)], { type: mimeType });
  const bitmap = await createImageBitmap(blob);
  
  let w = bitmap.width;
  let h = bitmap.height;
  
  if (w > maxWidth || h > maxHeight) {
    const ratio = Math.min(maxWidth / w, maxHeight / h);
    w = Math.round(w * ratio);
    h = Math.round(h * ratio);
  }
  
  const canvas = new OffscreenCanvas(w, h);
  const ctx = canvas.getContext('2d')!;
  ctx.drawImage(bitmap, 0, 0, w, h);
  bitmap.close();
  
  const outputType = mimeType === 'image/png' ? 'image/png' : 'image/jpeg';
  const outputBlob = await canvas.convertToBlob({ type: outputType, quality });
  const reader = new FileReader();
  
  return new Promise((resolve) => {
    reader.onload = () => {
      const dataUrl = reader.result as string;
      resolve({
        base64: dataUrl,
        mimeType: outputType,
      });
    };
    reader.readAsDataURL(outputBlob);
  });
};

const checkModelCapabilities = async (modelId: string) => {
  if (!modelId) return;
  try {
    const caps = await invoke<any>('get_model_capabilities', { modelId });
    if (caps) {
      canModelThink.value = caps.thinking;
      canModelVision.value = caps.vision ?? true;
      if (!caps.thinking) {
        isThinkingActive.value = false;
      }
    } else {
      canModelThink.value = true;
      canModelVision.value = true;
    }
  } catch (e) {
    console.error('Failed to check model capabilities:', e);
    canModelThink.value = true;
    canModelVision.value = true;
  }
};

const loadConfig = async () => {
  try {
    appConfig.value = await invoke('get_config');
    const activeProfile = appConfig.value.profiles.find((p: any) => p.id === appConfig.value.activeProfileId);
    if (activeProfile) {
      await checkModelCapabilities(activeProfile.config.mainModel);
    }
  } catch (e) {
    console.error('Failed to load config for input box:', e);
  }
};

const switchProfile = async (id: string) => {
  if (appConfig.value) {
    appConfig.value.activeProfileId = id;
    try {
      await invoke('save_config_cmd', { newConfig: appConfig.value });
      const activeSessionId = await invoke<string | null>('get_active_session_id');
      if (activeSessionId) {
        await invoke('update_session_profile', { id: activeSessionId, profileId: id });
      }
      showProfileMenu.value = false;
      const activeProfile = appConfig.value.profiles.find((p: any) => p.id === id);
      if (activeProfile) {
        await checkModelCapabilities(activeProfile.config.mainModel);
      }
      // 切模型后刷新历史，确保 data-user-message-index 同步
      try {
        if (session.activeSessionId) {
          const history = await invoke<string>('get_session_history', { sessionId: session.activeSessionId });
          session.clearSessionBuffers(session.activeSessionId);
          session.replaceSessionHistory(session.activeSessionId, history);
        }
      } catch { /* ignore */ }
    } catch (e) {
      console.error('Failed to switch profile:', e);
    }
  }
};

const switchWorkMode = (mode: AgentWorkMode) => {
  uiPrefs.setAgentWorkMode(mode);
  currentWorkMode.value = mode;
  showWorkModeMenu.value = false;
};

const closeMenuOnOutsideClick = (e: MouseEvent) => {
  const target = e.target as HTMLElement;
  if (!target.closest('.profile-selector')) {
    showProfileMenu.value = false;
  }
  if (!target.closest('.work-mode-selector')) {
    showWorkModeMenu.value = false;
  }
};

const hideVisionWarning = () => {
  showVisionWarning.value = false;
};

const processDroppedFiles = async (paths: string[]) => {
  let hasMediaFiles = false;
  
  for (const droppedPath of paths) {
    const lowerPath = droppedPath.toLowerCase();
    const isImage = lowerPath.endsWith('.jpg') || lowerPath.endsWith('.jpeg') || lowerPath.endsWith('.png') || lowerPath.endsWith('.gif') || lowerPath.endsWith('.webp');
    const isVideo = lowerPath.endsWith('.mp4') || lowerPath.endsWith('.webm') || lowerPath.endsWith('.mov');
    
    if (isImage || isVideo) {
      hasMediaFiles = true;
      
      let url = '';
      let base64 = '';
      
      if (isImage && canModelVision.value) {
        try {
          const fileData = await readFile(droppedPath);
          const ext = lowerPath.split('.').pop() || 'png';
          const mimeType = ext === 'jpg' || ext === 'jpeg' ? 'image/jpeg' : 
                           ext === 'gif' ? 'image/gif' : 
                           ext === 'webp' ? 'image/webp' : 'image/png';
          
          const compressed = await compressImage(new Uint8Array(fileData), mimeType);
          base64 = compressed.base64;
          url = URL.createObjectURL(new Blob([new Uint8Array(fileData)], { type: mimeType }));
        } catch (e) {
          console.error('Failed to read image file:', e);
        }
      }
      
      mediaFiles.value.push({
        path: droppedPath,
        type: isImage ? 'image' : 'video',
        url,
        base64,
      });
    } else {
      if (userInput.value) {
        userInput.value += ` ${droppedPath}`;
      } else {
        userInput.value = droppedPath;
      }
    }
  }
  
  if (hasMediaFiles && !canModelVision.value) {
    showVisionWarning.value = true;
  }
  
  nextTick(() => {
    inputRef.value?.focus();
    adjustHeight();
  });
};

onMounted(async () => {
  await loadConfig();
  // 如果没有活跃会话，将模型初始化为全局默认
  if (!session.activeSessionId && appConfig.value?.globalProfileId) {
    appConfig.value.activeProfileId = appConfig.value.globalProfileId;
    await invoke('save_config_cmd', { newConfig: appConfig.value });
  }
  await loadImageCompressConfig();

  unlistenConfig = await listen('config-updated', async () => {
    loadConfig();
    loadImageCompressConfig();
    try {
      if (session.activeSessionId) {
        const history = await invoke<string>('get_session_history', { sessionId: session.activeSessionId });
        session.clearSessionBuffers(session.activeSessionId);
        session.replaceSessionHistory(session.activeSessionId, history);
      }
    } catch { /* ignore */ }
  });

  // 监听 Agent 自动切换 WorkMode 的事件
  unlistenWorkMode = await listen<{ from: string; to: string; reason: string }>('agent-work-mode-changed', (event) => {
    currentWorkMode.value = event.payload.to as AgentWorkMode;
  });

  document.addEventListener('click', closeMenuOnOutsideClick);

  const appWindow = getCurrentWindow();
  unlistenDragDrop = await appWindow.onDragDropEvent((event) => {
    if (event.payload.type === 'enter') {
      isDragging.value = true;
      document.body.classList.add('dragging-active');
    } else if (event.payload.type === 'leave' || event.payload.type === 'drop') {
      isDragging.value = false;
      document.body.classList.remove('dragging-active');
    }
    
    if (event.payload.type === 'drop') {
      const paths = event.payload.paths;
      if (paths.length > 0) {
        processDroppedFiles(paths);
      }
    }
  });
});

onUnmounted(() => {
  if (unlistenDragDrop) unlistenDragDrop();
  if (unlistenConfig) unlistenConfig();
  if (unlistenWorkMode) unlistenWorkMode();
});

onBeforeUnmount(() => {
  document.removeEventListener('click', closeMenuOnOutsideClick);
});

const adjustHeight = () => {
  const el = inputRef.value;
  if (!el) return;
  el.style.height = 'auto';
  const newHeight = Math.min(el.scrollHeight, 200);
  el.style.height = `${newHeight}px`;
};

const handleInput = () => {
  adjustHeight();
};

const handleKeydown = (e: KeyboardEvent) => {
  if (e.key === 'Enter' && !e.shiftKey) {
    e.preventDefault();
    if (!isRunning.value) {
      handleSubmit();
    }
  }
};

const handleSubmit = () => {
  const msg = userInput.value.trim();
  if (msg || mediaFiles.value.length > 0) {
    const imageBase64List = mediaFiles.value
      .filter(m => m.type === 'image' && m.base64)
      .map(m => m.base64);
    chat.sendToJarvis(msg, isThinkingActive.value, imageBase64List);
    userInput.value = '';
    mediaFiles.value.forEach(m => {
      if (m.url) URL.revokeObjectURL(m.url);
    });
    mediaFiles.value = [];
    showVisionWarning.value = false;
    nextTick(() => {
      adjustHeight();
    });
  }
};

const isCancelling = ref(false);

const handleCancel = async () => {
  if (isCancelling.value) return;
  isCancelling.value = true;
  try {
    await chat.cancelJarvis();
  } finally {
    isCancelling.value = false;
  }
};

const removeMediaFile = (index: number) => {
  const [removed] = mediaFiles.value.splice(index, 1);
  if (removed?.url) {
    URL.revokeObjectURL(removed.url);
  }
  if (mediaFiles.value.length === 0) {
    showVisionWarning.value = false;
  }
};

watch(() => chat.rollbackRecalledMessage, (msg) => {
  if (msg) {
    userInput.value = msg;
    chat.rollbackRecalledMessage = "";
    nextTick(() => {
      inputRef.value?.focus();
      adjustHeight();
    });
  }
});

const handleRecallEdit = async () => {
  const text = await chat.recallAndEdit();
  if (text) {
    userInput.value = text;
    nextTick(() => {
      inputRef.value?.focus();
      adjustHeight();
    });
  }
};
</script>

<template>
  <div class="chat-input-container">
    <div class="chat-input-wrapper">
      
      <div v-if="chat.showRecallEdit" class="recall-edit-bar">
        <svg viewBox="0 0 24 24" width="14" height="14" stroke="currentColor" stroke-width="2" fill="none" stroke-linecap="round" stroke-linejoin="round">
          <polyline points="1 4 1 10 7 10"></polyline>
          <path d="M3.51 15a9 9 0 1 0 2.13-9.36L1 10"></path>
        </svg>
        <span>{{ t('input.recallHint') }}</span>
        <button class="recall-edit-btn" @click="handleRecallEdit">{{ t('input.recallEdit') }}</button>
        <button class="recall-dismiss-btn" @click="chat.dismissRecallEdit">✕</button>
      </div>

      <div v-if="showInterruptedResumeHint" class="resume-run-bar">
        <svg viewBox="0 0 24 24" width="14" height="14" stroke="currentColor" stroke-width="2" fill="none" stroke-linecap="round" stroke-linejoin="round">
          <path d="M3 12a9 9 0 0 1 15.5-6.2"></path>
          <polyline points="18 2 18 6 14 6"></polyline>
          <path d="M21 12a9 9 0 0 1-15.5 6.2"></path>
          <polyline points="6 22 6 18 10 18"></polyline>
        </svg>
        <span>{{ t('input.resumeHint') }}</span>
      </div>

      <div class="input-toolbar">
        <div class="profile-selector">
          <button class="profile-btn" @click="showProfileMenu = !showProfileMenu">
            <span class="profile-icon">✨</span>
            {{ appConfig?.profiles.find((p: any) => p.id === appConfig?.activeProfileId)?.name || t('input.selectModel') }}
            <svg viewBox="0 0 24 24" width="12" height="12" stroke="currentColor" stroke-width="2" fill="none"><polyline points="6 9 12 15 18 9"></polyline></svg>
          </button>
          
          <div v-if="showProfileMenu" class="profile-menu">
            <div 
              v-for="profile in appConfig?.profiles" 
              :key="profile.id"
              class="profile-menu-item"
              :class="{ active: appConfig?.activeProfileId === profile.id }"
              @click="switchProfile(profile.id)"
            >
              <div class="profile-menu-name">{{ profile.name }}</div>
              <div class="profile-menu-model">{{ profile.config.mainModel }}</div>
            </div>
          </div>
        </div>
        
        <div class="toolbar-spacer"></div>

        <div class="work-mode-selector">
          <button class="work-mode-btn" @click="showWorkModeMenu = !showWorkModeMenu">
            <span class="work-mode-dot" :class="currentWorkMode"></span>
            <span class="work-mode-label">{{ t('settings.general.' + currentWorkMode) }}</span>
            <svg viewBox="0 0 24 24" width="10" height="10" stroke="currentColor" stroke-width="2" fill="none"><polyline points="6 9 12 15 18 9"></polyline></svg>
          </button>

          <div v-if="showWorkModeMenu" class="work-mode-menu">
            <div class="work-mode-menu-inner">
            <div
              v-for="mode in (['chat', 'edit', 'plan'] as AgentWorkMode[])"
              :key="mode"
              class="work-mode-menu-item"
              :class="{ active: currentWorkMode === mode }"
              @click="switchWorkMode(mode)"
            >
              <span class="work-mode-dot" :class="mode"></span>
              <div class="work-mode-menu-text">
                <div class="work-mode-menu-name">{{ t('settings.general.' + mode) }}</div>
                <div class="work-mode-menu-desc">{{ t('settings.general.' + mode + 'DescShort') }}</div>
              </div>
            </div>
            </div>
          </div>
        </div>

        <div class="toolbar-right">
          <button
            class="action-toggle-btn"
            :class="{
              active: isThinkingActive,
              disabled: !canModelThink
            }"
            @click="canModelThink && (isThinkingActive = !isThinkingActive)"
            :title="!canModelThink ? t('input.thinkingUnsupportedTitle') : (isThinkingActive ? t('input.thinkingOnTitle') : t('input.thinkingOffTitle'))"
          >
            <svg viewBox="0 0 24 24" width="14" height="14" stroke="currentColor" stroke-width="2" fill="none" stroke-linecap="round" stroke-linejoin="round">
              <path d="M9.59 4.59A2 2 0 1 1 11 8H2m10.59 11.41A2 2 0 1 0 14 16H2m15.73-8.27a5 5 0 1 1-7.14 7.14" />
            </svg>
            <span>{{ !canModelThink ? t('input.thinkingUnsupported') : t('input.thinking') }}</span>
          </button>
        </div>
      </div>

      <div v-if="mediaFiles.length > 0" class="media-preview-container">
        <div v-for="(media, index) in mediaFiles" :key="index" class="media-preview-item">
          <template v-if="media.type === 'image' && media.url">
            <img :src="media.url" class="media-thumbnail" alt="preview" />
          </template>
          <template v-else>
            <div class="media-icon">
              <svg v-if="media.type === 'image'" viewBox="0 0 24 24" width="24" height="24" stroke="currentColor" stroke-width="2" fill="none" stroke-linecap="round" stroke-linejoin="round">
                <rect x="3" y="3" width="18" height="18" rx="2" ry="2"></rect>
                <circle cx="8.5" cy="8.5" r="1.5"></circle>
                <polyline points="21 15 16 10 5 21"></polyline>
              </svg>
              <svg v-else viewBox="0 0 24 24" width="24" height="24" stroke="currentColor" stroke-width="2" fill="none" stroke-linecap="round" stroke-linejoin="round">
                <polygon points="23 7 16 12 23 17 23 7"></polygon>
                <rect x="1" y="5" width="15" height="14" rx="2" ry="2"></rect>
              </svg>
            </div>
          </template>
          <span class="media-name">{{ media.path.split(/[/\\]/).pop() }}</span>
          <button class="remove-media-btn" @click.stop="removeMediaFile(index)" :title="t('input.remove')">✕</button>
        </div>
      </div>

      <div v-if="showVisionWarning" class="vision-warning">
        <svg viewBox="0 0 24 24" width="14" height="14" stroke="currentColor" stroke-width="2" fill="none" stroke-linecap="round" stroke-linejoin="round">
          <path d="M10.29 3.86L1.82 18a2 2 0 0 0 1.71 3h16.94a2 2 0 0 0 1.71-3L13.71 3.86a2 2 0 0 0-3.42 0z"></path>
          <line x1="12" y1="9" x2="12" y2="13"></line>
          <line x1="12" y1="17" x2="12.01" y2="17"></line>
        </svg>
        <span>{{ t('input.visionWarning') }}</span>
        <button class="warning-close-btn" @click="hideVisionWarning">✕</button>
      </div>

      <div class="input-row" @click="inputRef?.focus()">
        <textarea 
          ref="inputRef"
          v-model="userInput" 
          :placeholder="t('input.placeholder')"
          class="editor-input"
          autofocus
          rows="1"
          @input="handleInput"
          @keydown="handleKeydown"
        ></textarea>
        
        <button v-if="!isRunning" class="send-btn" :class="{ active: userInput.trim() || mediaFiles.length > 0 }" @click="handleSubmit" :title="t('input.send')">
          <svg viewBox="0 0 24 24" width="16" height="16" stroke="currentColor" stroke-width="2" fill="none" stroke-linecap="round" stroke-linejoin="round">
            <line x1="22" y1="2" x2="11" y2="13"></line>
            <polygon points="22 2 15 22 11 13 2 9 22 2"></polygon>
          </svg>
        </button>
        <button v-else class="send-btn active stop-state" @click="handleCancel" :title="t('input.stop')" :disabled="isCancelling">
          <svg v-if="!isCancelling" viewBox="0 0 24 24" width="16" height="16" fill="currentColor"><rect x="6" y="6" width="12" height="12" rx="2" /></svg>
          <svg v-else class="spinner-icon" viewBox="0 0 24 24" width="16" height="16" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round">
            <circle cx="12" cy="12" r="10"></circle>
            <path d="M12 2a10 10 0 0 1 10 10"></path>
          </svg>
        </button>
      </div>

      <!-- Token 使用统计 -->
      <div class="token-bar" v-if="sessionTokenTotal > 0">
        <span class="token-bar-item" title="累计输入 Token">
          <svg viewBox="0 0 24 24" width="12" height="12" stroke="currentColor" stroke-width="2" fill="none"><polyline points="15 14 20 9 15 4"></polyline><path d="M4 20v-7a4 4 0 0 1 4-4h12"></path></svg>
          {{ formatToken(session.totalInputTokens) }}
        </span>
        <span class="token-bar-sep">·</span>
        <span class="token-bar-item" title="累计输出 Token">
          <svg viewBox="0 0 24 24" width="12" height="12" stroke="currentColor" stroke-width="2" fill="none"><polyline points="4 17 9 12 4 7"></polyline><path d="M12 20v-7a4 4 0 0 1 4-4h4"></path></svg>
          {{ formatToken(session.totalOutputTokens) }}
        </span>
        <span class="token-bar-sep">·</span>
        <span class="token-bar-item token-bar-total" title="累计总消耗">
          {{ formatToken(sessionTokenTotal) }}
        </span>
        <span class="token-bar-spacer"></span>
        <span class="token-bar-item token-bar-model">{{ agentModel }}</span>
      </div>

    </div>
  </div>
</template>

<style scoped>
.chat-input-container {
  padding: 0;
  background-color: transparent;
  display: flex;
  flex-direction: column;
  position: relative;
  width: 100%;
  align-items: center; /* 居中核心 */
}

.chat-input-container::before {
  display: none;
}

.chat-input-wrapper {
  width: 100%;
  max-width: 1000px;
  background: var(--surface-strong);
  backdrop-filter: blur(var(--glass-blur-heavy));
  -webkit-backdrop-filter: blur(var(--glass-blur-heavy));
  border: 1px solid var(--glass-border);
  border-radius: 24px; /* 增加圆角度，使其更圆润 */
  box-shadow: 0 20px 50px rgba(0, 0, 0, 0.15), var(--glass-shadow);
  display: flex;
  flex-direction: column;
  transition: all var(--transition-normal);
  position: relative;
  z-index: 1;
}

.chat-input-wrapper:hover {
  border-color: color-mix(in srgb, var(--accent-blue) 40%, var(--glass-border));
  box-shadow: 0 25px 60px rgba(0, 0, 0, 0.2), var(--glass-shadow);
}

.chat-input-wrapper:focus-within {
  border-color: var(--accent-blue);
  box-shadow: 0 0 0 3px rgba(59, 130, 246, 0.2), 0 30px 70px rgba(0, 0, 0, 0.25);
}

.input-toolbar {
  display: flex;
  justify-content: space-between;
  align-items: center;
  padding: 12px 16px 0;
  flex-wrap: wrap;
  gap: 8px;
}

.profile-selector {
  position: relative;
}

.profile-btn {
  background: var(--glass-bg-light);
  border: 1px solid var(--glass-border-subtle);
  border-radius: var(--radius-md);
  color: var(--text-main);
  padding: 6px 12px;
  font-size: 0.85rem;
  font-weight: 600;
  display: flex;
  align-items: center;
  gap: 6px;
  cursor: pointer;
  transition: all var(--transition-fast);
  backdrop-filter: blur(8px);
  -webkit-backdrop-filter: blur(8px);
}

.profile-btn:hover {
  background: var(--glass-bg);
  border-color: var(--glass-border);
  transform: translateY(-1px);
}

.profile-icon {
  font-size: 0.9rem;
}

.profile-menu {
  position: absolute;
  bottom: calc(100% + 12px);
  left: 0;
  background: var(--surface-strong);
  backdrop-filter: blur(var(--glass-blur-heavy));
  -webkit-backdrop-filter: blur(var(--glass-blur-heavy));
  border: 1px solid color-mix(in srgb, var(--text-muted) 22%, transparent);
  border-radius: var(--radius-lg);
  box-shadow: 0 18px 45px rgba(15, 23, 42, 0.18), var(--glass-shadow);
  min-width: 240px;
  z-index: 100;
  overflow: hidden;
  animation: popIn var(--transition-fast);
  padding: 8px;
}

@keyframes popIn {
  from { opacity: 0; transform: translateY(8px) scale(0.96); }
  to { opacity: 1; transform: translateY(0) scale(1); }
}

.profile-menu-item {
  padding: 10px 14px;
  cursor: pointer;
  border-bottom: 1px solid color-mix(in srgb, var(--text-muted) 12%, transparent);
  transition: background var(--transition-fast);
  border-radius: var(--radius-md);
  margin-bottom: 2px;
}

.profile-menu-item:last-child {
  border-bottom: none;
  margin-bottom: 0;
}

.profile-menu-item:hover {
  background: color-mix(in srgb, var(--accent-blue) 10%, transparent);
}

.profile-menu-item.active {
  background: rgba(59, 130, 246, 0.12);
  border-left: 3px solid var(--accent-blue);
}

.profile-menu-name {
  font-size: 0.85rem;
  font-weight: 600;
  color: var(--text-main);
  margin-bottom: 4px;
}

.profile-menu-model {
  font-size: 0.75rem;
  color: var(--text-muted);
  font-family: var(--font-mono);
  white-space: nowrap;
  overflow: hidden;
  text-overflow: ellipsis;
}

.toolbar-right {
  display: flex;
  align-items: center;
  gap: 8px;
}

.action-toggle-btn {
  display: inline-flex;
  align-items: center;
  gap: 6px;
  background: var(--glass-bg-light);
  color: var(--text-muted);
  border: 1px solid var(--glass-border-subtle);
  border-radius: var(--radius-md);
  padding: 6px 12px;
  font-size: 0.75rem;
  font-weight: 600;
  cursor: pointer;
  transition: all var(--transition-fast);
  backdrop-filter: blur(8px);
  -webkit-backdrop-filter: blur(8px);
}

.action-toggle-btn:hover {
  background: var(--glass-bg);
  color: var(--text-main);
  border-color: var(--glass-border);
}

.action-toggle-btn.active {
  background: rgba(59, 130, 246, 0.12);
  color: var(--accent-blue);
  border-color: rgba(59, 130, 246, 0.35);
  box-shadow: 0 0 12px rgba(59, 130, 246, 0.1);
}

.action-toggle-btn.disabled {
  opacity: 0.5;
  cursor: not-allowed;
  filter: grayscale(1);
}

.toolbar-spacer {
  flex: 1;
}

.work-mode-selector {
  position: relative;
}

.work-mode-btn {
  background: var(--glass-bg-light);
  border: 1px solid var(--glass-border-subtle);
  border-radius: var(--radius-md);
  color: var(--text-main);
  padding: 6px 12px;
  font-size: 0.8rem;
  font-weight: 600;
  display: flex;
  align-items: center;
  gap: 6px;
  cursor: pointer;
  transition: background var(--transition-fast), border-color var(--transition-fast), transform var(--transition-fast), color var(--transition-fast), box-shadow var(--transition-fast);
  backdrop-filter: blur(8px);
  -webkit-backdrop-filter: blur(8px);
}

.work-mode-btn:hover {
  background: var(--glass-bg);
  border-color: var(--glass-border);
  transform: translateY(-1px);
}

.work-mode-dot {
  width: 8px;
  height: 8px;
  border-radius: 50%;
  flex-shrink: 0;
}

.work-mode-dot.chat { background: var(--accent-green, #22c55e); }
.work-mode-dot.edit { background: var(--accent-blue, #3b82f6); }
.work-mode-dot.plan { background: var(--accent-yellow, #f59e0b); }

.work-mode-label {
  white-space: nowrap;
}

.work-mode-menu {
  position: absolute;
  bottom: calc(100% + 12px);
  left: 50%;
  transform: translateX(-50%);
  z-index: 100;
  min-width: 200px;
}

.work-mode-menu-inner {
  background: var(--surface-strong);
  backdrop-filter: blur(var(--glass-blur-heavy));
  -webkit-backdrop-filter: blur(var(--glass-blur-heavy));
  border: 1px solid color-mix(in srgb, var(--text-muted) 22%, transparent);
  border-radius: var(--radius-lg);
  box-shadow: 0 18px 45px rgba(15, 23, 42, 0.18), var(--glass-shadow);
  overflow: hidden;
  animation: popIn var(--transition-fast);
  padding: 8px;
}

.work-mode-menu-item {
  display: flex;
  align-items: center;
  gap: 10px;
  padding: 10px 14px;
  cursor: pointer;
  transition: background var(--transition-fast);
  border-radius: var(--radius-md);
  margin-bottom: 2px;
}

.work-mode-menu-item:last-child {
  margin-bottom: 0;
}

.work-mode-menu-item:hover {
  background: color-mix(in srgb, var(--accent-blue) 10%, transparent);
}

.work-mode-menu-item.active {
  background: rgba(59, 130, 246, 0.12);
}

.work-mode-menu-text {
  display: flex;
  flex-direction: column;
}

.work-mode-menu-name {
  font-size: 0.85rem;
  font-weight: 600;
  color: var(--text-main);
}

.work-mode-menu-desc {
  font-size: 0.7rem;
  color: var(--text-muted);
  margin-top: 2px;
}

.input-row {
  display: flex;
  align-items: flex-end;
  padding: 14px 16px 16px;
  cursor: text;
}

.editor-input {
  flex: 1;
  background: transparent;
  border: none;
  color: var(--text-main);
  font-family: var(--font-mono);
  font-size: 0.95rem;
  outline: none;
  resize: none;
  overflow-y: auto;
  line-height: 1.6;
  padding: 0;
  margin: 0;
  max-height: 200px;
}

.editor-input::placeholder { 
  color: var(--text-muted); 
  opacity: 0.6;
}

.send-btn {
  background: var(--glass-bg);
  color: var(--text-muted);
  border: 1px solid var(--glass-border);
  border-radius: var(--radius-md);
  width: 36px;
  height: 36px;
  display: flex;
  align-items: center;
  justify-content: center;
  margin-left: 12px;
  cursor: pointer;
  transition: all var(--transition-fast);
  flex-shrink: 0;
  backdrop-filter: blur(8px);
  -webkit-backdrop-filter: blur(8px);
}

.send-btn.active {
  background: linear-gradient(135deg, var(--accent-blue) 0%, var(--accent-blue-hover) 100%);
  color: white;
  border-color: transparent;
  box-shadow: 0 4px 12px rgba(59, 130, 246, 0.25);
}

.send-btn.active:hover {
  transform: translateY(-2px) scale(1.05);
  box-shadow: 0 6px 16px rgba(59, 130, 246, 0.35);
}

.send-btn.active:active {
  transform: translateY(0);
}

.send-btn.stop-state {
  background: rgba(239, 68, 68, 0.12);
  color: var(--accent-red);
  border-color: rgba(239, 68, 68, 0.25);
  box-shadow: none;
}

.send-btn.stop-state:hover {
  background: var(--accent-red);
  color: white;
  transform: translateY(-2px) scale(1.05);
  box-shadow: 0 4px 12px rgba(239, 68, 68, 0.2);
}

.media-preview-container {
  display: flex;
  flex-wrap: wrap;
  gap: 8px;
  padding: 12px 16px 0;
}

.media-preview-item {
  position: relative;
  display: flex;
  align-items: center;
  gap: 8px;
  padding: 6px 10px;
  border-radius: var(--radius-md);
  background: var(--glass-bg-light);
  border: 1px solid var(--glass-border-subtle);
  max-width: 200px;
  backdrop-filter: blur(8px);
  -webkit-backdrop-filter: blur(8px);
}

.media-thumbnail {
  width: 40px;
  height: 40px;
  object-fit: cover;
  border-radius: var(--radius-sm);
  flex-shrink: 0;
}

.media-name {
  font-size: 0.75rem;
  color: var(--text-main);
  overflow: hidden;
  text-overflow: ellipsis;
  white-space: nowrap;
}

.remove-media-btn {
  width: 16px;
  height: 16px;
  background: rgba(0, 0, 0, 0.15);
  color: var(--text-muted);
  border: none;
  border-radius: 50%;
  font-size: 10px;
  display: flex;
  align-items: center;
  justify-content: center;
  cursor: pointer;
  transition: all var(--transition-fast);
  flex-shrink: 0;
}

.remove-media-btn:hover {
  background: rgba(239, 68, 68, 0.9);
  color: white;
}

.vision-warning {
  display: flex;
  align-items: center;
  gap: 8px;
  padding: 8px 16px;
  background: rgba(245, 158, 11, 0.1);
  border-top: 1px solid rgba(245, 158, 11, 0.2);
  color: var(--accent-yellow);
  font-size: 0.8rem;
}

.recall-edit-bar {
  display: flex;
  align-items: center;
  gap: 8px;
  padding: 8px 16px;
  background: rgba(59, 130, 246, 0.08);
  border-bottom: 1px solid rgba(59, 130, 246, 0.15);
  color: var(--accent-blue);
  font-size: 0.8rem;
  animation: slideDown 0.2s ease-out;
}

@keyframes slideDown {
  from { opacity: 0; transform: translateY(-4px); }
  to { opacity: 1; transform: translateY(0); }
}

.recall-edit-btn {
  background: rgba(59, 130, 246, 0.15);
  color: var(--accent-blue);
  border: 1px solid rgba(59, 130, 246, 0.3);
  border-radius: var(--radius-md);
  padding: 4px 12px;
  font-size: 0.75rem;
  font-weight: 600;
  cursor: pointer;
  transition: all var(--transition-fast);
  white-space: nowrap;
}

.recall-edit-btn:hover {
  background: var(--accent-blue);
  color: white;
  border-color: transparent;
  transform: translateY(-1px);
}

.recall-dismiss-btn {
  background: none;
  border: none;
  color: var(--accent-blue);
  opacity: 0.6;
  cursor: pointer;
  font-size: 0.9rem;
  line-height: 1;
  padding: 2px 6px;
  border-radius: var(--radius-sm);
  transition: all var(--transition-fast);
  display: flex;
  align-items: center;
  justify-content: center;
}

.recall-dismiss-btn:hover {
  opacity: 1;
  background: rgba(59, 130, 246, 0.15);
}

.resume-run-bar {
  display: flex;
  align-items: center;
  gap: 8px;
  padding: 8px 16px;
  background: rgba(245, 158, 11, 0.1);
  border-bottom: 1px solid rgba(245, 158, 11, 0.18);
  color: var(--accent-yellow);
  font-size: 0.8rem;
  animation: slideDown 0.2s ease-out;
}

@keyframes spin {
  100% { transform: rotate(360deg); }
}

.spinner-icon {
  animation: spin 1s linear infinite;
  opacity: 0.7;
}

/* ── Token 统计栏 ── */
.token-bar {
  display: flex;
  align-items: center;
  gap: 6px;
  width: 100%;
  max-width: 1000px;
  padding: 4px 16px 0;
  font-size: 0.65rem;
  color: var(--text-muted);
  user-select: none;
}

.token-bar-item {
  display: inline-flex;
  align-items: center;
  gap: 3px;
  white-space: nowrap;
}

.token-bar-item svg {
  opacity: 0.5;
}

.token-bar-sep {
  opacity: 0.35;
}

.token-bar-total {
  font-weight: 650;
}

.token-bar-spacer {
  flex: 1;
}

.token-bar-model {
  font-style: italic;
  font-size: 0.6rem;
  opacity: 0.6;
}
</style>