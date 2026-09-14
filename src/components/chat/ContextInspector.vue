<!--
# ContextInspector.vue — 上下文 token 组成诊断面板

展示当前会话最近一次请求的上下文快照，用总览卡、占比图、分段条和折叠明细帮助开发者定位 token 占用来源。

## Key Exports
- `ContextInspector`: 右侧 AgentPanel 内的上下文监控组件

## Dependencies
- Internal: `../../types`

## Constraints
- 只展示后端提供的估算值，不改变 Agent 请求或压缩策略
-->
<script setup lang="ts">
import { computed, ref, watch } from 'vue';
import { useI18n } from 'vue-i18n';
import { invoke } from '@tauri-apps/api/core';
import { emit as tauriEmit } from '@tauri-apps/api/event';
import ConfirmModal from '../common/ConfirmModal.vue';
import type { CacheHitPoint, ContextSectionSnapshot, SessionContextSnapshot } from '../../types';

const props = defineProps<{
  snapshot: SessionContextSnapshot | null;
  sessionId?: string | null;
}>();

const emit = defineEmits<{
  (e: 'compacted'): void;
}>();

const { t } = useI18n();

interface SectionView extends ContextSectionSnapshot {
  color: string;
  percent: number;
  dashOffset: number;
}

const palette = [
  'var(--accent-blue)',
  'color-mix(in srgb, var(--accent-blue) 60%, var(--text-muted))',
  'color-mix(in srgb, var(--accent-blue) 35%, var(--text-muted))',
  'color-mix(in srgb, var(--text-muted) 75%, transparent)',
  'color-mix(in srgb, var(--text-muted) 50%, transparent)',
  'var(--text-muted)',
];

const formatNumber = (value?: number | null): string => {
  const n = Number(value || 0);
  if (n >= 1_000_000) return `${(n / 1_000_000).toFixed(1)}m`;
  if (n >= 1000) return `${(n / 1000).toFixed(1)}k`;
  return String(n);
};

const formatToken = (value?: number | null): string => `${formatNumber(value)} tok`;

const formatFullNumber = (value?: number | null): string => Number(value || 0).toLocaleString();

const formatTime = (timestamp?: number | null): string => {
  if (!timestamp) return '';
  return new Date(timestamp).toLocaleTimeString([], {
    hour: '2-digit',
    minute: '2-digit',
    second: '2-digit',
  });
};

const sectionContent = (section: ContextSectionSnapshot): string => section.content?.trim() || t('monitor.context.emptyContent');

const methodLabel = (method?: string | null): string => {
  switch (method) {
    case 'tokenizer': return 'tokenizer';
    case 'estimate': return 'estimate';
    default: return method || 'unknown';
  }
};

const totalTokens = computed(() => Math.max(0, props.snapshot?.estimatedTokens || 0));
const providerTotalTokens = computed(() => props.snapshot?.providerTotalTokens ?? null);
const maxContextTokens = computed(() => props.snapshot?.maxContextTokens ?? null);
const contextUsagePercent = computed(() => {
  const max = maxContextTokens.value;
  if (!max) return null;
  return Math.min(999, Math.round((totalTokens.value / max) * 1000) / 10);
});

// ── 缓存命中 ──
// 语义：null = 该模型/链路未报告（显示 --，绝不显示 0%）；0 = 报告了但本次未命中（预热期）
const cacheHit = computed(() => props.snapshot?.cacheHitTokens ?? null);
const cacheMiss = computed(() => props.snapshot?.cacheMissTokens ?? null);
const cacheSource = computed(() => props.snapshot?.cacheSource ?? null);
const cacheTotal = computed(() => {
  if (cacheHit.value === null) return null;
  return (cacheHit.value ?? 0) + (cacheMiss.value ?? 0);
});
const cacheState = computed<'unknown' | 'warmup' | 'hit'>(() => {
  if (cacheHit.value === null) return 'unknown';
  return (cacheHit.value ?? 0) === 0 ? 'warmup' : 'hit';
});
const cacheRateLabel = computed(() => {
  if (cacheState.value === 'unknown') return t('monitor.context.cacheUnknown');
  const total = cacheTotal.value ?? 0;
  if (!total) return '0%';
  return `${Math.round(((cacheHit.value ?? 0) / total) * 100)}%`;
});
const cacheTitle = computed(() => {
  if (cacheState.value === 'unknown') return t('monitor.context.cacheUnknownHint');
  const src = cacheSource.value
    ? ` · ${t('monitor.context.cacheSource', { source: cacheSource.value })}`
    : '';
  const body = `${formatToken(cacheHit.value ?? 0)} / ${formatToken(cacheTotal.value ?? 0)}`;
  return cacheState.value === 'warmup'
    ? `${body}${src} · ${t('monitor.context.cacheWarmup')}`
    : `${t('monitor.context.cacheHit')} ${body}${src}`;
});

