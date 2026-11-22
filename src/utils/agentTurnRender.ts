import type { AgentExecutionLog, AgentToolCallView } from "../types";

/**
 * Agent 轮次渲染的工具函数。
 *
 * 这里**只保留活的那一半**：流式与历史两条路径现在都由 Vue 组件渲染
 * （`components/chat/AgentTurn.vue` 消费 `buildDeveloperTimeline`），
 * 曾经的「HTML 字符串渲染」实现（`renderAgentTurnSnapshot` / `renderExecutionPanel` /
 * `extractInterruptNotice` 等一整套，由当时的 `utils/historyRender.ts::renderStoredHistory`
 * 调用）已随死代码清理删除——它没有调用方，历史消息早已改走组件路径。
 * 注意：`utils/historyRender.ts` 这个文件**本身也已不存在**，此处提及仅为记录来龙去脉，
 * 不要再去按这个路径找代码。
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

/** 执行日志单行摘要：`tag` 为行首方括号标签原文，`text` 为剥离标签后的正文摘要。 */
export interface LogLineSummary {
  /** 摘要正文（已剥离标记与行首标签，必要时截断并追加 `...`） */
  text: string;
  /** 行首方括号标签原文（如「等待确认」）；行首没有标签时为空串 */
  tag: string;
}

/**
 * 把一条执行日志（后端下发的 Markdown 文本）压成一行摘要，供日志折叠行显示。
 *
 * 用途：执行过程里日志条目与「思考 / 工具」使用同一套折叠行视觉，
 * 摘要行只需要一行文字 + 一个可选标签，不再把整段 Markdown 直接铺开。
 *
 * 处理顺序（顺序本身有依赖，不要随意调整）：
 * 1. 按 `\n` 切分，取首个 `trim()` 后非空的行（日志常以空行或缩进开头）；
 * 2. 循环剥离行首引用符 `>` 与列表符 `-`，允许 `> -`、`- >` 等叠加形态；
 * 3. 去掉 `**` 与反引号——必须在第 4 步前做，否则 `**[标签]**` 匹配不到标签；
 * 4. 连续空白合并为单个空格；
 * 5. 匹配行首方括号标签（正则 `^\[([^\]]{1,12})\]` 后接可选空白）：命中则写入 `tag` 并从正文剔除
 *    （长度上限 12 是刻意的：更长的方括号多为正文内容而非标签）；
 * 6. 正文超过 `maxLen` 时截断为前 `maxLen` 个字符并追加 `...`；
 * 7. 正文为空时回退为 `tag` 原文，`tag` 也为空则回退为空串（避免摘要行为空）。
 *
 * 边界：纯函数、无副作用、不做渲染；只取首行，多行日志的后续行不会进入摘要
 * （完整内容由折叠展开后的正文承担）。`maxLen` 小于 1 时按 1 处理。
 */
export function summarizeLogLine(content: string, maxLen = 80): LogLineSummary {
  const limit = Math.max(1, maxLen);

  // 1) 首个非空行：逐行 trim 后取第一个非空行，整条日志为空时退化为空串。
  const firstLine = content
    .split("\n")
    .map((line) => line.trim())
    .find((line) => line.length > 0) ?? "";

  // 2) 循环剥离行首引用符与列表符：一轮可能剥掉 `> -`，再检查是否还有嵌套。
  let line = firstLine;
  let previous = "";
  while (previous !== line) {
    previous = line;
    line = line.replace(/^>\s*/, "").replace(/^-\s+/, "");
  }

  // 3) 去掉强调标记与行内代码标记；4) 合并连续空白。
  //    顺序要求见函数注释：先清标记，后面才能正确识别 `**[标签]**`。
  line = line
    .replace(/\*\*/g, "")
    .replace(/`/g, "")
    .replace(/\s+/g, " ")
    .trim();

  // 5) 行首方括号标签：命中后从正文剔除，标签单独交给摘要行渲染。
  let tag = "";
  const tagMatch = line.match(/^\[([^\]]{1,12})\]\s*/);
  if (tagMatch) {
    tag = tagMatch[1].trim();
    line = line.slice(tagMatch[0].length).trim();
  }

  // 6) 超长截断：先截前 limit 个字符再补省略号（总长度可能略超 maxLen）。
  if (line.length > limit) {
    line = `${line.slice(0, limit)}...`;
  }

  // 7) 正文为空则回退标签原文，两者都空时返回空串。
  return { text: line || tag, tag };
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
