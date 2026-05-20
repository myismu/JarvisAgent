<!--
# SkillDetailPanel.vue — Skill 详情弹窗

以弹出窗口形式展示单个 skill 的完整内容，包括名称、描述、路径和 markdown 正文。

## Key Exports
- `SkillDetailPanel`: Skill 详情弹窗组件

## Dependencies
- Internal: `../../types`
- External: `vue-i18n`, `@tauri-apps/api/core`
-->
<script setup lang="ts">
import { ref, watch } from 'vue';
import { useI18n } from 'vue-i18n';
import { invoke } from '@tauri-apps/api/core';
import type { SkillDetail } from '../../types';

const props = defineProps<{
  modelValue: boolean;
  skillName: string | null;
}>();

const emit = defineEmits<{
  (e: 'update:modelValue', value: boolean): void;
}>();

const { t } = useI18n();
const skill = ref<SkillDetail | null>(null);
const loading = ref(false);
const error = ref<string | null>(null);

const close = () => {
  emit('update:modelValue', false);
};

const loadSkillDetail = async (name: string) => {
  loading.value = true;
  error.value = null;
  skill.value = null;
  try {
    skill.value = await invoke<SkillDetail | null>('get_skill_detail', { name });
  } catch (err) {
    error.value = String(err);
    console.error('Failed to load skill detail:', err);
  } finally {
    loading.value = false;
  }
};

watch(() => props.modelValue, (newVal) => {
  if (newVal && props.skillName) {
    loadSkillDetail(props.skillName);
  }
});