// ── 缓存命中趋势 ──
// 单看一个数字会被"预热期"误导（实测 GLM 第 1、2 轮均为 0%，第 3 轮才 97.8%），
// 所以把最近若干 loop 的命中率画成小柱子，让"预热 → 命中"这件事一眼可见。
const pointRate = (point: CacheHitPoint): number => {
  const total = (point.hitTokens || 0) + (point.missTokens || 0);
  if (!total) return 0;
  return Math.round(((point.hitTokens || 0) / total) * 100);
};

const cacheTrend = computed<CacheHitPoint[]>(() => (props.snapshot?.cacheHistory ?? []).slice(-10));

const cacheTrendSummary = computed(() => {
  const list = cacheTrend.value;
  if (!list.length) return '';
  const first = pointRate(list[0]);
  const last = pointRate(list[list.length - 1]);
  if (list.length === 1) return `${last}%`;
  return first === last ? `${last}%` : `${first}% → ${last}%`;
});

const pointTone = (point: CacheHitPoint): string => {
  if (!(point.hitTokens || 0)) return 'warmup';
  return pointRate(point) >= 50 ? 'hit' : 'partial';
};

const pointTitle = (point: CacheHitPoint): string => {
  const total = (point.hitTokens || 0) + (point.missTokens || 0);
  const src = point.source
    ? ` · ${t('monitor.context.cacheSource', { source: point.source })}`
    : '';
  return `${t('monitor.context.cacheLoop', { loop: point.loopCount })} · ${pointRate(point)}% (${formatToken(
    point.hitTokens,
  )} / ${formatToken(total)})${src}`;
};

const sectionViews = computed<SectionView[]>(() => {
  const total = totalTokens.value;
  let cursor = 0;
  const raw = (props.snapshot?.sections ?? [])
    .filter((section) => section.estimatedTokens > 0 || section.chars > 0);
  const rawTotal = raw.reduce((sum, section) => sum + Math.max(0, section.estimatedTokens), 0) || 1;

  return raw.map((section, index) => {
    const share = total > 0 ? Math.max(0, (section.estimatedTokens / rawTotal) * 100) : 0;
    const view = {
      ...section,
      color: palette[index % palette.length],
      percent: Math.round(share),
      dashOffset: 100 - cursor,
    };
    cursor += share;
    return view;
  });
});

const dominantSection = computed(() => {
  return [...sectionViews.value].sort((a, b) => b.estimatedTokens - a.estimatedTokens)[0] ?? null;
});

const usageTone = computed(() => {
  const percent = contextUsagePercent.value;
  if (percent !== null) {
    if (percent >= 90) return 'critical';
    if (percent >= 70) return 'warning';
    return 'safe';
  }
  const tokens = totalTokens.value;
  if (tokens >= 50_000) return 'critical';
  if (tokens >= 30_000) return 'warning';
  return 'safe';
});

const compacting = ref(false);
const compactError = ref('');
const compactMessage = ref('');
const showCompactConfirm = ref(false);

const compactThreshold = 70; // 达到上下文窗口 70% 才建议压缩

const canCompact = computed(() => {
  if (!props.snapshot) return false;
  const tokens = props.snapshot.estimatedTokens || 0;
  const max = props.snapshot.maxContextTokens;
  if (max && tokens > 0) return tokens >= max * compactThreshold / 100;
  // 如果不知道上下文窗口大小，用 token 绝对值判断: > 30k 可压缩
  return tokens > 30_000;
});

