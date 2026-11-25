import type {
  AgentBlockStatus,
  AgentCurrentTurn,
  AgentExecutionLog,
  AgentStep,
  AgentTextBlock,
  AgentTextBlockKind,
  AgentThinkingBlock,
  AgentToolCallView,
  AgentToolStatus,
  AgentTurnSnapshot,
} from "../types";
// 全局 i18n 实例（与 `utils/toolDisplay.ts` / `utils/markdown.ts` 同一取法）：
// 本文件是纯工具函数，拿不到组件上下文，故走 `i18n.global.t`
import { i18n } from "../i18n";

let sequence = 0;

function nextId(prefix: string) {
  sequence += 1;
  return `${prefix}_${Date.now()}_${sequence}`;
}

function now() {
  return Date.now();
}

function bump(turn: AgentCurrentTurn) {
  turn.revision += 1;
}

function normalizeLoop(turn: AgentCurrentTurn, loop?: number | null) {
  if (typeof loop === "number" && Number.isFinite(loop) && loop > 0) {
    turn.loop = loop;
    return loop;
  }
  if (turn.loop <= 0) {
    turn.loop = 1;
  }
  return turn.loop;
}

function normalizeToolStatus(status: AgentToolStatus | string): AgentToolStatus {
  if (status === "running" || status === "completed" || status === "error" || status === "pending") {
    return status;
  }
  return "pending";
}

function findTextBlock(turn: AgentCurrentTurn, id: string | null) {
  if (!id) return undefined;
  return turn.textBlocks.find((item) => item.id === id);
}

function findThinkingBlock(turn: AgentCurrentTurn, id: string | null) {
  if (!id) return undefined;
  return turn.thinkingBlocks.find((item) => item.id === id);
}

function closeActiveText(turn: AgentCurrentTurn) {
  const block = findTextBlock(turn, turn.activeTextBlockId);
  if (block) {
    block.status = "done";
  }
  turn.activeTextBlockId = null;
}

function closeActiveThinking(turn: AgentCurrentTurn) {
  const block = findThinkingBlock(turn, turn.activeThinkingBlockId);
  if (block) {
    block.status = "done";
  }
  turn.activeThinkingBlockId = null;
}

function cloneTextBlock(block: AgentTextBlock): AgentTextBlock {
  return { ...block };
}

function cloneThinkingBlock(block: AgentThinkingBlock): AgentThinkingBlock {
  return { ...block };
}

function cloneToolCall(tool: AgentToolCallView): AgentToolCallView {
  return { ...tool, logs: [...tool.logs] };
}

function cloneLog(log: AgentExecutionLog): AgentExecutionLog {
  return { ...log };
}

export function createEmptyAgentCurrentTurn(): AgentCurrentTurn {
  return {
    id: nextId("turn"),
    loop: 0,
    revision: 0,
    isRunning: false,
    hasToolActivity: false,
    activeTextBlockId: null,
    activeThinkingBlockId: null,
    textBlocks: [],
    thinkingBlocks: [],
    toolCalls: [],
    logs: [],
    notice: undefined,
    // 清除信号（`notice_clear`）靠这个字段精确匹配来源，新建 turn 时必须一并置空，
    // 否则上一轮的来源标记会残留下来，导致清除时误匹配（2026-11-22）。
    noticeKind: undefined,
    startedAt: null,
  };
}

export function resetAgentCurrentTurn(view: { currentTurn: AgentCurrentTurn }) {
  view.currentTurn = createEmptyAgentCurrentTurn();
}

export function beginAgentLoop(
  view: { currentTurn: AgentCurrentTurn },
  loop?: number | null,
) {
  const turn = view.currentTurn;
  closeActiveText(turn);
  closeActiveThinking(turn);
  if (!turn.startedAt) {
    turn.startedAt = now();
  }
  turn.isRunning = true;
  normalizeLoop(turn, loop ?? (turn.loop > 0 ? turn.loop + 1 : 1));
  bump(turn);
}

export function appendAgentText(
  view: { currentTurn: AgentCurrentTurn },
  content: string,
  kind: AgentTextBlockKind = "assistant",
  loop?: number | null,
) {
  if (!content) return;
  const turn = view.currentTurn;
  turn.isRunning = true;
  const blockLoop = normalizeLoop(turn, loop);
  let block = findTextBlock(turn, turn.activeTextBlockId);
  if (!block || block.kind !== kind || block.status === "done") {
    block = {
      id: nextId("text"),
      loop: blockLoop,
      kind,
      content: "",
      status: "streaming",
      timestamp: now(),
    };
    turn.textBlocks.push(block);
    turn.activeTextBlockId = block.id;
  }
  block.content += content;
  block.status = "streaming";
  bump(turn);
}

