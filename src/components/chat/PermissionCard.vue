<!--
# PermissionCard.vue — 会话流里的权限确认卡片

风格：灰白朴素（无彩色强调、无毛玻璃、无阴影），信息分层为"动作 + 明细 + 三个选择"。

## 交互
- 三个选择：拒绝 / 本次会话都允许（按范围记忆）/ 允许一次
- 拒绝可附一句说明，会原样回灌给模型（提示它别换等价写法重试）
- 快捷键 A / S / R(Esc) 仅在焦点不在输入控件时生效

## 依赖
- Internal: `stores/permission`（待确认请求队列）、`stores/chat`（提交决策）
-->
<script setup lang="ts">
import { computed, onMounted, onUnmounted, ref, watch } from 'vue';
import { useI18n } from 'vue-i18n';
import { usePermissionStore } from '../../stores/permission';
import { useChatStore } from '../../stores/chat';

const { t } = useI18n();
const perm = usePermissionStore();
const chat = useChatStore();

/// 拒绝说明：可选，但会回灌给模型，避免它换个写法重试同一操作
const rejectFeedback = ref('');

/// 当前会话还有几个待决策请求（并行子代理可能同时发起）
const queueLength = computed(() => perm.permissionQueue.length);
const canAllowSession = computed(() => perm.permissionRequest?.allowSession !== false);

// 卡片切换（上一个请求被处理）时清空输入
watch(() => perm.permissionRequest?.id, () => {
  rejectFeedback.value = '';
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
  const lines = msg.split('\n');
  const title = (lines[0] ?? '').trim();
  const details = lines.slice(1).join('\n').trim();
  return { title, details };
});

/// 输入框/可编辑元素里的按键永远是打字，不是快捷键
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
  <Transition name="perm-fade">
    <div v-if="perm.permissionRequest" class="perm-card">
      <div class="perm-head">
        <span class="perm-title">{{ parsed.title }}</span>
        <span v-if="queueLength > 1" class="perm-queue">+{{ queueLength - 1 }}</span>
      </div>

      <pre v-if="parsed.details" class="perm-detail">{{ parsed.details }}</pre>

      <input
        v-model="rejectFeedback"
        class="perm-feedback"
        type="text"
        :placeholder="t('permission.rejectFeedbackPlaceholder')"
        @keyup.enter="chat.resolvePermission('reject', rejectFeedback)"
      />

      <div class="perm-actions">
        <button class="perm-btn" @click="chat.resolvePermission('reject', rejectFeedback)">
          {{ t('permission.reject') }}<kbd>R</kbd>
        </button>
        <button v-if="canAllowSession" class="perm-btn" @click="chat.resolvePermission('allow_session')">
          {{ t('permission.allowSession') }}<kbd>S</kbd>
        </button>
        <button class="perm-btn primary" @click="chat.resolvePermission('allow')">
          {{ t('permission.allowOnce') }}<kbd>A</kbd>
        </button>
      </div>
    </div>
  </Transition>
</template>

<style scoped>
/* 灰白朴素：只用边框与文字层级区分，不用彩色、不用毛玻璃、不用阴影 */
.perm-card {
  margin: 8px 16px 12px;
  padding: 12px 14px;
  border: 1px solid var(--glass-border-subtle);
  border-radius: var(--radius-md);
  background: transparent;
  max-width: 620px;
  font-size: 0.78rem;
  line-height: 1.5;
}

.perm-head {
  display: flex;
  align-items: baseline;
  gap: 8px;
  margin-bottom: 6px;
}

.perm-title {
  font-weight: 600;
  color: var(--text-main);
}

.perm-queue {
  font-size: 0.7rem;
  color: var(--text-muted);
}

.perm-detail {
  margin: 0 0 10px;
  padding: 0;
  font-family: var(--font-mono);
  font-size: 0.72rem;
  line-height: 1.6;
  color: var(--text-soft);
  white-space: pre-wrap;
  word-break: break-all;
  max-height: 160px;
  overflow-y: auto;
}

.perm-feedback {
  width: 100%;
  box-sizing: border-box;
  margin-bottom: 10px;
  padding: 6px 0;
  border: none;
  border-bottom: 1px solid var(--glass-border-subtle);
  background: transparent;
  color: var(--text-main);
  font-size: 0.75rem;
  outline: none;
}

.perm-feedback::placeholder {
  color: var(--text-muted);
}

.perm-feedback:focus {
  border-bottom-color: var(--text-muted);
}

.perm-actions {
  display: flex;
  justify-content: flex-end;
  gap: 8px;
}

.perm-btn {
  display: inline-flex;
  align-items: center;
  gap: 6px;
  padding: 5px 10px;
  border: 1px solid var(--glass-border-subtle);
  border-radius: var(--radius-sm, 4px);
  background: transparent;
  color: var(--text-soft);
  font-size: 0.75rem;
  cursor: pointer;
  transition: border-color 0.12s ease, color 0.12s ease;
}

.perm-btn:hover {
  border-color: var(--glass-border);
  color: var(--text-main);
}

.perm-btn.primary {
  color: var(--text-main);
  border-color: var(--glass-border);
}

.perm-btn kbd {
  font-size: 0.62rem;
  color: var(--text-muted);
  font-family: var(--font-mono);
}

.perm-fade-enter-active { transition: opacity 0.15s ease-out; }
.perm-fade-leave-active { transition: opacity 0.1s ease-in; }
.perm-fade-enter-from,
.perm-fade-leave-to { opacity: 0; }
</style>
