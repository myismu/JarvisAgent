<template>
  <div v-if="modelValue" class="settings-overlay" @click.self="close">
    <div class="settings-modal">
      <div class="settings-header">
        <div class="header-title">
          <svg viewBox="0 0 24 24" width="20" height="20" class="header-icon">
            <path fill="currentColor" d="M12,15.5A3.5,3.5 0 0,1 8.5,12A3.5,3.5 0 0,1 12,8.5A3.5,3.5 0 0,1 15.5,12A3.5,3.5 0 0,1 12,15.5M19.43,12.97C19.47,12.65 19.5,12.33 19.5,12C19.5,11.67 19.47,11.34 19.43,11.03L21.54,9.37C21.73,9.22 21.78,8.95 21.66,8.73L19.66,5.27C19.54,5.05 19.27,4.96 19.05,5.05L16.56,6.05C16.04,5.66 15.47,5.34 14.86,5.08L14.47,2.44C14.43,2.21 14.23,2 14,2H10C9.77,2 9.57,2.21 9.53,2.44L9.14,5.08C8.53,5.34 7.96,5.66 7.44,6.05L4.95,5.05C4.73,4.96 4.46,5.05 4.34,5.27L2.34,8.73C2.21,8.95 2.27,9.22 2.46,9.37L4.57,11.03C4.53,11.34 4.5,11.67 4.5,12C4.5,12.33 4.53,12.65 4.57,12.97L2.46,14.63C2.27,14.78 2.21,15.05 2.34,15.27L4.34,18.73C4.46,18.95 4.73,19.03 4.95,18.95L7.44,17.94C7.96,18.34 8.53,18.66 9.14,18.92L9.53,21.56C9.57,21.79 9.77,22 10,22H14C14.23,22 14.43,21.79 14.47,21.56L14.86,18.92C15.47,18.66 16.04,18.34 16.56,17.94L19.05,18.95C19.27,19.03 19.54,18.95 19.66,18.73L21.66,15.27C21.78,15.05 21.73,14.78 21.54,14.63L19.43,12.97Z" />
          </svg>
          <h3>{{ t('settings.title') }}</h3>
        </div>
        <button class="icon-btn close-btn" @click="close" :title="t('settings.close')">
          <svg viewBox="0 0 24 24" width="20" height="20">
            <path fill="currentColor" d="M19 6.41L17.59 5 12 10.59 6.41 5 5 6.41 10.59 12 5 17.59 6.41 19 12 13.41 17.59 19 19 17.59 13.41 12z" />
          </svg>
        </button>
      </div>

      <div class="settings-container">
        <!-- 左侧导航 -->
        <div class="settings-sidebar">
          <div class="sidebar-nav">
            <div
              class="nav-item"
              :class="{ active: activeTab === 'general' }"
              @click="activeTab = 'general'"
            >
              <svg viewBox="0 0 24 24" width="16" height="16" fill="none" stroke="currentColor" stroke-width="1.9" stroke-linecap="round" stroke-linejoin="round">
                <line x1="4" y1="21" x2="4" y2="14" /><line x1="4" y1="10" x2="4" y2="3" />
                <line x1="12" y1="21" x2="12" y2="12" /><line x1="12" y1="8" x2="12" y2="3" />
                <line x1="20" y1="21" x2="20" y2="16" /><line x1="20" y1="12" x2="20" y2="3" />
                <line x1="1" y1="14" x2="7" y2="14" /><line x1="9" y1="8" x2="15" y2="8" />
                <line x1="17" y1="16" x2="23" y2="16" />
              </svg>
              <span>{{ t('settings.tabs.general') }}</span>
            </div>
            <div
              class="nav-item"
              :class="{ active: activeTab === 'presets' }"
              @click="activeTab = 'presets'"
            >
              <svg viewBox="0 0 24 24" width="16" height="16" fill="none" stroke="currentColor" stroke-width="1.9" stroke-linecap="round" stroke-linejoin="round">
                <path d="M21 16V8a2 2 0 0 0-1-1.73l-7-4a2 2 0 0 0-2 0l-7 4A2 2 0 0 0 3 8v8a2 2 0 0 0 1 1.73l7 4a2 2 0 0 0 2 0l7-4A2 2 0 0 0 21 16z" />
                <polyline points="3.27 6.96 12 12.01 20.73 6.96" />
                <line x1="12" y1="22.08" x2="12" y2="12" />
              </svg>
              <span>{{ t('settings.tabs.presets') }}</span>
            </div>
            <div
              class="nav-item"
              :class="{ active: activeTab === 'prompts' }"
              @click="activeTab = 'prompts'"
            >
              <svg viewBox="0 0 24 24" width="16" height="16" fill="none" stroke="currentColor" stroke-width="1.9" stroke-linecap="round" stroke-linejoin="round">
                <path d="M14 2H6a2 2 0 0 0-2 2v16a2 2 0 0 0 2 2h12a2 2 0 0 0 2-2V8z" />
                <polyline points="14 2 14 8 20 8" />
                <line x1="16" y1="13" x2="8" y2="13" />
                <line x1="16" y1="17" x2="8" y2="17" />
              </svg>
              <span>{{ t('settings.tabs.prompts') }}</span>
            </div>
            <div
              class="nav-item"
              :class="{ active: activeTab === 'tools' }"
              @click="activeTab = 'tools'"
            >
              <svg viewBox="0 0 24 24" width="16" height="16" fill="none" stroke="currentColor" stroke-width="1.9" stroke-linecap="round" stroke-linejoin="round">
                <path d="M14.7 6.3a1 1 0 0 0 0 1.4l1.6 1.6a1 1 0 0 0 1.4 0l3.77-3.77a6 6 0 0 1-7.94 7.94l-6.91 6.91a2.12 2.12 0 0 1-3-3l6.91-6.91a6 6 0 0 1 7.94-7.94l-3.76 3.76z" />
              </svg>
              <span>{{ t('settings.tabs.tools') }}</span>
            </div>
            <div
              class="nav-item"
              :class="{ active: activeTab === 'about' }"
              @click="activeTab = 'about'"
            >
              <svg viewBox="0 0 24 24" width="16" height="16" fill="none" stroke="currentColor" stroke-width="1.9" stroke-linecap="round" stroke-linejoin="round">
                <circle cx="12" cy="12" r="10" />
                <line x1="12" y1="16" x2="12" y2="12" />
                <line x1="12" y1="8" x2="12.01" y2="8" />
              </svg>
              <span>{{ t('settings.tabs.about') }}</span>
            </div>
          </div>

          <template v-if="activeTab === 'presets'">
            <div class="sidebar-divider"></div>
            <div class="sidebar-section-header">
              <span>{{ t('settings.profiles.section') }}</span>
              <button class="add-btn" @click="addProfile" :title="t('settings.profiles.add')">+</button>
            </div>
            <div class="profile-list">
              <div
                v-for="(profile, index) in draftConfig.profiles"
                :key="profile.id"
                class="profile-item"
                :class="{
                  'active': selectedProfileId === profile.id,
                  'is-running': draftConfig.activeProfileId === profile.id,
                  'drag-over': dragIndex !== null && dragOverIndex === index && dragIndex !== index,
                  'dragging': dragIndex === index && dragMoved
                }"
                :style="dragIndex === index && dragMoved ? { transform: `translateY(${dragOffsetY}px)`, zIndex: 10 } : {}"
                @mousedown="onMouseDown($event, index)"
                @mousemove="onMouseMove($event)"
                @mouseup="onMouseUp($event, index)"
              >
                <span class="profile-name">{{ profile.name }}</span>
                <div class="profile-actions">
                  <label class="sidebar-switch" :title="t('settings.profiles.setGlobal')" @click.stop>
                    <input
                      type="checkbox"
                      :checked="savedConfig.globalProfileId === profile.id"
                      :disabled="actionLoading"
                      @change="toggleGlobalProfile(profile.id)"
                    />
                    <span class="slider"></span>
                  </label>
                  <button
                    class="copy-btn"
                    :disabled="actionLoading"
                    @click.stop="copyProfile(profile.id)"
                    :title="t('settings.profiles.copy')"
                  >
                    <svg viewBox="0 0 24 24" width="14" height="14" stroke="currentColor" stroke-width="2" fill="none" stroke-linecap="round" stroke-linejoin="round"><rect x="9" y="9" width="13" height="13" rx="2" ry="2"/><path d="M5 15H4a2 2 0 0 1-2-2V4a2 2 0 0 1 2-2h9a2 2 0 0 1 2 2v1"/></svg>
                  </button>
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
          </template>
        </div>

        <!-- 右侧内容区域 -->
        <div class="settings-content">
          <div class="settings-body" :class="{ 'no-scroll': activeTab === 'prompts' }">
            <!-- 常规设置页 -->
            <div v-if="activeTab === 'general'" class="tab-content">
              <GeneralTab
                :visible="modelValue"
                :action-loading="actionLoading"
                @reset-windows="resetDefaultWindows"
              />
            </div>

            <!-- 提示词管理页：列表 + 编辑器 + 拼装预览（独立 tab，即时生效型，无"保存全部"按钮） -->
            <div v-else-if="activeTab === 'prompts'" class="prompts-tab-content">
              <PromptsTab />
            </div>

            <!-- 工具开关页：逐个启停工具。
                 与技能开关不同，改完只对**新会话**生效 —— 核心工具 schema 必须会话内
                 字节恒定，否则 prompt cache 失效。要立刻生效点页内「应用到当前会话」。 -->
            <div v-else-if="activeTab === 'tools'" class="tab-content">
              <ToolsPanel />
            </div>

            <div v-else-if="activeTab === 'about'" class="tab-content">
              <AboutTab />
            </div>

            <!-- 预设编辑页 -->
            <div v-else-if="activeTab === 'presets' && editingProfile" class="tab-content">
              <!-- 基本信息卡片 -->
              <div class="setting-card">
                <div class="card-header">
                  <svg viewBox="0 0 24 24" width="18" height="18"><path fill="currentColor" d="M12,4A4,4 0 0,1 16,8A4,4 0 0,1 12,12A4,4 0 0,1 8,8A4,4 0 0,1 12,4M12,14C16.42,14 20,15.79 20,18V20H4V18C4,15.79 7.58,14 12,14Z"/></svg>
                  <h4>{{ t('settings.profileEditor.identity') }}</h4>
                </div>
                <div class="setting-item">
                  <label>{{ t('settings.profileEditor.name') }}</label>
                  <input type="text" v-model="editingProfile.name" :placeholder="t('settings.profileEditor.namePlaceholder')" />
                </div>
              </div>

              <!-- 连接配置卡片 -->
              <div class="setting-card">
                <div class="card-header">
                  <svg viewBox="0 0 24 24" width="18" height="18"><path fill="currentColor" d="M12,2L4.5,20.29L5.21,21L12,18L18.79,21L19.5,20.29L12,2Z"/></svg>
                  <h4>{{ t('settings.profileEditor.apiConnection') }}</h4>
                </div>
                <div class="setting-item">
                  <label>{{ t('settings.profileEditor.apiFormat') }}</label>
                  <select v-model="editingProfile.config.apiFormat" class="format-select">
                    <option value="anthropic">{{ t('settings.profileEditor.anthropicNative') }}</option>
                    <option value="openai">{{ t('settings.profileEditor.openaiCompatible') }}</option>
                  </select>
                </div>
                <div class="setting-item">
                  <label>Base URL</label>
                  <input type="text" v-model="editingProfile.config.baseUrl" placeholder="https://..." />
                </div>
                <div class="setting-item">
                  <label>API Key</label>
                  <!-- 明文切换按钮常驻：保存后也能随时核对（以前只有输入时能看到自己刚敲的内容） -->
                  <div class="input-with-toggle">
                    <input
                      :type="showApiKey ? 'text' : 'password'"
                      v-model="editingProfile.config.apiKey"
                      placeholder="sk-..."
                      autocomplete="off"
                      spellcheck="false"
                    />
                    <button
                      type="button"
                      class="toggle-visibility"
                      :class="{ active: showApiKey }"
                      :title="showApiKey ? t('settings.profileEditor.hideApiKey') : t('settings.profileEditor.showApiKey')"
                      :aria-label="showApiKey ? t('settings.profileEditor.hideApiKey') : t('settings.profileEditor.showApiKey')"
                      :aria-pressed="showApiKey"
                      @click="showApiKey = !showApiKey"
                    >
                      <svg v-if="showApiKey" viewBox="0 0 24 24" width="16" height="16" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round">
                        <path d="M17.94 17.94A10.07 10.07 0 0 1 12 20c-7 0-11-8-11-8a18.45 18.45 0 0 1 5.06-5.94M9.9 4.24A9.12 9.12 0 0 1 12 4c7 0 11 8 11 8a18.5 18.5 0 0 1-2.16 3.19m-6.72-1.07a3 3 0 1 1-4.24-4.24"></path>
                        <line x1="1" y1="1" x2="23" y2="23"></line>
                      </svg>
                      <svg v-else viewBox="0 0 24 24" width="16" height="16" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round">
                        <path d="M1 12s4-8 11-8 11 8 11 8-4 8-11 8-11-8-11-8z"></path>
                        <circle cx="12" cy="12" r="3"></circle>
                      </svg>
                    </button>
                  </div>
                  <div class="setting-desc">{{ t('settings.profileEditor.apiKeyDesc') }}</div>
                </div>
              </div>

              <!-- 模型配置卡片 -->
              <div class="setting-card">
                <div class="card-header">
                  <svg viewBox="0 0 24 24" width="18" height="18"><path fill="currentColor" d="M12,2A10,10 0 0,1 22,12A10,10 0 0,1 12,22A10,10 0 0,1 2,12A10,10 0 0,1 12,2M12,4A8,8 0 0,0 4,12A8,8 0 0,0 12,20A8,8 0 0,0 20,12A8,8 0 0,0 12,4M12,6A6,6 0 0,1 18,12A6,6 0 0,1 12,18A6,6 0 0,1 6,12A6,6 0 0,1 12,6M12,8A4,4 0 0,0 8,12A4,4 0 0,0 12,16A4,4 0 0,0 16,12A4,4 0 0,0 12,8Z"/></svg>
                  <h4>{{ t('settings.profileEditor.modelSelection') }}</h4>
                </div>
                <div class="setting-item">
                  <label>{{ t('settings.profileEditor.mainModel') }}</label>
                  <input
                    type="text"
                    v-model="editingProfile.config.mainModel"
                    placeholder="claude-3-5-sonnet-..."
                    @input="onMainModelInput"
                  />
                  <!-- 能力徽章 -->
                  <div class="capability-badges" v-if="mainModelCaps !== null">
                    <span
                      v-if="editingMainStatus"
                      class="badge"
                      :class="statusBadgeClass(editingMainStatus)"
                    >{{ statusLabel(editingMainStatus) }}</span>
                    <span class="badge" :class="mainModelCaps ? 'badge-ok' : 'badge-none'">
                      <span>{{ mainModelCaps ? t('settings.profileEditor.recognized') : t('settings.profileEditor.unknownModel') }}</span>
                    </span>
                    <template v-if="mainModelCaps">
                      <span v-if="mainModelCaps.thinking" class="badge badge-think">{{ t('settings.profileEditor.thinking') }}</span>
                      <span v-if="mainModelCaps.vision" class="badge badge-ok">{{ t('settings.profileEditor.vision') }}</span>
                      <span class="badge badge-info">{{ t('settings.profileEditor.outputTokens', { count: mainModelCaps.maxTokens.toLocaleString() }) }}</span>
                    </template>
                  </div>
                  <!-- 生命周期提示：退役=红色阻断，即将下线=黄色告警，未能验证/旧名=中性说明 -->
                  <div
                    v-if="editingMainStatus"
                    class="setting-desc model-status-hint"
                    :class="'hint-' + editingMainStatus"
                  >{{ statusHint(editingMainStatus) }}</div>
                  <div class="setting-item" v-if="mainModelCaps !== null">
                    <label>输出上限 (max_tokens)</label>
                    <input
                      type="number"
                      v-model.number="editingProfile.config.maxTokens"
                      :placeholder="String(mainModelCaps?.maxTokens ?? '')"
                      min="1"
                      :max="mainModelCaps?.maxTokens || 32768"
                      step="1"
                    />
                    <div class="setting-desc">留空使用模型默认值（{{ mainModelCaps?.maxTokens?.toLocaleString() ?? '未知' }}），填写则覆盖</div>
                  </div>
                </div>
                <div class="setting-item">
                  <label>{{ t('settings.profileEditor.utilityModel') }}</label>
                  <input type="text" v-model="editingProfile.config.utilityModel" placeholder="claude-3-5-haiku-..." />
                  <!-- 工具代理模型同样受退役拦截约束（徽章紧跟输入框，与主模型版式一致） -->
                  <div class="capability-badges" v-if="editingUtilityStatus">
                    <span class="badge" :class="statusBadgeClass(editingUtilityStatus)">{{ statusLabel(editingUtilityStatus) }}</span>
                  </div>
                  <div
                    v-if="editingUtilityStatus"
                    class="setting-desc model-status-hint"
                    :class="'hint-' + editingUtilityStatus"
                  >{{ statusHint(editingUtilityStatus) }}</div>
                  <div class="setting-desc">{{ t('settings.profileEditor.utilityModelDesc') }}</div>
                </div>

                <!--
                  深度思考的默认档位曾放在这里（预设级）。已迁到「常规设置」：
                  预设级会让"改一个预设"倒灌所有未表态会话（NULL 每轮现读该预设），
                  而它本质是个**全局默认值**，与预设的采样参数不是同一层概念。
                -->

                <div class="advanced-toggle" @click="showAdvanced = !showAdvanced">
                  <span>{{ t('settings.profileEditor.advancedParams') }}</span>
                  <svg :class="{ rotated: showAdvanced }" viewBox="0 0 24 24" width="16" height="16"><path fill="currentColor" d="M7.41,8.58L12,13.17L16.59,8.58L18,10L12,16L6,10L7.41,8.58Z"/></svg>
                </div>

                <div v-show="showAdvanced" class="advanced-content">
                  <div class="setting-item">
                    <label>温度 (Temp)</label>
                    <input type="number" v-model.number="editingProfile.config.temperature" step="0.1" min="0" max="2" />
                  </div>
                  <div class="setting-item">
                    <label>Top-P</label>
                    <input type="number" v-model.number="editingProfile.config.topP" step="0.1" min="0" max="1" />
                  </div>
                  <div class="setting-item" v-if="editingProfile.config.apiFormat === 'anthropic'">
                    <label>Top-K</label>
                    <input type="number" v-model.number="editingProfile.config.topK" step="1" min="0" />
                  </div>
                </div>
              </div>

            </div>

            <div v-else class="empty-state">
              <svg viewBox="0 0 24 24" width="48" height="48"><path fill="currentColor" d="M12,2A10,10 0 0,1 22,12A10,10 0 0,1 12,22A10,10 0 0,1 2,12A10,10 0 0,1 12,2M12,4A8,8 0 0,0 4,12A8,8 0 0,0 12,20A8,8 0 0,0 20,12A8,8 0 0,0 12,4M12,6A6,6 0 0,1 18,12A6,6 0 0,1 12,18A6,6 0 0,1 6,12A6,6 0 0,1 12,6M12,8A4,4 0 0,0 8,12A4,4 0 0,0 12,16A4,4 0 0,0 16,12A4,4 0 0,0 12,8Z"/></svg>
              <p>{{ t('settings.profiles.empty') }}</p>
            </div>
          </div>
        </div>
      </div>

      <!--
        退役阻断提示：必须常驻可见。
        保存按钮被置灰时点不动，`save()` 里那句报错永远不会显示——
        用户只会看到"保存是灰的"，却不知道为什么。
      -->
      <div
        v-if="activeTab === 'presets' && retiredBlockLines.length > 0"
        class="retired-block-notice"
        role="alert"
      >
        <div v-for="(line, i) in retiredBlockLines" :key="i" class="retired-block-line">{{ line }}</div>
      </div>

      <div v-if="activeTab === 'presets' || statusMsg" class="settings-footer">
        <span class="status-msg" :class="{ 'error': isError, 'success': isSuccess }">{{ statusMsg }}</span>
        <div class="footer-actions">
          <!-- 有**本次新引入的**退役模型时置灰：点得动再报错不如一开始就不让点 -->
          <button v-if="activeTab === 'presets'" class="save-btn" @click="save" :disabled="isSaving || actionLoading || retiredBlockers.length > 0">
            {{ isSaving ? t('settings.actions.saving') : t('settings.actions.save') }}
          </button>
        </div>
      </div>
    </div>

    <ConfirmModal
      :open="!!deleteConfirm"
      :title="deleteConfirm?.title || ''"
      :message="deleteConfirm?.message || ''"
      :warning="deleteConfirm?.warning || ''"
      :confirm-text="t('settings.profiles.deleteConfirm')"
      :cancel-text="t('settings.actions.cancel')"
      confirm-kind="danger"
      :loading="actionLoading"
      @cancel="deleteConfirm = null"
      @confirm="confirmDeleteProfile"
    />
  </div>
