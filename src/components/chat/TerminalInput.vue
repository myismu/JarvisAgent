<script setup lang="ts">
import { ref, onMounted, nextTick, onUnmounted, onBeforeUnmount } from 'vue';
import { getCurrentWindow } from '@tauri-apps/api/window';
import { useJarvis } from '../../composables/useJarvis';
import { invoke } from '@tauri-apps/api/core';

const userInput = ref("");
const isDragging = ref(false);
const inputRef = ref<HTMLTextAreaElement | null>(null);

const { sendToJarvis, systemStatus, cancelJarvis } = useJarvis();

let unlistenDragDrop: (() => void) | null = null;

// 模型切换逻辑
const appConfig = ref<any>(null);
const showProfileMenu = ref(false);

const loadConfig = async () => {
  try {
    appConfig.value = await invoke('get_config');
  } catch (e) {
    console.error('Failed to load config for input box:', e);
  }
};

const switchProfile = async (id: string) => {
  if (appConfig.value) {
    appConfig.value.activeProfileId = id;
    try {
      await invoke('save_config_cmd', { newConfig: appConfig.value });
      showProfileMenu.value = false;
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

onMounted(async () => {
  await loadConfig();
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
        const droppedPath = paths[0];
        if (userInput.value) {
          userInput.value += ` ${droppedPath}`;
        } else {
          userInput.value = droppedPath;
        }
        
        nextTick(() => {
          inputRef.value?.focus();
          adjustHeight();
        });
      }
    }
  });
});

onUnmounted(() => {
  if (unlistenDragDrop) unlistenDragDrop();
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
    handleSubmit();
  }
};

const handleSubmit = () => {
  const msg = userInput.value.trim();
  if (msg) {
    sendToJarvis(msg);
    userInput.value = '';
    nextTick(() => {
      adjustHeight();
    });
  }
};
</script>

<template>
  <div class="chat-input-container">
    <div class="chat-input-wrapper">
      
      <!-- 顶部工具栏 -->
      <div class="input-toolbar">
        <div class="profile-selector">
          <button class="profile-btn" @click="showProfileMenu = !showProfileMenu">
            <span class="profile-icon">✨</span>
            {{ appConfig?.profiles.find((p: any) => p.id === appConfig?.activeProfileId)?.name || '选择模型' }}
            <svg viewBox="0 0 24 24" width="12" height="12" stroke="currentColor" stroke-width="2" fill="none"><polyline points="6 9 12 15 18 9"></polyline></svg>
          </button>
          
          <!-- 模型下拉菜单 -->
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
        
        <button v-if="systemStatus === 'RUNNING'" class="stop-btn" @click="cancelJarvis" title="停止生成">
          <svg viewBox="0 0 24 24" width="12" height="12" fill="currentColor"><rect x="6" y="6" width="12" height="12" rx="2" /></svg>
          停止
        </button>
      </div>

      <!-- 输入区 -->
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
        
        <button class="send-btn" :class="{ active: userInput.trim() }" @click="handleSubmit" title="发送 (Enter)">
          <svg viewBox="0 0 24 24" width="16" height="16" stroke="currentColor" stroke-width="2" fill="none" stroke-linecap="round" stroke-linejoin="round">
            <line x1="22" y1="2" x2="11" y2="13"></line>
            <polygon points="22 2 15 22 11 13 2 9 22 2"></polygon>
          </svg>
        </button>
      </div>
      
    </div>
  </div>
</template>

<style scoped>
.chat-input-container {
  padding: 10px 24px 24px;
  background-color: transparent;
  display: flex;
  flex-direction: column;
  position: relative;
}

.chat-input-container::before {
  content: "";
  position: absolute;
  bottom: 0; left: 0; right: 0; top: -30px;
  background: linear-gradient(to top, var(--bg-panel) 50%, transparent);
  pointer-events: none;
  z-index: 0;
}

.chat-input-wrapper {
  background-color: var(--bg-sidebar);
  border: 1px solid var(--border-color);
  border-radius: 20px;
  box-shadow: 0 8px 24px rgba(0, 0, 0, 0.05), 0 2px 8px rgba(0, 0, 0, 0.02);
  display: flex;
  flex-direction: column;
  transition: all 0.3s cubic-bezier(0.2, 0.8, 0.2, 1);
  position: relative;
  z-index: 1;
}

