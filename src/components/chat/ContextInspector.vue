<!--
# ContextInspector.vue — 上下文 token 组成诊断面板

展示当前会话最近一次请求的上下文快照，用总览卡、占比图、分段条和折叠明细帮助开发者定位 token 占用来源。

## Key Exports
- `ContextInspector`: 右侧 AgentPanel 内的上下文监控组件

## Dependencies
- Internal: `../../types`

## Constraints
- 占用口径与输入框读数环同源（`utils/contextUsage.ts`）：厂商实测优先、本地估算兜底，
  估算态必须显式标注；分区明细无法实测（API 只回总量），只能估算
- 手动压缩按钮的**可用性判据必须与后端同源**：看消息条数（`COMPACT_MIN_MESSAGES`，
  对应 `command/session.rs::compact_inner()`），**不是** token 占比；占比只用于"建议压缩"徽标。
  自动压缩走另一套（`infra/llm/context_budget.rs` 的 token 判据），两者按设计分开
- 只读展示，不改变 Agent 请求或压缩策略（压缩按钮是显式用户操作）
-->
<script setup lang="ts">
import { computed, ref, watch } from 'vue';
import { useI18n } from 'vue-i18n';
import { invoke } from '@tauri-apps/api/core';
import { emit as tauriEmit } from '@tauri-apps/api/event';
import ConfirmModal from '../common/ConfirmModal.vue';
import { useSessionStore } from '../../stores/session';
import { canManuallyCompact, COMPACT_MIN_MESSAGES, COMPACT_SUGGEST_PERCENT, COMPACT_TRIGGER_PERCENT, isContextMeasured, resolveContextPercent, resolveContextTokens } from '../../utils/contextUsage';
import type { CacheHitPoint, ContextSectionSnapshot, SessionContextSnapshot } from '../../types';

const props = defineProps<{
  snapshot: SessionContextSnapshot | null;
  sessionId?: string | null;
}>();

