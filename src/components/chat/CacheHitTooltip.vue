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
 * - 「会话累计」——整个会话累加的缓存命中率（大字号 + 进度条），并按
 *   **主对话 / 子 Agent / 系统开销**三层拆出输入 / 输出 / 合计；
 * - 「本次请求」——**最近一次请求**的缓存命中（命中 token / 输入 token 总量），降为次要小字。
 *
 * ## 会话累计为什么必须拆三层
 * 会话的累计输入是「主对话 + 子 Agent + 系统内部调用（标题生成、压缩摘要、
 * 反思审查、记忆整理）」混在一起的数。只看一个总量，会把子 Agent 或后台任务烧掉的
 * token 读成主对话的开销 —— 也就答不出「这些钱花在谁身上」。三层各自给输入 / 输出 / 合计，
 * 才能一眼看出是主线在花还是派生任务在花。
 *
 * 系统开销那几类（反思 / 压缩 / 命名 / 记忆整理）量级小且用户不可见，
 * 在表格里**合并成一行**，点开才看该层的请求次数与缓存命中 / 未命中。
 *
 * ## 命中率的分母口径
 * 分母**只含上报过缓存字段的请求**，即「命中 + 未命中」。厂商没上报缓存的请求
 * 既不进分子也不进分母（后端按 0 累加），所以它们单独列一行
 * 「未上报请求 N 次 · 输入 M token」——让「没数据」有量级可见，
 * 而不是被悄悄算成没命中（那会把命中率系统性压低）。
 *
 * `hasDetail === false` 时（后端没有明细行，典型是早于明细表的历史会话）
 * **整块三层不渲染**，只退回展示四个累计值：把缺失的明细画成三层 0
 * 会被读成「主对话一个 token 都没花」，比不显示更糟。
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
 * 三层与「未上报」两个字段来自同一事件的 `breakdown`（按来源分组的绝对快照，
 * 前端归一化由 `utils/usageBreakdown.ts` 负责），本组件只负责展示，不重新聚合。
 *
 * 配色与右侧上下文面板（`ContextInspector.vue`）保持同一套语义：
 * **命中=绿、预热=蓝、未知=灰**——两侧不一致会让人误读状态。
 * 注意根节点的状态类现在跟随**会话累计**（大字号那个数），
 * 单次读数另用自己的 `is-*` 类上色，两者不再共用一个状态。
 *
 * 定位由父组件（`TerminalInput.vue`）负责，本组件只负责长什么样。
 */
import { computed, ref } from "vue";
import { useI18n } from "vue-i18n";
import type { SessionUsageBreakdown, UsageSourceBreakdown } from "../../types";

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
  /**
   * 会话用量的按来源分组快照（主 / 子 Agent / 系统开销 + 未上报）。
   *
   * 必填而不是可选：父组件（`TerminalInput.vue`）任何时候都能从 session store
   * 拿到它，缺明细时是 `hasDetail = false` 的归零占位 —— 用占位而不是 undefined，
   * 本组件就只需要判一个标志，不必再区分「没传」和「传了空」。
   */
  breakdown: SessionUsageBreakdown;
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
 * 命中率的分母：**只含上报过缓存字段的请求**（命中 + 未命中）。
 *
 * 判据是 `hit + miss > 0` 而不是"某一列非 0"：后端对未上报端点会把两列都留在 0
 * （`resolve_cache_outcome` 返回 None → 按 0 累加），所以两列全 0 恰好等价于"没数据"。
 * 反之，`hit + miss` 天然就是「已上报请求的输入量」，未上报的既不在分子也不在分母 ——
 * 这个口径必须写进浮层文案（「命中+未命中」），否则没人能确定分母里到底含什么。
 */
const sessionCacheTotal = computed(
  () => props.sessionCacheHitTokens + props.sessionCacheMissTokens,
);

const hasSessionCache = computed(() => sessionCacheTotal.value > 0);

/** 无数据时显示 `--`。绝不回退成 `0%`——"厂商没报"和"一次都没命中"是两回事 */
const sessionHitPercentText = computed(() => {
  const total = sessionCacheTotal.value;
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
  return Math.max((props.sessionCacheHitTokens / sessionCacheTotal.value) * 100, 2);
});

/** 命中率的分母明细。文案里明写「命中+未命中」，不写含糊的「共 X 输入 token」 */
const sessionDetail = computed(() => {
  if (!hasSessionCache.value) return "";
  return t("input.cacheRateDetail", {
    hit: formatToken(props.sessionCacheHitTokens),
    total: formatToken(sessionCacheTotal.value),
  });
});

/** 无明细时的未命中读数；没有上报过缓存时显示 `--`，不编一个 0 */
const sessionMissText = computed(() =>
  hasSessionCache.value ? formatToken(props.sessionCacheMissTokens) : "--",
);

/** 一个可展示的分层：层名 + 该来源的累计用量 */
interface UsageLayer {
  key: "main" | "subagent" | "system";
  label: string;
  data: UsageSourceBreakdown;
}

/**
 * 会话累计的三层分组：主对话 / 子 Agent / 系统开销。
 *
 * 顺序即展示顺序，**主对话在前**：它永远是花钱最多、用户最关心的那层；
 * 子 Agent 次之；系统开销（反思 / 压缩 / 命名 / 记忆整理）量级最小，放最后。
 * 分组口径完全来自后端（`UsageSource` → 三个桶的折叠在
 * `utils/usageBreakdown.ts` 完成），本组件不重新聚合，也不做加法外推。
 */
