<!--
# McpPromptPickerModal.vue — 聊天输入框的 MCP Prompt 模板选择与参数渲染弹窗

用户选择已启用 MCP 服务提供的 Prompt 模板，填写参数后渲染，
结果填入聊天输入框草稿由用户确认后再发送（不自动发送）。
-->
<script setup lang="ts">
import { ref, watch } from 'vue';
import { useI18n } from 'vue-i18n';
import { useMcpStore } from '../../stores/mcp';
import { showToast } from '../../composables/useToast';
import { extractCommandErrorMessage } from '../../utils/commandError';
import type { McpPromptItemView } from '../../types/generated/contracts';

const props = defineProps<{
  modelValue: boolean;
}>();

const emit = defineEmits<{
  (e: 'update:modelValue', v: boolean): void;
  (e: 'insert', text: string): void;
}>();

const { t } = useI18n();
const mcpStore = useMcpStore();

const prompts = ref<McpPromptItemView[]>([]);
const loading = ref(false);
const selected = ref<McpPromptItemView | null>(null);
const argValues = ref<Record<string, string>>({});
const rendering = ref(false);

const loadPrompts = async () => {
  loading.value = true;
  selected.value = null;
  argValues.value = {};
  try {
    prompts.value = await mcpStore.listPrompts();
    if (prompts.value.length > 0) {
      selectPrompt(prompts.value[0]);
    }
  } catch (err) {
    showToast(extractCommandErrorMessage(err), 'error');
  } finally {
    loading.value = false;
  }
};

const selectPrompt = (p: McpPromptItemView) => {
  selected.value = p;
  const next: Record<string, string> = {};
  for (const arg of p.arguments) {
    next[arg.name] = '';
  }
  argValues.value = next;
};

watch(
  () => props.modelValue,
  (open) => {
    if (open) void loadPrompts();
  },
);

const handleInsert = async () => {
  if (!selected.value) return;
  rendering.value = true;
  try {
    const result = await mcpStore.getPrompt(
      selected.value.serverName,
      selected.value.name,
      argValues.value,
    );
    if (result.combinedText.trim()) {
      emit('insert', result.combinedText);
      emit('update:modelValue', false);
    }
  } catch (err) {
    showToast(extractCommandErrorMessage(err), 'error');
  } finally {
    rendering.value = false;
  }
};

const close = () => emit('update:modelValue', false);
</script>

<template>
  <div v-if="modelValue" class="picker-backdrop" @click.self="close">
    <div class="picker-modal">
      <div class="picker-header">
        <h3>{{ t('extensions.mcp.promptPicker.title') }}</h3>
        <button class="btn" @click="close">{{ t('skillManager.close') }}</button>
      </div>

      <div class="picker-body">
        <div v-if="loading" class="picker-state">{{ t('skillManager.loading') }}</div>
        <div v-else-if="prompts.length === 0" class="picker-state">
          {{ t('extensions.mcp.promptPicker.empty') }}
        </div>
        <div v-else class="picker-layout">
          <div class="prompt-list">
            <button
              v-for="p in prompts"
              :key="`${p.serverName}::${p.name}`"
              class="prompt-item"
              :class="{ active: selected?.name === p.name && selected?.serverName === p.serverName }"
              @click="selectPrompt(p)"
            >
              <strong>{{ p.name }}</strong>
              <span class="srv-tag">{{ p.serverName }}</span>
            </button>
          </div>
          <div v-if="selected" class="prompt-detail">
            <p v-if="selected.description" class="desc">{{ selected.description }}</p>
            <div v-if="selected.arguments.length > 0" class="args-form">
              <div class="args-title">{{ t('extensions.mcp.promptPicker.arguments') }}</div>
              <div v-for="arg in selected.arguments" :key="arg.name" class="arg-field">
                <label>
                  {{ arg.name }}
                  <span v-if="arg.required" class="req">*</span>
                </label>
                <input
                  v-model="argValues[arg.name]"
                  type="text"
                  :placeholder="arg.description || arg.name"
                />
              </div>
            </div>
          </div>
        </div>
      </div>

      <div class="picker-footer">
        <button class="btn" @click="close">{{ t('extensions.mcp.editor.cancel') }}</button>
        <button
          class="btn primary"
          :disabled="!selected || rendering"
          @click="handleInsert"
        >
          {{ rendering ? t('extensions.mcp.promptPicker.rendering') : t('extensions.mcp.promptPicker.insertToDraft') }}
        </button>
      </div>
    </div>
  </div>
