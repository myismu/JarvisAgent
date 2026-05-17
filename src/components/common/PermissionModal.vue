<script setup lang="ts">
import { computed, onMounted, onUnmounted, watch, nextTick, ref } from 'vue';
import { useI18n } from 'vue-i18n';
import { usePermissionStore } from '../../stores/permission';
import { useChatStore } from '../../stores/chat';

const { t } = useI18n();

const perm = usePermissionStore();
const chat = useChatStore();
const cardRef = ref<HTMLElement | null>(null);

const canAllowSession = computed(() => perm.permissionRequest?.allowSession !== false);
const isLoopContinuation = computed(() => perm.permissionRequest?.kind === 'loop_continuation');

watch(() => perm.permissionRequest, async (req) => {
  if (req) {
    await nextTick();
    cardRef.value?.scrollIntoView({ behavior: 'smooth', block: 'center' });
  }
});

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
      const colonIndex = colonMatch.index;
      const potentialCommand = msg.substring(colonIndex + 1).trim();
      if (potentialCommand.length > 5) {
        reason = msg.substring(0, colonIndex + 1).trim();
        command = potentialCommand;
      }
    } else if (msg.length > 150) {
      reason = t('permission.complexCommandReason');
      command = msg;
    }
  }
  if (command.length > 500) {
    command = command.substring(0, 250) + `\n\n... [${t('permission.longCommandOmitted')}] ...\n\n` + command.substring(command.length - 200);
  }
  if (reason.length > 150) reason = reason.substring(0, 150) + '...';
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
    <div v-if="perm.permissionRequest" ref="cardRef" class="perm-card">
      <div class="perm-header">
        <svg viewBox="0 0 24 24" width="18" height="18" stroke="currentColor" stroke-width="2" fill="none" stroke-linecap="round" stroke-linejoin="round">
          <path d="M10.29 3.86L1.82 18a2 2 0 0 0 1.71 3h16.94a2 2 0 0 0 1.71-3L13.71 3.86a2 2 0 0 0-3.42 0z"></path>
          <line x1="12" y1="9" x2="12" y2="13"></line>
          <line x1="12" y1="17" x2="12.01" y2="17"></line>
        </svg>
        <div>
          <span class="perm-title">{{ isLoopContinuation ? t('permission.loopTitle') : t('permission.securityTitle') }}</span>
          <span class="perm-subtitle">{{ isLoopContinuation ? t('permission.loopSubtitle') : t('permission.securitySubtitle') }}</span>
        </div>
      </div>

      <div class="perm-body">
        <p class="perm-reason">{{ parsedData.reason }}</p>
        <div v-if="parsedData.command" class="perm-command">
          <pre class="perm-command-block"><code>{{ parsedData.command }}</code></pre>
        </div>
      </div>

      <div class="perm-actions">
        <button @click="chat.resolvePermission('reject')" class="perm-btn perm-reject" :title="t('permission.rejectShortcut')">
          <kbd>R</kbd> {{ t('permission.reject') }}
        </button>
        <div class="perm-allow-group">
          <button v-if="canAllowSession" @click="chat.resolvePermission('allow_session')" class="perm-btn perm-allow-session" :title="t('permission.allowSessionShortcut')">
            <kbd>S</kbd> {{ t('permission.allowSession') }}
          </button>
          <button @click="chat.resolvePermission('allow')" class="perm-btn perm-allow" :title="t('permission.allowShortcut')">
            <kbd>A</kbd> {{ t('permission.allowOnce') }}
          </button>
        </div>
      </div>
    </div>
  </Transition>
</template>

<style scoped>
.perm-card {
  margin: 8px 40px 4px;
  padding: 14px 18px;
  border-radius: var(--radius-lg);
  background: var(--glass-bg-heavy);
  backdrop-filter: blur(16px);
  -webkit-backdrop-filter: blur(16px);
  border: 1px solid var(--glass-border);
  box-shadow: var(--shadow-lg);
  max-width: 580px;
}

.perm-header {
  display: flex;
  align-items: flex-start;
  gap: 10px;
  margin-bottom: 10px;
  color: var(--text-muted);
}

.perm-header svg {
  flex-shrink: 0;
  margin-top: 1px;
  opacity: 0.7;
}

.perm-title {
  display: block;
  font-size: 0.85rem;
  font-weight: 700;
  color: var(--text-main);
}

.perm-subtitle {
  display: block;
  font-size: 0.72rem;
  color: var(--text-muted);
  margin-top: 1px;
}

.perm-body {
  margin-bottom: 12px;
}

.perm-reason {
  font-size: 0.82rem;
  line-height: 1.5;
  color: var(--text-main);
  margin: 0 0 8px;
  word-break: break-word;
}

.perm-command {
  margin-top: 6px;
}

.perm-command-block {
  background: color-mix(in srgb, var(--text-muted) 8%, transparent);
  border: 1px solid var(--glass-border-subtle);
  border-radius: var(--radius-md);
  padding: 10px 14px;
  max-height: 120px;
  overflow-y: auto;
  margin: 0;
  font-family: var(--font-mono);
  font-size: 0.75rem;
  color: var(--text-soft);
  white-space: pre-wrap;
  word-break: break-all;
  line-height: 1.4;
}

.perm-actions {
  display: flex;
  justify-content: space-between;
  align-items: center;
  gap: 10px;
}

.perm-allow-group {
  display: flex;
  gap: 8px;
}

.perm-btn {
  display: inline-flex;
  align-items: center;
  gap: 6px;
  padding: 7px 14px;
  border-radius: var(--radius-md);
  border: 1px solid var(--glass-border-subtle);
  background: var(--glass-bg-light);
  color: var(--text-main);
  font-size: 0.78rem;
  font-weight: 600;
  cursor: pointer;
  transition: all var(--transition-fast);
}

.perm-btn kbd {
  font-size: 0.6rem;
  padding: 1px 4px;
  border-radius: 3px;
  border: 1px solid var(--glass-border-subtle);
  background: var(--glass-bg);
  font-family: var(--font-mono);
  font-weight: 600;
}

.perm-btn:hover {
  background: var(--glass-bg);
  border-color: var(--glass-border);
}

.perm-reject:hover {
  color: var(--accent-red);
  border-color: color-mix(in srgb, var(--accent-red) 30%, transparent);
}

.perm-allow:hover {
  color: var(--text-main);
  border-color: var(--glass-border);
  background: var(--glass-bg);
}

.perm-allow-session:hover {
  border-color: var(--glass-border);
  background: var(--glass-bg);
}

.perm-slide-enter-active { transition: all 0.25s ease-out; }
.perm-slide-leave-active { transition: all 0.15s ease-in; }
.perm-slide-enter-from { opacity: 0; transform: translateY(12px); }
.perm-slide-leave-to { opacity: 0; transform: translateY(-6px); }
</style>
