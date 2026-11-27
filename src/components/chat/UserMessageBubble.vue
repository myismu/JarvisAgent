<!--
# UserMessageBubble.vue — 单条用户消息气泡与原地编辑组件

负责展示用户消息图片、长文本折叠/展开、无外框幽灵操作栏（复制 / 修改 / 撤回）
以及原地编辑输入框（支持自动增高、Escape 取消与 Ctrl/Cmd+Enter 提交）。

## Key Exports
- `UserMessageBubble`: 单条用户消息展示与原地编辑组件

## Dependencies
- Internal: `@/utils/userMessage`
- External: `vue`, `vue-i18n`

## Constraints
- 保留 `.chat-message.user-message`、`data-msg-id`、`data-user-message-index` 等 DOM 契约以兼容 `MessageRail`
-->
<script setup lang="ts">
import { computed, nextTick, onUnmounted, ref, watch } from 'vue';
import { useI18n } from 'vue-i18n';
import { isLongUserText, plainUserText } from '../../utils/userMessage';

const props = defineProps<{
  message: any;
  index: number;
  isEditing: boolean;
  menuOpen: boolean;
  disabled: boolean;
  rollbackLoading: boolean;
  copyPlainText: (text: string) => Promise<boolean>;
}>();

const emit = defineEmits<{
  (e: 'start-edit', message: any): void;
  (e: 'cancel-edit'): void;
  (e: 'confirm-edit', message: any, nextText: string): void;
  (e: 'open-rollback', message: any, event: MouseEvent): void;
}>();

const { t } = useI18n();

const expanded = ref(false);
const copied = ref(false);
let copiedTimer: number | null = null;

const editingText = ref('');
const editingOriginalText = ref('');
const editTextareaRef = ref<HTMLTextAreaElement | null>(null);

const messageText = computed(() =>
  plainUserText(props.message.text ?? props.message.userContent ?? props.message.content ?? ''),
);
const isLong = computed(() => isLongUserText(messageText.value));

const canConfirmEdit = computed(() => {
  const trimmed = editingText.value.trim();
  const hasImages = Array.isArray(props.message.images) && props.message.images.length > 0;
  if (!trimmed && !hasImages) return false;
  return editingText.value !== editingOriginalText.value;
});

function resizeEditTextarea(el?: HTMLTextAreaElement | null) {
  const target = el ?? editTextareaRef.value;
  if (!target) return;
  target.style.height = 'auto';
  target.style.height = `${Math.min(target.scrollHeight, 360)}px`;
}

watch(
  () => props.isEditing,
  (editing) => {
    if (editing) {
      const original = messageText.value;
      editingText.value = original;
      editingOriginalText.value = original;
      nextTick(() => {
        const el = editTextareaRef.value;
        if (!el) return;
        resizeEditTextarea(el);
        el.focus();
        const len = el.value.length;
        el.setSelectionRange(len, len);
      });
    } else {
      editingText.value = '';
      editingOriginalText.value = '';
    }
  },
  { immediate: true },
);

async function handleCopy() {
  const text = messageText.value;
  if (!text) return;
  const ok = await props.copyPlainText(text);
  if (!ok) return;
  copied.value = true;
  if (copiedTimer !== null) {
    window.clearTimeout(copiedTimer);
  }
  copiedTimer = window.setTimeout(() => {
    copied.value = false;
    copiedTimer = null;
  }, 1500);
}

function handleConfirm() {
  if (props.disabled || !canConfirmEdit.value) return;
  emit('confirm-edit', props.message, editingText.value);
}

function handleEditKeydown(e: KeyboardEvent) {
  if (e.key === 'Escape') {
    e.preventDefault();
    emit('cancel-edit');
  } else if (e.key === 'Enter' && (e.ctrlKey || e.metaKey)) {
    e.preventDefault();
    handleConfirm();
  }
}

onUnmounted(() => {
  if (copiedTimer !== null) {
    window.clearTimeout(copiedTimer);
    copiedTimer = null;
  }
});
</script>

