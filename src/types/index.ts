// === 核心会话类型 ===

export interface JarvisResult {
  status: string;
  content: string;
  input_tokens: number;
  output_tokens: number;
  session_input_tokens: number;
  session_output_tokens: number;
}

export interface TodoItem {
  id: string;
  text: string;
  status: "pending" | "in_progress" | "completed";
}

export interface PermissionRequest {
  id: string;
  message: string;
  sessionId?: string;
}

export interface PlanProposal {
  id: string;
  title: string;
  content: string;
  sessionId?: string;
}

export type PlanDocumentStatus = "pending" | "approved" | "rejected";

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
  | "retry"
  | "cancelled";

export interface AgentStep {
  type: AgentStepType;
  tool?: string;
  input_summary?: string;
  output_summary?: string;
  error?: string;
  task?: string;
  attempt?: number;
  max?: number;
  content?: string;
  timestamp: number;
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
  inputSummary?: string | null;
  outputSummary?: string | null;
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
  promptPreview: string;
  readOnly: boolean;
  status: SubAgentStatus;
  phase: SubAgentPhase;
  loopCount: number;
  maxLoops: number;
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
  inputSummary?: string | null;
  outputSummary?: string | null;
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
