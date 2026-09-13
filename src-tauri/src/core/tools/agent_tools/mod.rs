//! # agent_tools — Agent 专用工具模块
//!
//! 包含子代理执行引擎、技能加载、上下文压缩、记忆整理、方案审批等工具。
//!
//! ## 子模块
//! - `subagent`: 子代理执行引擎（独立 Agent Loop，支持只读/读写模式）
//! - `skill`: 技能加载
//! - `compact`: 上下文压缩
//! - `plan`: 方案审批工具
//!
//! ## 关键导出
//! - `run_subagent()`: 子代理执行引擎
//! - `load_skill()`: 按名称加载技能知识
//! - `compact()`: 手动触发上下文压缩
//! - `memory`: 全局记忆读写（ReadMemory / UpdateMemory / ConsolidateMemory）
//! - `propose_plan()`: 方案审批工具，推送方案到前端并阻塞等待用户决策

mod compact;
mod memory;
mod plan;
mod skill;
mod subagent;
mod switch_mode;

pub use compact::compact;
pub use memory::{
    consolidate_memory, extract_profile, read_memory, update_memory,
    MEMORY_BUDGET_CHARS, MEMORY_CONSOLIDATE_THRESHOLD_CHARS, MEMORY_SECTIONS, PROFILE_SECTIONS,
};
pub use plan::propose_plan;
pub use skill::load_skill;
pub use subagent::run_subagent;
pub use switch_mode::switch_work_mode;

use super::framework::agent_registry::AgentRegistry;
use super::framework::registry::ToolRegistry;
use crate::core::tools::framework;

/// GetToolCatalog 处理函数：从 ToolRegistry 获取延迟工具列表 + 从 skills 目录获取技能列表
pub async fn get_tool_catalog(
    _app: &tauri::AppHandle,
    _input: &serde_json::Value,
    _session_id: &str,
    intent: &str,
    work_mode: &str,
) -> framework::ToolCallResult {
    let mut out = String::new();

    // 能力边界先行：一次调用就给出确定结论，避免模型反复搜索被禁用的能力
    let caps = framework::capabilities::Capabilities::for_work_mode(work_mode);
    out.push_str("【本会话能力边界 · 系统强制】\n");
    out.push_str(&format!("- {}\n", caps.summary_line()));
    if !caps.write {
        out.push_str("- 修改文件 / 执行命令类工具在本模式下不存在，不要搜索或尝试调用它们\n");
    }
    out.push('\n');

    // 延迟工具列表
    let groups = ToolRegistry::global().get_deferred_by_category(intent, work_mode);
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
            "ReadMemory",
            desc: "读取全局记忆（跨会话用户档案）",
            hint: "read memory profile user preference global recall",
            schema_desc: "读取全局记忆的完整内容。每轮上下文里已带精简画像（身份 + 交互偏好），当你需要更细的信息（工程偏好、审美偏好、环境），或要在修改前确认原文时再调用。",
            props: {
                section: string => "可选：只读取某一个小节（身份/交互偏好/工程偏好/审美偏好/环境），省略则返回全文",
            },
            category: "系统",
            read_only: true,
            concurrency_safe: true,
        ),
        crate::tool_def!(
            "UpdateMemory",
            desc: "增改删全局记忆中的单条条目",
            hint: "update memory remember user preference forget",
            schema_desc: "维护跨会话的全局记忆（用户档案）。只在信息同时满足三条时才写：跨会话依然成立、跨项目依然成立、会影响之后怎么配合用户；用户明确说「记住…」时直接写（年龄、籍贯、学历、所在地、经历、求职或学习方向等档案信息），不受第三条限制。不要记录当前任务/项目状态、临时决定、会过期的配置（模型名、端口、版本号、API 提供商）、具体文件路径。一次记多条用 items 数组；单条增改删用 action/content；整体重写用 ConsolidateMemory。",
            props: {
                action: string enum expr ["add", "replace", "remove"] => "操作类型",
                section: string => "目标小节：身份 / 交互偏好 / 工程偏好 / 审美偏好 / 环境",
                content: string => "条目的新内容（一行，add/replace 必填）",
                match: string => "定位已有条目的关键词（replace/remove 必填）",
                items: array items {"type": "string"} => "一次新增多条条目（与 content 二选一，仅 add 用；比多次并发调用更安全）",
            },
            required: ["action", "section"],
            category: "系统",
        ),
        crate::tool_def!(
            "ConsolidateMemory",
            desc: "主动触发全局记忆整理",
            hint: "memory organize consolidate compact remember",
            schema_desc: "主动触发全局记忆整理：把当前记忆全文交给记忆整理者重写——合并同类项、删除过期与不合格条目、压缩冗余表述，不新增没有依据的事实。适合记忆出现重复/冗余，或结构损坏需要重建时使用。",
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
                task_breakdown: array items {
                    "type": "object",
                    "properties": {
                        "subject": {"type": "string", "description": "任务名。"},
                        "description": {"type": "string", "description": "任务详情，含预计耗时。"},
                        "depends_on": {"type": "array", "items": {"type": "integer"}, "description": "前置任务序号数组（1-based）。"},
                        "can_parallel_with": {"type": "array", "items": {"type": "integer"}, "description": "可并行任务序号数组（1-based）。"}
                    },
                    "required": ["subject"]
                } => "【必填】结构化任务分解列表，每项包含 subject（任务名）、description（详情，含预计耗时）、depends_on（前置任务序号数组）、can_parallel_with（可并行任务序号数组）",
            },
            required: ["title", "content", "task_breakdown"],
            category: "Agent 调度",
            defer: true,
        ),
        crate::tool_def!(
            "SwitchWorkMode",
            desc: "切换 Agent 工作模式",
            hint: "switch work mode edit plan",
            schema_desc: "切换 Agent 的工作模式，只支持 edit（编辑）和 plan（规划）。编辑模式下遇到复杂任务时，可切换到计划模式进行深度规划；规划完成后切回编辑模式执行。注意：权限档位（请求审批 / 帮我批准）由用户在界面上控制，不由本工具切换。此工具不影响用户类型（user/developer）。",
            props: {
                mode: string => "目标工作模式：edit（编辑）、plan（规划）",
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
                skills: array items {"type": "string"} => "Optional list of skill names to make available to this subagent. Only specified skills will be injected into the subagent's context. If omitted, no skills are injected. Use GetToolCatalog to discover available skill names.",
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