</template>

<script setup lang="ts">
import { ref } from 'vue'
import { useI18n } from 'vue-i18n'
import { useWindow } from '../../composables/useWindow'
import { useProfilesEditor } from '../../composables/settings/useProfilesEditor'
import ConfirmModal from '../common/ConfirmModal.vue'
import GeneralTab from './GeneralTab.vue'
import PromptsTab from './PromptsTab.vue'
import ToolsPanel from './ToolsPanel.vue'
import AboutTab from './AboutTab.vue'

const { t } = useI18n()
const { resetWindowStates } = useWindow()

const props = defineProps<{
  modelValue: boolean
}>()

const emit = defineEmits<{
  (e: 'update:modelValue', value: boolean): void
}>()

const activeTab = ref<'general' | 'presets' | 'prompts' | 'tools' | 'about'>('general')

const {
  savedConfig,
  draftConfig,
  selectedProfileId,
  showAdvanced,
  showApiKey,
  isSaving,
  actionLoading,
  statusMsg,
  isError,
  isSuccess,
  deleteConfirm,
  editingProfile,
  mainModelCaps,
  retiredBlockers,
  retiredBlockLines,
  editingMainStatus,
  editingUtilityStatus,
  dragIndex,
  dragOverIndex,
  dragOffsetY,
  dragMoved,
  statusBadgeClass,
  statusLabel,
  statusHint,
  resetStatus,
  setErrorStatus,
  setSuccessStatus,
  onMainModelInput,
  addProfile,
  copyProfile,
  onMouseDown,
  onMouseMove,
  onMouseUp,
  toggleGlobalProfile,
  requestDeleteProfile,
  confirmDeleteProfile,
  close,
  save,
} = useProfilesEditor({
  open: () => props.modelValue,
  activeTab,
  onClose: () => emit('update:modelValue', false),
})

