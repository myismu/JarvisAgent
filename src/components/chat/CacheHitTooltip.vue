<script setup lang="ts">
/**
 * 输入栏 token 读数的悬浮面板。
 *
 * 为什么不用原生 `title`：
 * - 样式跟随系统，和毛玻璃 UI 不搭；
 * - 约 1 秒延迟才出现；
 * - 单行不换行，内容一长就横跨半个窗口（`字段 prompt_cache_hit_tokens` 那串尤其明显）；
 * - 放不下进度条这类富文本。
 *
 * 内容分两组，**刻意用分组把两个口径隔开**：
 * - 「会话累计」——整个会话累加的缓存命中率（大字号 + 进度条）与输入 / 输出 / 合计 / 未命中；
 * - 「本次请求」——**最近一次请求**的缓存命中（命中 token / 输入 token 总量），降为次要小字。
 *
 * **为什么大字号给的是会话累计而不是单次**：字号与进度条本身就在暗示"这是主指标"，
 * 而单次口径恰是最弱的一个——多 loop 的回合里它只反映**最后一个 loop**，
 * 也就是缓存最暖、数字最好看的那次，系统性偏乐观。把最弱的数放在最显眼处，
 * 不细看就会把它当成整个会话的水平。所以主位给累计，单次退到下面当参考。
 *
 * 组标题曾经写作「本轮」，**那是错的**：快照的缓存字段由
 * `repository::update_context_snapshot_usage` 覆盖写入（不是累加），
 * 所以它描述的是最近一次请求。单 loop 的回合里两者等价，多 loop 的回合里
 * 旧标题会让人以为看到的是整轮的合计。
 *
 * 概览栏（`TerminalInput.vue`）没有分组标题，所以那边把限定词写进了标签本身
 * （「本次上下文」/「累计命中」），与本组件两个组标题一一对应。
 *
 * 「会话累计」里的缓存行是**解读「合计」的必要前提**：`合计 ≈ 输入`，
 * 而输入是每次请求整份上下文重复累加的结果（agent loop 每轮重发全量上下文），
 * 单看「合计 1.8M」会被读成"干了 1.8M token 的活"。配上命中率才知道
 * 那 1.8M 里 97% 走的是缓存价，真正全价计费的输入只有几十 k。
 *
 * 数据来源：后端 `sessions.total_cache_hit_tokens` / `total_cache_miss_tokens`，
 * 由 `pipeline.rs` **逐请求累加**、`session-usage-updated` 事件推送
 * （不再等回合收尾，否则长回合里这一栏会一直停在上一轮的值）。
 *
 * 配色与右侧上下文面板（`ContextInspector.vue`）保持同一套语义：
 * **命中=绿、预热=蓝、未知=灰**——两侧不一致会让人误读状态。
 * 注意根节点的状态类现在跟随**会话累计**（大字号那个数），
 * 单次读数另用自己的 `is-*` 类上色，两者不再共用一个状态。
 *
 * 定位由父组件（`TerminalInput.vue`）负责，本组件只负责长什么样。
 */
import { computed } from "vue";
import { useI18n } from "vue-i18n";

const props = defineProps<{
  /** null 表示当前还没有上下文快照，此时整块「本次请求」分组不渲染 */
  state: "hit" | "warmup" | "unknown" | null;
  percent: number | null;
  hitTokens: number | null;
  totalTokens: number | null;
  sessionInputTokens: number;
  sessionOutputTokens: number;
  sessionTotalTokens: number;
  /** 会话累计缓存命中 token。与 miss 同为 0 表示整个会话都没上报过缓存字段 */
  sessionCacheHitTokens: number;
  sessionCacheMissTokens: number;
}>();

const { t } = useI18n();

const formatToken = (n: number): string => {
  if (n >= 1_000_000) return `${(n / 1_000_000).toFixed(1)}M`;
  if (n >= 1_000) return `${(n / 1_000).toFixed(1)}k`;
  return `${n}`;
};

const hasCache = computed(() => props.state !== null);

const title = computed(() => {
  if (props.state === "hit") return t("input.cacheLabel");
  if (props.state === "warmup") return t("input.cacheWarmup");
  return t("input.cacheNotReported");
});

const percentText = computed(() => (props.percent === null ? "" : `${props.percent}%`));

const detail = computed(() => {
  if (props.totalTokens === null) return "";
  return t("input.cacheTokensDetail", {
    hit: formatToken(props.hitTokens ?? 0),
    total: formatToken(props.totalTokens),
  });
});

/**
 * 整个会话是否有任何请求上报过缓存字段。
 *
 * 判据是 `hit + miss > 0` 而不是"某一列非 0"：后端对未上报端点会把两列都留在 0
 * （`resolve_cache_outcome` 返回 None → 按 0 累加），所以两列全 0 恰好等价于"没数据"。
 */
