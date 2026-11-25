<!--
# McpServerCard.vue — 单个 MCP 服务状态与操作卡片

对齐 SkillCard 的视觉语言：圆角图标区、语义化状态胶囊、盾牌信任切换、
以及分主次的底部操作栏（左侧启停控制，右侧详情/编辑/删除图标按钮）。
-->
<script setup lang="ts">
import { computed } from 'vue';
import { useI18n } from 'vue-i18n';
import type { McpServerSummary } from '../../types/generated/contracts';

const props = defineProps<{
  server: McpServerSummary;
  restarting: boolean;
}>();

const emit = defineEmits<{
  (e: 'toggle-enabled', name: string, enabled: boolean): void;
  (e: 'toggle-trusted', name: string, trusted: boolean): void;
  (e: 'restart', name: string): void;
  (e: 'stop', name: string): void;
  (e: 'detail', name: string): void;
  (e: 'edit', server: McpServerSummary): void;
  (e: 'delete', name: string): void;
}>();

const { t } = useI18n();

const statusLabel = computed(() => t(`extensions.mcp.status.${props.server.status}`));
const transportLabel = computed(() => t(`extensions.mcp.transport.${props.server.transport}`));
const isRunning = computed(
  () => props.server.status === 'ready' || props.server.status === 'starting',
);
const endpointPreview = computed(() => {
  if (props.server.transport === 'streamable_http') {
    return props.server.config.url ?? '';
  }
  const cmd = props.server.config.command ?? '';
  const args = props.server.config.args.join(' ');
  return args ? `${cmd} ${args}` : cmd;
});
</script>

