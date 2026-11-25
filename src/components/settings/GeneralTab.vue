<!--
  # GeneralTab.vue — 常规设置页签

  从 SettingsPanel.vue 拆出的独立页签组件（与 PromptsTab / ToolsPanel / AboutTab 对齐）。
  负责外观、Agent 交互默认档位、Shell 执行环境、对话与消息、可靠性、窗口布局六组即时生效设置。
-->
<template>
  <div class="general-tab-content">
    <!--
      分区原则：卡片顺序 = 使用频率（外观最常动 → 窗口低频兜底）；
      每张卡回答用户找设置时的一个心理提问：
      「界面长什么样 / Agent 怎么干活 / 消息怎么处理 / 数据安不安全 / 窗口怎么摆」。
    -->
    <div class="setting-card">
      <!-- 外观：颜色、语言、字体、密度、消息气泡观感 -->
      <div class="card-header">
        <svg viewBox="0 0 24 24" width="18" height="18"><path fill="currentColor" d="M12,18V6L5,12L12,18M11,14.14L11,9.86L8.5,12L11,14.14M12,2A10,10 0 0,0 2,12A10,10 0 0,0 12,22A10,10 0 0,0 22,12A10,10 0 0,0 12,2M12,20A8,8 0 0,1 4,12A8,8 0 0,1 12,4A8,8 0 0,1 20,12A8,8 0 0,1 12,20Z"/></svg>
        <h4>{{ t('settings.general.appearance') }}</h4>
      </div>
      <div class="setting-item">
        <label>{{ t('settings.general.colorMode') }}</label>
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
          <span>{{ isDark ? t('settings.general.darkMode') : t('settings.general.lightMode') }}</span>
        </button>
      </div>
      <div class="setting-item">
        <label>{{ t('settings.general.language') }}</label>
        <div class="custom-select" :class="{ open: langMenuOpen }">
          <button class="custom-select-trigger" @click="langMenuOpen = !langMenuOpen">
            <span>{{ localeOptions[appLocale] }}</span>
            <svg viewBox="0 0 24 24" width="14" height="14" stroke="currentColor" stroke-width="2" fill="none" stroke-linecap="round" stroke-linejoin="round"><polyline points="6 9 12 15 18 9"></polyline></svg>
          </button>
          <div v-if="langMenuOpen" class="custom-select-menu">
            <div
              v-for="(label, value) in localeOptions"
              :key="value"
              class="custom-select-option"
              :class="{ active: appLocale === value }"
              @click="setAppLocale(value); langMenuOpen = false"
            >{{ label }}</div>
          </div>
        </div>
        <div class="setting-desc">{{ t('settings.general.languageDesc') }}</div>
      </div>
      <div class="setting-item">
        <label>{{ t('settings.general.fontSize') }}</label>
        <div class="display-mode-toggle">
          <button
            v-for="p in fontSizePresets"
            :key="p.value"
            class="display-mode-btn"
            :class="{ active: fontSize === p.value }"
            @click="setFontSize(p.value)"
          >{{ t(p.label) }}</button>
        </div>
        <div class="setting-desc">{{ t('settings.general.fontSizeDesc') }}</div>
      </div>
      <div class="setting-item">
        <label>{{ t('settings.general.agentMessageOpacity') }}</label>
        <div class="font-size-control">
          <div class="slider-track-wrap">
            <input type="range" min="0" max="100" :value="agentMessageOpacity" class="font-size-slider"
              :style="{ '--fill-pct': agentMessageOpacity + '%' }"
              @input="setAgentMessageOpacity(Number(($event.target as HTMLInputElement).value))" />
          </div>
          <span class="font-size-value">{{ agentMessageOpacity }}%</span>
        </div>
      </div>
      <div class="setting-item">
        <label>{{ t('settings.general.userMessageOpacity') }}</label>
        <div class="font-size-control">
          <div class="slider-track-wrap">
            <input type="range" min="0" max="100" :value="userMessageOpacity" class="font-size-slider"
              :style="{ '--fill-pct': userMessageOpacity + '%' }"
              @input="setUserMessageOpacity(Number(($event.target as HTMLInputElement).value))" />
          </div>
          <span class="font-size-value">{{ userMessageOpacity }}%</span>
        </div>
      </div>
    </div>

    <div class="setting-card">
      <!-- Agent 交互：视图模式、工作模式、审批、思考、反思——Agent 怎么干活、怎么呈现过程 -->
      <div class="card-header">
        <svg viewBox="0 0 24 24" width="18" height="18"><path fill="currentColor" d="M12,2A10,10 0 0,0 2,12A10,10 0 0,0 12,22A10,10 0 0,0 22,12A10,10 0 0,0 12,2M12,4A8,8 0 0,1 20,12A8,8 0 0,1 12,20A8,8 0 0,1 4,12A8,8 0 0,1 12,4M12,17A5,5 0 0,1 7,12A5,5 0 0,1 12,7A5,5 0 0,1 17,12A5,5 0 0,1 12,17Z"/></svg>
        <h4>{{ t('settings.general.agentInteraction') }}</h4>
      </div>
      <div class="setting-item">
        <label>{{ t('settings.general.audience') }}</label>
        <div class="display-mode-toggle">
          <button
            class="display-mode-btn"
            :class="{ active: agentAudience === 'user' }"
            @click="setAgentAudience('user')"
          >{{ t('settings.general.normal') }}</button>
          <button
            class="display-mode-btn"
            :class="{ active: agentAudience === 'developer' }"
            @click="setAgentAudience('developer')"
          >{{ t('settings.general.developer') }}</button>
        </div>
        <div class="setting-desc">{{ t('settings.general.audienceDesc') }}</div>
      </div>
      <div class="setting-item">
        <label>{{ t('settings.general.workMode') }}</label>
        <div class="display-mode-toggle">
          <button
            class="display-mode-btn"
            :class="{ active: agentWorkMode === 'edit' }"
            @click="setAgentWorkMode('edit')"
          >{{ t('settings.general.edit') }}</button>
          <button
            class="display-mode-btn"
            :class="{ active: agentWorkMode === 'plan' }"
            @click="setAgentWorkMode('plan')"
          >{{ t('settings.general.plan') }}</button>
        </div>
        <div class="setting-desc">{{ t('settings.general.workModeDesc') }}</div>
      </div>
      <div class="setting-item">
        <label>{{ t('settings.general.approvalMode') }}</label>
        <div class="display-mode-toggle">
          <button
            class="display-mode-btn"
            :class="{ active: agentApprovalMode === 'request_approval' }"
            @click="setAgentApprovalMode('request_approval')"
          >{{ t('settings.general.request_approval') }}</button>
          <button
            class="display-mode-btn"
            :class="{ active: agentApprovalMode === 'auto_approve' }"
            @click="setAgentApprovalMode('auto_approve')"
          >{{ t('settings.general.auto_approve') }}</button>
        </div>
        <div class="setting-desc">{{ t('settings.general.approvalModeDesc') }}</div>
      </div>
      <!--
        深度思考默认档位：与工作模式/权限档位并列的**设置默认值**（UiPreferences）。
        注意它只管「新建会话首次发消息时固化的值」，改它**不会**回溯影响已有会话
        （正因如此它必须与预设解耦——挂在预设上会导致改一个预设倒灌所有会话）。
      -->
      <div class="setting-item">
        <label>{{ t('settings.general.thinkingDefault') }}</label>
        <div class="display-mode-toggle">
          <button
            class="display-mode-btn"
            :class="{ active: thinkingDefault === 'follow_global' }"
            @click="setThinkingDefault('follow_global')"
          >{{ t('settings.general.thinkingFollowGlobal') }}</button>
          <button
            class="display-mode-btn"
            :class="{ active: thinkingDefault === 'on' }"
            @click="setThinkingDefault('on')"
          >{{ t('settings.general.thinkingOn') }}</button>
          <button
            class="display-mode-btn"
            :class="{ active: thinkingDefault === 'off' }"
            @click="setThinkingDefault('off')"
          >{{ t('settings.general.thinkingOff') }}</button>
        </div>
        <div class="setting-desc">{{ t('settings.general.thinkingDefaultDesc') }}</div>
      </div>
      <div class="setting-item">
        <label>{{ t('settings.general.reflectionMode') }}</label>
        <div class="custom-select" :class="{ open: reflectionMenuOpen }">
          <button
            class="custom-select-trigger"
            :title="t('settings.general.reflectionModeTooltip')"
            @click="reflectionMenuOpen = !reflectionMenuOpen"
          >
            <span>{{ t(reflectionMode === 'always' ? 'settings.general.reflectionAlways' : reflectionMode === 'off' ? 'settings.general.reflectionOff' : 'settings.general.reflectionSmart') }}</span>
            <svg viewBox="0 0 24 24" width="14" height="14" stroke="currentColor" stroke-width="2" fill="none" stroke-linecap="round" stroke-linejoin="round"><polyline points="6 9 12 15 18 9"></polyline></svg>
          </button>
          <div v-if="reflectionMenuOpen" class="custom-select-menu">
            <div
              class="custom-select-option"
              :class="{ active: reflectionMode === 'smart' }"
              @click="setReflectionMode('smart'); reflectionMenuOpen = false"
            >{{ t('settings.general.reflectionSmart') }}</div>
            <div
              class="custom-select-option"
              :class="{ active: reflectionMode === 'always' }"
              @click="setReflectionMode('always'); reflectionMenuOpen = false"
            >{{ t('settings.general.reflectionAlways') }}</div>
            <div
              class="custom-select-option"
              :class="{ active: reflectionMode === 'off' }"
              @click="setReflectionMode('off'); reflectionMenuOpen = false"
            >{{ t('settings.general.reflectionOff') }}</div>
          </div>
        </div>
        <div class="setting-desc">{{ t(`settings.general.reflection${reflectionMode === 'always' ? 'Always' : reflectionMode === 'off' ? 'Off' : 'Smart'}Desc`) }}</div>
      </div>
      <div class="setting-item">
        <label>{{ t('settings.general.expandExecutionWhileRunning') }}</label>
        <label class="toggle-switch">
          <input type="checkbox" :checked="expandExecutionWhileRunning" @change="setExpandExecutionWhileRunning(($event.target as HTMLInputElement).checked)" />
          <span class="toggle-slider"></span>
        </label>
        <div class="setting-desc">{{ t('settings.general.expandExecutionWhileRunningDesc') }}</div>
      </div>
    </div>

    <div class="setting-card">
      <!-- Shell 执行环境：解释器选择。探测只在应用启动时做一次，
           这里读的是启动期缓存，不再每次打开设置页都跑 git。 -->
      <div class="card-header">
        <h4>{{ t('settings.general.shellTitle') }}</h4>
      </div>
      <template v-if="shell">
        <div v-if="!shell.platformSupported" class="setting-item">
          <div class="setting-desc">{{ t('settings.general.shellUnsupported') }}</div>
        </div>
        <template v-else>
          <div class="setting-item">
            <label>{{ t('settings.general.shellMode') }}</label>
            <select v-model="shellMode">
              <option value="auto">{{ t('settings.general.shellAuto') }}</option>
              <option value="powershell">{{ t('settings.general.shellPowerShell') }}</option>
            </select>
            <button type="button" class="shell-save-btn" :disabled="shellSaving" @click="saveShell">
              {{ t('settings.general.shellSave') }}
            </button>
            <div class="setting-desc">
              {{ t('settings.general.shellHint') }}
              <span class="shell-restart-note">{{ t('settings.general.shellRestartHint') }}</span>
            </div>
          </div>
          <div v-if="shellMode === 'auto'" class="setting-item">
            <label>{{ t('settings.general.shellPath') }}</label>
            <input v-model="shellPath" type="text" :placeholder="t('settings.general.shellPathPlaceholder')" />
            <div class="setting-desc">
              <template v-if="shell.detectedBashPath">
                {{ t('settings.general.shellDetected', { path: shell.detectedBashPath }) }}
              </template>
              <template v-else>{{ t('settings.general.shellAutoFallback') }}</template>
            </div>
            <p v-if="shell.error" class="shell-error">{{ shell.error }}</p>
          </div>
        </template>
      </template>
    </div>

    <div class="setting-card">
      <!-- 对话与消息：消息流的表现方式与发消息时的处理 -->
      <div class="card-header">
        <svg viewBox="0 0 24 24" width="18" height="18"><path fill="currentColor" d="M20,2H4A2,2 0 0,0 2,4V22L6,18H20A2,2 0 0,0 22,16V4A2,2 0 0,0 20,2Z"/></svg>
        <h4>{{ t('settings.general.conversation') }}</h4>
      </div>
      <div class="setting-item">
        <label>{{ t('settings.general.autoScroll') }}</label>
        <label class="toggle-switch">
          <input type="checkbox" :checked="autoScroll" @change="setAutoScroll(($event.target as HTMLInputElement).checked)" />
          <span class="toggle-slider"></span>
        </label>
        <div class="setting-desc">{{ t('settings.general.autoScrollDesc') }}</div>
      </div>
      <!--
        图片压缩档位：全局偏好（UiPreferences），**所有预设共用**、不随预设切换而变。
        因此放在常规设置里，而不是模型预设页；改档位立刻生效，无需点保存。
      -->
      <div class="setting-item">
        <label>{{ t('settings.general.imageCompressTier') }}</label>
        <div class="display-mode-toggle">
          <button
            v-for="tier in imageCompressTiers"
            :key="tier.value"
            class="display-mode-btn"
            :class="{ active: imageCompressTier === tier.value }"
            :title="t(tier.tooltip)"
            @click="setImageCompressTier(tier.value)"
          >{{ t(tier.label) }}</button>
        </div>
        <div class="setting-desc">{{ t(`settings.general.imageCompressTier${imageCompressTier === 'eco' ? 'Eco' : imageCompressTier === 'hd' ? 'Hd' : 'Standard'}Desc`) }}</div>
      </div>
    </div>

    <div class="setting-card">
      <!-- 可靠性与数据：崩溃/断电场景下的会话数据保全 -->
      <div class="card-header">
        <svg viewBox="0 0 24 24" width="18" height="18"><path fill="currentColor" d="M12,2L4,5V11C4,16.25 7.4,21.15 12,22C16.6,21.15 20,16.25 20,11V5L12,2Z"/></svg>
        <h4>{{ t('settings.general.reliability') }}</h4>
      </div>
      <div class="setting-item">
        <label>{{ t('settings.general.crashProtection') }}</label>
        <label class="toggle-switch">
          <input type="checkbox" :checked="crashProtection" @change="setCrashProtection(($event.target as HTMLInputElement).checked)" />
          <span class="toggle-slider"></span>
        </label>
        <div class="setting-desc">{{ t('settings.general.crashProtectionDesc') }}</div>
      </div>
    </div>

    <div class="setting-card">
      <!-- 窗口与布局：监控窗口摆放与布局兜底 -->
      <div class="card-header">
        <svg viewBox="0 0 24 24" width="18" height="18"><path fill="currentColor" d="M21,16.5C21,16.88 20.79,17.21 20.47,17.38L12.57,21.82C12.41,21.94 12.21,22 12,22C11.79,22 11.59,21.94 11.43,21.82L3.53,17.38C3.21,17.21 3,16.88 3,16.5V7.5C3,7.12 3.21,6.79 3.53,6.62L11.43,2.18C11.59,2.06 11.79,2 12,2C12.21,2 12.41,2.06 12.57,2.18L20.47,6.62C20.79,6.79 21,7.12 21,7.5V16.5Z"/></svg>
        <h4>{{ t('settings.general.windowLayout') }}</h4>
      </div>
      <div class="setting-item">
        <label>{{ t('settings.general.agentPanelPosition') }}</label>
        <div class="display-mode-toggle">
          <button
            class="display-mode-btn"
            :class="{ active: agentPanelPosition === 'right' }"
            @click="setAgentPanelPosition('right')"
          >{{ t('settings.general.positionRight') }}</button>
          <button
            class="display-mode-btn"
            :class="{ active: agentPanelPosition === 'left' }"
            @click="setAgentPanelPosition('left')"
          >{{ t('settings.general.positionLeft') }}</button>
        </div>
        <div class="setting-desc">{{ t('settings.general.agentPanelPositionDesc') }}</div>
      </div>
      <div class="setting-item">
        <label>{{ t('settings.general.layoutManagement') }}</label>
        <button class="window-reset-btn" :disabled="actionLoading" @click="emit('reset-windows')">
          {{ t('settings.general.restoreLayout') }}
        </button>
        <div class="setting-desc">{{ t('settings.general.restoreLayoutDesc') }}</div>
      </div>
    </div>
  </div>