const hasSessionCache = computed(
  () => props.sessionCacheHitTokens + props.sessionCacheMissTokens > 0,
);

/** 无数据时显示 `--`。绝不回退成 `0%`——"厂商没报"和"一次都没命中"是两回事 */
const sessionHitPercentText = computed(() => {
  const total = props.sessionCacheHitTokens + props.sessionCacheMissTokens;
  if (total <= 0) return "--";
  // 有上报但命中为 0：这才是真正的 0%，可以放心显示
  if (props.sessionCacheHitTokens <= 0) return "0%";
  const pct = (props.sessionCacheHitTokens / total) * 100;
  // 有命中却不足 1% 时写 `<1%`：直接四舍五入成 `0%` 会和"完全没命中"混淆
  return pct < 1 ? "<1%" : `${Math.round(pct)}%`;
});

/**
 * 根节点的状态类现在跟随**会话累计**，而不是单次请求。
 *
 * 因为大字号 + 进度条已经改成展示累计值了，若状态色仍跟单次走，
 * 会出现"数字是累计的、颜色是单次的"这种自相矛盾的组合
 * （典型场景：本次请求预热中 → 蓝色，但累计其实早已命中 90%）。
 */
const sessionState = computed<"hit" | "warmup" | "unknown">(() => {
  if (!hasSessionCache.value) return "unknown";
  return props.sessionCacheHitTokens <= 0 ? "warmup" : "hit";
});

/** 命中为 0 时也留 2% 的可见宽度，否则进度条看起来像渲染失败 */
const sessionFillPercent = computed(() => {
  if (!hasSessionCache.value) return 0;
  const total = props.sessionCacheHitTokens + props.sessionCacheMissTokens;
  return Math.max((props.sessionCacheHitTokens / total) * 100, 2);
});

/** 会话累计口径的命中/总量明细，给大字号那个百分比做分母交代 */
const sessionDetail = computed(() => {
  if (!hasSessionCache.value) return "";
  return t("input.cacheTokensDetail", {
    hit: formatToken(props.sessionCacheHitTokens),
    total: formatToken(props.sessionCacheHitTokens + props.sessionCacheMissTokens),
  });
});

const sessionMissText = computed(() =>
  hasSessionCache.value ? formatToken(props.sessionCacheMissTokens) : "--",
);
</script>

<template>
  <div class="cache-tip" :class="`cache-${sessionState}`" role="tooltip">
    <div class="cache-tip-group">{{ t('input.cacheGroupSession') }}</div>

    <div class="cache-tip-head">
      <span class="cache-tip-title">{{ t('input.tokenSessionHitRate') }}</span>
      <span class="cache-tip-percent">{{ sessionHitPercentText }}</span>
    </div>

    <div v-if="hasSessionCache" class="cache-tip-track">
      <span class="cache-tip-fill" :style="{ width: `${sessionFillPercent}%` }" />
    </div>

    <div v-if="sessionDetail" class="cache-tip-detail">{{ sessionDetail }}</div>
    <div v-else class="cache-tip-hint">{{ t('input.tokenSessionCacheUnknownTitle') }}</div>

    <div class="cache-tip-row">
      <span>{{ t('input.tokenInput') }}</span>
      <span class="cache-tip-num">{{ formatToken(sessionInputTokens) }}</span>
    </div>
    <div class="cache-tip-row">
      <span>{{ t('input.tokenOutput') }}</span>
      <span class="cache-tip-num">{{ formatToken(sessionOutputTokens) }}</span>
    </div>
    <div class="cache-tip-row cache-tip-row-total">
      <span>{{ t('input.tokenTotal') }}</span>
      <span class="cache-tip-num">{{ formatToken(sessionTotalTokens) }}</span>
    </div>
    <div class="cache-tip-row">
      <span>{{ t('input.tokenSessionMiss') }}</span>
      <span
        class="cache-tip-num"
        :class="{ 'is-unknown': !hasSessionCache }"
        :title="hasSessionCache ? undefined : t('input.tokenSessionCacheUnknownTitle')"
      >{{ sessionMissText }}</span>
    </div>

    <template v-if="hasCache">
      <div class="cache-tip-divider" />

      <div class="cache-tip-group">{{ t('input.cacheGroupCurrent') }}</div>

      <div class="cache-tip-head">
        <span class="cache-tip-title">{{ title }}</span>
        <span class="cache-tip-num" :class="`is-${state}`">{{ percentText || '--' }}</span>
      </div>

      <div v-if="detail" class="cache-tip-detail">{{ detail }}</div>
      <div v-else class="cache-tip-hint">{{ t('input.cacheUnknownHint') }}</div>

      <div v-if="state === 'warmup'" class="cache-tip-hint">{{ t('input.cacheWarmupHint') }}</div>
    </template>
  </div>
