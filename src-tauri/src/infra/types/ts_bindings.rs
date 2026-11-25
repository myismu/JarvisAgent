//! # ts_bindings.rs — Rust → TypeScript 跨端类型契约守卫（全量覆盖 115 个 Tauri 命令）
//!
//! 职责：将全部 115 个 Tauri 命令与核心事件涉及的跨端 Rust 结构体（90 个类型）
//! 集中导出为前端 `src/types/generated/contracts.ts`，并在 `cargo test` 中校验
//! 磁盘文件与 Rust 定义逐字节一致。
//!
//! ## 工作方式
//! - **平时（CI / 本地 `cargo test`）**：纯只读比对。若 Rust 改了字段却忘同步 TS，测试直接红灯拦截。
//! - **需要刷新时**：在 `src-tauri/` 下执行：
//!   `UPDATE_TS_BINDINGS=1 cargo test ts_contracts_are_up_to_date`
//!   （PowerShell：`$env:UPDATE_TS_BINDINGS="1"; cargo test ts_contracts_are_up_to_date`）
//!
//! ## 为什么集中生成单文件而不是散落多文件
//! 避免多个生成文件之间产生交叉相对导入；前端只需从 `@/types/generated/contracts`
//! 统一引入，所有导出类型在同一文件内自然互相引用。

use std::path::PathBuf;
use ts_rs::TS;

use crate::command::app_config::{CustomWindowState, UiPreferences};
use crate::command::checkpoint::{
    Branch as CheckpointBranch, BranchInfo, Checkpoint, CheckpointTree, FileOperation, OpType,
    RollbackPreviewResult, RollbackRecallResult,
};
use crate::command::config::ImageCompressConfig;
use crate::command::history::{
    AgentExecutionLog as HistoryExecutionLog, AgentTextBlock as HistoryTextBlock,
    AgentThinkingBlock as HistoryThinkingBlock, AgentToolCallView as HistoryToolCallView,
    AgentTurnSnapshot as HistoryTurnSnapshot, AgentTurnTokens as HistoryTurnTokens,
    PagedSessionMessages, SessionMessage,
};
use crate::command::permission::{
    PendingPermissionItem, PermissionResolvedPayload, PermissionStateView, ResolvePermissionResult,
    SessionAllowance, SessionPermissionSettingsView,
};
use crate::command::prompt::{PromptDetail, PromptMeta};
use crate::command::session::SessionThinkingSnapshot;
use crate::command::tool::ToolMeta;
use crate::core::orchestration::agent_runs::{
    AgentRun, AgentRunLoopEvent, AgentRunStatus, ResumeAgentRunPlan,
};
use crate::core::orchestration::multi_agent::merge::{
    Conflict, ConflictResolution, ConflictType, MergeResult,
};
use crate::core::orchestration::multi_agent::sandbox::{
    AgentSandbox, SandboxComparison, SandboxStatus,
};
use crate::core::orchestration::subagents::{
    SubAgentPhase, SubAgentRun, SubAgentStatus, SubAgentTraceEvent, SubAgentTraceRunSummary,
};
use crate::core::rollback::patch::{DiffHunk, DiffLine, Patch, PatchSummary, TextDiff};
use crate::core::rollback::snapshot::{
    Branch as SnapshotBranch, BranchView, FileInfo, Snapshot, SnapshotNode, SnapshotSummary,
    SnapshotTreeView, Workspace, WorkspaceState,
};
use crate::core::session::{
    BatchOpFailure, BatchOpResult, ProjectMeta, SessionMeta, SessionUsageBreakdownView,
    SessionUsageView, UsageBreakdown, UsageSource, UsageSourceBreakdown, UsageSourceView,
};
use crate::infra::background::{BackgroundTask, Notification as BackgroundTaskNotification};
use crate::infra::config::config::{AgentConfig, AppConfig, ModelProfile};
use crate::infra::llm::registry::{ModelCapabilities, ModelRegistryEntry};
use crate::infra::shell_dialect::{ShellMode, ShellResolution, ShellSettings};
use crate::infra::types::events::{
    AgentReadOnlyChangedEvent, AgentStepEvent, AgentWorkModeChangedEvent, ApprovalModeChangedEvent,
    BackgroundFailedEvent, BgCompactingChangedEvent, ChatContentEvent, ChatStreamEvent,
    ChatThinkingEvent, ChatToolDebugEvent, ChatToolStartEvent, ChatTurnEndEvent,
    ChatTurnStartEvent, CheckpointCreatedEvent, MemoryUpdatedEvent, PermissionRequestEvent,
    PermissionResolvedEvent, PlanProposalEvent, PlanProposalStreamEvent, ResolveUserQuestionResult,
    SessionAllowancesChangedEvent, SessionMessagesChangedEvent, SessionThinkingModeChangedEvent,
    SnapshotCreatedEvent, SubagentTraceUpdatedEvent, TodoUpdateEvent, UserQuestionAnswerItem,
    UserQuestionItem, UserQuestionOption, UserQuestionRequestEvent, UserQuestionResolvedEvent,
};
use crate::infra::types::models::{
    CacheHitPoint, ContextSectionSnapshot, InterruptKind, JarvisResult, PlanDocument, PlanTask,
    SessionContextSnapshot, SkillDetail, SkillMeta, Task, TaskStatus, TodoGroup, TodoItem,
    TodoProgress, TodoStatus,
};

