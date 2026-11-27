//! # error.rs — 错误类型定义模块
//!
//! 定义应用中所有错误类型，使用 `thiserror` 实现标准化错误处理。
//! 包含 Agent 顶层错误、API 错误、工具错误和记忆系统错误。
//!
//! ## 关键导出
//! - `AgentError`: 顶层错误枚举，用于 Tauri command 返回值
//! - `ApiError`: API 调用相关错误（网络、认证、重试等）
//! - `ToolError`: 工具执行相关错误（权限、解析、执行失败等）
//! - `MemoryError`: 记忆/压缩相关错误
//!
//! ## 依赖
//! - Internal: 无
//! - External: `serde`, `thiserror`
//!
//! ## 约束
//! - 所有错误类型必须实现 `Serialize`，用于 Tauri 前端通信
//! - 使用 `#[error("...")]` 宏定义用户友好的错误消息（中文）

use serde::Serialize;
use thiserror::Error;

/// Agent 顶层错误类型，用于 Tauri command 返回值
#[derive(Debug, Error, Serialize)]
pub enum AgentError {
    #[error("配置错误: {0}")]
    Config(String),

    #[error("API 错误: {0}")]
    Api(#[from] ApiError),

    #[error("工具错误: {0}")]
    Tool(#[from] ToolError),

    #[error("记忆系统错误: {0}")]
    Memory(#[from] MemoryError),

    #[error("流处理错误: {0}")]
    Stream(String),

    #[error("会话错误: {0}")]
    Session(String),

    #[error("{0}")]
    ExecutionBusy(String),

    #[error("{0}")]
    ExecutionStopping(String),

    #[error("操作已取消")]
    Cancelled,

    #[error("循环次数超限: {0}/{1}")]
    LoopLimitExceeded(usize, usize),
}

impl From<String> for AgentError {
    fn from(s: String) -> Self {
        AgentError::Session(s)
    }
}

/// API 调用相关错误
#[derive(Debug, Error, Serialize)]
pub enum ApiError {
    #[error("未配置 API Key")]
    MissingApiKey,

    #[error("HTTP {status}: {body}")]
    HttpError { status: u16, body: String },

    #[error("网络错误: {0}")]
    Network(String),

    #[error("重试耗尽 ({max_retries}次): {last_error}")]
    RetriesExhausted {
        max_retries: u32,
        last_error: String,
    },

    #[error("响应解析错误: {0}")]
    Parse(String),
}

/// 工具执行相关错误
#[derive(Debug, Error, Serialize)]
pub enum ToolError {
    #[error("未知工具: {tool}")]
    NotFound { tool: String },

    #[error("工具 '{tool}' 参数解析失败: {reason}")]
    ParseError { tool: String, reason: String },

    #[error("工具 '{tool}' 执行失败: {reason}")]
    ExecutionError { tool: String, reason: String },

    #[error("工具 '{tool}' 权限被拒绝")]
    PermissionDenied { tool: String },
}

/// 记忆/压缩相关错误
#[derive(Debug, Error, Serialize)]
pub enum MemoryError {
    #[error("压缩失败: {0}")]
    CompactionFailed(String),

    #[error("记忆文件读取错误: {0}")]
    FileRead(String),

    #[error("记忆代理错误: {0}")]
    MemoryAgent(String),
}

/// 数据库/持久化层错误 — 供数据访问层使用，通过 From trait 与其他错误类型互转
#[derive(Debug, Error, Serialize)]
pub enum DbError {
    #[error("数据库错误: {0}")]
    Sqlite(String),
    #[error("序列化错误: {0}")]
    Serialize(String),
    #[error("未找到: {0}")]
    NotFound(String),
}

impl From<rusqlite::Error> for DbError {
    fn from(e: rusqlite::Error) -> Self {
        DbError::Sqlite(e.to_string())
    }
}

impl From<serde_json::Error> for DbError {
    fn from(e: serde_json::Error) -> Self {
        DbError::Serialize(e.to_string())
    }
}

impl From<String> for DbError {
    fn from(s: String) -> Self {
        DbError::Sqlite(s)
    }
}

/// 跨端统一错误码（与前端 `src/types/generated/contracts.ts` 自动对齐）。
///
/// 前端可据此区分输入校验、资源缺失、并发冲突、权限拒绝、网络/限流等错误类别，
/// 进行精准的国际化提示或重试交互，不再只能靠正则解析裸字符串。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, ts_rs::TS)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
#[ts(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum ErrorCode {
    InvalidInput,
    NotFound,
    Conflict,
    PermissionDenied,
    ConfigError,
    DatabaseError,
    NetworkError,
    RateLimited,
    Cancelled,
    InternalError,
}

/// 面向 Tauri 命令边界的统一结构化错误（可逐步替换 `Result<T, String>`）。
#[derive(Debug, Clone, Serialize, ts_rs::TS)]
#[serde(rename_all = "camelCase")]
#[ts(rename_all = "camelCase")]
pub struct CommandError {
    pub code: ErrorCode,
    pub message: String,
    pub retryable: bool,
}

impl CommandError {
    pub fn new(code: ErrorCode, message: impl Into<String>, retryable: bool) -> Self {
        Self {
            code,
            message: message.into(),
            retryable,
        }
    }

    pub fn invalid_input(message: impl Into<String>) -> Self {
        Self::new(ErrorCode::InvalidInput, message, false)
    }

    pub fn not_found(message: impl Into<String>) -> Self {
        Self::new(ErrorCode::NotFound, message, false)
    }

    pub fn conflict(message: impl Into<String>) -> Self {
        Self::new(ErrorCode::Conflict, message, true)
    }

    pub fn permission_denied(message: impl Into<String>) -> Self {
        Self::new(ErrorCode::PermissionDenied, message, false)
    }

    pub fn config(message: impl Into<String>) -> Self {
        Self::new(ErrorCode::ConfigError, message, false)
    }

    pub fn database(message: impl Into<String>) -> Self {
        Self::new(ErrorCode::DatabaseError, message, true)
    }

    pub fn internal(message: impl Into<String>) -> Self {
        Self::new(ErrorCode::InternalError, message, false)
    }

    /// 将既有仓储/服务层 `String` 错误按语义关键字映射为结构化错误码，便于渐进迁移。
    pub fn from_domain_message(message: impl Into<String>) -> Self {
        let msg = message.into();
        if msg.contains("不存在") || msg.contains("不在归档") || msg.contains("未找到") {
            Self::not_found(msg)
        } else if msg.contains("不能为空")
            || msg.contains("不能超过")
            || msg.contains("不支持")
            || msg.contains("不是文件夹")
            || msg.contains("非法")
        {
            Self::invalid_input(msg)
        } else if msg.contains("正在执行") || msg.contains("尚未结束") || msg.contains("锁")
        {
            Self::conflict(msg)
        } else if msg.contains("SQLite") || msg.contains("数据库") {
            Self::database(msg)
        } else {
            Self::internal(msg)
        }
    }
}

impl std::fmt::Display for CommandError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.message)
    }
}