</template>

<style scoped>
/* 毛玻璃配方与 .profile-menu 一致，避免浮层风格两套 */
.cache-tip {
  /* 定位：贴在读数标签正上方。左对齐到触发元素，236px 宽的浮层不会越过窗口左边界。 */
  position: absolute;
  bottom: calc(100% + 8px);
  left: 0;
  z-index: 120;
  width: 236px;
  padding: 10px 12px;
  background: var(--surface-strong);
  backdrop-filter: blur(var(--glass-blur-heavy));
  -webkit-backdrop-filter: blur(var(--glass-blur-heavy));
  border: 1px solid color-mix(in srgb, var(--text-muted) 22%, transparent);
  border-radius: var(--radius-lg);
  box-shadow: var(--shadow-lg);
  color: var(--text-muted);
  font-size: 0.65rem;
  line-height: 1.5;
  user-select: none;
  /* 只淡入：位移动画会和定位打架 */
  animation: cacheTipIn var(--transition-fast);
}

/* 透明桥接块，盖住浮层与触发元素之间的 8px 间隙。
   浮层是触发元素的 DOM 子元素，没有这段间隙时 hover 天然连续；
   补上它，指针往浮层方向挪一点（凑近看内容）不会中途触发 mouseleave。 */
.cache-tip::after {
  content: "";
  position: absolute;
  left: 0;
  right: 0;
  top: 100%;
  height: 8px;
}

@keyframes cacheTipIn {
  from { opacity: 0; }
  to { opacity: 1; }
}

/* 分组标题：「本次请求」/「会话累计」 */
.cache-tip-group {
  font-size: 0.6rem;
  letter-spacing: 0.02em;
  opacity: 0.6;
  margin-bottom: 2px;
}

.cache-tip-head {
  display: flex;
  align-items: baseline;
  justify-content: space-between;
  gap: 8px;
}

.cache-tip-title {
  font-weight: 500;
}

.cache-tip-percent {
  font-size: 0.85rem;
  font-weight: 650;
  font-variant-numeric: tabular-nums;
  color: var(--text-muted);
}

/* 状态色：与右侧面板同口径（命中=绿、预热=蓝、未知=灰）。
   根节点的状态类跟随**会话累计**，因为大字号那个数是累计口径的。 */
.cache-tip.cache-hit .cache-tip-percent {
  color: var(--accent-green);
}

.cache-tip.cache-warmup .cache-tip-percent {
  color: var(--accent-blue);
}

.cache-tip.cache-unknown .cache-tip-percent {
  color: var(--text-muted);
}

.cache-tip-track {
  position: relative;
  height: 4px;
  margin: 6px 0;
  border-radius: 2px;
  overflow: hidden;
  background: color-mix(in srgb, var(--text-muted) 20%, transparent);
}

.cache-tip-fill {
  display: block;
  height: 100%;
  border-radius: 2px;
  transition: width var(--transition-fast);
}

.cache-tip.cache-hit .cache-tip-fill {
  background: var(--accent-green);
}

/* 预热态：整条进度条只有 2% 的可见宽度（`sessionFillPercent` 的下限），
   不填色就完全看不见，会被当成渲染失败 */
.cache-tip.cache-warmup .cache-tip-fill {
  background: var(--accent-blue);
}

.cache-tip-detail {
  font-variant-numeric: tabular-nums;
}

.cache-tip-row {
  display: flex;
  align-items: baseline;
  justify-content: space-between;
  gap: 8px;
  font-variant-numeric: tabular-nums;
}

.cache-tip-row-total {
  font-weight: 500;
}

/* 「本次请求」组的读数：字号比大字号那个百分比小，颜色跟自己的状态走
   （大字号用的是根节点上的会话状态，两者不能共用一个类）。 */
.cache-tip-num.is-hit {
  font-weight: 600;
  color: var(--accent-green);
}

.cache-tip-num.is-warmup {
  font-weight: 600;
  color: var(--accent-blue);
}

/* 「未上报」态：命中率与未命中都显示 `--`。
   降透明度 + help 光标，让 `--` 读起来像"没数据"而不是"没命中"。
   选择器带两个类，才能盖过上面 .cache-tip-num.is-hit 的绿色。 */
.cache-tip-num.is-unknown {
  font-weight: 400;
  color: inherit;
  opacity: 0.55;
  cursor: help;
}

.cache-tip-hint {
  margin-top: 4px;
  opacity: 0.7;
  line-height: 1.45;
}

.cache-tip-divider {
  height: 1px;
  margin: 8px 0;
  background: color-mix(in srgb, var(--text-muted) 18%, transparent);
}
</style>