export function appendAgentThinking(
  view: { currentTurn: AgentCurrentTurn },
  content: string,
  loop?: number | null,
) {
  if (!content) return;
  const turn = view.currentTurn;
  turn.isRunning = true;
  const blockLoop = normalizeLoop(turn, loop);
  let block = findThinkingBlock(turn, turn.activeThinkingBlockId);
  if (!block || block.status === "done") {
    block = {
      id: nextId("thinking"),
      loop: blockLoop,
      content: "",
      status: "streaming",
      timestamp: now(),
    };
    turn.thinkingBlocks.push(block);
    turn.activeThinkingBlockId = block.id;
  }
  block.content += content;
  block.status = "streaming";
  bump(turn);
}

export function markAgentToolActivity(view: { currentTurn: AgentCurrentTurn }, loop?: number | null) {
  const turn = view.currentTurn;
  closeActiveText(turn);
  closeActiveThinking(turn);
  turn.hasToolActivity = true;
  turn.isRunning = true;
  normalizeLoop(turn, loop);
  bump(turn);
}

export function upsertAgentToolCall(
  view: { currentTurn: AgentCurrentTurn },
  id: string,
  name: string,
  status: AgentToolStatus | string,
  loop?: number | null,
) {
  if (!id || !name) return;
  const turn = view.currentTurn;
  const toolLoop = normalizeLoop(turn, loop);
  let item = turn.toolCalls.find((tool) => tool.id === id);
  if (!item) {
    item = {
      id,
      loop: toolLoop,
      name,
      status: "pending",
      logs: [],
      timestamp: now(),
      updatedAt: now(),
    };
    turn.toolCalls.push(item);
  }
  item.name = name;
  item.status = normalizeToolStatus(status);
  item.updatedAt = now();
  turn.hasToolActivity = true;
  turn.isRunning = item.status === "running" || turn.isRunning;
  bump(turn);
}

export function appendAgentExecutionLog(
  view: { currentTurn: AgentCurrentTurn },
  content: string,
  loop?: number | null,
) {
  if (!content) return;
  const turn = view.currentTurn;
  const logLoop = normalizeLoop(turn, loop);
  turn.logs.push({
    id: nextId("log"),
    loop: logLoop,
    content,
    timestamp: now(),
  });
  turn.hasToolActivity = true;
  turn.isRunning = true;
  bump(turn);
}

/**
 * 按调用 id 精确更新工具调用视图。
 *
 * 工具名**不是唯一标识**：同一轮里连续两次 `RunCommand` 会留下两个同名条目，
 * 只按名字反查会把结果写到别人身上（2026-11 真实事故：`tool_error` 命中了后一条，
 * 错误显示在错误的工具卡上）。所以主路径是 `toolCallId` 精确匹配。
 *
 * 按名字从后往前找**只是旧数据兜底**：历史快照 / 老后端的事件没有 id，
 * 此时退一步至少能把结果挂到最近一条同名条目上。
 *
 * @returns 是否命中（false 表示需要由调用方新建条目）
 */
function updateTool(
  turn: AgentCurrentTurn,
  toolCallId: string | undefined,
  name: string | undefined,
  patch: Partial<AgentToolCallView>,
) {
  const id = (toolCallId ?? "").trim();
  let item = id ? turn.toolCalls.find((tool) => tool.id === id) : undefined;
  // 只有事件本身没有 toolCallId（老数据）时，才退回按工具名兜底；
  // 明确带了新 toolCallId 但列表里尚不存在时，不得误改已有同名条目，应返回 false 让调用方新建。
  if (!item && !id) {
    if (!name) return false;
    // 旧数据兜底：取最后一条同名条目
    item = [...turn.toolCalls].reverse().find((tool) => tool.name === name);
  }
  if (!item) return false;
  Object.assign(item, patch, { updatedAt: now() });
  return true;
}

