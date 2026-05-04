import type { AgentDisplayMode, AgentToolCallView, AgentToolStatus } from "../types";

type ToolActionLabels = Record<AgentToolStatus, string>;

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
  action: string;
  unit: string;
  pendingVerb: string;
  runningVerb: string;
  completedVerb: string;
  errorVerb: string;
}

const CATEGORY_LABELS: Record<ToolDisplayCategory, string> = {
  task: "任务管理",
  file: "文件操作",
  search: "搜索检索",
  command: "命令执行",
  subagent: "子代理",
  planning: "方案审批",
  memory: "记忆上下文",
  system: "系统设置",
  other: "工具活动",
};

const DEFAULT_ACTION_LABELS: ToolActionLabels = {
  pending: "等待执行",
  running: "正在执行",
  completed: "已完成",
  error: "执行失败",
};

const DEFAULT_DESCRIPTOR: ToolDescriptor = {
  category: "other",
  action: "执行工具",
  unit: "次",
  pendingVerb: "等待执行",
  runningVerb: "正在执行",
  completedVerb: "已完成",
  errorVerb: "执行失败",
};

const TASK_UPDATE_DESCRIPTOR: ToolDescriptor = {
  category: "task",
  action: "更新任务",
  unit: "个任务",
  pendingVerb: "等待更新",
  runningVerb: "正在更新",
  completedVerb: "已更新",
  errorVerb: "更新失败",
};

const TASK_DEPENDENCY_DESCRIPTOR: ToolDescriptor = {
  category: "task",
  action: "设置依赖",
  unit: "条依赖",
  pendingVerb: "等待设置",
  runningVerb: "正在设置",
  completedVerb: "已设置",
  errorVerb: "设置失败",
};

