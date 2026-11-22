/**
 * # useRunElapsed.ts — 运行中用时的读秒源
 *
 * 折叠头的「执行中 · 用时 N 秒」只认一个基准：后端 `agent_runs.started_at`。
 * 组件不再自己维护开始时刻、累计等待量这些状态 —— 它们在 ChatArea 里既跨轮次又跨会话，
 * 一旦某条路径漏重置，就会接着上一个会话/上一轮的数字继续读。
 *
 * 等待时长同理只认后端 `AgentRun.waitMs`：它是「这一轮累计的非 Agent 责任时长」的
 * 权威值（含本组件挂载之前发生的等待）。本地只在后端值尚未下发的空窗期临时补扣一段。
 *
 * ## Key Exports
 * - `useRunElapsed(run, waiting)`: 返回 `elapsed`（秒）。run 为空即停表并归零。
 *
 * ## Dependencies
 * - Internal: `../types`（AgentRun）
 * - External: `vue`
 *
 * ## Constraints
 * - `run.startedAt` 必须与 `Date.now()` 同单位（毫秒；后端 v16 起已统一为毫秒）
 * - 本 composable 只负责「运行中」的展示读秒；轮次结束后的真实用时以
 *   后端 `durationMs` 为准（见 `AgentTurn.vue` 的 executionDurationSecs）
 */
import { onScopeDispose, ref, watch, type Ref } from "vue";
import type { AgentRun } from "../types";

export function useRunElapsed(
  run: Ref<AgentRun | null | undefined>,
  waiting: Ref<boolean>,
) {
  /** 已用秒数（向下取整）。 */
  const elapsed = ref(0);

  let timer: ReturnType<typeof setInterval> | null = null;
  /** 本次等待开始的时刻（0 = 当前不在等待）。等待期间读秒冻结在这个时刻。 */
  let waitStartedAt = 0;
  /**
   * 已落库的等待时长（毫秒）：直接取自后端 `AgentRun.waitMs`，是扣减的**基准**。
   *
   * 它是「这一轮到目前为止累计了多久非 Agent 责任时长」的权威口径，包含
   * 本组件挂载之前就已发生的等待 —— 切会话、事件迟到、监控面板后打开这些场景里，
   * 本地根本观察不到那段等待，只靠本地累计必然漏扣。
   */
  let backendWaitMs = 0;
  /**
   * 本地观察到的等待时长（毫秒）：各段「已结束时长为 `waitStartedAt` → 现在」之和。
   *
   * 只用于「后端值还没跟上」的空窗期：等待刚结束、后端 waitMs 尚未下发时，
   * 本地先扣掉这一段，秒表才不会在那一刻跳一下。
   * 它与 `backendWaitMs` 覆盖的是**同一段时间**，两者同时生效会重复扣除、
   * 让秒表读数比真实值偏小，所以后端值一到就把本变量归零（见下方 waitMs 的 watch）。
   */
  let localWaitedMs = 0;
  /** 读秒基准身份（runId + startedAt）：变化即视为新的一轮，累计量必须清零。 */
  let runKey = "";

  function stopTimer() {
    if (timer) {
      clearInterval(timer);
      timer = null;
    }
  }

  function startTimer() {
    if (timer) return;
    timer = setInterval(tick, 1000);
  }

  /** 重算一次当前用时。定时器每秒调用，等待状态变化时也会主动调一次。 */
  function tick() {
    const current = run.value;
    const startedAt = current?.startedAt || 0;
    if (!current || !startedAt) {
      elapsed.value = 0;
      return;
    }
    // 等待用户决策期间冻结在「开始等待」那一刻；恢复后把这段等待从总时长里扣掉。
    // 扣减 = 后端已落库的等待 + 本地尚未对账的那一段（两者不重叠，见各自注释）。
    const nowMs = waiting.value && waitStartedAt > 0 ? waitStartedAt : Date.now();
    const deductedMs = backendWaitMs + localWaitedMs;
    elapsed.value = Math.max(0, Math.floor((nowMs - startedAt - deductedMs) / 1000));
  }

  // 以 runKey 为唯一判据：新 run、run 消失、startedAt 被改写，都走同一条路径。
  watch(
    () => {
      const current = run.value;
      return current ? `${current.runId}:${current.startedAt}` : "";
    },
    (key) => {
      if (key !== runKey) {
        runKey = key;
        // 新的一轮：以该 run 已经落库的 waitMs 起步（旧 run 的累计不能带过来）。
        backendWaitMs = run.value?.waitMs ?? 0;
        localWaitedMs = 0;
        waitStartedAt = 0;
      }
      if (!key) {
        stopTimer();
        elapsed.value = 0;
        return;
      }
      tick();
      startTimer();
    },
    { immediate: true },
  );

  // 等待期间不清定时器：定时器照跑，但 tick 停在冻结值上，逻辑只有一条路径。
  watch(waiting, (isWaiting) => {
    if (!runKey) return;
    if (isWaiting) {
      if (waitStartedAt === 0) waitStartedAt = Date.now();
    } else if (waitStartedAt > 0) {
      localWaitedMs += Date.now() - waitStartedAt;
      waitStartedAt = 0;
    }
    tick();
  });

  // 后端 waitMs 一旦更新就以它为准：整值覆盖，并把本地累计归零。
  // 归零的原因不是「丢弃观察结果」，而是这段等待已经被后端计进去了，
  // 本地再留一份就会重复扣除（秒表读数偏小，且与对话区气泡对不上）。
  watch(
    () => run.value?.waitMs,
    (value) => {
      if (value == null) return;
      backendWaitMs = value;
      localWaitedMs = 0;
      tick();
    },
  );

  // 组件卸载（或所在 effect scope 失效）时停表，避免定时器泄漏。
  onScopeDispose(stopTimer);

  return { elapsed };
}
