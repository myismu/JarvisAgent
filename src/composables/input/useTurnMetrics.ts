/**
 * # useTurnMetrics.ts — 输入区概览栏指标与浮层派生状态
 *
 * 汇总上下文占用、进度环弧长、单次/会话累计缓存命中率、当前轮次/步数
 * 以及最近一次请求的输出吞吐速度（tok/s），供 `TerminalInput` 概览栏与
 * `CacheHitTooltip` 浮层消费。
 *
 * ## Key Exports
 * - `CONTEXT_RING_CIRCUMFERENCE`: 进度环圆周长常量（r=9）
 * - `formatToken()`: token 数量紧凑格式化（k / M）
 * - `useTurnMetrics()`: 返回全部响应式指标与浮层开关
 *
 * ## Dependencies
 * - Internal: `@/stores/session`, `@/stores/agent`, `@/utils/contextUsage`, `@/utils/usageBreakdown`
 * - External: `vue`, `vue-i18n`
 */
import { computed, ref } from "vue";
import { useI18n } from "vue-i18n";
import { useSessionStore } from "../../stores/session";
import { useAgentStore } from "../../stores/agent";
import { resolveContextTokens } from "../../utils/contextUsage";
import { createEmptySessionUsageBreakdown } from "../../utils/usageBreakdown";

/** 进度环的圆周长（r=9，必须与模板里 `<circle r>` 一致） */
export const CONTEXT_RING_CIRCUMFERENCE = 2 * Math.PI * 9;

export function formatToken(n: number): string {
  if (n >= 1_000_000) return `${(n / 1_000_000).toFixed(1)}M`;
  if (n >= 1_000) return `${(n / 1000).toFixed(1)}k`;
  return `${n}`;
}

