<!--
# ExtensionHub.vue — 扩展中心主页面

统一承载「技能 / MCP 服务 / 市场」三个扩展标签，替代原 SkillManager 顶层视图容器。
技能子组件（SkillCard / SkillDetailPanel）和市场占位组件（SkillMarket）继续复用。

## Key Exports
- `ExtensionHub`: 扩展中心主组件
-->
<script setup lang="ts">
import { ref, onMounted, onUnmounted } from 'vue';
import { useI18n } from 'vue-i18n';
import { invoke } from '@tauri-apps/api/core';
import type { SkillMeta } from '../../types';
import { useAppViewStore } from '../../stores/appView';
import { useMcpStore } from '../../stores/mcp';
import SkillCard from '../skill/SkillCard.vue';
import SkillDetailPanel from '../skill/SkillDetailPanel.vue';
import SkillMarket from '../skill/SkillMarket.vue';
import McpPanel from './McpPanel.vue';

const { t } = useI18n();
const appView = useAppViewStore();
const mcpStore = useMcpStore();

type TabType = 'skills' | 'mcp' | 'market';
const activeTab = ref<TabType>('skills');

const skills = ref<SkillMeta[]>([]);
const skillsLoading = ref(true);
const skillsError = ref<string | null>(null);
const selectedSkill = ref<string | null>(null);
const showSkillDetail = ref(false);

onMounted(async () => {
  void mcpStore.initListener();
  void mcpStore.loadServers();

  try {
    skills.value = await invoke<SkillMeta[]>('list_skills');
  } catch (err) {
    skillsError.value = String(err);
    console.error('Failed to load skills:', err);
  } finally {
    skillsLoading.value = false;
  }
});

onUnmounted(() => {
  mcpStore.disposeListener();
});

const handleViewSkillDetail = (name: string) => {
  selectedSkill.value = name;
  showSkillDetail.value = true;
};

const handleBackToChat = () => {
  appView.showChat();
};

const handleToggleSkillActive = (name: string, active: boolean) => {
  const skill = skills.value.find((s) => s.name === name);
  if (skill) {
    skill.active = active;
  }
};

const switchTab = (tab: TabType) => {
  activeTab.value = tab;
};
</script>

<template>
  <div class="extension-hub">
    <div class="extension-hub-header">
      <div class="header-left">
        <button class="back-btn" @click="handleBackToChat">
          <svg viewBox="0 0 24 24" width="16" height="16" stroke="currentColor" stroke-width="2" fill="none">
            <polyline points="15 18 9 12 15 6"></polyline>
          </svg>
          {{ t('skillManager.backToChat') }}
        </button>
      </div>
      <div class="header-center">
        <div class="tab-switcher" role="tablist">
          <button
            class="tab-btn"
            :class="{ active: activeTab === 'skills' }"
            role="tab"
            :aria-selected="activeTab === 'skills'"
            @click="switchTab('skills')"
          >
            <svg viewBox="0 0 24 24" width="14" height="14" stroke="currentColor" stroke-width="2" fill="none">
              <path d="M13 2L3 14h9l-1 8 10-12h-9l1-8z"></path>
            </svg>
            {{ t('extensions.tabs.skills') }}
          </button>
          <button
            class="tab-btn"
            :class="{ active: activeTab === 'mcp' }"
            role="tab"
            :aria-selected="activeTab === 'mcp'"
            @click="switchTab('mcp')"
          >
            <svg viewBox="0 0 24 24" width="14" height="14" stroke="currentColor" stroke-width="2" fill="none">
              <rect x="2" y="3" width="20" height="7" rx="2"></rect>
              <rect x="2" y="14" width="20" height="7" rx="2"></rect>
              <line x1="6" y1="6.5" x2="6.01" y2="6.5"></line>
              <line x1="6" y1="17.5" x2="6.01" y2="17.5"></line>
            </svg>
            {{ t('extensions.tabs.mcp') }}
          </button>
          <button
            class="tab-btn"
            :class="{ active: activeTab === 'market' }"
            role="tab"
            :aria-selected="activeTab === 'market'"
            @click="switchTab('market')"
          >
            <svg viewBox="0 0 24 24" width="14" height="14" stroke="currentColor" stroke-width="2" fill="none">
              <circle cx="9" cy="21" r="1"></circle>
              <circle cx="20" cy="21" r="1"></circle>
              <path d="M1 1h4l2.68 13.39a2 2 0 0 0 2 1.61h9.72a2 2 0 0 0 2-1.61L23 6H6"></path>
            </svg>
            {{ t('extensions.tabs.market') }}
          </button>
        </div>
      </div>
      <div class="header-right">
        <span v-if="activeTab === 'skills' && !skillsLoading && !skillsError" class="count-badge">
          {{ t('skillManager.skillCount', { count: skills.length }) }}
        </span>
        <span v-else-if="activeTab === 'mcp' && !mcpStore.loading" class="count-badge">
          {{ t('extensions.mcp.serverCount', { count: mcpStore.servers.length }) }}
        </span>
      </div>
    </div>

    <div class="extension-hub-content">
      <template v-if="activeTab === 'skills'">
        <div v-if="skillsLoading" class="hub-state">
          <div class="loading-spinner"></div>
          <span>{{ t('skillManager.loading') }}</span>
        </div>

        <div v-else-if="skillsError" class="hub-state">
          <p>{{ skillsError }}</p>
        </div>

        <div v-else-if="skills.length === 0" class="hub-state">
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
            @view-detail="handleViewSkillDetail"
            @toggle-active="handleToggleSkillActive"
          />
        </div>
      </template>

      <template v-else-if="activeTab === 'mcp'">
        <McpPanel />
      </template>

      <template v-else-if="activeTab === 'market'">
        <SkillMarket />
      </template>
    </div>

    <SkillDetailPanel v-model="showSkillDetail" :skill-name="selectedSkill" />
  </div>
</template>

<style scoped>
.extension-hub {
  display: flex;
  flex-direction: column;
  height: 100%;
  background: var(--glass-bg-heavy);
  overflow: hidden;
}

.extension-hub-header {
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

.header-right {
  display: flex;
  justify-content: flex-end;
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
  font-size: 0.8667rem;
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
  font-size: 0.8rem;
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

.count-badge {
  font-size: 0.8rem;
  color: var(--text-muted);
  background: var(--glass-bg-light);
  padding: 4px 10px;
  border-radius: var(--radius-sm);
}

.extension-hub-content {
  flex: 1;
  overflow-y: auto;
  padding: 20px;
}

.hub-state {
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
  to {
    transform: rotate(360deg);
  }
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
