<!--
# PermissionCard.vue — 权限确认卡片（复用 ComposerDockCard 统一骨架）

挂在输入框正上方，与 `UserQuestionCard` 共享 `ComposerDockCard` 视觉外壳、宽度与折叠能力，
仅承载权限审批专属的警示文案、操作明细、拒绝原因输入与快捷键（A / S / R）逻辑。

## Key Exports
- 默认导出组件：权限确认卡片

## Dependencies
- Internal: `./ComposerDockCard.vue`, `@/stores/permission`, `@/stores/chat`
- External: `vue`, `vue-i18n`
-->
<script setup lang="ts">
import { computed, onMounted, onUnmounted, ref, watch } from 'vue';
import { useI18n } from 'vue-i18n';
import { usePermissionStore } from '../../stores/permission';
import { useChatStore } from '../../stores/chat';
import ComposerDockCard from './ComposerDockCard.vue';

const { t } = useI18n();
const perm = usePermissionStore();
const chat = useChatStore();

/** 拒绝说明：可选，但会回灌给模型，避免它换个写法重试同一操作 */
const rejectFeedback = ref('');
/** 折叠态：允许用户先收起权限卡查看上方聊天记录 */
const isCollapsed = ref(false);

/** 当前会话还有几个待决策请求（并行子代理可能同时发起） */
const queueLength = computed(() => perm.permissionQueue.length);
const canAllowSession = computed(() => perm.permissionRequest?.allowSession !== false);

// 卡片切换（上一个请求被处理）时清空输入并自动展开
watch(() => perm.permissionRequest?.id, () => {
  rejectFeedback.value = '';
  isCollapsed.value = false;
});

/**
 * 权限判定发来的文案是"第一行=动作、其余=明细"的结构：
 *   删除文件
 *   C:\...\trash.txt
 *   工具：DeleteFile
 *   风险提示：…
 *   允许范围：…
 * 旧格式（如循环续跑确认）只有一行，就整行当标题。
 */
const parsed = computed(() => {
  const msg = perm.permissionRequest?.message ?? '';
  // 有独立警示字段时，把正文里的"风险提示："行抽出来用弱警示样式渲染（避免重复展示）
  const warning = perm.permissionRequest?.warning?.trim() || '';
  const lines = msg.split('\n');
  const title = (lines[0] ?? '').trim();
  const details = lines.slice(1)
    .filter((line) => !(warning && line.trimStart().startsWith('风险提示：')))
    .join('\n')
    .trim();
  return { title, details, warning };
});

const queueCounter = computed(() => (queueLength.value > 1 ? `1/${queueLength.value}` : ''));

/** 输入框/可编辑元素里的按键永远是打字，不是快捷键 */
function isTypingTarget(target: EventTarget | null): boolean {
  const el = target as HTMLElement | null;
  if (!el || !el.tagName) return false;
  const tag = el.tagName.toLowerCase();
  return tag === 'input' || tag === 'textarea' || tag === 'select' || el.isContentEditable === true;
}

const handleKeydown = (e: KeyboardEvent) => {
  if (!perm.permissionRequest) return;
  if (isTypingTarget(e.target)) return;
  const key = e.key.toLowerCase();
  if (key === 'a') { e.preventDefault(); chat.resolvePermission('allow'); }
  else if (key === 's' && canAllowSession.value) { e.preventDefault(); chat.resolvePermission('allow_session'); }
  else if (key === 'r' || key === 'escape') { e.preventDefault(); chat.resolvePermission('reject', rejectFeedback.value); }
};

onMounted(() => window.addEventListener('keydown', handleKeydown, true));
onUnmounted(() => window.removeEventListener('keydown', handleKeydown, true));
</script>

<template>
  <ComposerDockCard
    :visible="Boolean(perm.permissionRequest)"
    :title="parsed.title"
    :counter="queueCounter"
    :tone="parsed.warning ? 'warning' : 'neutral'"
    v-model:collapsed="isCollapsed"
    aria-label="权限确认"
  >
    <!-- 弱警示：琥珀小字，不做大红大跳的视觉冲击 -->
    <p v-if="parsed.warning" class="perm-warning">{{ parsed.warning }}</p>

    <pre v-if="parsed.details" class="perm-detail">{{ parsed.details }}</pre>

    <div class="perm-feedback-box">
      <input
        v-model="rejectFeedback"
        class="perm-feedback-input"
        type="text"
        :placeholder="t('permission.rejectFeedbackPlaceholder')"
        @keyup.enter="chat.resolvePermission('reject', rejectFeedback)"
      />
    </div>

    <template #footer-left>
      <button
        type="button"
        class="dock-btn-ghost"
        @click="chat.resolvePermission('reject', rejectFeedback)"
      >
        {{ t('permission.reject') }}
        <kbd class="dock-kbd">R</kbd>
      </button>
    </template>

    <template #footer-right>
      <button
        v-if="canAllowSession"
        type="button"
        class="dock-btn dock-btn-secondary"
        @click="chat.resolvePermission('allow_session')"
      >
        {{ t('permission.allowSession') }}
        <kbd class="dock-kbd">S</kbd>
      </button>
      <button
        type="button"
        class="dock-btn dock-btn-primary"
        @click="chat.resolvePermission('allow')"
      >
        {{ t('permission.allowOnce') }}
        <kbd class="dock-kbd">A</kbd>
      </button>
    </template>
  </ComposerDockCard>
</template>

<style scoped>
.perm-warning {
  margin: 0 0 8px;
  font-size: 0.76rem;
  line-height: 1.45;
  color: var(--text-warning, #d97706);
  white-space: pre-line;
}

.perm-detail {
  margin: 0 0 10px;
  padding: 9px 12px;
  border-radius: 10px;
  border: 1px solid var(--glass-border-subtle, rgba(0, 0, 0, 0.08));
  background: color-mix(in srgb, var(--text-main) 2.5%, transparent);
  font-family: var(--font-mono);
  font-size: 0.76rem;
  line-height: 1.5;
  color: var(--text-soft, var(--text-main));
  white-space: pre-wrap;
  word-break: break-all;
  max-height: 112px;
  overflow-y: auto;
}

.perm-feedback-box {
  margin-bottom: 12px;
  padding: 7px 12px;
  border-radius: 10px;
  border: 1px solid var(--glass-border-subtle, rgba(0, 0, 0, 0.08));
  background: transparent;
  transition: border-color 0.15s ease;
}

.perm-feedback-box:focus-within {
  border-color: color-mix(in srgb, var(--text-main) 45%, transparent);
}

.perm-feedback-input {
  width: 100%;
  box-sizing: border-box;
  padding: 0;
  border: none;
  background: transparent;
  color: var(--text-main);
  font-size: 0.8rem;
  line-height: 1.45;
  outline: none;
  box-shadow: none;
}

.perm-feedback-input::placeholder {
  color: var(--text-muted);
  opacity: 0.72;
}
</style>