export function applyAgentStepToCurrentTurn(
  view: { currentTurn: AgentCurrentTurn },
  step: AgentStep,
) {
  const turn = view.currentTurn;
  if (step.type === "tool_call") {
    const updated = updateTool(turn, step.toolCallId, step.tool, {
      input: step.content,
      status: "running",
    });
    if (!updated && step.tool) {
      // 新建条目**必须**用事件里的调用 id（存在时）：后续 tool_result / tool_error
      // 是按 id 寻址的，这里若换成前端临时 id，结果事件就永远匹配不上（只能退回按名字兜底，
      // 同名工具又会互相覆盖）。
      const id = (step.toolCallId ?? "").trim() || `step_${nextId("tool")}`;
      upsertAgentToolCall(view, id, step.tool, "running", turn.loop);
      updateTool(turn, id, step.tool, { input: step.content });
    }
  } else if (step.type === "tool_result") {
    updateTool(turn, step.toolCallId, step.tool, {
      output: step.content,
      status: "completed",
    });
  } else if (step.type === "tool_error") {
    updateTool(turn, step.toolCallId, step.tool, {
      error: step.error,
      output: step.content,
      status: "error",
    });
  } else if (step.type === "task_scheduled" && step.taskId != null) {
    const taskName = `Task #${step.taskId}: ${step.subject || ""}`;
    upsertAgentToolCall(view, `task_${step.taskId}`, taskName, "running", turn.loop);
  } else if (step.type === "task_completed" && step.taskId != null) {
    const taskPrefix = `Task #${step.taskId}:`;
    const item = [...turn.toolCalls].reverse().find((tool) =>
      tool.name.startsWith(taskPrefix),
    );
    if (item) {
      item.status = step.status === "完成" ? "completed" : "error";
      item.output = step.status;
      item.updatedAt = now();
    }
  }
  bump(turn);
}

export function finishAgentLoop(
  view: { currentTurn: AgentCurrentTurn },
  hasTool: boolean,
) {
  const turn = view.currentTurn;
  closeActiveText(turn);
  closeActiveThinking(turn);
  turn.isRunning = hasTool;
  if (hasTool) {
    turn.hasToolActivity = true;
  }
  turn.toolCalls.forEach((tool) => {
    if (tool.status === "running" && !hasTool) {
      tool.status = "completed";
      tool.updatedAt = now();
    }
  });
  bump(turn);
}

export function completeAgentCurrentTurn(view: { currentTurn: AgentCurrentTurn }) {
  const turn = view.currentTurn;
  closeActiveText(turn);
  closeActiveThinking(turn);
  turn.isRunning = false;
  bump(turn);
}

export function hasStructuredTurnContent(turn: AgentCurrentTurn) {
  return Boolean(
    turn.textBlocks.some((block) => block.content.trim()) ||
      turn.thinkingBlocks.some((block) => block.content.trim()) ||
      turn.toolCalls.length > 0 ||
      turn.logs.some((log) => log.content.trim()),
  );
}

/**
 * 把后端的状态标注（i18n key + 可选补充数据）渲染成最终展示文本。
 *
 * 后端**只给 key**（`notice.streamTimeout` 之类），文案在语言包的 `notice.*` 下 ——
 * 后端曾经直接返回中文句子，界面切英文后中断时仍冒中文。
 *
 * `detail` 是"无法翻译但必须原样显示"的补充（具体错误文本、等待秒数），
 * 由语言包的 `notice.detail` 包一层再拼到主句后面。
 *
 * ⚠️ 与 `useAgentEvents` 里的等待提示/重试进度**同一套 key 词汇**：
 * 那两处是运行期事件（带秒数参数），这里是落库后的历史重放，两边措辞必须一致，
 * 否则用户刷新前后会看到不同说法。
 */
export function noticeTextFromBackend(
  i18nKey?: string | null,
  detail?: string | null,
): string | undefined {
  if (!i18nKey) return undefined;
  const base = i18n.global.t(i18nKey);
  const extra = (detail ?? "").trim();
  if (!extra) return base;
  return `${base} ${i18n.global.t("notice.detail", { detail: extra })}`;
}

/**
 * 把后端最终回执的正文合并进快照的正文块，保证既不丢尾部、也不重复。
 *
 * 背景（为什么不能按时间戳裁剪）：
 * 快照里的正文来自两个源 —— 前端流式累积的 `textBlocks`（每个 loop 一段），
 * 以及后端在本轮收尾时返回的 `finalContent`（整轮最终正文）。
 * 这里曾经有过一版「删掉所有时间戳早于最后一次工具调用的正文块」的实现，
 * 名义上是去重，实际上：
 *   1. `finalContent` 原先只在 `textBlocks` 全空时才追加，两源不可能同时存在，
 *      那段裁剪的净效果只是把真实的中间叙述删掉；
 *   2. 后端重建历史（`history.rs` 的 `append_assistant_content`）逐条保留所有正文块，
 *      前端一裁剪，「刚结束」就比「刷新后」少内容，同一条数据出现两套口径。
 *
 * 所以这里改为按内容对齐，只处理最终正文与已有正文的重叠：
 *   - 已有正文为空 → 追加整段；
 *   - 已有正文已包含最终正文（相等或以它结尾）→ 不追加；
 *   - 最终正文以已有正文为前缀 → 只把差集接到最后一个正文块末尾，避免前半段重复；
 *   - 其余（两段文本有实质差异）→ 追加整段，此时 `finalContent` 才是完整版本。
 *
 * 「哪一段是最终回答」**不在这里判断** —— 那是渲染层 `timelineSplit` 的职责。
 * 快照层只保证正文完整且不重复；判断只保留一处，否则两层各判一次必然漂移。
 */