<template>
  <div class="mcp-card" :class="{ disabled: !server.enabled }">
    <div class="card-header">
      <div class="server-icon" :class="`status-${server.status}`">
        <svg v-if="server.transport === 'stdio'" viewBox="0 0 24 24" width="18" height="18" stroke="currentColor" stroke-width="1.9" fill="none" stroke-linecap="round" stroke-linejoin="round">
          <polyline points="4 17 10 11 4 5"></polyline>
          <line x1="12" y1="19" x2="20" y2="19"></line>
        </svg>
        <svg v-else viewBox="0 0 24 24" width="18" height="18" stroke="currentColor" stroke-width="1.9" fill="none" stroke-linecap="round" stroke-linejoin="round">
          <circle cx="12" cy="12" r="10"></circle>
          <line x1="2" y1="12" x2="22" y2="12"></line>
          <path d="M12 2a15.3 15.3 0 0 1 4 10 15.3 15.3 0 0 1-4 10 15.3 15.3 0 0 1-4-10 15.3 15.3 0 0 1 4-10z"></path>
        </svg>
      </div>

      <div class="header-main">
        <div class="title-row">
          <h3 class="server-name">{{ server.name }}</h3>
          <span class="status-pill" :class="`status-${server.status}`">
            <span class="status-dot"></span>
            {{ statusLabel }}
          </span>
        </div>
        <div class="sub-badges">
          <span class="meta-chip">{{ transportLabel }}</span>
          <span v-if="server.autoStart" class="meta-chip accent">{{ t('extensions.mcp.autoStartTag') }}</span>
        </div>
      </div>

      <button
        type="button"
        class="mcp-toggle"
        :class="{ active: server.enabled }"
        :title="server.enabled ? t('extensions.mcp.disableTooltip') : t('extensions.mcp.enableTooltip')"
        @click="emit('toggle-enabled', server.name, !server.enabled)"
      >
        <span class="toggle-track">
          <span class="toggle-thumb"></span>
        </span>
      </button>
    </div>

    <div class="endpoint-box" :title="endpointPreview">
      <code>{{ endpointPreview }}</code>
    </div>

    <p v-if="server.lastError" class="error-banner" :title="server.lastError">
      {{ server.lastError }}
    </p>

    <div class="meta-bar">
      <span v-if="server.hasCachedCapabilities" class="stats-text">
        {{ t('extensions.mcp.stats', { tools: server.toolCount, resources: server.resourceCount, prompts: server.promptCount }) }}
      </span>
      <span v-else class="stats-text muted">{{ t('extensions.mcp.uncachedHint') }}</span>

      <button
        type="button"
        class="trust-pill"
        :class="{ trusted: server.trusted }"
        :title="t('extensions.mcp.trustedHint')"
        @click="emit('toggle-trusted', server.name, !server.trusted)"
      >
        <svg viewBox="0 0 24 24" width="12" height="12" stroke="currentColor" stroke-width="2.1" fill="none" stroke-linecap="round" stroke-linejoin="round">
          <path d="M12 22s8-4 8-10V5l-8-3-8 3v7c0 6 8 10 8 10z"></path>
          <polyline v-if="server.trusted" points="9 12 11 14 15 10"></polyline>
        </svg>
        <span>{{ server.trusted ? t('extensions.mcp.trusted') : t('extensions.mcp.untrusted') }}</span>
      </button>
    </div>

    <div class="card-footer">
      <div class="footer-left">
        <button
          type="button"
          class="ctrl-btn primary-soft"
          :disabled="!server.enabled || restarting"
          @click="emit('restart', server.name)"
        >
          <svg v-if="!isRunning" viewBox="0 0 24 24" width="13" height="13" stroke="currentColor" stroke-width="2.2" fill="none" stroke-linecap="round" stroke-linejoin="round">
            <polygon points="5 3 19 12 5 21 5 3"></polygon>
          </svg>
          <svg v-else viewBox="0 0 24 24" width="13" height="13" stroke="currentColor" stroke-width="2.1" fill="none" stroke-linecap="round" stroke-linejoin="round">
            <polyline points="23 4 23 10 17 10"></polyline>
            <path d="M20.49 15a9 9 0 1 1-2.12-9.36L23 10"></path>
          </svg>
          <span>{{ restarting ? t('extensions.mcp.status.starting') : t('extensions.mcp.startOrRestart') }}</span>
        </button>

        <button
          type="button"
          class="ctrl-btn stop-btn"
          :disabled="!isRunning"
          :title="t('extensions.mcp.stopTooltip')"
          @click="emit('stop', server.name)"
        >
          <svg viewBox="0 0 24 24" width="12" height="12" fill="currentColor">
            <rect x="6" y="6" width="12" height="12" rx="2"></rect>
          </svg>
          <span>{{ t('extensions.mcp.stop') }}</span>
        </button>
      </div>

      <div class="footer-right">
        <button
          type="button"
          class="ghost-btn"
          @click="emit('detail', server.name)"
        >
          <svg viewBox="0 0 24 24" width="13" height="13" stroke="currentColor" stroke-width="2" fill="none" stroke-linecap="round" stroke-linejoin="round">
            <line x1="8" y1="6" x2="21" y2="6"></line>
            <line x1="8" y1="12" x2="21" y2="12"></line>
            <line x1="8" y1="18" x2="21" y2="18"></line>
            <line x1="3" y1="6" x2="3.01" y2="6"></line>
            <line x1="3" y1="12" x2="3.01" y2="12"></line>
            <line x1="3" y1="18" x2="3.01" y2="18"></line>
          </svg>
          <span>{{ t('extensions.mcp.detail') }}</span>
        </button>

        <button
          type="button"
          class="icon-btn"
          :title="t('extensions.mcp.edit')"
          @click="emit('edit', server)"
        >
          <svg viewBox="0 0 24 24" width="14" height="14" stroke="currentColor" stroke-width="2" fill="none" stroke-linecap="round" stroke-linejoin="round">
            <path d="M11 4H4a2 2 0 0 0-2 2v14a2 2 0 0 0 2 2h14a2 2 0 0 0 2-2v-7"></path>
            <path d="M18.5 2.5a2.121 2.121 0 0 1 3 3L12 15l-4 1 1-4 9.5-9.5z"></path>
          </svg>
        </button>

        <button
          type="button"
          class="icon-btn danger"
          :title="t('extensions.mcp.delete')"
          @click="emit('delete', server.name)"
        >
          <svg viewBox="0 0 24 24" width="14" height="14" stroke="currentColor" stroke-width="2" fill="none" stroke-linecap="round" stroke-linejoin="round">
            <polyline points="3 6 5 6 21 6"></polyline>
            <path d="M19 6l-1 14a2 2 0 0 1-2 2H8a2 2 0 0 1-2-2L5 6"></path>
            <path d="M10 11v6"></path>
            <path d="M14 11v6"></path>
            <path d="M9 6V4a1 1 0 0 1 1-1h4a1 1 0 0 1 1 1v2"></path>
          </svg>
        </button>
      </div>
    </div>
  </div>
