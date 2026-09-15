// === 核心会话类型 ===

export interface ProjectMeta {
  id: string;
  name: string;
  path: string;
  createdAt: number;
  updatedAt: number;
  sessionCount: number;
}

export interface SessionMeta {
  id: string;
  title: string;
  createdAt: number;
  updatedAt: number;
  messageCount: number;
  isSmartNamed?: boolean;
  profileId?: string | null;
  totalInputTokens?: number;
  totalOutputTokens?: number;
  titleSource?: string;
  projectId?: string | null;
  workingDirectory?: string | null;
  lastModel?: string | null;
  lastTool?: string | null;
  toolCallCount?: number;
  runCount?: number;
  checkpointCount?: number;
}

export interface SessionListFilter {
  keyword?: string | null;
  fromTs?: number | null;
  toTs?: number | null;
  profileId?: string | null;
  model?: string | null;
  tool?: string | null;
  hasToolCalls?: boolean | null;
  limit?: number | null;
  offset?: number | null;
}

export interface JarvisResult {
  status: string;
  content: string;
  input_tokens: number;
  output_tokens: number;
  session_input_tokens: number;
  session_output_tokens: number;
  user_message_id?: string | null;
  /** 本轮最终采用的深度思考状态（后端裁决层给出，前端不自行判断） */
  thinking_enabled?: boolean | null;
  /** 裁决原因（如 `ClampedByForced` / `SessionNever` / `ProfileDefault`） */
  thinking_reason?: string | null;
  /** 需提示用户时的 i18n key（如模型强制思考夹紧了用户的"关闭"意愿） */
  thinking_notice_i18n_key?: string | null;
}

export interface ContextSectionSnapshot {
  key: string;
  label: string;
  chars: number;
  estimatedTokens: number;
  tokenCountMethod: "tokenizer" | "estimate" | string;
  itemCount: number;
  content: string;
  truncated: boolean;
  /** 原始 JSON 数据（仅 messages 和 tools section 有值） */
  rawContent?: string | null;
}

/** 单个 loop 的缓存命中记录 */
export interface CacheHitPoint {
  loopCount: number;
  hitTokens: number;
  missTokens: number;
  /** 命中的字段名；服务商未报告时为 null */
  source?: string | null;
}

export interface SessionContextSnapshot {
  sessionId: string;
  runId?: string | null;
  loopCount: number;
  model: string;
  intent: string;
  apiFormat: string;
  createdAt: number;
  totalChars: number;
  estimatedTokens: number;
  providerInputTokens?: number | null;
  providerOutputTokens?: number | null;
  providerTotalTokens?: number | null;
  /** 缓存命中 / 未命中的输入 token；null/缺省 = 该模型或链路未报告（≠ 0 命中） */
  cacheHitTokens?: number | null;
  cacheMissTokens?: number | null;
  /** 命中的字段名，如 prompt_cache_hit_tokens / cached_tokens / cache_read_input_tokens */
  cacheSource?: string | null;
  /** 逐 loop 的缓存命中趋势（最近 N 条），用于画预热 → 命中的演进 */
  cacheHistory?: CacheHitPoint[];
  driftPercent?: number | null;
  maxContextTokens?: number | null;
  maxOutputTokens: number;
  messageCount: number;
  toolSchemaCount: number;
  toolCallCount: number;
  toolResultCount: number;
  sections: ContextSectionSnapshot[];
}

export interface TodoItem {
  id: string;
  content: string;
  activeForm: string;
  text?: string;
  status: "pending" | "in_progress" | "completed";
}

export interface TodoUpdatePayload {
  todos: TodoItem[];
  sessionId: string;
}

export interface PermissionRequest {
  id: string;
  message: string;
  sessionId?: string;
  kind?: "tool" | "loop_continuation" | string;
  allowSession?: boolean;
}

export interface PlanProposal {
  id: string;
  title: string;
  content: string;
  sessionId?: string;
}

