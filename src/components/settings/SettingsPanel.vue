<template>
  <div v-if="modelValue" class="settings-overlay" @click.self="close">
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
              v-for="profile in draftConfig.profiles"
              :key="profile.id"
              class="profile-item"
              :class="{
                'active': selectedProfileId === profile.id,
                'is-running': draftConfig.activeProfileId === profile.id
              }"
              @click="selectProfile(profile.id)"
            >
              <span class="profile-name">{{ profile.name }}</span>
              <div class="profile-actions">
                <label class="sidebar-switch" title="设为全局默认" @click.stop>
                  <input
                    type="checkbox"
                    :checked="savedConfig.globalProfileId === profile.id"
                    :disabled="actionLoading"
                    @change="toggleGlobalProfile(profile.id)"
                  />
                  <span class="slider"></span>
                </label>
                <button
                  v-if="draftConfig.profiles.length > 1"
                  class="delete-btn"
                  :disabled="actionLoading"
                  @click.stop="requestDeleteProfile(profile.id)"
                >
                  <svg viewBox="0 0 24 24" width="14" height="14"><path fill="currentColor" d="M19 6.41L17.59 5 12 10.59 6.41 5 5 6.41 10.59 12 5 17.59 6.41 19 12 13.41 17.59 19 19 17.59 13.41 12z"/></svg>
                </button>
              </div>
            </div>
          </div>

          <div class="sidebar-section-header" style="margin-top: 16px;">
            <span>外观</span>
          </div>
          <div class="theme-toggle-section">
            <button class="theme-toggle-btn" @click="toggleTheme">
              <svg v-if="isDark" viewBox="0 0 24 24" width="16" height="16" stroke="currentColor" stroke-width="2" fill="none" stroke-linecap="round" stroke-linejoin="round">
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
              <svg v-else viewBox="0 0 24 24" width="16" height="16" stroke="currentColor" stroke-width="2" fill="none" stroke-linecap="round" stroke-linejoin="round">
                <path d="M21 12.79A9 9 0 1 1 11.21 3 7 7 0 0 0 21 12.79z"></path>
              </svg>
              <span>{{ isDark ? '亮色模式' : '暗色模式' }}</span>
            </button>
          </div>
          <div class="display-mode-section">
            <div class="display-mode-label">回复视图</div>
            <div class="display-mode-toggle" role="group" aria-label="Agent display mode">
              <button
                type="button"
                class="display-mode-btn"
                :class="{ active: agentDisplayMode === 'user' }"
                @click="setAgentDisplayMode('user')"
                data-tooltip="普通视图：只显示适合日常使用的回复内容"
                aria-label="普通视图：只显示适合日常使用的回复内容"
              >
                普通
              </button>
              <button
                type="button"
                class="display-mode-btn"
                :class="{ active: agentDisplayMode === 'developer' }"
                @click="setAgentDisplayMode('developer')"
                data-tooltip="开发者视图：显示更完整的执行过程和调试信息"
                aria-label="开发者视图：显示更完整的执行过程和调试信息"
              >
                开发者
              </button>
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
                <label>对话模型 (Main + Sub Agent)</label>
                <input
                  type="text"
                  v-model="editingProfile.config.mainModel"
                  placeholder="deepseek-v4-pro / claude-3-5-sonnet-20241022"
                  @input="onMainModelInput"
                />
                <div class="capability-badges" v-if="mainModelCaps !== null">
                  <span class="badge" :class="mainModelCaps ? 'badge-ok' : 'badge-none'">
                    <svg v-if="mainModelCaps" viewBox="0 0 24 24" width="12" height="12" stroke="currentColor" stroke-width="2" fill="none" stroke-linecap="round" stroke-linejoin="round"><polyline points="20 6 9 17 4 12"></polyline></svg>
                    <svg v-else viewBox="0 0 24 24" width="12" height="12" stroke="currentColor" stroke-width="2" fill="none" stroke-linecap="round" stroke-linejoin="round"><circle cx="12" cy="12" r="10"></circle><path d="M9.09 9a3 3 0 0 1 5.83 1c0 2-3 3-3 3"></path><line x1="12" y1="17" x2="12.01" y2="17"></line></svg>
                    <span>{{ mainModelCaps ? '已识别' : '未知模型' }}</span>
                  </span>
                  <template v-if="mainModelCaps">
                    <span class="badge" :class="mainModelCaps.streaming ? 'badge-ok' : 'badge-no'">
                      <svg v-if="mainModelCaps.streaming" viewBox="0 0 24 24" width="12" height="12" stroke="currentColor" stroke-width="2" fill="none" stroke-linecap="round" stroke-linejoin="round"><polygon points="13 2 3 14 12 14 11 22 21 10 12 10 13 2"></polygon></svg>
                      <svg v-else viewBox="0 0 24 24" width="12" height="12" stroke="currentColor" stroke-width="2" fill="none" stroke-linecap="round" stroke-linejoin="round"><line x1="18" y1="6" x2="6" y2="18"></line><line x1="6" y1="6" x2="18" y2="18"></line></svg>
                      <span>{{ mainModelCaps.streaming ? '流式' : '无流式' }}</span>
                    </span>
                    <span class="badge" :class="mainModelCaps.thinking ? 'badge-think' : 'badge-none'">
                      <svg v-if="mainModelCaps.thinking" viewBox="0 0 24 24" width="12" height="12" stroke="currentColor" stroke-width="2" fill="none" stroke-linecap="round" stroke-linejoin="round"><path d="M9.5 2A2.5 2.5 0 0 1 12 4.5v15a2.5 2.5 0 0 1-4.96.44 2.5 2.5 0 0 1-2.96-3.08 3 3 0 0 1-.34-5.58 2.5 2.5 0 0 1 1.32-4.24 2.5 2.5 0 0 1 1.98-3A2.5 2.5 0 0 1 9.5 2Z"></path><path d="M14.5 2A2.5 2.5 0 0 0 12 4.5v15a2.5 2.5 0 0 0 4.96.44 2.5 2.5 0 0 0 2.96-3.08 3 3 0 0 0 .34-5.58 2.5 2.5 0 0 0-1.32-4.24 2.5 2.5 0 0 0-1.98-3A2.5 2.5 0 0 0 14.5 2Z"></path></svg>
                      <svg v-else viewBox="0 0 24 24" width="12" height="12" stroke="currentColor" stroke-width="2" fill="none" stroke-linecap="round" stroke-linejoin="round"><line x1="5" y1="12" x2="19" y2="12"></line></svg>
                      <span>{{ mainModelCaps.thinking ? '深度思考' : '无思考' }}</span>
                    </span>
                    <span class="badge" :class="mainModelCaps.temperature ? 'badge-ok' : 'badge-none'">
                      <svg v-if="mainModelCaps.temperature" viewBox="0 0 24 24" width="12" height="12" stroke="currentColor" stroke-width="2" fill="none" stroke-linecap="round" stroke-linejoin="round"><line x1="4" y1="21" x2="4" y2="14"></line><line x1="4" y1="10" x2="4" y2="3"></line><line x1="12" y1="21" x2="12" y2="12"></line><line x1="12" y1="8" x2="12" y2="3"></line><line x1="20" y1="21" x2="20" y2="16"></line><line x1="20" y1="12" x2="20" y2="3"></line><line x1="1" y1="14" x2="7" y2="14"></line><line x1="9" y1="8" x2="15" y2="8"></line><line x1="17" y1="16" x2="23" y2="16"></line></svg>
                      <svg v-else viewBox="0 0 24 24" width="12" height="12" stroke="currentColor" stroke-width="2" fill="none" stroke-linecap="round" stroke-linejoin="round"><line x1="5" y1="12" x2="19" y2="12"></line></svg>
                      <span>{{ mainModelCaps.temperature ? '温度可调' : '温度固定' }}</span>
                    </span>
                    <span class="badge" :class="mainModelCaps.vision ? 'badge-ok' : 'badge-none'">
                      <svg v-if="mainModelCaps.vision" viewBox="0 0 24 24" width="12" height="12" stroke="currentColor" stroke-width="2" fill="none" stroke-linecap="round" stroke-linejoin="round"><path d="M1 12s4-8 11-8 11 8 11 8-4 8-11 8-11-8-11-8z"></path><circle cx="12" cy="12" r="3"></circle></svg>
                      <svg v-else viewBox="0 0 24 24" width="12" height="12" stroke="currentColor" stroke-width="2" fill="none" stroke-linecap="round" stroke-linejoin="round"><path d="M17.94 17.94A10.07 10.07 0 0 1 12 20c-7 0-11-8-11-8a18.45 18.45 0 0 1 5.06-5.94M9.9 4.24A9.12 9.12 0 0 1 12 4c7 0 11 8 11 8a18.5 18.5 0 0 1-2.16 3.19m-6.72-1.07a3 3 0 1 1-4.24-4.24"></path><line x1="1" y1="1" x2="23" y2="23"></line></svg>
                      <span>{{ mainModelCaps.vision ? '多模态' : '纯文本' }}</span>
                    </span>
                    <span class="badge badge-info">
                      <svg viewBox="0 0 24 24" width="12" height="12" stroke="currentColor" stroke-width="2" fill="none" stroke-linecap="round" stroke-linejoin="round"><path d="M4 22h14a2 2 0 0 0 2-2V7.5L14.5 2H6a2 2 0 0 0-2 2v4"></path><polyline points="14 2 14 8 20 8"></polyline><path d="M2 15h10"></path><path d="M9 18l3-3-3-3"></path></svg>
                      <span>Max {{ mainModelCaps.maxTokens.toLocaleString() }} tokens</span>
                    </span>
                  </template>
                  <span class="badge badge-warn" v-else>手动确认是否支持各功能</span>
                </div>
                <div class="capability-note" v-if="mainModelCaps?.notes">{{ mainModelCaps.notes }}</div>
                <div class="setting-desc">主代理（对话）与子代理（执行任务）共用此模型。建议使用最强的模型。</div>
              </div>
              <div class="setting-item">
                <label>工具代理模型 (Utility Agent)</label>
                <input type="text" v-model="editingProfile.config.utilityModel" placeholder="claude-3-5-haiku-20241022" />
                <div class="setting-desc">负责后台记忆整理、摘要压缩等。可使用更便宜、更快的模型。</div>
              </div>

              <div class="setting-item">
                <label>温度 (Temperature)</label>
                <input type="number" v-model.number="editingProfile.config.temperature" placeholder="留空使用模型默认值" step="0.1" min="0" max="2" />
                <div class="setting-desc">控制输出的随机性，值越大越具创造性，越小越严谨。</div>
              </div>

              <div class="setting-item">
                <label>Top-P</label>
                <input type="number" v-model.number="editingProfile.config.topP" placeholder="留空使用模型默认值" step="0.1" min="0" max="1" />
                <div class="setting-desc">核采样，控制输出词汇的范围分布，与温度类似。</div>
              </div>

              <div class="setting-item" v-show="editingProfile.config.apiFormat === 'anthropic'">
                <label>Top-K</label>
                <input type="number" v-model.number="editingProfile.config.topK" placeholder="留空使用模型默认值" step="1" min="0" />
                <div class="setting-desc">控制每次采样只从概率最高的 K 个词中选取。</div>
              </div>
            </div>

            <div class="setting-group">
              <h4>图片压缩</h4>
              <div class="setting-item">
                <label>最大宽度 (px)</label>
                <input type="number" v-model.number="editingProfile.config.imageMaxWidth" placeholder="1920" step="1" min="100" max="8192" />
                <div class="setting-desc">超过此宽度的图片将等比缩放。留空使用默认值 1920。</div>
              </div>
              <div class="setting-item">
                <label>最大高度 (px)</label>
                <input type="number" v-model.number="editingProfile.config.imageMaxHeight" placeholder="1080" step="1" min="100" max="8192" />
                <div class="setting-desc">超过此高度的图片将等比缩放。留空使用默认值 1080。</div>
              </div>
              <div class="setting-item">
                <label>压缩质量</label>
                <input type="number" v-model.number="editingProfile.config.imageQuality" placeholder="0.8" step="0.05" min="0.1" max="1.0" />
                <div class="setting-desc">JPEG/WebP 压缩质量，0.1 最小体积 ~ 1.0 最高画质。留空使用默认值 0.8。</div>
              </div>
            </div>
          </div>
        </div>
      </div>

      <div class="settings-footer">
        <span class="status-msg" :class="{ 'error': isError, 'success': isSuccess }">{{ statusMsg }}</span>
        <div class="footer-actions">
          <button class="save-btn" @click="save" :disabled="isSaving || actionLoading">
            {{ isSaving ? '保存中...' : '保存所有更改' }}
          </button>
        </div>
      </div>
    </div>

    <ConfirmModal
      :open="!!deleteConfirm"
      :title="deleteConfirm?.title || ''"
      :message="deleteConfirm?.message || ''"
      :warning="deleteConfirm?.warning || ''"
      confirm-text="确认删除"
      cancel-text="取消"
      confirm-kind="danger"
      :loading="actionLoading"
      @cancel="deleteConfirm = null"
      @confirm="confirmDeleteProfile"
    />
  </div>
