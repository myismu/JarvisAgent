<!--
# PromptsTab.vue — 设置页「提示词」tab

可视化查看 + 编辑 13 个提示词文件。
列表以注册表为准；编辑保存写 data/prompts/（懒落盘）；
恢复默认 = 删磁盘文件回落内置；预览展示拼装后的 system 全文。

## Key Exports
- `PromptsTab`: 提示词管理 tab 主组件

## Dependencies
- Internal: `../../types`, `../../composables/usePreferences`
- External: `vue-i18n`, `@tauri-apps/api/core`
-->
<script setup lang="ts">
import { ref, computed, onMounted } from 'vue';
import { useI18n } from 'vue-i18n';
import { invoke } from '@tauri-apps/api/core';
import type { PromptMeta, PromptDetail } from '../../types';
import { usePreferences } from '../../composables/usePreferences';

const { t } = useI18n();
const uiPrefs = usePreferences();

const prompts = ref<PromptMeta[]>([]);
const loading = ref(true);
const error = ref<string | null>(null);
const selectedPath = ref<string | null>(null);
const detail = ref<PromptDetail | null>(null);
const detailLoading = ref(false);
const editText = ref('');
const dirty = ref(false);
const saving = ref(false);

// 预览弹层：target 决定看哪条拼装链路（主 Agent system / 子代理 system / 动态上下文），三份互相独立
const showPreview = ref(false);
const previewContent = ref('');
const previewTarget = ref<'main' | 'subagent' | 'dynamic'>('main');
const previewMode = ref<'edit' | 'plan'>('edit');
const previewLoading = ref(false);

// 分组顺序与标题 i18n key（与后端 PromptCategory as_str 对齐）
const CATEGORY_ORDER = ['base', 'audience', 'mode', 'os', 'subagent'] as const;
const CATEGORY_I18N: Record<string, string> = {
  base: 'settings.prompts.catBase',
  audience: 'settings.prompts.catAudience',
  mode: 'settings.prompts.catMode',
  os: 'settings.prompts.catOs',
  subagent: 'settings.prompts.catSubagent',
};

const grouped = computed(() => {
  const map = new Map<string, PromptMeta[]>();
  for (const p of prompts.value) {
    if (!map.has(p.category)) map.set(p.category, []);
    map.get(p.category)!.push(p);
  }
  return CATEGORY_ORDER.filter(c => map.has(c)).map(c => ({
    key: c,
    titleKey: CATEGORY_I18N[c],
    items: map.get(c)!,
  }));
});

onMounted(async () => {
  try {
    prompts.value = await invoke<PromptMeta[]>('list_prompts');
    // 默认选中第一项
    if (prompts.value.length > 0) {
      await selectPrompt(prompts.value[0].path);
    }
  } catch (err) {
    error.value = String(err);
    console.error('Failed to load prompts:', err);
  } finally {
    loading.value = false;
  }
});

const selectPrompt = async (path: string) => {
  if (dirty.value && !confirm(t('settings.prompts.unsavedConfirm'))) return;
  selectedPath.value = path;
  detailLoading.value = true;
  detail.value = null;
  try {
    detail.value = await invoke<PromptDetail>('get_prompt_detail', { path });
    editText.value = detail.value.currentContent;
    dirty.value = false;
  } catch (err) {
    console.error('Failed to load prompt detail:', err);
    error.value = String(err);
  } finally {
    detailLoading.value = false;
  }
};

const onEdit = () => {
  dirty.value = detail.value ? editText.value !== detail.value.currentContent : false;
};

const save = async () => {
  if (!selectedPath.value) return;
  saving.value = true;
  try {
    await invoke('save_prompt', { path: selectedPath.value, content: editText.value });
    // 刷新列表状态（customized 徽标、体积）
    prompts.value = await invoke<PromptMeta[]>('list_prompts');
    detail.value = await invoke<PromptDetail>('get_prompt_detail', { path: selectedPath.value });
    dirty.value = false;
  } catch (err) {
    console.error('Failed to save prompt:', err);
    error.value = String(err);
  } finally {
    saving.value = false;
  }
};

