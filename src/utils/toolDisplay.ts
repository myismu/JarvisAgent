import { i18n } from "../i18n";
import type { AgentDisplayMode, AgentToolCallView, AgentToolStatus } from "../types";

const t = i18n.global.t;

export type ToolDisplayCategory =
  | "task"
  | "file"
  | "search"
  | "command"
  | "subagent"
  | "planning"
  | "memory"
  | "system"
  | "other";

export interface ToolActionSummary {
  key: string;
  label: string;
  status: AgentToolStatus;
  count: number;
  unit: string;
  summary: string;
  names: string[];
}

export interface ToolCallGroup {
  id: string;
  key: string;
  name: string;
  category: ToolDisplayCategory;
  categoryLabel: string;
  tools: AgentToolCallView[];
  actions: ToolActionSummary[];
  status: AgentToolStatus;
  count: number;
  timestamp: number;
  updatedAt: number;
  summary: string;
}

interface ToolDescriptor {
  category: ToolDisplayCategory;
  key: string;
}

const DEFAULT_DESCRIPTOR: ToolDescriptor = {
  category: "other",
  key: "default",
};

const TASK_UPDATE_DESCRIPTOR: ToolDescriptor = {
  category: "task",
  key: "updateTask",
};

const TASK_DEPENDENCY_DESCRIPTOR: ToolDescriptor = {
  category: "task",
  key: "taskDependency",
};

const TOOL_DESCRIPTORS: Record<string, ToolDescriptor> = {
  createtask: { category: "task", key: "createTask" },
  updatetask: TASK_UPDATE_DESCRIPTOR,
  deletetask: { category: "task", key: "deleteTask" },
  listtasks: { category: "task", key: "listTasks" },
  gettask: { category: "task", key: "getTask" },
  summarizetasks: { category: "task", key: "summarizeTasks" },
  updatetodos: { category: "task", key: "updateTodos" },
  runsubagentssequentially: { category: "task", key: "runSubagentsSequentially" },
  runsubagent: { category: "subagent", key: "runSubagent" },
  proposeplan: { category: "planning", key: "proposePlan" },
  readfile: { category: "file", key: "readFile" },
  readfileskeleton: { category: "file", key: "readFileSkeleton" },
  writefile: { category: "file", key: "writeFile" },
  editfile: { category: "file", key: "editFile" },
  editnotebook: { category: "file", key: "editNotebook" },
  listdirectory: { category: "file", key: "listDirectory" },
  searchrepo: { category: "search", key: "searchRepo" },
  searchtext: { category: "search", key: "searchText" },
  findfiles: { category: "search", key: "findFiles" },
  searchtools: { category: "search", key: "searchTools" },
  runcommand: { category: "command", key: "runCommand" },
  startbackgroundcommand: { category: "command", key: "startBackgroundCommand" },
  checkbackgroundcommand: { category: "command", key: "checkBackgroundCommand" },
  rungitcommand: { category: "command", key: "runGitCommand" },
  loadskill: { category: "memory", key: "loadSkill" },
  compactconversation: { category: "memory", key: "compactConversation" },
  consolidatememory: { category: "memory", key: "consolidateMemory" },
  setworkspace: { category: "system", key: "setWorkspace" },
};

function translate(key: string, params?: Record<string, unknown>) {
  return t(key, params ?? {}) as string;
}

function descriptorText(descriptor: ToolDescriptor, field: string, params?: Record<string, unknown>) {
  return translate(`tools.actions.${descriptor.key}.${field}`, params);
}

function categoryText(category: ToolDisplayCategory) {
  return translate(`tools.categories.${category}`);
}

function defaultActionText(status: AgentToolStatus) {
  return translate(`tools.status.${status}`);
}

function normalizeToolName(name: string) {
  const trimmed = (name || "").trim();
  return trimmed || "unknown_tool";
}

function toolKey(name: string) {
  return normalizeToolName(name).toLowerCase();
}