</template>

<script setup lang="ts">
import { ref, watch, computed } from 'vue'
import { invoke } from '@tauri-apps/api/core'
import { useTheme } from '../../composables/useTheme'
import { usePreferences } from '../../composables/usePreferences'
import type { AgentDisplayMode } from '../../types'
import ConfirmModal from '../common/ConfirmModal.vue'

const { isDark, toggleTheme } = useTheme()
const uiPrefs = usePreferences()
const agentDisplayMode = uiPrefs.agentDisplayMode
const setAgentDisplayMode = (mode: AgentDisplayMode) => uiPrefs.setAgentDisplayMode(mode)

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
  utilityModel: string
  enableThinking?: boolean
  temperature?: number | null
  topP?: number | null
  topK?: number | null
  imageMaxWidth?: number | null
  imageMaxHeight?: number | null
  imageQuality?: number | null
}

interface ModelCapabilities {
  streaming: boolean
  thinking: boolean
  thinkingParam?: string
  temperature: boolean
  vision: boolean
  maxTokens: number
  notes: string
}

interface ModelProfile {
  id: string
  name: string
  config: AgentConfig
}

interface AppConfig {
  activeProfileId: string
  globalProfileId: string
  profiles: ModelProfile[]
}

const createEmptyConfig = (): AppConfig => ({
  activeProfileId: 'default',
  globalProfileId: 'default',
  profiles: []
})

