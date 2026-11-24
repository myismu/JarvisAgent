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
 * 内容分两组，按口径隔开：
 * - 「会话累计」——整个会话累加的缓存命中率（大字号 + 进度条），以及按
 *   **主对话 / 子 Agent / 系统开销**三层拆出的 输入 / 命中 / 未命中 / 输出
 *   与跨层合计；
 * - 「本次请求」——**最近一次请求**的同一组四指标，降为次要小字。
 *
 * ## 为什么表格是「输入 / 命中 / 未命中 / 输出」四列
 * 这四列是照价目表算账所需的最小完备集：输入总量、其中走缓存价的命中量、
 * 按全价计费的未命中量、以及输出量。命中与未命中本来就是**输入的两个分部**
 * （上报口径下 `输入 = 命中 + 未命中`），把它们与输入拆到两个区块
 * （上一版：命中率区只给命中 / 未命中、来源表只给输入 / 输出）会让人来回对照，
 * 也算不出一份能直接乘单价的账。
 *
 * **不设「输入 + 输出」合计列**：输入与输出单价不同，相加后的数字既不是钱，
 * 也不是任何一个可计费量。真正需要的合计是**三层按列相加** ——
 * 主对话 + 子 Agent + 系统开销 的输入 / 命中 / 未命中 / 输出，即整场会话的
 * 四项总量。表格底部因此单独有一行「合计」，逐列等于上方三行之和。
 *
 * ## 会话累计为什么必须拆三层
 * 会话的累计输入是「主对话 + 子 Agent + 系统内部调用（标题生成、压缩摘要、
 * 反思审查、记忆整理）」混在一起的数。只看一个总量，会把子 Agent 或后台任务烧掉的
 * token 读成主对话的开销 —— 也就答不出「这些钱花在谁身上」。三层各自给四列，
 * 才能一眼看出是主线在花还是派生任务在花。
 *
 * 系统开销那几类（反思 / 压缩 / 命名 / 记忆整理）量级小且用户不可见，
 * 在表格里**合并成一行**，不再提供展开明细 —— 用户在算价时只需要这一行的
 * 输入 / 命中 / 未命中 / 输出，请求次数与来源细分属于审计信息，不属于这个浮层。
 *
 * ## 命中率的分母口径
 * 分母**只含上报过缓存字段的请求**，即「命中 + 未命中」。厂商没上报缓存的请求
 * 既不进分子也不进分母（后端按 0 累加），但它们的输入仍计入「输入」列 ——
 * 所以有未上报时 `命中 + 未命中 < 输入` 是正确结果，不是表格算错了。
 * 界面上不再单独列「缓存未上报」一行：用户只需要能直接乘单价的四列数据。
 *
 * 单层命中 / 未命中列：`requestCount > 0 && cacheReported === 0` 时显示 `--`
 * 而不是 0（未知 ≠ 0）；完全没有请求的层（如未派生子 Agent）才是真正的 0。
 *
 * `hasDetail === false` 时（后端没有明细行，典型是早于明细表的历史会话）
 * **整块三层不渲染**，只退回一组四指标（输入 / 命中 / 未命中 / 输出）。
 * 命中 / 未命中来自 `sessions.total_*`，与明细表独立，所以这一段照常展示。
 * 把缺失的明细画成三层 0 会被读成「主对话一个 token 都没花」，比不显示更糟。
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
 * 「会话累计」的输入是每次请求整份上下文重复累加的结果（agent loop 每轮重发
 * 全量上下文），单看「输入 1.8M」会被读成"干了 1.8M token 的活"。表格把
 * 命中 / 未命中拆开后，才能看出那 1.8M 里 97% 走的是缓存价，全价计费的输入只有几十 k。
 *
 * 数据来源：后端 `sessions.total_cache_hit_tokens` / `total_cache_miss_tokens`，
 * 由 `pipeline.rs` **逐请求累加**、`session-usage-updated` 事件推送
 * （不再等回合收尾，否则长回合里这一栏会一直停在上一轮的值）。
 * 三层与「未上报」两个字段来自同一事件的 `breakdown`（按来源分组的绝对快照，
 * 前端归一化由 `utils/usageBreakdown.ts` 负责），本组件只负责展示，不重新聚合。
 *
 * 配色与右侧上下文面板（`ContextInspector.vue`）保持同一套语义：
 * **命中=绿、预热=蓝、未知=灰**——两侧不一致会让人误读状态。
 * 两条进度条**各按自己的状态上色**（会话累计跟根节点状态、本次请求跟 `state`），
 * 否则会出现"单次预热中 → 蓝色，但累计早已命中 90%"时把累计条也涂成蓝色的错误。
 *
 * 定位由父组件（`TerminalInput.vue`）负责，本组件只负责长什么样。
 */
