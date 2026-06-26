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
mod switch_mode;

pub use compact::{compact, dream};
pub use plan::propose_plan;
pub use skill::load_skill;
pub use subagent::run_subagent;
pub use switch_mode::switch_work_mode;

use super::framework::agent_registry::AgentRegistry;
use super::framework::registry::ToolRegistry;
use crate::core::tools::framework;

/// GetToolCatalog 处理函数：从 ToolRegistry 获取延迟工具列表 + 从 skills 目录获取技能列表
pub async fn get_tool_catalog(_app: &tauri::AppHandle, _input: &serde_json::Value, _session_id: &str, intent: &str) -> framework::ToolCallResult {
    let mut out = String::new();

    // 延迟工具列表
    let groups = ToolRegistry::global().get_deferred_by_category(intent);
    if !groups.is_empty() {
        out.push_str("【延迟工具】（需通过 ExecuteTool 执行）:\n");
        for (category, names) in &groups {
            out.push_str(&format!("- {}: {}\n", category, names.join(", ")));
        }
    }

    // 技能列表（只返回激活的技能）
    let skills = super::load_all_skills();
    let activations = crate::command::app_config::get_all_skill_activations();
    let active_skills: Vec<String> = skills
        .iter()
        .filter(|s| activations.get(&s.name).copied().unwrap_or(true))
        .map(|s| s.name.to_string())
        .collect();
    if !active_skills.is_empty() {
        out.push_str(&format!("\n【可用技能】（通过 LoadSkill 加载）:\n- {}\n", active_skills.join(", ")));
    }

    if out.is_empty() {
        return framework::ToolCallResult::error("当前意图下没有可用的延迟工具或技能。".to_string());
    }

    out.push_str("\n使用方式:\n- 延迟工具: 先 DiscoverTools 查询参数，再 ExecuteTool(name=\"工具名\", args={...}) 执行\n- 技能: 直接 LoadSkill(name=\"技能名\") 加载");
    framework::ToolCallResult::ok(out)
}

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
            category: "系统",
            read_only: true,
            concurrency_safe: true,
        ),
        crate::tool_def!(
            "GetToolCatalog",
            desc: "获取可用工具和技能目录",
            hint: "get tool catalog available tools skills discover",
            schema_desc: "获取当前可用的延迟工具列表（按分类分组）和技能列表。当你需要使用非核心工具（如 WriteFile、EditFile、RunCommand 等写操作工具）或加载技能时，必须先调用此工具获取可用资源目录。延迟工具通过 DiscoverTools + ExecuteTool 两步执行，技能通过 LoadSkill 直接加载。",
            category: "系统",
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
            category: "系统",
            defer: true,
        ),
        crate::tool_def!(
            "ConsolidateMemory",
            desc: "主动触发记忆整理（Dream Agent）",
            hint: "dream memory organize consolidate",
            schema_desc: "主动触发记忆整理（Dream Agent）。将当前的零散碎片记忆提炼并合并进结构化用户画像中。",
            category: "系统",
            defer: true,
        ),
        crate::tool_def!(
            "ProposePlan",
            desc: "提交复杂任务实施方案给用户审阅",
            hint: "propose plan review approval",
            schema_desc: "【方案审批工具】将实施方案提交给用户审阅。当面对复杂任务（涉及多步骤修改、架构变更等），必须使用此工具提交方案文档，等待用户确认后才能继续执行。方案内容使用 Markdown 格式。前端会以专门的预览面板展示方案，用户可以选择同意或拒绝。task_breakdown 字段用于结构化任务分解，每项包含 subject（任务名）、description（详情）、depends_on（前置任务序号数组，从1开始）、can_parallel_with（可并行任务序号数组）。",
            props: {
                title: string => "方案标题",
                content: string => "方案正文（Markdown 格式），必须包含：需求理解、变更范围、具体实现步骤、风险评估、任务拆分统计（任务数、阶段划分、预计耗时）、依赖关系图、并行执行策略",
                task_breakdown: array => "【必填】结构化任务分解列表，每项包含 subject（任务名）、description（详情，含预计耗时）、depends_on（前置任务序号数组）、can_parallel_with（可并行任务序号数组）",
            },
            required: ["title", "content", "task_breakdown"],
            category: "Agent 调度",
            defer: true,
        ),
        crate::tool_def!(
            "SwitchWorkMode",
            desc: "切换 Agent 工作模式",
            hint: "switch work mode chat edit plan",
            schema_desc: "切换 Agent 的工作模式（chat/edit/plan）。编辑模式下遇到复杂任务时，可切换到计划模式进行深度规划；规划完成后切回编辑模式执行。聊天模式下禁止切换到计划模式。此工具只切换工作模式，不影响用户类型（user/developer）。",
            props: {
                mode: string => "目标工作模式：chat（聊天）、edit（编辑）、plan（规划）",
                reason: string => "切换原因，会展示给用户",
            },
            required: ["mode", "reason"],
            category: "系统",
            defer: true,
            read_only: true,
            concurrency_safe: true,
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
                skills: array => "Optional list of skill names to make available to this subagent. Only specified skills will be injected into the subagent's context. If omitted, no skills are injected. Use GetToolCatalog to discover available skill names.",
            },
            required: ["prompt"],
            category: "Agent 调度",
            defer: true,
            concurrency_safe: true,
        ),
        crate::tool_def!(
            "RunSubagentsSequentially",
            desc: "启动任务调度器，根据依赖关系自动并行执行任务",
            hint: "run tasks scheduler execute parallel",
            schema_desc: "【任务调度器】启动自动任务调度。系统将根据任务依赖关系（blocked_by）自动执行：无依赖的任务并行运行，阻塞任务等待前置完成后自动启动。创建完所有任务和依赖关系后调用此工具一次性调度执行。不要用于简单启动项目/运行命令；这类任务应直接用 StartBackgroundCommand/RunCommand。若调度返回失败，禁止继续创建重复任务，应复用现有任务 ID 修复或报告阻塞。",
            category: "Agent 调度",
            defer: true,
        )
    }
}
