<!--
# McpPanel.vue — MCP 服务管理主面板

展示生效时机提示、操作栏（添加服务 / 导入 JSON / 刷新）和服务卡片网格，
挂载编辑弹窗（McpServerEditor）和详情抽屉（McpServerDetail）。
-->
<script setup lang="ts">
import { ref } from 'vue';
import { useI18n } from 'vue-i18n';
import { useMcpStore } from '../../stores/mcp';
import { showToast } from '../../composables/useToast';
import { extractCommandErrorMessage } from '../../utils/commandError';
import type { McpServerSummary } from '../../types/generated/contracts';
import McpServerCard from './McpServerCard.vue';
import McpServerEditor from './McpServerEditor.vue';
import McpServerDetail from './McpServerDetail.vue';
import ConfirmModal from '../common/ConfirmModal.vue';

const { t } = useI18n();
const mcpStore = useMcpStore();

const showEditor = ref(false);
const editorInitialTab = ref<'form' | 'json'>('form');
const editingServer = ref<McpServerSummary | null>(null);

const showDetail = ref(false);
const detailServerName = ref<string | null>(null);

const deletingServer = ref<string | null>(null);
const restartingNames = ref<Set<string>>(new Set());

const openAddForm = () => {
  editingServer.value = null;
  editorInitialTab.value = 'form';
  showEditor.value = true;
};

const openImportJson = () => {
  editingServer.value = null;
  editorInitialTab.value = 'json';
  showEditor.value = true;
};

const openEdit = (server: McpServerSummary) => {
  editingServer.value = server;
  editorInitialTab.value = 'form';
  showEditor.value = true;
};

const openDetail = (name: string) => {
  detailServerName.value = name;
  showDetail.value = true;
};

const handleToggleEnabled = async (name: string, enabled: boolean) => {
  try {
    await mcpStore.setServerEnabled(name, enabled);
  } catch (err) {
    showToast(extractCommandErrorMessage(err), 'error');
  }
};

const handleToggleTrusted = async (name: string, trusted: boolean) => {
  try {
    await mcpStore.setServerTrusted(name, trusted);
  } catch (err) {
    showToast(extractCommandErrorMessage(err), 'error');
  }
};

const handleRestart = async (name: string) => {
  const next = new Set(restartingNames.value);
  next.add(name);
  restartingNames.value = next;
  try {
    await mcpStore.restartServer(name);
    showToast(t('extensions.mcp.restarted', { name }));
  } catch (err) {
    showToast(t('extensions.mcp.restartFailed', { error: extractCommandErrorMessage(err) }), 'error');
  } finally {
    const done = new Set(restartingNames.value);
    done.delete(name);
    restartingNames.value = done;
  }
};

const handleStop = async (name: string) => {
  try {
    await mcpStore.stopServer(name);
    showToast(t('extensions.mcp.stopped', { name }));
  } catch (err) {
    showToast(extractCommandErrorMessage(err), 'error');
  }
};

const confirmDelete = async () => {
  const target = deletingServer.value;
  if (!target) return;
  deletingServer.value = null;
  try {
    await mcpStore.deleteServer(target);
    showToast(t('extensions.mcp.deleted', { name: target }));
  } catch (err) {
    showToast(extractCommandErrorMessage(err), 'error');
  }
};
</script>

<template>
  <div class="mcp-panel">
    <div class="mcp-toolbar">
      <div class="hint-wrap">
        <svg class="hint-icon" viewBox="0 0 24 24" width="15" height="15" stroke="currentColor" stroke-width="2" fill="none" stroke-linecap="round" stroke-linejoin="round">
          <circle cx="12" cy="12" r="10"></circle>
          <line x1="12" y1="16" x2="12" y2="12"></line>
          <line x1="12" y1="8" x2="12.01" y2="8"></line>
        </svg>
        <p class="effect-hint">{{ t('extensions.mcp.effectHint') }}</p>
      </div>
      <div class="toolbar-actions">
        <button type="button" class="toolbar-btn primary" @click="openAddForm">
          <svg viewBox="0 0 24 24" width="14" height="14" stroke="currentColor" stroke-width="2.2" fill="none" stroke-linecap="round">
            <line x1="12" y1="5" x2="12" y2="19"></line>
            <line x1="5" y1="12" x2="19" y2="12"></line>
          </svg>
          <span>{{ t('extensions.mcp.addServer') }}</span>
        </button>
        <button type="button" class="toolbar-btn" @click="openImportJson">
          <svg viewBox="0 0 24 24" width="14" height="14" stroke="currentColor" stroke-width="2" fill="none" stroke-linecap="round" stroke-linejoin="round">
            <polyline points="16 18 22 12 16 6"></polyline>
            <polyline points="8 6 2 12 8 18"></polyline>
          </svg>
          <span>{{ t('extensions.mcp.importJson') }}</span>
        </button>
        <button
          type="button"
          class="toolbar-btn icon-only"
          :title="t('extensions.mcp.refresh')"
          @click="mcpStore.loadServers()"
        >
          <svg viewBox="0 0 24 24" width="14" height="14" stroke="currentColor" stroke-width="2" fill="none" stroke-linecap="round" stroke-linejoin="round">
            <polyline points="23 4 23 10 17 10"></polyline>
            <path d="M20.49 15a9 9 0 1 1-2.12-9.36L23 10"></path>
          </svg>
        </button>
      </div>
    </div>

    <div v-if="mcpStore.loading && mcpStore.servers.length === 0" class="mcp-state">
      <span>{{ t('skillManager.loading') }}</span>
    </div>

    <div v-else-if="mcpStore.error" class="mcp-state error-text">
      <p>{{ mcpStore.error }}</p>
    </div>

    <div v-else-if="mcpStore.servers.length === 0" class="mcp-state empty-box">
      <div class="empty-icon">
        <svg viewBox="0 0 24 24" width="28" height="28" stroke="currentColor" stroke-width="1.6" fill="none" stroke-linecap="round" stroke-linejoin="round">
          <rect x="2" y="3" width="20" height="7" rx="2"></rect>
          <rect x="2" y="14" width="20" height="7" rx="2"></rect>
          <line x1="6" y1="6.5" x2="6.01" y2="6.5"></line>
          <line x1="6" y1="17.5" x2="6.01" y2="17.5"></line>
        </svg>
      </div>
      <p class="empty-title">{{ t('extensions.mcp.empty') }}</p>
      <p class="empty-hint">{{ t('extensions.mcp.emptyHint') }}</p>
    </div>

    <div v-else class="mcp-grid">
      <McpServerCard
        v-for="srv in mcpStore.servers"
        :key="srv.name"
        :server="srv"
        :restarting="restartingNames.has(srv.name)"
        @toggle-enabled="handleToggleEnabled"
        @toggle-trusted="handleToggleTrusted"
        @restart="handleRestart"
        @stop="handleStop"
        @detail="openDetail"
        @edit="openEdit"
        @delete="deletingServer = $event"
      />
    </div>

    <McpServerEditor
      v-model="showEditor"
      :initial-tab="editorInitialTab"
      :editing="editingServer"
    />

    <McpServerDetail
      v-model="showDetail"
      :server-name="detailServerName"
    />

    <ConfirmModal
      :open="Boolean(deletingServer)"
      :title="t('extensions.mcp.delete')"
      :message="deletingServer ? t('extensions.mcp.deleteConfirm', { name: deletingServer }) : ''"
      confirm-kind="danger"
      @confirm="confirmDelete"
      @cancel="deletingServer = null"
    />
  </div>
