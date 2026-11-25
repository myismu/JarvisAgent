<!--
# McpServerEditor.vue — 添加 / 编辑 MCP 服务弹窗（表单 + 原始 JSON 导入 + 测试连接）

env 与 headers 敏感值默认以密码框遮罩显示，支持切换明文查看。
-->
<script setup lang="ts">
import { ref, watch } from 'vue';
import { useI18n } from 'vue-i18n';
import { useMcpStore } from '../../stores/mcp';
import { showToast } from '../../composables/useToast';
import type {
  McpImportItemResult,
  McpServerConfig,
  McpServerSummary,
  McpTestConfigResult,
  McpTransportKind,
} from '../../types/generated/contracts';

interface KvPair {
  key: string;
  value: string;
  revealed: boolean;
}

const props = defineProps<{
  modelValue: boolean;
  initialTab: 'form' | 'json';
  editing: McpServerSummary | null;
}>();

const emit = defineEmits<{
  (e: 'update:modelValue', v: boolean): void;
}>();

const { t } = useI18n();
const mcpStore = useMcpStore();

const activeTab = ref<'form' | 'json'>('form');
const name = ref('');
const transport = ref<McpTransportKind>('stdio');
const command = ref('');
const argsText = ref('');
const cwd = ref('');
const url = ref('');
const envPairs = ref<KvPair[]>([]);
const headerPairs = ref<KvPair[]>([]);
const startupTimeoutSec = ref(60);
const callTimeoutSec = ref(60);
const idleTimeoutMin = ref(30);
const autoStart = ref(false);
const trusted = ref(false);

const testing = ref(false);
const testResult = ref<McpTestConfigResult | null>(null);
const saving = ref(false);

const rawJson = ref('');
const importReport = ref<McpImportItemResult[] | null>(null);

const resetForm = () => {
  testResult.value = null;
  importReport.value = null;
  activeTab.value = props.editing ? 'form' : props.initialTab;

  if (props.editing) {
    const c = props.editing.config;
    name.value = props.editing.name;
    transport.value = props.editing.transport;
    command.value = c.command ?? '';
    argsText.value = c.args.join('\n');
    cwd.value = c.cwd ?? '';
    url.value = c.url ?? '';
    envPairs.value = Object.entries(c.env ?? {}).map(([k, v]) => ({
      key: k,
      value: v ?? '',
      revealed: false,
    }));
    headerPairs.value = Object.entries(c.headers ?? {}).map(([k, v]) => ({
      key: k,
      value: v ?? '',
      revealed: false,
    }));
    startupTimeoutSec.value = c.startupTimeoutSec;
    callTimeoutSec.value = c.callTimeoutSec;
    idleTimeoutMin.value = c.idleTimeoutMin ?? 30;
    autoStart.value = c.autoStart;
    trusted.value = c.trusted;
  } else {
    name.value = '';
    transport.value = 'stdio';
    command.value = '';
    argsText.value = '';
    cwd.value = '';
    url.value = '';
    envPairs.value = [];
    headerPairs.value = [];
    startupTimeoutSec.value = 60;
    callTimeoutSec.value = 60;
    idleTimeoutMin.value = 30;
    autoStart.value = false;
    trusted.value = false;
    rawJson.value = '';
  }
};

watch(
  () => props.modelValue,
  (open) => {
    if (open) resetForm();
  },
);

const pairsToRecord = (pairs: KvPair[]): Record<string, string> => {
  const out: Record<string, string> = {};
  for (const p of pairs) {
    const k = p.key.trim();
    if (k) out[k] = p.value;
  }
  return out;
};

const buildConfig = (): McpServerConfig => {
  const isStdio = transport.value === 'stdio';
  return {
    command: isStdio ? command.value.trim() : undefined,
    args: isStdio
      ? argsText.value
          .split(/\r?\n/)
          .map((s) => s.trim())
          .filter(Boolean)
      : [],
    env: isStdio ? pairsToRecord(envPairs.value) : {},
    cwd: isStdio && cwd.value.trim() ? cwd.value.trim() : undefined,
    url: !isStdio ? url.value.trim() : undefined,
    headers: !isStdio ? pairsToRecord(headerPairs.value) : {},
    enabled: props.editing ? props.editing.config.enabled : true,
    trusted: trusted.value,
    autoStart: autoStart.value,
    isolation: props.editing ? props.editing.config.isolation : 'global',
    startupTimeoutSec: Number(startupTimeoutSec.value) || 60,
    callTimeoutSec: Number(callTimeoutSec.value) || 60,
    idleTimeoutMin: Math.max(0, Number(idleTimeoutMin.value) || 0),
    disabledTools: props.editing ? props.editing.config.disabledTools : [],
  };
};