</template>

<script setup lang="ts">
import { ref, computed, watch, nextTick, onMounted, onUnmounted } from 'vue'
import { invoke } from '@tauri-apps/api/core'
import { useI18n } from 'vue-i18n'
import { useTheme } from '../../composables/useTheme'
import {
  usePreferences,
  FONT_SIZE_PRESETS,
  type AgentPanelPosition,
  type ImageCompressTier,
} from '../../composables/usePreferences'
import { useWindow } from '../../composables/useWindow'
import { showToast } from '../../composables/useToast'
import type { AgentUserMode, ShellMode, ShellResolution } from '../../types'
import type { ThinkingDefault } from '../../utils/thinking'

const props = defineProps<{
  visible: boolean
  actionLoading: boolean
}>()

const emit = defineEmits<{
  (e: 'reset-windows'): void
}>()

const { t, locale } = useI18n()
const { isDark, toggleTheme } = useTheme()
const uiPrefs = usePreferences()
const { notifyMonitorLocaleChanged } = useWindow()

const agentAudience = uiPrefs.agentAudience
const setAgentAudience = (val: 'user' | 'developer') => uiPrefs.setAgentAudience(val)
const agentWorkMode = uiPrefs.agentWorkMode
const agentApprovalMode = uiPrefs.agentApprovalMode