const createBlankProfile = (id: string): ModelProfile => ({
  id,
  name: '新预设',
  config: {
    apiFormat: 'openai',
    apiKey: '',
    baseUrl: '',
    mainModel: '',
    utilityModel: '',
    enableThinking: false,
    temperature: null,
    topP: null,
    topK: null,
    imageMaxWidth: null,
    imageMaxHeight: null,
    imageQuality: null
  }
})

const cloneConfig = <T>(value: T): T => JSON.parse(JSON.stringify(value))

const normalizeProfileConfig = (config: AppConfig) => {
  config.profiles.forEach((p) => {
    p.config.temperature = p.config.temperature == null ? null : Number(p.config.temperature)
    p.config.topP = p.config.topP == null ? null : Number(p.config.topP)
    p.config.topK = p.config.topK == null ? null : Number(p.config.topK)
    p.config.imageMaxWidth = p.config.imageMaxWidth == null ? null : Number(p.config.imageMaxWidth)
    p.config.imageMaxHeight = p.config.imageMaxHeight == null ? null : Number(p.config.imageMaxHeight)
    p.config.imageQuality = p.config.imageQuality == null ? null : Number(p.config.imageQuality)
  })
  return config
}

const ensureValidSelection = (config: AppConfig, preferredId?: string) => {
  if (config.profiles.length === 0) {
    config.activeProfileId = 'default'
    config.globalProfileId = 'default'
    selectedProfileId.value = 'default'
    return
  }

  const fallbackId = preferredId && config.profiles.find((p) => p.id === preferredId)
    ? preferredId
    : config.profiles[0].id

  if (!config.profiles.find((p) => p.id === config.activeProfileId)) {
    config.activeProfileId = config.profiles[0].id
  }
  if (!config.profiles.find((p) => p.id === config.globalProfileId)) {
    config.globalProfileId = config.profiles[0].id
  }
  if (!config.profiles.find((p) => p.id === selectedProfileId.value)) {
    selectedProfileId.value = fallbackId
  }
}