const handleTest = async () => {
  testing.value = true;
  testResult.value = null;
  try {
    testResult.value = await mcpStore.testConfig(buildConfig());
  } catch (err) {
    testResult.value = {
      ok: false,
      toolCount: 0,
      resourceCount: 0,
      promptCount: 0,
      message: String(err),
    };
  } finally {
    testing.value = false;
  }
};

const handleSave = async () => {
  saving.value = true;
  try {
    const trimmed = name.value.trim();
    await mcpStore.saveServer(trimmed, buildConfig());
    showToast(t('extensions.mcp.saved', { name: trimmed }));
    emit('update:modelValue', false);
  } catch (err) {
    showToast(t('extensions.mcp.saveFailed', { error: String(err) }), 'error');
  } finally {
    saving.value = false;
  }
};

const handleImport = async () => {
  saving.value = true;
  importReport.value = null;
  try {
    const report = await mcpStore.importJson(rawJson.value);
    importReport.value = report;
    const okCount = report.filter((r) => r.ok).length;
    const failCount = report.length - okCount;
    showToast(t('extensions.mcp.editor.importSuccess', { ok: okCount, fail: failCount }));
    if (failCount === 0) {
      emit('update:modelValue', false);
    }
  } catch (err) {
    showToast(String(err), 'error');
  } finally {
    saving.value = false;
  }
};

const close = () => emit('update:modelValue', false);
</script>