// 深度思考的**设置默认档位**（三值）。与其他设置默认值同理：只喂新建会话，
// 「跟随全局」的解析在后端按模型能力完成，前端不做推导。
const thinkingDefault = uiPrefs.thinkingDefault
const setThinkingDefault = (val: ThinkingDefault) => uiPrefs.setThinkingDefault(val)

/**
 * Shell 执行环境设置（原在「工具」页，已移到常规设置）。
 *
 * 探测只在应用启动时做一次，后端返回的是启动期缓存；本组件打开设置页时
 * 只做一次廉价的配置读取，不会再卡几秒。
 */
const shell = ref<ShellResolution | null>(null)
const shellMode = ref<ShellMode>('auto')
const shellPath = ref('')
const shellSaving = ref(false)

const loadShell = async () => {
  try {
    const res = await invoke<ShellResolution>('get_shell_settings')
    shell.value = res
    shellMode.value = res.mode
    shellPath.value = res.gitBashPath ?? ''
  } catch (err) {
    console.error('加载 Shell 设置失败:', err)
  }
}

const saveShell = async () => {
  shellSaving.value = true
  try {
    const res = await invoke<ShellResolution>('save_shell_settings', {
      settings: {
        mode: shellMode.value,
        gitBashPath: shellPath.value.trim() || null,
      },
    })
    shell.value = res
    showToast(t('settings.general.shellSaved'))
  } catch (err) {
    console.error('保存 Shell 设置失败:', err)
    showToast(t('settings.general.shellSaveError', { error: String(err) }), 'error')
  } finally {
    shellSaving.value = false
  }
}

