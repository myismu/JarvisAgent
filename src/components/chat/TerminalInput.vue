<script setup lang="ts">
import { ref, onMounted, nextTick, onUnmounted, onBeforeUnmount, watch } from 'vue';
import { getCurrentWindow } from '@tauri-apps/api/window';
import { useSessionStore } from '../../stores/session';
import { useChatStore } from '../../stores/chat';
import { invoke } from '@tauri-apps/api/core';
import { listen, UnlistenFn } from '@tauri-apps/api/event';
import { readFile } from '@tauri-apps/plugin-fs';

const userInput = ref("");
const isDragging = ref(false);
const inputRef = ref<HTMLTextAreaElement | null>(null);
const mediaFiles = ref<{path: string, type: 'image' | 'video', url: string, base64: string}[]>([]);
const showVisionWarning = ref(false);

const session = useSessionStore();
const chat = useChatStore();

let unlistenDragDrop: (() => void) | null = null;
let unlistenConfig: UnlistenFn | null = null;

const appConfig = ref<any>(null);
const showProfileMenu = ref(false);
const isThinkingActive = ref(false);
const canModelThink = ref(true);
const canModelVision = ref(true);

const imageCompressConfig = ref({ maxWidth: 1920, maxHeight: 1080, quality: 0.8 });

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
    } catch (e) {
      console.error('Failed to switch profile:', e);
    }
  }
};