<template>
  <div v-if="modelValue" class="modal-backdrop" @click.self="close">
    <div class="modal-box">
      <div class="modal-header">
        <h3>{{ editing ? t('extensions.mcp.editor.editTitle') : t('extensions.mcp.editor.addTitle') }}</h3>
        <div v-if="!editing" class="mini-tabs">
          <button :class="{ active: activeTab === 'form' }" @click="activeTab = 'form'">
            {{ t('extensions.mcp.editor.tabForm') }}
          </button>
          <button :class="{ active: activeTab === 'json' }" @click="activeTab = 'json'">
            {{ t('extensions.mcp.editor.tabJson') }}
          </button>
        </div>
      </div>

      <div v-if="activeTab === 'form'" class="modal-body">
        <div class="field">
          <label>{{ t('extensions.mcp.editor.name') }}</label>
          <input
            v-model="name"
            type="text"
            :disabled="Boolean(editing)"
            :placeholder="t('extensions.mcp.editor.namePlaceholder')"
          />
        </div>

        <div class="field">
          <label>{{ t('extensions.mcp.editor.transport') }}</label>
          <select v-model="transport">
            <option value="stdio">{{ t('extensions.mcp.transport.stdio') }}</option>
            <option value="streamable_http">{{ t('extensions.mcp.transport.streamable_http') }}</option>
          </select>
        </div>

        <template v-if="transport === 'stdio'">
          <div class="field">
            <label>{{ t('extensions.mcp.editor.command') }}</label>
            <input v-model="command" type="text" :placeholder="t('extensions.mcp.editor.commandPlaceholder')" />
          </div>
          <div class="field">
            <label>{{ t('extensions.mcp.editor.args') }}</label>
            <textarea v-model="argsText" rows="3" :placeholder="t('extensions.mcp.editor.argsPlaceholder')"></textarea>
          </div>
          <div class="field">
            <label>{{ t('extensions.mcp.editor.cwd') }}</label>
            <input v-model="cwd" type="text" />
          </div>
          <div class="field">
            <div class="field-head">
              <label>{{ t('extensions.mcp.editor.env') }}</label>
              <button type="button" class="link-btn" @click="envPairs.push({ key: '', value: '', revealed: false })">
                + {{ t('extensions.mcp.editor.addPair') }}
              </button>
            </div>
            <div v-for="(pair, idx) in envPairs" :key="idx" class="kv-row">
              <input v-model="pair.key" type="text" :placeholder="t('extensions.mcp.editor.keyPlaceholder')" />
              <input
                v-model="pair.value"
                :type="pair.revealed ? 'text' : 'password'"
                :placeholder="t('extensions.mcp.editor.valuePlaceholder')"
              />
              <button type="button" class="small-btn" @click="pair.revealed = !pair.revealed">
                {{ pair.revealed ? t('extensions.mcp.editor.hideSecret') : t('extensions.mcp.editor.showSecret') }}
              </button>
              <button type="button" class="small-btn danger" @click="envPairs.splice(idx, 1)">x</button>
            </div>
          </div>
        </template>

        <template v-else>
          <div class="field">
            <label>{{ t('extensions.mcp.editor.url') }}</label>
            <input v-model="url" type="text" :placeholder="t('extensions.mcp.editor.urlPlaceholder')" />
          </div>
          <div class="field">
            <div class="field-head">
              <label>{{ t('extensions.mcp.editor.headers') }}</label>
              <button type="button" class="link-btn" @click="headerPairs.push({ key: '', value: '', revealed: false })">
                + {{ t('extensions.mcp.editor.addPair') }}
              </button>
            </div>
            <div v-for="(pair, idx) in headerPairs" :key="idx" class="kv-row">
              <input v-model="pair.key" type="text" :placeholder="t('extensions.mcp.editor.keyPlaceholder')" />
              <input
                v-model="pair.value"
                :type="pair.revealed ? 'text' : 'password'"
                :placeholder="t('extensions.mcp.editor.valuePlaceholder')"
              />
              <button type="button" class="small-btn" @click="pair.revealed = !pair.revealed">
                {{ pair.revealed ? t('extensions.mcp.editor.hideSecret') : t('extensions.mcp.editor.showSecret') }}
              </button>
              <button type="button" class="small-btn danger" @click="headerPairs.splice(idx, 1)">x</button>
            </div>
          </div>
        </template>

        <div class="two-col">
          <div class="field">
            <label>{{ t('extensions.mcp.editor.startupTimeout') }}</label>
            <input v-model.number="startupTimeoutSec" type="number" min="1" max="600" />
          </div>
          <div class="field">
            <label>{{ t('extensions.mcp.editor.callTimeout') }}</label>
            <input v-model.number="callTimeoutSec" type="number" min="1" max="600" />
          </div>
        </div>

        <div class="field">
          <label>{{ t('extensions.mcp.editor.idleTimeout') }}</label>
          <input v-model.number="idleTimeoutMin" type="number" min="0" max="1440" />
        </div>

        <label class="check-row">
          <input v-model="autoStart" type="checkbox" />
          <span>{{ t('extensions.mcp.editor.autoStart') }}</span>
        </label>
        <label class="check-row">
          <input v-model="trusted" type="checkbox" />
          <span>{{ t('extensions.mcp.editor.trusted') }}</span>
        </label>

        <div v-if="testResult" class="test-result" :class="{ ok: testResult.ok, fail: !testResult.ok }">
          {{ testResult.message }}
        </div>
      </div>

      <div v-else class="modal-body">
        <textarea
          v-model="rawJson"
          rows="12"
          class="json-area"
          :placeholder="t('extensions.mcp.editor.jsonPlaceholder')"
        ></textarea>
        <div v-if="importReport" class="import-report">
          <div v-for="item in importReport" :key="item.originalName" class="report-row" :class="{ fail: !item.ok }">
            <span>{{ item.originalName }} -&gt; {{ item.savedName }}</span>
            <span>{{ item.ok ? (item.message || 'OK') : item.message }}</span>
          </div>
        </div>
      </div>

      <div class="modal-footer">
        <button v-if="activeTab === 'form'" class="btn" :disabled="testing" @click="handleTest">
          {{ testing ? t('extensions.mcp.editor.testing') : t('extensions.mcp.editor.testConnection') }}
        </button>
        <div class="spacer"></div>
        <button class="btn" @click="close">{{ t('extensions.mcp.editor.cancel') }}</button>
        <button
          v-if="activeTab === 'form'"
          class="btn primary"
          :disabled="saving || !name.trim()"
          @click="handleSave"
        >
          {{ t('extensions.mcp.editor.save') }}
        </button>
        <button
          v-else
          class="btn primary"
          :disabled="saving || !rawJson.trim()"
          @click="handleImport"
        >
          {{ t('extensions.mcp.editor.importSubmit') }}
        </button>
      </div>
    </div>
  </div>
</template>

<style scoped>
.modal-backdrop {
  position: fixed;
  inset: 0;
  background: rgba(0, 0, 0, 0.45);
  backdrop-filter: blur(4px);
  display: flex;
  align-items: center;
  justify-content: center;
  z-index: 1000;
}

.modal-box {
  width: min(620px, 92vw);
  max-height: 86vh;
  display: flex;
  flex-direction: column;
  background: var(--glass-bg-heavy);
  border: 1px solid var(--glass-border);
  border-radius: var(--radius-lg);
  box-shadow: 0 20px 48px rgba(0, 0, 0, 0.24);
  overflow: hidden;
}

