<!--
# TerminalInput.vue — 主输入区

消息输入、附件管理、工作模式与审批模式切换、发送/停止按钮。

## Key Exports
- 默认导出组件：会话输入栏

## Constraints
- 停止按钮为中性色（中断非破坏性操作）；模式圆点统一中性灰
-->
<script setup lang="ts">
import { computed, ref, onMounted, nextTick, onUnmounted, onBeforeUnmount, watch } from 'vue';
import { useI18n } from 'vue-i18n';
import { getCurrentWindow } from '@tauri-apps/api/window';
import { useSessionStore } from '../../stores/session';
import { useChatStore } from '../../stores/chat';
import { usePreferences, prefsReady } from '../../composables/usePreferences';
import { useTurnMetrics } from '../../composables/input/useTurnMetrics';
import { useMediaAttachments } from '../../composables/input/useMediaAttachments';
import { useProfileSwitcher } from '../../composables/input/useProfileSwitcher';
import { useSessionControlModes } from '../../composables/input/useSessionControlModes';
import { invoke } from '@tauri-apps/api/core';
import { listen, type UnlistenFn } from '@tauri-apps/api/event';
import ConfirmModal from '../common/ConfirmModal.vue';
import CacheHitTooltip from './CacheHitTooltip.vue';
import McpPromptPickerModal from './McpPromptPickerModal.vue';
import type { AgentApprovalMode, AgentUserMode, AgentWorkMode } from '../../types';

const { t } = useI18n();
const session = useSessionStore();
const chat = useChatStore();
const uiPrefs = usePreferences();

const userInput = ref("");
const isDragging = ref(false);
const inputRef = ref<HTMLTextAreaElement | null>(null);
const showMcpPromptPicker = ref(false);

const insertMcpPromptDraft = (text: string) => {
  userInput.value = userInput.value.trim()
    ? `${userInput.value}\n\n${text}`
    : text;
  nextTick(() => {
    adjustHeight();
    inputRef.value?.focus();
  });
};

const isRunning = computed(
  () => session.getSessionView(session.activeSessionId).status === 'RUNNING',
);
const isStopping = computed(() => session.getSessionView(session.activeSessionId).stopping);
const showInterruptedResumeHint = computed(() => {
  const view = session.currentSessionView;
  return !isRunning.value && (view.status === "INTERRUPTED" || Boolean(view.resumableRunId));
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

// ── 概览栏指标（上下文占用、缓存命中、轮/步、吞吐量） ──
const {
  CONTEXT_RING_CIRCUMFERENCE,
  sessionTokenTotal,
  usageBreakdown,
  contextTokens,
  contextUsageLabel,
  contextRingDash,
  cacheUsage,
  currentOutputTokens,
  barCacheUsage,
  turnProgressLabel,
  throughputLabel,
  showCacheTip,
  showContextTip,
  openContextPanel,
} = useTurnMetrics();

// ── 预设切换、模型能力探测与深度思考联动 ──
const {
  appConfig,
  agentModel,
  showProfileMenu,
  profileMenuRef,
  showProfileCacheWarning,
  pendingProfileId,
  retiredSwitchNotice,
  retiredSwitchMessage,
  isThinkingForced,
  canModelThink,
  canModelVision,
  isThinkingActive,
  thinkingSaving,
  thinkingToggleDisabled,
  thinkingLocked,
  toggleThinking,
  loadConfig,
  switchProfile,
  confirmSwitchProfile,
  openProfileMenuLayout,
} = useProfileSwitcher();

// ── 图片/媒体附件与压缩管线 ──
const {
  mediaFiles,
  showVisionWarning,
  mediaTooltip,
  processDroppedFiles,
  handlePaste,
  removeMediaFile,
  clearMediaFiles,
  hideVisionWarning,
} = useMediaAttachments({
  canModelVision,
  userInput,
  inputRef,
  adjustHeight,
});

// ── 工作模式、权限档位、只读保护与压缩状态 ──
const {
  isCompacting,
  applyCompactingStatus,
  syncCompactingStatus,
  showWorkModeMenu,
  showApprovalMenu,
  showOptionsMenu,
  currentWorkMode,
  currentApprovalMode,
  agentReadOnly,
  workModeWarning,
  approvalModeOptions,
  toggleReadOnly,
  applyApprovalMode,
  applyWorkMode,
  syncPermissionFromSession,
  hideWorkModeWarning,
} = useSessionControlModes();

// ── 未发送草稿：按会话隔离 ──
const NEW_SESSION_DRAFT_KEY = '__new__';
const draftKeyOf = (sessionId: string | null) => sessionId || NEW_SESSION_DRAFT_KEY;

function restoreDraftForCurrentSession() {
  if (userInput.value) return;
  userInput.value = uiPrefs.getInputDraft(draftKeyOf(session.activeSessionId));
  nextTick(() => {
    adjustHeight();
  });
}

watch(prefsReady, (ready) => {
  if (ready) restoreDraftForCurrentSession();
}, { immediate: true });

watch(() => session.activeSessionId, (next, prev) => {
  uiPrefs.setInputDraft(draftKeyOf(prev), userInput.value);
  userInput.value = uiPrefs.getInputDraft(draftKeyOf(next));
  // 附件不跨会话，切换时释放 ObjectURL 并清空
  clearMediaFiles();
  nextTick(() => {
    adjustHeight();
  });
});

// 欢迎页快捷指令：一次性草稿，填入后清空并聚焦
watch(() => session.inputDraft, (draft) => {
  if (!draft) return;
  userInput.value = draft;
  session.inputDraft = null;
  nextTick(() => {
    inputRef.value?.focus();
    handleInput();
  });
});

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

const closeMenuOnOutsideClick = (e: MouseEvent) => {
  const target = e.target as HTMLElement;
  if (!target.closest('.profile-selector')) showProfileMenu.value = false;
  if (!target.closest('.work-mode-selector')) showWorkModeMenu.value = false;
  if (!target.closest('.approval-selector')) showApprovalMenu.value = false;
  if (!target.closest('.options-selector')) showOptionsMenu.value = false;
};

let unlistenDragDrop: (() => void) | null = null;
let unlistenConfig: UnlistenFn | null = null;
let unlistenWorkMode: UnlistenFn | null = null;
let unlistenApprovalMode: UnlistenFn | null = null;
let unlistenCompacting: UnlistenFn | null = null;
let unlistenSessionCompacted: UnlistenFn | null = null;

// 将输入区实际高度写入根级 CSS 变量 `--input-area-height`，供消息列表底部留白与回底按钮对齐
const inputAreaRef = ref<HTMLElement | null>(null);
let inputAreaObserver: ResizeObserver | null = null;

const syncInputAreaHeight = () => {
  const host = inputAreaRef.value?.closest('.floating-terminal-container') as HTMLElement | null;
  if (!host) return;
  const height = Math.round(host.getBoundingClientRect().height);
  if (height > 0) {
    document.documentElement.style.setProperty('--input-area-height', `${height}px`);
  }
};

onMounted(async () => {
  await loadConfig();
  await syncPermissionFromSession();
  if (!session.activeSessionId && appConfig.value?.globalProfileId) {
    appConfig.value.activeProfileId = appConfig.value.globalProfileId;
    await invoke('save_config_cmd', { newConfig: appConfig.value });
  }

  unlistenConfig = await listen('config-updated', () => {
    void loadConfig();
  });
  unlistenWorkMode = await listen<{ from: string; to: string; reason: string }>(
    'agent-work-mode-changed',
    (event) => {
      currentWorkMode.value = event.payload.to as AgentWorkMode;
    },
  );
  unlistenApprovalMode = await listen<{ mode: AgentApprovalMode }>(
    'approval-mode-changed',
    (event) => {
      currentApprovalMode.value = event.payload.mode;
    },
  );
  unlistenCompacting = await listen<{ sessionId: string; compacting: boolean }>(
    'bg-compacting-changed',
    (event) => {
      if (event.payload?.sessionId) {
        applyCompactingStatus(event.payload.sessionId, event.payload.compacting);
      }
    },
  );
  unlistenSessionCompacted = await listen<{ sessionId: string }>('session-compacted', (event) => {
    if (event.payload?.sessionId) {
      applyCompactingStatus(event.payload.sessionId, false);
    }
  });
  if (session.activeSessionId) {
    await syncCompactingStatus(session.activeSessionId);
  }

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
    if (event.payload.type === 'drop' && event.payload.paths.length > 0) {
      void processDroppedFiles(event.payload.paths);
    }
  });
});

