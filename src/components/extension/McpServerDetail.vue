<!--
# McpServerDetail.vue — MCP 服务详情抽屉（工具 / 资源 / 模板 / 日志）
-->
<script setup lang="ts">
import { ref, watch } from 'vue';
import { useI18n } from 'vue-i18n';
import { useMcpStore } from '../../stores/mcp';
import { showToast } from '../../composables/useToast';
import type {
  McpPromptItemView,
  McpResourceItemView,
  McpToolView,
} from '../../types/generated/contracts';

const props = defineProps<{
  modelValue: boolean;
  serverName: string | null;
}>();

const emit = defineEmits<{
  (e: 'update:modelValue', v: boolean): void;
}>();

const { t } = useI18n();
const mcpStore = useMcpStore();

type DetailTab = 'tools' | 'resources' | 'prompts' | 'logs';
const activeTab = ref<DetailTab>('tools');

const tools = ref<McpToolView[]>([]);
const resources = ref<McpResourceItemView[]>([]);
const prompts = ref<McpPromptItemView[]>([]);
const logs = ref<string[]>([]);
const expandedTools = ref<Set<string>>(new Set());

const previewTitle = ref<string | null>(null);
const previewContent = ref<string>('');

const loadAll = async () => {
  const srv = props.serverName;
  if (!srv) return;
  previewTitle.value = null;
  previewContent.value = '';
  try {
    const [tList, rList, pList, logList] = await Promise.all([
      mcpStore.listTools(srv),
      mcpStore.listResources(srv),
      mcpStore.listPrompts(srv),
      mcpStore.getServerLogs(srv),
    ]);
    tools.value = tList;
    resources.value = rList;
    prompts.value = pList;
    logs.value = logList;
  } catch (err) {
    showToast(String(err), 'error');
  }
};

watch(
  () => [props.modelValue, props.serverName],
  ([open]) => {
    if (open) {
      activeTab.value = 'tools';
      void loadAll();
    }
  },
);

const toggleTool = async (tool: McpToolView) => {
  const next = !tool.enabled;
  try {
    await mcpStore.setToolEnabled(tool.serverName, tool.name, next);
    tool.enabled = next;
  } catch (err) {
    showToast(String(err), 'error');
  }
};

const toggleSchema = (name: string) => {
  const next = new Set(expandedTools.value);
  if (next.has(name)) next.delete(name);
  else next.add(name);
  expandedTools.value = next;
};

interface ParamRow {
  name: string;
  type: string;
  required: boolean;
  description: string;
}

const paramsOf = (tool: McpToolView): ParamRow[] => {
  const schema = tool.inputSchema as {
    properties?: Record<string, { type?: string; description?: string }>;
    required?: string[];
  };
  const propsObj = schema?.properties ?? {};
  const required = schema?.required ?? [];
  return Object.entries(propsObj).map(([k, v]) => ({
    name: k,
    type: v?.type ?? 'any',
    required: required.includes(k),
    description: v?.description ?? '',
  }));
};

const handlePreviewResource = async (res: McpResourceItemView) => {
  try {
    const data = await mcpStore.readResource(res.serverName, res.uri);
    previewTitle.value = res.uri;
    previewContent.value = data.content;
  } catch (err) {
    showToast(String(err), 'error');
  }
};

const handlePreviewPrompt = async (prompt: McpPromptItemView) => {
  try {
    const data = await mcpStore.getPrompt(prompt.serverName, prompt.name, {});
    previewTitle.value = prompt.name;
    previewContent.value = data.combinedText;
  } catch (err) {
    showToast(String(err), 'error');
  }
};

const refreshLogs = async () => {
  if (!props.serverName) return;
  logs.value = await mcpStore.getServerLogs(props.serverName);
};

const close = () => emit('update:modelValue', false);
</script>

