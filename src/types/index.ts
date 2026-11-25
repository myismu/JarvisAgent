/**
 * # index.ts — 前端跨模块共享的数据契约入口
 *
 * 统一重导出由 Rust 侧 `ts-rs` 自动生成的跨端 DTO（见 `./generated/contracts.ts`），
 * 并定义纯前端视图模型（如 `AgentCurrentTurn`、`SubAgentConversationItem`、`AppView`）。
 *
 * ## Key Exports
 * - 跨端命令与事件契约：全部从 `./generated/contracts` 重导出（单一事实来源）
 * - 纯前端视图类型：`AgentCurrentTurn`、`AgentTurnSnapshot`、`SubAgentConversationItem`、`AppView`
 *
 * ## Constraints
 * - 跨端结构禁止在此手工重写，变更 Rust DTO 后请运行 `pnpm gen:types` 同步
 */
// === 核心会话类型 ===
// ProjectMeta / SessionMeta / JarvisResult / ContextSectionSnapshot / CacheHitPoint /
// SessionContextSnapshot 等跨端类型已全部由 Rust 侧 ts-rs 自动生成（见 ./generated/contracts.ts）。

/**
 * 单一调用来源（谁发起的请求）的用量分组。
 *
 * 之所以按来源拆开：同一个会话里主 Agent、子 Agent、内部系统调用（标题生成、
 * 压缩摘要、反思审查等）的 token 会混进同一个累计值，只看总量无法判断
 * 「钱花在谁身上」。分组是**后端的口径**，前端只负责展示，不重新聚合。
 *
 * 全部是**累计绝对值**（会话级），不是单次请求的增量。
 */
/**
 * 会话用量按来源分组的快照与视图（由 Rust 侧 `ts-rs` 自动生成，单一事实来源）。
 *
 * 对应 `src-tauri/src/core/session/mod.rs` 中的 `UsageSourceView` /
 * `SessionUsageBreakdownView` / `SessionUsageView`，改 Rust 结构体后
 * `cargo test` 会直接校验 `src/types/generated/contracts.ts` 是否同步。
 *
 * 归一化（缺字段补 0）由 `utils/usageBreakdown.ts` 统一负责，
 * 事件监听与 store 写入都走那一层，避免各处各写一遍兜底逻辑。
 */