const compactHint = computed(() => {
  if (!props.snapshot) return t('monitor.context.compactDisabled');
  const tokens = props.snapshot.estimatedTokens || 0;
  const max = props.snapshot.maxContextTokens;
  if (max) {
    const pct = Math.round(tokens / max * 100);
    if (pct >= 90) return `Token 占用 ${pct}%，建议立即压缩`;
    if (pct >= compactThreshold) return `${t('monitor.context.compact')}（${pct}%）`;
    return `Token 占用 ${pct}%，可手动压缩`;
  }
  if (tokens >= 50_000) return `Token 占用 ${formatNumber(tokens)}，建议立即压缩`;
  if (tokens >= 30_000) return t('monitor.context.compact');
  return `Token 占用 ${formatNumber(tokens)}，可手动压缩`;
});

const triggerCompact = () => {
  if (!props.sessionId || compacting.value) return;
  showCompactConfirm.value = true;
};

const confirmCompact = async () => {
  showCompactConfirm.value = false;
  if (!props.sessionId) return;
  compacting.value = true;
  compactError.value = '';
  compactMessage.value = '';
  tauriEmit('bg-compacting-changed', { compacting: true });
  try {
    const result = await invoke<string>('compact_conversation', { sessionId: props.sessionId });
    compactMessage.value = result;
    setTimeout(() => { compactMessage.value = ''; }, 4000);
    tauriEmit('session-compacted', { sessionId: props.sessionId });
    emit('compacted');
  } catch (err) {
    compactError.value = String(err);
    setTimeout(() => { compactError.value = ''; }, 6000);
  } finally {
    compacting.value = false;
    tauriEmit('bg-compacting-changed', { compacting: false });
  }
};

// 挂载时查询后端，恢复 F5 刷新前的压缩状态
watch(() => props.sessionId, async (sid) => {
  if (!sid) return;
  try {
    const active = await invoke<boolean>('is_session_compacting', { sessionId: sid });
    compacting.value = active;
  } catch { /* ignore */ }
}, { immediate: true });

const usageLabel = computed(() => {
  switch (usageTone.value) {
    case 'critical': return t('monitor.context.critical');
    case 'warning': return t('monitor.context.warning');
    default: return t('monitor.context.safe');
  }
});

const copiedSection = ref<string | null>(null);
const showRawContent = ref(true);

const copySectionContent = async (section: ContextSectionSnapshot) => {
  const text = showRawContent.value && section.rawContent ? section.rawContent : sectionContent(section);
  try {
    await navigator.clipboard.writeText(text);
    copiedSection.value = section.key;
    setTimeout(() => { copiedSection.value = null; }, 1500);
  } catch {
    // fallback silently
  }
};
</script>

