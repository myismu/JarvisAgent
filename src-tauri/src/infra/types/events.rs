//! # infra/types/events.rs — Tauri 后端推送事件强类型契约定义
//!
//! 统一收口后端通过 `app.emit(...)` 推送给前端的全部 25 类事件载荷结构体，
//! 替代分散在业务代码中的 `serde_json::json!`，并通过 `ts-rs` 自动导出到前端
//! `src/types/generated/contracts.ts`，保证前后端字段名与类型完全一致。
//!
//! ## Key Exports
//! - 对话与流式事件：`ChatTurnStartEvent`、`ChatTurnEndEvent`、`ChatContentEvent`、`ChatThinkingEvent`、`ChatStreamEvent`、`ChatToolStartEvent`、`ChatToolDebugEvent`
//! - 执行步骤事件：`AgentStepEvent`（`#[serde(tag = "type")]` 判别联合枚举）
//! - 治理与权限事件：`PermissionRequestEvent`、`PermissionResolvedEvent`、`PlanProposalEvent`、`PlanProposalStreamEvent`、`ApprovalModeChangedEvent`、`AgentReadOnlyChangedEvent`、`AgentWorkModeChangedEvent`、`SessionAllowancesChangedEvent`
//! - 会话与系统事件：`SessionThinkingModeChangedEvent`、`SessionMessagesChangedEvent`、`BgCompactingChangedEvent`、`BackgroundFailedEvent`、`CheckpointCreatedEvent`、`SnapshotCreatedEvent`、`MemoryUpdatedEvent`、`SubagentTraceUpdatedEvent`、`TodoUpdateEvent`
//!
//! ## Dependencies
//! - Internal: `crate::infra::types::models::{TodoGroup, TodoItem}`
//! - External: `serde`, `ts-rs`

use serde::Serialize;
use ts_rs::TS;

use crate::infra::types::models::{TodoGroup, TodoItem};

// ── 对话与流式事件 ──

/// `chat-turn-start`：新一轮模型调用开始
#[derive(Debug, Clone, Serialize, TS)]
#[serde(rename_all = "camelCase")]
pub struct ChatTurnStartEvent {
    pub session_id: String,
    pub loop_count: usize,
}

/// `chat-turn-end`：单轮模型调用结束
/// 注意：`has_tool` 在传输层保持下划线命名以兼容既有前端消费方。
#[derive(Debug, Clone, Serialize, TS)]
pub struct ChatTurnEndEvent {
    pub has_tool: bool,
    #[serde(rename = "sessionId")]
    pub session_id: String,
    #[serde(rename = "loopCount")]
    pub loop_count: usize,
}

/// `chat-content`：助手正文增量片段
#[derive(Debug, Clone, Serialize, TS)]
#[serde(rename_all = "camelCase")]
pub struct ChatContentEvent {
    pub content: String,
    pub session_id: String,
    pub loop_count: usize,
}

/// `chat-thinking`：深度思考增量片段（主循环或子代理）
#[derive(Debug, Clone, Serialize, TS)]
#[serde(rename_all = "camelCase")]
pub struct ChatThinkingEvent {
    pub content: String,
    pub session_id: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub loop_count: Option<usize>,
    #[serde(skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub is_sub_agent: Option<bool>,
}

/// `chat-stream`：管线状态说明、调度进度与子代理过程流式提示
#[derive(Debug, Clone, Serialize, TS)]
#[serde(rename_all = "camelCase")]
pub struct ChatStreamEvent {
    pub content: String,
    pub session_id: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub loop_count: Option<usize>,
    #[serde(skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub is_sub_agent: Option<bool>,
}

/// `chat-tool-start`：模型开始输出工具调用参数
#[derive(Debug, Clone, Serialize, TS)]
#[serde(rename_all = "camelCase")]
pub struct ChatToolStartEvent {
    pub session_id: String,
    pub loop_count: usize,
    pub tool_call_id: String,
    pub tool: String,
}

