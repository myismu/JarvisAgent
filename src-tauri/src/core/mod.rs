//! # core/mod.rs — 业务层入口
//!
//! Agent 核心业务流程，包含 Agent 主循环、调度编排、会话管理、
//! 回滚策略和工具执行。依赖基础设施层（crate::infra），不涉及 Tauri 命令处理。
//!
//! ## 子模块
//! - `agent`: Agent 主循环 (ask_jarvis)
//! - `orchestration`: 调度器、任务编排
//! - `session`: 会话生命周期、记忆
//! - `rollback`: 快照、回滚、GC、分支合并
//! - `intent`: 意图识别
//! - `tools`: 所有工具实现

pub mod agent;
pub mod intent;
pub mod orchestration;
pub mod rollback;
pub mod session;
pub mod tools;

#[macro_export]
macro_rules! jarvis_debug { ($tag:expr, $($arg:tt)*) => { println!($($arg)*); } }
#[macro_export]
macro_rules! jarvis_info { ($tag:expr, $($arg:tt)*) => { println!($($arg)*); } }
#[macro_export]
macro_rules! jarvis_warn { ($tag:expr, $($arg:tt)*) => { eprintln!($($arg)*); } }