export function useTurnMetrics() {
  const { t } = useI18n();
  const session = useSessionStore();
  const agent = useAgentStore();

  const sessionTokenTotal = computed(
    () => (session.totalInputTokens || 0) + (session.totalOutputTokens || 0),
  );

  /** 会话用量的按来源分组快照（主对话 / 子 Agent / 系统开销 / 未上报请求） */
  const usageBreakdown = computed(
    () => session.currentSessionView?.sessionUsageBreakdown ?? createEmptySessionUsageBreakdown(),
  );

  /** 当前上下文占用：优先厂商实测 providerInputTokens，缺失回退本地估算 */
  const contextTokens = computed(() => resolveContextTokens(agent.currentContextSnapshot));

  /** 上下文窗口上限（来自编译期内嵌的 model_registry.json；未登记为 null） */
  const contextWindow = computed(() => agent.currentContextSnapshot?.maxContextTokens ?? null);

  const contextUsageLabel = computed(() => {
    if (contextTokens.value <= 0) return null;
    const used = formatToken(contextTokens.value);
    const max = contextWindow.value;
    if (!max) {
      return `${t("input.tokenContext")} ${used} · ${t("input.tokenWindowUnknown")}`;
    }
    const percent = Math.min(100, Math.round((contextTokens.value / max) * 1000) / 10);
    return `${t("input.tokenContext")} ${percent.toFixed(1)}% · ${used} / ${formatToken(max)}`;
  });

  const contextRingDash = computed(() => {
    const max = contextWindow.value;
    if (!max || contextTokens.value <= 0) return 0;
    const ratio = Math.min(1, contextTokens.value / max);
    return ratio * CONTEXT_RING_CIRCUMFERENCE;
  });

  /** 单次口径缓存命中读数（供 CacheHitTooltip「本次请求」组使用） */
  const cacheUsage = computed(() => {
    const snap = agent.currentContextSnapshot;
    if (!snap) return null;
    const hit = snap.cacheHitTokens ?? null;
    if (hit === null) {
      return {
        label: t("input.cacheNotReported"),
        state: "unknown" as const,
        percent: null,
        hitTokens: null,
        totalTokens: null,
      };
    }
    const total = hit + (snap.cacheMissTokens ?? 0);
    if (!total) return null;
    const percent = Math.round((hit / total) * 100);
    if (hit === 0) {
      return {
        label: t("input.cacheLabelWarmup"),
        state: "warmup" as const,
        percent: 0,
        hitTokens: 0,
        totalTokens: total,
      };
    }
    return {
      label: `${t("input.cacheLabel")} ${percent}%`,
      state: "hit" as const,
      percent,
      hitTokens: hit,
      totalTokens: total,
    };
  });

  /** 本次请求的输出 token（厂商未报时保留 null） */
  const currentOutputTokens = computed(
    () => agent.currentContextSnapshot?.providerOutputTokens ?? null,
  );

  /** 概览栏缓存读数（会话累计口径） */
  const barCacheUsage = computed(() => {
    if (!agent.currentContextSnapshot) return null;
    const hit = session.totalCacheHitTokens || 0;
    const miss = session.totalCacheMissTokens || 0;
    if (hit + miss <= 0) {
      return { label: t("input.cacheNotReported"), state: "unknown" as const };
    }
    if (hit === 0) {
      return { label: t("input.cacheLabelWarmup"), state: "warmup" as const };
    }
    const raw = (hit / (hit + miss)) * 100;
    const text = raw < 1 ? "<1" : `${Math.round(raw)}`;
    return { label: `${t("input.cacheLabelSession")} ${text}%`, state: "hit" as const };
  });

  /** 概览栏「轮」与「步」共用的目标 run（活跃优先，否则回退最近 run） */
  const currentTurnRun = computed(() => {
    const runs = agent.currentAgentRuns;
    const activeRunId = session.currentSessionView?.activeRunId;
    if (activeRunId) {
      const active = runs.find((run) => run.runId === activeRunId);
      if (active) return active;
    }
    return runs[0] ?? null;
  });

  const conversationTurn = computed(() => {
    const runs = agent.currentAgentRuns;
    const run = currentTurnRun.value;
    if (!runs.length || !run) return 0;
    const index = runs.findIndex((item) => item.runId === run.runId);
    return runs.length - (index < 0 ? 0 : index);
  });

  /** 三源取最大：运行中 currentTurn.loop / 内存 loopStepCount / 持久化 agent_run_events 行数 */
  const turnStepCount = computed(() => {
    const view = session.currentSessionView;
    const live = view?.currentTurn?.loop || 0;
    const snapshot = view?.loopStepCount || 0;
    const runId = currentTurnRun.value?.runId;
    const persisted = runId ? agent.getAgentRunEvents(runId).length : 0;
    return Math.max(live, snapshot, persisted);
  });

  const turnProgressLabel = computed(() => {
    const turn = conversationTurn.value;
    if (!turn) return null;
    const step = turnStepCount.value;
    const turnText = t("input.turnLabel", { loop: turn });
    return step > 0 ? `${turnText} · ${t("input.stepLabel", { step })}` : turnText;
  });

  /** 最近一次请求的输出速度（tok/s） */
  const throughputLabel = computed(() => {
    const rate = session.currentSessionView?.lastRequestTokPerSec;
    if (typeof rate !== "number" || !Number.isFinite(rate) || rate <= 0) return null;
    return `${rate.toFixed(1)} tok/s`;
  });

  const showCacheTip = ref(false);
  const showContextTip = ref(false);

  const openContextPanel = () => {
    agent.showAgentPanel = true;
  };

  return {
    CONTEXT_RING_CIRCUMFERENCE,
    formatToken,
    sessionTokenTotal,
    usageBreakdown,
    contextTokens,
    contextWindow,
    contextUsageLabel,
    contextRingDash,
    cacheUsage,
    currentOutputTokens,
    barCacheUsage,
    currentTurnRun,
    conversationTurn,
    turnStepCount,
    turnProgressLabel,
    throughputLabel,
    showCacheTip,
    showContextTip,
    openContextPanel,
  };
}