const layers = computed<UsageLayer[]>(() => [
  { key: "main", label: t("input.layerMain"), data: props.breakdown.main },
  { key: "subagent", label: t("input.layerSubagent"), data: props.breakdown.subagent },
  { key: "system", label: t("input.layerSystem"), data: props.breakdown.system },
]);

/** 后端没有明细行时（`hasDetail = false`）不渲染三层，退回四个扁平累计值 */
const showLayers = computed(() => props.breakdown.hasDetail);

/** 系统开销行的展开开关（默认合并成一行，避免小数字占满浮层） */
const systemExpanded = ref(false);

/** 系统开销展开后的明细：请求次数 / 其中上报缓存的次数 / 命中与未命中 */
const systemDetailText = computed(() => {
  const layer = props.breakdown.system;
  return t("input.layerSystemDetail", {
    requests: layer.requestCount,
    reported: layer.cacheReported,
    hit: formatToken(layer.cacheHit),
    miss: formatToken(layer.cacheMiss),
  });
});

/**
 * 未上报缓存的请求（不在命中率分母里），单列一行让「不知道」也有量级。
 *
 * 为 0 时也照常显示：它正是分母可信的依据（所有请求都上报了缓存）。
 */
const unreportedText = computed(() =>
  t("input.layerUnreportedValue", {
    count: props.breakdown.unreportedRequestCount,
    input: formatToken(props.breakdown.unreportedInput),
  }),
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

    <!-- 三层分组：仅在后端给出明细时渲染（hasDetail=true） -->
    <template v-if="showLayers">
      <div class="cache-tip-layers">
        <div class="cache-tip-layer cache-tip-layer-head">
          <span>{{ t('input.layerColumnName') }}</span>
          <span>{{ t('input.tokenInput') }}</span>
          <span>{{ t('input.tokenOutput') }}</span>
          <span>{{ t('input.tokenTotal') }}</span>
        </div>
        <div v-for="layer in layers" :key="layer.key" class="cache-tip-layer">
          <span class="cache-tip-layer-name">{{ layer.label }}</span>
          <span>{{ formatToken(layer.data.input) }}</span>
          <span>{{ formatToken(layer.data.output) }}</span>
          <span>{{ formatToken(layer.data.input + layer.data.output) }}</span>
        </div>
      </div>

      <!-- 系统开销（反思 / 压缩 / 命名 / 记忆整理）合并成一行，展开看明细。
           点击必须 .stop：外层 .token-bar-usage 的点击会打开右侧监控窗口，
           展开明细不该顺带把面板弹出来。 -->
      <button type="button" class="cache-tip-more" @click.stop="systemExpanded = !systemExpanded">
        {{ systemExpanded ? t('input.layerSystemCollapse') : t('input.layerSystemExpand') }}
      </button>
      <div v-if="systemExpanded" class="cache-tip-detail cache-tip-layer-detail">
        {{ systemDetailText }}
      </div>

      <div class="cache-tip-row">
        <span>{{ t('input.layerUnreportedLabel') }}</span>
        <span class="cache-tip-num">{{ unreportedText }}</span>
      </div>
    </template>

    <!-- 无明细：只展示会话的四个累计值，三层一律不画 -->
    <template v-else>
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
    </template>

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
  /* 定位：贴在读数标签正上方。左对齐到触发元素，268px 宽的浮层不会越过窗口左边界。 */
  position: absolute;
  bottom: calc(100% + 8px);
  left: 0;
  z-index: 120;
  /* 比旧版（236px）宽一点：三层表格要同时装下「层名 + 输入 / 输出 / 合计」四列 */
  width: 268px;
  padding: 10px 12px;
  background: var(--surface-strong);
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
  opacity: 0.85;
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
  font-weight: var(--fw-semibold);
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

/* 三层分组表：层名 + 输入 / 输出 / 合计。
   用 grid 而不是 flex：表头与数据行必须跨行对齐，
   否则同一列的数字会因层名长度不同而错位。 */
.cache-tip-layers {
  display: flex;
  flex-direction: column;
  gap: 1px;
  margin-top: 6px;
  font-variant-numeric: tabular-nums;
}

.cache-tip-layer {
  display: grid;
  grid-template-columns: 1fr 44px 44px 44px;
  gap: 4px;
  text-align: right;
}

/* 表头只是列名（输入 / 输出 / 合计），缩一号字并降透明度，不与数据行抢视线 */
.cache-tip-layer-head {
  font-size: 0.6rem;
  opacity: 0.75;
}

.cache-tip-layer-name {
  text-align: left;
}

/* 「系统开销明细」展开按钮：做成无边框小字，
   否则浮层里会冒出一个像表单控件的方块，和整块读数风格不搭 */
.cache-tip-more {
  margin-top: 4px;
  padding: 0;
  border: none;
  background: none;
  color: inherit;
  font: inherit;
  opacity: 0.85;
  cursor: pointer;
  text-align: left;
}

.cache-tip-more:hover {
  opacity: 1;
  text-decoration: underline;
}

.cache-tip-layer-detail {
  margin-top: 2px;
  opacity: 0.85;
  line-height: 1.45;
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
  opacity: 0.85;
  cursor: help;
}

.cache-tip-hint {
  margin-top: 4px;
  opacity: 0.85;
  line-height: 1.45;
}

.cache-tip-divider {
  height: 1px;
  margin: 8px 0;
  background: color-mix(in srgb, var(--text-muted) 18%, transparent);
}
</style>