const resetDefaultWindows = async () => {
  if (actionLoading.value) return

  actionLoading.value = true
  resetStatus()
  try {
    await resetWindowStates()
    setSuccessStatus(t('settings.general.restoreLayoutSuccess'))
  } catch (e) {
    console.error('恢复默认窗口失败:', e)
    setErrorStatus(t('settings.general.restoreLayoutError', { error: String(e) }))
  } finally {
    actionLoading.value = false
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
  animation: fadeIn var(--transition-fast);
}

@keyframes fadeIn {
  from { opacity: 0; }
  to { opacity: 1; }
}

.settings-modal {
  /* 选中态实心底：小面积开关用纯 --text-main 视觉刚好，大面积实心会被衬得更黑，
     故混入更多底色（76/24）让两者看起来是同一种"黑" */
  --sel-fill: color-mix(in srgb, var(--text-main) 76%, var(--surface-strong));
  background: var(--surface-strong);
  border: 1px solid var(--glass-border);
  border-radius: var(--radius-lg);
  width: min(1080px, calc(100vw - 48px));
  max-width: 96vw;
  height: min(800px, calc(100vh - 48px));
  max-height: 92vh;
  display: flex;
  flex-direction: column;
  box-shadow: var(--shadow-lg);
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
  min-height: 48px;
  padding: 10px 18px 10px 20px;
  border-bottom: 1px solid var(--glass-border);
  background: var(--surface-strong);
}

.header-title {
  display: flex;
  align-items: center;
  gap: 10px;
  color: var(--text-main);
}

.header-icon {
  color: var(--text-main);
}

.settings-header h3 {
  margin: 0;
  font-size: 1rem;
  font-weight: 600;
  letter-spacing: 0.02em;
}

.settings-container {
  flex: 1;
  display: flex;
  overflow: hidden;
}

/* 侧边栏样式：与主界面侧栏同源（--bg-sidebar），拉出左右双区层级 */
.settings-sidebar {
  width: 236px;
  flex: 0 0 236px;
  border-right: 1px solid var(--glass-border-subtle);
  background: var(--bg-sidebar);
  display: flex;
  flex-direction: column;
}

.sidebar-nav {
  padding: 12px 10px;
  display: flex;
  flex-direction: column;
  gap: 2px;
}

.nav-item {
  display: flex;
  align-items: center;
  gap: 10px;
  padding: 8px 12px;
  border: 1px solid transparent;
  border-radius: var(--radius-md);
  color: var(--text-soft);
  font-size: 0.875rem;
  font-weight: 500;
  cursor: pointer;
  transition: background var(--transition-fast), border-color var(--transition-fast), color var(--transition-fast), box-shadow var(--transition-fast);
}

.nav-item:hover {
  background: var(--surface-1);
  color: var(--text-main);
}

.nav-item.active {
  background: var(--bg-panel);
  border-color: var(--glass-border-subtle);
  color: var(--text-main);
  font-weight: 600;
  box-shadow: 0 1px 2px rgba(0, 0, 0, 0.04);
}

.sidebar-divider {
  height: 1px;
  background: var(--glass-border);
  margin: 8px 12px;
}

.sidebar-section-header {
  padding: 12px 18px 8px;
  display: flex;
  justify-content: space-between;
  align-items: center;
  color: var(--text-muted);
  font-size: 0.75rem;
  font-weight: 600;
  text-transform: uppercase;
  letter-spacing: 0.05em;
}

.add-btn {
  background: var(--glass-bg-light);
  color: var(--text-main);
  border: 1px solid color-mix(in srgb, var(--text-main) 25%, transparent);
  width: 20px;
  height: 20px;
  border-radius: 4px;
  display: flex;
  align-items: center;
  justify-content: center;
  cursor: pointer;
  font-size: 0.9333rem;
  transition: all var(--transition-fast);
}

.add-btn:hover {
  background: color-mix(in srgb, var(--text-main) 10%, transparent);
  border-color: var(--text-main);
}

.profile-list {
  flex: 1;
  overflow-y: auto;
  padding: 4px 12px 12px;
}

.profile-item {
  padding: 8px 10px 8px 12px;
  border-radius: var(--radius-md);
  cursor: pointer;
  margin-bottom: 2px;
  display: flex;
  justify-content: space-between;
  align-items: center;
  gap: 8px;
  transition: all var(--transition-fast);
  color: var(--text-muted);
  font-size: 0.8667rem;
}

.profile-item:hover {
  background: var(--glass-bg-light);
  color: var(--text-main);
}

.profile-item.active {
  background: var(--bg-panel);
  border: 1px solid var(--glass-border-subtle);
  color: var(--text-main);
  font-weight: 600;
  box-shadow: 0 1px 2px rgba(0, 0, 0, 0.04);
}

.profile-name {
  white-space: nowrap;
  overflow: hidden;
  text-overflow: ellipsis;
  flex: 1;
}

.profile-actions {
  display: flex;
  align-items: center;
  gap: 6px;
}

.sidebar-switch {
  position: relative;
  display: inline-block;
  width: 24px;
  height: 14px;
}

.sidebar-switch input { opacity: 0; width: 0; height: 0; }
.sidebar-switch .slider {
  position: absolute;
  cursor: pointer;
  top: 0; left: 0; right: 0; bottom: 0;
  background-color: var(--border-color);
  transition: .3s;
  border-radius: 14px;
}

.sidebar-switch .slider:before {
  position: absolute;
  content: "";
  height: 10px; width: 10px;
  left: 2px; bottom: 2px;
  background-color: white;
  transition: .3s;
  border-radius: 50%;
}

.sidebar-switch input:checked + .slider { background-color: var(--sel-fill); }
.sidebar-switch input:checked + .slider:before { transform: translateX(10px); background-color: var(--surface-strong); }

.copy-btn, .delete-btn {
  background: transparent;
  border: none;
  color: var(--text-muted);
  opacity: 0;
  cursor: pointer;
  padding: 2px;
}

.profile-item:hover .copy-btn, .profile-item:hover .delete-btn { opacity: 1; }
.copy-btn:hover { color: var(--text-main); }
.delete-btn:hover { color: var(--accent-red); }

.profile-item { cursor: grab; }
.profile-item.dragging { opacity: 0.5; cursor: grabbing; }
.profile-item.drag-over {
  border-top: 2px solid var(--text-main);
}

/* 内容区域样式：微浅灰底托起纯白圆角分组卡片（macOS / Linear 设置页层级） */
.settings-content {
  flex: 1;
  min-width: 0;
  background: color-mix(in srgb, var(--bg-sidebar) 55%, var(--bg-panel));
}

.settings-body {
  height: 100%;
  padding: 22px 32px 28px;
  overflow-y: auto;
  overflow-x: hidden;
}

/* 提示词 tab 自带滚动（textarea 内滚），外层锁高度让编辑器占满可视区 */
.settings-body.no-scroll {
  overflow-y: hidden;
  display: flex;
  flex-direction: column;
}

.tab-content {
  display: flex;
  flex-direction: column;
  gap: 20px;
  max-width: 780px;
  margin: 0 auto;
}

/* 提示词 tab：占满整个内容区（编辑器要纵向空间），弹层以内容区为定位容器 */
.prompts-tab-content {
  position: relative;
  height: 100%;
  min-height: 0;
  max-width: 1100px;
  margin: 0 auto;
  width: 100%;
}

/* 分组卡片：纯白底 + 柔和细描边，行间发丝线分隔 */
.setting-card {
  background: var(--bg-panel);
  border: 1px solid var(--glass-border-subtle);
  border-radius: var(--radius-lg);
  padding: 6px 16px;
  box-shadow: 0 1px 2px rgba(0, 0, 0, 0.02);
}

/* 分区小标题：弱化处理（小字 + 弱色 + 字距），读作"分组标签"而非"卡片标题" */
.card-header {
  display: flex;
  align-items: center;
  gap: 8px;
  margin-bottom: 0;
  padding: 10px 2px 6px;
  color: var(--text-faint);
  border-bottom: none;
}

/* 行式布局里图标是噪音，分区靠文字分组即可 */
.card-header svg {
  display: none;
}

.card-header h4 {
  margin: 0;
  font-size: 0.75rem;
  font-weight: 600;
  letter-spacing: 0.04em;
  text-transform: uppercase;
  color: var(--text-faint);
}

/* 设置行：标签居左、控件居右；描述文字折到下一整行。
   行间用发丝线分隔，最后一行无线。 */
.setting-item {
  display: flex;
  flex-direction: row;
  flex-wrap: wrap;
  align-items: center;
  column-gap: 20px;
  row-gap: 6px;
  margin-bottom: 0;
  padding: 13px 2px;
  border-bottom: 1px solid var(--glass-border-subtle);
}

.setting-item:last-child {
  margin-bottom: 0;
  border-bottom: none;
}

.setting-item label {
  flex: 1 1 auto;
  min-width: 0;
  font-size: 0.8667rem;
  font-weight: 500;
  color: var(--text-main);
}

/* 描述折行到标签下方（整行宽），不与控件抢右侧空间 */
.setting-item .setting-desc {
  flex-basis: 100%;
  order: 99;
}

/* 嵌套设置行（预设页 max_tokens）与能力徽章：独占一整行，左基线严格对齐 */
.setting-item .setting-item {
  flex-basis: 100%;
  border-bottom: none;
  padding: 10px 0 0;
}

.setting-item > .capability-badges {
  flex-basis: 100%;
  order: 50;
  padding-left: 0;
}

/* 右侧控件区：纯白底 + 精细边框 + 统一 320px 右对齐，告别"灰底像禁用" */
.setting-item input:not([type="range"]):not([type="checkbox"]),
.setting-item select {
  flex: 0 0 320px;
  width: 320px;
  max-width: 100%;
  height: 32px;
  padding: 0 11px;
  background-color: var(--bg-panel);
  border: 1px solid var(--glass-border);
  border-radius: var(--radius-md);
  color: var(--text-main);
  font-size: 0.84rem;
  box-shadow: 0 1px 2px rgba(0, 0, 0, 0.03);
  transition: border-color var(--transition-fast), box-shadow var(--transition-fast), background-color var(--transition-fast);
}

/* 包装类控件（下拉/滑杆/开关组等）右对齐且不被压缩。
   注意 .toggle-switch 本身也是 label，必须整体覆盖 flex，否则会被标签规则撑开。 */
.setting-item > .custom-select,
.setting-item > .display-mode-toggle,
.setting-item > .toggle-switch,
.setting-item > .theme-toggle-btn,
.setting-item > .font-size-control {
  flex: 0 0 auto;
}

.setting-item > .custom-select {
  min-width: 200px;
}

.setting-item > .font-size-control {
  width: 220px;
}

.setting-item select {
  appearance: none;
  -webkit-appearance: none;
  background-image: url("data:image/svg+xml;charset=US-ASCII,%3Csvg%20xmlns%3D%22http%3A%2F%2Fwww.w3.org%2F2000%2Fsvg%22%20width%3D%2214%22%20height%3D%2214%22%20viewBox%3D%220%200%2024%2024%22%20fill%3D%22none%22%20stroke%3D%22%2364748b%22%20stroke-width%3D%222%22%20stroke-linecap%3D%22round%22%20stroke-linejoin%3D%22round%22%3E%3Cpolyline%20points%3D%226%209%2012%2015%2018%209%22%3E%3C%2Fpolyline%3E%3C%2Fsvg%3E");
  background-repeat: no-repeat;
  background-position: right 12px center;
  background-size: 14px;
  padding-right: 32px;
  cursor: pointer;
}

.setting-item select:hover {
  border-color: var(--text-soft);
  background-color: var(--bg-panel);
}

.format-select {
  flex: 0 0 320px !important;
  width: 320px !important;
}

/* API Key：与上下输入框保持完全一致的 320px 宽度，右边缘严丝合缝对齐 */
.input-with-toggle {
  position: relative;
  flex: 0 0 320px;
  width: 320px;
  max-width: 100%;
  display: flex;
  align-items: center;
}

.input-with-toggle input {
  flex: 1 1 auto !important;
  width: 100% !important;
  min-width: 0;
  padding-right: 38px;
}

.toggle-visibility {
  position: absolute;
  right: 6px;
  width: 28px;
  height: 28px;
  display: flex;
  align-items: center;
  justify-content: center;
  border: none;
  border-radius: 6px;
  background: transparent;
  color: var(--text-muted);
  cursor: pointer;
  transition: color 0.15s, background-color 0.15s;
}

.toggle-visibility:hover {
  color: var(--text-main);
  background-color: var(--glass-bg);
}

.toggle-visibility.active {
  color: var(--text-main);
}

/* ── 自定义下拉（替代原生 select）── */
.custom-select {
  position: relative;
  width: fit-content;
}

.custom-select-trigger {
  width: 100%;
  height: 34px;
  padding: 0 10px;
  padding-right: 28px;
  background-color: var(--glass-bg-light);
  border: 1px solid var(--glass-border);
  border-radius: 8px;
  color: var(--text-main);
  font-size: 0.8667rem;
  display: flex;
  align-items: center;
  justify-content: space-between;
  gap: 6px;
  cursor: pointer;
  transition: border-color 0.2s, box-shadow 0.2s, background-color 0.2s;
  white-space: nowrap;
}

.custom-select-trigger:hover {
  border-color: var(--text-soft);
  background-color: var(--glass-bg);
}

.custom-select.open .custom-select-trigger {
  border-color: var(--text-main);
  box-shadow: 0 0 0 3px color-mix(in srgb, var(--text-main) 10%, transparent);
}

.custom-select-trigger svg {
  flex-shrink: 0;
  color: var(--text-muted);
  transition: transform 0.2s;
}

.custom-select.open .custom-select-trigger svg {
  transform: rotate(180deg);
}

.custom-select-menu {
  position: absolute;
  top: calc(100% + 4px);
  left: 0;
  min-width: 100%;
  z-index: 110;
  background: var(--surface-strong);
  border: 1px solid color-mix(in srgb, var(--text-muted) 20%, transparent);
  border-radius: 8px;
  box-shadow: 0 8px 24px rgba(10, 10, 10, 0.12);
  overflow: hidden;
  padding: 4px;
  white-space: nowrap;
  animation: popIn var(--transition-fast);
}

.custom-select-option {
  padding: 8px 12px;
  border-radius: 6px;
  cursor: pointer;
  font-size: 0.8667rem;
  color: var(--text-main);
  transition: background-color 0.15s;
}

.custom-select-option:hover {
  background: color-mix(in srgb, var(--text-main) 8%, transparent);
}

.custom-select-option.active {
  background: color-mix(in srgb, var(--text-main) 14%, transparent);
  color: var(--text-main);
  font-weight: 600;
}

:global(body.dark-mode) .setting-item select {
  background-image: url("data:image/svg+xml;charset=US-ASCII,%3Csvg%20xmlns%3D%22http%3A%2F%2Fwww.w3.org%2F2000%2Fsvg%22%20width%3D%2214%22%20height%3D%2214%22%20viewBox%3D%220%200%2024%2024%22%20fill%3D%22none%22%20stroke%3D%22%23a1a1aa%22%20stroke-width%3D%222%22%20stroke-linecap%3D%22round%22%20stroke-linejoin%3D%22round%22%3E%3Cpolyline%20points%3D%226%209%2012%2015%2018%209%22%3E%3C%2Fpolyline%3E%3C%2Fsvg%3E");
}

.setting-item select option {
  background: var(--surface-strong);
  color: var(--text-main);
}

.setting-item input:focus, .setting-item select:focus {
  outline: none;
  border-color: var(--text-main);
  box-shadow: 0 0 0 3px color-mix(in srgb, var(--text-main) 10%, transparent);
}

.setting-desc {
  font-size: 0.8rem;
  color: var(--text-muted);
  line-height: 1.5;
}

/* 高级参数折叠 */
.advanced-toggle {
  display: flex;
  align-items: center;
  gap: 8px;
  margin-top: 12px;
  padding: 10px 0;
  color: var(--text-main);
  font-size: 0.8667rem;
  font-weight: 600;
  cursor: pointer;
  border-top: 1px dashed var(--glass-border);
}

.advanced-toggle svg {
  transition: transform 0.2s;
}

.advanced-toggle svg.rotated {
  transform: rotate(180deg);
}

.advanced-content {
  padding-top: 12px;
  display: flex;
  flex-direction: column;
  gap: 14px;
}

/* 网格布局 */
.grid-3 {
  display: grid;
  grid-template-columns: repeat(3, 1fr);
  gap: 16px;
}

.sub-item {
  display: flex;
  flex-direction: column;
  gap: 6px;
}

.sub-item label {
  font-size: 0.7333rem;
  color: var(--text-muted);
}

/* 能力徽章 */
.capability-badges {
  display: flex;
  flex-wrap: wrap;
  gap: 6px;
  margin-top: 4px;
}

.badge {
  padding: 2px 8px;
  border-radius: 4px;
  font-size: 0.7333rem;
  font-weight: 600;
}

.badge-ok { background: color-mix(in srgb, var(--accent-green) 10%, transparent); color: var(--accent-green); }
.badge-think { background: color-mix(in srgb, var(--text-muted) 12%, transparent); color: var(--text-soft); }
.badge-info { background: color-mix(in srgb, var(--text-main) 10%, transparent); color: var(--text-main); }
.badge-none { background: color-mix(in srgb, var(--text-muted) 10%, transparent); color: var(--text-muted); }

/* ── 模型生命周期徽章 ──
   retired 用红色强调：它是唯一会阻断保存的状态。
   deprecated 用琥珀色告警（尚未生效）。
   unverifiable 刻意**不用红色**，改用灰色 + 虚边表达"我们不确定"——
   它和 retired 不是一回事（见 registry.rs 里 status 字段的文档：
   查不到 ≠ 已退役，同 thinking.ts 的「未知 ≠ 不支持」）。 */
.badge-retired {
  background: color-mix(in srgb, var(--accent-red) 14%, transparent);
  color: var(--accent-red);
  border: 1px solid color-mix(in srgb, var(--accent-red) 35%, transparent);
}

.badge-deprecated {
  background: var(--surface-warning);
  color: #b45309;
  border: 1px solid color-mix(in srgb, #d97706 35%, transparent);
}

body.dark-mode .badge-deprecated { color: #fbbf24; }

.badge-unverifiable {
  /* 原为 rgba(100, 116, 139, …) —— 那是 slate-500 的 alpha 写法，带蓝味；
     上一轮去蓝只搜了 rgba(15,23,42) 与十六进制，漏掉了这种形式。
     这里改用与相邻徽章同源的 --text-muted（现已中性化），保持「同色系不同浓度」的层级。 */
  background: color-mix(in srgb, var(--text-muted) 8%, transparent);
  color: var(--text-muted);
  border: 1px dashed color-mix(in srgb, var(--text-muted) 35%, transparent);
}

/* 生命周期提示行：与徽章同色系但更弱，避免和"能力徽章"抢注意力 */
.model-status-hint {
  margin-top: 4px;
  line-height: 1.5;
}
.model-status-hint.hint-retired { color: var(--accent-red); }
.model-status-hint.hint-deprecated { color: #b45309; }
body.dark-mode .model-status-hint.hint-deprecated { color: #fbbf24; }
.model-status-hint.hint-unverifiable,
.model-status-hint.hint-alias { color: var(--text-muted); }

/* 退役阻断提示条：贴在 footer 上方，按钮置灰时唯一能解释原因的地方 */
.retired-block-notice {
  padding: 8px 14px;
  background: color-mix(in srgb, var(--accent-red) 8%, transparent);
  border-top: 1px solid color-mix(in srgb, var(--accent-red) 25%, transparent);
  color: var(--accent-red);
  font-size: 0.8rem;
  line-height: 1.6;
}
.retired-block-line + .retired-block-line { margin-top: 4px; }

.empty-state {
  height: 100%;
  display: flex;
  flex-direction: column;
  align-items: center;
  justify-content: center;
  color: var(--text-muted);
  gap: 16px;
}

.settings-footer {
  min-height: 52px;
  padding: 10px 20px;
  border-top: 1px solid var(--glass-border);
  display: flex;
  justify-content: space-between;
  align-items: center;
  background: var(--surface-strong);
}

.save-btn {
  background: var(--sel-fill);
  color: var(--surface-strong);
  border: none;
  padding: 7px 18px;
  border-radius: 8px;
  font-size: 0.8667rem;
  font-weight: 600;
  cursor: pointer;
  transition: all 0.2s;
}

.save-btn:hover:not(:disabled) {
  opacity: 0.9;
}

.save-btn:disabled {
  opacity: 0.5;
  cursor: not-allowed;
}

.status-msg {
  font-size: 0.8667rem;
  font-weight: 500;
}

.status-msg.error { color: var(--accent-red); }
.status-msg.success { color: var(--accent-green); }

.icon-btn {
  background: transparent;
  border: none;
  color: var(--text-muted);
  cursor: pointer;
  padding: 4px;
  border-radius: 4px;
  display: flex;
  transition: all 0.2s;
}

.icon-btn:hover {
  background: var(--glass-bg-light);
  color: var(--text-main);
}
</style>

