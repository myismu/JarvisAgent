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
  gap: 14px;
}

.about-card {
  border: 1px solid var(--glass-border);
  border-radius: var(--radius-lg);
  background: var(--glass-bg);
  padding: 16px 18px;
}

.about-card h4 {
  margin: 0 0 12px;
  font-size: 14px;
  font-weight: var(--fw-semibold);
  color: var(--text-main);
}

.about-row {
  display: flex;
  align-items: center;
  gap: 10px;
}

.about-label {
  font-size: 13px;
  color: var(--text-muted);
}

.about-value {
  font-family: var(--font-mono);
  font-size: 13px;
  color: var(--text-main);
}

.about-btn {
  margin-left: auto;
  padding: 6px 14px;
  border-radius: var(--radius-md);
  border: 1px solid var(--glass-border);
  background: var(--glass-bg-light);
  color: var(--text-main);
  font-size: 12px;
  cursor: pointer;
  transition: border-color var(--transition-fast), opacity var(--transition-fast);
}

.about-btn:hover:not(:disabled) {
  border-color: var(--accent-blue);
}

.about-btn.primary {
  background: var(--accent-blue);
  border-color: var(--accent-blue);
  color: #fff;
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