.chat-input-wrapper:focus-within {
  border-color: var(--accent-blue);
  box-shadow: 0 10px 30px rgba(0, 102, 204, 0.1), 0 0 0 3px rgba(0, 102, 204, 0.05);
  transform: translateY(-2px);
}

.input-toolbar {
  display: flex;
  justify-content: space-between;
  align-items: center;
  padding: 10px 14px 0;
}

.profile-selector {
  position: relative;
}

.profile-btn {
  background: transparent;
  border: 1px solid transparent;
  border-radius: 8px;
  color: var(--text-main);
  padding: 6px 10px;
  font-size: 0.85rem;
  font-weight: 500;
  display: flex;
  align-items: center;
  gap: 6px;
  cursor: pointer;
  transition: all 0.2s cubic-bezier(0.2, 0.8, 0.2, 1);
}

.profile-btn:hover {
  background: rgba(128, 128, 128, 0.08);
  transform: translateY(-1px);
}

.profile-icon {
  font-size: 0.9rem;
}

.profile-menu {
  position: absolute;
  bottom: calc(100% + 8px);
  left: 0;
  background: var(--bg-panel);
  border: 1px solid var(--border-color);
  border-radius: 12px;
  box-shadow: 0 8px 30px rgba(0, 0, 0, 0.12);
  min-width: 220px;
  z-index: 100;
  overflow: hidden;
  animation: popIn 0.25s cubic-bezier(0.2, 0.8, 0.2, 1);
  padding: 6px;
}

@keyframes popIn {
  from { opacity: 0; transform: translateY(10px) scale(0.96); }
  to { opacity: 1; transform: translateY(0) scale(1); }
}

.profile-menu-item {
  padding: 10px 14px;
  cursor: pointer;
  border-bottom: 1px solid var(--border-color);
  transition: background 0.2s cubic-bezier(0.2, 0.8, 0.2, 1);
  border-radius: 6px;
  margin-bottom: 2px;
}

.profile-menu-item:last-child {
  border-bottom: none;
  margin-bottom: 0;
}

.profile-menu-item:hover {
  background: rgba(128, 128, 128, 0.08);
}

.profile-menu-item.active {
  background: rgba(0, 102, 204, 0.08);
  border-left: 3px solid var(--accent-blue);
}

.profile-menu-name {
  font-size: 0.85rem;
  font-weight: 600;
  color: var(--text-main);
  margin-bottom: 2px;
}

.profile-menu-model {
  font-size: 0.7rem;
  color: var(--text-muted);
  font-family: var(--font-mono);
  white-space: nowrap;
  overflow: hidden;
  text-overflow: ellipsis;
}

.stop-btn {
  display: inline-flex;
  align-items: center;
  gap: 4px;
  background: rgba(215, 58, 73, 0.1);
  color: var(--accent-red);
  border: 1px solid rgba(215, 58, 73, 0.2);
  border-radius: 8px;
  padding: 6px 12px;
  font-size: 0.75rem;
  font-weight: 600;
  cursor: pointer;
  transition: all 0.2s cubic-bezier(0.2, 0.8, 0.2, 1);
}

.stop-btn:hover {
  background: var(--accent-red);
  color: white;
  transform: translateY(-1px);
  box-shadow: 0 4px 12px rgba(215, 58, 73, 0.2);
}

.input-row {
  display: flex;
  align-items: flex-end;
  padding: 12px 14px 14px;
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
  opacity: 0.5;
}

.send-btn {
  background: var(--bg-panel);
  color: var(--text-muted);
  border: 1px solid var(--border-color);
  border-radius: 10px;
  width: 36px;
  height: 36px;
  display: flex;
  align-items: center;
  justify-content: center;
  margin-left: 12px;
  cursor: pointer;
  transition: all 0.25s cubic-bezier(0.2, 0.8, 0.2, 1);
  flex-shrink: 0;
}

.send-btn.active {
  background: linear-gradient(135deg, var(--accent-blue) 0%, #0052a3 100%);
  color: white;
  border-color: transparent;
  box-shadow: 0 4px 12px rgba(0, 102, 204, 0.3);
}

.send-btn.active:hover {
  transform: translateY(-2px) scale(1.05);
  box-shadow: 0 6px 16px rgba(0, 102, 204, 0.4);
}

.send-btn.active:active {
  transform: translateY(0);
}
</style>