</template>

<style scoped>
.picker-backdrop {
  position: fixed;
  inset: 0;
  background: rgba(0, 0, 0, 0.5);
  display: flex;
  align-items: center;
  justify-content: center;
  z-index: 1000;
}

.picker-modal {
  width: min(640px, 92vw);
  max-height: 78vh;
  display: flex;
  flex-direction: column;
  background: var(--glass-bg-heavy);
  border: 1px solid var(--glass-border);
  border-radius: var(--radius-md);
  overflow: hidden;
}

.picker-header,
.picker-footer {
  display: flex;
  justify-content: space-between;
  align-items: center;
  gap: 8px;
  padding: 12px 16px;
  border-bottom: 1px solid var(--glass-border);
}

.picker-footer {
  justify-content: flex-end;
  border-bottom: none;
  border-top: 1px solid var(--glass-border);
}

.picker-header h3 {
  margin: 0;
  font-size: 0.96rem;
  color: var(--text-main);
}

.picker-body {
  padding: 14px 16px;
  overflow-y: auto;
  flex: 1;
}

.picker-state {
  text-align: center;
  padding: 32px 16px;
  color: var(--text-muted);
  font-size: 0.84rem;
}

.picker-layout {
  display: grid;
  grid-template-columns: 200px 1fr;
  gap: 14px;
}

.prompt-list {
  display: flex;
  flex-direction: column;
  gap: 6px;
  border-right: 1px solid var(--glass-border);
  padding-right: 10px;
}

.prompt-item {
  display: flex;
  flex-direction: column;
  align-items: flex-start;
  gap: 2px;
  padding: 8px 10px;
  border: 1px solid transparent;
  border-radius: var(--radius-sm);
  background: transparent;
  color: var(--text-main);
  cursor: pointer;
  text-align: left;
}

.prompt-item.active {
  background: var(--glass-bg);
  border-color: var(--accent-blue);
}

.srv-tag {
  font-size: 0.72rem;
  color: var(--text-muted);
}

.desc {
  margin: 0 0 10px 0;
  font-size: 0.82rem;
  color: var(--text-muted);
}

.args-form {
  display: flex;
  flex-direction: column;
  gap: 8px;
}

.args-title {
  font-size: 0.78rem;
  font-weight: 600;
  color: var(--text-main);
}

.arg-field {
  display: flex;
  flex-direction: column;
  gap: 4px;
}

.arg-field label {
  font-size: 0.78rem;
  color: var(--text-muted);
}

.arg-field input {
  padding: 6px 8px;
  border: 1px solid var(--glass-border);
  border-radius: var(--radius-sm);
  background: var(--glass-bg-light);
  color: var(--text-main);
  font-size: 0.82rem;
}

.req {
  color: var(--accent-red, #ef4444);
}

.btn {
  padding: 6px 12px;
  font-size: 0.8rem;
  border-radius: var(--radius-sm);
  border: 1px solid var(--glass-border);
  background: var(--glass-bg-light);
  color: var(--text-main);
  cursor: pointer;
}

.btn.primary {
  background: var(--accent-blue);
  border-color: var(--accent-blue);
  color: #fff;
}

.btn:disabled {
  opacity: 0.5;
  cursor: not-allowed;
}
</style>