onMounted(() => {
  const host = inputAreaRef.value?.closest('.floating-terminal-container') as HTMLElement | null;
  syncInputAreaHeight();
  if (!host || typeof ResizeObserver === 'undefined') return;
  inputAreaObserver = new ResizeObserver(syncInputAreaHeight);
  inputAreaObserver.observe(host);
});

onUnmounted(() => {
  if (unlistenDragDrop) unlistenDragDrop();
  if (unlistenConfig) unlistenConfig();
  if (unlistenWorkMode) unlistenWorkMode();
  if (unlistenApprovalMode) unlistenApprovalMode();
  if (unlistenCompacting) unlistenCompacting();
  if (unlistenSessionCompacted) unlistenSessionCompacted();
});

onBeforeUnmount(() => {
  document.removeEventListener('click', closeMenuOnOutsideClick);
  inputAreaObserver?.disconnect();
  inputAreaObserver = null;
  document.documentElement.style.removeProperty('--input-area-height');
});

const handleKeydown = (e: KeyboardEvent) => {
  if (e.key === 'Enter' && !e.shiftKey) {
    e.preventDefault();
    if (!isRunning.value && !isCompacting.value) {
      handleSubmit();
    }
  }
};

const handleSubmit = () => {
  const msg = userInput.value.trim();
  if (msg || mediaFiles.value.length > 0) {
    const imageBase64List = mediaFiles.value
      .filter((m) => m.type === 'image' && m.base64)
      .map((m) => m.base64);
    const submittedMediaFiles = [...mediaFiles.value];
    const sendPromise = chat.sendToJarvis(msg, null, imageBase64List);
    userInput.value = '';
    uiPrefs.setInputDraft(draftKeyOf(session.activeSessionId), '');
    mediaFiles.value = [];
    showVisionWarning.value = false;
    void sendPromise.then((outcome) => {
      const retryDraft = outcome?.retryDraft;
      if (retryDraft) {
        const retrySessionId = retryDraft.sessionId;
        const retryDraftKey = draftKeyOf(retrySessionId);
        if (retrySessionId === session.activeSessionId) {
          userInput.value = userInput.value
            ? `${retryDraft.text}\n\n${userInput.value}`
            : retryDraft.text;
          uiPrefs.setInputDraft(retryDraftKey, userInput.value);
          mediaFiles.value = [...submittedMediaFiles, ...mediaFiles.value];
          showVisionWarning.value = mediaFiles.value.length > 0;
          nextTick(() => {
            inputRef.value?.focus();
            adjustHeight();
          });
          return;
        }
        uiPrefs.setInputDraft(retryDraftKey, retryDraft.text);
      }
      submittedMediaFiles.forEach((m) => {
        if (m.url) URL.revokeObjectURL(m.url);
      });
    });
    nextTick(() => {
      adjustHeight();
    });
  }
};