/// `chat-tool-debug`：工具执行状态变更与参数修复调试信息
#[derive(Debug, Clone, Serialize, TS)]
#[serde(rename_all = "camelCase")]
pub struct ChatToolDebugEvent {
    #[serde(skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub kind: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub status: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub tool: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub tool_call_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub content: Option<String>,
    pub session_id: String,
    pub loop_count: usize,
}

// ── Agent 执行步骤判别联合事件 ──

/// `agent-step`：Agent 主循环、工具执行、反思审查、重试与子代理关键步骤事件
#[derive(Debug, Clone, Serialize, TS)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum AgentStepEvent {
    /// 开始执行某个工具调用
    #[serde(rename_all = "camelCase")]
    ToolCall {
        tool: String,
        content: String,
        session_id: String,
        loop_count: usize,
    },
    /// 工具执行成功返回
    #[serde(rename_all = "camelCase")]
    ToolResult {
        tool: String,
        tool_call_id: String,
        content: String,
        error: Option<String>,
        session_id: String,
        loop_count: usize,
    },
    /// 工具执行失败或参数解析失败
    #[serde(rename_all = "camelCase")]
    ToolError {
        tool: String,
        tool_call_id: String,
        error: Option<String>,
        content: String,
        #[serde(rename = "output_summary", skip_serializing_if = "Option::is_none")]
        #[ts(optional)]
        output_summary: Option<String>,
        session_id: String,
        loop_count: usize,
    },
    /// 清除指定类型的临时提示（如重试提示、等待提示）
    #[serde(rename_all = "camelCase")]
    NoticeClear {
        kinds: Vec<String>,
        session_id: String,
        #[serde(skip_serializing_if = "Option::is_none")]
        #[ts(optional)]
        loop_count: Option<usize>,
    },
    /// 流式响应静默超时前的倒计时等待提示
    #[serde(rename_all = "camelCase")]
    WaitingHint {
        #[ts(type = "number")]
        silent_secs: u64,
        #[ts(type = "number")]
        remain_secs: u64,
        session_id: String,
        loop_count: usize,
    },
    /// 请求失败或 429 限流重试提示
    #[serde(rename_all = "camelCase")]
    Retry {
        attempt: u32,
        max: u32,
        #[ts(type = "number")]
        wait_secs: u64,
        session_id: String,
    },
    /// 反思审查 Agent 判定结论
    #[serde(rename_all = "camelCase")]
    Reflection {
        session_id: String,
        loop_count: usize,
        judgment: String,
        #[serde(skip_serializing_if = "Option::is_none")]
        #[ts(optional)]
        reason: Option<String>,
        #[serde(skip_serializing_if = "Option::is_none")]
        #[ts(optional)]
        suggestion: Option<String>,
    },
    /// 本轮执行被异常或超时中断
    #[serde(rename_all = "camelCase")]
    Interrupted {
        reason: String,
        session_id: String,
        loop_count: usize,
    },
    /// 本轮执行被用户主动取消
    #[serde(rename_all = "camelCase")]
    Cancelled {
        session_id: String,
        loop_count: usize,
    },
    /// 依赖图调度器派发子任务
    #[serde(rename_all = "camelCase")]
    TaskScheduled {
        task_id: i32,
        subject: String,
        session_id: String,
    },
    /// 依赖图调度器完成子任务
    #[serde(rename_all = "camelCase")]
    TaskCompleted {
        task_id: i32,
        status: String,
        session_id: String,
    },
    /// 子代理开始执行
    #[serde(rename_all = "camelCase")]
    SubagentStart {
        task: String,
        session_id: String,
        is_sub_agent: bool,
    },
    /// 子代理执行结束
    #[serde(rename_all = "camelCase")]
    SubagentEnd {
        session_id: String,
        is_sub_agent: bool,
    },
}

// ── 权限、方案审批与模式治理事件 ──