const TOOL_DESCRIPTORS: Record<string, ToolDescriptor> = {
  createtask: {
    category: "task",
    action: "创建任务",
    unit: "个任务",
    pendingVerb: "等待创建",
    runningVerb: "正在创建",
    completedVerb: "已创建",
    errorVerb: "创建失败",
  },
  updatetask: TASK_UPDATE_DESCRIPTOR,
  deletetask: {
    category: "task",
    action: "删除任务",
    unit: "个任务",
    pendingVerb: "等待删除",
    runningVerb: "正在删除",
    completedVerb: "已删除",
    errorVerb: "删除失败",
  },
  listtasks: {
    category: "task",
    action: "查看任务",
    unit: "次",
    pendingVerb: "等待查看",
    runningVerb: "正在查看",
    completedVerb: "已查看",
    errorVerb: "查看失败",
  },
  gettask: {
    category: "task",
    action: "读取任务",
    unit: "个任务",
    pendingVerb: "等待读取",
    runningVerb: "正在读取",
    completedVerb: "已读取",
    errorVerb: "读取失败",
  },
  summarizetasks: {
    category: "task",
    action: "汇总任务",
    unit: "次",
    pendingVerb: "等待汇总",
    runningVerb: "正在汇总",
    completedVerb: "已汇总",
    errorVerb: "汇总失败",
  },
  updatetodos: {
    category: "task",
    action: "更新待办",
    unit: "次",
    pendingVerb: "等待更新",
    runningVerb: "正在更新",
    completedVerb: "已更新",
    errorVerb: "更新失败",
  },
  runsubagentssequentially: {
    category: "task",
    action: "调度任务",
    unit: "次",
    pendingVerb: "等待调度",
    runningVerb: "正在调度",
    completedVerb: "已调度",
    errorVerb: "调度失败",
  },
  runsubagent: {
    category: "subagent",
    action: "启动子代理",
    unit: "个任务",
    pendingVerb: "等待启动",
    runningVerb: "正在执行",
    completedVerb: "已完成",
    errorVerb: "执行失败",
  },
  proposeplan: {
    category: "planning",
    action: "提交方案",
    unit: "份方案",
    pendingVerb: "等待提交",
    runningVerb: "正在提交",
    completedVerb: "已提交",
    errorVerb: "提交失败",
  },
  readfile: {
    category: "file",
    action: "读取文件",
    unit: "个文件",
    pendingVerb: "等待读取",
    runningVerb: "正在读取",
    completedVerb: "已读取",
    errorVerb: "读取失败",
  },
  readfileskeleton: {
    category: "file",
    action: "读取结构",
    unit: "个文件",
    pendingVerb: "等待读取",
    runningVerb: "正在读取",
    completedVerb: "已读取",
    errorVerb: "读取失败",
  },
  writefile: {
    category: "file",
    action: "写入文件",
    unit: "个文件",
    pendingVerb: "等待写入",
    runningVerb: "正在写入",
    completedVerb: "已写入",
    errorVerb: "写入失败",
  },
  editfile: {
    category: "file",
    action: "修改文件",
    unit: "个文件",
    pendingVerb: "等待修改",
    runningVerb: "正在修改",
    completedVerb: "已修改",
    errorVerb: "修改失败",
  },
  editnotebook: {
    category: "file",
    action: "编辑 Notebook",
    unit: "个单元",
    pendingVerb: "等待编辑",
    runningVerb: "正在编辑",
    completedVerb: "已编辑",
    errorVerb: "编辑失败",
  },
  listdirectory: {
    category: "file",
    action: "查看目录",
    unit: "个目录",
    pendingVerb: "等待查看",
    runningVerb: "正在查看",
    completedVerb: "已查看",
    errorVerb: "查看失败",
  },
  searchrepo: {
    category: "search",
    action: "搜索代码",
    unit: "次",
    pendingVerb: "等待搜索",
    runningVerb: "正在搜索",
    completedVerb: "已搜索",
    errorVerb: "搜索失败",
  },
  searchtext: {
    category: "search",
    action: "搜索文本",
    unit: "次",
    pendingVerb: "等待搜索",
    runningVerb: "正在搜索",
    completedVerb: "已搜索",
    errorVerb: "搜索失败",
  },
  findfiles: {
    category: "search",
    action: "匹配文件",
    unit: "次",
    pendingVerb: "等待匹配",
    runningVerb: "正在匹配",
    completedVerb: "已匹配",
    errorVerb: "匹配失败",
  },
  searchtools: {
    category: "search",
    action: "查找工具",
    unit: "次",
    pendingVerb: "等待查找",
    runningVerb: "正在查找",
    completedVerb: "已查找",
    errorVerb: "查找失败",
  },
  runcommand: {
    category: "command",
    action: "运行命令",
    unit: "条命令",
    pendingVerb: "等待运行",
    runningVerb: "正在运行",
    completedVerb: "运行成功",
    errorVerb: "运行失败",
  },
  startbackgroundcommand: {
    category: "command",
    action: "启动后台命令",
    unit: "条命令",
    pendingVerb: "等待启动",
    runningVerb: "正在启动",
    completedVerb: "已启动",
    errorVerb: "启动失败",
  },
  checkbackgroundcommand: {
    category: "command",
    action: "检查后台任务",
    unit: "次",
    pendingVerb: "等待检查",
    runningVerb: "正在检查",
    completedVerb: "已检查",
    errorVerb: "检查失败",
  },
  rungitcommand: {
    category: "command",
    action: "运行 Git",
    unit: "条命令",
    pendingVerb: "等待运行",
    runningVerb: "正在运行",
    completedVerb: "运行成功",
    errorVerb: "运行失败",
  },
  loadskill: {
    category: "memory",
    action: "加载技能",
    unit: "个技能",
    pendingVerb: "等待加载",
    runningVerb: "正在加载",
    completedVerb: "已加载",
    errorVerb: "加载失败",
  },
  compactconversation: {
    category: "memory",
    action: "压缩上下文",
    unit: "次",
    pendingVerb: "等待压缩",
    runningVerb: "正在压缩",
    completedVerb: "已压缩",
    errorVerb: "压缩失败",
  },
  consolidatememory: {
    category: "memory",
    action: "整理记忆",
    unit: "次",
    pendingVerb: "等待整理",
    runningVerb: "正在整理",
    completedVerb: "已整理",
    errorVerb: "整理失败",
  },
  setworkspace: {
    category: "system",
    action: "设置工作区",
    unit: "个目录",
    pendingVerb: "等待设置",
    runningVerb: "正在设置",
    completedVerb: "已设置",
    errorVerb: "设置失败",
  },
  getsysteminfo: {
    category: "system",
    action: "读取系统信息",
    unit: "次",
    pendingVerb: "等待读取",
    runningVerb: "正在读取",
    completedVerb: "已读取",
    errorVerb: "读取失败",
  },
};

function normalizeToolName(name: string) {
  const trimmed = (name || "").trim();
  return trimmed || "unknown_tool";
}

function toolKey(name: string) {
  return normalizeToolName(name).toLowerCase();
}