function hasDependencyUpdate(tools: AgentToolCallView[]) {
  return tools.some((tool) => {
    const text = `${tool.input || ""}\n${tool.output || ""}`.toLowerCase();
    return (
      text.includes("add_blocked_by") ||
      text.includes("add_blocks") ||
      text.includes("blockedby") ||
      text.includes("blocked_by") ||
      text.includes("dependency") ||
      text.includes("dependencies") ||
      text.includes("依赖")
    );
  });
}

function descriptorForTools(name: string, tools: AgentToolCallView[] = []): ToolDescriptor {
  const key = toolKey(name);
  if (key === "updatetask" && hasDependencyUpdate(tools)) {
    return TASK_DEPENDENCY_DESCRIPTOR;
  }
  return TOOL_DESCRIPTORS[key] ?? DEFAULT_DESCRIPTOR;
}

function groupingKeyForTool(tool: AgentToolCallView) {
  const descriptor = descriptorForTools(tool.name, [tool]);
  return descriptor.category === "other" ? `other:${toolKey(tool.name)}` : descriptor.category;
}

function actionKeyForTool(tool: AgentToolCallView) {
  const descriptor = descriptorForTools(tool.name, [tool]);
  return `${descriptor.category}:${descriptor.key}`;
}

function actionVerb(descriptor: ToolDescriptor, status: AgentToolStatus) {
  return descriptorText(descriptor, `${status}Verb`);
}

function phraseWithoutCount(descriptor: ToolDescriptor, status: AgentToolStatus) {
  return descriptorText(descriptor, `${status}Phrase`);
}

function formatCount(count: number, unit: string) {
  return translate("tools.summary.count", { count, unit: unit || translate("tools.units.time") });
}

function commandFromInput(tool: AgentToolCallView) {
  const input = parseInputSummary(tool.input);
  if (!input || typeof input !== "object" || Array.isArray(input)) return null;

  if (typeof input.command === "string" && input.command.trim()) {
    return input.command.trim();
  }

  if (Array.isArray(input.args) && input.args.length) {
    const args = input.args
      .filter((item): item is string => typeof item === "string")
      .map((item) => item.trim())
      .filter(Boolean);
    if (args.length) return `git ${args.join(" ")}`;
  }

  return null;
}

function labelTargetFromInput(tool: AgentToolCallView) {
  const input = parseInputSummary(tool.input);
  if (!input || typeof input !== "object" || Array.isArray(input)) return null;

  const candidates = [
    "name",
    "title",
    "path",
    "notebook_path",
    "pattern",
    "dir",
    "task_id",
    "id",
  ];
  for (const key of candidates) {
    const value = input[key];
    if (typeof value === "string" && value.trim()) return value.trim();
    if (typeof value === "number" && Number.isFinite(value)) return String(value);
  }
  return null;
}

function parseInputSummary(summary?: string): Record<string, unknown> | null {
  if (!summary) return null;
  const trimmed = summary.trim();
  if (!trimmed || trimmed.endsWith("...")) return null;
  try {
    const parsed = JSON.parse(trimmed);
    return parsed && typeof parsed === "object" && !Array.isArray(parsed)
      ? (parsed as Record<string, unknown>)
      : null;
  } catch {
    return null;
  }
}

function commandSentence(tool: AgentToolCallView, descriptor: ToolDescriptor, status: AgentToolStatus) {
  const command = commandFromInput(tool);
  if (!command) return null;

  if (descriptor.key === "startBackgroundCommand") {
    return descriptorText(descriptor, `${status}Target`, { command });
  }

  if (descriptor.key === "checkBackgroundCommand") {
    return descriptorText(descriptor, `${status}Phrase`);
  }

  return descriptorText(descriptor, `${status}Target`, { command });
}