const syncConfigs = (config: AppConfig, preferredId?: string) => {
  const normalized = normalizeProfileConfig(cloneConfig(config))
  ensureValidSelection(normalized, preferredId)
  savedConfig.value = cloneConfig(normalized)
  draftConfig.value = cloneConfig(normalized)
}

const savedConfig = ref<AppConfig>(createEmptyConfig())
const draftConfig = ref<AppConfig>(createEmptyConfig())
const selectedProfileId = ref('default')
const isSaving = ref(false)
const actionLoading = ref(false)
const statusMsg = ref('')
const isError = ref(false)
const isSuccess = ref(false)
const deleteConfirm = ref<{ id: string; title: string; message: string; warning: string } | null>(null)

const editingProfile = computed(() => {
  return draftConfig.value.profiles.find(p => p.id === selectedProfileId.value)
})

const mainModelCaps = ref<ModelCapabilities | null | undefined>(undefined)
let capQueryTimer: ReturnType<typeof setTimeout> | null = null

const resetStatus = () => {
  statusMsg.value = ''
  isError.value = false
  isSuccess.value = false
}

const setErrorStatus = (message: string) => {
  statusMsg.value = message
  isError.value = true
  isSuccess.value = false
}

const setSuccessStatus = (message: string) => {
  statusMsg.value = message
  isError.value = false
  isSuccess.value = true
}

const onMainModelInput = () => {
  if (capQueryTimer) clearTimeout(capQueryTimer)
  mainModelCaps.value = undefined
  capQueryTimer = setTimeout(async () => {
    const modelId = editingProfile.value?.config.mainModel?.trim()
    if (!modelId) {
      mainModelCaps.value = undefined
      return
    }
    try {
      const caps = await invoke<ModelCapabilities | null>('get_model_capabilities', { modelId })
      mainModelCaps.value = caps
    } catch {
      mainModelCaps.value = null
    }
  }, 400)
}

watch(selectedProfileId, () => {
  const modelId = editingProfile.value?.config.mainModel?.trim()
  if (modelId) onMainModelInput()
  else mainModelCaps.value = undefined
})

const loadConfig = async () => {
  try {
    const res = await invoke<AppConfig>('get_config')
    syncConfigs(res, res.activeProfileId || res.profiles[0]?.id)
  } catch (e) {
    console.error('Failed to load config:', e)
  }
}

watch(() => props.modelValue, (newVal) => {
  if (newVal) {
    resetStatus()
    deleteConfirm.value = null
    loadConfig()
  }
})

const selectProfile = (id: string) => {
  selectedProfileId.value = id
}

const addProfile = () => {
  resetStatus()
  const newId = `profile_${Date.now()}`
  draftConfig.value.profiles.push(createBlankProfile(newId))
  selectedProfileId.value = newId
}

const toggleGlobalProfile = async (profileId: string) => {
  if (actionLoading.value || savedConfig.value.globalProfileId === profileId) return

  actionLoading.value = true
  resetStatus()
  try {
    const nextConfig = cloneConfig(savedConfig.value)
    nextConfig.globalProfileId = profileId
    ensureValidSelection(nextConfig, selectedProfileId.value)
    await invoke('save_config_cmd', { newConfig: nextConfig })
    syncConfigs(nextConfig, selectedProfileId.value)
    setSuccessStatus('全局默认预设已更新')
  } catch (e) {
    console.error('保存全局预设失败:', e)
    setErrorStatus(`保存全局预设失败: ${e}`)
  } finally {
    actionLoading.value = false
  }
}

const requestDeleteProfile = (id: string) => {
  if (draftConfig.value.profiles.length <= 1) return

  const profile = draftConfig.value.profiles.find(p => p.id === id)
  if (!profile) return

  deleteConfirm.value = {
    id,
    title: '确认删除配置预设',
    message: `确定要删除配置预设“${profile.name}”吗？`,
    warning: '此操作会立即保存且不可撤销。'
  }
}

