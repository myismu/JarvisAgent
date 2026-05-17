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
  AgentTurnTokens,
} from "../types";

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

function updateLatestToolByName(
  turn: AgentCurrentTurn,
  name: string | undefined,
  patch: Partial<AgentToolCallView>,
) {
  if (!name) return false;
  const item = [...turn.toolCalls].reverse().find((tool) => tool.name === name);
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
    const updated = updateLatestToolByName(turn, step.tool, {
      input: step.content,
      status: "running",
    });
    if (!updated && step.tool) {
      upsertAgentToolCall(view, `step_${nextId("tool")}`, step.tool, "running", turn.loop);
      updateLatestToolByName(turn, step.tool, { input: step.content });
    }
  } else if (step.type === "tool_result") {
    updateLatestToolByName(turn, step.tool, {
      output: step.content,
      status: "completed",
    });
  } else if (step.type === "tool_error") {
    updateLatestToolByName(turn, step.tool, {
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

export function buildAgentTurnSnapshot(
  turn: AgentCurrentTurn,
  finalContent: string,
  fallbackExecution: string,
  tokens: AgentTurnTokens | undefined,
  status: string,
): AgentTurnSnapshot {
  completeAgentCurrentTurn({ currentTurn: turn });

  const textBlocks = turn.textBlocks.map(cloneTextBlock);
  const thinkingBlocks = turn.thinkingBlocks.map(cloneThinkingBlock);
  const toolCalls = turn.toolCalls.map(cloneToolCall);
  const logs = turn.logs.map(cloneLog);

  const visibleText = textBlocks.map((block) => block.content).join("").trim();
  if (finalContent.trim() && !visibleText) {
    textBlocks.push({
      id: nextId("text_final"),
      loop: Math.max(turn.loop, 1),
      kind: "assistant",
      content: finalContent,
      status: "done",
      timestamp: now(),
    });
  }

  const hasExecution = thinkingBlocks.length > 0 || toolCalls.length > 0 || logs.length > 0;
  if (toolCalls.length > 0) {
    const latestToolTs = Math.max(...toolCalls.map((tool) => tool.timestamp));
    for (let i = textBlocks.length - 1; i >= 0; i--) {
      if (textBlocks[i].timestamp < latestToolTs) {
        textBlocks.splice(i, 1);
      }
    }
  }
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
    tokens,
    finalContent,
    createdAt: now(),
  };
}