const handleCancel = async () => {
  if (isStopping.value) return;
  await chat.cancelJarvis(session.activeSessionId ?? undefined);
};

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
  <div class="chat-input-container" ref="inputAreaRef">
    <div class="chat-input-wrapper">
      
      <div v-if="chat.showRecallEdit" class="recall-edit-bar">
        <svg viewBox="0 0 24 24" width="14" height="14" stroke="currentColor" stroke-width="2" fill="none" stroke-linecap="round" stroke-linejoin="round">
          <polyline points="1 4 1 10 7 10"></polyline>
          <path d="M3.51 15a9 9 0 1 0 2.13-9.36L1 10"></path>
        </svg>
        <span>{{ t('input.recallHint') }}</span>
        <button class="recall-edit-btn" @click="handleRecallEdit">{{ t('input.recallEdit') }}</button>
        <button class="recall-dismiss-btn" @click="chat.dismissRecallEdit" :aria-label="t('common.close')">
          <svg viewBox="0 0 24 24" width="12" height="12" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round" aria-hidden="true">
            <line x1="18" y1="6" x2="6" y2="18"></line>
            <line x1="6" y1="6" x2="18" y2="18"></line>
          </svg>
        </button>
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

      <div v-if="mediaFiles.length > 0" class="media-preview-container">
        <div v-for="(media, index) in mediaFiles" :key="index" class="media-preview-item">
          <template v-if="media.type === 'image' && media.url">
            <img :src="media.url" class="media-thumbnail" alt="preview" :title="mediaTooltip(media)" />
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
          <span class="media-name" :title="mediaTooltip(media)">{{ media.path.split(/[/\\]/).pop() }}</span>
          <button class="remove-media-btn" @click.stop="removeMediaFile(index)" :title="t('input.remove')" :aria-label="t('input.remove')">
            <svg viewBox="0 0 24 24" width="10" height="10" fill="none" stroke="currentColor" stroke-width="2.5" stroke-linecap="round" stroke-linejoin="round" aria-hidden="true">
              <line x1="18" y1="6" x2="6" y2="18"></line>
              <line x1="6" y1="6" x2="18" y2="18"></line>
            </svg>
          </button>
        </div>
      </div>

      <div v-if="showVisionWarning" class="vision-warning">
        <svg viewBox="0 0 24 24" width="14" height="14" stroke="currentColor" stroke-width="2" fill="none" stroke-linecap="round" stroke-linejoin="round">
          <path d="M10.29 3.86L1.82 18a2 2 0 0 0 1.71 3h16.94a2 2 0 0 0 1.71-3L13.71 3.86a2 2 0 0 0-3.42 0z"></path>
          <line x1="12" y1="9" x2="12" y2="13"></line>
          <line x1="12" y1="17" x2="12.01" y2="17"></line>
        </svg>
        <span>{{ t('input.visionWarning') }}</span>
        <button class="warning-close-btn" @click="hideVisionWarning" :aria-label="t('common.close')">
          <svg viewBox="0 0 24 24" width="12" height="12" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round" aria-hidden="true">
            <line x1="18" y1="6" x2="6" y2="18"></line>
            <line x1="6" y1="6" x2="18" y2="18"></line>
          </svg>
        </button>
      </div>

      <div v-if="workModeWarning" class="vision-warning">
        <svg viewBox="0 0 24 24" width="14" height="14" stroke="currentColor" stroke-width="2" fill="none" stroke-linecap="round" stroke-linejoin="round">
          <path d="M10.29 3.86L1.82 18a2 2 0 0 0 1.71 3h16.94a2 2 0 0 0 1.71-3L13.71 3.86a2 2 0 0 0-3.42 0z"></path>
          <line x1="12" y1="9" x2="12" y2="13"></line>
          <line x1="12" y1="17" x2="12.01" y2="17"></line>
        </svg>
        <span>{{ workModeWarning }}</span>
        <button class="warning-close-btn" @click="hideWorkModeWarning" :aria-label="t('common.close')">
          <svg viewBox="0 0 24 24" width="12" height="12" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round" aria-hidden="true">
            <line x1="18" y1="6" x2="6" y2="18"></line>
            <line x1="6" y1="6" x2="18" y2="18"></line>
          </svg>
        </button>
      </div>

      <!-- 输入区置顶：打开即见打字区，符合现代 AI Composer 视觉重心 -->
      <div class="input-row" @click="inputRef?.focus()">
        <textarea
          ref="inputRef"
          v-model="userInput"
          :placeholder="isCompacting ? '上下文压缩中，请稍候...' : t('input.placeholder')"
          class="editor-input"
          :class="{ 'input-disabled': isCompacting }"
          autofocus
          rows="1"
          :disabled="isCompacting"
          @input="handleInput"
          @keydown="handleKeydown"
          @paste="handlePaste"
        ></textarea>
      </div>

      <!-- 底部极简工具栏：左侧仅留高频（模型 + 工作模式），右侧收纳会话选项上拉面板与发送键 -->
      <div class="input-toolbar">
        <div class="profile-selector">
          <button
            class="profile-btn"
            @click="showWorkModeMenu = false; showOptionsMenu = false; showProfileMenu = !showProfileMenu; showProfileMenu && openProfileMenuLayout()"
          >
            <svg viewBox="0 0 24 24" width="13" height="13" stroke="currentColor" stroke-width="2.2" fill="none" stroke-linecap="round" stroke-linejoin="round" class="profile-icon">
              <path d="m12 3-1.912 5.813a2 2 0 0 1-1.275 1.275L3 12l5.813 1.912a2 2 0 0 1 1.275 1.275L12 21l1.912-5.813a2 2 0 0 1 1.275-1.275L21 12l-5.813-1.912a2 2 0 0 1-1.275-1.275L12 3Z"></path>
            </svg>
            {{ appConfig?.profiles.find((p: any) => p.id === appConfig?.activeProfileId)?.name || t('input.selectModel') }}
            <svg viewBox="0 0 24 24" width="11" height="11" stroke="currentColor" stroke-width="2" fill="none"><polyline points="6 9 12 15 18 9"></polyline></svg>
          </button>

          <div v-if="showProfileMenu" class="profile-menu" ref="profileMenuRef">
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

        <!-- 工作模式：编辑（直接干）/ 规划（先出方案），高频入口留在明面 -->
        <div class="work-mode-selector">
          <button
            class="work-mode-btn"
            @click="showProfileMenu = false; showOptionsMenu = false; showWorkModeMenu = !showWorkModeMenu"
          >
            <span class="work-mode-dot" :class="currentWorkMode"></span>
            <span class="work-mode-label">{{ t('settings.general.' + currentWorkMode) }}</span>
            <svg viewBox="0 0 24 24" width="10" height="10" stroke="currentColor" stroke-width="2" fill="none"><polyline points="6 9 12 15 18 9"></polyline></svg>
          </button>

          <div v-if="showWorkModeMenu" class="work-mode-menu">
            <div class="work-mode-menu-inner">
              <div
                v-for="mode in (['edit', 'plan'] as AgentUserMode[])"
                :key="mode"
                class="work-mode-menu-item"
                :class="{ active: currentWorkMode === mode }"
                @click="applyWorkMode(mode)"
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

        <div class="toolbar-spacer"></div>

        <div class="toolbar-right">
          <!-- 特殊安全状态微标（仅开启只读保护或完全批准时显现，平时零干扰） -->
          <button
            v-if="agentReadOnly || currentApprovalMode === 'full_approve'"
            type="button"
            class="safety-pill"
            :class="{ 'is-danger': currentApprovalMode === 'full_approve' && !agentReadOnly }"
            @click="showProfileMenu = false; showWorkModeMenu = false; showOptionsMenu = !showOptionsMenu"
          >
            <span class="safety-pill-dot"></span>
            <span>{{ agentReadOnly ? t('settings.general.readOnly') : t('settings.general.full_approve') }}</span>
          </button>

          <!-- 会话选项上拉面板：收纳审批权限、只读保护、深度思考 -->
          <div class="options-selector">
            <button
              type="button"
              class="options-trigger-btn"
              :class="{ open: showOptionsMenu }"
              @click="showProfileMenu = false; showWorkModeMenu = false; showOptionsMenu = !showOptionsMenu"
              :title="`${t('settings.general.' + currentApprovalMode)} · ${t('settings.general.readOnly')} · ${t('input.thinking')}`"
            >
              <svg viewBox="0 0 24 24" width="14" height="14" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round" aria-hidden="true">
                <line x1="4" y1="21" x2="4" y2="14" />
                <line x1="4" y1="10" x2="4" y2="3" />
                <line x1="12" y1="21" x2="12" y2="12" />
                <line x1="12" y1="8" x2="12" y2="3" />
                <line x1="20" y1="21" x2="20" y2="16" />
                <line x1="20" y1="12" x2="20" y2="3" />
                <line x1="1" y1="14" x2="7" y2="14" />
                <line x1="9" y1="8" x2="15" y2="8" />
                <line x1="17" y1="16" x2="23" y2="16" />
              </svg>
            </button>

            <div v-if="showOptionsMenu" class="options-popover">
              <!-- 1. 审批权限分段控制 -->
              <div class="options-section">
                <div class="options-section-title">{{ t('settings.general.approvalMode') }}</div>
                <div class="options-segmented">
                  <button
                    v-for="mode in approvalModeOptions"
                    :key="mode"
                    type="button"
                    class="options-seg-btn"
                    :class="{ active: currentApprovalMode === mode, 'is-danger': mode === 'full_approve' }"
                    @click="applyApprovalMode(mode)"
                  >
                    {{ t('settings.general.' + mode) }}
                  </button>
                </div>
                <div class="options-section-desc">{{ t('settings.general.' + currentApprovalMode + 'DescShort') }}</div>
              </div>

              <div class="options-divider"></div>

              <!-- 2. 只读保护开关行 -->
              <button
                type="button"
                class="options-toggle-row"
                :class="{ active: agentReadOnly }"
                @click="toggleReadOnly"
                :title="agentReadOnly ? t('settings.general.readOnlyOnTitle') : t('settings.general.readOnlyOffTitle')"
              >
                <span class="options-row-left">
                  <svg viewBox="0 0 24 24" width="14" height="14" fill="none" stroke="currentColor" stroke-width="1.9" stroke-linecap="round" stroke-linejoin="round">
                    <rect x="3" y="11" width="18" height="11" rx="2" />
                    <path d="M7 11V7a5 5 0 0 1 10 0v4" />
                  </svg>
                  <span class="options-row-label">{{ t('settings.general.readOnly') }}</span>
                </span>
                <span class="mini-switch" :class="{ on: agentReadOnly }" aria-hidden="true"></span>
              </button>

              <!-- 3. 深度思考开关行 -->
              <button
                type="button"
                class="options-toggle-row"
                :class="{
                  active: isThinkingActive,
                  disabled: thinkingToggleDisabled,
                  locked: thinkingLocked
                }"
                :disabled="thinkingToggleDisabled || thinkingSaving"
                @click="toggleThinking"
                :title="!canModelThink ? t('input.thinkingUnsupportedTitle') : isThinkingForced ? t('input.thinkingForcedTitle') : (isThinkingActive ? t('input.thinkingOnTitle') : t('input.thinkingOffTitle'))"
              >
                <span class="options-row-left">
                  <svg viewBox="0 0 24 24" width="14" height="14" fill="none" stroke="currentColor" stroke-width="1.9" stroke-linecap="round" stroke-linejoin="round">
                    <path d="M9.59 4.59A2 2 0 1 1 11 8H2m10.59 11.41A2 2 0 1 0 14 16H2m15.73-8.27a5 5 0 1 1-7.14 7.14" />
                  </svg>
                  <span class="options-row-label">{{ !canModelThink ? t('input.thinkingUnsupported') : isThinkingForced ? t('input.thinkingForced') : t('input.thinking') }}</span>
                  <svg
                    v-if="thinkingLocked"
                    class="thinking-lock-badge"
                    viewBox="0 0 24 24"
                    width="11"
                    height="11"
                    fill="none"
                    stroke="currentColor"
                    stroke-width="2.2"
                    stroke-linecap="round"
                    stroke-linejoin="round"
                    aria-hidden="true"
                  >
                    <rect x="5" y="11" width="14" height="10" rx="2" />
                    <path d="M8 11V7a4 4 0 0 1 8 0v4" />
                  </svg>
                </span>
                <span class="mini-switch" :class="{ on: isThinkingActive }" aria-hidden="true"></span>
              </button>
              <button
                type="button"
                class="toolbar-btn mcp-prompt-btn"
                :title="t('extensions.mcp.promptPicker.buttonTitle')"
                @click="showMcpPromptPicker = true"
              >
                <span>{{ t('extensions.mcp.promptPicker.buttonLabel') }}</span>
              </button>
            </div>
          </div>

          <button v-if="isCompacting" class="send-btn compacting-state" disabled :title="'上下文压缩中'">
            <svg class="spinner-icon" viewBox="0 0 24 24" width="15" height="15" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round">
              <circle cx="12" cy="12" r="10" stroke-opacity="0.25"></circle>
              <path d="M12 2a10 10 0 0 1 10 10"></path>
            </svg>
          </button>
          <button v-else-if="!isRunning && !isStopping" class="send-btn" :class="{ active: userInput.trim() || mediaFiles.length > 0 }" @click="handleSubmit" :title="t('input.send')">
            <svg viewBox="0 0 24 24" width="15" height="15" stroke="currentColor" stroke-width="2.2" fill="none" stroke-linecap="round" stroke-linejoin="round">
              <line x1="12" y1="19" x2="12" y2="5"></line>
              <polyline points="5 12 12 5 19 12"></polyline>
            </svg>
          </button>
          <button v-else class="send-btn active stop-state" @click="handleCancel" :title="t('input.stop')" :disabled="isStopping" :aria-busy="isStopping ? 'true' : undefined">
            <svg v-if="!isStopping" viewBox="0 0 24 24" width="14" height="14" fill="currentColor"><rect x="6" y="6" width="12" height="12" rx="2" /></svg>
            <svg v-else class="spinner-icon" viewBox="0 0 24 24" width="15" height="15" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round">
              <circle cx="12" cy="12" r="10"></circle>
              <path d="M12 2a10 10 0 0 1 10 10"></path>
            </svg>
          </button>
        </div>
      </div>

    </div>

    <McpPromptPickerModal
      v-model="showMcpPromptPicker"
      @insert="insertMcpPromptDraft"
    />

    <!-- 上下文与缓存遥测条：外置到输入框卡片正下方，让卡片本体保持两层轻盈 -->
    <div class="token-bar" v-if="contextTokens > 0 || sessionTokenTotal > 0">
        <!-- 上下文占用：概览栏只留一个进度环，读数收进悬停浮层（Codex / WorkBuddy 同款）。
             环的填充比例 = 已用 / 窗口；窗口未登记时只画灰轨道，浮层文字里注明「窗口未知」。

             刻意放在 `.token-bar-usage` **外面**：那一组整体 hover 会弹缓存明细浮层，
             环若嵌在里面，悬停时会同时冒出两个浮层、互相遮挡。 -->
        <span
          v-if="contextUsageLabel"
          class="token-bar-item token-bar-ring"
          @mouseenter="showContextTip = true"
          @mouseleave="showContextTip = false"
          @click="openContextPanel"
        >
          <svg class="context-ring" viewBox="0 0 24 24" width="13" height="13" aria-hidden="true">
            <circle class="ring-track" cx="12" cy="12" r="9" />
            <circle
              class="ring-fill"
              cx="12"
              cy="12"
              r="9"
              :stroke-dasharray="`${contextRingDash} ${CONTEXT_RING_CIRCUMFERENCE}`"
              transform="rotate(-90 12 12)"
            />
          </svg>
          <span v-if="showContextTip" class="context-tip" role="tooltip">
            {{ contextUsageLabel }}
          </span>
        </span>

        <!-- 缓存命中：hover 出明细浮层，点一下打开右侧上下文监控窗口 -->
        <span
          class="token-bar-usage"
          @mouseenter="showCacheTip = true"
          @mouseleave="showCacheTip = false"
          @click="openContextPanel"
        >
          <template v-if="barCacheUsage">
            <span v-if="contextTokens > 0" class="token-bar-sep">·</span>
            <span class="token-bar-item token-bar-cache" :class="`cache-${barCacheUsage.state}`">
              {{ barCacheUsage.label }}
            </span>
          </template>
          <CacheHitTooltip
            v-if="showCacheTip"
            :state="cacheUsage?.state ?? null"
            :percent="cacheUsage?.percent ?? null"
            :hit-tokens="cacheUsage?.hitTokens ?? null"
            :total-tokens="cacheUsage?.totalTokens ?? null"
            :output-tokens="currentOutputTokens"
            :session-input-tokens="session.totalInputTokens || 0"
            :session-output-tokens="session.totalOutputTokens || 0"
            :session-cache-hit-tokens="session.totalCacheHitTokens"
            :session-cache-miss-tokens="session.totalCacheMissTokens"
            :breakdown="usageBreakdown"
          />
        </span>
        <!-- 主 Agent 进度（第几轮 · 本轮第几次工具调用）+ 最近一次请求的吞吐。
             与左侧上下文/缓存同一行、同一层级的遥测读数；不可点、无浮层，
             所以只用 .token-bar-item 的基础排版，不套 .token-bar-usage
             （那个类带 cursor:pointer，会让人以为能点）。 -->
        <span v-if="turnProgressLabel || throughputLabel" class="token-bar-item token-bar-progress">
          <span v-if="contextTokens > 0 || barCacheUsage" class="token-bar-sep">·</span>
          <span v-if="turnProgressLabel">{{ turnProgressLabel }}</span>
          <span v-if="turnProgressLabel && throughputLabel" class="token-bar-sep">·</span>
          <span v-if="throughputLabel" class="token-bar-throughput">{{ throughputLabel }}</span>
        </span>
        <span class="token-bar-spacer"></span>
        <span class="token-bar-item token-bar-model">{{ agentModel }}</span>
      </div>
  </div>

  <ConfirmModal
    :open="showProfileCacheWarning"
    :title="t('settings.tabs.presets')"
    :message="t('settings.profiles.cacheWarning', { name: appConfig?.profiles.find((p: any) => p.id === pendingProfileId)?.name || '' })"
    confirm-kind="primary"
    @cancel="showProfileCacheWarning = false; pendingProfileId = null"
    @confirm="confirmSwitchProfile"
  />

  <!-- 切到退役模型的阻断提示：没有"继续"选项，只能去设置里改 -->
  <ConfirmModal
    :open="!!retiredSwitchNotice"
    :title="t('settings.profileEditor.statusRetired')"
    :message="retiredSwitchMessage"
    confirm-kind="primary"
    :confirm-text="t('common.confirm')"
    :cancel-text="t('common.close')"
    @cancel="retiredSwitchNotice = null"
    @confirm="retiredSwitchNotice = null"
  />
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
  max-width: min(85%, 960px); /* 与消息区同一套宽度口径，上下对齐 */
  background: var(--surface-strong);
  border: 1px solid var(--glass-border);
  border-radius: 18px;
  box-shadow: var(--shadow-md);
  display: flex;
  flex-direction: column;
  transition: border-color var(--transition-fast), box-shadow var(--transition-fast);
  position: relative;
  z-index: 1;
}