const emit = defineEmits<{
  (e: 'compacted', sessionId: string): void;
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

/**
 * 上下文占用（分子）。
 *
 * 口径与输入框读数环**完全同源**（`utils/contextUsage.ts`）：实测优先、估算兜底。
 * 面板早期全量使用 `estimatedTokens`，那时输入框用的也是估算，两边一致；输入框改成
 * 实测口径后两边会差出数倍（估算对 `tool_result` 截断，系统性偏低），所以这里一并统一。
 */
const totalTokens = computed(() => resolveContextTokens(props.snapshot));
/** 分子是否来自厂商实测（false = 本地估算兜底，显示时要标出来，别和实测值长得一样） */
const contextMeasured = computed(() => isContextMeasured(props.snapshot));
const providerTotalTokens = computed(() => props.snapshot?.providerTotalTokens ?? null);
const maxContextTokens = computed(() => props.snapshot?.maxContextTokens ?? null);
const contextUsagePercent = computed(() =>
  resolveContextPercent(totalTokens.value, maxContextTokens.value),
);

// ── 缓存命中 ──
// 语义：null = 该模型/链路未报告（显示 --，绝不显示 0%）；0 = 报告了但本次未命中（预热期）
//
// ⚠️ 口径：用**会话累计**（session store），与输入框的「累计命中」同源。
// 原先取 snapshot 的单轮值，导致面板显示 99%、输入框显示 92%，用户合理认为坏了。
// 单轮信息没有丢失——下面的缓存趋势图本来就是逐轮的。
const session = useSessionStore();
const cacheReported = computed(() => props.snapshot?.cacheHitTokens != null);
const cacheHit = computed(() =>
  cacheReported.value ? session.totalCacheHitTokens : null,
);
const cacheMiss = computed(() =>
  cacheReported.value ? session.totalCacheMissTokens : null,
);
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

/** 趋势只在异常时展示：稳态下它是一排"一切正常"的零信息柱子。
   异常 = 最近一轮（已起命中的轮次）命中率 < 80%，或较上一轮骤降 > 20pt。
   预热轮（hitTokens=0）不参与判断 —— 会话初期缓存未热是正常生理现象。 */
const cacheTrendAnomaly = computed(() => {
  const pts = cacheTrend.value.filter((p) => (p.hitTokens || 0) > 0);
  if (pts.length < 2) return false;
  const last = pointRate(pts[pts.length - 1]);
  const prev = pointRate(pts[pts.length - 2]);
  return last < 80 || prev - last > 20;
});

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

/**
 * 明细里不再按行展示的"历史分区"。
 *
 * 后端已把分区收敛为 system / messages / tools 三项（三者之和即总量），但**落库的旧快照**
 * 里还留着 `attachments`（图片）与 `runtime`（tool_result / thinking）两个分区。
 * 它们都不是体积来源，而是对 messages 内容的重复统计：继续按行显示会让"分区之和"
 * 对不上总量，也会让用户以为删掉的内容还在占 token。故一并从明细里剔除：
 * - `runtime`：数量信息由顶层 `toolCallCount` / `toolResultCount` 承担，直接丢弃；
 * - `attachments`：它的 `itemCount` 是旧快照里唯一的图片数来源，留给 `imageCount` 兜底。
 */
const LEGACY_SECTION_KEYS = new Set(['runtime', 'attachments']);

const sectionViews = computed<SectionView[]>(() => {
  const total = totalTokens.value;
  let cursor = 0;
  const raw = (props.snapshot?.sections ?? [])
    .filter((section) => !LEGACY_SECTION_KEYS.has(section.key)
      && (section.estimatedTokens > 0 || section.chars > 0));
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

/**
 * 图片块数量。
 *
 * 新快照直接读顶层 `imageCount`（后端 `image_count`）；旧快照没有该字段，那个数字
 * 只存在于已废弃的 `attachments` 分区的 `itemCount` 里，故用它兜底。
 * 注意判据是 `!= null` 而不是真值 —— `imageCount === 0` 是有效值（确实没有图片），
 * 不能因此掉进兜底分支去读旧分区。
 */
const imageCount = computed(() => {
  const direct = props.snapshot?.imageCount;
  if (direct != null) return direct;
  const legacy = (props.snapshot?.sections ?? []).find((section) => section.key === 'attachments');
  return legacy?.itemCount ?? 0;
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

// ── 上下文环形图 ──
// 只画「已用 / 可用」两段，不把 sections 搬进来：分区来源曾经就是甜甜圈图，
// 因为「4 个来源」信息量太低被删掉（见下方明细区注释）。明细已由 section 列表承担。
const RING_RADIUS = 52;
const RING_CIRCUMFERENCE = 2 * Math.PI * RING_RADIUS;

const ringPercent = computed(() => contextUsagePercent.value ?? 0);
const ringUsedLength = computed(
  () => (Math.min(100, ringPercent.value) / 100) * RING_CIRCUMFERENCE,
);

/** 剩余可用 token（窗口未登记时为 null，不编数字） */
const remainingTokens = computed(() => {
  const max = maxContextTokens.value;
  if (!max || max <= 0) return null;
  return Math.max(0, max - totalTokens.value);
});

/**
 * 自动压缩触发点在环上的占比。
 *
 * ⚠️ 不是 85% 的窗口：后端判据是 `(窗口 − 输出预算) × 85%`（infra/llm/context_budget.rs）。
 * 121K 窗口 / 32K 输出预算下实际约 62.5%。直接按窗口画 85% 会严重误导。
 */
const compactTriggerPercent = computed(() => {
  const max = maxContextTokens.value;
  if (!max || max <= 0) return null;
  const usable = max - (props.snapshot?.maxOutputTokens ?? 0);
  if (usable <= 0) return null;
  return Math.min(100, (usable / max) * COMPACT_TRIGGER_PERCENT);
});

/** 阈值刻度线：沿环外侧画一道径向短线 */
const compactTriggerTick = computed(() => {
  const percent = compactTriggerPercent.value;
  if (percent === null) return null;
  const rad = (((percent / 100) * 360 - 90) * Math.PI) / 180;
  return {
    x1: 60 + Math.cos(rad) * 45,
    y1: 60 + Math.sin(rad) * 45,
    x2: 60 + Math.cos(rad) * 59,
    y2: 60 + Math.sin(rad) * 59,
  };
});

const compacting = ref(false);
const compactError = ref('');
const compactMessage = ref('');
const showCompactConfirm = ref(false);

/** 快照里的消息条数（后端判据的同一个量，可能偏小、不会偏大 —— 见 `canManuallyCompact`） */
const compactMessageCount = computed(() => props.snapshot?.messageCount ?? 0);

/**
 * 按钮能不能按 —— **判据是消息条数，与后端 `compact_inner()` 同源**。
 *
 * 不再是"token 占比 ≥ 70%"。旧写法把容量信号当成了功能闸门，导致按钮亮着
 * 点下去后端回"消息不足"。口径统一后按钮状态永远可信。
 */
const canCompact = computed(() => canManuallyCompact(props.snapshot?.messageCount));

/**
 * 要不要提示"建议压缩"（**只是提示，不参与按钮可用性**）。
 *
 * 窗口未知时不给建议：没有分母就没有诚实的占比，旧代码用一个 30k 绝对值兜底，
 * 那是旧闸门逻辑的拐杖，闸门搬走后一并去掉，避免再造一个没有依据的阈值。
 */
const suggestCompact = computed(() => {
  const percent = contextUsagePercent.value;
  return percent !== null && percent >= COMPACT_SUGGEST_PERCENT;
});

/** 徽标只在"该压"和"能压"同时成立时出现，避免"建议压缩"和"按钮不可用"打架 */
const showCompactSuggest = computed(() => suggestCompact.value && canCompact.value);

const compactHint = computed(() => {
  if (!props.snapshot) return t('monitor.context.compactDisabled');
  const count = compactMessageCount.value;
  if (!canCompact.value) {
    return t('monitor.context.compactHintTooFew', {
      count,
      min: COMPACT_MIN_MESSAGES,
    });
  }
  const percent = contextUsagePercent.value;
  if (percent !== null && percent >= COMPACT_SUGGEST_PERCENT) {
    return t('monitor.context.compactHintSuggested', { percent, count });
  }
  return t('monitor.context.compactHintReady', { count });
});

const triggerCompact = () => {
  if (!props.sessionId || compacting.value) return;
  showCompactConfirm.value = true;
};

let compactingQueryGeneration = 0;
let compactToastGeneration = 0;

const confirmCompact = async () => {
  showCompactConfirm.value = false;
  const compactSessionId = props.sessionId;
  if (!compactSessionId) return;
  // 压缩可能跨会话切换继续运行，后续结果都归属点击时捕获的会话。
  // 使切换会话前发出的状态查询失效，避免旧查询覆盖本次压缩状态。
  compactingQueryGeneration += 1;
  const toastGeneration = ++compactToastGeneration;
  compacting.value = true;
  compactError.value = '';
  compactMessage.value = '';
  try {
    const result = await invoke<string>('compact_conversation', { sessionId: compactSessionId });
    if (props.sessionId === compactSessionId) {
      compactMessage.value = result;
      setTimeout(() => {
        if (toastGeneration === compactToastGeneration) compactMessage.value = '';
      }, 4000);
    }
    tauriEmit('session-compacted', { sessionId: compactSessionId });
    emit('compacted', compactSessionId);
  } catch (err) {
    if (props.sessionId === compactSessionId) {
      compactError.value = String(err);
      setTimeout(() => {
        if (toastGeneration === compactToastGeneration) compactError.value = '';
      }, 6000);
    }
  } finally {
    if (props.sessionId === compactSessionId) {
      compacting.value = false;
    }
  }
};

// 挂载时查询后端，恢复 F5 刷新前的压缩状态
let compactingSessionQueryGeneration = 0;
watch(() => props.sessionId, async (sid) => {
  const generation = ++compactingSessionQueryGeneration;
  const queryGeneration = ++compactingQueryGeneration;
  compactToastGeneration += 1;
  compactMessage.value = '';
  compactError.value = '';
  if (!sid) {
    compacting.value = false;
    return;
  }
  try {
    const active = await invoke<boolean>('is_session_compacting', { sessionId: sid });
    if (
      generation === compactingSessionQueryGeneration
      && queryGeneration === compactingQueryGeneration
      && sid === props.sessionId
    ) {
      compacting.value = active;
    }
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

// ── 明细展开状态（受控手风琴）──
// 不再用 <details>：它收起时内容**仍挂在 DOM 里**（只是 display:none）。
// 取消后端截断后 messages 一节可达几十万字符，收起状态照样要付渲染/排版成本。
// 改成受控状态 + v-if：未展开的分区连 <pre> 节点都不存在。
const expandedKeys = ref<Record<string, boolean>>({});

const isExpanded = (key: string): boolean => expandedKeys.value[key] === true;

const toggleSection = (key: string) => {
  expandedKeys.value = { ...expandedKeys.value, [key]: !isExpanded(key) };
};

// ── 「原始 JSON」开关（按分区独立）──
// rawContent 只有 messages 有，且与可读文本内容不同；system 没有 raw，
// tools 的 content 与 raw 是同一份 pretty JSON —— 给它们一个永远无效的开关只是噪音。
const rawKeys = ref<Record<string, boolean>>({});

const isRawOpen = (key: string): boolean => rawKeys.value[key] === true;

const toggleRaw = (key: string) => {
  rawKeys.value = { ...rawKeys.value, [key]: !isRawOpen(key) };
};

/** 该分区的原始 JSON 是否与可读文本存在实际差异（有差异才配一个切换按钮）
 *
 * 算一次缓存在 computed 里，而不是每次渲染都 trim 两份大字符串：
 * messages 的 raw 是完整 JSON，动辄几十万字符，每次展开/收起都重新 trim 太浪费。
 */
const rawDiffKeys = computed(() => {
  const keys = new Set<string>();
  for (const section of props.snapshot?.sections ?? []) {
    const raw = section.rawContent;
    if (raw && raw.trim() !== (section.content ?? '').trim()) keys.add(section.key);
  }
  return keys;
});

const hasRawDiff = (section: ContextSectionSnapshot): boolean => rawDiffKeys.value.has(section.key);

/**
 * 展开后实际渲染的文本。
 *
 * 默认就是后端给的 `content`（完整可读文本）；只有"已展开 + raw 开关打开 +
 * 确实有差异"三条同时成立才换成原始 JSON，避免为一个不存在的差异渲染两份大字符串。
 */
const displayContent = (section: ContextSectionSnapshot): string => {
  if (isExpanded(section.key) && isRawOpen(section.key) && hasRawDiff(section)) {
    return section.rawContent as string;
  }
  return sectionContent(section);
};

const copySectionContent = async (section: ContextSectionSnapshot) => {
  // 复制取的是"最完整的那份数据"，不跟随界面的 raw/格式化开关：
  // 有 rawContent 就用它，没有（system / tools）则回退可读文本。
  const text = section.rawContent || sectionContent(section);
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
        <div class="context-ring-wrap">
          <svg class="context-ring" viewBox="0 0 120 120" role="img" :aria-label="usageLabel">
            <circle class="ring-track" cx="60" cy="60" :r="RING_RADIUS" />
            <circle
              v-if="contextUsagePercent !== null"
              class="ring-used"
              cx="60"
              cy="60"
              :r="RING_RADIUS"
              :stroke-dasharray="`${ringUsedLength} ${RING_CIRCUMFERENCE}`"
              transform="rotate(-90 60 60)"
            />
            <!-- 自动压缩触发点：按 (窗口−输出预算)×85% 换算，不是窗口的 85% -->
            <line
              v-if="compactTriggerTick"
              class="ring-tick"
              :x1="compactTriggerTick.x1"
              :y1="compactTriggerTick.y1"
              :x2="compactTriggerTick.x2"
              :y2="compactTriggerTick.y2"
            />
          </svg>
          <div class="ring-center">
            <strong>{{ contextUsagePercent !== null ? `${contextUsagePercent}%` : '—' }}</strong>
            <span>{{ contextMeasured ? '' : '≈ ' }}{{ formatToken(totalTokens) }}</span>
          </div>
        </div>

        <div class="context-hero-info">
          <div class="context-kicker">
            <!-- 分区标题由外层 AgentPanel 的「上下文预算」承担，这里只保留来源标记 -->
            <span
              class="context-source-tag"
              :class="contextMeasured ? 'is-measured' : 'is-estimated'"
              :title="contextMeasured ? t('monitor.context.measuredHint') : t('monitor.context.estimatedHint')"
            >
              {{ contextMeasured ? t('monitor.context.measuredTag') : t('monitor.context.estimatedTag') }}
            </span>
          </div>
          <div class="context-budget-row">
            <span>{{ t('monitor.context.used') }}</span>
            <strong>{{ formatToken(totalTokens) }}</strong>
          </div>
          <div class="context-budget-row">
            <span>{{ t('monitor.context.remaining') }}</span>
            <strong>
              {{ remainingTokens !== null ? formatToken(remainingTokens) : t('monitor.context.unknownContextWindow') }}
            </strong>
          </div>
          <div class="context-budget-row">
            <span>{{ t('monitor.context.outputBudget') }}</span>
            <strong>{{ formatToken(snapshot.maxOutputTokens) }}</strong>
          </div>
          <div class="context-subtitle">
            {{ t('monitor.context.updated', { time: formatTime(snapshot.createdAt) }) }}
          </div>
        </div>
        <div class="context-hero-actions">
          <span
            v-if="sessionId && showCompactSuggest"
            class="compact-suggest-badge"
            :title="compactHint"
          >{{ t('monitor.context.compactSuggestBadge') }}</span>
          <!-- 按钮在会话早期（≤ 阈值条数）会长期灰着：必须把原因摆在旁边，
               否则又是一个"看起来坏了"。 -->
          <span
            v-else-if="sessionId && !canCompact && !compacting"
            class="compact-blocked-note"
            :title="compactHint"
          >{{ t('monitor.context.compactBlockedNote', { min: COMPACT_MIN_MESSAGES }) }}</span>
          <button
            v-if="sessionId"
            class="compact-btn"
            :class="{
              'compact-ready': canCompact,
              'is-compacting': compacting,
              'is-blocked': !canCompact && !compacting,
            }"
            :disabled="compacting || !canCompact"
            @click="triggerCompact"
            :title="compacting ? t('monitor.context.compactingTitle') : compactHint"
          >
            <svg v-if="compacting" class="compact-spinner" viewBox="0 0 24 24" width="13" height="13" fill="none" stroke="currentColor" stroke-width="2.5" stroke-linecap="round">
              <circle cx="12" cy="12" r="10" stroke-opacity="0.2" />
              <path d="M12 2a10 10 0 0 1 10 10" />
            </svg>
            <span v-else class="compact-btn-icon">&#9881;</span>
            <span class="compact-btn-text">{{ compacting ? t('monitor.context.compacting') : t('monitor.context.compactAction') }}</span>
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
        <!-- 窗口 / 占用两张卡在"窗口未登记"时直接不渲染：此时主数卡已经写了「未知上下文窗口」，
             再铺两张值为"未知"的卡只是白占格子。有值才占位，格子留给真正有信息的指标 -->
        <div v-if="maxContextTokens" class="context-stat-card">
          <span>{{ t('monitor.context.contextWindow') }}</span>
          <strong>{{ formatNumber(maxContextTokens) }}</strong>
        </div>
        <div class="context-stat-card">
          <span>{{ t('monitor.context.providerActual') }}</span>
          <strong>{{ providerTotalTokens !== null ? formatToken(providerTotalTokens) : t('monitor.context.waitingUsage') }}</strong>
        </div>
        <div class="context-stat-card" :class="`cache-${cacheState}`">
          <span>{{ t('monitor.context.cacheHitCumulative') }}</span>
          <strong :title="cacheTitle">{{ cacheRateLabel }}</strong>
        </div>
      </div>
      <div v-if="cacheState !== 'hit'" class="context-cache-hint" :class="`cache-${cacheState}`">
        {{ cacheState === 'unknown'
          ? t('monitor.context.cacheUnknownHint')
          : t('monitor.context.cacheWarmup') }}
      </div>

      <div v-if="cacheTrendAnomaly" class="context-cache-trend">
        <div class="trend-head">
          <span>{{ t('monitor.context.cacheTrend') }}</span>
          <strong>{{ cacheTrendSummary }}</strong>
        </div>
        <div class="trend-bars">
          <div
            v-for="(point, index) in cacheTrend"
            :key="point.loopCount"
            class="trend-slot"
            :class="`cache-${pointTone(point)}`"
            :title="pointTitle(point)"
          >
            <span class="trend-fill" :style="{ height: `${Math.max(pointRate(point), 4)}%` }" />
            <!-- 柱内不再标数字：7+ 根柱子各塞一个数字是纯噪音，精确值放 title（悬停可见） -->
            <!-- 轮次只标首尾：10 根全标会挤成一团，精确轮次在 title 里 -->
            <span v-if="index === 0 || index === cacheTrend.length - 1" class="trend-loop">
              L{{ point.loopCount }}
            </span>
          </div>
        </div>
        <!-- 模块只在异常时出现，必须附带"为什么出现"，否则用户不知道这图突然冒出来是什么意思 -->
        <div class="trend-anomaly-note">{{ t('monitor.context.cacheTrendAnomalyNote') }}</div>
      </div>
    </div>

    <div class="context-section-list">
      <!-- 最大来源：原来是甜甜圈图，但"4 来源"信息量太低、占比明细行里已有占比条，
           收成明细区上方一行小字即可 -->
      <div v-if="dominantSection" class="context-dominant-line">
        {{ t('monitor.context.dominantSource') }} · <strong>{{ dominantSection.label }}</strong>
        <span>{{ t('monitor.context.dominantShare', { tokens: formatToken(dominantSection.estimatedTokens), percent: dominantSection.percent }) }}</span>
      </div>

      <!-- 分区明细只能靠本地分词器估算：厂商 API 只回总量，不回"消息 / 工具 / 系统提示词各占多少"。
           所以这里必须显式说明，避免用户拿分区之和去核对上面的实测总量。 -->
      <div class="context-estimate-note">{{ t('monitor.context.sectionsEstimatedHint') }}</div>

      <div class="context-detail-header">
        <span class="context-detail-label">{{ t('monitor.context.details') }}</span>
      </div>
      <!-- 受控手风琴：展开态由 expandedKeys 记录，内容用 v-if 挂载。
           这里不能用 <details>：它收起时内容仍留在 DOM（只是不可见），
           取消后端截断后单节可达几十万字符，收起照样要付渲染成本。 -->
      <div
        v-for="section in sectionViews"
        :key="section.key"
        class="context-section-item"
      >
        <!-- summary 的替身：div + role=button，保留原版式（标题行 + 占比条两行） -->
        <div
          class="context-section-summary"
          :class="{ expanded: isExpanded(section.key) }"
          role="button"
          tabindex="0"
          :aria-expanded="isExpanded(section.key)"
          @click="toggleSection(section.key)"
          @keydown.enter.prevent="toggleSection(section.key)"
          @keydown.space.prevent="toggleSection(section.key)"
        >
          <span class="context-section-head">
            <span class="context-chevron" :class="{ expanded: isExpanded(section.key) }" aria-hidden="true"></span>
            <span class="context-dot" :style="{ background: section.color }"></span>
            <span class="context-section-title">{{ section.label }}</span>
            <span class="context-section-count">{{ section.itemCount }}</span>
            <!-- 图片块数只挂在 messages 行：图片块本来就是 messages 内容的一部分，
                 它们不再单独成一个分区（否则会把同一份内容重复计入总量） -->
            <span
              v-if="section.key === 'messages' && imageCount > 0"
              class="context-section-images"
            >{{ t('monitor.context.imageCountNote', { count: imageCount }) }}</span>
            <span class="context-section-stat">
              {{ formatToken(section.estimatedTokens) }} · {{ Math.round(section.percent) }}%
            </span>
          </span>
          <!-- 占比条放进 summary 内：分区占比是主信息，收起状态下也必须看得见；
               原来它只在已删掉的 .context-bars 里画一遍，导致同一份数据渲染两次 -->
          <span class="context-bar-track" aria-hidden="true">
            <span :style="{ width: Math.max(3, section.percent) + '%', background: section.color }"></span>
          </span>
        </div>

        <!-- 以下内容只在展开态挂载 -->
        <template v-if="isExpanded(section.key)">
          <div class="context-section-extra">
            <span>{{ t('monitor.context.sectionDetailMeta', { chars: formatFullNumber(section.chars), method: methodLabel(section.tokenCountMethod) }) }}</span>
          </div>
          <div class="context-section-body">
            <div class="context-section-actions">
              <!-- 只在“原始 JSON 与可读文本确实不同”时给开关（目前只有 messages） -->
              <button
                v-if="hasRawDiff(section)"
                class="raw-toggle-btn"
                :class="{ active: isRawOpen(section.key) }"
                @click="toggleRaw(section.key)"
                :title="isRawOpen(section.key) ? t('monitor.context.contentModeFormattedTitle') : t('monitor.context.contentModeRawTitle')"
              >
                {{ isRawOpen(section.key) ? t('monitor.context.contentModeFormatted') : t('monitor.context.contentModeRaw') }}
              </button>
              <button
                class="copy-btn"
                :class="{ copied: copiedSection === section.key }"
                @click="copySectionContent(section)"
              >
                {{ copiedSection === section.key ? t('common.copied') : t('common.copy') }}
              </button>
            </div>
            <pre>{{ displayContent(section) }}</pre>
          </div>
        </template>
      </div>
    </div>
  </div>

  <div v-else class="section-empty">{{ t('monitor.context.emptySnapshot') }}</div>

  <ConfirmModal
    :open="showCompactConfirm"
    :title="t('monitor.context.compact')"
    :message="t('monitor.context.cacheWarning', { tokens: formatToken(totalTokens) })"
    confirm-kind="primary"
    @cancel="showCompactConfirm = false"
    @confirm="confirmCompact"
  />
</template>

<style scoped>
.context-inspector {
  display: flex;
  flex-direction: column;
  /* 卡片去除后，分区之间靠更大的留白分组（原 12px 是给描边卡片用的间距） */
  gap: 22px;
  min-width: 0;
}

/* 两层网格都改成单列纵向流。
   原因：面板内容宽只有 ~578px，先切两列、右列里再切两列，最终每格只剩 ~110px，
   装不下"标签 + 值"这一行 —— 只能把字号压到 0.6rem 以下、让文字换行挤压。
   改单列后宽度还给内容，字号也才有条件回到可读区间。 */
.context-top-grid {
  display: flex;
  flex-direction: column;
  gap: 12px;
}

.section-empty {
  padding: 10px 2px;
  color: var(--text-muted);
  font-size: 0.72rem;
}

/* === 扁平化（与设置页同一口径）===
   监控面板主角是数字，不是卡片：去掉描边/底色/阴影的"卡片套卡片"，
   分区靠留白与发丝线分隔，状态色只出现在"值"上（见下方 cache-* 规则）。 */

.context-hero {
  position: relative;
  display: flex;
  align-items: center;
  justify-content: space-between;
  gap: 12px;
  padding: 2px 2px 0;
}

/* 占用偏高/告急时hero区淡染色：这是状态信号，保留但压到很弱 */
.context-hero.tone-warning {
  background: color-mix(in srgb, var(--accent-blue) 4%, transparent);
}

.context-hero.tone-critical {
  background: color-mix(in srgb, var(--accent-red) 5%, transparent);
}

.context-kicker,
.context-subtitle,
.context-stat-card span,
.context-chart-copy span,
.context-chart-copy p,
.context-section-extra,
.context-section-stat {
  color: var(--text-muted);
  font-size: 0.78rem;
}

.context-kicker {
  text-transform: uppercase;
  letter-spacing: 0.08em;
  font-weight: var(--fw-heavy);
}

/* 数据来源标记：实测 / 本地估算。两者可信度差一个量级，必须长得不一样，
   否则用户会把兜底估算当成实测值。 */
.context-source-tag {
  display: inline-block;
  margin-left: 6px;
  padding: 0 5px;
  border-radius: 5px;
  font-size: 0.66rem;
  letter-spacing: 0;
  text-transform: none;
  vertical-align: 1px;
}

.context-source-tag.is-measured {
  color: var(--accent-blue);
  background: color-mix(in srgb, var(--accent-blue) 14%, transparent);
}

.context-source-tag.is-estimated {
  color: var(--text-muted);
  background: color-mix(in srgb, var(--text-muted) 16%, transparent);
}

.context-token-value {
  margin-top: 4px;
  color: var(--text-main);
  font-size: 1.38rem;
  font-weight: var(--fw-heavy);
  font-variant-numeric: tabular-nums;
}

.context-subtitle {
  margin-top: 3px;
  font-variant-numeric: tabular-nums;
}

/* ── 上下文环形图 ── */
.context-ring-wrap {
  position: relative;
  width: 104px;
  height: 104px;
  flex-shrink: 0;
}
.context-ring {
  width: 104px;
  height: 104px;
  display: block;
}
.ring-track {
  fill: none;
  stroke: var(--glass-border);
  stroke-width: 9;
}
.ring-used {
  fill: none;
  stroke: var(--accent-blue);
  stroke-width: 9;
  stroke-linecap: round;
  transition: stroke-dasharray 0.3s ease, stroke 0.3s ease;
}
.tone-warning .ring-used { stroke: #d99a2b; }
.tone-critical .ring-used { stroke: var(--accent-red); }
/* 自动压缩触发点：比进度弧更细的径向刻度，不抢主信息 */
.ring-tick {
  stroke: var(--text-main);
  stroke-width: 2;
  opacity: 0.5;
}
.ring-center {
  position: absolute;
  inset: 0;
  display: flex;
  flex-direction: column;
  align-items: center;
  justify-content: center;
  gap: 1px;
  pointer-events: none;
}
.ring-center strong {
  color: var(--text-main);
  font-size: 1rem;
  font-weight: var(--fw-heavy);
  font-variant-numeric: tabular-nums;
}
.ring-center span {
  color: var(--text-muted);
  font-size: 0.62rem;
  font-variant-numeric: tabular-nums;
}

.context-hero-info {
  flex: 1;
  min-width: 0;
  display: flex;
  flex-direction: column;
  gap: 3px;
}
.context-budget-row {
  display: flex;
  align-items: baseline;
  justify-content: space-between;
  gap: 10px;
  font-size: 0.72rem;
  color: var(--text-muted);
}
.context-budget-row strong {
  color: var(--text-main);
  font-weight: var(--fw-heavy);
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
  font-size: 0.72rem;
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
/* 压缩中与"条数不够不可按"都会 disabled，但语义不同，光标与配色必须能分开 */
.compact-btn:disabled {
  opacity: 0.6;
  cursor: not-allowed;
}
.compact-btn:disabled:hover {
  color: var(--text-muted);
  border-color: var(--border-color);
}
.compact-btn.is-compacting:disabled,
.compact-btn.is-compacting:disabled:hover {
  color: var(--accent-blue);
  border-color: color-mix(in srgb, var(--accent-blue) 40%, transparent);
  cursor: wait;
}
.compact-btn.is-blocked:disabled {
  opacity: 0.5;
}
/* "建议压缩"徽标：容量信号，只在按钮可用时出现（否则会和不可按状态打架） */
.compact-suggest-badge {
  flex-shrink: 0;
  padding: 2px 7px;
  border-radius: 999px;
  color: var(--accent-blue);
  background: color-mix(in srgb, var(--accent-blue) 16%, transparent);
  font-size: 0.66rem;
  font-weight: var(--fw-heavy);
  white-space: nowrap;
}
.tone-critical .compact-suggest-badge {
  color: var(--accent-red);
  background: color-mix(in srgb, var(--accent-red) 18%, transparent);
}
/* 灰按钮旁边的原因说明：低调但可见，别让它抢走状态标签的注意力 */
.compact-blocked-note {
  flex-shrink: 0;
  color: var(--text-muted);
  font-size: 0.66rem;
  white-space: nowrap;
  cursor: help;
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
  font-size: 0.72rem;
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
  font-size: 0.72rem;
  font-weight: var(--fw-heavy);
}

.tone-warning .context-health {
  background: color-mix(in srgb, var(--accent-blue) 18%, transparent);
}

.tone-critical .context-health {
  background: color-mix(in srgb, var(--accent-red) 22%, transparent);
}

/* auto-fit + 最小格宽 104px：面板 ~578px 时五个指标正好一行排满，
   避免 3+2 两行右下角空洞；窗口拖窄时自动落行 */
.context-overview-grid {
  display: grid;
  grid-template-columns: repeat(auto-fit, minmax(104px, 1fr));
  /* 统计带：上下发丝线夹出一条"指标区"，格子本身无卡片 */
  border-top: 1px solid var(--glass-border-subtle);
  border-bottom: 1px solid var(--glass-border-subtle);
}

.context-stat-card {
  min-width: 0;
  padding: 10px 12px 10px 2px;
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
  font-size: 0.78rem;
  font-weight: var(--fw-heavy);
  text-overflow: ellipsis;
  white-space: nowrap;
  font-variant-numeric: tabular-nums;
}

/* 缓存命中：状态色只落在数值上（命中=绿、预热=蓝、未知=灰），
   不再整卡染色 —— 绿色是"状态健康"的语义色，不是装饰 */
.context-stat-card.cache-hit strong {
  color: var(--accent-green);
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
  font-size: 0.72rem;
  line-height: 1.5;
}

.context-cache-hint.cache-warmup {
  border-color: color-mix(in srgb, var(--accent-blue) 30%, var(--border-color));
  color: color-mix(in srgb, var(--accent-blue) 75%, var(--text-main));
}

.context-cache-trend {
  margin-top: 8px;
  padding: 0 2px;
}

/* 异常说明：跟随警示语义色 */
.trend-anomaly-note {
  margin-top: 4px;
  color: var(--text-warning);
  font-size: 0.72rem;
  line-height: 1.5;
}

.trend-head {
  display: flex;
  align-items: baseline;
  justify-content: space-between;
  gap: 8px;
  margin-bottom: 7px;
  color: var(--text-muted);
  font-size: 0.72rem;
}

.trend-head strong {
  color: var(--text-main);
  font-size: 0.72rem;
  font-variant-numeric: tabular-nums;
}

/* 高度 46px 太矮：命中率 81%~99% 在这一高度里只差 8px，等于看不出差别 ——
   画了一根"看起来是量化图、实际读不出量"的柱子，比不画更糟。
   柱区 64px（含 16px 轮次标留白），再补一条 50% 基线做参照 */
.trend-bars {
  display: flex;
  align-items: flex-end;
  gap: 4px;
  height: 64px;
  padding-bottom: 16px;
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
  cursor: default;
}

/* 50% 基线：没有参照线时 95% 和 81% 长得一模一样，柱子等于白画 */
.trend-slot::after {
  content: '';
  position: absolute;
  inset: auto 0 50% 0;
  border-top: 1px dashed color-mix(in srgb, var(--text-muted) 45%, transparent);
  pointer-events: none;
}

/* 柱体瘦身 + 中性化：命中率趋势是次要指标，不该是全页最吵的元素。
   默认灰蓝细柱；只有"命中率异常偏低（<50%）"才上警示色 —— 颜色即语义。 */
.trend-fill {
  display: block;
  width: 55%;
  margin-inline: auto;
  border-radius: 3px;
  background: color-mix(in srgb, var(--text-muted) 40%, transparent);
  transition: height 0.25s ease;
}

.trend-slot.cache-hit .trend-fill {
  background: color-mix(in srgb, var(--accent-blue) 45%, var(--text-muted) 40%);
}

.trend-slot.cache-partial .trend-fill {
  background: var(--text-warning);
}

/* 预热态（0 命中）用斜纹：它和"命中了但很低"是两件事，不能长得一样。
   原来的 Math.max(…,3) 让它变成一根很矮的柱子，看着像"命中率很低"而不是"还没开始命中" */
.trend-slot.cache-warmup .trend-fill {
  background: repeating-linear-gradient(
    -45deg,
    color-mix(in srgb, var(--text-muted) 55%, transparent) 0 3px,
    transparent 3px 6px
  );
}

/* 轮次标：画在槽下方的留白里（.trend-bars 的 padding-bottom 就是为此留的）。
   slot 去掉了 overflow:hidden，否则它会被裁掉 */
.trend-loop {
  position: absolute;
  inset: auto 0 -15px 0;
  text-align: center;
  color: var(--text-muted);
  font-size: 0.72rem;
  font-weight: 700;
  font-variant-numeric: tabular-nums;
  pointer-events: none;
}

/* 最大来源一行：替代原甜甜圈模块 */
.context-dominant-line {
  padding: 0 2px;
  color: var(--text-muted);
  font-size: 0.72rem;
  line-height: 1.5;
}

.context-dominant-line strong {
  color: var(--text-main);
  font-weight: var(--fw-bold);
}

.context-dominant-line span {
  margin-left: 4px;
  font-variant-numeric: tabular-nums;
}

.context-estimate-note {
  margin: 4px 0 2px;
  padding: 0 2px;
  color: var(--text-muted);
  font-size: 0.72rem;
  line-height: 1.45;
}

.context-section-head,
.context-section-extra {
  display: flex;
  align-items: center;
  gap: 6px;
  min-width: 0;
}

.context-dot {
  width: 7px;
  height: 7px;
  flex-shrink: 0;
  border-radius: 999px;
  box-shadow: 0 0 12px currentColor;
}

.context-section-title {
  min-width: 0;
  overflow: hidden;
  color: var(--text-main);
  font-size: 0.78rem;
  font-weight: var(--fw-bold);
  text-overflow: ellipsis;
  white-space: nowrap;
}

/* 占比条：现在挂在 summary 里（有 gap 撑开），不再需要自己的 margin-top */
.context-bar-track {
  display: block;
  width: 100%;
  height: 6px;
  overflow: hidden;
  border-radius: 999px;
  background: color-mix(in srgb, var(--text-muted) 12%, transparent);
}

.context-bar-track span {
  display: block;
  height: 100%;
  border-radius: inherit;
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
  color: var(--text-soft);
  font-size: 0.66rem;
  font-weight: var(--fw-heavy);
  letter-spacing: 0.08em;
  text-transform: uppercase;
}

.raw-toggle-btn {
  padding: 2px 8px;
  border: 1px solid var(--border-color);
  border-radius: 4px;
  background: var(--glass-bg);
  color: var(--text-muted);
  font-size: 0.66rem;
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

.context-section-list {
  display: flex;
  flex-direction: column;
}

/* 明细分区：去卡片，行与行之间发丝线分隔（同设置页口径） */
.context-section-item {
  border-bottom: 1px solid var(--glass-border-subtle);
}

.context-section-item:last-child {
  border-bottom: none;
}

/* summary 的替身（div + role=button）：版式与原 summary 完全一致 ——
   纵向两行，第一行"名称 + 项数 + token + 占比"，第二行占比条。
   占比条必须留在这一层：收起状态下它是最该看见的信息。 */
.context-section-summary {
  display: flex;
  flex-direction: column;
  gap: 6px;
  min-height: 40px;
  padding: 10px 2px;
  cursor: pointer;
  user-select: none;
  border-radius: 4px;
}

.context-section-summary:hover {
  background: var(--glass-bg-light);
}

.context-section-summary:focus-visible {
  outline: 1px solid var(--accent-blue);
  outline-offset: -1px;
}

/* 展开态指示：纯 CSS 三角（不引图标、不用 emoji），展开时转 90 度 */
.context-chevron {
  flex-shrink: 0;
  width: 0;
  height: 0;
  border-left: 4px solid color-mix(in srgb, var(--text-muted) 80%, transparent);
  border-top: 3px solid transparent;
  border-bottom: 3px solid transparent;
  transition: transform 0.15s ease;
}

.context-chevron.expanded {
  transform: rotate(90deg);
}

/* 图片块数：纯计数，不参与 token 合计，用弱色小字附在 messages 行 */
.context-section-images {
  flex-shrink: 0;
  color: var(--text-muted);
  font-size: 0.66rem;
  white-space: nowrap;
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
  font-size: 0.66rem;
  font-weight: var(--fw-bold);
  font-variant-numeric: tabular-nums;
}

.context-section-stat {
  flex-shrink: 0;
  margin-left: auto;
  white-space: nowrap;
  /* 监控页数字是主角：右侧读数加重加大，标签反而弱化 */
  color: var(--text-main);
  font-size: 0.78rem;
  font-weight: var(--fw-heavy);
  font-variant-numeric: tabular-nums;
}

.context-section-extra {
  justify-content: space-between;
  padding: 6px 8px 0;
  font-variant-numeric: tabular-nums;
}

.context-section-body {
  display: flex;
  flex-direction: column;
  gap: 4px;
}

/* 操作行从"绝对定位浮在 pre 右上角"改成正常流：
   原写法 .context-section-actions 与 .copy-btn 都写了 position:absolute，
   两层偏移叠加、位置对不上；加进 raw 开关后会直接盖住正文首行。 */
.context-section-actions {
  display: flex;
  justify-content: flex-end;
  gap: 4px;
  padding: 2px 8px 0;
}

.context-section-item pre {
  /* max-height + overflow：取消截断后单节可达几十万字符，
     不限高会把整个面板铺开成一面墙，也拖慢布局。 */
  max-height: 60vh;
  margin: 0 8px 8px;
  padding: 8px;
  overflow: auto;
  color: var(--text-main);
  border: 1px solid var(--border-color);
  border-radius: 6px;
  background: color-mix(in srgb, var(--bg-dark) 78%, var(--surface-strong));
  font-family: var(--font-mono);
  /* 这里显示的是被选中的那段上下文原文，属代码内容 */
  font-size: var(--code-font-size);
  line-height: 1.45;
  white-space: pre-wrap;
  word-break: break-word;
}

.copy-btn {
  padding: 2px 6px;
  border: 1px solid var(--border-color);
  border-radius: 4px;
  background: var(--glass-bg);
  color: var(--text-muted);
  font-size: 0.66rem;
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

/* 这里不再需要断点：两个容器已改成单列纵向流，`.context-overview-grid` 用
   auto-fit 自己决定落几列。
   原写法是 `@media (max-width: 560px)` —— 它挂在**视口**上，而面板宽度与视口无关：
   独立监控窗口宽 640 > 560，永不触发；内嵌主窗口时视口 1600、面板只有 620，更不可能触发。
   现在改用 auto-fit，由布局自己决定列数，不需要任何断点。 */
</style>
