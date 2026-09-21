//! # mod.rs — Shell 工具模块入口
//!
//! 导出 Shell 和后台任务管理工具定义，统筹模块内部的各个安全与执行组件。
//! （git 不再有专用工具：读写统一走 RunCommand，权限与拦截系统分层管控。）
//!
//! ## Key Exports
//! - `register_tools()`: 注册 shell 相关的 ToolDef
//!
//! ## Dependencies
//! - Internal: crate::core::tools::framework::registry::ToolDef
//! - External: serde_json

pub mod background;
pub mod execution;
pub mod guards;
pub mod readonly;
pub mod regexes;
pub mod security;
pub mod types;
pub mod utils;

pub use background::{background_run, check_background};
pub use execution::run_shell;

use crate::core::tools::framework::registry::ToolDef;
use serde_json::json;
use utils::{dir_param_description, shell_tool_description};

// --- 工具注册 ---
crate::define_tools! {
    pub fn register_tools(registry) {
        ToolDef {
            name: "RunCommand",
            description: if cfg!(target_os = "windows") { "执行 Windows PowerShell 命令（统一入口）" } else { "执行 Unix bash 命令（统一入口）" },
            search_hint: "shell powershell bash command execute run background",
            category: "命令执行",
            schema: json!({
                "name": "RunCommand",
                "description": shell_tool_description(),
                "input_schema": {
                    "type": "object",
                    "properties": {
                        "command": {"type": "string", "description": if cfg!(target_os = "windows") { "要执行的 PowerShell 命令" } else { "要执行的 bash 命令" }},
                        "description": {"type": "string", "description": "一句话说明命令用途（显示在权限确认中）"},
                        // dir 于 2026-09-21 补齐：提示词两处早就写着 RunCommand 有这个参数，
                        // 实现却一直没读（命令恒在工作区根目录跑）；而沙箱禁止 cd、
                        // 子代理又没有 StartBackgroundCommand 权限，导致"在子目录跑 npm install"
                        // 根本做不到。现按 StartBackgroundCommand 的同款口径实现。
                        "dir": {"type": "string", "description": dir_param_description()},
                        "timeout": {"type": "integer", "description": "超时秒数，默认 120，范围 5-600"},
                        "run_in_background": {"type": "boolean", "description": "是否后台执行。长周期任务（如开发服务器）必须设为 true。"}
                    },
                    "required": ["command", "description"]
                }
            }),
            should_defer: false,
            is_read_only: false,
            is_concurrency_safe: false,
            is_enabled: true,
        },
        ToolDef {
            name: "StartBackgroundCommand",
            description: "在后台执行长时间运行的命令（独立入口）",
            search_hint: "background long running server dev",
            category: "命令执行",
            schema: json!({
                "name": "StartBackgroundCommand",
                // 末句原写"推荐优先使用 RunCommand 的 run_in_background 参数"，方向反了
                // （2026-09-21 修正）：两者是**分工**而非替代 —— RunCommand 的后台模式固定
                // 在工作区根目录执行（background_run_internal 传的是 session workspace），
                // 只有本工具能用 dir 指定子目录。backend/ + frontend/ 这类结构必须用本工具，
                // 原句会把模型引向一个做不到的方案。
                "description": "在后台执行长时间运行的命令（如启动前端 npm run dev、后端服务器等）。执行后立刻返回任务ID，不阻塞对话。与 RunCommand 的 run_in_background 的分工：本工具可用 dir 指定工作目录，需要进子目录启动时必须用本工具；RunCommand 的后台模式固定在工作区根目录执行。",
                "input_schema": {
                    "type": "object",
                    "properties": {
                        "command": {"type": "string", "description": "要执行的具体命令（如 npm run dev）"},
                        "dir": {"type": "string", "description": dir_param_description()}
                    },
                    "required": ["command", "dir"]
                }
            }),
            should_defer: false,
            is_read_only: false,
            is_concurrency_safe: true,
            is_enabled: true,
        },
        ToolDef {
            name: "CheckBackgroundCommand",
            description: "检查后台任务的执行状态和输出",
            search_hint: "check background task status output",
            category: "命令执行",
            schema: json!({
                "name": "CheckBackgroundCommand",
                // 两个使用时机是**完整清单**（2026-09-21 从提示词搬来）：原提示词写过
                // "界面弹出失败提醒后用户让你检查时"，schema 只留了"用户主动询问"这一半。
                "description": "检查后台任务的执行状态和输出。使用时机有二：用户主动询问后台任务状态时；界面弹出任务失败提醒、用户让你排查时。完整输出也可在监控面板查看。严禁在自己的思考循环中连续轮询此工具！",
                "input_schema": {
                    "type": "object",
                    "properties": {
                        "task_id": {"type": "string", "description": "后台任务 ID。如果留空则返回所有任务状态。"}
                    }
                }
            }),
            should_defer: false,
            is_read_only: true,
            is_concurrency_safe: true,
            is_enabled: true,
        }
    }
}
