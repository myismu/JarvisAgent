//! # mod.rs — Shell 工具模块入口
//!
//! 导出 Shell、Git 和后台任务管理工具定义，统筹模块内部的各个安全与执行组件。
//!
//! ## Key Exports
//! - `register_tools()`: 注册 shell 相关的 ToolDef
//!
//! ## Dependencies
//! - Internal: crate::core::tools::framework::registry::ToolDef
//! - External: serde_json

pub mod background;
pub mod execution;
pub mod git;
pub mod guards;
pub mod readonly;
pub mod regexes;
pub mod security;
pub mod types;
pub mod utils;

pub use background::{background_run, check_background};
pub use execution::run_shell;
pub use git::git_command;

use crate::core::tools::framework::registry::ToolDef;
use serde_json::json;
use utils::shell_tool_description;

// --- 工具注册 ---
crate::define_tools! {
    pub fn register_tools(registry) {
        ToolDef {
            name: "run_shell",
            description: if cfg!(target_os = "windows") { "执行 Windows PowerShell 命令（统一入口）" } else { "执行 Unix bash 命令（统一入口）" },
            search_hint: "shell powershell bash command execute run background",
            schema: json!({
                "name": "run_shell",
                "description": shell_tool_description(),
                "input_schema": {
                    "type": "object",
                    "properties": {
                        "command": {"type": "string", "description": if cfg!(target_os = "windows") { "要执行的 PowerShell 命令" } else { "要执行的 bash 命令" }},
                        "description": {"type": "string", "description": "一句话说明命令用途（显示在权限确认中）"},
                        "timeout": {"type": "integer", "description": "超时秒数，默认 120，范围 5-600"},
                        "run_in_background": {"type": "boolean", "description": "是否后台执行。长周期任务（如开发服务器）必须设为 true。"}
                    },
                    "required": ["command", "description"]
                }
            }),
            should_defer: true,
            is_read_only: false,
            is_concurrency_safe: false,
            is_enabled: true,
        },
        ToolDef {
            name: "git_command",
            description: "执行低风险的 git 操作（status/diff/log 等）",
            search_hint: "git status diff log version control",
            schema: json!({
                "name": "git_command",
                "description": "执行低风险的 git 操作（如 status, diff, log）。禁止执行修改历史或推送的操作。",
                "input_schema": {
                    "type": "object",
                    "properties": {
                        "args": {
                            "type": "array",
                            "items": {"type": "string"},
                            "description": "git 命令的参数列表，例如 [\"status\"] 或 [\"log\", \"-n\", \"5\"]"
                        }
                    },
                    "required": ["args"]
                }
            }),
            should_defer: true,
            is_read_only: true,
            is_concurrency_safe: true,
            is_enabled: true,
        },
        ToolDef {
            name: "background_run",
            description: "在后台执行长时间运行的命令（独立入口）",
            search_hint: "background long running server dev",
            schema: json!({
                "name": "background_run",
                "description": "在后台执行长时间运行的命令（如启动前端 npm run dev、后端服务器等）。执行后立刻返回任务ID，不阻塞对话。推荐优先使用 run_shell 的 run_in_background 参数。",
                "input_schema": {
                    "type": "object",
                    "properties": {
                        "command": {"type": "string", "description": "要执行的具体命令（如 npm run dev）"},
                        "dir": {"type": "string", "description": "命令执行的工作目录的绝对路径"}
                    },
                    "required": ["command", "dir"]
                }
            }),
            should_defer: true,
            is_read_only: false,
            is_concurrency_safe: true,
            is_enabled: true,
        },
        ToolDef {
            name: "check_background",
            description: "检查后台任务的执行状态和输出",
            search_hint: "check background task status output",
            schema: json!({
                "name": "check_background",
                "description": "检查后台任务的执行状态和输出。仅当用户主动询问后台任务状态时才使用，严禁在自己的思考循环中连续轮询此工具！",
                "input_schema": {
                    "type": "object",
                    "properties": {
                        "task_id": {"type": "string", "description": "后台任务 ID。如果留空则返回所有任务状态。"}
                    }
                }
            }),
            should_defer: true,
            is_read_only: true,
            is_concurrency_safe: true,
            is_enabled: true,
        }
    }
}