/// 统一渲染所有纳入契约守卫的 TypeScript 类型定义（固定 LF 换行）。
fn render_contracts_ts() -> String {
    let decls = [
        // 会话元数据、批量操作与用量视图契约（core/session/mod.rs）
        ProjectMeta::decl(),
        SessionMeta::decl(),
        BatchOpFailure::decl(),
        BatchOpResult::decl(),
        UsageSource::decl(),
        UsageSourceBreakdown::decl(),
        UsageBreakdown::decl(),
        UsageSourceView::decl(),
        SessionUsageBreakdownView::decl(),
        SessionUsageView::decl(),
        SessionThinkingSnapshot::decl(),
        // Shell 方言设置与探测状态契约（infra/shell_dialect.rs）
        ShellMode::decl(),
        ShellSettings::decl(),
        ShellResolution::decl(),
        // 配置、窗口状态与 UI 偏好契约（infra/config & command/app_config & command/config）
        AgentConfig::decl(),
        ModelProfile::decl(),
        AppConfig::decl(),
        ImageCompressConfig::decl(),
        CustomWindowState::decl(),
        UiPreferences::decl(),
        // 提示词、工具、技能与模型能力注册表契约
        PromptMeta::decl(),
        PromptDetail::decl(),
        ToolMeta::decl(),
        SkillMeta::decl(),
        SkillDetail::decl(),
        ModelCapabilities::decl(),
        ModelRegistryEntry::decl(),
        // 权限状态与决议契约（command/permission.rs）
        ResolvePermissionResult::decl(),
        PendingPermissionItem::decl(),
        SessionAllowance::decl(),
        PermissionStateView::decl(),
        SessionPermissionSettingsView::decl(),
        PermissionResolvedPayload::decl(),
        // 核心 Agent 返回与上下文快照契约（infra/types/models.rs）
        JarvisResult::decl(),
        ContextSectionSnapshot::decl(),
        CacheHitPoint::decl(),
        SessionContextSnapshot::decl(),
        InterruptKind::decl(),
        // 历史消息与分页契约（command/history.rs）
        HistoryTurnTokens::decl(),
        HistoryTextBlock::decl(),
        HistoryThinkingBlock::decl(),
        HistoryToolCallView::decl(),
        HistoryExecutionLog::decl(),
        HistoryTurnSnapshot::decl(),
        SessionMessage::decl(),
        PagedSessionMessages::decl(),
        // 方案、待办与任务契约（infra/types/models.rs）
        PlanTask::decl(),
        PlanDocument::decl(),
        TodoStatus::decl(),
        TodoProgress::decl(),
        TodoItem::decl(),
        TodoGroup::decl(),
        TaskStatus::decl(),
        Task::decl(),
        // Agent / 子 Agent 运行态契约（core/orchestration/）
        AgentRunStatus::decl(),
        AgentRun::decl(),
        AgentRunLoopEvent::decl(),
        ResumeAgentRunPlan::decl(),
        SubAgentStatus::decl(),
        SubAgentPhase::decl(),
        SubAgentRun::decl(),
        SubAgentTraceEvent::decl(),
        SubAgentTraceRunSummary::decl(),
        // 后台任务与通知契约（infra/background.rs）
        BackgroundTask::decl(),
        BackgroundTaskNotification::decl(),
        // 检查点与回滚预览契约（command/checkpoint.rs）
        OpType::decl(),
        FileOperation::decl(),
        Checkpoint::decl(),
        CheckpointBranch::decl(),
        BranchInfo::decl(),
        CheckpointTree::decl(),
        RollbackRecallResult::decl(),
        RollbackPreviewResult::decl(),
        // 快照引擎与补丁契约（core/rollback/）
        DiffLine::decl(),
        DiffHunk::decl(),
        TextDiff::decl(),
        Patch::decl(),
        PatchSummary::decl(),
        FileInfo::decl(),
        WorkspaceState::decl(),
        Workspace::decl(),
        Snapshot::decl(),
        SnapshotBranch::decl(),
        SnapshotNode::decl(),
        BranchView::decl(),
        SnapshotTreeView::decl(),
        SnapshotSummary::decl(),
        // 多 Agent 沙箱与分支合并契约（core/orchestration/multi_agent/）
        SandboxStatus::decl(),
        AgentSandbox::decl(),
        SandboxComparison::decl(),
        ConflictType::decl(),
        ConflictResolution::decl(),
        Conflict::decl(),
        MergeResult::decl(),
        // 后端推送事件契约（infra/types/events.rs，覆盖全部 25 类动态事件）
        ChatTurnStartEvent::decl(),
        ChatTurnEndEvent::decl(),
        ChatContentEvent::decl(),
        ChatThinkingEvent::decl(),
        ChatStreamEvent::decl(),
        ChatToolStartEvent::decl(),
        ChatToolDebugEvent::decl(),
        AgentStepEvent::decl(),
        PermissionRequestEvent::decl(),
        PermissionResolvedEvent::decl(),
        PlanProposalEvent::decl(),
        PlanProposalStreamEvent::decl(),
        ApprovalModeChangedEvent::decl(),
        AgentReadOnlyChangedEvent::decl(),
        AgentWorkModeChangedEvent::decl(),
        SessionAllowancesChangedEvent::decl(),
        SessionThinkingModeChangedEvent::decl(),
        SessionMessagesChangedEvent::decl(),
        BgCompactingChangedEvent::decl(),
        BackgroundFailedEvent::decl(),
        CheckpointCreatedEvent::decl(),
        SnapshotCreatedEvent::decl(),
        MemoryUpdatedEvent::decl(),
        SubagentTraceUpdatedEvent::decl(),
        TodoUpdateEvent::decl(),
        UserQuestionOption::decl(),
        UserQuestionItem::decl(),
        UserQuestionRequestEvent::decl(),
        UserQuestionResolvedEvent::decl(),
        UserQuestionAnswerItem::decl(),
        ResolveUserQuestionResult::decl(),
    ];

    let mut out = String::from(
        "// 本文件由 Rust 端 ts-rs 自动生成，请勿手工修改。\n\
         // 若 Rust 结构体字段有变，请在 src-tauri/ 下运行：\n\
         //   UPDATE_TS_BINDINGS=1 cargo test ts_contracts_are_up_to_date\n\n",
    );

    for decl in decls {
        // ts-rs 的 decl() 返回 `type Foo = ...;`，补上 `export ` 供前端直接导入
        let exported = if decl.starts_with("export ") {
            decl
        } else {
            format!("export {}", decl)
        };
        let cleaned = exported
            .replace("\r\n", "\n")
            .lines()
            .map(str::trim_end)
            .collect::<Vec<_>>()
            .join("\n");
        out.push_str(&cleaned);
        out.push_str("\n\n");
    }

    // 保留单一末尾换行
    while out.ends_with("\n\n") {
        out.pop();
    }
    out
}

fn contracts_file_path() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("..")
        .join("src")
        .join("types")
        .join("generated")
        .join("contracts.ts")
}

#[test]
fn ts_contracts_are_up_to_date() {
    let expected = render_contracts_ts();
    let target = contracts_file_path();

    if std::env::var("UPDATE_TS_BINDINGS").as_deref() == Ok("1") {
        if let Some(parent) = target.parent() {
            std::fs::create_dir_all(parent).expect("创建 src/types/generated 目录失败");
        }
        std::fs::write(&target, expected.as_bytes()).expect("写入 contracts.ts 失败");
        return;
    }

    let actual = std::fs::read_to_string(&target).unwrap_or_else(|err| {
        panic!(
            "读取 {} 失败（{}）。请先运行 UPDATE_TS_BINDINGS=1 cargo test ts_contracts_are_up_to_date 生成契约文件。",
            target.display(),
            err
        )
    });

    assert_eq!(
        actual.replace("\r\n", "\n"),
        expected,
        "前端生成契约 {} 与 Rust 结构体定义已漂移！\n请运行 UPDATE_TS_BINDINGS=1 cargo test ts_contracts_are_up_to_date 同步。",
        target.display()
    );
}