.chat-input-wrapper:hover {
  border-color: var(--border-strong);
}

.chat-input-wrapper:focus-within {
  border-color: var(--border-strong);
  box-shadow: var(--shadow-lg);
}

.input-toolbar {
  display: flex;
  justify-content: space-between;
  align-items: center;
  padding: 6px 10px 10px;
  flex-wrap: wrap;
  gap: 4px;
}

.profile-selector {
  position: relative;
}

.profile-btn {
  height: 28px;
  background: transparent;
  border: 1px solid transparent;
  border-radius: var(--radius-md);
  color: var(--text-main);
  padding: 0 9px;
  font-size: 0.8rem;
  font-weight: 600;
  display: inline-flex;
  align-items: center;
  gap: 5px;
  cursor: pointer;
  transition: background var(--transition-fast), border-color var(--transition-fast), color var(--transition-fast);
}

.profile-btn:hover {
  background: var(--surface-1);
  border-color: transparent;
}

.profile-icon {
  flex-shrink: 0;
  color: var(--accent-blue);
  opacity: 0.8;
}

.profile-menu {
  position: absolute;
  bottom: calc(100% + 12px);
  left: 0;
  background: var(--surface-strong);
  border: 1px solid color-mix(in srgb, var(--text-muted) 22%, transparent);
  border-radius: var(--radius-lg);
  box-shadow: var(--shadow-lg);
  min-width: 240px;
  /* 预设过多时不再无限向上撑高，改为在菜单内部滚动。
     取 min() 双保险：绝对高度上限 340px（约 4.5 项，暗示"下面还有"），
     同时不超过视口 55%，避免小窗口里菜单顶到窗口外被裁掉。 */
  max-height: min(340px, 55vh);
  overflow-y: auto;
  overflow-x: hidden;
  overscroll-behavior: contain; /* 滚到底不把滚动传给聊天区 */
  scrollbar-gutter: stable;     /* 有无滚动条时宽度不跳 */
  z-index: 100;
  animation: popIn var(--transition-fast);
  padding: 8px;
}