import type {
  ProjectMeta,
  SessionMeta,
  BatchOpFailure,
  BatchOpResult,
  UsageSource,
  UsageSourceAuditRow,
  UsageBreakdown,
  UsageSourceView,
  SessionUsageBreakdownView,
  SessionUsageView as GeneratedSessionUsageView,
  SessionThinkingSnapshot,
  ShellMode,
  ShellSettings,
  ShellResolution,
  AgentConfig,
  ModelProfile,
  AppConfig,
  ImageCompressConfig,
  CustomWindowState,
  UiPreferences,
  PromptMeta,
  PromptDetail,
  ToolMeta,
  SkillMeta,
  SkillDetail,
  ModelCapabilities,
  ModelRegistryEntry,
  ResolvePermissionResult,
  PendingPermissionItem,
  SessionAllowance,
  PermissionStateView,
  SessionPermissionSettingsView,
  PermissionResolvedPayload,
  JarvisResult,
  ContextSectionSnapshot,
  CacheHitPoint,
  SessionContextSnapshot,
  InterruptKind,
  HistoryTurnTokens,
  HistoryTextBlock,
  HistoryThinkingBlock,
  HistoryToolCallView,
  HistoryExecutionLog,
  HistoryTurnSnapshot,
  SessionMessage,
  PagedSessionMessages,
  PlanTask,
  PlanDocument,
  TodoStatus,
  TodoProgress,
  TodoItem,
  TodoGroup,
  TaskStatus,
  Task,
  AgentRunStatus,
  AgentRun,
  AgentRunLoopEvent,
  ResumeAgentRunPlan,
  SubAgentStatus,
  SubAgentPhase,
  SubAgentRun,
  SubAgentTraceEvent,
  SubAgentTraceRunSummary,
  BackgroundTask,
  Notification as BackgroundTaskNotification,
  OpType,
  FileOperation,
  Checkpoint,
  Branch,
  BranchInfo,
  CheckpointTree,
  RollbackRecallResult,
  RollbackPreviewResult,
  DiffLine,
  DiffHunk,
  TextDiff,
  Patch,
  PatchSummary,
  FileInfo,
  WorkspaceState,
  Workspace,
  Snapshot,
  SnapshotBranch,
  SnapshotNode,
  BranchView,
  SnapshotTreeView,
  SnapshotSummary,
  SandboxStatus,
  AgentSandbox,
  SandboxComparison,
  ConflictType,
  ConflictResolution,
  Conflict,
  MergeResult,
  ChatTurnStartEvent,
  ChatTurnEndEvent,
  ChatContentEvent,
  ChatThinkingEvent,
  ChatStreamEvent,
  ChatToolStartEvent,
  ChatToolDebugEvent,
  AgentStepEvent,
  PermissionRequestEvent,
  PermissionResolvedEvent,
  PlanProposalEvent,
  PlanProposalStreamEvent,
  ApprovalModeChangedEvent,
  AgentReadOnlyChangedEvent,
  AgentWorkModeChangedEvent,
  SessionAllowancesChangedEvent,
  SessionThinkingModeChangedEvent,
  SessionMessagesChangedEvent,
  BgCompactingChangedEvent,
  BackgroundFailedEvent,
  CheckpointCreatedEvent,
  SnapshotCreatedEvent,
  MemoryUpdatedEvent,
  SubagentTraceUpdatedEvent,
  TodoUpdateEvent,
  UserQuestionOption,
  UserQuestionItem,
  UserQuestionRequestEvent,
  UserQuestionResolvedEvent,
  UserQuestionAnswerItem,
  ResolveUserQuestionResult,
} from "./generated/contracts";

export type UsageSourceBreakdown = UsageSourceView;
export type SessionUsageBreakdown = SessionUsageBreakdownView;
export type SessionUsageView = GeneratedSessionUsageView;
export type {
  ProjectMeta,
  SessionMeta,
  BatchOpFailure,
  BatchOpResult,
  UsageSource,
  UsageSourceAuditRow,
  UsageBreakdown,
  SessionThinkingSnapshot,
  ShellMode,
  ShellSettings,
  ShellResolution,
  AgentConfig,
  ModelProfile,
  AppConfig,
  ImageCompressConfig,
  CustomWindowState,
  UiPreferences,
  PromptMeta,
  PromptDetail,
  ToolMeta,
  SkillMeta,
  SkillDetail,
  ModelCapabilities,
  ModelRegistryEntry,
  ResolvePermissionResult,
  PendingPermissionItem,
  SessionAllowance,
  PermissionStateView,
  SessionPermissionSettingsView,
  PermissionResolvedPayload,
  JarvisResult,
  ContextSectionSnapshot,
  CacheHitPoint,
  SessionContextSnapshot,
  InterruptKind,
  HistoryTurnTokens,
  HistoryTextBlock,
  HistoryThinkingBlock,
  HistoryToolCallView,
  HistoryExecutionLog,
  HistoryTurnSnapshot,
  SessionMessage,
  PagedSessionMessages,
  PlanTask,
  PlanDocument,
  TodoStatus,
  TodoProgress,
  TodoItem,
  TodoGroup,
  TaskStatus,
  Task,
  AgentRunStatus,
  AgentRun,
  AgentRunLoopEvent,
  ResumeAgentRunPlan,
  SubAgentStatus,
  SubAgentPhase,
  SubAgentRun,
  SubAgentTraceEvent,
  SubAgentTraceRunSummary,
  BackgroundTask,
  BackgroundTaskNotification,
  OpType,
  FileOperation,
  Checkpoint,
  Branch,
  BranchInfo,
  CheckpointTree,
  RollbackRecallResult,
  RollbackPreviewResult,
  DiffLine,
  DiffHunk,
  TextDiff,
  Patch,
  PatchSummary,
  FileInfo,
  WorkspaceState,
  Workspace,
  Snapshot,
  SnapshotBranch,
  SnapshotNode,
  BranchView,
  SnapshotTreeView,
  SnapshotSummary,
  SandboxStatus,
  AgentSandbox,
  SandboxComparison,
  ConflictType,
  ConflictResolution,
  Conflict,
  MergeResult,
  ChatTurnStartEvent,
  ChatTurnEndEvent,
  ChatContentEvent,
  ChatThinkingEvent,
  ChatStreamEvent,
  ChatToolStartEvent,
  ChatToolDebugEvent,
  AgentStepEvent,
  PermissionRequestEvent,
  PermissionResolvedEvent,
  PlanProposalEvent,
  PlanProposalStreamEvent,
  ApprovalModeChangedEvent,
  AgentReadOnlyChangedEvent,
  AgentWorkModeChangedEvent,
  SessionAllowancesChangedEvent,
  SessionThinkingModeChangedEvent,
  SessionMessagesChangedEvent,
  BgCompactingChangedEvent,
  BackgroundFailedEvent,
  CheckpointCreatedEvent,
  SnapshotCreatedEvent,
  MemoryUpdatedEvent,
  SubagentTraceUpdatedEvent,
  TodoUpdateEvent,
  UserQuestionOption,
  UserQuestionItem,
  UserQuestionRequestEvent,
  UserQuestionResolvedEvent,
  UserQuestionAnswerItem,
  ResolveUserQuestionResult,
};

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
  /**
   * 按来源分组的明细（可选）。
   *
   * 同样是**绝对值**：接收方整值覆盖即可，不要自行累加，否则数字会翻倍。
   * 旧后端不推送该字段，此时前端按空值归零处理且不报错。
   */
  breakdown?: SessionUsageBreakdown | null;
}