const reset = async () => {
  if (!selectedPath.value) return;
  if (!confirm(t('settings.prompts.resetConfirm'))) return;
  try {
    await invoke('reset_prompt', { path: selectedPath.value });
    prompts.value = await invoke<PromptMeta[]>('list_prompts');
    detail.value = await invoke<PromptDetail>('get_prompt_detail', { path: selectedPath.value });
    editText.value = detail.value.currentContent;
    dirty.value = false;
  } catch (err) {
    console.error('Failed to reset prompt:', err);
    error.value = String(err);
  }
};

const openPreview = async () => {
  showPreview.value = true;
  await rePreview();
};

const rePreview = async () => {
  previewLoading.value = true;
  try {
    previewContent.value = await invoke<string>('get_assembled_system_prompt', {
      target: previewTarget.value,
      audience: uiPrefs.agentAudience.value,
      // 模式只对动态上下文有意义（mode_rules 是它的一部分）；system 两份不受模式影响
      workMode: previewTarget.value === 'dynamic' ? previewMode.value : null,
      workspace: '',
    });
  } catch (err) {
    previewContent.value = String(err);
  } finally {
    previewLoading.value = false;
  }
};
</script>

<template>
  <div class="prompts-tab">
    <!-- 左：分组列表（顶部是全局拼装预览入口——它与选中的提示词无关，故不放进编辑器） -->
    <div class="prompts-sidebar">
      <button class="preview-entry-btn" @click="openPreview">
        <svg viewBox="0 0 24 24" width="14" height="14" stroke="currentColor" stroke-width="2" fill="none" stroke-linecap="round" stroke-linejoin="round">
          <path d="M1 12s4-8 11-8 11 8 11 8-4 8-11 8-11-8-11-8z"></path>
          <circle cx="12" cy="12" r="3"></circle>
        </svg>
        {{ t('settings.prompts.previewAssembled') }}
      </button>
      <div v-if="loading" class="prompts-loading">{{ t('settings.prompts.loading') }}</div>
      <div v-else-if="error && prompts.length === 0" class="prompts-error">{{ error }}</div>
      <template v-else>
        <div v-for="group in grouped" :key="group.key" class="prompts-group">
          <div class="group-title">{{ t(group.titleKey) }}</div>
          <div
            v-for="p in group.items"
            :key="p.path"
            class="prompt-item"
            :class="{ active: selectedPath === p.path }"
            @click="selectPrompt(p.path)"
          >
            <div class="prompt-item-main">
              <span class="prompt-name">{{ p.displayName }}</span>
              <span v-if="p.customized" class="badge customized">{{ t('settings.prompts.customized') }}</span>
              <span v-if="p.isPlatformActive" class="badge platform">{{ t('settings.prompts.thisMachine') }}</span>
            </div>
          </div>
        </div>
      </template>
    </div>

    <!-- 右：编辑器 -->
    <div class="prompts-editor">
      <template v-if="detail">
        <div class="editor-header">
          <div class="editor-title">
            <h4>{{ detail.displayName }}</h4>
            <div class="editor-sub">
              <span class="mono">{{ detail.path }}</span>
              <span class="sep">·</span>
              <span>{{ detail.goesIntoSystem ? t('settings.prompts.effNewSession') : t('settings.prompts.effNextTurn') }}</span>
              <span class="sep">·</span>
              <span>{{ detail.customized ? t('settings.prompts.customVersion') : t('settings.prompts.embeddedVersion') }}</span>
            </div>
          </div>
          <div class="editor-actions">
            <button
              v-if="detail.customized"
              class="btn ghost danger"
              @click="reset"
            >{{ t('settings.prompts.resetDefault') }}</button>
            <button class="btn primary" :disabled="!dirty || saving" @click="save">
              {{ saving ? t('settings.prompts.saving') : t('settings.prompts.save') }}
            </button>
          </div>
        </div>

        <textarea
          v-model="editText"
          class="editor-textarea"
          spellcheck="false"
          @input="onEdit"
        ></textarea>

        <div class="editor-footer">
          <span v-if="dirty">{{ t('settings.prompts.dirtyHint') }}</span>
          <span v-else>{{ detail.customized ? t('settings.prompts.customizedHint') : t('settings.prompts.embeddedHint') }}</span>
          <span class="eff-hint">{{ detail.goesIntoSystem ? t('settings.prompts.newSessionHint') : t('settings.prompts.nextTurnHint') }}</span>
        </div>
      </template>
      <div v-else-if="detailLoading" class="prompts-loading">{{ t('settings.prompts.loading') }}</div>
    </div>

    <!-- 预览弹层 -->
    <div v-if="showPreview" class="preview-overlay" @click.self="showPreview = false">
      <div class="preview-modal">
        <div class="preview-header">
          <h4>{{ t('settings.prompts.previewTitle') }}</h4>
          <div class="preview-controls">
            <button
              class="display-btn"
              :class="{ active: previewTarget === 'main' }"
              @click="previewTarget = 'main'; rePreview()"
            >{{ t('settings.prompts.targetMain') }}</button>
            <button
              class="display-btn"
              :class="{ active: previewTarget === 'subagent' }"
              @click="previewTarget = 'subagent'; rePreview()"
            >{{ t('settings.prompts.targetSubagent') }}</button>
            <button
              class="display-btn"
              :class="{ active: previewTarget === 'dynamic' }"
              @click="previewTarget = 'dynamic'; rePreview()"
            >{{ t('settings.prompts.targetDynamic') }}</button>
            <template v-if="previewTarget === 'dynamic'">
              <span class="mode-divider"></span>
              <button
                class="display-btn"
                :class="{ active: previewMode === 'edit' }"
                @click="previewMode = 'edit'; rePreview()"
              >{{ t('settings.prompts.modeEdit') }}</button>
              <button
                class="display-btn"
                :class="{ active: previewMode === 'plan' }"
                @click="previewMode = 'plan'; rePreview()"
              >{{ t('settings.prompts.modePlan') }}</button>
            </template>
            <button class="icon-btn" @click="showPreview = false">✕</button>
          </div>
        </div>
        <pre class="preview-body">{{ previewLoading ? t('settings.prompts.loading') : previewContent }}</pre>
        <div class="preview-footer">{{ t('settings.prompts.previewFooter') }}</div>
      </div>
    </div>
  </div>
