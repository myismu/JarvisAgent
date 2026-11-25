/**
 * agentTurnState.ts 单元测试
 *
 * 覆盖单轮 Agent 流式执行状态机的核心行为：
 * 1. 流式文本与思考块的追加、切换与封口
 * 2. 同名工具调用通过 toolCallId 精确匹配（防止结果串台）
 * 3. 收尾快照的正文合并（mergeFinalText 四种分支）与临时标注隔离
 */
import { describe, it, expect } from "vitest";
import {
  createEmptyAgentCurrentTurn,
  beginAgentLoop,
  appendAgentText,
  appendAgentThinking,
  markAgentToolActivity,
  upsertAgentToolCall,
  applyAgentStepToCurrentTurn,
  finishAgentLoop,
  hasStructuredTurnContent,
  noticeTextFromBackend,
  buildAgentTurnSnapshot,
} from "../agentTurnState";

describe("流式块追加与循环切换", () => {
  it("同 kind 连续追加到同一个文本块，切换 kind 或工具活动后开启新块", () => {
    const view = { currentTurn: createEmptyAgentCurrentTurn() };
    beginAgentLoop(view, 1);

    appendAgentThinking(view, "先读代码");
    appendAgentThinking(view, "再分析");
    expect(view.currentTurn.thinkingBlocks).toHaveLength(1);
    expect(view.currentTurn.thinkingBlocks[0].content).toBe("先读代码再分析");

    appendAgentText(view, "第一段", "assistant");
    appendAgentText(view, "继续", "assistant");
    expect(view.currentTurn.textBlocks).toHaveLength(1);
    expect(view.currentTurn.textBlocks[0].content).toBe("第一段继续");

    // 发生工具活动：封口当前文本与思考块
    markAgentToolActivity(view, 1);
    expect(view.currentTurn.textBlocks[0].status).toBe("done");
    expect(view.currentTurn.thinkingBlocks[0].status).toBe("done");

    // 再次追加正文必须新开块，不能写回旧块
    appendAgentText(view, "第二段", "assistant");
    expect(view.currentTurn.textBlocks).toHaveLength(2);
    expect(view.currentTurn.textBlocks[1].content).toBe("第二段");
    expect(hasStructuredTurnContent(view.currentTurn)).toBe(true);
  });
});

describe("applyAgentStepToCurrentTurn 工具调用匹配", () => {
  it("同一轮多次同名 RunCommand 按 toolCallId 精确匹配，不互相覆盖", () => {
    const view = { currentTurn: createEmptyAgentCurrentTurn() };
    beginAgentLoop(view, 1);

    applyAgentStepToCurrentTurn(view, {
      type: "tool_call",
      toolCallId: "call_1",
      tool: "RunCommand",
      content: '{"command":"echo 1"}',
      timestamp: 1000,
    });
    applyAgentStepToCurrentTurn(view, {
      type: "tool_call",
      toolCallId: "call_2",
      tool: "RunCommand",
      content: '{"command":"echo 2"}',
      timestamp: 1001,
    });

    // 第一条报错、第二条成功：必须严格落在各自的条目上
    applyAgentStepToCurrentTurn(view, {
      type: "tool_error",
      toolCallId: "call_1",
      tool: "RunCommand",
      error: "permission denied",
      content: "exit 1",
      timestamp: 1002,
    });
    applyAgentStepToCurrentTurn(view, {
      type: "tool_result",
      toolCallId: "call_2",
      tool: "RunCommand",
      content: "2",
      timestamp: 1003,
    });

    const first = view.currentTurn.toolCalls.find((t) => t.id === "call_1");
    const second = view.currentTurn.toolCalls.find((t) => t.id === "call_2");
    expect(first?.status).toBe("error");
    expect(first?.error).toBe("permission denied");
    expect(second?.status).toBe("completed");
    expect(second?.output).toBe("2");
  });

  it("缺少 toolCallId 的旧数据按最后一条同名工具兜底更新", () => {
    const view = { currentTurn: createEmptyAgentCurrentTurn() };
    upsertAgentToolCall(view, "legacy_1", "ReadFile", "running", 1);
    applyAgentStepToCurrentTurn(view, {
      type: "tool_result",
      tool: "ReadFile",
      content: "file content",
      timestamp: 1004,
    });

    expect(view.currentTurn.toolCalls[0].status).toBe("completed");
    expect(view.currentTurn.toolCalls[0].output).toBe("file content");
  });

  it("finishAgentLoop(hasTool=false) 会将残留 running 工具收口为 completed", () => {
    const view = { currentTurn: createEmptyAgentCurrentTurn() };
    upsertAgentToolCall(view, "c1", "ReadFile", "running", 1);
    finishAgentLoop(view, false);

    expect(view.currentTurn.isRunning).toBe(false);
    expect(view.currentTurn.toolCalls[0].status).toBe("completed");
  });
});

