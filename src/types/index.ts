/**
 * # index.ts — 前端跨模块共享的数据契约
 *
 * 定义会话、Agent、工具、运行轨迹和界面状态使用的 TypeScript 类型。
 *
 * ## Key Exports
 * - `SessionMeta`: 会话列表与元数据结构
 * - `AgentRun` / `SubAgentRun`: 主 Agent 与子 Agent 运行状态
 * - `SubAgentTraceRunSummary` / `SubAgentTraceEvent`: 子 Agent 持久化轨迹索引与事件
 *
 * ## Constraints
 * - 与 Rust/Tauri 序列化字段保持 camelCase 对齐
 */
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
  /** 会话累计缓存命中 token。0（或缺失）表示该会话从未有请求上报缓存字段 */
  totalCacheHitTokens?: number;
  /** 会话累计缓存未命中 token。与命中数一样，0 表示未报告 */
  totalCacheMissTokens?: number;
  titleSource?: string;
  projectId?: string | null;
  workingDirectory?: string | null;
  lastModel?: string | null;
  lastTool?: string | null;
  toolCallCount?: number;
  runCount?: number;
  checkpointCount?: number;
}

export interface JarvisResult {
  status: string;
  content: string;
  input_tokens: number;
  output_tokens: number;
  session_input_tokens: number;
  session_output_tokens: number;
  /** 会话累计缓存命中 / 未命中 token。两者都为 0 表示该会话从未上报过缓存字段 */
  session_cache_hit_tokens: number;
  session_cache_miss_tokens: number;
  /**
   * 后端为本轮用户消息分配的 UUID。
   *
   * 前端发消息时先在本地插一条用户消息占位（那一刻后端还没生成 ID），
   * 靠它把真实 ID 补回去，撤回按钮才会立即出现 —— 否则要等刷新从数据库重读。
   * 续跑（`resume_jarvis`）不注入用户消息，故那条路径上恒为空。
   */
  user_message_id?: string | null;
  /**
   * 本轮创建的文件检查点 id（有文件编辑的轮次由后端收尾时才创建 Git 快照）。
   *
   * 与 `user_message_id` 同一设计动机：发消息那一刻检查点尚不存在，
   * 靠返回值让前端实时回填用户消息的撤回信息，「会话和代码撤回」立即出现，
   * 不必等刷新走 `get_session_messages` 重查库。纯聊天轮次为空。
   */
  checkpoint_id?: string | null;
  /**
   * 本轮是否有文件编辑补丁（与刷新路径的 `rollback_info.has_file_edits` 同口径；
   * 快照创建失败但它仍为 true，此时回填的撤回模式是 both 而 id 为空）。
   * 仅在 `=== true` 时前端才回填 both 模式，缺省 / 纯聊天轮次不回填。
   */
  checkpoint_has_patches?: boolean | null;
  /** 本轮最终采用的深度思考状态（后端裁决层给出，前端不自行判断） */
  thinking_enabled?: boolean | null;
  /** 裁决原因（如 `ClampedByForced` / `SessionNever` / `ProfileDefault`） */
  thinking_reason?: string | null;
  /** 需提示用户时的 i18n key（如模型强制思考夹紧了用户的"关闭"意愿） */
  thinking_notice_i18n_key?: string | null;
  /**
   * 本轮状态标注的 **i18n key**（渲染在回复气泡**下方**的小字）：中断 / 取消说明等。
   *
   * 这是状态标注的**唯一来源** —— 正文不再夹带系统标记，前端也不再从正文里
   * 正则剥离（2026-09-21 删除了 `INTERRUPT_MARKER_LINE_RE` 那套）。
   *
   * 后端只给 key（如 `notice.userCancel`），**不再给文案** —— 它曾经直接返回
   * 中文句子，于是界面切成英文后中断时仍冒中文。文案统一在语言包的 `notice.*` 下。
   */
  notice_i18n_key?: string | null;
  /** 状态标注的补充数据（原样展示、不翻译，如具体错误文本） */
  notice_detail?: string | null;
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
  /**
   * 图片块数量（Image block 数），对应后端的 `image_count`。
   *
   * 与 `toolCallCount` / `toolResultCount` 同族：都是"这类内容块出现了几次"的计数，
   * 不参与分区体积合计。旧快照里没有这个键（后端用 `#[serde(default)]` 兼容），
   * 前端消费时按 0 处理，并用已废弃的 `attachments` 分区兜底（见 `ContextInspector`）。
   */
  imageCount?: number;
  sections: ContextSectionSnapshot[];
}

