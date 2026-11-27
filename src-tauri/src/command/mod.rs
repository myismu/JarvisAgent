//! # mod.rs — Tauri 命令模块入口
//!
//! 组织所有前端可调用的 Tauri 命令子模块。每个子模块对应一个功能领域，
//! 通过 `#[tauri::command]` 宏注册为前端 `invoke` 可调用的函数。
//!
//! ## 子模块
//! - `app_config`: 窗口状态、UI 偏好与技能开关命令
//! - `background`: 后台任务、子代理与会话待办查询命令
//! - `checkpoint`: 检查点与分支管理命令
//! - `config`: 模型配置读写命令
//! - `history`: 会话历史读取命令
//! - `mcp`: MCP 扩展服务管理命令
//! - `merge`: 分支合并命令
//! - `permission`: 权限确认与取消命令
//! - `project`: 项目管理与工作目录命令
//! - `prompt`: 提示词磁盘化读写命令
//! - `sandbox`: 多 Agent 沙箱命令
//! - `session`: 会话生命周期管理命令
//! - `shell`: Shell 执行方言设置命令
//! - `skill`: 技能管理命令
//! - `snapshot`: 快照引擎命令
//! - `tool`: 工具开关命令

pub mod app_config;
pub mod background;
pub mod checkpoint;
pub mod config;
pub mod history;
pub mod mcp;
pub mod merge;
pub mod permission;
pub mod project;
pub mod prompt;
pub mod sandbox;
pub mod session;
pub mod shell;
pub mod skill;
pub mod snapshot;
pub mod tool;