/**
 * 设置页只写**默认值**，刻意不碰任何会话。
 *
 * 这三个值是三条独立通道：设置默认值 / 会话界面值 / session 表值。设置页改的是
 * "以后新会话用什么"，若顺手把当前会话也改了（旧行为），用户就丧失了"只改默认"的
 * 能力——而且当前会话本应由用户在该会话界面里自己拨。会话值的写入时机统一在
 * 发送消息时（`chat.ts ensureActiveSessionForSend` 消费 pending 落库）。
 */
const setAgentWorkMode = (val: AgentUserMode) => {
  uiPrefs.setAgentWorkMode(val)
}

/**
 * 权限档位**默认值**：请求审批（改动一律问）/ 帮我批准（只问风险操作）。
 *
 * 设置页刻意只给两档：更松的「完全批准」（full_approve）跳过删除/改名/命令的确认，
 * 依靠项目回滚快照与回收站兜底，只能由用户在**绑定项目的会话**里用输入栏选择器临时开启，
 * 不能作为全局默认（否则新建的独立会话会落在没有回滚依据的档位上）。
 */
const setAgentApprovalMode = (val: 'request_approval' | 'auto_approve') => {
  uiPrefs.setAgentApprovalMode(val)
}

const fontSize = computed(() => uiPrefs.fontSize)
const setFontSize = (val: number) => uiPrefs.setFontSize(val)
// 字号是离散偏好而非连续量：四档预设替代滑杆。
// 挡位数值一律取自 usePreferences 的唯一出处，这里只负责挂文案。
const fontSizePresets = [
  { value: FONT_SIZE_PRESETS[0], label: 'settings.general.sizeS' },
  { value: FONT_SIZE_PRESETS[1], label: 'settings.general.sizeM' },
  { value: FONT_SIZE_PRESETS[2], label: 'settings.general.sizeL' },
  { value: FONT_SIZE_PRESETS[3], label: 'settings.general.sizeXL' },
]

