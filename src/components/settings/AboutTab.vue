<!--
# AboutTab.vue — 设置里的「关于与更新」页

展示当前版本，提供手动检查更新；发现新版本时展示说明并按需下载安装、重启生效。
状态来自 useUpdater 单例，与启动时的更新弹窗共享。

## Constraints
- 开发模式（tauri dev）下检查更新会提示"开发模式不支持"，避免误装正式安装包
- 版本号来自 Tauri getVersion()，与安装包版本一致
-->
<script setup lang="ts">
import { computed } from 'vue';
import { useI18n } from 'vue-i18n';
import { useUpdater } from '../../composables/useUpdater';

const { t } = useI18n();
const updater = useUpdater();

const progressText = computed(() =>
  updater.progress.value >= 0
    ? t('settings.about.downloading', { percent: updater.progress.value })
    : t('settings.about.downloadingUnknown'),
);

const pending = computed(() => updater.update.value);
const availableText = computed(() =>
  t('settings.about.available', {
    version: pending.value?.version ?? '',
    current: updater.currentVersion.value || '?',
  }),
);
const notesText = computed(() => pending.value?.body || t('settings.about.noNotes'));

const statusText = computed(() => {
  if (updater.checking.value) return '';
  if (updater.error.value === 'dev') return t('settings.about.devHint');
  if (updater.error.value) return t('settings.about.updateError', { error: updater.error.value });
  if (updater.upToDate.value) return t('settings.about.upToDate');
  return '';
});
</script>

<template>
  <div class="about-tab">
    <!-- 品牌头块：极简反应环标志 + 名称 + 版本徽章 -->
    <div class="about-hero">
      <div class="about-logo" aria-hidden="true">
        <svg viewBox="0 0 48 48" width="44" height="44" fill="none" xmlns="http://www.w3.org/2000/svg">
          <circle cx="24" cy="24" r="21" stroke="var(--accent-blue)" stroke-width="1.5" stroke-dasharray="6 4" opacity="0.55" />
          <circle cx="24" cy="24" r="14" stroke="var(--accent-blue)" stroke-width="1.5" stroke-dasharray="28 60" stroke-linecap="round" />
          <circle cx="24" cy="24" r="4.5" fill="var(--accent-blue)" />
        </svg>
      </div>
      <div class="about-brand">
        <div class="about-brand-row">
          <span class="about-brand-name">JarvisAgent</span>
          <span class="about-version-pill">v{{ updater.currentVersion.value || '—' }}</span>
        </div>
        <p class="about-brand-sub">AI 驱动的桌面端编程助手</p>
      </div>
    </div>

    <div class="about-card">
      <h4>{{ t('settings.about.title') }}</h4>
      <div class="about-row">
        <span class="about-label">{{ t('settings.about.version') }}</span>
        <span class="about-value">{{ updater.currentVersion.value || '—' }}</span>
        <button
          class="about-btn"
          :disabled="updater.checking.value"
          @click="updater.checkForUpdate(true)"
        >
          {{ updater.checking.value ? t('settings.about.checking') : t('settings.about.check') }}
        </button>
      </div>
      <div class="about-row">
        <span class="about-label">{{ t('settings.about.channel') }}</span>
        <span class="about-value">{{ t('settings.about.channelValue') }}</span>
      </div>
      <div v-if="statusText" class="about-status">{{ statusText }}</div>
    </div>

    <div v-if="pending" class="about-card">
      <h4>{{ t('settings.about.updateFoundTitle') }}</h4>
      <div class="about-desc">{{ availableText }}</div>
      <pre class="about-notes">{{ notesText }}</pre>

      <div v-if="updater.downloading.value || updater.installed.value" class="about-progress">
        <div class="progress-track">
          <div
            class="progress-fill"
            :style="{ width: `${updater.progress.value < 0 ? 12 : updater.progress.value}%` }"
          />
        </div>
        <span class="progress-label">
          {{ updater.installed.value ? t('settings.about.installed') : progressText }}
        </span>
      </div>

      <div class="about-actions">
        <button v-if="updater.installed.value" class="about-btn primary" @click="updater.restart">
          {{ t('settings.about.restart') }}
        </button>
        <button
          v-else
          class="about-btn primary"
          :disabled="updater.downloading.value"
          @click="updater.install"
        >
          {{ updater.downloading.value ? progressText : t('settings.about.install') }}
        </button>
      </div>
    </div>
  </div>
