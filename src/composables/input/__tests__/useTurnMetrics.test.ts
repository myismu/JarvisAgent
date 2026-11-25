/**
 * composables/input/useTurnMetrics.ts 单元测试
 *
 * 验证输入区概览栏的核心指标派生口径：
 * 1. 上下文窗口未知时不编造百分比、进度环弧长为 0
 * 2. 缓存命中率区分“未报告（unknown）”“预热期（warmup）”“<1%”与正常百分比
 * 3. “第 N 轮 · 第 M 步”三源取最大（覆盖重启后 activeRunId 为空的退化场景）
 */
import { describe, it, expect, beforeEach, vi } from "vitest";
import { createPinia, setActivePinia } from "pinia";

vi.mock("vue-i18n", async (importOriginal) => {
  const actual = await importOriginal<typeof import("vue-i18n")>();
  return {
    ...actual,
    useI18n: () => ({
      t: (key: string, params?: Record<string, unknown>) =>
        params ? `${key}:${JSON.stringify(params)}` : key,
    }),
  };
});

import { useSessionStore } from "../../../stores/session";
import { useAgentStore } from "../../../stores/agent";
import { useTurnMetrics, formatToken } from "../useTurnMetrics";

describe("useTurnMetrics", () => {
  beforeEach(() => {
    setActivePinia(createPinia());
  });

  it("formatToken 正确格式化普通数、千级与百万级 token", () => {
    expect(formatToken(512)).toBe("512");
    expect(formatToken(12500)).toBe("12.5k");
    expect(formatToken(1_850_000)).toBe("1.9M");
  });

  it("上下文窗口未知时不编造百分比且进度环弧长为 0；已知时按实测优先计算", () => {
    const session = useSessionStore();
    const agent = useAgentStore();
    session.activeSessionId = "s1";

    agent.upsertContextSnapshot({
      sessionId: "s1",
      loopCount: 1,
      model: "custom-model",
      intent: "edit",
      apiFormat: "openai",
      createdAt: 1000,
      totalChars: 4000,
      estimatedTokens: 1000,
      providerInputTokens: 2000,
      cacheHistory: [],
      maxContextTokens: null,
      maxOutputTokens: 4096,
      messageCount: 2,
      toolSchemaCount: 10,
      toolCallCount: 0,
      toolResultCount: 0,
      imageCount: 0,
      sections: [],
    });

    const metrics = useTurnMetrics();
    expect(metrics.contextTokens.value).toBe(2000);
    expect(metrics.contextRingDash.value).toBe(0);
    expect(metrics.contextUsageLabel.value).toContain("input.tokenWindowUnknown");

    // 补充窗口上限 10000 → 20.0%
    agent.upsertContextSnapshot({
      ...agent.currentContextSnapshot!,
      maxContextTokens: 10000,
    });
    expect(metrics.contextRingDash.value).toBeCloseTo(metrics.CONTEXT_RING_CIRCUMFERENCE * 0.2, 5);
    expect(metrics.contextUsageLabel.value).toContain("20.0%");
  });

  it("概览栏累计缓存区分 unknown / warmup / <1% / 正常百分比", () => {
    const session = useSessionStore();
    const agent = useAgentStore();
    session.activeSessionId = "s1";
    agent.upsertContextSnapshot({
      sessionId: "s1",
      loopCount: 1,
      model: "m",
      intent: "edit",
      apiFormat: "openai",
      createdAt: 1000,
      totalChars: 100,
      estimatedTokens: 50,
      cacheHistory: [],
      maxOutputTokens: 1024,
      messageCount: 1,
      toolSchemaCount: 1,
      toolCallCount: 0,
      toolResultCount: 0,
      imageCount: 0,
      sections: [],
    });

    const metrics = useTurnMetrics();

    // 命中与未命中都为 0 → unknown（未报告）
    expect(metrics.barCacheUsage.value?.state).toBe("unknown");

    // 命中为 0、未命中 > 0 → warmup
    session.setSessionUsageTotals("s1", { cacheHit: 0, cacheMiss: 5000 });
    expect(metrics.barCacheUsage.value?.state).toBe("warmup");

    // 命中率 < 1% → 展示 <1% 而非 0%
    session.setSessionUsageTotals("s1", { cacheHit: 1, cacheMiss: 500 });
    expect(metrics.barCacheUsage.value?.state).toBe("hit");
    expect(metrics.barCacheUsage.value?.label).toContain("<1%");
  });

  it("重启后 activeRunId 为空时，轮次与步数正确回退到最近 run 与持久化 events 行数", () => {
    const session = useSessionStore();
    const agent = useAgentStore();
    session.activeSessionId = "s1";

    agent.upsertAgentRun({
      runId: "r1",
      sessionId: "s1",
      status: "completed",
      loopCount: 0,
      inputTokens: 100,
      outputTokens: 50,
      startedAt: 1000,
      updatedAt: 2000,
      resumable: false,
    });
    agent.upsertAgentRun({
      runId: "r2",
      sessionId: "s1",
      status: "completed",
      loopCount: 0,
      inputTokens: 100,
      outputTokens: 50,
      startedAt: 3000,
      updatedAt: 4000,
      resumable: false,
    });
    agent.agentRunEventsByRun = {
      r2: [
        {
          eventId: "e1",
          runId: "r2",
          sessionId: "s1",
          loopIndex: 1,
          respBlocks: [],
          toolResults: [],
          status: "complete",
          inputTokens: 10,
          outputTokens: 10,
          startedAt: 3000,
          updatedAt: 3500,
        },
        {
          eventId: "e2",
          runId: "r2",
          sessionId: "s1",
          loopIndex: 2,
          respBlocks: [],
          toolResults: [],
          status: "complete",
          inputTokens: 10,
          outputTokens: 10,
          startedAt: 3600,
          updatedAt: 4000,
        },
      ],
    };

    const metrics = useTurnMetrics();
    expect(metrics.conversationTurn.value).toBe(2);
    expect(metrics.turnStepCount.value).toBe(2);
  });
});