</template>

<style scoped>
.mcp-panel {
  display: flex;
  flex-direction: column;
  gap: 18px;
}

.mcp-toolbar {
  display: flex;
  flex-wrap: wrap;
  align-items: center;
  justify-content: space-between;
  gap: 14px;
  padding: 12px 16px;
  background: var(--glass-bg);
  border: 1px solid var(--glass-border);
  border-radius: var(--radius-lg);
}

.hint-wrap {
  display: flex;
  align-items: center;
  gap: 8px;
  flex: 1;
  min-width: 240px;
}

.hint-icon {
  flex-shrink: 0;
  color: var(--text-muted);
  opacity: 0.75;
}

.effect-hint {
  margin: 0;
  font-size: 0.8rem;
  color: var(--text-muted);
  line-height: 1.5;
}

.toolbar-actions {
  display: flex;
  align-items: center;
  gap: 8px;
  flex-shrink: 0;
}

.toolbar-btn {
  display: inline-flex;
  align-items: center;
  justify-content: center;
  gap: 6px;
  height: 32px;
  padding: 0 12px;
  font-size: 0.8rem;
  font-weight: 500;
  border-radius: var(--radius-md);
  border: 1px solid var(--glass-border);
  background: transparent;
  color: var(--text-main);
  cursor: pointer;
  transition: all var(--transition-fast);
}

.toolbar-btn.icon-only {
  width: 32px;
  padding: 0;
  color: var(--text-muted);
}

.toolbar-btn:hover {
  background: var(--glass-bg-light);
  border-color: var(--accent-blue);
  color: var(--text-main);
  transform: translateY(-1px);
  box-shadow: 0 2px 8px rgba(0, 0, 0, 0.08);
}

.toolbar-btn:active {
  transform: translateY(0);
}

.toolbar-btn.primary {
  background: var(--btn-primary-bg, var(--accent-blue));
  border-color: transparent;
  color: var(--btn-primary-fg, #fff);
}

.toolbar-btn.primary:hover {
  background: var(--btn-primary-bg, var(--accent-blue));
  border-color: transparent;
  color: var(--btn-primary-fg, #fff);
  filter: brightness(1.08);
  box-shadow: 0 4px 12px rgba(59, 130, 246, 0.28);
}

.mcp-grid {
  display: grid;
  grid-template-columns: repeat(auto-fill, minmax(340px, 1fr));
  gap: 16px;
}

.mcp-state {
  display: flex;
  flex-direction: column;
  align-items: center;
  justify-content: center;
  gap: 10px;
  padding: 56px 20px;
  color: var(--text-muted);
  text-align: center;
}

.empty-box {
  border: 1px dashed var(--glass-border);
  border-radius: var(--radius-lg);
  background: var(--glass-bg);
}

.empty-icon {
  width: 52px;
  height: 52px;
  display: flex;
  align-items: center;
  justify-content: center;
  border-radius: 50%;
  background: var(--glass-bg-light);
  color: var(--text-muted);
  margin-bottom: 4px;
}

.empty-title {
  margin: 0;
  font-size: 0.96rem;
  font-weight: 600;
  color: var(--text-main);
}

.empty-hint {
  margin: 0;
  font-size: 0.82rem;
  max-width: 460px;
  line-height: 1.5;
}

.error-text {
  color: var(--accent-red, #ef4444);
}
</style>