// 「运行中展开执行过程」：只管轮次运行期间的表现，本轮结束后一律收起
const expandExecutionWhileRunning = computed(() => uiPrefs.expandExecutionWhileRunning)
const setExpandExecutionWhileRunning = (val: boolean) => uiPrefs.setExpandExecutionWhileRunning(val)
const autoScroll = computed(() => uiPrefs.autoScroll)
const setAutoScroll = (val: boolean) => uiPrefs.setAutoScroll(val)
// 崩溃保护（实时保存）：纯后端口径开关，前端只负责落到偏好里
const crashProtection = computed(() => uiPrefs.crashProtection)
const setCrashProtection = (val: boolean) => uiPrefs.setCrashProtection(val)
const agentPanelPosition = computed(() => uiPrefs.agentPanelPosition)
const setAgentPanelPosition = (val: AgentPanelPosition) => uiPrefs.setAgentPanelPosition(val)
const agentMessageOpacity = computed(() => uiPrefs.agentMessageOpacity)
const setAgentMessageOpacity = (val: number) => uiPrefs.setAgentMessageOpacity(val)
const userMessageOpacity = computed(() => uiPrefs.userMessageOpacity)
const setUserMessageOpacity = (val: number) => uiPrefs.setUserMessageOpacity(val)
const reflectionMode = computed(() => uiPrefs.reflectionMode)
const setReflectionMode = (val: 'always' | 'smart' | 'off') => uiPrefs.setReflectionMode(val)