<template>
  <div v-if="snapshot" class="context-inspector">
    <div class="context-top-grid">
      <div class="context-hero" :class="`tone-${usageTone}`">
        <div>
          <div class="context-kicker">Context Budget</div>
          <div class="context-token-value">≈ {{ formatToken(snapshot.estimatedTokens) }}</div>
          <div class="context-subtitle">
            <template v-if="maxContextTokens">
              / {{ formatNumber(maxContextTokens) }} {{ t('monitor.context.contextSuffix') }} · {{ contextUsagePercent }}%
            </template>
            <template v-else>
              / {{ t('monitor.context.unknownContextWindow') }}
            </template>
            · {{ t('monitor.context.updated', { time: formatTime(snapshot.createdAt) }) }}
          </div>
        </div>
        <div class="context-hero-actions">
          <button
            v-if="sessionId"
            class="compact-btn"
            :class="{ 'compact-ready': canCompact, 'is-compacting': compacting }"
            :disabled="compacting"
            @click="triggerCompact"
            :title="compacting ? '压缩中...' : compactHint"
          >
            <svg v-if="compacting" class="compact-spinner" viewBox="0 0 24 24" width="13" height="13" fill="none" stroke="currentColor" stroke-width="2.5" stroke-linecap="round">
              <circle cx="12" cy="12" r="10" stroke-opacity="0.2" />
              <path d="M12 2a10 10 0 0 1 10 10" />
            </svg>
            <span v-else class="compact-btn-icon">&#9881;</span>
            <span class="compact-btn-text">{{ compacting ? '压缩中' : '压缩' }}</span>
          </button>
          <span class="context-health">{{ usageLabel }}</span>
        </div>
        <Transition name="toast-fade">
          <div v-if="compactMessage" class="compact-toast compact-toast-success">{{ compactMessage }}</div>
          <div v-else-if="compactError" class="compact-toast compact-toast-error">{{ compactError }}</div>
        </Transition>
      </div>

      <div class="context-overview-grid">
        <div class="context-stat-card">
          <span>{{ t('monitor.context.model') }}</span>
          <strong :title="snapshot.model">{{ snapshot.model }}</strong>
        </div>
        <div class="context-stat-card">
          <span>{{ t('monitor.context.contextWindow') }}</span>
          <strong>{{ maxContextTokens ? formatNumber(maxContextTokens) : t('monitor.context.unknown') }}</strong>
        </div>
        <div class="context-stat-card">
          <span>{{ t('monitor.context.usage') }}</span>
          <strong>{{ contextUsagePercent !== null ? `${contextUsagePercent}%` : t('monitor.context.unknown') }}</strong>
        </div>
        <div class="context-stat-card">
          <span>{{ t('monitor.context.providerActual') }}</span>
          <strong>{{ providerTotalTokens !== null ? formatToken(providerTotalTokens) : t('monitor.context.waitingUsage') }}</strong>
        </div>
        <div class="context-stat-card" :class="`cache-${cacheState}`">
          <span>{{ t('monitor.context.cacheHit') }}</span>
          <strong :title="cacheTitle">{{ cacheRateLabel }}</strong>
        </div>
      </div>
      <div v-if="cacheState !== 'hit'" class="context-cache-hint" :class="`cache-${cacheState}`">
        {{ cacheState === 'unknown'
          ? t('monitor.context.cacheUnknownHint')
          : t('monitor.context.cacheWarmup') }}
      </div>

      <div v-if="cacheTrend.length >= 2" class="context-cache-trend">
        <div class="trend-head">
          <span>{{ t('monitor.context.cacheTrend') }}</span>
          <strong>{{ cacheTrendSummary }}</strong>
        </div>
        <div class="trend-bars">
          <div
            v-for="point in cacheTrend"
            :key="point.loopCount"
            class="trend-slot"
            :class="`cache-${pointTone(point)}`"
            :title="pointTitle(point)"
          >
            <span class="trend-fill" :style="{ height: `${Math.max(pointRate(point), 3)}%` }" />
            <span class="trend-rate">{{ pointRate(point) }}</span>
          </div>
        </div>
      </div>
    </div>

    <div class="context-visual-grid">
      <div class="context-chart-card">
        <div class="donut" aria-hidden="true">
          <svg viewBox="0 0 42 42">
            <circle class="donut-track" cx="21" cy="21" r="15.9" />
            <circle
              v-for="section in sectionViews"
              :key="section.key"
              class="donut-segment"
              cx="21"
              cy="21"
              r="15.9"
              :stroke="section.color"
              :stroke-dasharray="`${section.percent} ${100 - section.percent}`"
              :stroke-dashoffset="section.dashOffset"
            />
          </svg>
          <div class="donut-center">
            <strong>{{ sectionViews.length }}</strong>
            <span>{{ t('monitor.context.sources') }}</span>
          </div>
        </div>
        <div class="context-chart-copy">
          <span>{{ t('monitor.context.dominantSource') }}</span>
          <strong v-if="dominantSection">{{ dominantSection.label }}</strong>
          <p v-if="dominantSection">
            {{ t('monitor.context.dominantShare', { tokens: formatToken(dominantSection.estimatedTokens), percent: dominantSection.percent }) }}
          </p>
          <p v-else>{{ t('monitor.context.noSources') }}</p>
        </div>
      </div>

      <div class="context-bars">
        <div v-for="section in sectionViews" :key="section.key" class="context-bar-row">
          <div class="context-bar-head">
            <span class="context-dot" :style="{ background: section.color }"></span>
            <span class="context-bar-title">{{ section.label }}</span>
            <strong>{{ formatToken(section.estimatedTokens) }}</strong>
          </div>
          <div class="context-bar-track" aria-hidden="true">
            <span :style="{ width: Math.max(3, section.percent) + '%', background: section.color }"></span>
          </div>
          <div class="context-bar-meta">
            <span>{{ Math.round(section.percent) }}%</span>
            <span>{{ t('monitor.context.sectionMeta', { chars: formatFullNumber(section.chars), items: section.itemCount, method: methodLabel(section.tokenCountMethod) }) }}</span>
          </div>
        </div>
      </div>
    </div>

    <div class="context-section-list">
      <div class="context-detail-header">
        <span class="context-detail-label">{{ t('monitor.context.details') }}</span>
        <button
          class="raw-toggle-btn"
          :class="{ active: showRawContent }"
          @click="showRawContent = !showRawContent"
          :title="showRawContent ? '显示格式化内容' : '显示原始 JSON'"
        >
          {{ showRawContent ? 'Formatted' : 'Raw JSON' }}
        </button>
      </div>
      <details
        v-for="section in sectionViews"
        :key="section.key"
        class="context-section-item"
      >
        <summary>
          <span class="context-section-head">
            <span class="context-dot" :style="{ background: section.color }"></span>
            <span class="context-section-title">{{ section.label }}</span>
            <span class="context-section-count">{{ section.itemCount }}</span>
          </span>
          <span class="context-section-stat">
            {{ formatToken(section.estimatedTokens) }}
          </span>
        </summary>
        <div class="context-section-extra">
          <span>{{ t('monitor.context.sectionDetailMeta', { chars: formatFullNumber(section.chars), method: methodLabel(section.tokenCountMethod) }) }}</span>
          <span v-if="section.truncated">{{ t('monitor.context.truncated') }}</span>
        </div>
        <div class="context-section-body">
          <pre>{{ showRawContent && section.rawContent ? section.rawContent : sectionContent(section) }}</pre>
          <div class="context-section-actions">
            <button
              class="copy-btn"
              :class="{ copied: copiedSection === section.key }"
              @click.stop="copySectionContent(section)"
            >
              {{ copiedSection === section.key ? '已复制' : '复制' }}
            </button>
          </div>
        </div>
      </details>
    </div>
  </div>

  <div v-else class="section-empty">{{ t('monitor.context.emptySnapshot') }}</div>

  <ConfirmModal
    :open="showCompactConfirm"
    :title="t('monitor.context.compact')"
    :message="t('monitor.context.cacheWarning', { tokens: formatToken(snapshot?.estimatedTokens || 0) })"
    confirm-kind="primary"
    @cancel="showCompactConfirm = false"
    @confirm="confirmCompact"
  />