</template>

<style scoped>
.prompts-tab {
  display: grid;
  grid-template-columns: 250px 1fr;
  gap: 12px;
  height: 100%;
  min-height: 0;
}

.prompts-sidebar {
  border-right: 1px solid var(--glass-border);
  padding-right: 8px;
  overflow-y: auto;
}

/* 全局拼装预览入口：与选中项无关，独立放列表顶部 */
.preview-entry-btn {
  display: flex;
  align-items: center;
  justify-content: center;
  gap: 6px;
  width: 100%;
  margin-bottom: 8px;
  padding: 8px 12px;
  font-size: 0.8333rem;
  border: 1px solid var(--glass-border);
  border-radius: var(--radius-md);
  background: transparent;
  color: var(--text-main);
  cursor: pointer;
  transition: all var(--transition-fast);
}

.preview-entry-btn:hover {
  border-color: var(--accent-blue);
  color: var(--accent-blue);
}

.mode-divider {
  width: 1px;
  height: 16px;
  background: var(--glass-border);
  margin: 0 2px;
}

.group-title {
  font-size: 0.7333rem;
  color: var(--text-muted);
  padding: 10px 10px 4px;
}

.prompt-item {
  display: flex;
  justify-content: space-between;
  align-items: center;
  gap: 8px;
  padding: 7px 10px;
  border-radius: var(--radius-md);
  cursor: pointer;
  transition: all var(--transition-fast);
  font-size: 0.8667rem;
  color: var(--text-main);
}

.prompt-item:hover {
  background: var(--glass-bg-light);
}

.prompt-item.active {
  background: var(--glass-bg-light);
  font-weight: 500;
}

.prompt-item-main {
  display: flex;
  align-items: center;
  gap: 6px;
  min-width: 0;
  flex-wrap: wrap;
}

.prompt-name {
  white-space: nowrap;
}

.badge {
  font-size: 0.6667rem;
  padding: 1px 6px;
  border-radius: 8px;
  white-space: nowrap;
}