/**
 * 图片压缩三档。`title` 是**悬停用**的一句话说明（什么时候该选这一档），
 * 下方的 setting-desc 则是当前档位的展开描述 —— 两者分工不同，不要合并。
 */
const imageCompressTiers: {
  value: ImageCompressTier
  label: string
  tooltip: string
}[] = [
  { value: 'eco', label: 'settings.general.imageCompressTierEco', tooltip: 'settings.general.imageCompressTierEcoTooltip' },
  { value: 'standard', label: 'settings.general.imageCompressTierStandard', tooltip: 'settings.general.imageCompressTierStandardTooltip' },
  { value: 'hd', label: 'settings.general.imageCompressTierHd', tooltip: 'settings.general.imageCompressTierHdTooltip' },
]
const imageCompressTier = computed(() => uiPrefs.imageCompressTier)
const setImageCompressTier = (val: ImageCompressTier) => uiPrefs.setImageCompressTier(val)

const langMenuOpen = ref(false)
const reflectionMenuOpen = ref(false)
const localeOptions: Record<string, string> = { 'zh-CN': '简体中文', 'en-US': 'English' }
const appLocale = uiPrefs.locale
watch(appLocale, (val) => {
  locale.value = val
}, { immediate: true })
const setAppLocale = async (value: string) => {
  uiPrefs.setLocale(value as typeof appLocale.value)
  await notifyMonitorLocaleChanged(appLocale.value)
  await nextTick()
}