</template>

<style scoped>
.context-inspector {
  display: flex;
  flex-direction: column;
  gap: 12px;
  min-width: 0;
}

.context-top-grid {
  display: grid;
  grid-template-columns: minmax(0, 1.2fr) minmax(220px, 0.8fr);
  gap: 12px;
}

.context-visual-grid {
  display: grid;
  grid-template-columns: minmax(220px, 0.85fr) minmax(0, 1.15fr);
  gap: 12px;
}

.section-empty {
  padding: 10px 2px;
  color: var(--text-muted);
  font-size: 0.68rem;
}

.context-hero,
.context-chart-card,
.context-stat-card,
.context-section-item,
.context-bar-row {
  border: 1px solid var(--border-color);
  background: var(--glass-bg);
  box-shadow: 0 8px 24px rgba(0, 0, 0, 0.12);
}

.context-hero {
  position: relative;
  display: flex;
  align-items: flex-start;
  justify-content: space-between;
  gap: 10px;
  padding: 14px;
  border-radius: 12px;
  background: var(--glass-bg);
}

.context-hero.tone-warning {
  background: color-mix(in srgb, var(--accent-blue) 5%, var(--glass-bg));
}

.context-hero.tone-critical {
  background: color-mix(in srgb, var(--accent-red) 6%, var(--glass-bg));
}

.context-kicker,
.context-subtitle,
.context-stat-card span,
.context-chart-copy span,
.context-chart-copy p,
.context-bar-meta,
.context-section-extra,
.context-section-stat {
  color: var(--text-muted);
  font-size: 0.62rem;
}

.context-kicker {
  text-transform: uppercase;
  letter-spacing: 0.08em;
  font-weight: 800;
}

.context-token-value {
  margin-top: 4px;
  color: var(--text-main);
  font-size: 1.38rem;
  font-weight: 850;
  font-variant-numeric: tabular-nums;
}

.context-subtitle {
  margin-top: 3px;
  font-variant-numeric: tabular-nums;
}