<template>
  <div
    class="chat-message user-message"
    :class="{
      'is-editing-row': isEditing,
      'menu-open': menuOpen,
    }"
    :data-msg-id="message.id"
  >
    <div
      class="message-content"
      :class="{ 'is-editing': isEditing }"
      :data-user-message-index="index"
      :data-message-id="message.messageId"
      :data-rollback-checkpoint-id="message.rollbackCheckpointId || ''"
      :data-rollback-mode="message.rollbackMode || 'session'"
    >
      <!-- 图片 -->
      <div v-if="message.images && message.images.length > 0" class="user-images">
        <img
          v-for="(img, imgIdx) in message.images"
          :key="`img-${imgIdx}`"
          :src="img"
          class="user-image"
          alt="用户发送的图片"
        />
      </div>
      <!-- 原地编辑态 -->
      <template v-if="isEditing">
        <textarea
          :ref="(el) => { if (el) editTextareaRef = el as HTMLTextAreaElement; }"
          v-model="editingText"
          class="user-edit-textarea"
          rows="1"
          :disabled="rollbackLoading"
          @input="resizeEditTextarea()"
          @keydown="handleEditKeydown"
        />
        <div class="user-edit-footer">
          <span class="user-edit-hint">{{ t('chat.editHint') }}</span>
          <div class="user-edit-buttons">
            <button
              type="button"
              class="user-edit-btn cancel"
              :disabled="rollbackLoading"
              @click="emit('cancel-edit')"
            >
              {{ t('common.cancel') }}
            </button>
            <button
              type="button"
              class="user-edit-btn confirm"
              :disabled="disabled || !canConfirmEdit"
              @click="handleConfirm"
            >
              {{ rollbackLoading ? t('common.processing') : t('chat.editConfirm') }}
            </button>
          </div>
        </div>
      </template>
      <!-- 正常展示态 -->
      <template v-else>
        <div class="user-text" :class="{ collapsed: isLong && !expanded }">
          {{ messageText }}
        </div>
        <button
          v-if="isLong"
          class="user-msg-toggle"
          @click="expanded = !expanded"
        >
          {{ expanded ? t('chat.collapse') : t('chat.expandAll') }}
        </button>
      </template>
    </div>
    <!-- 气泡下方右侧：无外框纯幽灵图标操作栏（复制 / 修改 / 撤回） -->
    <div v-if="!isEditing" class="user-msg-actions">
      <button
        type="button"
        class="user-action-btn"
        :class="{ 'is-copied': copied }"
        :title="copied ? t('chat.copied') : t('chat.copyMessage')"
        :aria-label="copied ? t('chat.copied') : t('chat.copyMessage')"
        @click.stop="handleCopy"
      >
        <svg
          v-if="copied"
          viewBox="0 0 24 24"
          width="14"
          height="14"
          fill="none"
          stroke="currentColor"
          stroke-width="2"
          stroke-linecap="round"
          stroke-linejoin="round"
          aria-hidden="true"
        >
          <polyline points="20 6 9 17 4 12" />
        </svg>
        <svg
          v-else
          viewBox="0 0 24 24"
          width="14"
          height="14"
          fill="none"
          stroke="currentColor"
          stroke-width="1.75"
          stroke-linecap="round"
          stroke-linejoin="round"
          aria-hidden="true"
        >
          <rect x="9" y="9" width="13" height="13" rx="2" ry="2" />
          <path d="M5 15H4a2 2 0 0 1-2-2V4a2 2 0 0 1 2-2h9a2 2 0 0 1 2 2v1" />
        </svg>
      </button>
      <button
        v-if="message.rollbackCheckpointId || message.messageId"
        type="button"
        class="user-action-btn"
        :disabled="disabled"
        :title="t('chat.editMessage')"
        :aria-label="t('chat.editMessage')"
        @click.stop="emit('start-edit', message)"
      >
        <svg
          viewBox="0 0 24 24"
          width="14"
          height="14"
          fill="none"
          stroke="currentColor"
          stroke-width="1.75"
          stroke-linecap="round"
          stroke-linejoin="round"
          aria-hidden="true"
        >
          <path d="M12 20h9" />
          <path d="M16.5 3.5a2.121 2.121 0 0 1 3 3L7 19l-4 1 1-4L16.5 3.5z" />
        </svg>
      </button>
      <button
        v-if="message.rollbackCheckpointId || message.messageId"
        type="button"
        class="user-action-btn rollback-trigger"
        :data-cp-id="message.rollbackCheckpointId || ''"
        :disabled="disabled"
        :title="t('rollback.trigger')"
        :aria-label="t('rollback.trigger')"
        @click.stop="emit('open-rollback', message, $event)"
      >
        <svg
          viewBox="0 0 24 24"
          width="14"
          height="14"
          fill="none"
          stroke="currentColor"
          stroke-width="1.75"
          stroke-linecap="round"
          stroke-linejoin="round"
          aria-hidden="true"
        >
          <path d="M9 14 4 9l5-5" />
          <path d="M4 9h10.5a5.5 5.5 0 0 1 5.5 5.5V20" />
        </svg>
      </button>
    </div>
  </div>
</template>

<style scoped>
.user-edit-textarea {
  width: 100%;
  min-height: 44px;
  max-height: 360px;
  padding: 0;
  margin: 0;
  border: none;
  outline: none;
  resize: none;
  background: transparent;
  color: var(--text-main);
  font-family: inherit;
  font-size: 0.95rem;
  line-height: 1.6;
  letter-spacing: 0.01em;
  overflow-y: auto;
}

.user-edit-footer {
  display: flex;
  align-items: center;
  justify-content: space-between;
  gap: 12px;
  margin-top: 10px;
  padding-top: 8px;
  border-top: 1px solid color-mix(in srgb, var(--text-main) 8%, transparent);
}

.user-edit-hint {
  font-size: 0.75rem;
  color: var(--text-muted);
  user-select: none;
}

.user-edit-buttons {
  display: inline-flex;
  align-items: center;
  gap: 8px;
}

.user-edit-btn {
  padding: 5px 12px;
  border-radius: 6px;
  font-size: 0.82rem;
  font-weight: 600;
  cursor: pointer;
  border: 1px solid transparent;
  transition: background 0.15s ease, color 0.15s ease, opacity 0.15s ease;
}

.user-edit-btn.cancel {
  background: transparent;
  color: var(--text-muted);
  border-color: var(--glass-border-subtle);
}

.user-edit-btn.cancel:hover:not(:disabled) {
  background: color-mix(in srgb, var(--text-main) 7%, transparent);
  color: var(--text-main);
}

.user-edit-btn.confirm {
  background: var(--btn-primary-bg);
  color: var(--btn-primary-fg);
}

.user-edit-btn.confirm:hover:not(:disabled) {
  background: var(--btn-primary-bg-hover);
}

.user-edit-btn:disabled {
  opacity: 0.45;
  cursor: not-allowed;
}
</style>