impl std::error::Error for CommandError {}

impl From<String> for CommandError {
    fn from(message: String) -> Self {
        Self::from_domain_message(message)
    }
}

impl From<&str> for CommandError {
    fn from(message: &str) -> Self {
        Self::from_domain_message(message)
    }
}

impl From<DbError> for CommandError {
    fn from(err: DbError) -> Self {
        match err {
            DbError::NotFound(msg) => Self::not_found(msg),
            other => Self::database(other.to_string()),
        }
    }
}

impl From<AgentError> for CommandError {
    fn from(err: AgentError) -> Self {
        match err {
            AgentError::Config(msg) => Self::config(msg),
            AgentError::Api(ApiError::HttpError { status: 429, body }) => {
                Self::new(ErrorCode::RateLimited, body, true)
            }
            AgentError::Api(api_err) => {
                let retryable = matches!(
                    api_err,
                    ApiError::Network(_) | ApiError::RetriesExhausted { .. }
                );
                Self::new(ErrorCode::NetworkError, api_err.to_string(), retryable)
            }
            AgentError::Tool(ToolError::PermissionDenied { tool }) => {
                Self::permission_denied(format!("工具 '{}' 权限被拒绝", tool))
            }
            AgentError::Tool(tool_err) => Self::internal(tool_err.to_string()),
            AgentError::ExecutionBusy(msg) | AgentError::ExecutionStopping(msg) => {
                Self::conflict(msg)
            }
            AgentError::Cancelled => Self::new(ErrorCode::Cancelled, "操作已取消", false),
            other => Self::internal(other.to_string()),
        }
    }
}