const confirmDeleteProfile = async () => {
  if (!deleteConfirm.value || actionLoading.value) return

  actionLoading.value = true
  resetStatus()
  try {
    const targetId = deleteConfirm.value.id
    const nextConfig = cloneConfig(savedConfig.value)
    const index = nextConfig.profiles.findIndex((p) => p.id === targetId)
    if (index === -1) {
      deleteConfirm.value = null
      return
    }

    nextConfig.profiles.splice(index, 1)
    if (nextConfig.profiles.length === 0) {
      throw new Error('至少需要保留一个配置预设')
    }

    const fallbackId = nextConfig.profiles[0].id
    if (nextConfig.activeProfileId === targetId) {
      nextConfig.activeProfileId = fallbackId
    }
    if (nextConfig.globalProfileId === targetId) {
      nextConfig.globalProfileId = fallbackId
    }

    const nextSelectedId = selectedProfileId.value === targetId ? fallbackId : selectedProfileId.value
    ensureValidSelection(nextConfig, nextSelectedId)
    await invoke('save_config_cmd', { newConfig: nextConfig })
    syncConfigs(nextConfig, nextSelectedId)
    deleteConfirm.value = null
    setSuccessStatus('配置预设已删除并保存')
  } catch (e) {
    console.error('删除配置预设失败:', e)
    setErrorStatus(`删除配置预设失败: ${e}`)
  } finally {
    actionLoading.value = false
  }
}

const hasMeaningfulDraftValue = (value: unknown): boolean => {
  if (value === null || value === undefined || value === false) return false
  if (typeof value === 'number') return Number.isFinite(value)
  return String(value).trim().length > 0
}

const hasNewProfileContent = (profile: ModelProfile): boolean => {
  const blank = createBlankProfile(profile.id)
  if (profile.name.trim() && profile.name.trim() !== blank.name) return true
  if (profile.config.apiFormat !== blank.config.apiFormat) return true
  if ((profile.config.enableThinking ?? false) !== (blank.config.enableThinking ?? false)) return true

  const contentKeys: Array<keyof AgentConfig> = [
    'apiKey',
    'baseUrl',
    'mainModel',
    'utilityModel',
    'temperature',
    'topP',
    'topK',
    'imageMaxWidth',
    'imageMaxHeight',
    'imageQuality'
  ]

  return contentKeys.some((key) => hasMeaningfulDraftValue(profile.config[key]))
}

const persistFilledNewProfilesBeforeClose = async () => {
  const savedIds = new Set(savedConfig.value.profiles.map((profile) => profile.id))
  const filledNewProfiles = draftConfig.value.profiles.filter((profile) => {
    return !savedIds.has(profile.id) && hasNewProfileContent(profile)
  })

  if (filledNewProfiles.length === 0) return

  const nextConfig = cloneConfig(savedConfig.value)
  nextConfig.profiles.push(...filledNewProfiles.map((profile) => cloneConfig(profile)))
  ensureValidSelection(nextConfig, selectedProfileId.value)
  const normalized = normalizeProfileConfig(nextConfig)

  await invoke('save_config_cmd', { newConfig: normalized })
  syncConfigs(normalized, selectedProfileId.value)
}

const close = async () => {
  if (isSaving.value || actionLoading.value) return

  try {
    await persistFilledNewProfilesBeforeClose()
  } catch (e) {
    console.error('保存已填写的新预设失败:', e)
    setErrorStatus(`保存已填写的新预设失败: ${e}`)
    return
  }

  draftConfig.value = cloneConfig(savedConfig.value)
  ensureValidSelection(draftConfig.value, selectedProfileId.value)
  deleteConfirm.value = null
  resetStatus()
  emit('update:modelValue', false)
}