// 点击自定义下拉外部时自动收起菜单
const onWindowMouseUp = (e: MouseEvent) => {
  if (e.target instanceof HTMLElement && !e.target.closest('.custom-select')) {
    if (langMenuOpen.value) langMenuOpen.value = false
    if (reflectionMenuOpen.value) reflectionMenuOpen.value = false
  }
}

onMounted(() => window.addEventListener('mouseup', onWindowMouseUp))
onUnmounted(() => window.removeEventListener('mouseup', onWindowMouseUp))

// 打开设置页时读取一次 Shell 配置。后端只返回启动期缓存，不会触发探测。
watch(
  () => props.visible,
  (open) => {
    if (open) void loadShell()
  },
  { immediate: true },
)
</script>

<style scoped>
.general-tab-content {
  display: flex;
  flex-direction: column;
  gap: 24px;
}

.setting-card {
  background: transparent;
  border: none;
  border-radius: 0;
  padding: 0;
  box-shadow: none;
}

.card-header {
  display: flex;
  align-items: center;
  gap: 10px;
  margin-bottom: 2px;
  color: var(--text-faint);
  border-bottom: none;
  padding-bottom: 0;
}

.card-header svg {
  display: none;
}

.card-header h4 {
  margin: 0;
  font-size: 0.8rem;
  font-weight: 600;
  letter-spacing: 0.04em;
  color: var(--text-faint);
}

