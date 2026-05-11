//! # events.rs — Tauri 事件名称常量
//!
//! 集中定义所有前端事件名，建立统一的 `domain:action` 命名规范。
//! 新增事件应遵循此规范，旧事件名保留作为向后兼容别名。
//!
//! ## 命名规范
//! - `domain:action` 格式（如 `chat:content`、`agent:step`）
//! - 领域前缀：chat / agent / subagent / session / snapshot / config / permission
//! - 动作使用 kebab-case

// --- Chat 领域 ---
pub const CHAT_CONTENT: &str = "chat:content";
#[allow(dead_code)]
const CHAT_CONTENT_LEGACY: &str = "chat-content";

pub const CHAT_THINKING: &str = "chat:thinking";
#[allow(dead_code)]
const CHAT_THINKING_LEGACY: &str = "chat-thinking";

pub const CHAT_TOOL_START: &str = "chat:tool-start";
#[allow(dead_code)]
const CHAT_TOOL_START_LEGACY: &str = "chat-tool-start";

pub const CHAT_TOOL_DEBUG: &str = "chat:tool-debug";
#[allow(dead_code)]
const CHAT_TOOL_DEBUG_LEGACY: &str = "chat-tool-debug";

pub const CHAT_TURN_START: &str = "chat:turn-start";
#[allow(dead_code)]
const CHAT_TURN_START_LEGACY: &str = "chat-turn-start";

pub const CHAT_TURN_END: &str = "chat:turn-end";
#[allow(dead_code)]
const CHAT_TURN_END_LEGACY: &str = "chat-turn-end";

pub const CHAT_STREAM: &str = "chat:stream";
#[allow(dead_code)]
const CHAT_STREAM_LEGACY: &str = "chat-stream";

// --- Agent 领域 ---
pub const AGENT_STEP: &str = "agent:step";
#[allow(dead_code)]
const AGENT_STEP_LEGACY: &str = "agent-step";

pub const AGENT_RUN_UPDATED: &str = "agent:run-updated";
#[allow(dead_code)]
const AGENT_RUN_UPDATED_LEGACY: &str = "agent-run-updated";

pub const AGENT_RUN_EVENT: &str = "agent:run-event";
#[allow(dead_code)]
const AGENT_RUN_EVENT_LEGACY: &str = "agent-run-event";

// --- SubAgent 领域 ---
pub const SUBAGENT_UPDATED: &str = "subagent:updated";
#[allow(dead_code)]
const SUBAGENT_UPDATED_LEGACY: &str = "subagent-updated";

pub const SUBAGENT_EVENT: &str = "subagent:event";
#[allow(dead_code)]
const SUBAGENT_EVENT_LEGACY: &str = "subagent-event";

// --- Session 领域 ---
pub const SESSION_UPDATED: &str = "session:updated";
#[allow(dead_code)]
const SESSION_UPDATED_LEGACY: &str = "session-updated";

pub const SESSION_RENAMED: &str = "session:renamed";
#[allow(dead_code)]
const SESSION_RENAMED_LEGACY: &str = "session-renamed";

pub const ACTIVE_SESSION_CHANGED: &str = "session:active-changed";
#[allow(dead_code)]
const ACTIVE_SESSION_CHANGED_LEGACY: &str = "active-session-changed";

// --- Snapshot / Checkpoint 领域 ---
pub const CHECKPOINT_CREATED: &str = "snapshot:checkpoint-created";
#[allow(dead_code)]
const CHECKPOINT_CREATED_LEGACY: &str = "checkpoint-created";

pub const SNAPSHOT_CREATED: &str = "snapshot:created";
#[allow(dead_code)]
const SNAPSHOT_CREATED_LEGACY: &str = "snapshot-created";

pub const CONTEXT_SNAPSHOT_UPDATED: &str = "snapshot:context-updated";
#[allow(dead_code)]
const CONTEXT_SNAPSHOT_UPDATED_LEGACY: &str = "context-snapshot-updated";

// --- 权限与计划 领域 ---
pub const PERMISSION_REQUEST: &str = "permission:request";
#[allow(dead_code)]
const PERMISSION_REQUEST_LEGACY: &str = "permission-request";

pub const PLAN_PROPOSAL: &str = "plan:proposal";
#[allow(dead_code)]
const PLAN_PROPOSAL_LEGACY: &str = "plan-proposal";

pub const PLAN_DOCUMENT_UPDATED: &str = "plan:document-updated";
#[allow(dead_code)]
const PLAN_DOCUMENT_UPDATED_LEGACY: &str = "plan-document-updated";

// --- 杂项 ---
pub const TODO_UPDATE: &str = "todo:update";
#[allow(dead_code)]
const TODO_UPDATE_LEGACY: &str = "todo-update";

pub const CONFIG_UPDATED: &str = "config:updated";
#[allow(dead_code)]
const CONFIG_UPDATED_LEGACY: &str = "config-updated";

pub const BG_TASK_DONE: &str = "bg:task-done";

pub const UI_PREFERENCES_CHANGED: &str = "ui:preferences-changed";
#[allow(dead_code)]
const UI_PREFERENCES_CHANGED_LEGACY: &str = "ui-preferences-changed";
