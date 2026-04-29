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
    RetriesExhausted { max_retries: u32, last_error: String },

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
}