/**
 * `session-usage-updated` 事件的载荷：会话累计用量的**当前值**。
 *
 * 由后端在**每次请求**拿到 usage 后推送（`pipeline::update_provider_usage_snapshot`），
 * 而不是等回合收尾——所以概览栏的「累计命中」在回合进行中也会刷新。
 *
 * 注意这里是**绝对值**，不是增量：接收方整值覆盖写入即可，
 * 千万不要再累加一遍，否则数字会越滚越大。
 */
export interface SessionUsageUpdatedPayload {
  sessionId: string;
  inputTokens: number;
  outputTokens: number;
  cacheHitTokens: number;
  cacheMissTokens: number;
}

export interface TodoItem {
  id: string;
  content: string;
  activeForm: string;
  text?: string;
  status: "pending" | "in_progress" | "completed";
  /** 子任务调度进度。仅当该条目正被 RunSubagentsSequentially 驱动时有值 */
  progress?: TodoProgress | null;
}

/** 某条 todo 对应的子任务完成度 */
export interface TodoProgress {
  done: number;
  total: number;
}

/**
 * 一组待办清单。
 *
 * 一次 UpdateTodos 调用产生一组；条目 id 与已有组有交集时更新那一组，否则新建一组追加。
 * 长会话里多轮清单因此能共存，而不会互相覆盖。
 */
export interface TodoGroup {
  id: string;
  items: TodoItem[];
}

export interface TodoUpdatePayload {
  /** 所有组的条目扁平后的结果，兼容老消费者 */
  todos: TodoItem[];
  /** 分组结构；新代码优先用它渲染 */
  groups?: TodoGroup[];
  sessionId: string;
}