export type PlanDocumentStatus = "pending" | "approved" | "rejected" | "revision_requested";

export interface PlanDocument {
  id: string;
  sessionId: string;
  title: string;
  content: string;
  status: PlanDocumentStatus | string;
  path?: string | null;
  createdAt: number;
  updatedAt: number;
  decidedAt?: number | null;
  /** 用户拒绝方案时的修改意见（与 content 分离，不覆盖原始方案） */
  rejectionFeedback?: string | null;
}

export interface BackgroundTask {
  id: string;
  sessionId?: string | null;
  command: string;
  status: string;
  result?: string | null;
  port?: number | null;
  taskType?: string | null;
  task_type?: string | null;
}

// === Agent 执行追踪类型 ===

export type AgentStepType =
  | "thinking"
  | "plan"
  | "tool_call"
  | "tool_result"
  | "tool_error"
  | "subagent_start"
  | "subagent_end"
  | "task_scheduled"
  | "task_completed"
  | "retry"
  | "cancelled"
  | "interrupted"
  /** 长时间未收到数据时的等待提示（渲染为气泡下方小字，不写正文） */
  | "waiting_hint";

export interface AgentStep {
  type: AgentStepType;
  tool?: string;
  content?: string;
  error?: string;
  task?: string;
  taskId?: number;
  subject?: string;
  status?: string;
  attempt?: number;
  max?: number;
  timestamp: number;
}

export type AgentDisplayMode = "user" | "developer";

/** 用户类型（谁在用）→ 影响 UI 渲染细节和交流风格 */
export type AgentAudience = "user" | "developer";

/**
 * 工作模式（在干什么）→ 影响工具集和系统提示词。
 * 只有 edit / plan；历史上的 "chat（只读保护）" 已在第二步取消。
 */
export type AgentWorkMode = "edit" | "plan";

/** 用户在界面上可选择的工作模式 */
export type AgentUserMode = "edit" | "plan";

/**
 * 权限档位（第二步新增）：决定"改动前问得多严"。
 * - request_approval：请求审批 —— 改文件/删文件/跑命令一律先问（默认）
 * - auto_approve：帮我批准 —— 只有风险操作（删除/覆盖/改名/跑命令/批量）才问
 */
export type AgentApprovalMode = "request_approval" | "auto_approve";

/** 本会话已允许的范围（工具 + 范围） */
export interface SessionAllowance {
  tool: string;
  scope: string;
  label: string;
}

export type AgentTextBlockKind = "assistant" | "tool_stream" | "system";
export type AgentBlockStatus = "streaming" | "done";
export type AgentToolStatus = "pending" | "running" | "completed" | "error";

export interface AgentTextBlock {
  id: string;
  loop: number;
  kind: AgentTextBlockKind;
  content: string;
  status: AgentBlockStatus;
  timestamp: number;
}

export interface AgentThinkingBlock {
  id: string;
  loop: number;
  content: string;
  status: AgentBlockStatus;
  timestamp: number;
}

export interface AgentExecutionLog {
  id: string;
  loop: number;
  content: string;
  timestamp: number;
}

export interface AgentToolCallView {
  id: string;
  loop: number;
  name: string;
  status: AgentToolStatus;
  input?: string;
  output?: string;
  error?: string;
  logs: string[];
  timestamp: number;
  updatedAt: number;
}

export interface AgentCurrentTurn {
  id: string;
  loop: number;
  revision: number;
  isRunning: boolean;
  hasToolActivity: boolean;
  activeTextBlockId: string | null;
  activeThinkingBlockId: string | null;
  textBlocks: AgentTextBlock[];
  thinkingBlocks: AgentThinkingBlock[];
  toolCalls: AgentToolCallView[];
  logs: AgentExecutionLog[];
  tokens?: AgentTurnTokens;
  /**
   * 本轮的状态标注（气泡下方小字）：等待提示、中断/取消说明等。
   *
   * 与 textBlocks 的区别是**展示位置**——textBlocks 渲染在回复气泡内，
   * notice 渲染在气泡下方。运行状态信息一律走这里，避免挤进正文
   * 看起来像模型自己说的话。
   */
  notice?: string;
  startedAt: number | null;
}