describe("buildAgentTurnSnapshot 正文合并与临时状态隔离", () => {
  it("分支 1：无流式正文时整段补入 finalContent", () => {
    const turn = createEmptyAgentCurrentTurn();
    const snap = buildAgentTurnSnapshot(turn, "最终回复", "", "completed");
    expect(snap.textBlocks).toHaveLength(1);
    expect(snap.textBlocks[0].content).toBe("最终回复");
  });

  it("分支 2：流式正文已完整覆盖 finalContent 时不重复追加", () => {
    const view = { currentTurn: createEmptyAgentCurrentTurn() };
    appendAgentText(view, "过程说明。", "assistant", 1);
    markAgentToolActivity(view, 1);
    appendAgentText(view, "最终结论", "assistant", 2);

    const snap = buildAgentTurnSnapshot(view.currentTurn, "最终结论", "", "completed");
    expect(snap.textBlocks).toHaveLength(2);
    expect(snap.textBlocks[1].content).toBe("最终结论");
  });

  it("分支 3：finalContent 以已有正文为前缀时，只把差集接到末尾", () => {
    const view = { currentTurn: createEmptyAgentCurrentTurn() };
    appendAgentText(view, "前半段", "assistant", 1);

    const snap = buildAgentTurnSnapshot(view.currentTurn, "前半段后半段", "", "completed");
    expect(snap.textBlocks).toHaveLength(1);
    expect(snap.textBlocks[0].content).toBe("前半段后半段");
  });

  it("分支 4：流式正文与最终正文实质不同，追加完整版本且保留中间叙述", () => {
    const view = { currentTurn: createEmptyAgentCurrentTurn() };
    appendAgentText(view, "我先查一下日志", "assistant", 1);

    const snap = buildAgentTurnSnapshot(view.currentTurn, "问题已修复完成", "", "completed");
    expect(snap.textBlocks).toHaveLength(2);
    expect(snap.textBlocks[0].content).toBe("我先查一下日志");
    expect(snap.textBlocks[1].content).toBe("问题已修复完成");
  });

  it("不继承 turn.notice 临时等待提示，且仅在无执行记录时落入 fallbackExecution", () => {
    const view = { currentTurn: createEmptyAgentCurrentTurn() };
    view.currentTurn.notice = "已 30 秒未收到数据";

    const snap = buildAgentTurnSnapshot(
      view.currentTurn,
      "完成",
      "兜底执行摘要",
      "completed",
      undefined,
      undefined,
      1234,
    );
    expect(snap.noticeI18nKey).toBeUndefined();
    expect(snap.durationMs).toBe(1234);
    expect(snap.logs).toHaveLength(1);
    expect(snap.logs[0].content).toBe("兜底执行摘要");
  });
});

describe("noticeTextFromBackend", () => {
  it("空 key 返回 undefined；有 detail 时附加详情文案", () => {
    expect(noticeTextFromBackend(null)).toBeUndefined();
    expect(noticeTextFromBackend("")).toBeUndefined();

    const plain = noticeTextFromBackend("notice.streamTimeout");
    const withDetail = noticeTextFromBackend("notice.streamTimeout", "read timeout");
    expect(plain).toBeTruthy();
    expect(withDetail).toContain("read timeout");
    expect(withDetail!.length).toBeGreaterThan(plain!.length);
  });
});
