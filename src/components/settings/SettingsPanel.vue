<template>
  <div v-if="modelValue" class="settings-overlay">
    <div class="settings-modal">
      <div class="settings-header">
        <h3>系统设置</h3>
        <button class="icon-btn close-btn" @click="close" title="关闭">
          <svg viewBox="0 0 24 24" width="20" height="20">
            <path fill="currentColor" d="M19 6.41L17.59 5 12 10.59 6.41 5 5 6.41 10.59 12 5 17.59 6.41 19 12 13.41 17.59 19 19 17.59 13.41 12z" />
          </svg>
        </button>
      </div>
      
      <div class="settings-container">
        <!-- 左侧预设列表 -->
        <div class="settings-sidebar">
          <div class="sidebar-section-header">
            <span>配置预设</span>
            <button class="add-btn" @click="addProfile" title="添加新预设">+</button>
          </div>
          <div class="profile-list">
            <div 
              v-for="profile in appConfig.profiles" 
              :key="profile.id"
              class="profile-item"
              :class="{ 
                'active': selectedProfileId === profile.id,
                'is-running': appConfig.activeProfileId === profile.id 
              }"
              @click="selectProfile(profile.id)"
            >
              <span class="profile-name">{{ profile.name }}</span>
              <div class="profile-actions">
                <button v-if="appConfig.profiles.length > 1" class="delete-btn" @click.stop="deleteProfile(profile.id)">
                  <svg viewBox="0 0 24 24" width="14" height="14"><path fill="currentColor" d="M19 6.41L17.59 5 12 10.59 6.41 5 5 6.41 10.59 12 5 17.59 6.41 19 12 13.41 17.59 19 19 17.59 13.41 12z"/></svg>
                </button>
              </div>
            </div>
          </div>
        </div>

        <!-- 右侧编辑区域 -->
        <div class="settings-content" v-if="editingProfile">
          <div class="settings-body">
            <div class="setting-group">
              <h4>预设基本信息</h4>
              <div class="setting-item">
                <label>预设名称</label>
                <input type="text" v-model="editingProfile.name" placeholder="例如：Claude 3.5 Pro" />
              </div>
              <div class="setting-item">
                <label>设为当前激活</label>
                <div class="switch-container">
                  <input type="checkbox" :checked="appConfig.activeProfileId === selectedProfileId" @change="toggleActive" />
                  <span class="switch-label">{{ appConfig.activeProfileId === selectedProfileId ? '当前正在使用' : '切换到此预设' }}</span>
                </div>
              </div>
            </div>

            <div class="setting-group">
              <h4>API 配置</h4>
              <div class="setting-item">
                <label>API 风格 (API Format)</label>
                <select v-model="editingProfile.config.apiFormat" class="format-select">
                  <option value="anthropic">Anthropic (Claude) 原生格式</option>
                  <option value="openai">OpenAI (Chat Completions) 兼容格式</option>
                </select>
                <div class="setting-desc">如果使用火山引擎等国产模型直连，请选择 OpenAI 兼容格式。</div>
              </div>
              <div class="setting-item">
                <label>API Key</label>
                <input type="password" v-model="editingProfile.config.apiKey" placeholder="sk-..." />
              </div>
              <div class="setting-item">
                <label>Base URL</label>
                <input type="text" v-model="editingProfile.config.baseUrl" placeholder="https://api.anthropic.com/v1/messages" />
              </div>
            </div>

            <div class="setting-group">
              <h4>模型配置</h4>
              <div class="setting-item">
                <label>主代理模型 (Main Agent)</label>
                <input type="text" v-model="editingProfile.config.mainModel" placeholder="claude-3-5-sonnet-20241022" />
                <div class="setting-desc">负责对话、意图识别与任务分发。建议使用最强的模型。</div>
              </div>
              <div class="setting-item">
                <label>子代理模型 (Sub Agent)</label>
                <input type="text" v-model="editingProfile.config.subModel" placeholder="claude-3-5-sonnet-20241022" />
                <div class="setting-desc">负责实际执行具体任务（文件读写、搜索等）。</div>
              </div>
              <div class="setting-item">
                <label>工具代理模型 (Utility Agent)</label>
                <input type="text" v-model="editingProfile.config.utilityModel" placeholder="claude-3-5-haiku-20241022" />
                <div class="setting-desc">负责后台记忆整理、摘要压缩等。可使用更便宜、更快的模型。</div>
              </div>
            </div>
          </div>
        </div>
      </div>

      <div class="settings-footer">
        <span class="status-msg" :class="{ 'error': isError, 'success': isSuccess }">{{ statusMsg }}</span>
        <div class="footer-actions">
          <button class="save-btn" @click="save" :disabled="isSaving">
            {{ isSaving ? '保存中...' : '保存所有更改' }}
          </button>
        </div>
      </div>
    </div>
  </div>
</template>