export interface AgentTurnTokens {
  input: number;
  output: number;
  sessionInput?: number;
  sessionOutput?: number;
}

export interface AgentTurnSnapshot {
  version: 1;
  status: string;
  textBlocks: AgentTextBlock[];
  thinkingBlocks: AgentThinkingBlock[];
  toolCalls: AgentToolCallView[];
  logs: AgentExecutionLog[];
  tokens?: AgentTurnTokens;
  finalContent?: string;
  notice?: string;
  createdAt: number;
}

export type AgentRunStatus =
  | "running"
  | "completed"
  | "failed"
  | "cancelled"
  | "interrupted";

export interface AgentRun {
  runId: string;
  sessionId: string;
  status: AgentRunStatus | string;
  userMessagePreview: string;
  loopCount: number;
  inputTokens: number;
  outputTokens: number;
  startedAt: number;
  updatedAt: number;
  finishedAt?: number | null;
  lastSafePoint?: string | null;
  liveThinking: string;
  liveToolBuffer: string;
  liveContent: string;
  error?: string | null;
  summary?: string | null;
  resumable: boolean;
  resumedFromRunId?: string | null;
}

export interface AgentRunEvent {
  eventId: string;
  runId: string;
  sessionId: string;
  eventType: string;
  message: string;
  tool?: string | null;
  input?: string | null;
  output?: string | null;
  error?: string | null;
  loopCount: number;
  inputTokens: number;
  outputTokens: number;
  timestamp: number;
}

export type SubAgentStatus = "running" | "completed" | "failed" | "cancelled";

export type SubAgentPhase =
  | "starting"
  | "waiting_model"
  | "streaming"
  | "thinking"
  | "calling_tool"
  | "processing_tool_result"
  | "finalizing";

export interface SubAgentRun {
  runId: string;
  sessionId: string;
  taskId?: number | null;
  label: string;
  agentRole: string;
  prompt?: string | null;
  promptPreview: string;
  readOnly: boolean;
  status: SubAgentStatus;
  phase: SubAgentPhase;
  loopCount: number;
  maxLoops: number;
  timeoutSecs: number;
  currentTool?: string | null;
  currentToolInput?: string | null;
  inputTokens: number;
  outputTokens: number;
  startedAt: number;
  updatedAt: number;
  finishedAt?: number | null;
  error?: string | null;
  summary?: string | null;
}

export type SubAgentEventType =
  | "start"
  | "phase"
  | "tool_call"
  | "tool_result"
  | "complete"
  | "cancel"
  | "error";

export interface SubAgentEvent {
  eventId: string;
  runId: string;
  sessionId: string;
  eventType: SubAgentEventType | string;
  message: string;
  tool?: string | null;
  input?: string | null;
  output?: string | null;
  error?: string | null;
  loopCount: number;
  inputTokens: number;
  outputTokens: number;
  timestamp: number;
}

// === 检查点/快照类型（旧版） ===

export type OpType = "edit" | "write" | "create" | "delete" | "rename";

export interface FileOperation {
  opType: OpType;
  path: string;
  oldContentHash?: string;
  backupPath?: string;
  newContentHash?: string;
  diffSummary?: string;
}

export interface Checkpoint {
  id: string;
  sessionId: string;
  parentId?: string;
  branchName: string;
  agentId?: string;
  workspaceId?: string;
  createdAt: number;
  triggerMessage: string;
  operations: FileOperation[];
  metadata: Record<string, string>;
}

export interface Branch {
  name: string;
  sessionId: string;
  headCheckpointId?: string;
  createdAt: number;
  agentId?: string;
  description: string;
  isActive: boolean;
}

