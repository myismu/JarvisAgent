/**
 * toolDisplay.ts 单元测试
 *
 * 验证工具展示分类、延迟工具（ExecuteTool）解包、状态聚合优先级、
 * 相邻工具分组归并、以及行内仅在 error 态自动展开的折叠口径。
 */
import { describe, it, expect } from "vitest";
import type { AgentToolCallView } from "../../types";
import {
  unwrapDeferredTool,
  aggregateToolStatus,
  groupAdjacentToolCalls,
  createToolCallGroup,
  hasToolGroupDetails,
  shouldOpenToolGroup,
  truncateToolContent,
  isSubAgentToolName,
  toolDisplayCategory,
} from "../toolDisplay";

/** 构造最小工具调用对象，减少测试样板代码 */
function makeTool(overrides: Partial<AgentToolCallView> = {}): AgentToolCallView {
  return {
    id: overrides.id ?? "t1",
    loop: overrides.loop ?? 1,
    name: overrides.name ?? "ReadFile",
    status: overrides.status ?? "completed",
    logs: overrides.logs ?? [],
    timestamp: overrides.timestamp ?? 1000,
    updatedAt: overrides.updatedAt ?? 1000,
    input: overrides.input,
    output: overrides.output,
    error: overrides.error,
  };
}

describe("unwrapDeferredTool", () => {
  it("非 ExecuteTool 原样返回工具名与解析后的参数", () => {
    const tool = makeTool({ name: "ReadFile", input: '{"path":"src/main.ts"}' });
    const res = unwrapDeferredTool(tool);
    expect(res.innerName).toBeNull();
    expect(res.displayName).toBe("ReadFile");
    expect(res.input).toEqual({ path: "src/main.ts" });
  });

  it("ExecuteTool 解包内部工具名与 args 参数", () => {
    const tool = makeTool({
      name: "ExecuteTool",
      input: JSON.stringify({ name: "WriteFile", args: { path: "a.ts" } }),
    });
    const res = unwrapDeferredTool(tool);
    expect(res.innerName).toBe("WriteFile");
    expect(res.displayName).toContain("WriteFile");
    expect(res.input).toEqual({ path: "a.ts" });
  });

  it("被截断的 input（以 ... 结尾）或非法 JSON 不抛异常", () => {
    const truncated = makeTool({ name: "ExecuteTool", input: '{"name":"WriteFile"...' });
    expect(unwrapDeferredTool(truncated).innerName).toBeNull();

    const invalid = makeTool({ name: "ExecuteTool", input: "{not json" });
    expect(unwrapDeferredTool(invalid).innerName).toBeNull();
  });
});

describe("aggregateToolStatus", () => {
  it("严格遵循 error > running > pending > completed 优先级", () => {
    expect(
      aggregateToolStatus([
        makeTool({ status: "completed" }),
        makeTool({ status: "running" }),
        makeTool({ status: "error" }),
      ]),
    ).toBe("error");

    expect(
      aggregateToolStatus([
        makeTool({ status: "completed" }),
        makeTool({ status: "pending" }),
        makeTool({ status: "running" }),
      ]),
    ).toBe("running");

    expect(
      aggregateToolStatus([
        makeTool({ status: "completed" }),
        makeTool({ status: "pending" }),
      ]),
    ).toBe("pending");

    expect(aggregateToolStatus([makeTool({ status: "completed" })])).toBe("completed");
  });
});

describe("groupAdjacentToolCalls", () => {
  it("相邻同分类工具（含 ExecuteTool 包装的同类工具）自动合并，异类拆分", () => {
    const tools = [
      makeTool({ id: "1", name: "ReadFile", timestamp: 100, updatedAt: 150 }),
      makeTool({
        id: "2",
        name: "ExecuteTool",
        input: JSON.stringify({ name: "EditFile", args: { path: "a.ts" } }),
        timestamp: 200,
        updatedAt: 250,
      }),
      makeTool({ id: "3", name: "RunCommand", timestamp: 300, updatedAt: 350 }),
      makeTool({ id: "4", name: "ReadFile", timestamp: 400, updatedAt: 450 }),
    ];

    const groups = groupAdjacentToolCalls(tools);
    expect(groups).toHaveLength(3);
    expect(groups[0].category).toBe("file");
    expect(groups[0].count).toBe(2);
    expect(groups[0].timestamp).toBe(100);
    expect(groups[0].updatedAt).toBe(250);
    expect(groups[1].category).toBe("command");
    expect(groups[2].category).toBe("file");
  });
});

describe("折叠与展开口径", () => {
  it("行内仅在 error 状态自动展开，completed/running 均保持收起", () => {
    const completedGroup = createToolCallGroup([
      makeTool({ status: "completed", output: "ok" }),
    ]);
    const runningGroup = createToolCallGroup([
      makeTool({ status: "running", input: '{"path":"a.ts"}' }),
    ]);
    const errorGroup = createToolCallGroup([
      makeTool({ status: "error" }), // 即使无 output/error 文本，失败本身也必须可展开
    ]);

    expect(shouldOpenToolGroup(completedGroup, "developer")).toBe(false);
    expect(shouldOpenToolGroup(runningGroup, "user")).toBe(false);
    expect(hasToolGroupDetails(errorGroup)).toBe(true);
    expect(shouldOpenToolGroup(errorGroup, "user")).toBe(true);
    expect(shouldOpenToolGroup(errorGroup, "developer")).toBe(true);
  });
});

describe("辅助工具函数", () => {
  it("truncateToolContent 截断超长文本并保留短文本", () => {
    expect(truncateToolContent(undefined)).toBe("");
    expect(truncateToolContent("  short  ", 10)).toBe("short");
    expect(truncateToolContent("12345678901", 10)).toBe("1234567890...");
  });

  it("isSubAgentToolName 与 toolDisplayCategory 正确识别分类", () => {
    expect(isSubAgentToolName("RunSubagent")).toBe(true);
    expect(isSubAgentToolName("RunSubagentsSequentially")).toBe(true);
    expect(isSubAgentToolName("ReadFile")).toBe(false);

    expect(toolDisplayCategory("ReadFile")).toBe("file");
    expect(toolDisplayCategory("RunCommand")).toBe("command");
    expect(toolDisplayCategory("NonExistentTool")).toBe("other");
  });
});