impl From<DbError> for AgentError {
    fn from(e: DbError) -> Self {
        AgentError::Session(e.to_string())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn agent_error_display() {
        let err = AgentError::Config("测试配置错误".to_string());
        assert_eq!(format!("{}", err), "配置错误: 测试配置错误");
    }

    #[test]
    fn agent_error_from_string() {
        let err: AgentError = String::from("会话错误").into();
        assert_eq!(format!("{}", err), "会话错误: 会话错误");
    }

    #[test]
    fn api_error_display() {
        let err = ApiError::MissingApiKey;
        assert_eq!(format!("{}", err), "未配置 API Key");

        let err = ApiError::HttpError {
            status: 404,
            body: "Not Found".to_string(),
        };
        assert!(format!("{}", err).contains("404"));
        assert!(format!("{}", err).contains("Not Found"));
    }

    #[test]
    fn api_error_from_agent_error() {
        let api_err = ApiError::Network("timeout".to_string());
        let agent_err: AgentError = api_err.into();
        assert!(format!("{}", agent_err).contains("timeout"));
    }

    #[test]
    fn tool_error_display() {
        let err = ToolError::NotFound {
            tool: "test_tool".to_string(),
        };
        assert!(format!("{}", err).contains("test_tool"));

        let err = ToolError::PermissionDenied {
            tool: "dangerous_op".to_string(),
        };
        assert!(format!("{}", err).contains("dangerous_op"));
    }

    #[test]
    fn memory_error_display() {
        let err = MemoryError::FileRead("path/to/file not found".to_string());
        assert!(format!("{}", err).contains("path/to/file"));
    }

    #[test]
    fn command_error_classifies_domain_messages_and_serializes() {
        let not_found: CommandError = "会话 s1 不存在".into();
        assert_eq!(not_found.code, ErrorCode::NotFound);
        assert!(!not_found.retryable);

        let invalid: CommandError = "项目名不能为空".into();
        assert_eq!(invalid.code, ErrorCode::InvalidInput);
        assert!(!invalid.retryable);

        let conflict: CommandError = "当前任务尚未结束，请等待".into();
        assert_eq!(conflict.code, ErrorCode::Conflict);
        assert!(conflict.retryable);

        let json = serde_json::to_value(&invalid).expect("CommandError 应可序列化");
        assert_eq!(json["code"], "INVALID_INPUT");
        assert_eq!(json["message"], "项目名不能为空");
        assert_eq!(json["retryable"], false);
    }

    #[test]
    fn command_error_converts_from_agent_error() {
        let rate_limited: CommandError = AgentError::Api(ApiError::HttpError {
            status: 429,
            body: "rate limit".to_string(),
        })
        .into();
        assert_eq!(rate_limited.code, ErrorCode::RateLimited);
        assert!(rate_limited.retryable);

        let busy: CommandError = AgentError::ExecutionBusy("执行中".to_string()).into();
        assert_eq!(busy.code, ErrorCode::Conflict);
        assert!(busy.retryable);

        let denied: CommandError = AgentError::Tool(ToolError::PermissionDenied {
            tool: "WriteFile".to_string(),
        })
        .into();
        assert_eq!(denied.code, ErrorCode::PermissionDenied);
        assert!(!denied.retryable);
    }
}
