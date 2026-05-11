//! # infra/mod.rs — 基础设施层入口
//!
//! 基础设施层提供纯技术能力，不包含 Agent 业务语义。
//! 组织并重导出所有子模块和关键类型。
//!
//! ## 子模块
//! - `types`: 数据模型、错误类型、trait 定义、常量
//! - `config`: 配置管理、数据路径
//! - `state`: 全局状态管理、事件常量
//! - `db`: SQLite 数据库连接
//! - `llm`: LLM 客户端、适配器、注册表
//! - `providers`: LLM 提供商具体实现
//!
//! ## 关键重导出
//! - `SessionManager`, `SessionContext`, `WorkspaceState`, `SnapshotRegistry`
//! - `AgentError`, `ApiError`, `ToolError`, `MemoryError`
//! - `AgentConfig`, `ConfigState`, `load_config`, `save_config`

pub mod types;
pub mod config;
pub mod state;
pub mod db;
pub mod llm;
pub mod providers;

pub mod background;
pub mod debug_logger;


// 重导出常用基础类型
pub use types::{
    constants, error,
    models::{self, Content, ContentBlock, Message, SessionMemory, Task, TaskStatus},
    traits,
};
pub use config::data_paths;
pub use state::events;