/// `permission-request`：向用户发起工具执行权限确认
#[derive(Debug, Clone, Serialize, TS)]
#[serde(rename_all = "camelCase")]
pub struct PermissionRequestEvent {
    pub id: String,
    pub message: String,
    pub session_id: String,
    pub kind: String,
    pub allow_session: bool,
    pub warning: Option<String>,
}

/// `permission-resolved`：权限请求已得出决策或被中断
#[derive(Debug, Clone, Serialize, TS)]
#[serde(rename_all = "camelCase")]
pub struct PermissionResolvedEvent {
    pub id: String,
    pub session_id: String,
    pub decision: String,
    pub decision_text: String,
}

/// `plan-proposal`：Agent 提交完整方案文档待用户审批
#[derive(Debug, Clone, Serialize, TS)]
#[serde(rename_all = "camelCase")]
pub struct PlanProposalEvent {
    pub id: String,
    pub title: String,
    pub content: String,
    pub session_id: String,
}

/// `plan-proposal-stream`：方案文档生成期间的流式增量正文
#[derive(Debug, Clone, Serialize, TS)]
#[serde(rename_all = "camelCase")]
pub struct PlanProposalStreamEvent {
    pub content: String,
    pub session_id: String,
}

/// `approval-mode-changed`：会话权限档位切换
#[derive(Debug, Clone, Serialize, TS)]
#[serde(rename_all = "camelCase")]
pub struct ApprovalModeChangedEvent {
    pub session_id: String,
    pub mode: String,
}

/// `agent-read-only-changed`：会话只读保护闸门切换
#[derive(Debug, Clone, Serialize, TS)]
#[serde(rename_all = "camelCase")]
pub struct AgentReadOnlyChangedEvent {
    pub session_id: String,
    pub enabled: bool,
}

/// `agent-work-mode-changed`：会话工作模式（edit / plan）切换
#[derive(Debug, Clone, Serialize, TS)]
#[serde(rename_all = "camelCase")]
pub struct AgentWorkModeChangedEvent {
    pub session_id: String,
    pub from: String,
    pub to: String,
    pub reason: String,
}

/// `session-allowances-changed`：会话已授权范围清单发生变更
#[derive(Debug, Clone, Serialize, TS)]
#[serde(rename_all = "camelCase")]
pub struct SessionAllowancesChangedEvent {
    pub session_id: String,
}

// ── 会话状态、后台任务与持久化通知事件 ──

/// `session-thinking-mode-changed`：会话深度思考开关生效状态变更
#[derive(Debug, Clone, Serialize, TS)]
#[serde(rename_all = "camelCase")]
pub struct SessionThinkingModeChangedEvent {
    pub session_id: String,
    pub thinking_enabled: bool,
}

/// `session-messages-changed`：会话落库消息数量/最大序号发生变更（前端按需刷新或标脏）
#[derive(Debug, Clone, Serialize, TS)]
#[serde(rename_all = "camelCase")]
pub struct SessionMessagesChangedEvent {
    pub session_id: String,
    #[ts(type = "number")]
    pub count: i64,
    #[ts(type = "number | null")]
    pub last_seq: Option<i64>,
    pub reason: String,
}

/// `bg-compacting-changed`：后台上下文压缩状态变更
#[derive(Debug, Clone, Serialize, TS)]
#[serde(rename_all = "camelCase")]
pub struct BgCompactingChangedEvent {
    pub session_id: String,
    pub compacting: bool,
}

/// `background-failed`：后台服务或命令任务异常退出提醒
#[derive(Debug, Clone, Serialize, TS)]
#[serde(rename_all = "camelCase")]
pub struct BackgroundFailedEvent {
    pub session_id: Option<String>,
    pub task_id: String,
    pub command: String,
    pub result: String,
    pub port: Option<u16>,
    pub task_type: Option<String>,
}