export interface PermissionRequest {
  id: string;
  message: string;
  sessionId?: string;
  kind?: "tool" | "loop_continuation" | string;
  allowSession?: boolean;
  /** 命中"危险命令警示"的警示文案（无则 null）：卡片据此把风险行渲染成弱警示样式 */
  warning?: string | null;
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

/**
 * 状态标注（气泡下方小字）的**来源**，用于精确清除。
 *
 * 2026-11-22 新增。背景：`notice` 是单向写入的字段，重试/等待这类**过程态**
 * 在事情办成后并无任何自清机制，会一直挂到整轮 loop 结束。现在后端在恢复点发
 * `notice_clear`，前端据 `kinds` 只清对应来源，不误伤仍在有效的反思标注。
 */
export type AgentNoticeKind = "waiting_hint" | "retry" | "reflection";

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
  /** 反思审查 Agent 的判定（后端 pipeline 发出，带 judgment / reason / suggestion） */
  | "reflection"
  | "retry"
  | "cancelled"
  | "interrupted"
  /** 长时间未收到数据时的等待提示（渲染为气泡下方小字，不写正文） */
  | "waiting_hint"
  /**
   * 过程态标注的**清除**信号（后端在恢复点发出）。
   *
   * 触发时机：API 重试成功拿到响应头、SSE 收到新数据、本轮不再等上游数据。
   * 前端收到后清掉 `currentTurn.notice` 里对应来源的标注。
   */
  | "notice_clear";

export interface AgentStep {
  type: AgentStepType;
  tool?: string;
  /**
   * 后端 `agent-step` 工具事件（tool_call / tool_result / tool_error）携带的
   * **调用唯一标识**，与 `AgentToolCallView.id` 同源（后端 tool_use_id）。
   *
   * 为什么必须有：工具名不唯一 —— 同一轮里连续两次 `RunCommand` 会产生两个同名条目，
   * 只按名字反查会把结果写到另一条上。前端据此精确寻址；字段缺失时
   * （旧数据 / 老后端）才退回按名字兜底。
   */
  toolCallId?: string;
  content?: string;
  error?: string;
  task?: string;
  taskId?: number;
  subject?: string;
  status?: string;
  attempt?: number;
  max?: number;
  /**
   * 产生该步骤的轮次（后端 `agent-step` 事件携带的 `loopCount`）。
   *
   * 类型里此前缺了它，但**运行时一直带着** —— `payloadLoop()` 读的就是它。
   * 少数事件（`task_scheduled` / `task_completed`）不发这个字段，故为可选。
   */
  loopCount?: number;
  /** 反思审查的判定，仅 `type === "reflection"` 时存在：`"ok"` / `"not_ok"` */
  judgment?: string;
  /** 反思审查给出的原因，仅 `judgment === "not_ok"` 时存在 */
  reason?: string;
  /** 反思审查给出的修正建议，仅 `judgment === "not_ok"` 时存在 */
  suggestion?: string;
  /**
   * 进度类标注的**秒数参数**，仅 `waiting_hint` / `retry` 携带。
   *
   * 后端只发数字，句子由前端 `notice.waitingHint` / `notice.retry` 拼 ——
   * 它曾经直接发拼好的中文 `content`，英文界面下会冒中文。
   */
  silentSecs?: number;
  remainSecs?: number;
  waitSecs?: number;
  /**
   * `notice_clear` 要清除的标注来源，仅 `type === "notice_clear"` 时存在。
   *
   * 为什么不无条件清空：反思标注（`reflection`）不在重试恢复的语义范围内，
   * 无条件清会把「要求改进」这句仍然有效的信息一并抹掉。
   */
  kinds?: AgentNoticeKind[];
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
  /**
   * 本轮的状态标注（气泡下方小字）：等待提示、中断/取消说明等。
   *
   * 与 textBlocks 的区别是**展示位置**——textBlocks 渲染在回复气泡内，
   * notice 渲染在气泡下方。运行状态信息一律走这里，避免挤进正文
   * 看起来像模型自己说的话。
   *
   * ⚠️ 存的是**已翻译好的成句**（历史快照直接给 key，运行期的等待提示由
   * `useAgentEvents` 现场用语言包拼好）——两者在这里统一成最终展示文本。
   */
  notice?: string;
  /**
   * 当前 `notice` 的来源，供 `notice_clear` 精确匹配。
   *
   * 仅在**运行期**写入（历史快照不带）：历史里的标注已经是定论，
   * 不该被运行中的清除事件影响。
   */
  noticeKind?: AgentNoticeKind;
  startedAt: number | null;
  /**
   * 本轮已结束时的总时长（毫秒），**唯一来源是后端**。
   *
   * 运行中为 null / 缺省——实时秒数由前端 `useRunElapsed` 按 `startedAt` 推算，
   * 结束后改用后端落库的确定值，避免前端本地时钟与后端计时口径对不上。
   */
  durationMs?: number | null;
}

export interface AgentTurnSnapshot {
  version: 1;
  status: string;
  textBlocks: AgentTextBlock[];
  thinkingBlocks: AgentThinkingBlock[];
  toolCalls: AgentToolCallView[];
  logs: AgentExecutionLog[];
  finalContent?: string;
  /** 状态标注的 i18n key（后端只给 key，文案由语言包出） */
  noticeI18nKey?: string;
  /** 状态标注的补充数据（原样展示、不翻译，如具体错误文本） */
  noticeDetail?: string;
  /**
   * 本轮总时长（毫秒），后端落库值。
   *
   * 历史快照不重新计时：前端只把它格式化后显示，不再用 `createdAt` 反推。
   * 缺省 / null 表示后端没有记录该字段（旧数据），此时不显示时长。
   */
  durationMs?: number | null;
  createdAt: number;
}

export type AgentRunStatus =
  | "running"
  | "completed"
  | "failed"
  | "cancelled"
  | "interrupted"
  | "recovering"
  | "closed";

export interface AgentRun {
  runId: string;
  sessionId: string;
  status: AgentRunStatus | string;
  messageId?: string | null;
  loopCount: number;
  inputTokens: number;
  outputTokens: number;
  startedAt: number;
  updatedAt: number;
  finishedAt?: number | null;
  lastSafePoint?: string | null;
  summary?: string | null;
  /** run 级中断类型（枚举字符串，见后端 InterruptKind）；未中断为 null */
  interruptKind?: string | null;
  /**
   * 本轮**非 Agent 责任时长**的累计值（毫秒），由后端给：等待用户批准、
   * 上游重试退避、限流等待等。前端计时（`useRunElapsed`）把它从流逝时间里扣掉，
   * 保证「耗时」只反映 Agent 真正在干活的时间。
   *
   * 缺省 / null 视为 0；后端在运行中会持续增大该值。
   */
  waitMs?: number | null;
  resumable: boolean;
  resumedFromRunId?: string | null;
}

/// 一轮（1 loop）的落库记录——后端 `agent_run_events` 的「每轮一行」。
///
/// 它是**崩溃重建的唯一数据源**：`respBlocks` 是本轮响应（正文/思考/工具调用，
/// 按产生顺序排列），`toolResults` 是本轮工具执行结果。
/// 行内可能还有帧级通道写入的半截文本（崩溃保护开启时），status 为 `streaming`。
export interface AgentRunLoopEvent {
  eventId: string;
  runId: string;
  sessionId: string;
  /** 第几轮（从 1 起），与界面显示的"第 N 轮"一致 */
  loopIndex: number;
  respBlocks: Array<Record<string, unknown>>;
  toolResults: Array<Record<string, unknown>>;
  /** "streaming" | "complete" | "interrupted" */
  status: string;
  error?: string | null;
  inputTokens: number;
  outputTokens: number;
  model?: string | null;
  /**
   * 本轮反思审查的判定：`"ok"` / `"not_ok"`；`null` / 缺省 = 本轮未触发反思。
   *
   * 与 `reason` / `suggestion` 一起由后端 `mark_loop_reflection` 写在
   * `agent_run_events` 的这一行上（v22 起落库，此前只走 `agent-step` 事件、
   * 重载即失）。界面目前尚未消费它们，字段先按数据契约补齐。
   */
  reflectionJudgment?: string | null;
  reflectionReason?: string | null;
  reflectionSuggestion?: string | null;
  startedAt: number;
  updatedAt: number;
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

export interface SubAgentTraceEvent {
  eventId: string;
  runId: string;
  sessionId: string;
  loopIndex: number;
  eventType: string;
  payload: unknown;
  model?: string | null;
  inputTokens?: number | null;
  outputTokens?: number | null;
  timestamp: number;
}

export interface SubAgentTraceRunSummary {
  runId: string;
  sessionId: string;
  parentRunId?: string | null;
  label: string;
  startedAt: number;
  loopCount: number;
}

/** 子 Agent 视图中的会话级条目；saved 表示仅从持久化轨迹恢复，运行状态不再驻留内存。 */
export interface SubAgentConversationItem extends SubAgentTraceRunSummary {
  status: SubAgentStatus | "saved";
  phase: SubAgentPhase;
  maxLoops: number;
  currentTool?: string | null;
  inputTokens: number;
  outputTokens: number;
  error?: string | null;
  summary?: string | null;
  promptPreview?: string;
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

// === 提示词管理类型 ===

export interface PromptMeta {
  path: string;
  displayName: string;
  description: string;
  /** "personal" | "base" | "audience" | "mode" | "os" | "subagent" */
  category: string;
  /** true = 进 system（新会话生效）；false = 进动态上下文（下一轮生效） */
  goesIntoSystem: boolean;
  /** true = 可选文件：没有出厂默认，未配置时为空且不会被注入 */
  optional: boolean;
  customized: boolean;
  sizeChars: number;
  /** 仅 os 分类有意义：当前编译平台的那一项 */
  isPlatformActive: boolean;
}

export interface PromptDetail {
  path: string;
  displayName: string;
  description: string;
  goesIntoSystem: boolean;
  /** true = 可选文件：没有出厂默认，未配置时为空 */
  optional: boolean;
  /** 当前生效版（磁盘版或内置版）——编辑器初始内容 */
  currentContent: string;
  /** 内置出厂版原文（恢复默认的参照） */
  embeddedContent: string;
  customized: boolean;
}