.modal-header {
  display: flex;
  align-items: center;
  justify-content: space-between;
  padding: 14px 18px;
  border-bottom: 1px solid var(--glass-border);
}

.modal-header h3 {
  margin: 0;
  font-size: 1rem;
  color: var(--text-main);
}

.mini-tabs {
  display: flex;
  gap: 4px;
}

.mini-tabs button {
  padding: 4px 10px;
  font-size: 0.78rem;
  border: 1px solid var(--glass-border);
  border-radius: var(--radius-sm);
  background: transparent;
  color: var(--text-muted);
  cursor: pointer;
}

.mini-tabs button.active {
  background: var(--glass-bg);
  color: var(--text-main);
  border-color: var(--accent-blue);
}

.modal-body {
  padding: 16px 18px;
  overflow-y: auto;
  display: flex;
  flex-direction: column;
  gap: 12px;
}

.field {
  display: flex;
  flex-direction: column;
  gap: 5px;
}

.field-head {
  display: flex;
  justify-content: space-between;
  align-items: center;
}

.field label {
  font-size: 0.8rem;
  color: var(--text-muted);
}

.field input,
.field select,
.field textarea,
.json-area {
  padding: 7px 10px;
  border-radius: var(--radius-sm);
  border: 1px solid var(--glass-border);
  background: var(--glass-bg-light);
  color: var(--text-main);
  font-size: 0.84rem;
}

.json-area {
  font-family: var(--font-mono, monospace);
}

.two-col {
  display: grid;
  grid-template-columns: 1fr 1fr;
  gap: 12px;
}

.kv-row {
  display: grid;
  grid-template-columns: 1fr 1.4fr auto auto;
  gap: 6px;
}

.check-row {
  display: flex;
  align-items: center;
  gap: 8px;
  font-size: 0.82rem;
  color: var(--text-main);
  cursor: pointer;
}

.test-result {
  padding: 8px 10px;
  border-radius: var(--radius-sm);
  font-size: 0.8rem;
  white-space: pre-wrap;
}

.test-result.ok {
  background: rgba(34, 197, 94, 0.12);
  color: var(--accent-green, #22c55e);
}

.test-result.fail {
  background: rgba(239, 68, 68, 0.12);
  color: var(--accent-red, #ef4444);
}

.import-report {
  display: flex;
  flex-direction: column;
  gap: 4px;
  font-size: 0.78rem;
}

.report-row {
  display: flex;
  justify-content: space-between;
  padding: 4px 8px;
  border-radius: var(--radius-sm);
  background: var(--glass-bg-light);
}

.report-row.fail {
  color: var(--accent-red, #ef4444);
}

.modal-footer {
  display: flex;
  align-items: center;
  gap: 8px;
  padding: 12px 18px;
  border-top: 1px solid var(--glass-border);
}

.spacer {
  flex: 1;
}

.btn,
.small-btn,
.link-btn {
  padding: 6px 13px;
  font-size: 0.8rem;
  font-weight: 500;
  border-radius: var(--radius-sm);
  border: 1px solid var(--glass-border);
  background: var(--glass-bg-light);
  color: var(--text-main);
  cursor: pointer;
  transition: all var(--transition-fast);
}

.btn:hover:not(:disabled),
.small-btn:hover:not(:disabled) {
  border-color: var(--accent-blue);
  background: var(--glass-bg);
}

.btn:active:not(:disabled),
.small-btn:active:not(:disabled) {
  transform: scale(0.98);
}

.small-btn {
  padding: 4px 8px;
  font-size: 0.74rem;
}

.small-btn.danger {
  color: #dc2626;
}

.small-btn.danger:hover {
  background: rgba(239, 68, 68, 0.12);
  border-color: rgba(239, 68, 68, 0.35);
}

.link-btn {
  border: none;
  background: transparent;
  color: var(--accent-blue);
  padding: 2px 6px;
}

.link-btn:hover {
  text-decoration: underline;
}

.btn.primary {
  background: var(--btn-primary-bg, var(--accent-blue));
  border-color: transparent;
  color: var(--btn-primary-fg, #fff);
}

.btn.primary:hover:not(:disabled) {
  background: var(--btn-primary-bg, var(--accent-blue));
  border-color: transparent;
  filter: brightness(1.08);
  box-shadow: 0 3px 10px rgba(59, 130, 246, 0.25);
}

.btn:disabled {
  opacity: 0.5;
  cursor: not-allowed;
}
</style>