<template>
  <div v-if="modelValue && serverName" class="drawer-backdrop" @click.self="close">
    <div class="drawer">
      <div class="drawer-header">
        <div>
          <h3>{{ serverName }}</h3>
          <p class="shared-note">{{ t('extensions.mcp.detailPanel.sharedNote') }}</p>
        </div>
        <button class="close-btn" @click="close">{{ t('skillManager.close') }}</button>
      </div>

      <div class="drawer-tabs">
        <button :class="{ active: activeTab === 'tools' }" @click="activeTab = 'tools'">
          {{ t('extensions.mcp.detailPanel.tabTools') }} ({{ tools.length }})
        </button>
        <button :class="{ active: activeTab === 'resources' }" @click="activeTab = 'resources'">
          {{ t('extensions.mcp.detailPanel.tabResources') }} ({{ resources.length }})
        </button>
        <button :class="{ active: activeTab === 'prompts' }" @click="activeTab = 'prompts'">
          {{ t('extensions.mcp.detailPanel.tabPrompts') }} ({{ prompts.length }})
        </button>
        <button :class="{ active: activeTab === 'logs' }" @click="activeTab = 'logs'">
          {{ t('extensions.mcp.detailPanel.tabLogs') }} ({{ logs.length }})
        </button>
      </div>

      <div class="drawer-body">
        <template v-if="activeTab === 'tools'">
          <p v-if="tools.length === 0" class="empty-msg">{{ t('extensions.mcp.detailPanel.noTools') }}</p>
          <div v-for="tool in tools" :key="tool.qualifiedName" class="item-card">
            <div class="item-head">
              <div class="item-title" @click="toggleSchema(tool.qualifiedName)">
                <strong>{{ tool.name }}</strong>
                <span class="mono-sub">{{ tool.qualifiedName }}</span>
              </div>
              <input type="checkbox" :checked="tool.enabled" @change="toggleTool(tool)" />
            </div>
            <p class="item-desc">{{ tool.description }}</p>
            <div v-if="expandedTools.has(tool.qualifiedName)" class="schema-box">
              <div v-if="paramsOf(tool).length === 0" class="muted">
                {{ t('extensions.mcp.detailPanel.noParams') }}
              </div>
              <div v-for="p in paramsOf(tool)" :key="p.name" class="param-row">
                <span class="mono">{{ p.name }}</span>
                <span class="badge">{{ p.type }}</span>
                <span v-if="p.required" class="req">{{ t('extensions.mcp.detailPanel.required') }}</span>
                <span class="param-desc">{{ p.description }}</span>
              </div>
            </div>
          </div>
        </template>

        <template v-else-if="activeTab === 'resources'">
          <p v-if="resources.length === 0" class="empty-msg">{{ t('extensions.mcp.detailPanel.noResources') }}</p>
          <div v-for="res in resources" :key="res.uri" class="item-card">
            <div class="item-head">
              <div>
                <strong>{{ res.name }}</strong>
                <div class="mono-sub">{{ res.uri }}</div>
              </div>
              <button class="small-btn" @click="handlePreviewResource(res)">
                {{ t('extensions.mcp.detailPanel.preview') }}
              </button>
            </div>
            <p v-if="res.description" class="item-desc">{{ res.description }}</p>
          </div>
        </template>

        <template v-else-if="activeTab === 'prompts'">
          <p v-if="prompts.length === 0" class="empty-msg">{{ t('extensions.mcp.detailPanel.noPrompts') }}</p>
          <div v-for="p in prompts" :key="p.name" class="item-card">
            <div class="item-head">
              <strong>{{ p.name }}</strong>
              <button class="small-btn" @click="handlePreviewPrompt(p)">
                {{ t('extensions.mcp.detailPanel.render') }}
              </button>
            </div>
            <p v-if="p.description" class="item-desc">{{ p.description }}</p>
          </div>
        </template>

        <template v-else-if="activeTab === 'logs'">
          <div class="logs-head">
            <button class="small-btn" @click="refreshLogs">
              {{ t('extensions.mcp.detailPanel.refreshLogs') }}
            </button>
          </div>
          <p v-if="logs.length === 0" class="empty-msg">{{ t('extensions.mcp.detailPanel.noLogs') }}</p>
          <pre v-else class="log-box">{{ logs.join('\n') }}</pre>
        </template>

        <div v-if="previewTitle" class="preview-box">
          <div class="preview-head">
            <strong>{{ previewTitle }}</strong>
            <button class="small-btn" @click="previewTitle = null">x</button>
          </div>
          <pre class="preview-content">{{ previewContent }}</pre>
        </div>
      </div>
    </div>
  </div>
</template>

<style scoped>
.drawer-backdrop {
  position: fixed;
  inset: 0;
  background: rgba(0, 0, 0, 0.4);
  backdrop-filter: blur(3px);
  display: flex;
  justify-content: flex-end;
  z-index: 1000;
}

