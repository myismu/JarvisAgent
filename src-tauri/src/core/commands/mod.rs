//! # mod.rs — Tauri 命令模块入口
//!
//! 组织所有前端可调用的 Tauri 命令子模块。每个子模块对应一个功能领域，
//! 通过 `#[tauri::command]` 宏注册为前端 `invoke` 可调用的函数。
//!
//! ## 子模块
//! - `config`: 配置读写
//! - `session`: 会话生命周期管理
//! - `permission`: 权限确认与取消
//! - `history`: 会话历史渲染
//! - `checkpoint`: 检查点与分支管理
//! - `snapshot`: 快照引擎命令
//! - `sandbox`: 多 Agent 沙箱会话
//! - `merge`: 分支合并

pub mod checkpoint;
pub mod config;
pub mod history;
pub mod merge;
pub mod permission;
pub mod sandbox;
pub mod session;
pub mod snapshot;
