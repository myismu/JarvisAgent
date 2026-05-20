<!--
# SkillManager.vue — Skill 管理主页面

展示本地 skill 列表和 skill 市场（预留），支持查看详情和返回聊天。

## Key Exports
- `SkillManager`: Skill 管理页面主组件

## Dependencies
- Internal: `../../types`, `../../stores/appView`, `./SkillCard`, `./SkillDetailPanel`, `./SkillMarket`
- External: `vue-i18n`, `@tauri-apps/api/core`
-->
<script setup lang="ts">
import { ref, onMounted } from 'vue';
import { useI18n } from 'vue-i18n';
import { invoke } from '@tauri-apps/api/core';
import type { SkillMeta } from '../../types';
import { useAppViewStore } from '../../stores/appView';
import SkillCard from './SkillCard.vue';
import SkillDetailPanel from './SkillDetailPanel.vue';
import SkillMarket from './SkillMarket.vue';

const { t } = useI18n();
const appView = useAppViewStore();

type TabType = 'local' | 'market';
const activeTab = ref<TabType>('local');

const skills = ref<SkillMeta[]>([]);
const loading = ref(true);
const error = ref<string | null>(null);
const selectedSkill = ref<string | null>(null);
const showDetail = ref(false);

onMounted(async () => {
  try {
    skills.value = await invoke<SkillMeta[]>('list_skills');
  } catch (err) {
    error.value = String(err);
    console.error('Failed to load skills:', err);
  } finally {
    loading.value = false;
  }
});

const handleViewDetail = (name: string) => {
  selectedSkill.value = name;
  showDetail.value = true;
};

const handleBackToChat = () => {
  appView.showChat();
};

const handleToggleActive = (name: string, active: boolean) => {
  const skill = skills.value.find(s => s.name === name);
  if (skill) {
    skill.active = active;
  }
};

const switchTab = (tab: TabType) => {
  activeTab.value = tab;
};
</script>

<template>
  <div class="skill-manager">
    <div class="skill-manager-header">
      <div class="header-left">
        <button class="back-btn" @click="handleBackToChat">
          <svg viewBox="0 0 24 24" width="16" height="16" stroke="currentColor" stroke-width="2" fill="none">
            <polyline points="15 18 9 12 15 6"></polyline>
          </svg>
          {{ t('skillManager.backToChat') }}
        </button>
      </div>
      <div class="header-center">
        <div class="tab-switcher">
          <button
            class="tab-btn"
            :class="{ active: activeTab === 'local' }"
            @click="switchTab('local')"
          >
            <svg viewBox="0 0 24 24" width="14" height="14" stroke="currentColor" stroke-width="2" fill="none">
              <path d="M3 9l9-7 9 7v11a2 2 0 0 1-2 2H5a2 2 0 0 1-2-2z"></path>
              <polyline points="9 22 9 12 15 12 15 22"></polyline>
            </svg>
            {{ t('skillManager.localSkills') }}
          </button>
          <button
            class="tab-btn"
            :class="{ active: activeTab === 'market' }"
            @click="switchTab('market')"
          >
            <svg viewBox="0 0 24 24" width="14" height="14" stroke="currentColor" stroke-width="2" fill="none">
              <circle cx="9" cy="21" r="1"></circle>
              <circle cx="20" cy="21" r="1"></circle>
              <path d="M1 1h4l2.68 13.39a2 2 0 0 0 2 1.61h9.72a2 2 0 0 0 2-1.61L23 6H6"></path>
            </svg>
            {{ t('skillManager.skillMarket') }}
          </button>
        </div>
      </div>
      <div class="header-right">
        <span v-if="activeTab === 'local' && !loading && !error" class="skill-count">
          {{ t('skillManager.skillCount', { count: skills.length }) }}
        </span>
      </div>
    </div>

    <div class="skill-manager-content">
      <template v-if="activeTab === 'local'">
        <div v-if="loading" class="skill-manager-loading">
          <div class="loading-spinner"></div>
          <span>{{ t('skillManager.loading') }}</span>
        </div>

        <div v-else-if="error" class="skill-manager-error">
          <p>{{ error }}</p>
        </div>

        <div v-else-if="skills.length === 0" class="skill-manager-empty">
          <svg viewBox="0 0 24 24" width="48" height="48" stroke="currentColor" stroke-width="1.5" fill="none" opacity="0.3">
            <path d="M13 2L3 14h9l-1 8 10-12h-9l1-8z"></path>
          </svg>
          <p>{{ t('skillManager.noSkills') }}</p>
        </div>

        <div v-else class="skill-grid">
          <SkillCard
            v-for="skill in skills"
            :key="skill.name"
            :skill="skill"
            @view-detail="handleViewDetail"
            @toggle-active="handleToggleActive"
          />
        </div>
      </template>

      <template v-else-if="activeTab === 'market'">
        <SkillMarket />
      </template>
    </div>

    <SkillDetailPanel v-model="showDetail" :skill-name="selectedSkill" />
  </div>
</template>

<style scoped>
.skill-manager {
  display: flex;
  flex-direction: column;
  height: 100%;
  background: var(--glass-bg-heavy);
  overflow: hidden;
}

.skill-manager-header {
  display: flex;
  align-items: center;
  justify-content: space-between;
  padding: 16px 20px;
  background: var(--glass-bg);
  border-bottom: 1px solid var(--glass-border);
  flex-shrink: 0;
}

.header-left,
.header-right {
  min-width: 120px;
}

.header-center {
  text-align: center;
}

.tab-switcher {
  display: flex;
  gap: 4px;
  background: var(--glass-bg-light);
  border-radius: var(--radius-md);
  padding: 3px;
}

.tab-btn {
  display: inline-flex;
  align-items: center;
  gap: 6px;
  background: transparent;
  border: none;
  color: var(--text-muted);
  font-size: 13px;
  padding: 6px 14px;
  border-radius: var(--radius-sm);
  cursor: pointer;
  transition: all var(--transition-fast);
  white-space: nowrap;
}

.tab-btn:hover {
  color: var(--text-main);
  background: var(--glass-bg);
}

.tab-btn.active {
  color: var(--text-main);
  background: var(--glass-bg);
  box-shadow: 0 1px 3px rgba(0, 0, 0, 0.2);
}

.back-btn {
  display: inline-flex;
  align-items: center;
  gap: 6px;
  background: transparent;
  border: 1px solid var(--glass-border);
  color: var(--text-muted);
  font-size: 12px;
  padding: 6px 12px;
  border-radius: var(--radius-md);
  cursor: pointer;
  transition: all var(--transition-fast);
}

.back-btn:hover {
  background: var(--glass-bg-light);
  color: var(--text-main);
  border-color: var(--accent-blue);
}

.skill-manager-title {
  font-size: 18px;
  font-weight: 600;
  color: var(--text-main);
  margin: 0 0 4px 0;
}

.skill-manager-subtitle {
  font-size: 12px;
  color: var(--text-muted);
  margin: 0;
}

.skill-count {
  font-size: 12px;
  color: var(--text-muted);
  background: var(--glass-bg-light);
  padding: 4px 10px;
  border-radius: var(--radius-sm);
}

.skill-manager-content {
  flex: 1;
  overflow-y: auto;
  padding: 20px;
}

.skill-manager-loading,
.skill-manager-error,
.skill-manager-empty {
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

.skill-grid {
  display: grid;
  grid-template-columns: repeat(auto-fill, minmax(280px, 1fr));
  gap: 16px;
}

@media (max-width: 768px) {
  .skill-grid {
    grid-template-columns: 1fr;
  }
}
</style>