.drawer {
  width: min(560px, 92vw);
  height: 100%;
  display: flex;
  flex-direction: column;
  background: var(--glass-bg-heavy);
  border-left: 1px solid var(--glass-border);
  box-shadow: -12px 0 36px rgba(0, 0, 0, 0.18);
}

.drawer-header {
  display: flex;
  justify-content: space-between;
  align-items: flex-start;
  gap: 12px;
  padding: 16px 18px;
  border-bottom: 1px solid var(--glass-border);
}

.drawer-header h3 {
  margin: 0 0 4px 0;
  color: var(--text-main);
}

.shared-note {
  margin: 0;
  font-size: 0.75rem;
  color: var(--text-muted);
}

.close-btn,
.small-btn {
  padding: 5px 11px;
  font-size: 0.78rem;
  font-weight: 500;
  border: 1px solid var(--glass-border);
  border-radius: var(--radius-sm);
  background: var(--glass-bg-light);
  color: var(--text-main);
  cursor: pointer;
  transition: all var(--transition-fast);
}

.close-btn:hover,
.small-btn:hover {
  border-color: var(--accent-blue);
  background: var(--glass-bg);
}

.drawer-tabs {
  display: flex;
  gap: 6px;
  padding: 10px 18px;
  border-bottom: 1px solid var(--glass-border);
}

.drawer-tabs button {
  padding: 6px 11px;
  font-size: 0.8rem;
  font-weight: 500;
  border: 1px solid transparent;
  border-radius: var(--radius-sm);
  background: transparent;
  color: var(--text-muted);
  cursor: pointer;
  transition: all var(--transition-fast);
}

.drawer-tabs button:hover {
  color: var(--text-main);
  background: var(--glass-bg-light);
}

.drawer-tabs button.active {
  background: var(--glass-bg);
  border-color: var(--glass-border);
  color: var(--text-main);
  box-shadow: 0 1px 3px rgba(0, 0, 0, 0.08);
}

.drawer-body {
  flex: 1;
  overflow-y: auto;
  padding: 14px 18px;
  display: flex;
  flex-direction: column;
  gap: 10px;
}

.item-card {
  padding: 10px 12px;
  border: 1px solid var(--glass-border);
  border-radius: var(--radius-sm);
  background: var(--glass-bg);
}

.item-head {
  display: flex;
  justify-content: space-between;
  align-items: center;
  gap: 8px;
}

.item-title {
  display: flex;
  flex-direction: column;
  cursor: pointer;
}

.mono-sub,
.mono {
  font-family: var(--font-mono, monospace);
  font-size: 0.74rem;
  color: var(--text-muted);
}

.item-desc {
  margin: 6px 0 0 0;
  font-size: 0.8rem;
  color: var(--text-muted);
}

.schema-box {
  margin-top: 8px;
  padding-top: 8px;
  border-top: 1px dashed var(--glass-border);
  display: flex;
  flex-direction: column;
  gap: 4px;
}

.param-row {
  display: flex;
  flex-wrap: wrap;
  align-items: center;
  gap: 6px;
  font-size: 0.76rem;
}

.badge {
  padding: 1px 5px;
  border-radius: 3px;
  background: var(--glass-bg-light);
  color: var(--text-muted);
}

.req {
  color: var(--accent-red, #ef4444);
  font-size: 0.72rem;
}

.param-desc {
  color: var(--text-muted);
}

.empty-msg,
.muted {
  color: var(--text-muted);
  font-size: 0.82rem;
}

.logs-head {
  display: flex;
  justify-content: flex-end;
}

.log-box,
.preview-content {
  margin: 0;
  padding: 10px;
  background: var(--glass-bg-light);
  border-radius: var(--radius-sm);
  font-family: var(--font-mono, monospace);
  font-size: 0.74rem;
  white-space: pre-wrap;
  word-break: break-all;
  max-height: 360px;
  overflow-y: auto;
}

.preview-box {
  margin-top: 8px;
  border: 1px solid var(--accent-blue);
  border-radius: var(--radius-sm);
  padding: 10px;
}

.preview-head {
  display: flex;
  justify-content: space-between;
  align-items: center;
  margin-bottom: 6px;
}
</style>