function targetedSentence(tool: AgentToolCallView, descriptor: ToolDescriptor, status: AgentToolStatus) {
  if (descriptor.category === "command") {
    return commandSentence(tool, descriptor, status);
  }

  // 文件/搜索/系统类工具：提取目标路径/名称
  const target = labelTargetFromInput(tool);
  if (target) {
    const targeted = descriptorText(descriptor, `${status}Target`, { target });
    // i18n fallback: 如果没有定义 xxxTarget，返回 null 回退通用文本
    if (targeted && !targeted.startsWith("tools.actions.")) return targeted;
    return null;
  }

  return null;
}

export function aggregateToolStatus(tools: AgentToolCallView[]): AgentToolStatus {
  if (tools.some((tool) => tool.status === "error")) return "error";
  if (tools.some((tool) => tool.status === "running")) return "running";
  if (tools.some((tool) => tool.status === "pending")) return "pending";
  return "completed";
}

function buildActionSummary(tools: AgentToolCallView[]): ToolActionSummary {
  const first = tools[0];
  const descriptor = descriptorForTools(first?.name ?? "", tools);
  const status = aggregateToolStatus(tools);
  const count = tools.length;
  const singleTargeted = count === 1 && first ? targetedSentence(first, descriptor, status) : null;
  const completed = tools.filter((tool) => tool.status === "completed").length;
  const verb = actionVerb(descriptor, status);
  const unit = descriptorText(descriptor, "unit");
  const countLabel =
    status === "running" && completed > 0 && completed < count
      ? translate("tools.summary.progress", { completed, count, unit })
      : count > 1
        ? formatCount(count, unit)
        : "";
  const summary = singleTargeted ?? (countLabel ? translate("tools.summary.verbCount", { verb, count: countLabel }) : phraseWithoutCount(descriptor, status));
  const names = Array.from(new Set(tools.map((tool) => normalizeToolName(tool.name))));

  return {
    key: actionKeyForTool(first),
    label: descriptorText(descriptor, "action"),
    status,
    count,
    unit,
    summary,
    names,
  };
}

function buildActionSummaries(tools: AgentToolCallView[]) {
  const buckets = new Map<string, AgentToolCallView[]>();

  tools.forEach((tool) => {
    const key = actionKeyForTool(tool);
    const bucket = buckets.get(key);
    if (bucket) {
      bucket.push(tool);
    } else {
      buckets.set(key, [tool]);
    }
  });

  return Array.from(buckets.values()).map(buildActionSummary);
}

function summarizeGroup(actions: ToolActionSummary[]) {
  return actions.map((action) => action.summary).join(translate("tools.summary.separator"));
}

export function createToolCallGroup(tools: AgentToolCallView[]): ToolCallGroup {
  const firstTool = tools[0];
  const descriptor = descriptorForTools(firstTool?.name ?? "", tools);
  const category = descriptor.category;
  const actions = buildActionSummaries(tools);
  const timestamps = tools.map((tool) => tool.timestamp || 0);
  const updates = tools.map((tool) => tool.updatedAt || tool.timestamp || 0);
  const timestamp = timestamps.length ? Math.min(...timestamps) : 0;
  const updatedAt = updates.length ? Math.max(...updates) : timestamp;
  const key = firstTool ? groupingKeyForTool(firstTool) : "other:empty";

  return {
    id: `${key}-${firstTool?.id ?? "empty"}`,
    key,
    name: firstTool ? normalizeToolName(firstTool.name) : "unknown_tool",
    category,
    categoryLabel: categoryText(category),
    tools,
    actions,
    status: aggregateToolStatus(tools),
    count: tools.length,
    timestamp,
    updatedAt,
    summary: summarizeGroup(actions),
  };
}

export function canMergeToolGroups(a: ToolCallGroup, b: ToolCallGroup) {
  const firstA = a.tools[0];
  const firstB = b.tools[0];
  // Loop ids can advance while one logical task batch is still streaming.
  return Boolean(firstA && firstB && a.key === b.key);
}

