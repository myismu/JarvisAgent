<script setup lang="ts">
import { ref, onMounted, onUnmounted } from 'vue';
import { invoke } from '@tauri-apps/api/core';
import { listen } from '@tauri-apps/api/event';
import { useJarvis, triggerRender } from '../../composables/useJarvis';

const { todos, jarvisResponse, toolBuffer, contentBuffer, tempBuffer } = useJarvis();

// 会话管理状态
interface SessionMeta {
  id: string;
  title: string;
  createdAt: number;
  updatedAt: number;
  messageCount: number;
}

const sessions = ref<SessionMeta[]>([]);
const activeSessionId = ref<string | null>(null);

// 加载会话列表
const loadSessions = async () => {
  try {
    sessions.value = await invoke<SessionMeta[]>('list_sessions');
  } catch (err) {
    console.error('加载会话列表失败:', err);
  }
};

// 创建新会话
const createNewSession = async () => {
  try {
    const meta = await invoke<SessionMeta>('create_session');
    activeSessionId.value = meta.id;
    // 清空所有前端状态
    jarvisResponse.value = 'Ready for input...';
    toolBuffer.value = '';
    contentBuffer.value = '';
    tempBuffer.value = '';
    triggerRender(); // 立即刷新，清除上一轮残留渲染
    await loadSessions();
  } catch (err) {
    console.error('创建会话失败:', err);
  }
};

// 切换会话
const switchToSession = async (id: string) => {
  if (id === activeSessionId.value) return;
  try {
    await invoke<SessionMeta>('switch_session', { id });
    activeSessionId.value = id;
    // 清空流式状态
    toolBuffer.value = '';
    contentBuffer.value = '';
    tempBuffer.value = '';
    triggerRender(); // 立即刷新，清除上一轮残留渲染
    // 从后端获取当前会话的可渲染历史文本
    const history = await invoke<string>('get_session_history');
    if (history && history.trim()) {
      jarvisResponse.value = history;
    } else {
      jarvisResponse.value = 'Ready for input...';
    }
    await loadSessions();
  } catch (err) {
    console.error('切换会话失败:', err);
  }
};

// 删除会话
const deleteSession = async (id: string, event: Event) => {
  event.stopPropagation();
  if (id === activeSessionId.value) return;
  try {
    await invoke('delete_session', { id });
    await loadSessions();
  } catch (err) {
    console.error('删除会话失败:', err);
  }
};

let unlistenRenamed: (() => void) | null = null;
let unlistenUpdated: (() => void) | null = null;

onMounted(async () => {
  await loadSessions();
  
  try {
    const activeId = await invoke<string | null>('get_active_session_id');
    if (activeId) {
      activeSessionId.value = activeId;
    } else if (sessions.value.length > 0) {
      activeSessionId.value = sessions.value[0].id;
    }
  } catch (err) {
    if (sessions.value.length > 0) {
      activeSessionId.value = sessions.value[0].id;
    }
  }
  
  unlistenRenamed = await listen('session-renamed', () => {
    loadSessions();
  });
  
  unlistenUpdated = await listen('session-updated', () => {
    loadSessions();
  });
});

onUnmounted(() => {
  if (unlistenRenamed) unlistenRenamed();
  if (unlistenUpdated) unlistenUpdated();
});
</script>

<template>
  <div class="sidebar">
    <!-- 会话管理区域 -->
    <div class="sidebar-section">
      <div class="sidebar-title">
        <span>SESSIONS</span>
        <button class="new-session-btn" @click="createNewSession" title="新建会话">
          <svg viewBox="0 0 24 24" width="14" height="14" stroke="currentColor" stroke-width="2" fill="none">
            <line x1="12" y1="5" x2="12" y2="19"></line>
            <line x1="5" y1="12" x2="19" y2="12"></line>
          </svg>
        </button>
      </div>
      <ul class="session-list">
        <li 
          v-for="session in sessions" 
          :key="session.id" 
          :class="['session-item', { active: session.id === activeSessionId }]"
          @click="switchToSession(session.id)"
        >
          <svg viewBox="0 0 24 24" width="13" height="13" stroke="currentColor" stroke-width="2" fill="none" class="session-icon">
            <path d="M21 15a2 2 0 0 1-2 2H7l-4 4V5a2 2 0 0 1 2-2h14a2 2 0 0 1 2 2z"></path>
          </svg>
          <span class="session-title">{{ session.title }}</span>
          <span class="session-count" v-if="session.messageCount > 0">{{ session.messageCount }}</span>
          <button 
            v-if="session.id !== activeSessionId"
            class="delete-btn" 
            @click="deleteSession(session.id, $event)"
            title="删除会话"
          >
            <svg viewBox="0 0 24 24" width="11" height="11" stroke="currentColor" stroke-width="2" fill="none">
              <line x1="18" y1="6" x2="6" y2="18"></line>
              <line x1="6" y1="6" x2="18" y2="18"></line>
            </svg>
          </button>
        </li>
      </ul>
    </div>

    <!-- 任务列表区域 -->
    <div class="sidebar-section" v-if="todos.length > 0">
      <div class="sidebar-title"><span>TASKS</span></div>
      <ul class="todo-list">
        <li v-for="todo in todos" :key="todo.id" :class="['todo-item', todo.status]">
          <span class="todo-icon">
            <svg v-if="todo.status === 'completed'" viewBox="0 0 24 24" width="14" height="14" stroke="currentColor" stroke-width="2" fill="none" stroke-linecap="round" stroke-linejoin="round">
              <polyline points="20 6 9 17 4 12"></polyline>
            </svg>
            <svg v-else-if="todo.status === 'in_progress'" viewBox="0 0 24 24" width="14" height="14" stroke="currentColor" stroke-width="2" fill="none" stroke-linecap="round" stroke-linejoin="round" class="spin">
              <circle cx="12" cy="12" r="3"></circle>
              <path d="M19.4 15a1.65 1.65 0 0 0 .33 1.82l.06.06a2 2 0 0 1 0 2.83 2 2 0 0 1-2.83 0l-.06-.06a1.65 1.65 0 0 0-1.82-.33 1.65 1.65 0 0 0-1 1.51V21a2 2 0 0 1-2 2 2 2 0 0 1-2-2v-.09A1.65 1.65 0 0 0 9 19.4a1.65 1.65 0 0 0-1.82.33l-.06.06a2 2 0 0 1-2.83 0 2 2 0 0 1 0-2.83l.06-.06a1.65 1.65 0 0 0 .33-1.82 1.65 1.65 0 0 0-1.51-1H3a2 2 0 0 1-2-2 2 2 0 0 1 2-2h.09A1.65 1.65 0 0 0 4.6 9a1.65 1.65 0 0 0-.33-1.82l-.06-.06a2 2 0 0 1 0-2.83 2 2 0 0 1 2.83 0l.06.06a1.65 1.65 0 0 0 1.82.33H9a1.65 1.65 0 0 0 1-1.51V3a2 2 0 0 1 2-2 2 2 0 0 1 2 2v.09a1.65 1.65 0 0 0 1 1.51 1.65 1.65 0 0 0 1.82-.33l.06-.06a2 2 0 0 1 2.83 0 2 2 0 0 1 0 2.83l-.06.06a1.65 1.65 0 0 0-.33 1.82V9a1.65 1.65 0 0 0 1.51 1H21a2 2 0 0 1 2 2 2 2 0 0 1-2 2h-.09a1.65 1.65 0 0 0-1.51 1z"></path>
            </svg>
            <svg v-else viewBox="0 0 24 24" width="14" height="14" stroke="currentColor" stroke-width="2" fill="none" stroke-linecap="round" stroke-linejoin="round">
              <circle cx="12" cy="12" r="10"></circle>
            </svg>
          </span>
          <span class="todo-text">{{ todo.text }}</span>
        </li>
      </ul>
    </div>
  </div>