.context-hero-actions {
  display: flex;
  align-items: center;
  gap: 6px;
}
.compact-btn {
  flex-shrink: 0;
  display: inline-flex;
  align-items: center;
  gap: 5px;
  height: 26px;
  padding: 0 9px;
  border: 1px solid var(--border-color);
  border-radius: 6px;
  background: var(--glass-bg);
  color: var(--text-muted);
  font-size: 0.65rem;
  font-weight: 700;
  cursor: pointer;
  transition: all 0.15s;
  white-space: nowrap;
}
.compact-btn:hover {
  color: var(--accent-blue);
  border-color: var(--accent-blue);
}
.compact-btn.compact-ready {
  color: var(--accent-blue);
  border-color: color-mix(in srgb, var(--accent-blue) 50%, transparent);
}
.compact-btn.is-compacting {
  color: var(--accent-blue);
  border-color: color-mix(in srgb, var(--accent-blue) 40%, transparent);
  cursor: wait;
}
.compact-btn:disabled {
  opacity: 0.6;
  cursor: wait;
}
.compact-btn-icon {
  font-size: 0.75rem;
  line-height: 1;
}
.compact-btn-text {
  line-height: 1;
}
.compact-spinner {
  flex-shrink: 0;
  animation: compact-spin 0.8s linear infinite;
}
@keyframes compact-spin {
  to { transform: rotate(360deg); }
}

.compact-toast {
  position: absolute;
  bottom: -36px;
  left: 50%;
  transform: translateX(-50%);
  z-index: 10;
  max-width: 340px;
  padding: 5px 14px;
  border-radius: 6px;
  font-size: 0.65rem;
  font-weight: 700;
  line-height: 1.4;
  text-align: center;
  white-space: nowrap;
  pointer-events: none;
  box-shadow: 0 4px 16px rgba(0, 0, 0, 0.22);
}
.compact-toast-success {
  color: var(--accent-green);
  background: color-mix(in srgb, var(--accent-green) 14%, var(--surface-strong));
  border: 1px solid color-mix(in srgb, var(--accent-green) 30%, transparent);
}
.compact-toast-error {
  color: var(--accent-red);
  background: color-mix(in srgb, var(--accent-red) 14%, var(--surface-strong));
  border: 1px solid color-mix(in srgb, var(--accent-red) 30%, transparent);
}

.toast-fade-enter-active,
.toast-fade-leave-active {
  transition: all 0.2s ease;
}
.toast-fade-enter-from,
.toast-fade-leave-to {
  opacity: 0;
  transform: translateX(-50%) translateY(4px);
}

.context-health {
  flex-shrink: 0;
  padding: 4px 7px;
  border-radius: 999px;
  color: var(--text-main);
  background: color-mix(in srgb, var(--accent-green) 18%, transparent);
  font-size: 0.62rem;
  font-weight: 800;
}

.tone-warning .context-health {
  background: color-mix(in srgb, var(--accent-blue) 18%, transparent);
}

.tone-critical .context-health {
  background: color-mix(in srgb, var(--accent-red) 22%, transparent);
}

.context-overview-grid {
  display: grid;
  grid-template-columns: repeat(2, minmax(0, 1fr));
  gap: 8px;
}

.context-stat-card {
  min-width: 0;
  padding: 9px;
  border-radius: 10px;
}

.context-stat-card span {
  display: block;
  margin-bottom: 3px;
}

.context-stat-card strong {
  display: block;
  min-width: 0;
  overflow: hidden;
  color: var(--text-main);
  font-size: 0.72rem;
  font-weight: 800;
  text-overflow: ellipsis;
  white-space: nowrap;
  font-variant-numeric: tabular-nums;
}

/* 缓存命中卡片：命中=绿、预热=蓝、未知=灰；颜色只做辅助，语义靠文案 */
.context-stat-card.cache-hit {
  border-color: color-mix(in srgb, var(--accent-green) 42%, var(--border-color));
  background: color-mix(in srgb, var(--accent-green) 8%, var(--glass-bg));
}

.context-stat-card.cache-hit strong {
  color: var(--accent-green);
}

.context-stat-card.cache-warmup {
  border-color: color-mix(in srgb, var(--accent-blue) 34%, var(--border-color));
}

.context-stat-card.cache-warmup strong {
  color: var(--accent-blue);
}

