<!--
# PermissionCard.vue — 内联权限请求卡片

嵌在会话区内，不阻塞用户切换会话。像 Claude Code 一样在对话流中等待确认。
-->
<script setup lang="ts">
import { computed, onMounted, onUnmounted } from 'vue';
import { useI18n } from 'vue-i18n';
import { usePermissionStore } from '../../stores/permission';
import { useChatStore } from '../../stores/chat';

const { t } = useI18n();
const perm = usePermissionStore();
const chat = useChatStore();

const canAllowSession = computed(() => perm.permissionRequest?.allowSession !== false);
const isLoopContinuation = computed(() => perm.permissionRequest?.kind === 'loop_continuation');

const parsedData = computed(() => {
  if (!perm.permissionRequest) return { reason: '', command: '' };
  const msg = perm.permissionRequest.message;
  let reason = msg;
  let command = '';

  const codeMatch = msg.match(/`([^`]+)`/);
  if (codeMatch) {
    reason = msg.replace(codeMatch[0], '').trim();
    command = codeMatch[1].trim();
  } else {
    const colonMatch = msg.match(/[:：]/);
    if (colonMatch && colonMatch.index !== undefined) {
      const potentialCommand = msg.substring(colonMatch.index + 1).trim();
      if (potentialCommand.length > 5) {
        reason = msg.substring(0, colonMatch.index + 1).trim();
        command = potentialCommand;
      }
    } else if (msg.length > 150) {
      reason = t('permission.complexCommandReason');
      command = msg;
    }
  }

  if (command.length > 600) {
    command = command.substring(0, 280) + `\n... [${t('permission.longCommandOmitted')}] ...\n` + command.substring(command.length - 200);
  }
  if (reason.length > 200) {
    reason = reason.substring(0, 200) + '...';
  }
  return { reason, command };
});

const handleKeydown = (e: KeyboardEvent) => {
  if (!perm.permissionRequest) return;
  const key = e.key.toLowerCase();
  if (key === 'a') { e.preventDefault(); chat.resolvePermission('allow'); }
  else if (key === 's' && canAllowSession.value) { e.preventDefault(); chat.resolvePermission('allow_session'); }
  else if (key === 'r' || key === 'escape') { e.preventDefault(); chat.resolvePermission('reject'); }
};

onMounted(() => window.addEventListener('keydown', handleKeydown, true));
onUnmounted(() => window.removeEventListener('keydown', handleKeydown, true));
</script>

<template>
  <Transition name="perm-slide">
    <div v-if="perm.permissionRequest" class="perm-card">
      <div class="perm-header">
        <svg class="perm-icon" viewBox="0 0 24 24" width="18" height="18" stroke="currentColor" stroke-width="2" fill="none" stroke-linecap="round" stroke-linejoin="round">
          <path d="M10.29 3.86L1.82 18a2 2 0 0 0 1.71 3h16.94a2 2 0 0 0 1.71-3L13.71 3.86a2 2 0 0 0-3.42 0z"/>
          <line x1="12" y1="9" x2="12" y2="13"/>
          <line x1="12" y1="17" x2="12.01" y2="17"/>
        </svg>
        <div>
          <strong>{{ isLoopContinuation ? t('permission.loopTitle') : t('permission.securityTitle') }}</strong>
          <span>{{ isLoopContinuation ? t('permission.loopSubtitle') : t('permission.securitySubtitle') }}</span>
        </div>
      </div>

      <p class="perm-reason">{{ parsedData.reason }}</p>

      <pre v-if="parsedData.command" class="perm-command"><code>{{ parsedData.command }}</code></pre>

      <div class="perm-actions">
        <button class="perm-btn reject" @click="chat.resolvePermission('reject')">
          <kbd>R</kbd> {{ t('permission.reject') }}
        </button>
        <button class="perm-btn allow" @click="chat.resolvePermission('allow')">
          <kbd>A</kbd> {{ t('permission.allowOnce') }}
        </button>
        <button v-if="canAllowSession" class="perm-btn session" @click="chat.resolvePermission('allow_session')">
          <kbd>S</kbd> {{ t('permission.allowSession') }}
        </button>
      </div>
    </div>
  </Transition>
</template>

<style scoped>
.perm-card {
  margin: 8px 16px 12px;
  padding: 14px 16px;
  border: 1px solid rgba(245, 158, 11, 0.3);
  border-radius: 12px;
  background: color-mix(in srgb, var(--surface-strong) 60%, transparent);
  box-shadow: 0 0 24px rgba(245, 158, 11, 0.08);
  max-width: 580px;
}

.perm-header {
  display: flex;
  align-items: center;
  gap: 10px;
  margin-bottom: 10px;
}

.perm-icon {
  color: var(--accent-yellow);
  flex-shrink: 0;
}

.perm-header strong {
  display: block;
  color: var(--text-main);
  font-size: 0.85rem;
  font-weight: 800;
}

.perm-header span {
  display: block;
  color: var(--text-muted);
  font-size: 0.68rem;
  margin-top: 1px;
}

.perm-reason {
  margin: 0 0 10px;
  color: var(--text-soft);
  font-size: 0.8rem;
  line-height: 1.55;
  word-break: break-word;
}

.perm-command {
  margin: 0 0 14px;
  padding: 10px 12px;
  max-height: 180px;
  overflow: auto;
  border: 1px solid var(--glass-border);
  border-radius: 8px;
  background: rgba(0, 0, 0, 0.18);
  font-family: var(--font-mono);
  font-size: 0.72rem;
  line-height: 1.5;
  white-space: pre-wrap;
  word-break: break-all;
  color: var(--text-soft);
}

.perm-actions {
  display: flex;
  gap: 10px;
  align-items: center;
}

.perm-btn {
  display: inline-flex;
  align-items: center;
  gap: 6px;
  padding: 7px 14px;
  border: 1px solid var(--glass-border);
  border-radius: 8px;
  background: var(--glass-bg-light);
  color: var(--text-main);
  font-size: 0.78rem;
  font-weight: 650;
  cursor: pointer;
  transition: all 0.15s ease;
}

.perm-btn kbd {
  display: inline-flex;
  align-items: center;
  justify-content: center;
  min-width: 18px;
  height: 18px;
  padding: 0 4px;
  border-radius: 4px;
  background: rgba(255,255,255,0.08);
  border: 1px solid rgba(255,255,255,0.12);
  font-family: var(--font-mono);
  font-size: 0.6rem;
  font-weight: 700;
}

.perm-btn:hover {
  transform: translateY(-1px);
  box-shadow: 0 4px 12px rgba(0,0,0,0.15);
}

.perm-btn.reject { color: var(--accent-red); border-color: rgba(239,68,68,0.25); }
.perm-btn.reject:hover { background: rgba(239,68,68,0.08); }
.perm-btn.allow { color: var(--accent-blue); border-color: rgba(59,130,246,0.25); }
.perm-btn.allow:hover { background: rgba(59,130,246,0.08); }
.perm-btn.session { color: var(--accent-yellow); border-color: rgba(245,158,11,0.25); }
.perm-btn.session:hover { background: rgba(245,158,11,0.08); }

.perm-slide-enter-active { transition: all 0.25s ease; }
.perm-slide-leave-active { transition: all 0.15s ease; }
.perm-slide-enter-from { opacity: 0; transform: translateY(-12px); }
.perm-slide-leave-to { opacity: 0; transform: translateY(-8px); }
</style>
