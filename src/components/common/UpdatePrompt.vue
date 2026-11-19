<!--
# UpdatePrompt.vue — 启动时发现的更新提示弹窗

展示新版本号与更新说明，提供「稍后」与「下载并安装」；安装完成后切换为「重启应用」。
状态来自 useUpdater 单例，与设置-关于页共享。

## Constraints
- 下载过程中禁用「稍后」：NSIS 安装器即将接管，此时放弃会让状态难以恢复
- 安装完成不自动重启，由用户决定重启时机
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
</script>

<template>
  <Teleport to="body">
    <div v-if="updater.modalVisible.value && pending" class="update-overlay">
      <div class="update-modal">
        <h3>{{ t('settings.about.updateFoundTitle') }}</h3>
        <p class="update-subtitle">{{ availableText }}</p>

        <div class="update-notes">
          <div class="notes-title">{{ t('settings.about.notes') }}</div>
          <pre class="notes-body">{{ notesText }}</pre>
        </div>

        <div v-if="updater.downloading.value || updater.installed.value" class="update-progress">
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

        <div v-if="updater.error.value && updater.error.value !== 'dev'" class="update-error">
          {{ t('settings.about.updateError', { error: updater.error.value }) }}
        </div>

        <div class="update-actions">
          <button class="btn-ghost" :disabled="updater.downloading.value" @click="updater.dismiss">
            {{ t('settings.about.later') }}
          </button>
          <button v-if="updater.installed.value" class="btn-primary" @click="updater.restart">
            {{ t('settings.about.restart') }}
          </button>
          <button
            v-else
            class="btn-primary"
            :disabled="updater.downloading.value"
            @click="updater.install"
          >
            {{ updater.downloading.value ? progressText : t('settings.about.install') }}
          </button>
        </div>
      </div>
    </div>
  </Teleport>
</template>

<style scoped>
.update-overlay {
  position: fixed;
  inset: 0;
  z-index: 1200;
  display: flex;
  align-items: center;
  justify-content: center;
  background: rgba(0, 0, 0, 0.45);
}

.update-modal {
  width: min(520px, calc(100vw - 48px));
  padding: 22px 24px 18px;
  border-radius: var(--radius-lg);
  border: 1px solid var(--glass-border);
  background: var(--bg-panel, var(--glass-bg));
  color: var(--text-main);
  box-shadow: 0 18px 60px rgba(0, 0, 0, 0.35);
}

.update-modal h3 {
  margin: 0 0 6px;
  font-size: 16px;
  font-weight: var(--fw-semibold);
}

.update-subtitle {
  margin: 0 0 14px;
  font-size: 13px;
  color: var(--text-soft);
}

.update-notes {
  border: 1px solid var(--glass-border-subtle);
  border-radius: var(--radius-md);
  background: var(--glass-bg-light);
  padding: 10px 12px;
  margin-bottom: 14px;
}

.notes-title {
  font-size: 12px;
  color: var(--text-muted);
  margin-bottom: 6px;
}

.notes-body {
  margin: 0;
  max-height: 220px;
  overflow: auto;
  white-space: pre-wrap;
  word-break: break-word;
  font-family: var(--font-mono);
  font-size: 12px;
  line-height: 1.55;
  color: var(--text-soft);
}

.update-progress {
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

.update-error {
  margin-bottom: 12px;
  font-size: 12px;
  color: var(--text-warning);
  word-break: break-word;
}

.update-actions {
  display: flex;
  justify-content: flex-end;
  gap: 10px;
}

.btn-primary,
.btn-ghost {
  padding: 7px 16px;
  border-radius: var(--radius-md);
  border: 1px solid var(--glass-border);
  font-size: 13px;
  cursor: pointer;
  transition: border-color var(--transition-fast), opacity var(--transition-fast);
}

.btn-primary {
  background: var(--accent-blue);
  border-color: var(--accent-blue);
  color: #fff;
}

.btn-ghost {
  background: transparent;
  color: var(--text-soft);
}

.btn-primary:disabled,
.btn-ghost:disabled {
  opacity: 0.55;
  cursor: default;
}
</style>