export function mergeToolGroups(a: ToolCallGroup, b: ToolCallGroup) {
  return createToolCallGroup([...a.tools, ...b.tools]);
}

export function groupAdjacentToolCalls(tools: AgentToolCallView[]): ToolCallGroup[] {
  const groups: ToolCallGroup[] = [];

  tools.forEach((tool) => {
    const next = createToolCallGroup([tool]);
    const previous = groups[groups.length - 1];

    if (previous && canMergeToolGroups(previous, next)) {
      groups[groups.length - 1] = mergeToolGroups(previous, next);
      return;
    }

    groups.push(next);
  });

  return groups;
}

export function toolActionLabel(name: string, status: AgentToolStatus, tool?: AgentToolCallView) {
  const descriptor = descriptorForTools(name, tool ? [tool] : []);
  const targeted = tool ? targetedSentence(tool, descriptor, status) : null;
  if (targeted) return targeted;
  return TOOL_DESCRIPTORS[toolKey(name)]
    ? phraseWithoutCount(descriptor, status)
    : defaultActionText(status);
}

export function toolGroupTitle(group: ToolCallGroup) {
  return group.categoryLabel;
}

export function toolCategoryLabel(name: string) {
  const descriptor = descriptorForTools(name);
  return categoryText(descriptor.category);
}

export function toolGroupActionLabel(group: ToolCallGroup) {
  return group.summary || defaultActionText(group.status);
}

export function toolActionCountLabel(action: ToolActionSummary) {
  return action.count > 1 ? translate("tools.summary.actionCount", { action: action.label, count: action.count }) : action.label;
}

export function summarizeToolGroupsForPanel(groups: ToolCallGroup[], totalCount: number) {
  if (!groups.length) return translate("execution.noToolActivity");
  // 优先展示具体工具操作摘要（如"读取 main.rs"），而非笼统分类标签（如"文件操作"）
  const summaries: string[] = [];
  for (const group of groups) {
    for (const action of group.actions) {
      if (action.summary) summaries.push(action.summary);
    }
  }
  if (summaries.length > 0) {
    const sep = translate("tools.summary.labelSeparator");
    const visible = summaries.slice(0, 4).join(sep);
    const text = summaries.length > 4
      ? translate("tools.summary.moreCategories", { count: summaries.length })
      : visible;
    return translate("tools.summary.panel", { categories: text, count: totalCount });
  }
  // 回退：无具体摘要时用分类标签
  const labels = Array.from(new Set(groups.map((group) => group.categoryLabel)));
  const visible = labels.slice(0, 3).join(translate("tools.summary.labelSeparator"));
  return translate("tools.summary.panel", { categories: visible, count: totalCount });
}

/** 将全量参数/输出截断为摘要文本（前端自行控制显示长度） */
export function truncateToolContent(content: string | undefined, maxLen = 120): string {
  if (!content) return "";
  const s = content.trim();
  if (s.length <= maxLen) return s;
  return s.slice(0, maxLen) + "...";
}

export function hasToolDetails(tool: AgentToolCallView) {
  return Boolean(tool.input || tool.output || tool.error || tool.logs.length);
}

export function hasToolGroupDetails(group: ToolCallGroup) {
  return group.count > 1 || group.actions.length > 0 || group.tools.some(hasToolDetails);
}

export function isSubAgentToolName(name: string) {
  const normalized = toolKey(name);
  return normalized === "runsubagent" || normalized.includes("subagent");
}

export function isSubAgentToolGroup(group: ToolCallGroup) {
  return group.category === "subagent" || group.tools.some((tool) => isSubAgentToolName(tool.name));
}

export function shouldOpenToolGroup(group: ToolCallGroup, mode: AgentDisplayMode) {
  if (!hasToolGroupDetails(group)) return false;
  if (group.status === "error") return true;
  // 开发者模式：始终展开，看到全部细节
  if (mode === "developer") return true;
  // 用户模式：仅运行中展开
  return ["running", "pending"].includes(group.status);
}