const save = async () => {
  if (!draftConfig.value.profiles || draftConfig.value.profiles.length === 0) return

  ensureValidSelection(draftConfig.value, selectedProfileId.value)

  for (const p of draftConfig.value.profiles) {
    if (!p.name || !p.name.trim()) {
      setErrorStatus('预设名称不能为空')
      return
    }
    if (!p.config.baseUrl || !p.config.baseUrl.trim()) {
      setErrorStatus(`预设 [${p.name}] 的 API Base URL 不能为空`)
      return
    }
    if (!p.config.mainModel || !p.config.mainModel.trim()) {
      setErrorStatus(`预设 [${p.name}] 的主代理模型不能为空`)
      return
    }
    if (!p.config.utilityModel || !p.config.utilityModel.trim()) {
      setErrorStatus(`预设 [${p.name}] 的工具代理模型不能为空`)
      return
    }
  }

  isSaving.value = true
  resetStatus()

  try {
    const nextConfig = normalizeProfileConfig(cloneConfig(draftConfig.value))
    ensureValidSelection(nextConfig, selectedProfileId.value)
    await invoke('save_config_cmd', { newConfig: nextConfig })
    syncConfigs(nextConfig, selectedProfileId.value)
    setSuccessStatus('配置已保存并同步')
    setTimeout(() => {
      close()
    }, 800)
  } catch (e) {
    setErrorStatus(`保存失败: ${e}`)
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
  background: rgba(0, 0, 0, 0.4);
  display: flex;
  justify-content: center;
  align-items: center;
  z-index: 1000;
  backdrop-filter: blur(12px);
  -webkit-backdrop-filter: blur(12px);
  animation: fadeIn var(--transition-fast);
}

@keyframes fadeIn {
  from { opacity: 0; }
  to { opacity: 1; }
}

.settings-modal {
  background: var(--surface-strong);
  backdrop-filter: blur(var(--glass-blur-heavy));
  -webkit-backdrop-filter: blur(var(--glass-blur-heavy));
  border: 1px solid var(--glass-border);
  border-radius: var(--radius-xl);
  width: min(1080px, calc(100vw - 48px));
  max-width: 96vw;
  height: min(760px, calc(100vh - 48px));
  max-height: 90vh;
  display: flex;
  flex-direction: column;
  box-shadow: 0 24px 70px rgba(15, 23, 42, 0.22), var(--glass-shadow);
  animation: slideIn var(--transition-normal);
  overflow: hidden;
}

@keyframes slideIn {
  from { opacity: 0; transform: scale(0.96) translateY(12px); }
  to { opacity: 1; transform: scale(1) translateY(0); }
}

.settings-header {
  display: flex;
  justify-content: space-between;
  align-items: center;
  min-height: 58px;
  padding: 14px 22px 14px 24px;
  border-bottom: 1px solid var(--glass-border);
  background: var(--surface-strong);
  backdrop-filter: blur(var(--glass-blur));
  -webkit-backdrop-filter: blur(var(--glass-blur));
}

.settings-header h3 {
  margin: 0;
  font-size: 1.1rem;
  font-weight: 600;
  color: var(--text-main);
  letter-spacing: 0.02em;
}

.settings-container {
  flex: 1;
  display: flex;
  overflow: hidden;
}

/* 侧边栏样式 */
.settings-sidebar {
  width: 260px;
  flex: 0 0 260px;
  border-right: 1px solid var(--glass-border);
  background: color-mix(in srgb, var(--surface-strong) 76%, var(--glass-bg-light));
  backdrop-filter: blur(var(--glass-blur));
  -webkit-backdrop-filter: blur(var(--glass-blur));
  display: flex;
  flex-direction: column;
}

.sidebar-section-header {
  padding: 18px 18px 10px;
  display: flex;
  justify-content: space-between;
  align-items: center;
  color: var(--text-muted);
  font-size: 0.8rem;
  font-weight: 600;
  text-transform: uppercase;
  letter-spacing: 0.05em;
}

.add-btn {
  background: var(--glass-bg-light);
  color: var(--accent-blue);
  border: 1px solid rgba(59, 130, 246, 0.3);
  width: 24px;
  height: 24px;
  border-radius: var(--radius-md);
  display: flex;
  align-items: center;
  justify-content: center;
  cursor: pointer;
  font-size: 16px;
  font-weight: 600;
  transition: all var(--transition-fast);
  backdrop-filter: blur(8px);
  -webkit-backdrop-filter: blur(8px);
}

.add-btn:hover {
  background: rgba(59, 130, 246, 0.1);
  border-color: var(--accent-blue);
  transform: translateY(-1px);
}

.theme-toggle-section {
  padding: 0 12px;
}

.theme-toggle-btn {
  display: flex;
  align-items: center;
  gap: 10px;
  width: 100%;
  padding: 10px 12px;
  background: transparent;
  border: 1px solid var(--glass-border-subtle);
  border-radius: var(--radius-md);
  color: var(--text-main);
  font-size: 0.85rem;
  cursor: pointer;
  transition: all var(--transition-fast);
}

.theme-toggle-btn:hover {
  background: var(--glass-bg-light);
  border-color: var(--accent-blue);
}

.display-mode-section {
  padding: 14px 12px 18px;
}

.display-mode-label {
  margin-bottom: 8px;
  padding: 0 4px;
  color: var(--text-muted);
  font-size: 0.75rem;
  font-weight: 600;
}

.display-mode-toggle {
  display: grid;
  grid-template-columns: 1fr 1fr;
  gap: 4px;
  padding: 3px;
  border: 1px solid var(--glass-border-subtle);
  border-radius: var(--radius-md);
  background: var(--glass-bg-light);
  overflow: visible;
}

.display-mode-toggle button {
  position: relative;
  min-width: 0;
  height: 30px;
  border: 0;
  border-radius: 6px;
  background: transparent;
  color: var(--text-muted);
  cursor: pointer;
  font-size: 0.78rem;
  font-weight: 600;
  transition: background var(--transition-fast), color var(--transition-fast);
}

.display-mode-toggle button:hover {
  color: var(--text-main);
}

.display-mode-btn::before,
.display-mode-btn::after {
  position: absolute;
  left: 50%;
  pointer-events: none;
  opacity: 0;
  visibility: hidden;
  transform: translate(-50%, -4px);
  transition: opacity var(--transition-fast), transform var(--transition-fast), visibility var(--transition-fast);
  z-index: 20;
}

.display-mode-btn::before {
  content: attr(data-tooltip);
  bottom: calc(100% + 10px);
  width: max-content;
  max-width: 220px;
  padding: 8px 10px;
  border: 1px solid var(--glass-border);
  border-radius: 8px;
  background: var(--surface-strong);
  color: var(--text-main);
  box-shadow: 0 10px 26px rgba(15, 23, 42, 0.22), var(--shadow-sm);
  font-size: 12px;
  font-weight: 500;
  line-height: 1.45;
  text-align: left;
  white-space: normal;
}

.display-mode-btn:first-child::before {
  left: 0;
  transform: translate(0, -4px);
}

.display-mode-btn:last-child::before {
  right: 0;
  left: auto;
  transform: translate(0, -4px);
}

.display-mode-btn::after {
  content: "";
  bottom: calc(100% + 5px);
  width: 9px;
  height: 9px;
  border-right: 1px solid var(--glass-border);
  border-bottom: 1px solid var(--glass-border);
  background: var(--surface-strong);
  transform: translate(-50%, -4px) rotate(45deg);
}

.display-mode-btn:hover::before,
.display-mode-btn:hover::after,
.display-mode-btn:focus-visible::before,
.display-mode-btn:focus-visible::after {
  opacity: 1;
  visibility: visible;
}

.display-mode-btn:hover::before,
.display-mode-btn:focus-visible::before {
  transform: translate(-50%, 0);
}

.display-mode-btn:first-child:hover::before,
.display-mode-btn:first-child:focus-visible::before,
.display-mode-btn:last-child:hover::before,
.display-mode-btn:last-child:focus-visible::before {
  transform: translate(0, 0);
}

.display-mode-btn:hover::after,
.display-mode-btn:focus-visible::after {
  transform: translate(-50%, 0) rotate(45deg);
}

.display-mode-toggle button.active {
  background: var(--glass-bg-heavy);
  color: var(--accent-blue);
  box-shadow: var(--shadow-sm);
}

.profile-list {
  flex: 1;
  overflow-y: auto;
  padding: 6px 12px 12px;
}

.profile-item {
  min-height: 38px;
  padding: 8px 10px 8px 12px;
  border-radius: var(--radius-md);
  cursor: pointer;
  margin-bottom: 3px;
  display: flex;
  justify-content: space-between;
  align-items: center;
  gap: 8px;
  transition: all var(--transition-fast);
  color: var(--text-muted);
}

.profile-item:hover {
  background: var(--glass-bg-light);
  color: var(--text-main);
}

.profile-item.active {
  background: rgba(0, 102, 204, 0.12);
  color: var(--accent-blue);
  font-weight: 500;
  box-shadow: inset 3px 0 0 var(--accent-blue);
}

.profile-name {
  white-space: nowrap;
  overflow: hidden;
  text-overflow: ellipsis;
  flex: 1;
  font-size: 13px;
  padding-right: 8px;
}

.profile-actions {
  display: flex;
  align-items: center;
  gap: 6px;
}

.sidebar-switch {
  position: relative;
  display: inline-block;
  width: 28px;
  height: 16px;
  margin: 0;
}

.sidebar-switch input {
  opacity: 0;
  width: 0;
  height: 0;
}

.sidebar-switch .slider {
  position: absolute;
  cursor: pointer;
  top: 0;
  left: 0;
  right: 0;
  bottom: 0;
  background-color: var(--border-color);
  transition: .3s;
  border-radius: 16px;
}

.sidebar-switch .slider:before {
  position: absolute;
  content: "";
  height: 12px;
  width: 12px;
  left: 2px;
  bottom: 2px;
  background-color: white;
  transition: .3s;
  border-radius: 50%;
  box-shadow: 0 1px 2px rgba(0,0,0,0.2);
}

.sidebar-switch input:checked + .slider {
  background-color: var(--accent-green);
}

.sidebar-switch input:checked + .slider:before {
  transform: translateX(12px);
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
  min-width: 0;
  background: color-mix(in srgb, var(--surface-strong) 88%, var(--glass-bg-heavy));
}

.settings-body {
  padding: 26px 34px 34px;
  overflow-y: auto;
}

.setting-group {
  margin-bottom: 34px;
  padding-bottom: 28px;
  border-bottom: 1px solid color-mix(in srgb, var(--text-muted) 13%, transparent);
}

.setting-group:last-child {
  margin-bottom: 0;
  padding-bottom: 0;
  border-bottom: 0;
}

.setting-group h4 {
  margin: 0 0 18px 0;
  font-size: 14px;
  color: var(--text-main);
  font-weight: 700;
  border-left: 0;
  padding-left: 0;
  letter-spacing: 0;
}

.setting-item {
  display: grid;
  grid-template-columns: 170px minmax(0, 1fr);
  column-gap: 24px;
  align-items: start;
  margin-bottom: 18px;
}

.setting-item:last-child {
  margin-bottom: 0;
}

.setting-item label {
  display: flex;
  align-items: center;
  min-height: 38px;
  margin-bottom: 0;
  font-size: 13px;
  font-weight: 600;
  color: var(--text-main);
  line-height: 1.35;
}

.setting-item input, .setting-item select {
  grid-column: 2;
  width: 100%;
  min-height: 38px;
  padding: 8px 12px;
  background: color-mix(in srgb, var(--surface-strong) 70%, var(--glass-bg-light));
  border: 1px solid color-mix(in srgb, var(--text-muted) 20%, transparent);
  border-radius: 8px;
  color: var(--text-main);
  font-family: var(--font-mono);
  font-size: 13px;
  transition: all 0.2s;
  box-sizing: border-box;
  backdrop-filter: blur(8px);
  -webkit-backdrop-filter: blur(8px);
}

.setting-item input:focus, .setting-item select:focus {
  outline: none;
  border-color: var(--accent-blue);
  box-shadow: 0 0 0 3px rgba(59, 130, 246, 0.1);
  background: var(--surface-strong);
}

.switch-container {
  grid-column: 2;
  display: flex;
  align-items: center;
  gap: 10px;
  padding: 8px 12px;
  background: var(--glass-bg-light);
  border-radius: var(--radius-md);
  border: 1px solid var(--glass-border-subtle);
  width: fit-content;
  transition: background var(--transition-fast);
}

.switch-label {
  font-size: 13px;
  font-weight: 500;
  color: var(--text-main);
}

.capability-badges {
  grid-column: 2;
  display: flex;
  flex-wrap: wrap;
  gap: 8px;
  margin-top: 10px;
}

.badge {
  display: inline-flex;
  align-items: center;
  gap: 4px;
  padding: 4px 10px;
  border-radius: 20px;
  font-size: 11px;
  font-weight: 600;
  letter-spacing: 0.02em;
  white-space: nowrap;
  transition: all var(--transition-fast);
}

.badge-ok {
  background: rgba(16, 185, 129, 0.1);
  color: var(--accent-green);
  border: 1px solid rgba(16, 185, 129, 0.2);
}

.badge-think {
  background: rgba(139, 92, 246, 0.1);
  color: #a080f0;
  border: 1px solid rgba(139, 92, 246, 0.2);
}

.badge-none {
  background: rgba(100, 116, 139, 0.08);
  color: var(--text-muted);
  border: 1px solid rgba(100, 116, 139, 0.15);
}

.badge-no {
  background: rgba(239, 68, 68, 0.1);
  color: var(--accent-red);
  border: 1px solid rgba(239, 68, 68, 0.2);
}

.badge-info {
  background: rgba(59, 130, 246, 0.1);
  color: var(--accent-blue);
  border: 1px solid rgba(59, 130, 246, 0.2);
}

.badge-warn {
  background: rgba(245, 158, 11, 0.1);
  color: var(--accent-yellow);
  border: 1px solid rgba(245, 158, 11, 0.2);
}

.capability-note {
  grid-column: 2;
  font-size: 11px;
  color: var(--text-muted);
  margin-top: 6px;
  font-style: italic;
  line-height: 1.5;
}

.switch-disabled {
  opacity: 0.5;
  cursor: not-allowed;
}

.setting-desc {
  grid-column: 2;
  font-size: 12px;
  color: var(--text-muted);
  margin-top: 8px;
  line-height: 1.5;
  max-width: 620px;
}

.settings-footer {
  min-height: 64px;
  padding: 12px 24px;
  border-top: 1px solid var(--glass-border);
  display: flex;
  justify-content: space-between;
  align-items: center;
  gap: 18px;
  background: var(--surface-strong);
  backdrop-filter: blur(var(--glass-blur));
  -webkit-backdrop-filter: blur(var(--glass-blur));
}

.footer-actions {
  display: flex;
  justify-content: flex-end;
  align-items: center;
  flex-shrink: 0;
}

.save-btn {
  background: var(--accent-blue);
  color: var(--text-inverse);
  border: none;
  min-width: 148px;
  min-height: 38px;
  padding: 9px 22px;
  border-radius: var(--radius-md);
  cursor: pointer;
  font-size: 14px;
  font-weight: 600;
  transition: all var(--transition-fast);
  box-shadow: 0 4px 12px rgba(59, 130, 246, 0.25);
}

.save-btn:hover:not(:disabled) {
  background: var(--accent-blue-hover);
  transform: translateY(-1px);
  box-shadow: 0 6px 16px rgba(59, 130, 246, 0.35);
}

.save-btn:active:not(:disabled) {
  transform: translateY(0);
}

.save-btn:disabled {
  opacity: 0.5;
  cursor: not-allowed;
}

.status-msg {
  min-width: 0;
  flex: 1;
  font-size: 13px;
  font-weight: 600;
  color: var(--text-muted);
  white-space: nowrap;
  overflow: hidden;
  text-overflow: ellipsis;
}

.status-msg.error {
  color: var(--accent-red);
}

.status-msg.success {
  color: var(--accent-green);
}

.icon-btn {
  background: var(--glass-bg-light);
  border: 1px solid var(--glass-border-subtle);
  color: var(--text-muted);
  cursor: pointer;
  padding: 8px;
  border-radius: var(--radius-md);
  display: flex;
  align-items: center;
  justify-content: center;
  transition: all var(--transition-fast);
}

.icon-btn:hover {
  background: var(--glass-bg);
  border-color: var(--glass-border);
  color: var(--text-main);
}

@media (max-width: 860px) {
  .settings-modal {
    width: calc(100vw - 24px);
    height: calc(100vh - 24px);
  }

  .settings-sidebar {
    width: 220px;
    flex-basis: 220px;
  }

  .settings-body {
    padding: 22px 24px 30px;
  }

  .setting-item {
    grid-template-columns: 1fr;
    row-gap: 8px;
  }

  .setting-item label,
  .setting-item input,
  .setting-item select,
  .setting-desc,
  .capability-badges,
  .capability-note,
  .switch-container {
    grid-column: 1;
  }

  .setting-item label {
    min-height: auto;
  }
}
</style>