.context-stat-card.cache-unknown strong {
  color: var(--text-muted);
}

.context-cache-hint {
  margin-top: 8px;
  padding: 7px 9px;
  border-radius: 9px;
  border: 1px dashed var(--border-color);
  color: var(--text-muted);
  font-size: 0.64rem;
  line-height: 1.5;
}

.context-cache-hint.cache-warmup {
  border-color: color-mix(in srgb, var(--accent-blue) 30%, var(--border-color));
  color: color-mix(in srgb, var(--accent-blue) 75%, var(--text-main));
}

.context-cache-trend {
  margin-top: 8px;
  padding: 9px;
  border-radius: 10px;
  border: 1px solid var(--border-color);
  background: var(--glass-bg);
}

.trend-head {
  display: flex;
  align-items: baseline;
  justify-content: space-between;
  gap: 8px;
  margin-bottom: 7px;
  color: var(--text-muted);
  font-size: 0.62rem;
}

.trend-head strong {
  color: var(--text-main);
  font-size: 0.68rem;
  font-variant-numeric: tabular-nums;
}

.trend-bars {
  display: flex;
  align-items: flex-end;
  gap: 4px;
  height: 46px;
}

.trend-slot {
  position: relative;
  display: flex;
  flex: 1 1 0;
  min-width: 0;
  height: 100%;
  align-items: flex-end;
  border-radius: 4px;
  background: color-mix(in srgb, var(--text-muted) 14%, transparent);
  overflow: hidden;
  cursor: default;
}

.trend-fill {
  display: block;
  width: 100%;
  border-radius: 4px;
  background: var(--accent-blue);
  transition: height 0.25s ease;
}

.trend-slot.cache-hit .trend-fill {
  background: var(--accent-green);
}

.trend-slot.cache-partial .trend-fill {
  background: var(--accent-blue);
}

.trend-slot.cache-warmup .trend-fill {
  background: color-mix(in srgb, var(--text-muted) 55%, transparent);
}

.trend-rate {
  position: absolute;
  inset: auto 0 2px 0;
  text-align: center;
  color: var(--text-main);
  font-size: 0.54rem;
  font-weight: 700;
  font-variant-numeric: tabular-nums;
  text-shadow: 0 1px 2px rgba(0, 0, 0, 0.35);
  pointer-events: none;
}

.context-chart-card {
  display: grid;
  grid-template-columns: 92px minmax(0, 1fr);
  align-items: center;
  gap: 14px;
  padding: 12px;
  border-radius: 12px;
}

.donut {
  position: relative;
  width: 86px;
  height: 86px;
}

.donut svg {
  width: 100%;
  height: 100%;
  transform: rotate(-90deg);
}

.donut-track,
.donut-segment {
  fill: none;
  stroke-width: 4;
}

.donut-track {
  stroke: color-mix(in srgb, var(--text-muted) 14%, transparent);
}

.donut-segment {
  transition: stroke-dasharray 180ms ease-out;
}

.donut-center {
  position: absolute;
  inset: 0;
  display: flex;
  flex-direction: column;
  align-items: center;
  justify-content: center;
  pointer-events: none;
}

.donut-center strong {
  color: var(--text-main);
  font-size: 1rem;
  line-height: 1;
}

.donut-center span {
  margin-top: 2px;
  color: var(--text-muted);
  font-size: 0.58rem;
}

.context-chart-copy {
  min-width: 0;
}

.context-chart-copy strong {
  display: block;
  margin-top: 4px;
  overflow: hidden;
  color: var(--text-main);
  font-size: 0.86rem;
  font-weight: 850;
  text-overflow: ellipsis;
  white-space: nowrap;
}

.context-chart-copy p {
  margin: 4px 0 0;
  line-height: 1.45;
}

.context-bars {
  display: flex;
  flex-direction: column;
  gap: 7px;
}

.context-bar-row {
  padding: 9px;
  border-radius: 10px;
}

.context-bar-head,
.context-section-head,
.context-bar-meta,
.context-section-extra {
  display: flex;
  align-items: center;
  gap: 6px;
  min-width: 0;
}

.context-bar-head strong {
  margin-left: auto;
  color: var(--text-main);
  font-size: 0.66rem;
  font-variant-numeric: tabular-nums;
}