</template>

<style scoped>
.sidebar {
  width: 250px;
  background-color: var(--bg-sidebar);
  border-right: 1px solid var(--border-color);
  display: flex;
  flex-direction: column;
  overflow-y: auto;
}

.sidebar-section {
  display: flex;
  flex-direction: column;
}

.sidebar-section + .sidebar-section {
  border-top: 1px solid var(--border-color);
}

.sidebar-title {
  font-size: 0.75rem;
  color: var(--text-muted);
  padding: 10px 15px;
  letter-spacing: 1px;
  display: flex;
  justify-content: space-between;
  align-items: center;
  flex-shrink: 0;
}

.new-session-btn {
  background: none;
  border: none;
  color: var(--text-muted);
  cursor: pointer;
  padding: 2px;
  display: inline-flex;
  align-items: center;
  border-radius: 3px;
  transition: all 0.2s;
  -webkit-app-region: no-drag;
}
.new-session-btn:hover {
  color: var(--accent-blue);
  background: rgba(0, 102, 204, 0.1);
}

/* 会话列表样式 */
.session-list {
  list-style: none;
  padding: 0;
  margin: 0;
}

.session-item {
  padding: 6px 15px;
  font-size: 0.8rem;
  display: flex;
  align-items: center;
  cursor: pointer;
  color: var(--text-muted);
  transition: background-color 0.15s;
  position: relative;
}
.session-item:hover {
  background-color: rgba(0, 0, 0, 0.05);
}
.session-item.active {
  background-color: rgba(0, 102, 204, 0.08);
  color: var(--text-main);
}

.session-icon {
  margin-right: 8px;
  flex-shrink: 0;
  opacity: 0.6;
}
.session-item.active .session-icon {
  opacity: 1;
  color: var(--accent-blue);
}

.session-title {
  flex: 1;
  overflow: hidden;
  text-overflow: ellipsis;
  white-space: nowrap;
}

.session-count {
  font-size: 0.65rem;
  background: var(--border-color);
  color: var(--text-muted);
  padding: 0 5px;
  border-radius: 8px;
  margin-left: 4px;
  flex-shrink: 0;
}

.delete-btn {
  display: none;
  background: none;
  border: none;
  color: var(--text-muted);
  cursor: pointer;
  padding: 2px;
  margin-left: 4px;
  border-radius: 3px;
  align-items: center;
  -webkit-app-region: no-drag;
}
.session-item:hover .delete-btn {
  display: inline-flex;
}
.delete-btn:hover {
  color: var(--accent-red);
  background: rgba(215, 58, 73, 0.1);
}

/* 任务列表样式 */
.todo-list {
  list-style: none;
  padding: 0;
  margin: 0;
  overflow-y: auto;
}

.todo-item {
  padding: 6px 15px;
  font-size: 0.85rem;
  display: flex;
  align-items: flex-start;
  cursor: default;
}
.todo-item:hover {
  background-color: rgba(0,0,0,0.03);
}

.todo-icon {
  margin-right: 8px;
  font-size: 0.9rem;
  display: inline-flex;
  align-items: center;
  justify-content: center;
}

.todo-item.pending { color: var(--text-muted); }
.todo-item.in_progress { color: var(--accent-yellow); }
.todo-item.completed { color: var(--accent-green); opacity: 0.7; text-decoration: line-through; }

@keyframes spin {
  100% { transform: rotate(360deg); }
}
.spin {
  animation: spin 4s linear infinite;
}
</style>