.setting-item {
  display: flex;
  flex-direction: row;
  flex-wrap: wrap;
  align-items: center;
  column-gap: 24px;
  row-gap: 6px;
  margin-bottom: 0;
  padding: 12px 2px;
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

.setting-item .setting-desc {
  flex-basis: 100%;
  order: 99;
}

.setting-desc {
  font-size: 0.8rem;
  color: var(--text-muted);
  line-height: 1.5;
}

/* Shell 执行环境：保存按钮与缓存提示 */
.shell-save-btn {
  flex: 0 0 auto;
  height: 32px;
  padding: 0 14px;
  border-radius: 8px;
  border: 1px solid var(--glass-border);
  background: var(--glass-bg-light);
  color: var(--text-main);
  font-size: 0.8rem;
  cursor: pointer;
  transition: border-color var(--transition-fast);
}
.shell-save-btn:hover:not(:disabled) {
  border-color: var(--accent-blue);
}
.shell-save-btn:disabled {
  opacity: 0.6;
  cursor: default;
}
.shell-restart-note {
  margin-left: 6px;
  color: var(--text-faint);
}
.shell-error {
  flex-basis: 100%;
  order: 99;
  margin: 0;
  color: var(--text-warning);
  font-size: 0.78rem;
}

.setting-item input, .setting-item select {
  flex: 0 1 320px;
  width: auto;
  min-width: 220px;
  height: 32px;
  padding: 0 12px;
  background-color: var(--glass-bg-light);
  border: 1px solid var(--glass-border);
  border-radius: 8px;
  color: var(--text-main);
  font-size: 0.8667rem;
  transition: border-color 0.2s, box-shadow 0.2s, background-color 0.2s;
}

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
  background-color: var(--glass-bg);
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

/* 自定义下拉 */
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

/* 透明度滑杆 */
.font-size-control {
  display: flex;
  align-items: center;
  gap: 10px;
}

.slider-track-wrap {
  flex: 1;
  max-width: 180px;
  display: flex;
  align-items: center;
}

.font-size-slider {
  width: 100%;
  height: 16px;
  -webkit-appearance: none;
  appearance: none;
  background: transparent;
  outline: none;
  cursor: pointer;
  margin: 0;
}

.font-size-slider::-webkit-slider-runnable-track {
  height: 4px;
  border-radius: 2px;
  background: linear-gradient(
    to right,
    var(--sel-fill) 0%,
    var(--sel-fill) var(--fill-pct, 50%),
    var(--glass-bg-light) var(--fill-pct, 50%),
    var(--glass-bg-light) 100%
  );
  border: 0.5px solid var(--glass-border-subtle);
}

.font-size-slider::-webkit-slider-thumb {
  -webkit-appearance: none;
  appearance: none;
  width: 14px;
  height: 14px;
  border-radius: 50%;
  background: var(--surface-strong);
  border: 2px solid var(--sel-fill);
  box-shadow: 0 1px 4px rgba(0, 0, 0, 0.12);
  margin-top: -5px;
  cursor: pointer;
  transition: box-shadow 0.2s ease, transform 0.15s ease;
}

.font-size-slider::-webkit-slider-thumb:hover {
  box-shadow: 0 1px 6px rgba(0, 0, 0, 0.2);
}

.font-size-slider::-webkit-slider-thumb:active {
  transform: scale(0.92);
}

.font-size-slider::-moz-range-track {
  height: 4px;
  border-radius: 2px;
  background: var(--glass-bg-light);
  border: 0.5px solid var(--glass-border-subtle);
}

.font-size-slider::-moz-range-progress {
  height: 4px;
  border-radius: 2px;
  background: var(--sel-fill);
}

.font-size-slider::-moz-range-thumb {
  width: 14px;
  height: 14px;
  border-radius: 50%;
  background: var(--surface-strong);
  border: 2px solid var(--sel-fill);
  box-shadow: 0 1px 4px rgba(0, 0, 0, 0.12);
  cursor: pointer;
}

.font-size-value {
  font-size: 0.8rem;
  font-weight: var(--fw-semibold);
  color: var(--text-main);
  background: var(--glass-bg);
  border: 1px solid var(--glass-border-subtle);
  border-radius: 6px;
  padding: 4px 10px;
  min-width: 44px;
  text-align: center;
  font-variant-numeric: tabular-nums;
  flex-shrink: 0;
}

/* 开关切换 */
.toggle-switch {
  position: relative;
  display: inline-block;
  width: 44px;
  height: 24px;
  cursor: pointer;
}

.toggle-switch input {
  opacity: 0;
  width: 0;
  height: 0;
}

.toggle-slider {
  position: absolute;
  top: 0;
  left: 0;
  right: 0;
  bottom: 0;
  background: var(--glass-bg-light);
  border: 1px solid var(--glass-border);
  border-radius: 12px;
  transition: all 0.2s;
}

.toggle-slider::before {
  content: "";
  position: absolute;
  height: 18px;
  width: 18px;
  left: 2px;
  bottom: 2px;
  background: var(--text-muted);
  border-radius: 50%;
  transition: all 0.2s;
}

.toggle-switch input:checked + .toggle-slider {
  background: var(--sel-fill);
  border-color: var(--sel-fill);
}

.toggle-switch input:checked + .toggle-slider::before {
  transform: translateX(20px);
  background: var(--surface-strong);
}

/* 按钮组样式 */
.display-mode-toggle {
  display: flex;
  background: var(--glass-bg-light);
  padding: 4px;
  border-radius: 8px;
  width: fit-content;
}

.display-mode-btn {
  padding: 6px 16px;
  border: none;
  background: transparent;
  border-radius: 6px;
  font-size: 0.8667rem;
  font-weight: 600;
  color: var(--text-muted);
  cursor: pointer;
  transition: all 0.2s;
}

.display-mode-btn.active {
  background: var(--sel-fill);
  color: var(--surface-strong);
  box-shadow: 0 2px 4px rgba(0, 0, 0, 0.1);
}

.theme-toggle-btn {
  display: flex;
  align-items: center;
  gap: 8px;
  width: fit-content;
  padding: 8px 16px;
  background: var(--glass-bg-light);
  border: 1px solid var(--glass-border);
  border-radius: 8px;
  color: var(--text-main);
  font-size: 0.8667rem;
  cursor: pointer;
}

.window-reset-btn {
  width: fit-content;
  padding: 8px 16px;
  background: transparent;
  border: 1px solid var(--glass-border);
  border-radius: 8px;
  color: var(--accent-red);
  font-size: 0.8667rem;
  font-weight: 600;
  cursor: pointer;
}
</style>