@keyframes popIn {
  from { opacity: 0; transform: translateY(8px) scale(0.96); }
  to { opacity: 1; transform: translateY(0) scale(1); }
}

.profile-menu-item {
  position: relative;
  padding: 10px 32px 10px 14px;
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
  background: color-mix(in srgb, var(--text-muted) 8%, transparent);
}

.profile-menu-item.active {
  background: color-mix(in srgb, var(--text-muted) 16%, transparent);
  border: 1px solid var(--glass-border);
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

/* 深度思考开关：中性色方案（N1）
   关闭 = 下沉面 + 细描边 + 中字重 + 弱文字
   开启 = 抬升面 + 常规边 + 粗描边 + 重字重 + 主色文字 + 微阴影
   主信号是"文字对比度跨度"（约 2.4 倍），面与边只作辅助——纯中性色下底色的
   可用对比度只有约 1.23:1，不足以独立承担状态区分。 */
.action-toggle-btn {
  height: 28px;
  display: inline-flex;
  align-items: center;
  gap: 5px;
  background: transparent;
  color: var(--text-muted);
  border: 1px solid transparent;
  border-radius: var(--radius-md);
  padding: 0 9px;
  font-size: 0.78rem;
  font-weight: 500;
  cursor: pointer;
  transition: background var(--transition-fast), border-color var(--transition-fast), color var(--transition-fast);
  box-shadow: none;
  position: relative;
}

.action-toggle-btn svg {
  stroke-width: 1.8;
  transition: stroke-width var(--transition-fast);
}

/* 关闭 · hover：统一 ghost 风格，轻底色反馈 */
.action-toggle-btn:hover {
  background: var(--surface-1);
  border-color: transparent;
  color: var(--text-main);
}

/* 开启（选中）：柔和底色 + 高对比文字 + 加粗，不堆硬边框 */
.action-toggle-btn.active {
  background: var(--surface-1);
  border-color: var(--glass-border-subtle);
  color: var(--thinking-on-fg);
  font-weight: 600;
  box-shadow: none;
}

.action-toggle-btn.active svg {
  stroke-width: 2.1;
}

.action-toggle-btn.active:hover {
  background: var(--surface-2);
  border-color: var(--glass-border);
}

/* 鼠标按下：项目通用按压反馈 */
.action-toggle-btn:active {
  transform: scale(0.97);
}

/* 不可点（不支持思考 / 模型强制开启、用户改不了）：
   必须是"未选中"外观，否则用户会以为它选着。 */
.action-toggle-btn.disabled {
  cursor: not-allowed;
  opacity: 0.65;
  transform: none;
}

.action-toggle-btn.disabled:not(.active) {
  background: var(--thinking-off-surface);
  border-style: dashed;
  border-color: var(--glass-border);
  color: var(--text-muted);
  font-weight: 500;
}

.action-toggle-btn.disabled:not(.active) svg {
  stroke-width: 1.7;
}

/* 锁定态（模型强制开启）：读作"开启"，只靠锁角标与虚线边说明"你改不了" */
.action-toggle-btn.locked {
  cursor: not-allowed;
}

.action-toggle-btn.locked:active {
  transform: none;
}

.action-toggle-btn.locked .thinking-lock-badge {
  flex-shrink: 0;
  color: var(--text-muted);
  opacity: 0.85;
  margin-left: 1px;
}

/* 迷你滑块开关：只承担"状态"信号，位置即状态（左=关 / 右=开）。
   刻意全中性色（与整个工具条的极简口径一致）：
   开 = 轨道填充 --text-main，滑块反白；
   关 = 下沉轨道 + 弱色滑块。不引入任何彩色。 */
.action-toggle-btn .mini-switch {
  flex: none;
  width: 22px;
  height: 12px;
  border-radius: 999px;
  border: 1px solid var(--glass-border);
  background: var(--thinking-off-surface);
  position: relative;
  transition: background var(--transition-fast), border-color var(--transition-fast);
}

.action-toggle-btn .mini-switch::after {
  content: "";
  position: absolute;
  top: 1px;
  left: 1px;
  width: 8px;
  height: 8px;
  border-radius: 50%;
  background: var(--text-muted);
  transition: transform var(--transition-fast), background var(--transition-fast);
}

.action-toggle-btn .mini-switch.on {
  background: var(--text-main);
  border-color: var(--text-main);
}

.action-toggle-btn .mini-switch.on::after {
  transform: translateX(10px);
  background: var(--bg-panel);
}

.toolbar-spacer {
  flex: 1;
}

.work-mode-selector {
  position: relative;
}

.work-mode-btn {
  height: 28px;
  background: transparent;
  border: 1px solid transparent;
  border-radius: var(--radius-md);
  color: var(--text-soft);
  padding: 0 9px;
  font-size: 0.78rem;
  font-weight: 500;
  display: inline-flex;
  align-items: center;
  gap: 5px;
  cursor: pointer;
  transition: background var(--transition-fast), border-color var(--transition-fast), color var(--transition-fast);
}

.work-mode-btn:hover {
  background: var(--surface-1);
  border-color: transparent;
  color: var(--text-main);
}

.work-mode-dot {
  width: 8px;
  height: 8px;
  border-radius: 50%;
  flex-shrink: 0;
}

.work-mode-dot.chat { background: var(--text-muted); }
.work-mode-dot.edit { background: var(--text-muted); }
.work-mode-dot.plan { background: var(--text-muted); }
.work-mode-dot.readonly { background: var(--text-muted); }
.work-mode-dot.request_approval { background: var(--text-muted); }
.work-mode-dot.auto_approve { background: var(--text-muted); }
.work-mode-dot.full_approve { background: var(--surface-danger); }
/* 完全批准是危险选项：标题用警示红，与普通档位区分 */
.work-mode-menu-name.is-danger,
.work-mode-label.is-danger {
  color: var(--surface-danger);
}

.work-mode-btn.readonly {
  border-color: var(--glass-border);
}

.work-mode-menu-hint {
  font-size: 0.68rem;
  color: var(--text-muted);
  padding: 4px 14px 2px;
}

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
  border: 1px solid color-mix(in srgb, var(--text-muted) 22%, transparent);
  border-radius: var(--radius-lg);
  box-shadow: var(--shadow-lg);
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
  background: color-mix(in srgb, var(--accent-blue) 12%, transparent);
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

.work-mode-menu-sep {
  height: 1px;
  margin: 6px 8px;
  background: color-mix(in srgb, var(--text-muted) 22%, transparent);
}

.work-mode-menu-item.readonly-item .work-mode-menu-text {
  flex: 1;
}

.work-mode-switch {
  width: 30px;
  height: 17px;
  border-radius: 999px;
  background: color-mix(in srgb, var(--text-muted) 35%, transparent);
  display: inline-flex;
  align-items: center;
  padding: 2px;
  flex-shrink: 0;
  transition: background var(--transition-fast);
}

.work-mode-switch.on {
  background: var(--accent-blue);
}

.work-mode-switch-knob {
  width: 13px;
  height: 13px;
  border-radius: 50%;
  background: #fff;
  transition: transform var(--transition-fast);
}

.work-mode-switch.on .work-mode-switch-knob {
  transform: translateX(13px);
}

.input-row {
  display: flex;
  align-items: flex-end;
  padding: 12px 14px 4px;
  cursor: text;
}

.editor-input {
  flex: 1;
  background: transparent;
  border: none;
  border-radius: var(--radius-md);
  color: var(--text-main);
  font-family: var(--font-sans);
  font-size: 0.9333rem;
  outline: none;
  resize: none;
  overflow-y: auto;
  scrollbar-width: none;
  line-height: 1.6;
  padding: 4px 6px 6px;
  margin: 0;
  max-height: 200px;
  transition: all var(--transition-fast);
}

.editor-input::-webkit-scrollbar {
  width: 0;
  height: 0;
}

.editor-input:focus {
  background: transparent;
  border: none;
  box-shadow: none;
}

.editor-input::placeholder {
  color: var(--text-faint);
  opacity: 0.9;
}

.editor-input.input-disabled {
  opacity: 0.45;
  cursor: not-allowed;
}

.send-btn {
  background: var(--surface-1);
  color: var(--text-faint);
  border: 1px solid transparent;
  border-radius: 9px;
  width: 30px;
  height: 30px;
  display: flex;
  align-items: center;
  justify-content: center;
  margin-left: 4px;
  cursor: pointer;
  transition: background var(--transition-fast), color var(--transition-fast), transform var(--transition-fast);
  flex-shrink: 0;
}

.send-btn:hover:not(:disabled) {
  background: var(--surface-2);
  color: var(--text-soft);
}

.send-btn.active {
  background: var(--btn-primary-bg);
  color: var(--btn-primary-fg);
  border-color: transparent;
}

.send-btn.active:hover {
  background: var(--btn-primary-bg-hover);
}

.send-btn.active:active {
  background: var(--btn-primary-bg-hover);
}

.send-btn.stop-state {
  background: var(--glass-bg-light);
  color: var(--text-soft);
  border-color: var(--border-color);
  box-shadow: none;
}

.send-btn.stop-state:hover {
  background: var(--border-color);
  color: var(--text-main);
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
  padding: 0;
  display: flex;
  align-items: center;
  justify-content: center;
  cursor: pointer;
  transition: all var(--transition-fast);
  flex-shrink: 0;
}

.remove-media-btn:hover {
  background: color-mix(in srgb, var(--accent-red) 90%, transparent);
  color: white;
}

.vision-warning {
  display: flex;
  align-items: center;
  gap: 8px;
  padding: 8px 16px;
  background: var(--glass-bg-light);
  border-top: 1px solid var(--border-color);
  color: var(--text-muted);
  font-size: 0.8rem;
}

.recall-edit-bar {
  display: flex;
  align-items: center;
  gap: 8px;
  padding: 8px 16px;
  background: color-mix(in srgb, var(--accent-blue) 8%, transparent);
  border-bottom: 1px solid color-mix(in srgb, var(--accent-blue) 15%, transparent);
  color: var(--accent-blue);
  font-size: 0.8rem;
  animation: slideDown 0.2s ease-out;
}

@keyframes slideDown {
  from { opacity: 0; transform: translateY(-4px); }
  to { opacity: 1; transform: translateY(0); }
}

.recall-edit-btn {
  background: color-mix(in srgb, var(--accent-blue) 15%, transparent);
  color: var(--accent-blue);
  border: 1px solid color-mix(in srgb, var(--accent-blue) 30%, transparent);
  border-radius: var(--radius-md);
  padding: 4px 12px;
  font-size: 0.75rem;
  font-weight: 600;
  cursor: pointer;
  transition: all var(--transition-fast);
  white-space: nowrap;
}

.recall-edit-btn:hover {
  background: var(--btn-primary-bg);
  color: var(--btn-primary-fg);
  border-color: transparent;
  transform: translateY(-1px);
}

.recall-dismiss-btn {
  background: none;
  border: none;
  color: var(--accent-blue);
  opacity: 0.85;
  cursor: pointer;
  padding: 2px 6px;
  border-radius: var(--radius-sm);
  transition: all var(--transition-fast);
  display: flex;
  align-items: center;
  justify-content: center;
}

.recall-dismiss-btn:hover {
  opacity: 1;
  background: color-mix(in srgb, var(--accent-blue) 15%, transparent);
}

/* 提示条右侧关闭按钮：与 remove-media-btn 同构，跟随提示文字颜色 */
.warning-close-btn {
  background: none;
  border: none;
  padding: 2px;
  margin-left: auto;
  color: inherit;
  opacity: 0.85;
  cursor: pointer;
  border-radius: var(--radius-sm);
  transition: all var(--transition-fast);
  display: flex;
  align-items: center;
  justify-content: center;
  flex-shrink: 0;
}

.warning-close-btn:hover {
  opacity: 1;
  background: color-mix(in srgb, var(--text-muted) 18%, transparent);
}

.resume-run-bar {
  display: flex;
  align-items: center;
  gap: 8px;
  padding: 8px 16px;
  background: var(--glass-bg-light);
  border-bottom: 1px solid var(--border-color);
  color: var(--text-muted);
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

/* ── 会话选项上拉面板（收纳权限、只读保护、深度思考）── */
.options-selector {
  position: relative;
}

.options-trigger-btn {
  width: 28px;
  height: 28px;
  display: inline-flex;
  align-items: center;
  justify-content: center;
  background: transparent;
  border: 1px solid transparent;
  border-radius: var(--radius-md);
  color: var(--text-muted);
  cursor: pointer;
  transition: background var(--transition-fast), color var(--transition-fast), border-color var(--transition-fast);
}

.options-trigger-btn:hover,
.options-trigger-btn.open {
  background: var(--surface-1);
  color: var(--text-main);
}

.safety-pill {
  height: 24px;
  display: inline-flex;
  align-items: center;
  gap: 5px;
  padding: 0 8px;
  border-radius: 999px;
  border: 1px solid var(--glass-border-subtle);
  background: var(--surface-1);
  color: var(--text-soft);
  font-size: 0.72rem;
  font-weight: 500;
  cursor: pointer;
  transition: background var(--transition-fast), border-color var(--transition-fast);
}

.safety-pill:hover {
  border-color: var(--glass-border);
}

.safety-pill-dot {
  width: 6px;
  height: 6px;
  border-radius: 50%;
  background: var(--text-muted);
}

.safety-pill.is-danger {
  border-color: var(--border-danger);
  color: var(--accent-red);
}

.safety-pill.is-danger .safety-pill-dot {
  background: var(--accent-red);
}

.options-popover {
  position: absolute;
  bottom: calc(100% + 10px);
  right: 0;
  width: 296px;
  padding: 10px;
  background: var(--surface-strong);
  border: 1px solid color-mix(in srgb, var(--text-muted) 22%, transparent);
  border-radius: var(--radius-lg);
  box-shadow: var(--shadow-lg);
  z-index: 120;
  display: flex;
  flex-direction: column;
  gap: 4px;
  animation: popIn var(--transition-fast);
}

.options-section {
  display: flex;
  flex-direction: column;
  gap: 6px;
  padding: 2px 2px 4px;
}

.options-section-title {
  font-size: 0.72rem;
  font-weight: 600;
  color: var(--text-faint);
  letter-spacing: 0.02em;
}

.options-segmented {
  display: flex;
  gap: 3px;
  padding: 3px;
  background: var(--surface-1);
  border-radius: var(--radius-md);
}

.options-seg-btn {
  flex: 1;
  height: 26px;
  border: none;
  border-radius: 6px;
  background: transparent;
  color: var(--text-muted);
  font-size: 0.75rem;
  font-weight: 500;
  cursor: pointer;
  transition: background var(--transition-fast), color var(--transition-fast), box-shadow var(--transition-fast);
  white-space: nowrap;
}

.options-seg-btn:hover {
  color: var(--text-main);
}

.options-seg-btn.active {
  background: var(--bg-panel);
  color: var(--text-main);
  font-weight: 600;
  box-shadow: var(--shadow-sm);
}

.options-seg-btn.active.is-danger {
  color: var(--accent-red);
}

.options-section-desc {
  font-size: 0.72rem;
  color: var(--text-muted);
  line-height: 1.4;
}

.options-divider {
  height: 1px;
  background: var(--glass-border-subtle);
  margin: 4px 0;
}

.options-toggle-row {
  width: 100%;
  display: flex;
  align-items: center;
  justify-content: space-between;
  gap: 10px;
  padding: 8px 8px;
  border: none;
  border-radius: var(--radius-md);
  background: transparent;
  color: var(--text-soft);
  font-size: 0.8rem;
  font-weight: 500;
  cursor: pointer;
  transition: background var(--transition-fast), color var(--transition-fast);
}

.options-toggle-row:hover:not(:disabled) {
  background: var(--surface-1);
  color: var(--text-main);
}

.options-toggle-row.active {
  color: var(--text-main);
  font-weight: 600;
}

.options-toggle-row.disabled,
.options-toggle-row.locked {
  cursor: not-allowed;
  opacity: 0.75;
}

.options-row-left {
  display: inline-flex;
  align-items: center;
  gap: 8px;
  min-width: 0;
}

.options-row-label {
  white-space: nowrap;
  overflow: hidden;
  text-overflow: ellipsis;
}

.options-toggle-row .mini-switch {
  flex: none;
  width: 24px;
  height: 13px;
  border-radius: 999px;
  border: 1px solid var(--glass-border);
  background: var(--thinking-off-surface);
  position: relative;
  transition: background var(--transition-fast), border-color var(--transition-fast);
}

.options-toggle-row .mini-switch::after {
  content: "";
  position: absolute;
  top: 1px;
  left: 1px;
  width: 9px;
  height: 9px;
  border-radius: 50%;
  background: var(--text-muted);
  transition: transform var(--transition-fast), background var(--transition-fast);
}

.options-toggle-row .mini-switch.on {
  background: var(--text-main);
  border-color: var(--text-main);
}

.options-toggle-row .mini-switch.on::after {
  transform: translateX(11px);
  background: var(--bg-panel);
}

/* ── Token 统计栏（外置到卡片正下方，与输入框等宽对齐）── */
.token-bar {
  display: flex;
  align-items: center;
  gap: 6px;
  width: 100%;
  max-width: min(85%, 960px);
  padding: 6px 14px 0;
  font-size: 0.68rem;
  color: var(--text-faint);
  user-select: none;
}

.token-bar-item {
  display: inline-flex;
  align-items: center;
  gap: 3px;
  white-space: nowrap;
}

.token-bar-item svg {
  opacity: 0.85;
}

.token-bar-sep {
  opacity: 0.35;
}

.token-bar-total {
  font-weight: var(--fw-semibold);
}

/* ── 上下文进度环（取代原来铺在概览栏上的一长串读数）── */

/* 环是浮层的定位上下文；`.token-bar-item svg { opacity: .5 }` 那条通用规则要盖掉，
   否则环会跟旁边的文字一样半透明，进度就看不出来了 */
.token-bar-ring {
  position: relative;
  cursor: pointer;
  padding: 0 3px;
  border-radius: 4px;
  transition: background-color 0.15s;
}

/* 与缓存读数同一套 hover 反馈：只给背景、不改字色 */
.token-bar-ring:hover {
  background: var(--glass-bg);
}

.token-bar-ring svg.context-ring {
  display: block;
  opacity: 1;
}

.ring-track {
  fill: none;
  stroke: var(--text-muted);
  stroke-opacity: 0.22;
  stroke-width: 2.5;
}

.ring-fill {
  fill: none;
  stroke: var(--text-muted);
  stroke-width: 2.5;
  stroke-linecap: round;
  transition: stroke-dasharray var(--transition-fast);
}

/* 单行读数浮层：宽度交给内容（读数本来就是一行），贴环正上方 */
.context-tip {
  position: absolute;
  bottom: calc(100% + 8px);
  left: 0;
  z-index: 120;
  padding: 5px 9px;
  white-space: nowrap;
  background: var(--surface-strong);
  border: 1px solid color-mix(in srgb, var(--text-muted) 22%, transparent);
  border-radius: var(--radius-md);
  box-shadow: var(--shadow-lg);
  color: var(--text-muted);
  font-size: 0.65rem;
  user-select: none;
  /* 只淡入：位移动画会和定位打架（与 CacheHitTooltip 同一处理） */
  animation: contextTipIn var(--transition-fast);
}

/* 本组件 scoped，keyframes 必须自己定义：CacheHitTooltip 里的同名动画
   会被 scoped 重命名，跨组件引用拿不到。 */
@keyframes contextTipIn {
  from { opacity: 0; }
  to { opacity: 1; }
}

/* 透明桥接块，盖住浮层与环之间的 8px 间隙，指针往上挪不会中途触发 mouseleave */
.context-tip::after {
  content: "";
  position: absolute;
  left: 0;
  right: 0;
  top: 100%;
  height: 8px;
}

.token-bar-spacer {
  flex: 1;
}

/* 会话累计 + 缓存命中读数：整组可点击 → 打开上下文监控窗口；hover 出明细浮层 */
.token-bar-usage {
  display: inline-flex;
  align-items: center;
  gap: 6px;
  position: relative; /* 明细浮层（CacheHitTooltip）的定位上下文 */
  cursor: pointer;
  border-radius: 4px;
  padding: 0 4px;
  transition: color 0.15s, background-color 0.15s;
}

/* hover 只给背景，不改字色。
   这里原本是 `color: var(--accent-blue)`——但蓝色在右侧上下文面板的语义里代表
   「预热」，拿它当命中态的 hover 色会让同一颜色在两处含义相反。 */
.token-bar-usage:hover {
  background: var(--glass-bg);
}

/* 状态色与右侧面板同口径：命中=绿、预热=蓝、未知=灰 */
.token-bar-cache.cache-hit {
  color: var(--accent-green);
}

.token-bar-cache.cache-warmup {
  color: var(--accent-blue);
  opacity: 0.8;
}

.token-bar-cache.cache-unknown {
  opacity: 0.85;
}

/* 主 Agent 进度 + 本次请求吞吐：整组不可点、无浮层 */
.token-bar-progress {
  gap: 6px;
  margin-left: 2px;
}

/* 吞吐数字用等宽数字：读数每轮跳一次，比例数字会让整条栏左右抖 */
.token-bar-throughput {
  font-variant-numeric: tabular-nums;
}

.token-bar-model {
  font-style: italic;
  font-size: 0.6rem;
  opacity: 0.85;
}
</style>
