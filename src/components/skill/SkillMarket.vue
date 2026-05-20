<!--
# SkillMarket.vue — Skill 市场页面（占位）

预留的 Skill 市场页面，后续接入主流 Skill 市场提供下载功能。

## Key Exports
- `SkillMarket`: Skill 市场占位组件

## Dependencies
- Internal: 无
- External: `vue-i18n`
-->
<script setup lang="ts">
import { useI18n } from 'vue-i18n';

const { t } = useI18n();
</script>

<template>
  <div class="skill-market">
    <div class="market-placeholder">
      <div class="market-icon">
        <svg viewBox="0 0 24 24" width="64" height="64" stroke="currentColor" stroke-width="1.5" fill="none">
          <circle cx="9" cy="21" r="1"></circle>
          <circle cx="20" cy="21" r="1"></circle>
          <path d="M1 1h4l2.68 13.39a2 2 0 0 0 2 1.61h9.72a2 2 0 0 0 2-1.61L23 6H6"></path>
        </svg>
      </div>
      <h2 class="market-title">{{ t('skillManager.marketComingSoon') }}</h2>
      <p class="market-description">{{ t('skillManager.marketDescription') }}</p>
      <div class="market-features">
        <div class="feature-item">
          <svg viewBox="0 0 24 24" width="20" height="20" stroke="currentColor" stroke-width="2" fill="none">
            <polyline points="20 6 9 17 4 12"></polyline>
          </svg>
          <span>Claude Skills</span>
        </div>
        <div class="feature-item">
          <svg viewBox="0 0 24 24" width="20" height="20" stroke="currentColor" stroke-width="2" fill="none">
            <polyline points="20 6 9 17 4 12"></polyline>
          </svg>
          <span>LangChain Hub</span>
        </div>
        <div class="feature-item">
          <svg viewBox="0 0 24 24" width="20" height="20" stroke="currentColor" stroke-width="2" fill="none">
            <polyline points="20 6 9 17 4 12"></polyline>
          </svg>
          <span>Custom Markets</span>
        </div>
      </div>
    </div>
  </div>
</template>

<style scoped>
.skill-market {
  display: flex;
  align-items: center;
  justify-content: center;
  height: 100%;
  padding: 48px 24px;
}

.market-placeholder {
  text-align: center;
  max-width: 480px;
}

.market-icon {
  color: var(--text-muted);
  opacity: 0.3;
  margin-bottom: 24px;
}

.market-title {
  font-size: 24px;
  font-weight: 600;
  color: var(--text-main);
  margin: 0 0 12px 0;
}

.market-description {
  font-size: 14px;
  color: var(--text-muted);
  margin: 0 0 32px 0;
  line-height: 1.6;
}

.market-features {
  display: flex;
  flex-direction: column;
  gap: 12px;
  align-items: center;
}

.feature-item {
  display: flex;
  align-items: center;
  gap: 8px;
  color: var(--text-muted);
  font-size: 14px;
}

.feature-item svg {
  color: var(--accent-green);
}
</style>