export interface BranchInfo {
  name: string;
  headCheckpointId?: string;
  checkpointCount: number;
  isActive: boolean;
}

export interface CheckpointTree {
  sessionId: string;
  branches: BranchInfo[];
  checkpoints: Checkpoint[];
}

// === 新快照引擎类型 ===

export interface Patch {
  type: "create_file" | "delete_file" | "update_file" | "rename_file";
  path: string;
  content?: string;
  oldContent?: string;
  newContent?: string;
  oldPath?: string;
  newPath?: string;
  diff?: TextDiff;
}

export interface TextDiff {
  hunks: DiffHunk[];
}

export interface DiffHunk {
  oldStart: number;
  oldLines: number;
  newStart: number;
  newLines: number;
  lines: DiffLine[];
}

export interface DiffLine {
  type: "Context" | "Addition" | "Deletion";
  content: string;
}

export interface PatchSummary {
  path: string;
  operation: string;
  linesAdded: number;
  linesRemoved: number;
}

export interface Snapshot {
  id: string;
  parentId?: string;
  branchName: string;
  patches: Patch[];
  message?: string;
  isCheckpoint: boolean;
  workspaceState?: WorkspaceState;
  agentId?: string;
  workspaceId?: string;
  createdAt: number;
  metadata: Record<string, string>;
}

export interface WorkspaceState {
  files: Record<string, FileInfo>;
}

export interface FileInfo {
  hash: string;
  size: number;
}

export interface SnapshotSummary {
  id: string;
  message?: string;
  timestamp: number;
  isCheckpoint: boolean;
  agentId?: string;
  patchCount: number;
  patchSummary: PatchSummary[];
}

export interface SnapshotNode {
  id: string;
  message?: string;
  timestamp: number;
  isCheckpoint: boolean;
  agentId?: string;
  children: SnapshotNode[];
}

export interface BranchView {
  name: string;
  description: string;
  agentId?: string;
  isActive: boolean;
  root: SnapshotNode;
}

export interface SnapshotTreeView {
  branches: BranchView[];
  currentBranch: string;
  currentSnapshotId: string;
}

export interface Workspace {
  files: Record<string, string>;
}

// === P6: 多Agent沙箱类型 ===

export interface AgentSandbox {
  sandboxId: string;
  agentId: string;
  workspaceId: string;
  branchName: string;
  baseSnapshotId: string;
  workspacePath: string;
  status: "active" | "completed" | "published" | "abandoned";
  createdAt: number;
  description: string;
}

export interface SandboxComparison {
  sandboxId: string;
  agentId: string;
  filesChanged: number;
  linesAdded: number;
  linesRemoved: number;
  snapshotCount: number;
  lastSnapshotId: string;
  lastMessage?: string;
}

// === P7: 分支合并类型 ===

export interface MergeResult {
  success: boolean;
  targetBranch: string;
  sourceBranch: string;
  mergedSnapshotId?: string;
  conflicts: Conflict[];
  autoResolved: number;
  manualRequired: number;
}

export interface Conflict {
  path: string;
  conflictType: "both_modified" | "source_deleted" | "target_deleted" | "both_created" | "both_renamed";
  sourceContent?: string;
  targetContent?: string;
  baseContent?: string;
  resolution?: ConflictResolution;
}

export type ConflictResolution =
  | { type: "keep_source" }
  | { type: "keep_target" }
  | { type: "keep_both"; newPath: string }
  | { type: "manual"; resolvedContent: string }
  | { type: "custom"; content: string };

// === Skill 管理类型 ===

export interface SkillMeta {
  name: string;
  description: string;
  path: string;
  bodyTokens: number;
  active: boolean;
}

export interface SkillDetail {
  name: string;
  description: string;
  path: string;
  body: string;
}

export type AppView = 'chat' | 'skill-manager';