const renderMarkdown = (text: string): string => {
  return text
    .replace(/&/g, '&amp;')
    .replace(/</g, '&lt;')
    .replace(/>/g, '&gt;')
    .replace(/^### (.*$)/gm, '<h3>$1</h3>')
    .replace(/^## (.*$)/gm, '<h2>$1</h2>')
    .replace(/^# (.*$)/gm, '<h1>$1</h1>')
    .replace(/\*\*(.*?)\*\*/g, '<strong>$1</strong>')
    .replace(/\*(.*?)\*/g, '<em>$1</em>')
    .replace(/`(.*?)`/g, '<code>$1</code>')
    .replace(/```([\s\S]*?)```/g, '<pre><code>$1</code></pre>')
    .replace(/^\- (.*$)/gm, '<li>$1</li>')
    .replace(/\n\n/g, '</p><p>')
    .replace(/\n/g, '<br>');
};
</script>

<template>
  <div v-if="modelValue" class="skill-detail-overlay" @click.self="close">
    <div class="skill-detail-modal">
      <div class="skill-detail-header">
        <div class="header-title">
          <svg viewBox="0 0 24 24" width="20" height="20" class="header-icon">
            <path d="M13 2L3 14h9l-1 8 10-12h-9l1-8z" fill="currentColor"></path>
          </svg>
          <h3>{{ skillName }}</h3>
        </div>
        <button class="close-btn" @click="close" :title="t('skillManager.close')">
          <svg viewBox="0 0 24 24" width="20" height="20">
            <path fill="currentColor" d="M19 6.41L17.59 5 12 10.59 6.41 5 5 6.41 10.59 12 5 17.59 6.41 19 12 13.41 17.59 19 19 17.59 13.41 12z" />
          </svg>
        </button>
      </div>

      <div class="skill-detail-body">
        <div v-if="loading" class="skill-detail-loading">
          <div class="loading-spinner"></div>
          <span>{{ t('skillManager.loading') }}</span>
        </div>

        <div v-else-if="error" class="skill-detail-error">
          <p>{{ error }}</p>
        </div>

        <div v-else-if="skill" class="skill-detail-content">
          <div class="skill-meta">
            <div class="meta-item">
              <span class="meta-label">{{ t('skillManager.description') }}:</span>
              <span class="meta-value">{{ skill.description }}</span>
            </div>
            <div class="meta-item">
              <span class="meta-label">{{ t('skillManager.path') }}:</span>
              <span class="meta-value path-value">{{ skill.path }}</span>
            </div>
          </div>

          <div class="skill-body">
            <div class="body-content" v-html="renderMarkdown(skill.body)"></div>
          </div>
        </div>

        <div v-else class="skill-detail-empty">
          <p>{{ t('skillManager.noSkills') }}</p>
        </div>
      </div>
    </div>
  </div>
</template>

<style scoped>
.skill-detail-overlay {
  position: fixed;
  top: 0;
  left: 0;
  width: 100%;
  height: 100%;
  background: rgba(0, 0, 0, 0.4);
  display: flex;
  justify-content: center;
  align-items: center;
  z-index: 1000;
  backdrop-filter: blur(12px);
  -webkit-backdrop-filter: blur(12px);
  animation: fadeIn var(--transition-fast);
}

@keyframes fadeIn {
  from { opacity: 0; }
  to { opacity: 1; }
}

.skill-detail-modal {
  width: 90%;
  max-width: 700px;
  max-height: 80vh;
  background: var(--glass-bg-heavy);
  border: 1px solid var(--glass-border);
  border-radius: var(--radius-xl);
  display: flex;
  flex-direction: column;
  overflow: hidden;
  box-shadow: 0 8px 32px rgba(0, 0, 0, 0.3);
  animation: slideUp var(--transition-fast);
}

@keyframes slideUp {
  from { transform: translateY(20px); opacity: 0; }
  to { transform: translateY(0); opacity: 1; }
}

.skill-detail-header {
  display: flex;
  align-items: center;
  justify-content: space-between;
  padding: 16px 20px;
  background: var(--glass-bg);
  border-bottom: 1px solid var(--glass-border);
  flex-shrink: 0;
}

.header-title {
  display: flex;
  align-items: center;
  gap: 10px;
}

.header-icon {
  color: var(--accent-yellow);
}

.header-title h3 {
  font-size: 16px;
  font-weight: 600;
  color: var(--text-main);
  margin: 0;
}

.close-btn {
  background: transparent;
  border: none;
  color: var(--text-muted);
  cursor: pointer;
  padding: 4px;
  border-radius: var(--radius-sm);
  transition: all var(--transition-fast);
  display: flex;
  align-items: center;
  justify-content: center;
}

.close-btn:hover {
  background: var(--glass-bg-light);
  color: var(--text-main);
}

.skill-detail-body {
  flex: 1;
  overflow-y: auto;
  padding: 20px;
}

.skill-detail-loading,
.skill-detail-error,
.skill-detail-empty {
  display: flex;
  flex-direction: column;
  align-items: center;
  justify-content: center;
  gap: 12px;
  padding: 48px;
  color: var(--text-muted);
}

.loading-spinner {
  width: 24px;
  height: 24px;
  border: 2px solid var(--glass-border);
  border-top-color: var(--accent-blue);
  border-radius: 50%;
  animation: spin 1s linear infinite;
}

@keyframes spin {
  to { transform: rotate(360deg); }
}

.skill-detail-content {
  display: flex;
  flex-direction: column;
  gap: 20px;
}

.skill-meta {
  display: flex;
  flex-direction: column;
  gap: 8px;
  padding: 12px;
  background: var(--glass-bg);
  border: 1px solid var(--glass-border);
  border-radius: var(--radius-md);
}

.meta-item {
  display: flex;
  gap: 8px;
  font-size: 13px;
}

.meta-label {
  color: var(--text-muted);
  font-weight: 500;
  min-width: 80px;
}

.meta-value {
  color: var(--text-main);
}

.path-value {
  font-family: var(--font-mono);
  font-size: 12px;
  word-break: break-all;
}

.skill-body {
  background: var(--glass-bg);
  border: 1px solid var(--glass-border);
  border-radius: var(--radius-md);
  padding: 16px;
}

.body-content {
  font-size: 14px;
  line-height: 1.6;
  color: var(--text-main);
}

.body-content :deep(h1),
.body-content :deep(h2),
.body-content :deep(h3) {
  margin: 16px 0 8px 0;
  font-weight: 600;
}

.body-content :deep(h1) { font-size: 20px; }
.body-content :deep(h2) { font-size: 18px; }
.body-content :deep(h3) { font-size: 16px; }

.body-content :deep(code) {
  background: var(--glass-bg-light);
  padding: 2px 6px;
  border-radius: var(--radius-sm);
  font-family: var(--font-mono);
  font-size: 13px;
}

.body-content :deep(pre) {
  background: var(--glass-bg-light);
  padding: 12px;
  border-radius: var(--radius-md);
  overflow-x: auto;
}

.body-content :deep(pre code) {
  background: transparent;
  padding: 0;
}

.body-content :deep(li) {
  margin-left: 20px;
  margin-bottom: 4px;
}
</style>
