import type { AgentExecutionLog, AgentToolCallView } from "../types";

/**
 * Agent 轮次渲染的工具函数。
 *
 * 这里**只保留活的那一半**：流式与历史两条路径现在都由 Vue 组件渲染
 * （`components/chat/AgentTurn.vue` 消费 `buildDeveloperTimeline`），
 * 曾经的「HTML 字符串渲染」实现（`renderAgentTurnSnapshot` / `renderExecutionPanel` /
 * `extractInterruptNotice` 等一整套，由 `utils/historyRender.ts::renderStoredHistory` 调用）
 * 已随死代码清理删除——它没有调用方，历史消息早已改走组件路径。
 */

export const PSEUDO_TOOL_CALL_RE = /(?:<tool_call>\s*)?<function=[\s\S]*$/;

export function stripPseudoToolCalls(content: string) {
  return content.replace(PSEUDO_TOOL_CALL_RE, "").trimEnd();
}

export function describeThinkingStatic(content: string) {
  const text = content.replace(/\s+/g, " ").trim();
  if (!text) return "分析中...";
  if (/(方案|计划|审批|plan|proposal)/i.test(text)) return "制定方案";
  if (/(工具|调用|tool|function|参数)/i.test(text)) return "选择工具";
  if (/(文件|目录|代码|实现|修改|file|code|implement)/i.test(text)) return "分析代码";
  if (/(错误|失败|修复|bug|error|fix)/i.test(text)) return "定位问题";
  if (/(测试|验证|build|check|test)/i.test(text)) return "规划验证";
  const sentence = text.split(/[。.!?？；;]/)[0]?.trim() || text;
  return sentence.length > 20 ? `${sentence.slice(0, 20)}...` : sentence;
}

export type DevTimelineItem =
  | { type: "text"; timestamp: number; content: string }
  | { type: "thinking"; timestamp: number; content: string; status: string; streaming: boolean }
  | { type: "tool"; timestamp: number; tool: AgentToolCallView; streaming: boolean }
  | { type: "log"; timestamp: number; content: string; loop: number };

/** 从快照数据构建开发者时间线（按时间戳交错排序），直播和历史共用 */
export function buildDeveloperTimeline(
  snapshot: {
    textBlocks: { content: string; timestamp: number }[];
    thinkingBlocks: { content: string; status: string; timestamp: number }[];
    toolCalls: AgentToolCallView[];
    logs: AgentExecutionLog[];
  },
  live: boolean,
): DevTimelineItem[] {
  const timeline: DevTimelineItem[] = [];

  snapshot.textBlocks.forEach((block) => {
    if (!block.content.trim()) return;
    timeline.push({ type: "text", timestamp: block.timestamp, content: block.content.trim() });
  });

  snapshot.thinkingBlocks.forEach((block) => {
    if (!block.content.trim()) return;
    timeline.push({
      type: "thinking",
      timestamp: block.timestamp,
      content: block.content,
      status: block.status,
      streaming: live && block.status === "streaming",
    });
  });

  snapshot.toolCalls.forEach((tool) => {
    timeline.push({
      type: "tool",
      timestamp: tool.timestamp,
      tool,
      streaming: live && (tool.status === "running" || tool.status === "error"),
    });
  });

  snapshot.logs.forEach((log) => {
    if (!log.content.trim()) return;
    timeline.push({
      type: "log",
      timestamp: log.timestamp,
      content: log.content,
      loop: log.loop || 1,
    });
  });

  timeline.sort((a, b) => a.timestamp - b.timestamp);
  return timeline;
}