/// `checkpoint-created`：检查点创建完成通知
#[derive(Debug, Clone, Serialize, TS)]
#[serde(rename_all = "camelCase")]
pub struct CheckpointCreatedEvent {
    pub session_id: String,
    pub checkpoint_id: Option<String>,
    pub has_patches: bool,
    pub can_rollback: bool,
    pub message: String,
}

/// `snapshot-created`：文件快照创建完成通知
#[derive(Debug, Clone, Serialize, TS)]
#[serde(rename_all = "camelCase")]
pub struct SnapshotCreatedEvent {
    pub session_id: String,
    pub snapshot_id: String,
}

/// `memory-updated`：全局用户记忆更新完成通知
#[derive(Debug, Clone, Serialize, TS)]
#[serde(rename_all = "camelCase")]
pub struct MemoryUpdatedEvent {
    pub session_id: String,
    pub summary: String,
}

/// `subagent-trace-updated`：子代理轨迹事件新增通知
#[derive(Debug, Clone, Serialize, TS)]
#[serde(rename_all = "camelCase")]
pub struct SubagentTraceUpdatedEvent {
    pub run_id: String,
    pub loop_index: usize,
}

/// `todo-update`：会话待办清单与分组快照更新
#[derive(Debug, Clone, Serialize, TS)]
#[serde(rename_all = "camelCase")]
pub struct TodoUpdateEvent {
    pub todos: Vec<TodoItem>,
    pub groups: Vec<TodoGroup>,
    pub session_id: String,
}

// ── 运行期用户咨询事件与答复契约（AskUser 工具） ──

/// 单个咨询选项（展示文案、可选说明、是否推荐项）
#[derive(Debug, Clone, Serialize, serde::Deserialize, TS, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct UserQuestionOption {
    pub label: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub description: Option<String>,
    #[serde(default)]
    pub recommended: bool,
}

/// 单道咨询问题（支持单选 / 多选与“其他”自填框）
#[derive(Debug, Clone, Serialize, serde::Deserialize, TS, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct UserQuestionItem {
    pub id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub header: Option<String>,
    pub question: String,
    #[serde(default)]
    pub multi_select: bool,
    pub options: Vec<UserQuestionOption>,
    #[serde(default = "default_allow_custom")]
    pub allow_custom: bool,
}

fn default_allow_custom() -> bool {
    true
}

/// `user-question-request`：Agent 运行期向用户发起结构化咨询
#[derive(Debug, Clone, Serialize, TS)]
#[serde(rename_all = "camelCase")]
pub struct UserQuestionRequestEvent {
    pub id: String,
    pub session_id: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub title: Option<String>,
    pub questions: Vec<UserQuestionItem>,
}

/// `user-question-resolved`：咨询请求已作答、跳过或被取消中断
#[derive(Debug, Clone, Serialize, TS)]
#[serde(rename_all = "camelCase")]
pub struct UserQuestionResolvedEvent {
    pub id: String,
    pub session_id: String,
    /// `answered` / `skipped` / `interrupted`
    pub status: String,
}

/// 单道题的用户答复（支持多选与自定义文本）
#[derive(Debug, Clone, Serialize, serde::Deserialize, TS, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct UserQuestionAnswerItem {
    pub question_id: String,
    #[serde(default)]
    pub selected_options: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub custom_text: Option<String>,
}

/// `resolve_user_question` 命令返回结果
#[derive(Debug, Clone, Serialize, TS)]
#[serde(rename_all = "camelCase")]
pub struct ResolveUserQuestionResult {
    pub resolved: bool,
    pub status: String,
}

// ── MCP 扩展服务事件 ──

/// `mcp-server-status`：MCP 服务运行状态或能力数量变化通知
#[derive(Debug, Clone, Serialize, TS)]
#[serde(rename_all = "camelCase")]
pub struct McpServerStatusEvent {
    pub server_name: String,
    pub status: crate::core::mcp::McpServerStatus,
    pub tool_count: usize,
    pub resource_count: usize,
    pub prompt_count: usize,
    #[serde(skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub last_error: Option<String>,
}