</template>

<style scoped>
.about-tab {
  display: flex;
  flex-direction: column;
  gap: 18px;
}

.about-hero {
  display: flex;
  align-items: center;
  gap: 16px;
  padding: 18px 20px;
  border: 1px solid var(--glass-border-subtle);
  border-radius: var(--radius-lg);
  background: var(--bg-panel);
  box-shadow: 0 1px 2px rgba(0, 0, 0, 0.02);
}

.about-logo {
  flex-shrink: 0;
  width: 44px;
  height: 44px;
  display: flex;
  align-items: center;
  justify-content: center;
}

.about-brand {
  display: flex;
  flex-direction: column;
  gap: 4px;
  min-width: 0;
}

.about-brand-row {
  display: flex;
  align-items: center;
  gap: 10px;
}

.about-brand-name {
  font-size: 1.05rem;
  font-weight: 700;
  letter-spacing: 0.02em;
  color: var(--text-main);
}

.about-version-pill {
  padding: 2px 8px;
  border-radius: 999px;
  border: 1px solid var(--glass-border-subtle);
  background: var(--surface-1);
  font-family: var(--font-mono);
  font-size: 0.72rem;
  font-weight: 600;
  color: var(--text-soft);
}

.about-brand-sub {
  margin: 0;
  font-size: 0.82rem;
  color: var(--text-muted);
}

.about-card {
  border: 1px solid var(--glass-border-subtle);
  border-radius: var(--radius-lg);
  background: var(--bg-panel);
  padding: 14px 18px;
  box-shadow: 0 1px 2px rgba(0, 0, 0, 0.02);
}

.about-card h4 {
  margin: 0 0 8px;
  font-size: 0.75rem;
  font-weight: 600;
  letter-spacing: 0.04em;
  text-transform: uppercase;
  color: var(--text-faint);
}

.about-row {
  display: flex;
  align-items: center;
  gap: 12px;
  padding: 10px 0;
  border-bottom: 1px solid var(--glass-border-subtle);
}

.about-row:last-of-type {
  border-bottom: none;
  padding-bottom: 4px;
}

.about-label {
  font-size: 0.85rem;
  font-weight: 500;
  color: var(--text-main);
  min-width: 72px;
}

.about-value {
  font-family: var(--font-mono);
  font-size: 0.82rem;
  color: var(--text-muted);
}

.about-btn {
  margin-left: auto;
  height: 30px;
  padding: 0 14px;
  border-radius: var(--radius-md);
  border: 1px solid var(--glass-border);
  background: var(--bg-panel);
  color: var(--text-main);
  font-size: 0.78rem;
  font-weight: 500;
  cursor: pointer;
  box-shadow: 0 1px 2px rgba(0, 0, 0, 0.03);
  transition: border-color var(--transition-fast), background var(--transition-fast), opacity var(--transition-fast);
}

.about-btn:hover:not(:disabled) {
  border-color: var(--border-strong);
  background: var(--surface-1);
}

.about-btn.primary {
  background: var(--btn-primary-bg);
  border-color: var(--btn-primary-bg);
  color: var(--btn-primary-fg);
}

.about-btn:disabled {
  opacity: 0.55;
  cursor: default;
}

.about-status {
  margin-top: 10px;
  font-size: 12px;
  color: var(--text-muted);
  word-break: break-word;
}

.about-desc {
  font-size: 13px;
  color: var(--text-soft);
  margin-bottom: 10px;
}

.about-notes {
  margin: 0 0 12px;
  max-height: 220px;
  overflow: auto;
  white-space: pre-wrap;
  word-break: break-word;
  font-family: var(--font-mono);
  font-size: 12px;
  line-height: 1.55;
  color: var(--text-soft);
  border: 1px solid var(--glass-border-subtle);
  border-radius: var(--radius-md);
  background: var(--glass-bg-light);
  padding: 10px 12px;
}

.about-progress {
  margin-bottom: 12px;
}

.progress-track {
  height: 6px;
  border-radius: 999px;
  background: var(--toggle-track-inactive, rgba(127, 127, 127, 0.25));
  overflow: hidden;
}

.progress-fill {
  height: 100%;
  border-radius: 999px;
  background: var(--accent-blue);
  transition: width var(--transition-fast);
}

.progress-label {
  display: inline-block;
  margin-top: 6px;
  font-size: 12px;
  color: var(--text-muted);
}
</style>