const closeMenuOnOutsideClick = (e: MouseEvent) => {
  const target = e.target as HTMLElement;
  if (!target.closest('.profile-selector')) {
    showProfileMenu.value = false;
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
  await loadImageCompressConfig();
  
  unlistenConfig = await listen('config-updated', () => {
    loadConfig();
    loadImageCompressConfig();
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
    if (!session.isCurrentSessionRunning) {
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
    const recalled = await chat.cancelJarvis();
    if (recalled) {
      userInput.value = recalled;
      nextTick(() => {
        inputRef.value?.focus();
        adjustHeight();
      });
    }
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
        <span>已取消生成，可撤回上一条消息重新编辑</span>
        <button class="recall-edit-btn" @click="handleRecallEdit">撤回并编辑</button>
        <button class="recall-dismiss-btn" @click="chat.dismissRecallEdit">✕</button>
      </div>

      <div class="input-toolbar">
        <div class="profile-selector">
          <button class="profile-btn" @click="showProfileMenu = !showProfileMenu">
            <span class="profile-icon">✨</span>
            {{ appConfig?.profiles.find((p: any) => p.id === appConfig?.activeProfileId)?.name || '选择模型' }}
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
        
        <div class="toolbar-right">
          <button 
            class="action-toggle-btn" 
            :class="{ 
              active: isThinkingActive, 
              disabled: !canModelThink 
            }" 
            @click="canModelThink && (isThinkingActive = !isThinkingActive)"
            :title="!canModelThink ? '该模型不支持深度思考模式' : (isThinkingActive ? '深度思考模式: 已开启' : '深度思考模式: 已关闭')"
          >
            <svg viewBox="0 0 24 24" width="14" height="14" stroke="currentColor" stroke-width="2" fill="none" stroke-linecap="round" stroke-linejoin="round">
              <path d="M9.59 4.59A2 2 0 1 1 11 8H2m10.59 11.41A2 2 0 1 0 14 16H2m15.73-8.27a5 5 0 1 1-7.14 7.14" />
            </svg>
            <span>{{ !canModelThink ? '不支持思考' : '深度思考' }}</span>
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
          <button class="remove-media-btn" @click.stop="removeMediaFile(index)" title="移除">✕</button>
        </div>
      </div>

      <div v-if="showVisionWarning" class="vision-warning">
        <svg viewBox="0 0 24 24" width="14" height="14" stroke="currentColor" stroke-width="2" fill="none" stroke-linecap="round" stroke-linejoin="round">
          <path d="M10.29 3.86L1.82 18a2 2 0 0 0 1.71 3h16.94a2 2 0 0 0 1.71-3L13.71 3.86a2 2 0 0 0-3.42 0z"></path>
          <line x1="12" y1="9" x2="12" y2="13"></line>
          <line x1="12" y1="17" x2="12.01" y2="17"></line>
        </svg>
        <span>当前模型不支持多模态，图片/视频将仅作为文件路径传递</span>
        <button class="warning-close-btn" @click="hideVisionWarning">✕</button>
      </div>

      <div class="input-row" @click="inputRef?.focus()">
        <textarea 
          ref="inputRef"
          v-model="userInput" 
          placeholder="给贾维斯发送指令... (Shift+Enter 换行)" 
          class="editor-input"
          autofocus
          rows="1"
          @input="handleInput"
          @keydown="handleKeydown"
        ></textarea>
        
        <button v-if="!session.isCurrentSessionRunning" class="send-btn" :class="{ active: userInput.trim() || mediaFiles.length > 0 }" @click="handleSubmit" title="发送 (Enter)">
          <svg viewBox="0 0 24 24" width="16" height="16" stroke="currentColor" stroke-width="2" fill="none" stroke-linecap="round" stroke-linejoin="round">
            <line x1="22" y1="2" x2="11" y2="13"></line>
            <polygon points="22 2 15 22 11 13 2 9 22 2"></polygon>
          </svg>
        </button>
        <button v-else class="send-btn active stop-state" @click="handleCancel" title="停止生成" :disabled="isCancelling">
          <svg v-if="!isCancelling" viewBox="0 0 24 24" width="16" height="16" fill="currentColor"><rect x="6" y="6" width="12" height="12" rx="2" /></svg>
          <svg v-else class="spinner-icon" viewBox="0 0 24 24" width="16" height="16" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round">
            <circle cx="12" cy="12" r="10"></circle>
            <path d="M12 2a10 10 0 0 1 10 10"></path>
          </svg>
        </button>
      </div>
      
    </div>
  </div>
</template>

<style scoped>
.chat-input-container {
  padding: 12px 24px 24px;
  background-color: transparent;
  display: flex;
  flex-direction: column;
  position: relative;
}

.chat-input-container::before {
  content: "";
  position: absolute;
  bottom: 0; left: 0; right: 0; top: -30px;
  background: linear-gradient(to top, var(--glass-bg-heavy) 40%, transparent);
  pointer-events: none;
  z-index: 0;
}

.chat-input-wrapper {
  background: var(--glass-bg-heavy);
  backdrop-filter: blur(var(--glass-blur-heavy));
  -webkit-backdrop-filter: blur(var(--glass-blur-heavy));
  border: 1px solid var(--glass-border);
  border-radius: var(--radius-xl);
  box-shadow: var(--glass-shadow);
  display: flex;
  flex-direction: column;
  transition: all var(--transition-normal);
  position: relative;
  z-index: 1;
}

.chat-input-wrapper:focus-within {
  border-color: var(--accent-blue);
  box-shadow: 0 0 0 3px rgba(59, 130, 246, 0.15), var(--glass-shadow);
  transform: translateY(-2px);
}

.input-toolbar {
  display: flex;
  justify-content: space-between;
  align-items: center;
  padding: 12px 16px 0;
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
  background: var(--glass-bg-heavy);
  backdrop-filter: blur(var(--glass-blur-heavy));
  -webkit-backdrop-filter: blur(var(--glass-blur-heavy));
  border: 1px solid var(--glass-border);
  border-radius: var(--radius-lg);
  box-shadow: var(--glass-shadow);
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
  border-bottom: 1px solid var(--glass-border-subtle);
  transition: background var(--transition-fast);
  border-radius: var(--radius-md);
  margin-bottom: 2px;
}

.profile-menu-item:last-child {
  border-bottom: none;
  margin-bottom: 0;
}

.profile-menu-item:hover {
  background: var(--glass-bg-light);
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

.stop-btn {
  display: inline-flex;
  align-items: center;
  gap: 4px;
  background: rgba(239, 68, 68, 0.1);
  color: var(--accent-red);
  border: 1px solid rgba(239, 68, 68, 0.2);
  border-radius: var(--radius-md);
  padding: 6px 12px;
  font-size: 0.75rem;
  font-weight: 600;
  cursor: pointer;
  transition: all var(--transition-fast);
}

.stop-btn:hover {
  background: var(--accent-red);
  color: white;
  transform: translateY(-1px);
  box-shadow: 0 4px 12px rgba(239, 68, 68, 0.2);
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

.media-icon {
  display: flex;
  align-items: center;
  justify-content: center;
  color: var(--text-muted);
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

.vision-warning svg {
  flex-shrink: 0;
}

.vision-warning span {
  flex: 1;
}

.warning-close-btn {
  background: transparent;
  border: none;
  color: var(--accent-yellow);
  cursor: pointer;
  padding: 2px 6px;
  border-radius: 4px;
  font-size: 12px;
}

.warning-close-btn:hover {
  background: rgba(245, 158, 11, 0.2);
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

.recall-edit-bar svg {
  flex-shrink: 0;
}

.recall-edit-bar span {
  flex: 1;
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
  background: transparent;
  border: none;
  color: var(--text-muted);
  cursor: pointer;
  padding: 2px 6px;
  border-radius: 4px;
  font-size: 12px;
  transition: all var(--transition-fast);
}

.recall-dismiss-btn:hover {
  background: rgba(100, 116, 139, 0.15);
  color: var(--text-main);
}
@keyframes spin {
  100% { transform: rotate(360deg); }
}

.spinner-icon {
  animation: spin 1s linear infinite;
  opacity: 0.7;
}
</style>