<script setup lang="ts">
import { ref, watch, computed } from 'vue'
import { invoke } from '@tauri-apps/api/core'

const props = defineProps<{
  modelValue: boolean
}>()

const emit = defineEmits<{
  (e: 'update:modelValue', value: boolean): void
}>()

interface AgentConfig {
  apiFormat: string
  apiKey: string
  baseUrl: string
  mainModel: string
  subModel: string
  utilityModel: string
}

interface ModelProfile {
  id: string
  name: string
  config: AgentConfig
}

interface AppConfig {
  activeProfileId: string
  profiles: ModelProfile[]
}

const appConfig = ref<AppConfig>({
  activeProfileId: 'default',
  profiles: []
})

const selectedProfileId = ref('default')
const isSaving = ref(false)
const statusMsg = ref('')
const isError = ref(false)
const isSuccess = ref(false)

const editingProfile = computed(() => {
  return appConfig.value.profiles.find(p => p.id === selectedProfileId.value)
})

const loadConfig = async () => {
  try {
    const res = await invoke<AppConfig>('get_config')
    appConfig.value = res
    if (res.profiles.length > 0 && !res.profiles.find(p => p.id === selectedProfileId.value)) {
      selectedProfileId.value = res.activeProfileId || res.profiles[0].id
    }
  } catch (e) {
    console.error('Failed to load config:', e)
  }
}

watch(() => props.modelValue, (newVal) => {
  if (newVal) {
    loadConfig()
    statusMsg.value = ''
  }
})

const selectProfile = (id: string) => {
  selectedProfileId.value = id
}

const addProfile = () => {
  const newId = `profile_${Date.now()}`
  appConfig.value.profiles.push({
    id: newId,
    name: '新预设',
    config: {
      apiFormat: 'openai',
      apiKey: '',
      baseUrl: '',
      mainModel: '',
      subModel: '',
      utilityModel: ''
    }
  })
  selectedProfileId.value = newId
}

const deleteProfile = (id: string) => {
  if (appConfig.value.profiles.length <= 1) return
  
  const index = appConfig.value.profiles.findIndex(p => p.id === id)
  if (index !== -1) {
    appConfig.value.profiles.splice(index, 1)
    if (selectedProfileId.value === id) {
      selectedProfileId.value = appConfig.value.profiles[0].id
    }
    if (appConfig.value.activeProfileId === id) {
      appConfig.value.activeProfileId = appConfig.value.profiles[0].id
    }
  }
}

const toggleActive = (e: Event) => {
  const checked = (e.target as HTMLInputElement).checked
  if (checked) {
    appConfig.value.activeProfileId = selectedProfileId.value
  }
}

const close = () => {
  emit('update:modelValue', false)
}

const save = async () => {
  isSaving.value = true
  statusMsg.value = ''
  isError.value = false
  isSuccess.value = false

  try {
    await invoke('save_config_cmd', { newConfig: appConfig.value })
    isSuccess.value = true
    statusMsg.value = '配置已保存并同步'
    setTimeout(() => {
      close()
    }, 800)
  } catch (e) {
    isError.value = true
    statusMsg.value = `保存失败: ${e}`
  } finally {
    isSaving.value = false
  }
}
</script>

<style scoped>
.settings-overlay {
  position: fixed;
  top: 0;
  left: 0;
  width: 100%;
  height: 100%;
  background: rgba(0, 0, 0, 0.6);
  display: flex;
  justify-content: center;
  align-items: center;
  z-index: 1000;
  backdrop-filter: blur(8px);
}

.settings-modal {
  background: var(--bg-panel);
  border: 1px solid var(--border-color);
  border-radius: 16px;
  width: 850px;
  max-width: 95%;
  height: 600px;
  max-height: 90vh;
  display: flex;
  flex-direction: column;
  box-shadow: 0 12px 48px rgba(0, 0, 0, 0.3);
  animation: slideIn 0.3s cubic-bezier(0.16, 1, 0.3, 1);
  overflow: hidden;
}

@keyframes slideIn {
  from { opacity: 0; transform: scale(0.95) translateY(10px); }
  to { opacity: 1; transform: scale(1) translateY(0); }
}

.settings-header {
  display: flex;
  justify-content: space-between;
  align-items: center;
  padding: 16px 24px;
  border-bottom: 1px solid var(--border-color);
  background: var(--bg-sidebar);
}

.settings-header h3 {
  margin: 0;
  font-size: 18px;
  font-weight: 600;
  color: var(--text-main);
}

.settings-container {
  flex: 1;
  display: flex;
  overflow: hidden;
}

/* 侧边栏样式 */
.settings-sidebar {
  width: 220px;
  border-right: 1px solid var(--border-color);
  background: var(--bg-sidebar);
  display: flex;
  flex-direction: column;
}