</template>

<style scoped>
.mcp-card {
  display: flex;
  flex-direction: column;
  gap: 12px;
  padding: 16px;
  background: var(--glass-bg);
  border: 1px solid var(--glass-border);
  border-radius: var(--radius-lg);
  transition: transform var(--transition-fast), border-color var(--transition-fast), box-shadow var(--transition-fast), background var(--transition-fast);
}

.mcp-card:hover {
  border-color: var(--accent-blue);
  transform: translateY(-2px);
  box-shadow: 0 6px 20px rgba(0, 0, 0, 0.08);
}

.mcp-card.disabled {
  opacity: 0.72;
}

.card-header {
  display: flex;
  align-items: flex-start;
  gap: 12px;
}

.server-icon {
  flex-shrink: 0;
  width: 38px;
  height: 38px;
  border-radius: var(--radius-md);
  display: flex;
  align-items: center;
  justify-content: center;
  background: var(--glass-bg-light);
  color: var(--text-muted);
  border: 1px solid var(--glass-border);
  transition: all var(--transition-fast);
}

.server-icon.status-ready {
  color: var(--accent-green, #16a34a);
  background: rgba(34, 197, 94, 0.1);
  border-color: rgba(34, 197, 94, 0.25);
}

.server-icon.status-starting {
  color: var(--accent-blue, #3b82f6);
  background: rgba(59, 130, 246, 0.1);
  border-color: rgba(59, 130, 246, 0.25);
}

.server-icon.status-failed {
  color: var(--accent-red, #ef4444);
  background: rgba(239, 68, 68, 0.1);
  border-color: rgba(239, 68, 68, 0.25);
}

.header-main {
  flex: 1;
  min-width: 0;
  display: flex;
  flex-direction: column;
  gap: 5px;
}

.title-row {
  display: flex;
  align-items: center;
  gap: 8px;
  flex-wrap: wrap;
}

.server-name {
  margin: 0;
  font-size: 0.96rem;
  font-weight: 600;
  color: var(--text-main);
  letter-spacing: -0.01em;
}

.status-pill {
  display: inline-flex;
  align-items: center;
  gap: 5px;
  font-size: 0.72rem;
  font-weight: 500;
  padding: 2px 8px;
  border-radius: 999px;
  background: var(--glass-bg-light);
  color: var(--text-muted);
}

.status-dot {
  width: 6px;
  height: 6px;
  border-radius: 50%;
  background: currentColor;
}

.status-pill.status-ready {
  background: rgba(34, 197, 94, 0.14);
  color: #16a34a;
}

.status-pill.status-idle {
  background: rgba(245, 158, 11, 0.14);
  color: #d97706;
}

.status-pill.status-starting {
  background: rgba(59, 130, 246, 0.14);
  color: var(--accent-blue, #2563eb);
}

.status-pill.status-failed {
  background: rgba(239, 68, 68, 0.14);
  color: #dc2626;
}

.sub-badges {
  display: flex;
  align-items: center;
  gap: 6px;
}

.meta-chip {
  font-size: 0.72rem;
  color: var(--text-muted);
}

.meta-chip.accent {
  padding: 1px 6px;
  border-radius: var(--radius-sm);
  background: var(--glass-bg-light);
  color: var(--text-main);
}

.mcp-toggle {
  flex-shrink: 0;
  background: none;
  border: none;
  padding: 2px;
  cursor: pointer;
  outline: none;
}

.toggle-track {
  display: block;
  width: 36px;
  height: 20px;
  border-radius: 10px;
  background: var(--toggle-track-inactive, rgba(128, 128, 128, 0.3));
  position: relative;
  transition: background var(--transition-fast);
}

.mcp-toggle.active .toggle-track {
  background: var(--accent-green, #22c55e);
}

.toggle-thumb {
  display: block;
  width: 16px;
  height: 16px;
  border-radius: 50%;
  background: #fff;
  box-shadow: 0 1px 3px rgba(0, 0, 0, 0.2);
  position: absolute;
  top: 2px;
  left: 2px;
  transition: transform var(--transition-fast);
}

.mcp-toggle.active .toggle-thumb {
  transform: translateX(16px);
}

.endpoint-box {
  padding: 7px 10px;
  border-radius: var(--radius-sm);
  background: var(--glass-bg-light);
  border: 1px solid var(--glass-border);
  overflow: hidden;
  text-overflow: ellipsis;
  white-space: nowrap;
}

.endpoint-box code {
  font-family: var(--font-mono, monospace);
  font-size: 0.75rem;
  color: var(--text-muted);
}

.error-banner {
  margin: 0;
  padding: 7px 10px;
  font-size: 0.75rem;
  color: #dc2626;
  background: rgba(239, 68, 68, 0.1);
  border: 1px solid rgba(239, 68, 68, 0.22);
  border-radius: var(--radius-sm);
  max-height: 54px;
  overflow: hidden;
  text-overflow: ellipsis;
}

.meta-bar {
  display: flex;
  align-items: center;
  justify-content: space-between;
  gap: 8px;
}

.stats-text {
  font-size: 0.78rem;
  color: var(--text-main);
  font-weight: 500;
}

.stats-text.muted {
  color: var(--text-muted);
  font-weight: 400;
}

.trust-pill {
  display: inline-flex;
  align-items: center;
  gap: 5px;
  padding: 3px 9px;
  font-size: 0.74rem;
  font-weight: 500;
  border-radius: 999px;
  border: 1px solid var(--glass-border);
  background: transparent;
  color: var(--text-muted);
  cursor: pointer;
  transition: all var(--transition-fast);
}

.trust-pill:hover {
  border-color: var(--accent-blue);
  color: var(--text-main);
  background: var(--glass-bg-light);
}

.trust-pill.trusted {
  border-color: rgba(34, 197, 94, 0.35);
  background: rgba(34, 197, 94, 0.12);
  color: #16a34a;
}

.trust-pill.trusted:hover {
  background: rgba(34, 197, 94, 0.2);
}

.card-footer {
  display: flex;
  align-items: center;
  justify-content: space-between;
  gap: 8px;
  margin-top: auto;
  padding-top: 10px;
  border-top: 1px solid var(--glass-border);
}

.footer-left,
.footer-right {
  display: flex;
  align-items: center;
  gap: 6px;
}

.ctrl-btn,
.ghost-btn,
.icon-btn {
  display: inline-flex;
  align-items: center;
  justify-content: center;
  gap: 5px;
  height: 28px;
  font-size: 0.76rem;
  font-weight: 500;
  border-radius: var(--radius-sm);
  cursor: pointer;
  transition: all var(--transition-fast);
}

.ctrl-btn {
  padding: 0 10px;
  border: 1px solid var(--glass-border);
  background: var(--glass-bg-light);
  color: var(--text-main);
}

.ctrl-btn.primary-soft:hover:not(:disabled) {
  background: var(--btn-primary-bg, var(--accent-blue));
  border-color: var(--btn-primary-bg, var(--accent-blue));
  color: var(--btn-primary-fg, #fff);
  box-shadow: 0 2px 8px rgba(59, 130, 246, 0.24);
}

.ctrl-btn.stop-btn:hover:not(:disabled) {
  background: rgba(245, 158, 11, 0.14);
  border-color: rgba(245, 158, 11, 0.45);
  color: #d97706;
}

.ctrl-btn:active:not(:disabled),
.ghost-btn:active,
.icon-btn:active {
  transform: scale(0.97);
}

.ctrl-btn:disabled {
  opacity: 0.42;
  cursor: not-allowed;
}

.ghost-btn {
  padding: 0 9px;
  border: 1px solid transparent;
  background: transparent;
  color: var(--text-muted);
}

.ghost-btn:hover {
  background: var(--glass-bg-light);
  border-color: var(--glass-border);
  color: var(--text-main);
}

.icon-btn {
  width: 28px;
  padding: 0;
  border: 1px solid transparent;
  background: transparent;
  color: var(--text-muted);
}

.icon-btn:hover {
  background: var(--glass-bg-light);
  border-color: var(--glass-border);
  color: var(--text-main);
}

.icon-btn.danger:hover {
  background: rgba(239, 68, 68, 0.12);
  border-color: rgba(239, 68, 68, 0.35);
  color: #dc2626;
}
</style>