.context-dot {
  width: 7px;
  height: 7px;
  flex-shrink: 0;
  border-radius: 999px;
  box-shadow: 0 0 12px currentColor;
}

.context-bar-title,
.context-section-title {
  min-width: 0;
  overflow: hidden;
  color: var(--text-main);
  font-size: 0.7rem;
  font-weight: 750;
  text-overflow: ellipsis;
  white-space: nowrap;
}

.context-bar-track {
  height: 5px;
  margin-top: 7px;
  overflow: hidden;
  border-radius: 999px;
  background: color-mix(in srgb, var(--text-muted) 12%, transparent);
}

.context-bar-track span {
  display: block;
  height: 100%;
  border-radius: inherit;
}

.context-bar-meta {
  justify-content: space-between;
  margin-top: 5px;
  font-variant-numeric: tabular-nums;
}

.context-section-list {
  display: flex;
  flex-direction: column;
  gap: 6px;
}

.context-detail-header {
  display: flex;
  align-items: center;
  justify-content: space-between;
  gap: 8px;
}

.context-detail-label {
  color: var(--text-muted);
  font-size: 0.58rem;
  font-weight: 850;
  letter-spacing: 0.08em;
  text-transform: uppercase;
}

.raw-toggle-btn {
  padding: 2px 8px;
  border: 1px solid var(--border-color);
  border-radius: 4px;
  background: var(--glass-bg);
  color: var(--text-muted);
  font-size: 0.55rem;
  font-weight: 700;
  cursor: pointer;
  transition: all 0.15s;
}

.raw-toggle-btn:hover {
  color: var(--text-main);
  border-color: var(--text-muted);
}

.raw-toggle-btn.active {
  color: var(--accent-blue);
  border-color: var(--accent-blue);
  background: color-mix(in srgb, var(--accent-blue) 10%, transparent);
}

.context-section-item {
  border-radius: 8px;
  overflow: hidden;
}

.context-section-item summary {
  min-height: 34px;
  display: grid;
  grid-template-columns: minmax(0, 1fr) auto;
  align-items: center;
  gap: 8px;
  padding: 7px 8px;
  cursor: pointer;
  list-style: none;
}

.context-section-item summary::-webkit-details-marker {
  display: none;
}

.context-section-item summary:hover {
  background: var(--glass-bg-light);
}

.context-section-count {
  min-width: 16px;
  height: 16px;
  padding: 0 5px;
  border-radius: 8px;
  display: inline-flex;
  align-items: center;
  justify-content: center;
  color: var(--text-muted);
  background: var(--glass-bg-light);
  font-size: 0.58rem;
  font-weight: 750;
  font-variant-numeric: tabular-nums;
}

.context-section-stat {
  white-space: nowrap;
  font-weight: 750;
  font-variant-numeric: tabular-nums;
}

.context-section-extra {
  justify-content: space-between;
  padding: 6px 8px 0;
  font-variant-numeric: tabular-nums;
}

.context-section-body {
  position: relative;
}

.context-section-actions {
  position: absolute;
  top: 10px;
  right: 12px;
  display: flex;
  gap: 4px;
}

.context-section-item pre {
  margin: 6px 8px 8px;
  padding: 8px 60px 8px 8px;
  overflow: auto;
  color: var(--text-main);
  border: 1px solid var(--border-color);
  border-radius: 6px;
  background: color-mix(in srgb, var(--bg-dark) 78%, var(--surface-strong));
  font-family: var(--font-mono);
  font-size: 0.62rem;
  line-height: 1.45;
  white-space: pre-wrap;
  word-break: break-word;
}

.copy-btn {
  position: absolute;
  top: 10px;
  right: 12px;
  padding: 2px 6px;
  border: 1px solid var(--border-color);
  border-radius: 4px;
  background: var(--glass-bg);
  color: var(--text-muted);
  font-size: 0.55rem;
  font-weight: 700;
  cursor: pointer;
  transition: all var(--transition-fast);
}

.copy-btn:hover {
  color: var(--text-main);
  border-color: var(--text-muted);
}

.copy-btn.copied {
  color: var(--accent-green);
  border-color: var(--accent-green);
}

@media (max-width: 560px) {
  .context-top-grid,
  .context-visual-grid {
    grid-template-columns: 1fr;
  }
}
</style>