function hasDependencyUpdate(tools: AgentToolCallView[]) {
  return tools.some((tool) => {
    const text = `${tool.inputSummary || ""}\n${tool.outputSummary || ""}`.toLowerCase();
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
  return `${descriptor.category}:${descriptor.action}`;
}

function actionVerb(descriptor: ToolDescriptor, status: AgentToolStatus) {
  if (status === "pending") return descriptor.pendingVerb;
  if (status === "running") return descriptor.runningVerb;
  if (status === "completed") return descriptor.completedVerb;
  return descriptor.errorVerb;
}

function actionObject(action: string) {
  return action
    .replace(
      /^(创建|更新|删除|查看|读取|写入|修改|编辑|搜索|匹配|查找|运行|启动|检查|加载|压缩|整理|设置|提交|调度|汇总)/,
      "",
    )
    .trim();
}

function phraseWithoutCount(descriptor: ToolDescriptor, status: AgentToolStatus) {
  const object = actionObject(descriptor.action) || descriptor.action;
  const verb = actionVerb(descriptor, status);

  if (status === "error") return `${object}${verb}`;
  if (descriptor.category === "command" && status === "completed") return `${object}${verb}`;
  return `${verb}${object}`;
}

function formatCount(count: number, unit: string) {
  return `${count} ${unit || "次"}`;
}

function commandFromInput(tool: AgentToolCallView) {
  const input = parseInputSummary(tool.inputSummary);
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
  const input = parseInputSummary(tool.inputSummary);
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

  if (descriptor.action === "启动后台命令") {
    if (status === "pending") return `等待启动 ${command}`;
    if (status === "running") return `正在启动 ${command}`;
    if (status === "completed") return `${command} 已启动`;
    return `${command} 启动失败`;
  }

  if (descriptor.action === "检查后台任务") {
    if (status === "pending") return "等待检查后台任务";
    if (status === "running") return "正在检查后台任务";
    if (status === "completed") return "已检查后台任务";
    return "后台任务检查失败";
  }

  if (status === "pending") return `等待运行 ${command}`;
  if (status === "running") return `正在运行 ${command}`;
  if (status === "completed") return `${command} 运行成功`;
  return `${command} 运行失败`;
}

function targetedSentence(tool: AgentToolCallView, descriptor: ToolDescriptor, status: AgentToolStatus) {
  if (descriptor.category === "command") {
    return commandSentence(tool, descriptor, status);
  }

  if (descriptor.action === "加载技能") {
    const target = labelTargetFromInput(tool);
    if (!target) return null;
    if (status === "pending") return `等待加载 ${target}`;
    if (status === "running") return `正在加载 ${target}`;
    if (status === "completed") return `已加载 ${target}`;
    return `${target} 加载失败`;
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
  const countLabel =
    status === "running" && completed > 0 && completed < count
      ? `${completed}/${count} ${descriptor.unit}`
      : count > 1
        ? formatCount(count, descriptor.unit)
        : "";
  const summary = singleTargeted ?? (countLabel ? `${verb} ${countLabel}` : phraseWithoutCount(descriptor, status));
  const names = Array.from(new Set(tools.map((tool) => normalizeToolName(tool.name))));

  return {
    key: actionKeyForTool(first),
    label: descriptor.action,
    status,
    count,
    unit: descriptor.unit,
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
  return actions.map((action) => action.summary).join("，");
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
    categoryLabel: CATEGORY_LABELS[category],
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
    : DEFAULT_ACTION_LABELS[status];
}

export function toolGroupTitle(group: ToolCallGroup) {
  return group.categoryLabel;
}

export function toolCategoryLabel(name: string) {
  const descriptor = descriptorForTools(name);
  return CATEGORY_LABELS[descriptor.category];
}

export function toolGroupActionLabel(group: ToolCallGroup) {
  return group.summary || DEFAULT_ACTION_LABELS[group.status];
}

export function toolActionCountLabel(action: ToolActionSummary) {
  return action.count > 1 ? `${action.label} × ${action.count}` : action.label;
}

export function summarizeToolGroupsForPanel(groups: ToolCallGroup[], totalCount: number) {
  if (!groups.length) return "无工具活动";
  const labels = Array.from(new Set(groups.map((group) => group.categoryLabel)));
  const visible = labels.slice(0, 3).join("、");
  const suffix = labels.length > 3 ? `等 ${labels.length} 类活动` : visible;
  return `${suffix} · ${totalCount} 步`;
}

export function hasToolDetails(tool: AgentToolCallView) {
  return Boolean(tool.inputSummary || tool.outputSummary || tool.error || tool.logs.length);
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
  return mode === "developer" && ["running", "pending"].includes(group.status);
}


