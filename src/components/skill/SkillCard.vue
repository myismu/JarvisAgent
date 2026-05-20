<!--
# SkillCard.vue — Skill 卡片组件

展示单个 skill 的摘要信息，包括名称、描述和内容长度。
支持激活开关和查看详情。

## Key Exports
- `SkillCard`: Skill 列表项卡片组件

## Dependencies
- Internal: `../../types`
- External: `vue-i18n`, `@tauri-apps/api/core`
-->
<script setup lang="ts">
import { useI18n } from 'vue-i18n';
import { invoke } from '@tauri-apps/api/core';
import type { SkillMeta } from '../../types';

const props = defineProps<{
  skill: SkillMeta;
}>();

const emit = defineEmits<{
  (e: 'view-detail', name: string): void;
  (e: 'toggle-active', name: string, active: boolean): void;
}>();

const { t } = useI18n();

const handleClick = () => {
  emit('view-detail', props.skill.name);
};

const handleToggle = async (event: Event) => {
  event.stopPropagation();
  const newActive = !props.skill.active;
  try {
    await invoke('set_skill_active', { skillName: props.skill.name, active: newActive });
    emit('toggle-active', props.skill.name, newActive);
  } catch (err) {
    console.error('Failed to toggle skill:', err);
  }
};
</script>

<template>
  <div class="skill-card" :class="{ inactive: !skill.active }" @click="handleClick">
    <div class="skill-card-header">
      <div class="skill-icon">
        <svg viewBox="0 0 24 24" width="20" height="20" stroke="currentColor" stroke-width="2" fill="none">
          <path d="M13 2L3 14h9l-1 8 10-12h-9l1-8z"></path>
        </svg>
      </div>
      <div class="skill-info">
        <h3 class="skill-name">{{ skill.name }}</h3>
        <p class="skill-description">{{ skill.description }}</p>
      </div>
      <button class="skill-toggle" :class="{ active: skill.active }" @click="handleToggle" :title="skill.active ? t('skillManager.deactivate') : t('skillManager.activate')">
        <span class="toggle-track">
          <span class="toggle-thumb"></span>
        </span>
      </button>
    </div>
    <div class="skill-card-footer">
      <span class="skill-size">{{ skill.bodyTokens }} {{ t('skillManager.bodyTokens') }}</span>
      <button class="skill-view-btn" @click.stop="handleClick">
        {{ t('skillManager.viewDetail') }}
      </button>
    </div>
  </div>
</template>

<style scoped>
.skill-card {
  background: var(--glass-bg);
  border: 1px solid var(--glass-border);
  border-radius: var(--radius-lg);
  padding: 16px;
  cursor: pointer;
  transition: all var(--transition-fast);
  display: flex;
  flex-direction: column;
  gap: 12px;
}

.skill-card.inactive {
  opacity: 0.5;
}

.skill-card:hover {
  background: var(--glass-bg-light);
  border-color: var(--accent-blue);
  transform: translateY(-2px);
  box-shadow: 0 4px 12px rgba(0, 0, 0, 0.2);
}

.skill-card-header {
  display: flex;
  gap: 12px;
  align-items: flex-start;
}

.skill-icon {
  flex-shrink: 0;
  width: 36px;
  height: 36px;
  display: flex;
  align-items: center;
  justify-content: center;
  background: var(--glass-bg-light);
  border-radius: var(--radius-md);
  color: var(--accent-yellow);
}

.skill-info {
  flex: 1;
  min-width: 0;
}

.skill-name {
  font-size: 14px;
  font-weight: 600;
  color: var(--text-main);
  margin: 0 0 4px 0;
  white-space: nowrap;
  overflow: hidden;
  text-overflow: ellipsis;
}

.skill-description {
  font-size: 12px;
  color: var(--text-muted);
  margin: 0;
  display: -webkit-box;
  -webkit-line-clamp: 2;
  -webkit-box-orient: vertical;
  overflow: hidden;
  line-height: 1.5;
}

.skill-toggle {
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
  background: var(--glass-border);
  position: relative;
  transition: background var(--transition-fast);
}

.skill-toggle.active .toggle-track {
  background: var(--accent-green, #22c55e);
}

.toggle-thumb {
  display: block;
  width: 16px;
  height: 16px;
  border-radius: 50%;
  background: white;
  position: absolute;
  top: 2px;
  left: 2px;
  transition: transform var(--transition-fast);
}

.skill-toggle.active .toggle-thumb {
  transform: translateX(16px);
}

.skill-card-footer {
  display: flex;
  align-items: center;
  justify-content: space-between;
}

.skill-size {
  font-size: 11px;
  color: var(--text-muted);
}

.skill-view-btn {
  background: transparent;
  border: 1px solid var(--glass-border);
  color: var(--text-muted);
  font-size: 11px;
  padding: 4px 10px;
  border-radius: var(--radius-sm);
  cursor: pointer;
  transition: all var(--transition-fast);
}

.skill-view-btn:hover {
  background: var(--accent-blue);
  border-color: var(--accent-blue);
  color: white;
}
</style>