.sidebar-section-header {
  padding: 16px;
  display: flex;
  justify-content: space-between;
  align-items: center;
  color: var(--text-muted);
  font-size: 12px;
  font-weight: 600;
  text-transform: uppercase;
  letter-spacing: 0.5px;
}

.add-btn {
  background: var(--accent-blue);
  color: white;
  border: none;
  width: 20px;
  height: 20px;
  border-radius: 4px;
  display: flex;
  align-items: center;
  justify-content: center;
  cursor: pointer;
  font-size: 16px;
  transition: all 0.2s;
}

.add-btn:hover {
  transform: scale(1.1);
  filter: brightness(1.1);
}

.profile-list {
  flex: 1;
  overflow-y: auto;
  padding: 8px;
}

.profile-item {
  padding: 10px 12px;
  border-radius: 8px;
  cursor: pointer;
  margin-bottom: 4px;
  display: flex;
  justify-content: space-between;
  align-items: center;
  transition: all 0.2s;
  color: var(--text-muted);
}

.profile-item:hover {
  background: rgba(128, 128, 128, 0.1);
  color: var(--text-main);
}

.profile-item.active {
  background: rgba(0, 102, 204, 0.1);
  color: var(--accent-blue);
  font-weight: 500;
}

.profile-item.is-running::after {
  content: "RUNNING";
  font-size: 9px;
  padding: 2px 4px;
  background: var(--accent-green);
  color: white;
  border-radius: 4px;
  font-weight: 700;
}

.profile-name {
  white-space: nowrap;
  overflow: hidden;
  text-overflow: ellipsis;
  flex: 1;
  font-size: 13px;
}

.delete-btn {
  background: transparent;
  border: none;
  color: var(--text-muted);
  opacity: 0;
  cursor: pointer;
  padding: 2px;
}

.profile-item:hover .delete-btn {
  opacity: 1;
}

.delete-btn:hover {
  color: var(--accent-red);
}

/* 内容区域样式 */
.settings-content {
  flex: 1;
  display: flex;
  flex-direction: column;
  background: var(--bg-panel);
}

.settings-body {
  padding: 24px 32px;
  overflow-y: auto;
}

.setting-group {
  margin-bottom: 32px;
}

.setting-group h4 {
  margin: 0 0 16px 0;
  font-size: 14px;
  color: var(--accent-blue);
  font-weight: 600;
  border-left: 3px solid var(--accent-blue);
  padding-left: 10px;
}

.setting-item {
  margin-bottom: 20px;
}

.setting-item label {
  display: block;
  margin-bottom: 8px;
  font-size: 13px;
  font-weight: 500;
  color: var(--text-main);
}

.setting-item input, .setting-item select {
  width: 100%;
  padding: 10px 14px;
  background: var(--bg-sidebar);
  border: 1px solid var(--border-color);
  border-radius: 8px;
  color: var(--text-main);
  font-family: var(--font-mono);
  font-size: 13px;
  transition: all 0.2s;
  box-sizing: border-box;
}

.setting-item input:focus, .setting-item select:focus {
  outline: none;
  border-color: var(--accent-blue);
  box-shadow: 0 0 0 3px rgba(0, 102, 204, 0.15);
}

.switch-container {
  display: flex;
  align-items: center;
  gap: 10px;
  padding: 8px 12px;
  background: rgba(128, 128, 128, 0.05);
  border-radius: 8px;
  width: fit-content;
}

.switch-label {
  font-size: 13px;
  color: var(--text-main);
}

.setting-desc {
  font-size: 12px;
  color: var(--text-muted);
  margin-top: 6px;
  line-height: 1.4;
}

.settings-footer {
  padding: 16px 24px;
  border-top: 1px solid var(--border-color);
  display: flex;
  justify-content: space-between;
  align-items: center;
  background: var(--bg-sidebar);
}

.save-btn {
  background: var(--accent-blue);
  color: white;
  border: none;
  padding: 10px 24px;
  border-radius: 8px;
  cursor: pointer;
  font-size: 14px;
  font-weight: 600;
  transition: all 0.2s;
  box-shadow: 0 2px 8px rgba(0, 102, 204, 0.3);
}

.save-btn:hover {
  filter: brightness(1.1);
  transform: translateY(-1px);
}

.save-btn:active {
  transform: translateY(0);
}

.save-btn:disabled {
  opacity: 0.5;
  cursor: not-allowed;
  transform: none;
}

.status-msg {
  font-size: 13px;
  font-weight: 500;
}

.status-msg.error {
  color: var(--accent-red);
}

.status-msg.success {
  color: var(--accent-green);
}

.icon-btn {
  background: transparent;
  border: none;
  color: var(--text-muted);
  cursor: pointer;
  padding: 6px;
  border-radius: 8px;
  display: flex;
  align-items: center;
  justify-content: center;
  transition: all 0.2s;
}

.icon-btn:hover {
  background: rgba(128, 128, 128, 0.15);
  color: var(--text-main);
}
</style>