function mergeFinalText(textBlocks: AgentTextBlock[], finalContent: string, loop: number) {
  const final = finalContent.trim();
  if (!final) return;

  const visible = textBlocks.map((block) => block.content).join("").trim();

  // 1) 没有任何流式正文：整段补进来（模型只返回最终内容、没有过程流的场景）。
  if (!visible) {
    textBlocks.push({
      id: nextId("text_final"),
      loop: Math.max(loop, 1),
      kind: "assistant",
      content: final,
      status: "done",
      timestamp: now(),
    });
    return;
  }

  // 2) 最终正文已被流式正文覆盖（完全相等，或流式正文以它结尾）：不重复追加。
  if (visible === final || visible.endsWith(final)) return;

  // 3) 最终正文 = 已有正文 + 尾部补充：只把差集接到最后一个块末尾，不能整段再追加一遍。
  if (final.startsWith(visible)) {
    const tail = final.slice(visible.length);
    if (tail.trim()) {
      const last = textBlocks[textBlocks.length - 1];
      last.content = `${last.content}${tail}`;
    }
    return;
  }

  // 4) 其余情况：流式文本与最终文本不是同一版本（流式被截断、或后端重写了整段）。
  //    此时 finalContent 才是完整版本，整段追加，且**不删除**任何已有的过程叙述。
  textBlocks.push({
    id: nextId("text_final"),
    loop: Math.max(loop, 1),
    kind: "assistant",
    content: final,
    status: "done",
    timestamp: now(),
  });
}

export function buildAgentTurnSnapshot(
  turn: AgentCurrentTurn,
  finalContent: string,
  fallbackExecution: string,
  status: string,
  /**
   * 状态标注的 i18n key（气泡下方小字）。**显式传入**，不从 turn.notice 继承
   * （理由见下方 return 处）。
   *
   * 传 key 而不是成句：这样界面切语言时，这条标注会跟着变 ——
   * 存成句的话切语言后它仍是旧语言，直到重新加载。
   */
  noticeI18nKey?: string,
  /** 状态标注的补充数据（原样展示，如具体错误文本） */
  noticeDetail?: string,
  /**
   * 本轮总时长（毫秒）。**前端不自己计时**，由调用方从后端落库的
   * `agent_runs.started_at` / `finished_at` 差值算好传进来 ——
   * 刷新后的历史快照走的是同一条口径，两边数值才能一致。
   *
   * 缺省 / null：本轮时长未知（如续跑路径没有对应的用户消息、
   * 或本次 run 尚未收到收尾事件），此时不写字段，界面不显示时长。
   */
  durationMs?: number | null,
): AgentTurnSnapshot {
  completeAgentCurrentTurn({ currentTurn: turn });

  const textBlocks = turn.textBlocks.map(cloneTextBlock);
  const thinkingBlocks = turn.thinkingBlocks.map(cloneThinkingBlock);
  const toolCalls = turn.toolCalls.map(cloneToolCall);
  const logs = turn.logs.map(cloneLog);

  // 正文合并：只补差集、不裁过程（详见 mergeFinalText 的注释）。
  // 这里**不再**按「最后一次工具调用的时间戳」删除正文块 —— 那会让中间叙述
  // 在刚结束时消失、刷新后又出现（后端重建不裁剪）。
  mergeFinalText(textBlocks, finalContent, turn.loop);

  const hasExecution = thinkingBlocks.length > 0 || toolCalls.length > 0 || logs.length > 0;
  if (fallbackExecution.trim() && !hasExecution) {
    logs.push({
      id: nextId("log_fallback"),
      loop: Math.max(turn.loop, 1),
      content: fallbackExecution,
      timestamp: now(),
    });
  }

  return {
    version: 1,
    status,
    textBlocks: textBlocks.map((block) => ({ ...block, status: "done" as AgentBlockStatus })),
    thinkingBlocks: thinkingBlocks.map((block) => ({ ...block, status: "done" as AgentBlockStatus })),
    toolCalls,
    logs,
    // 状态标注**显式传入**，刻意不从 turn.notice 继承：
    // turn.notice 会累积"等待提示"这类临时标注，若继承下来，请求最终成功时
    // 那条"已 30 秒未收到数据"会残留在快照里（明明已经成功完成）。
    noticeI18nKey,
    noticeDetail,
    // 时长同上：只落调用方给的确定值，不让快照自己按 createdAt 反推
    durationMs: durationMs ?? undefined,
    finalContent,
    createdAt: now(),
  };
}