// TodoItem / TodoProgress / TodoGroup 已迁移至 ./generated/contracts.ts 自动生成。

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

// PlanDocument / BackgroundTask 已迁移至 ./generated/contracts.ts 自动生成。

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
 * 权限档位：决定"改动前问得多严"。
 * - request_approval：请求审批 —— 改文件/删文件/跑命令一律先问（默认）
 * - auto_approve：帮我批准 —— 只有风险操作（删除/覆盖/改名/跑命令/批量）才问
 * - full_approve：完全批准 —— 一切改动/删除/改名/命令都不再问（危险命令仍保留一次确认），
 *   比 auto_approve 更松，唯一的回滚依据是项目的回滚快照与回收站
 */
export type AgentApprovalMode = "request_approval" | "auto_approve" | "full_approve";

/**
 * **全局设置**（UiPreferences）里的权限档位默认值：只有两档。
 *
 * 为什么不是 AgentApprovalMode：full_approve 会跳过几乎所有确认，只能由用户在
 * **绑定项目的会话里**临时开启（回滚快照与回收站是它的安全垫，独立会话没有）。
 * 若允许它进全局默认，新建的独立会话就会落在一个没有任何回滚依据的档位上。
 * 后端 `UiPreferences.agent_approval_mode` 的收敛口径同样是这两档。
 */
export type AgentApprovalDefaultMode = Exclude<AgentApprovalMode, "full_approve">;

// SessionAllowance 已迁移至 ./generated/contracts.ts 自动生成。

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

// AgentRunStatus / AgentRun / AgentRunLoopEvent / SubAgentStatus / SubAgentPhase /
// SubAgentRun / SubAgentTraceEvent / SubAgentTraceRunSummary 已迁移至 ./generated/contracts.ts。

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

// 检查点 / 快照 / 沙箱 / 合并 / Skill / Prompt 等跨端 DTO 均已迁移至 ./generated/contracts.ts。

export type AppView = 'chat' | 'skill-manager' | 'archive';