import { computed } from "vue";
import { useI18n } from "vue-i18n";
import type { SessionUsageBreakdown, UsageSourceBreakdown } from "../../types";

const props = defineProps<{
  /** null 表示当前还没有上下文快照，此时整块「本次请求」分组不渲染 */
  state: "hit" | "warmup" | "unknown" | null;
  percent: number | null;
  hitTokens: number | null;
  totalTokens: number | null;
  /** 本次请求的输出 token；null = 该次快照没有输出字段（显示 `--`） */
  outputTokens: number | null;
  sessionInputTokens: number;
  sessionOutputTokens: number;
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

/** 本次请求的未命中：由「输入总量 − 命中」推出，两列口径来自同一份上报 */
const currentMissTokens = computed<number | null>(() => {
  if (props.hitTokens === null || props.totalTokens === null) return null;
  return Math.max(0, props.totalTokens - props.hitTokens);
});

/** 本次请求进度条宽度：与累计条同样留 2% 可见宽度，避免预热态看起来像渲染失败 */
const currentFillPercent = computed(() => {
  if (props.hitTokens === null || !props.totalTokens) return 0;
  return Math.max((props.hitTokens / props.totalTokens) * 100, 2);
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

/**
 * 三层按列合计：表格底部的「合计」行。
 *
 * 口径必须与上方三行完全一致（逐字段相加），不能改用 `sessions.total_*`：
 * 两个数据源来自同一次事件，但一旦某次事件只更新了一边，表格就会出现
 * 「合计 ≠ 三行之和」的自相矛盾。合计行只做加法，不引入第二套口径。
 */
const layerTotals = computed(() => {
  const total = { input: 0, output: 0, cacheHit: 0, cacheMiss: 0 };
  for (const layer of layers.value) {
    total.input += layer.data.input;
    total.output += layer.data.output;
    total.cacheHit += layer.data.cacheHit;
    total.cacheMiss += layer.data.cacheMiss;
  }
  return total;
});

/**
 * 单层命中 / 未命中单元格的文本。
 *
 * 有请求但一条缓存字段都没上报 → `--`（未知 ≠ 0）；
 * 完全没有请求的层（如从未派生子 Agent）→ 真正的 0。
 */
const layerCacheCell = (layer: UsageSourceBreakdown, value: number): string => {
  if (layer.requestCount > 0 && layer.cacheReported <= 0) return "--";
  return formatToken(value);
};

/** 后端没有明细行时（`hasDetail = false`）不渲染三层，退回一组四指标 */
const showLayers = computed(() => props.breakdown.hasDetail);
</script>

<template>
  <div class="cache-tip" :class="`cache-${sessionState}`" role="tooltip">
    <!-- ── 会话累计：命中率 + 命中/未命中绝对值 ── -->
    <div class="cache-tip-group">{{ t('input.cacheGroupSession') }}</div>

    <div class="cache-tip-hero">
      <span class="cache-tip-title">{{ t('input.tokenSessionHitRate') }}</span>
      <span class="cache-tip-percent">{{ sessionHitPercentText }}</span>
    </div>

    <div v-if="hasSessionCache" class="cache-tip-track">
      <span
        class="cache-tip-fill"
        :class="`is-${sessionState}`"
        :style="{ width: `${sessionFillPercent}%` }"
      />
    </div>
    <div v-else class="cache-tip-hint">{{ t('input.tokenSessionCacheUnknownTitle') }}</div>

    <!-- ── 分层用量：主对话 / 子 Agent / 系统开销 ── -->
    <template v-if="showLayers">
      <div class="cache-tip-layers">
        <div class="cache-tip-layer cache-tip-layer-head">
          <span />
          <span>{{ t('input.tokenInput') }}</span>
          <span>{{ t('input.tokenSessionHit') }}</span>
          <span>{{ t('input.tokenSessionMiss') }}</span>
          <span>{{ t('input.tokenOutput') }}</span>
        </div>

        <div
          v-for="layer in layers"
          :key="layer.key"
          class="cache-tip-layer"
          :class="{ 'cache-tip-layer-primary': layer.key === 'main' }"
        >
          <span class="cache-tip-layer-name">{{ layer.label }}</span>
          <span>{{ formatToken(layer.data.input) }}</span>
          <span>{{ layerCacheCell(layer.data, layer.data.cacheHit) }}</span>
          <span>{{ layerCacheCell(layer.data, layer.data.cacheMiss) }}</span>
          <span>{{ formatToken(layer.data.output) }}</span>
        </div>

        <!-- 合计：三层按列相加。这里才是「整场会话花了多少」的答案，
             不是「输入 + 输出」（两者单价不同，相加无可计费含义）。 -->
        <div class="cache-tip-layer cache-tip-layer-total">
          <span class="cache-tip-layer-name">{{ t('input.layerTotalLabel') }}</span>
          <span>{{ formatToken(layerTotals.input) }}</span>
          <span>{{ hasSessionCache ? formatToken(layerTotals.cacheHit) : '--' }}</span>
          <span>{{ hasSessionCache ? formatToken(layerTotals.cacheMiss) : '--' }}</span>
          <span>{{ formatToken(layerTotals.output) }}</span>
        </div>
      </div>
    </template>

    <!-- 无明细：同一组四指标，命中 / 未命中在无数据时显示 `--` -->
    <template v-else>
      <div class="cache-tip-metrics">
        <div class="cache-tip-metric">
          <span class="cache-tip-metric-label">{{ t('input.tokenInput') }}</span>
          <span class="cache-tip-metric-value">{{ formatToken(sessionInputTokens) }}</span>
        </div>
        <div class="cache-tip-metric">
          <span class="cache-tip-metric-label">{{ t('input.tokenSessionHit') }}</span>
          <span class="cache-tip-metric-value">{{ hasSessionCache ? formatToken(sessionCacheHitTokens) : '--' }}</span>
        </div>
        <div class="cache-tip-metric">
          <span class="cache-tip-metric-label">{{ t('input.tokenSessionMiss') }}</span>
          <span class="cache-tip-metric-value">{{ hasSessionCache ? formatToken(sessionCacheMissTokens) : '--' }}</span>
        </div>
        <div class="cache-tip-metric">
          <span class="cache-tip-metric-label">{{ t('input.tokenOutput') }}</span>
          <span class="cache-tip-metric-value">{{ formatToken(sessionOutputTokens) }}</span>
        </div>
      </div>
    </template>

    <!-- ── 本次请求：最近一次请求的缓存读数（次要口径） ── -->
    <template v-if="hasCache">
      <div class="cache-tip-divider" />

      <div class="cache-tip-group">{{ t('input.cacheGroupCurrent') }}</div>

      <div class="cache-tip-hero">
        <span class="cache-tip-title">{{ title }}</span>
        <span class="cache-tip-num cache-tip-percent-sm" :class="`is-${state}`">{{ percentText || '--' }}</span>
      </div>

      <template v-if="currentMissTokens !== null">
        <div class="cache-tip-track is-thin">
          <span
            class="cache-tip-fill"
            :class="`is-${state}`"
            :style="{ width: `${currentFillPercent}%` }"
          />
        </div>
        <div class="cache-tip-metrics">
          <div class="cache-tip-metric">
            <span class="cache-tip-metric-label">{{ t('input.tokenInput') }}</span>
            <span class="cache-tip-metric-value">{{ formatToken(totalTokens ?? 0) }}</span>
          </div>
          <div class="cache-tip-metric">
            <span class="cache-tip-metric-label">{{ t('input.tokenSessionHit') }}</span>
            <span class="cache-tip-metric-value">{{ formatToken(hitTokens ?? 0) }}</span>
          </div>
          <div class="cache-tip-metric">
            <span class="cache-tip-metric-label">{{ t('input.tokenSessionMiss') }}</span>
            <span class="cache-tip-metric-value">{{ formatToken(currentMissTokens) }}</span>
          </div>
          <div class="cache-tip-metric">
            <span class="cache-tip-metric-label">{{ t('input.tokenOutput') }}</span>
            <span class="cache-tip-metric-value">{{ outputTokens === null ? '--' : formatToken(outputTokens) }}</span>
          </div>
        </div>
      </template>
      <div v-else class="cache-tip-hint">{{ t('input.cacheUnknownHint') }}</div>

      <div v-if="state === 'warmup'" class="cache-tip-hint">{{ t('input.cacheWarmupHint') }}</div>
    </template>
  </div>
</template>

<style scoped>
/* 毛玻璃配方与 .profile-menu 一致，避免浮层风格两套 */
.cache-tip {
  /* 定位：贴在读数标签正上方。左对齐到触发元素，340px 宽的浮层不会越过窗口左边界。 */
  position: absolute;
  bottom: calc(100% + 8px);
  left: 0;
  z-index: 120;
  /* 340px：五列表格（层名 + 输入 / 命中 / 未命中 / 输出）需要比旧的
     四列（输入 / 输出 / 合计）更宽；四指标区也按五列同宽对齐 */
  width: 340px;
  padding: 12px;
  background: var(--surface-strong);
  border: 1px solid color-mix(in srgb, var(--text-muted) 22%, transparent);
  border-radius: var(--radius-lg);
  box-shadow: var(--shadow-lg);
  color: var(--text-muted);
  font-size: 0.65rem;
  line-height: 1.45;
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

/* 分组标题：右侧接一条细线，读作分节标题而不是又一行数据 */
.cache-tip-group {
  display: flex;
  align-items: center;
  gap: 8px;
  margin-bottom: 2px;
  font-size: 0.58rem;
  letter-spacing: 0.08em;
  color: var(--text-muted);
  opacity: 0.9;
}

.cache-tip-group::after {
  content: "";
  flex: 1;
  height: 1px;
  background: color-mix(in srgb, var(--text-muted) 16%, transparent);
}

.cache-tip-hero {
  display: flex;
  align-items: baseline;
  justify-content: space-between;
  gap: 8px;
}

.cache-tip-title {
  color: var(--text-soft);
  font-weight: 500;
}

.cache-tip-percent {
  font-size: 1.3rem;
  line-height: 1.15;
  font-weight: var(--fw-semibold);
  font-variant-numeric: tabular-nums;
  letter-spacing: -0.02em;
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
  color: var(--text-faint);
}

.cache-tip-track {
  position: relative;
  height: 6px;
  margin: 8px 0;
  border-radius: 3px;
  overflow: hidden;
  background: color-mix(in srgb, var(--text-muted) 24%, transparent);
}

/* 「本次请求」的条更薄：次要口径不该和累计条抢分量 */
.cache-tip-track.is-thin {
  height: 3px;
  margin: 6px 0;
}

.cache-tip-fill {
  display: block;
  height: 100%;
  border-radius: 3px;
  transition: width var(--transition-fast);
}

/* 两条进度条各按自己的状态上色：累计条跟 sessionState，单次条跟 state。
   共用根节点状态会在「单次预热中、累计已命中」时把累计条错涂成蓝色。 */
.cache-tip-fill.is-hit {
  background: var(--accent-green);
}

/* 预热态：整条只有 2% 的可见宽度（`*FillPercent` 的下限），
   不填色就完全看不见，会被当成渲染失败 */
.cache-tip-fill.is-warmup {
  background: var(--accent-blue);
}

/* 四指标：无明细的累计区与「本次请求」共用。
   标签在上、数值在下，四列等宽，与主表的四个数字列保持同一节奏。 */
.cache-tip-metrics {
  display: grid;
  grid-template-columns: repeat(4, minmax(0, 1fr));
  gap: 8px;
  margin-top: 8px;
}

.cache-tip-metric {
  display: flex;
  flex-direction: column;
  gap: 1px;
  min-width: 0;
}

.cache-tip-metric-label {
  font-size: 0.58rem;
  color: var(--text-faint);
}

.cache-tip-metric-value {
  font-size: 0.72rem;
  font-weight: var(--fw-semibold);
  font-variant-numeric: tabular-nums;
  color: var(--text-main);
}

/* 未上报说明与表格注脚已移除：用户要求只保留可算价的四列数据。
   未上报缓存的输入仍计入「输入」列，因此「命中 + 未命中 < 输入」时
   差额就是未上报部分，不再在界面上单独解释。 */

/* 三层分组表：层名 + 输入 / 命中 / 未命中 / 输出。
   用 grid 而不是 flex：表头与数据行必须跨行对齐，
   否则同一列的数字会因层名长度不同而错位。 */
.cache-tip-layers {
  display: flex;
  flex-direction: column;
  margin-top: 10px;
  font-variant-numeric: tabular-nums;
}

.cache-tip-layer {
  display: grid;
  grid-template-columns: minmax(0, 1fr) 54px 54px 54px 54px;
  align-items: baseline;
  gap: 4px;
  padding: 5px 0;
  text-align: right;
  border-bottom: 1px solid color-mix(in srgb, var(--text-muted) 10%, transparent);
}

/* 表头只是列名（分组 / 输入 / 命中 / 未命中 / 输出）：缩一号字、降透明度 */
.cache-tip-layer-head {
  padding-top: 0;
  font-size: 0.58rem;
  color: var(--text-muted);
  opacity: 0.9;
  border-bottom-color: color-mix(in srgb, var(--text-muted) 18%, transparent);
}

/* 表头首列跟层名一样左对齐：右对齐的「分组」会飘到数字列上方，读不出是列名 */
.cache-tip-layer-head > span:first-child {
  text-align: left;
}

.cache-tip-layer-name {
  display: flex;
  align-items: center;
  gap: 4px;
  text-align: left;
  color: var(--text-muted);
}

/* 主对话是花钱最多的一层：层名与数字各提亮一档，形成表格内的主次 */
.cache-tip-layer-primary .cache-tip-layer-name {
  color: var(--text-main);
  font-weight: var(--fw-semibold);
}

.cache-tip-layer-primary > span + span {
  color: var(--text-soft);
}

/* 合计行：逐列等于上方三层之和。更实的分隔线 + 提亮字重，
   让「整场会话」与「某一层」一眼分开。 */
.cache-tip-layer-total {
  border-top: 1px solid color-mix(in srgb, var(--text-muted) 30%, transparent);
  border-bottom: none;
  font-weight: var(--fw-semibold);
  color: var(--text-main);
}

.cache-tip-layer-total .cache-tip-layer-name {
  color: var(--text-main);
  font-weight: var(--fw-semibold);
}

/* 「本次请求」组的读数：字号比大字号那个百分比小，颜色跟自己的状态走
   （大字号用的是根节点上的会话状态，两者不能共用一个类）。 */
.cache-tip-percent-sm {
  font-size: 0.85rem;
  font-weight: var(--fw-semibold);
  font-variant-numeric: tabular-nums;
}

.cache-tip-num.is-hit {
  font-weight: var(--fw-semibold);
  color: var(--accent-green);
}

.cache-tip-num.is-warmup {
  font-weight: var(--fw-semibold);
  color: var(--accent-blue);
}

/* 「未上报」态：读数显示 `--`，降透明度 + help 光标，
   让 `--` 读起来像"没数据"而不是"没命中"。
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
  line-height: 1.5;
}

.cache-tip-divider {
  height: 1px;
  margin: 10px 0 8px;
  background: color-mix(in srgb, var(--text-muted) 18%, transparent);
}
</style>
