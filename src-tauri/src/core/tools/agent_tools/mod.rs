//! # agent_tools — Agent 专用工具模块
//!
//! 包含子代理执行引擎、技能加载、上下文压缩、记忆整理、方案审批等工具。
//!
//! ## 子模块
//! - `subagent`: 子代理执行引擎（独立 Agent Loop，支持只读/读写模式）
//! - `skill`: 技能加载
//! - `compact`: 上下文压缩 + 记忆整理
//! - `plan`: 方案审批工具
//!
//! ## 关键导出
//! - `run_subagent()`: 子代理执行引擎
//! - `load_skill()`: 按名称加载技能知识
//! - `compact()`: 手动触发上下文压缩
//! - `dream()`: 触发记忆整理（Dream Agent）
//! - `propose_plan()`: 方案审批工具，推送方案到前端并阻塞等待用户决策

mod compact;
mod plan;
mod skill;
mod subagent;

pub use compact::{compact, dream};
pub use plan::propose_plan;
pub use skill::load_skill;
pub use subagent::run_subagent;

use super::framework::agent_registry::AgentRegistry;

// --- 工具注册 ---
crate::define_tools! {
    pub fn register_tools(registry) {
        crate::tool_def!(
            "LoadSkill",
            desc: "按名称加载专业技能知识",
            hint: "load skill knowledge domain",
            schema_desc: "按名称加载专业技能知识。在你需要处理特定领域（如查阅API、审查代码）的不熟悉知识时使用。",
            props: {
                name: string => "要加载的技能名称",
            },
            required: ["name"],
            read_only: true,
            concurrency_safe: true,
        ),
        crate::tool_def!(
            "CompactConversation",
            desc: "手动触发对话上下文压缩",
            hint: "compact context compress summarize",
            schema_desc: "手动触发对话上下文压缩。当对话上下文过长觉得需要清理或重置记忆时使用该工具。",
            props: {
                focus: string => "摘要时需要特别保留的重点方向",
            },
            defer: true,
        ),
        crate::tool_def!(
            "ConsolidateMemory",
            desc: "主动触发记忆整理（Dream Agent）",
            hint: "dream memory organize consolidate",
            schema_desc: "主动触发记忆整理（Dream Agent）。将当前的零散碎片记忆提炼并合并进结构化用户画像中。",
            defer: true,
        ),
        crate::tool_def!(
            "ProposePlan",
            desc: "提交复杂任务实施方案给用户审阅",
            hint: "propose plan review approval",
            schema_desc: "【方案审批工具】将实施方案提交给用户审阅。当面对复杂任务（涉及多步骤修改、架构变更等），必须使用此工具提交方案文档，等待用户确认后才能继续执行。方案内容使用 Markdown 格式。前端会以专门的预览面板展示方案，用户可以选择同意或拒绝。",
            props: {
                title: string => "方案标题",
                content: string => "方案正文（Markdown 格式），包含需求理解、变更范围、具体步骤、风险评估等",
            },
            required: ["title", "content"],
            defer: true,
        ),
        crate::tool_def!(
            "RunSubagent",
            desc: "产生具有干净上下文的子代理执行具体操作",
            hint: "task subagent delegate spawn worker",
            schema_desc: format!("【真正执行】产生一个具有干净上下文环境的子代理 (Subagent) 去实际执行探索或具体操作任务。适合单个临时委派；复杂任务应优先使用 CreateTask/UpdateTask 构建依赖图，再调用 RunSubagentsSequentially 统一调度，避免手动连续 RunSubagent 串行执行。使用 description 提供短活动标签，使用 prompt 提供完整任务说明，使用 subagent_type 选择专用代理。与父进程共享文件系统但不共享对话历史。可用 subagent_type:\n{}", AgentRegistry::global().prompt_listing()),
            props: {
                prompt: string => "要子代理完成的任务说明，越详细越好。包括你想要子代理返回什么数据。",
                description: string => "Short 3-8 word activity label shown in the UI, e.g. 'Review notebook edits'.",
                subagent_type: string enum expr AgentRegistry::global().available_types() => format!("Specialized agent profile. If omitted, uses general. Available profiles:\n{}", AgentRegistry::global().prompt_listing()),
                model: string => "Optional model id override for this subagent. If omitted, inherits the active main model or the agent definition default.",
                task_id: integer => "Optional persistent task id for scheduler/board integration.",
                label: string => "Deprecated alias for description; prefer description.",
                read_only: boolean => "Optional permission override. If omitted, the selected subagent_type default is used. true filters out every tool whose registry metadata is not read-only; false still respects the selected agent allowlist/denylist.",
            },
            required: ["prompt"],
            defer: true,
            concurrency_safe: true,
        ),
        crate::tool_def!(
            "RunSubagentsSequentially",
            desc: "启动任务调度器，根据依赖关系自动并行执行任务",
            hint: "run tasks scheduler execute parallel",
            schema_desc: "【任务调度器】启动自动任务调度。系统将根据任务依赖关系（blocked_by）自动执行：无依赖的任务并行运行，阻塞任务等待前置完成后自动启动。创建完所有任务和依赖关系后调用此工具一次性调度执行。不要用于简单启动项目/运行命令；这类任务应直接用 StartBackgroundCommand/RunCommand。若调度返回失败，禁止继续创建重复任务，应复用现有任务 ID 修复或报告阻塞。",
            defer: true,
        )
    }
}