.badge.customized {
  background: rgba(250, 179, 21, 0.18);
  color: var(--accent-amber, #e8a33d);
}

.badge.platform {
  background: rgba(59, 130, 246, 0.15);
  color: var(--accent-blue);
}

.prompts-editor {
  display: flex;
  flex-direction: column;
  gap: 10px;
  min-height: 0;
}

.editor-header {
  display: flex;
  justify-content: space-between;
  align-items: flex-start;
  gap: 12px;
}

.editor-title h4 {
  margin: 0;
  font-size: 0.9333rem;
  font-weight: 500;
  color: var(--text-main);
}

.editor-sub {
  display: flex;
  gap: 6px;
  align-items: center;
  font-size: 0.8rem;
  color: var(--text-muted);
  margin-top: 3px;
}

.mono {
  font-family: var(--font-mono);
}

.sep {
  opacity: 0.5;
}

.editor-actions {
  display: flex;
  gap: 8px;
  flex-shrink: 0;
}

.btn {
  font-size: 0.8rem;
  padding: 6px 14px;
  border-radius: var(--radius-md);
  border: 1px solid var(--glass-border);
  background: transparent;
  color: var(--text-main);
  cursor: pointer;
  transition: all var(--transition-fast);
}

.btn:hover:not(:disabled) {
  border-color: var(--accent-blue);
}

.btn:disabled {
  opacity: 0.45;
  cursor: not-allowed;
}

.btn.primary {
  background: var(--accent-blue);
  border-color: var(--accent-blue);
  color: #fff;
}

.btn.ghost.danger:hover {
  border-color: #e5484d;
  color: #e5484d;
}

.editor-textarea {
  flex: 1;
  min-height: 0;
  resize: none;
  border: 1px solid var(--glass-border);
  border-radius: var(--radius-md);
  background: var(--glass-bg-light);
  color: var(--text-main);
  font-family: var(--font-mono);
  font-size: 0.8333rem;
  line-height: 1.7;
  padding: 12px;
  outline: none;
}

.editor-textarea:focus {
  border-color: var(--accent-blue);
}

.editor-footer {
  display: flex;
  justify-content: space-between;
  font-size: 0.8rem;
  color: var(--text-muted);
}

.eff-hint {
  color: var(--accent-blue);
}

.prompts-loading {
  display: flex;
  align-items: center;
  justify-content: center;
  height: 200px;
  color: var(--text-muted);
  font-size: 0.8667rem;
}

.prompts-error {
  padding: 20px;
  color: #e5484d;
  font-size: 0.8rem;
  word-break: break-all;
}

/* 预览弹层 */
.preview-overlay {
  position: absolute;
  inset: 0;
  background: rgba(0, 0, 0, 0.5);
  display: flex;
  align-items: center;
  justify-content: center;
  z-index: 50;
  border-radius: inherit;
}

.preview-modal {
  width: min(860px, 92%);
  height: 86%;
  background: var(--glass-bg-heavy);
  border: 1px solid var(--glass-border);
  border-radius: var(--radius-lg);
  display: flex;
  flex-direction: column;
  padding: 16px;
  gap: 10px;
}

.preview-header {
  display: flex;
  justify-content: space-between;
  align-items: center;
}

.preview-header h4 {
  margin: 0;
  font-size: 0.9333rem;
  font-weight: 500;
  color: var(--text-main);
}

.preview-controls {
  display: flex;
  gap: 6px;
  align-items: center;
}

.display-btn {
  font-size: 0.8rem;
  padding: 5px 12px;
  border-radius: var(--radius-md);
  border: 1px solid var(--glass-border);
  background: transparent;
  color: var(--text-muted);
  cursor: pointer;
}

.display-btn.active {
  color: var(--text-main);
  background: var(--glass-bg-light);
}

.icon-btn {
  border: none;
  background: transparent;
  color: var(--text-muted);
  cursor: pointer;
  font-size: 0.9333rem;
  padding: 4px 8px;
}

.preview-body {
  flex: 1;
  overflow: auto;
  margin: 0;
  padding: 14px;
  background: var(--glass-bg-light);
  border: 1px solid var(--glass-border);
  border-radius: var(--radius-md);
  font-family: var(--font-mono);
  font-size: 0.8rem;
  line-height: 1.7;
  white-space: pre-wrap;
  word-break: break-word;
  color: var(--text-main);
}

.preview-footer {
  font-size: 0.8rem;
  color: var(--text-muted);
}
</style>
