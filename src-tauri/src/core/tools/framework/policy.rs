//! # policy.rs — 工具权限策略表 + 判定器
//!
//! 这是"执行前权限判定"的唯一一张策略表，回答一个问题：
//! **这次工具调用，应该直接放行、先问用户一句，还是直接拒绝。**
//!
//! ## Key Exports
//! - `ApprovalMode`：权限档位（请求审批 / 帮我批准 / 完全批准）
//! - `ToolClass`：工具归类（只看不碰 / 新建 / 改内容 / 删除 / 改名 / 跑命令 …）
//! - `TOOL_POLICIES` / `policy_for()`：唯一一张策略表与查询
//! - `judge()`：判定器（纯函数，接收已查明的事实）
//! - `files_in_patch()`：从补丁文本里数出涉及的文件
//!
//! ## 设计要点
//!
//! 1. **结构化优先**：判定看的是"用的是哪个工具、参数里的路径、文件是否已存在、
//!    一次影响几个文件"这类确定信息，而不是读用户的自然语言去猜。
//! 2. **一张表**：每个工具的归类只写在这里；新增工具如果没登记，测试会直接失败
//!    （见 `tests::every_registered_tool_is_classified`），避免出现"两张表各说各话"。
//! 3. **纯函数**：`judge()` 不做任何 IO、不加锁、不写日志，只接收已经准备好的事实，
//!    因此可以被完整单测；真正的落盘/统计在 `policy_shadow` 模块里。
//!
//! ## 档位（用户可选）
//!
//! - `RequestApproval`（请求审批，默认）：改文件、删文件、跑命令一律先问
//! - `AutoApprove`（帮我批准）：普通改动放行，只有风险操作才问
//! - `FullApprove`（完全批准）：新建/改内容/跑命令/批量一律放行，只保留硬边界——
//!   现有运行时检查要拒绝（Deny）、越界路径（Deny）、危险操作警示（每次都问 Ask）、
//!   未登记分类（Ask，同时提醒补表）

/// 用户的权限档位
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ApprovalMode {
    /// 请求审批：改动类操作一律先问用户
    RequestApproval,
    /// 帮我批准：只有风险操作才问
    AutoApprove,
    /// 完全批准：改动 / 命令 / 批量都不再弹卡，只留"拒绝类"与"危险警示类"硬边界
    FullApprove,
}

impl ApprovalMode {
    pub fn as_str(&self) -> &'static str {
        match self {
            ApprovalMode::RequestApproval => "request_approval",
            ApprovalMode::AutoApprove => "auto_approve",
            ApprovalMode::FullApprove => "full_approve",
        }
    }

    /// 中文名（写进日志/汇总，便于人看）
    pub fn label(&self) -> &'static str {
        match self {
            ApprovalMode::RequestApproval => "请求审批",
            ApprovalMode::AutoApprove => "帮我批准",
            ApprovalMode::FullApprove => "完全批准",
        }
    }

    /// 字符串 → 档位（**唯一**的字符串解析入口）。
    ///
    /// 会话偏好落盘、前端传参、档位切换清扫都走这里，避免各处各写一套 match
    /// （旧写法散落在 `policy_guard` 两处，新增档位时极易漏改一处、导致"切了档位没反应"）。
    ///
    /// 契约：**未知值一律退化为 `RequestApproval`**（最保守的一档），不 panic、
    /// 也不静默放宽——脏数据不该换来更宽的权限。
    #[allow(clippy::should_implement_trait)] // 有意做成"任意字符串兜底解析"，与 FromStr 的 Err 语义不同
    pub fn from_str(raw: &str) -> ApprovalMode {
        match raw {
            "auto_approve" => ApprovalMode::AutoApprove,
            "full_approve" => ApprovalMode::FullApprove,
            _ => ApprovalMode::RequestApproval,
        }
    }
}

/// 工具归类。判定规则按这个分类走，而不是按工具名散落判断。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ToolClass {
    /// 只看不碰（读文件、搜索、查状态）
    ReadOnly,
    /// 新建文件（若目标已存在，按"覆盖"处理）
    CreateFile,
    /// 改文件内容
    ModifyContent,
    /// 删除
    Delete,
    /// 改名 / 移动
    Rename,
    /// 跑命令（内容不确定，保守视为有风险）
    RunCommand,
    /// 起后台服务（长周期、影响面大）
    Background,
    /// 任务编排 / 派子代理
    Orchestrate,
    /// 会话与记忆管理
    SessionMgmt,
    /// 应用自身控制（模式切换等）
    AppControl,
}

impl ToolClass {
    pub fn as_str(&self) -> &'static str {
        match self {
            ToolClass::ReadOnly => "read_only",
            ToolClass::CreateFile => "create_file",
            ToolClass::ModifyContent => "modify_content",
            ToolClass::Delete => "delete",
            ToolClass::Rename => "rename",
            ToolClass::RunCommand => "run_command",
            ToolClass::Background => "background",
            ToolClass::Orchestrate => "orchestrate",
            ToolClass::SessionMgmt => "session_mgmt",
            ToolClass::AppControl => "app_control",
        }
    }

    /// 中文标签（写进人话汇总）
    pub fn label(&self) -> &'static str {
        match self {
            ToolClass::ReadOnly => "只看不碰",
            ToolClass::CreateFile => "新建文件",
            ToolClass::ModifyContent => "改文件内容",
            ToolClass::Delete => "删除文件",
            ToolClass::Rename => "改名/移动",
            ToolClass::RunCommand => "跑命令",
            ToolClass::Background => "起后台服务",
            ToolClass::Orchestrate => "任务编排",
            ToolClass::SessionMgmt => "会话与记忆",
            ToolClass::AppControl => "应用控制",
        }
    }

    /// 会不会改变用户项目里的东西（用于统计"这一轮改了几个文件"）
    pub fn mutates_project(&self) -> bool {
        matches!(
            self,
            ToolClass::CreateFile
                | ToolClass::ModifyContent
                | ToolClass::Delete
                | ToolClass::Rename
        )
    }

    /// 是否进入"默认要问"的档位判定区（规则 4 的入口名单）。
    ///
    /// 名单：删除 / 改名 / 跑命令 / 后台。进入本区不等于一定问——区内三段判定：
    /// ① 只读命令直接放行；② 帮我批准档下 `auto_approved_in_auto_mode()` 命中且无警示 → 放行；
    /// ③ 其余落回 Ask。**当前两档都问的只剩删除**（改名/命令/后台在帮我批准档均已放行）。
    pub fn always_asks(&self) -> bool {
        matches!(
            self,
            ToolClass::Delete | ToolClass::Rename | ToolClass::RunCommand | ToolClass::Background
        )
    }

    /// 该类别在"帮我批准（AutoApprove）"档下是否自动放行。
    ///
    /// 语义（2026-09-18 沐拍板）：AutoApprove 档 = "别烦我，我看着结果就行"。
    /// 用户主动切到这一档即声明接受放行代价，因此**非危险操作**不再弹卡（危险命令仍必问，
    /// 见 `ask_always_repeated` / 危险警示名单——那是唯一硬边界，不受档位影响）。
    ///
    /// 覆盖范围：`CreateFile` / `ModifyContent` / `RunCommand` / `Background` / `Rename`。
    /// - 文件类：写操作走补丁暂存 + 快照回滚，旧内容找得回来；
    /// - 命令类：只放行"非危险"的（危险警示命中在规则 4 之前仍 Ask）；
    /// - **`Background` 也在列**（2026-09-19 沐拍板调整；2026-11-20 合并独立后台工具后
    ///   由 `RunCommand` + `run_in_background=true` 派生）：帮批准档连后台服务也不再问——
    ///   与 A 的档位语义自洽（"别烦我"）；带危险警示的后台命令仍必问（同一套
    ///   `inspect_existing_guards`）。请求审批档下后台**保持恒问**（每次都要人确认）。
    /// - **`Rename` 也在列**（2026-09-19 沐拍板"和 A 同逻辑"）：改名与编辑/覆盖完全同权——
    ///   `Patch::RenameFile` 在回滚系统完整支持（snapshot apply_patch / replay 精准清孤儿
    ///   路径把文件挪回原位 / patch 有专项测试），且它本就共享 edit_project 键
    ///   （label"新建 / 编辑 / 改名都算"）。闭合 E 之后"帮批准档编辑静默、改名叫卡"的缝隙。
    ///   请求审批档下改名照旧问，可点「本项目允许」记忆。
    /// - `Delete` 不在列：删除虽有软删除兜底，但语义上仍属"用户亲自批"的范畴。
    pub fn auto_approved_in_auto_mode(&self) -> bool {
        matches!(
            self,
            ToolClass::CreateFile
                | ToolClass::ModifyContent
                | ToolClass::RunCommand
                | ToolClass::Background
                | ToolClass::Rename
        )
    }

    /// 该类别在"完全批准（FullApprove）"档下是否自动放行。
    ///
    /// 范围（2026-11 收紧后的最终口径）：**除只读外全部改动类**——
    /// 新建 / 改内容 / 删除 / 改名 / 跑命令 / 起后台服务。
    /// 共同前提是项目会话（档位本身要求绑定项目），且各自有兜底：
    /// - 文件类走补丁暂存 + 快照回滚，删除进回收站可恢复；
    /// - 命令与后台走权限审计日志（`permission_audit_logger`），事后可追溯；
    /// - 真正不可撤回的操作（递归强删、`git reset --hard` 等）仍由
    ///   `destructive_warning` 单独拦下问一次，不看档位。
    pub fn auto_approved_in_full_mode(&self) -> bool {
        matches!(
            self,
            ToolClass::CreateFile
                | ToolClass::ModifyContent
                | ToolClass::Delete
                | ToolClass::Rename
                | ToolClass::RunCommand
                | ToolClass::Background
        )
    }
}

/// 一个工具的策略条目
pub struct ToolPolicy {
    pub class: ToolClass,
    /// 参数字段名：哪个字段是"目标路径"（可能多个，例如改名有源和目标）
    pub path_fields: &'static [&'static str],
    /// 参数字段名：命令文本（只有命令类工具有）
    pub command_field: Option<&'static str>,
    /// 参数字段名：补丁文本（需要从文本里数出影响几个文件）
    pub patch_field: Option<&'static str>,
}

const READ_ONLY: ToolClass = ToolClass::ReadOnly;

/// **唯一一张策略表**：工具名 → 归类 + 参数字段。
pub const TOOL_POLICIES: &[(&str, ToolPolicy)] = &[
    // ── 只看不碰 ──
    (
        "ReadFile",
        ToolPolicy {
            class: READ_ONLY,
            path_fields: &["path"],
            command_field: None,
            patch_field: None,
        },
    ),
    (
        "ReadFileSkeleton",
        ToolPolicy {
            class: READ_ONLY,
            path_fields: &["path"],
            command_field: None,
            patch_field: None,
        },
    ),
    (
        "SymbolSearch",
        ToolPolicy {
            class: READ_ONLY,
            path_fields: &["dir", "path"],
            command_field: None,
            patch_field: None,
        },
    ),
    (
        "SearchRepo",
        ToolPolicy {
            class: READ_ONLY,
            path_fields: &["dir"],
            command_field: None,
            patch_field: None,
        },
    ),
    (
        "FindFiles",
        ToolPolicy {
            class: READ_ONLY,
            path_fields: &["dir"],
            command_field: None,
            patch_field: None,
        },
    ),
    (
        "ListDirectory",
        ToolPolicy {
            class: READ_ONLY,
            path_fields: &["path"],
            command_field: None,
            patch_field: None,
        },
    ),
    (
        "CheckBackgroundCommand",
        ToolPolicy {
            class: READ_ONLY,
            path_fields: &[],
            command_field: None,
            patch_field: None,
        },
    ),
    (
        "ListTasks",
        ToolPolicy {
            class: READ_ONLY,
            path_fields: &[],
            command_field: None,
            patch_field: None,
        },
    ),
    (
        "GetTask",
        ToolPolicy {
            class: READ_ONLY,
            path_fields: &[],
            command_field: None,
            patch_field: None,
        },
    ),
    (
        "SummarizeTasks",
        ToolPolicy {
            class: READ_ONLY,
            path_fields: &[],
            command_field: None,
            patch_field: None,
        },
    ),
    (
        "ReadMemory",
        ToolPolicy {
            class: READ_ONLY,
            path_fields: &[],
            command_field: None,
            patch_field: None,
        },
    ),
    (
        "GetToolCatalog",
        ToolPolicy {
            class: READ_ONLY,
            path_fields: &[],
            command_field: None,
            patch_field: None,
        },
    ),
    (
        "DiscoverTools",
        ToolPolicy {
            class: READ_ONLY,
            path_fields: &[],
            command_field: None,
            patch_field: None,
        },
    ),
    (
        "LoadSkill",
        ToolPolicy {
            class: READ_ONLY,
            path_fields: &[],
            command_field: None,
            patch_field: None,
        },
    ),
    // ── 改文件 ──
    (
        "WriteFile",
        ToolPolicy {
            class: ToolClass::CreateFile,
            path_fields: &["path"],
            command_field: None,
            patch_field: None,
        },
    ),
    (
        "EditFile",
        ToolPolicy {
            class: ToolClass::ModifyContent,
            path_fields: &["path"],
            command_field: None,
            patch_field: None,
        },
    ),
    (
        "ApplyPatch",
        ToolPolicy {
            class: ToolClass::ModifyContent,
            path_fields: &[],
            command_field: None,
            patch_field: Some("patch"),
        },
    ),
    (
        "EditNotebook",
        ToolPolicy {
            class: ToolClass::ModifyContent,
            path_fields: &["notebook_path"],
            command_field: None,
            patch_field: None,
        },
    ),
    (
        "DeleteFile",
        ToolPolicy {
            class: ToolClass::Delete,
            path_fields: &["path"],
            command_field: None,
            patch_field: None,
        },
    ),
    (
        "RenameFile",
        ToolPolicy {
            class: ToolClass::Rename,
            path_fields: &["path", "new_path"],
            command_field: None,
            patch_field: None,
        },
    ),
    // ── 命令 ──
    // （SetWorkspace 已退役：见 system_tools/mod.rs 模块注释）
    (
        "RunCommand",
        ToolPolicy {
            class: ToolClass::RunCommand,
            path_fields: &[],
            command_field: Some("command"),
            patch_field: None,
        },
    ),
    // ── 编排 / 会话 / 应用 ──
    (
        "CreateTask",
        ToolPolicy {
            class: ToolClass::Orchestrate,
            path_fields: &[],
            command_field: None,
            patch_field: None,
        },
    ),
    (
        "UpdateTask",
        ToolPolicy {
            class: ToolClass::Orchestrate,
            path_fields: &[],
            command_field: None,
            patch_field: None,
        },
    ),
    (
        "DeleteTask",
        ToolPolicy {
            class: ToolClass::Orchestrate,
            path_fields: &[],
            command_field: None,
            patch_field: None,
        },
    ),
    (
        "UpdateTodos",
        ToolPolicy {
            class: ToolClass::Orchestrate,
            path_fields: &[],
            command_field: None,
            patch_field: None,
        },
    ),
    (
        "RunSubagent",
        ToolPolicy {
            class: ToolClass::Orchestrate,
            path_fields: &[],
            command_field: None,
            patch_field: None,
        },
    ),
    (
        "RunSubagentsSequentially",
        ToolPolicy {
            class: ToolClass::Orchestrate,
            path_fields: &[],
            command_field: None,
            patch_field: None,
        },
    ),
    (
        "ProposePlan",
        ToolPolicy {
            class: ToolClass::Orchestrate,
            path_fields: &[],
            command_field: None,
            patch_field: None,
        },
    ),
    (
        "CompactConversation",
        ToolPolicy {
            class: ToolClass::SessionMgmt,
            path_fields: &[],
            command_field: None,
            patch_field: None,
        },
    ),
    (
        "ConsolidateMemory",
        ToolPolicy {
            class: ToolClass::SessionMgmt,
            path_fields: &[],
            command_field: None,
            patch_field: None,
        },
    ),
    (
        "UpdateMemory",
        ToolPolicy {
            class: ToolClass::SessionMgmt,
            path_fields: &[],
            command_field: None,
            patch_field: None,
        },
    ),
    (
        "SwitchWorkMode",
        ToolPolicy {
            class: ToolClass::AppControl,
            path_fields: &[],
            command_field: None,
            patch_field: None,
        },
    ),
    (
        "ExecuteTool",
        ToolPolicy {
            class: ToolClass::AppControl,
            path_fields: &[],
            command_field: None,
            patch_field: None,
        },
    ),
];

/// 查策略表；没登记返回 `None`（调用方要把它标成"未分类"）
pub fn policy_for(tool: &str) -> Option<&'static ToolPolicy> {
    TOOL_POLICIES
        .iter()
        .find(|(name, _)| *name == tool)
        .map(|(_, policy)| policy)
}

/// 判定结果
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Outcome {
    /// 直接放行
    Allow,
    /// 先问用户
    Ask,
    /// 直接拒绝
    Deny,
}

impl Outcome {
    pub fn as_str(&self) -> &'static str {
        match self {
            Outcome::Allow => "allow",
            Outcome::Ask => "ask",
            Outcome::Deny => "deny",
        }
    }

    pub fn label(&self) -> &'static str {
        match self {
            Outcome::Allow => "直接放行",
            Outcome::Ask => "要问你",
            Outcome::Deny => "直接拒绝",
        }
    }
}

/// 判定输入：全部是"已经查明的事实"，判定器本身不做任何 IO
#[derive(Debug, Clone, Default)]
pub struct JudgementInput {
    /// 本次调用涉及的目标路径（相对或绝对都行，只用于统计与展示）
    pub targets: Vec<String>,
    /// 落在项目外面的路径（调用方用沙箱规则算好）
    pub out_of_workspace: Vec<String>,
    /// 本次调用一次影响几个文件（补丁可能一次动多个）
    pub files_in_call: usize,
    /// 本轮累计改动过的不同文件数（含本次）
    pub files_in_turn: usize,
    /// 目标文件是否已经存在（覆盖判断）；无法确定时用 None
    pub target_exists: Option<bool>,
    /// 现有运行时检查给出的"本该直接拒绝"结论（例如命令安全检查命中）
    pub existing_deny_reason: Option<String>,
    /// 现有运行时检查给出的提醒（例如"长周期命令建议后台执行"）
    pub existing_warning: Option<String>,
    /// 现有检查命中的是不是**破坏性**操作（递归强删、`git reset --hard`、`DROP TABLE` 等）。
    ///
    /// 与 `existing_warning` 的区别：后者是所有提醒的合并文案，包含长周期、sleep、
    /// 命令替换这类**可撤回**的提醒。完全批准档只用本字段决定要不要问——
    /// 普通提醒在该档放行（审计留痕），破坏性操作仍问一次。
    pub destructive_warning: bool,
    /// 命令是不是"只读命令"（今天这类命令本来就免弹窗，新规则也应当直接放行）
    pub command_is_readonly: bool,
    /// 本次调用是否是 RunCommand 的后台模式（run_in_background=true）。
    ///
    /// 2026-11-20 合并 StartBackgroundCommand 后，后台语义由入参派生：
    /// judge() 据此把类别视同 Background（卡片文案"起后台服务"、只读保护直接拒绝），
    /// 而不是再依赖一个独立的工具名。
    pub background_run: bool,
}

/// 一次判定的结果
#[derive(Debug, Clone)]
pub struct ShadowDecision {
    /// 工具归类；未登记分类时为 None
    pub class: Option<ToolClass>,
    pub outcome: Outcome,
    /// 人话原因（写进汇总，便于复盘）
    pub reason: String,
}

/// 批量阈值：同一轮改到第 3 个文件时问（也可被一次调用影响 3 个文件触发）
pub const BATCH_FILE_THRESHOLD: usize = 3;

/// 判定：给定事实 + 档位，这次调用应该放行 / 问 / 拒绝。
///
/// 规则顺序（先命中先返回）：
/// 1. 现有运行时检查已经判定要拒绝 → 拒绝（沿用同一套结论，不另立标准）
/// 2. 路径跑出项目 → 拒绝
/// 3. 工具未登记分类 → 问（保守，同时提醒补表）
/// 4. 默认就要问的类别（删/改名/跑命令/后台）→ 问；
///    例外：只读命令直接放行；档位豁免（帮我批准 / 完全批准）下、该类别被该档覆盖
///    且无「会拦住放行」的警示 → 放行（完全批准只看破坏性警示）
/// 5. 破坏性操作警示 → 问（**所有档位都不豁免**；普通提醒在完全批准档放行，
///    覆盖已有文件已于 2026-09-18 摘出本条，与编辑同权走规则 7）
/// 6. 批量（本次调用 ≥3 个文件，或本轮第 3 个文件）→ 问（完全批准档不问）
/// 7. 其余按档位：请求审批档问，帮我批准 / 完全批准档放行
pub fn judge(tool: &str, mode: ApprovalMode, input: &JudgementInput) -> ShadowDecision {
    let policy = policy_for(tool);
    // 后台语义由入参派生（2026-11-20 合并 StartBackgroundCommand 后，后台服务唯一入口是
    // RunCommand 的 run_in_background=true）：类别视同 Background，权限卡片按"起后台服务"
    // 展示，只读保护与长周期警示的口径也跟着入参走，而不是再靠一个独立工具名。
    let class = if input.background_run {
        Some(ToolClass::Background)
    } else {
        policy.map(|p| p.class)
    };

    // 1. 现有运行时检查（命令安全检查等）已经要拒绝
    if let Some(reason) = &input.existing_deny_reason {
        return ShadowDecision {
            class,
            outcome: Outcome::Deny,
            reason: reason.clone(),
        };
    }

    // 2. 项目外路径（会话挂了项目 = 沙箱会话，边界即项目目录；非沙箱会话 workspace 为 None，不会走到这里）
    if !input.out_of_workspace.is_empty() {
        return ShadowDecision {
            class,
            outcome: Outcome::Deny,
            reason: format!(
                "路径在会话绑定的项目目录（沙箱边界）外：{}。需要操作其他目录，请让用户新建不挂项目的会话（非沙箱会话）。",
                input.out_of_workspace.join(", ")
            ),
        };
    }

    // 3. 未登记分类
    let Some(class_value) = class else {
        return ShadowDecision {
            class: None,
            outcome: Outcome::Ask,
            reason: "工具未登记分类（需要补策略表）".to_string(),
        };
    };

    // 4. 默认就要问的类别
    if class_value.always_asks() {
        // 只读命令（Get-ChildItem 这类）本来就免弹窗，新规则同样直接放行，
        // 否则观察数据会把"免询问的只读命令"算成打扰次数
        if input.command_is_readonly {
            return ShadowDecision {
                class,
                outcome: Outcome::Allow,
                reason: "只读命令（免询问）".to_string(),
            };
        }
        // 命令类在档位豁免下自动放行（2026-09-18 沐拍板 A：AutoApprove = "别烦我"；
        // 2026-11 新增 FullApprove = "连批量也别拦"）。
        // ⚠️ 危险警示命令不在这里放行——它在上面的只读判断之后、且规则 5 前仍需 Ask；
        //    为保持"危险永不自动放行"的硬边界，这里先挡一道：有警示就落到规则 5 去 Ask。
        // 警示的分母按档位区分：完全批准只看**破坏性**警示（长周期、sleep、命令替换
        // 这类可撤回的提醒在该档放行，审计留痕）；其余档位仍看全部警示。
        let warning_blocks_auto = match mode {
            ApprovalMode::FullApprove => input.destructive_warning,
            _ => input.existing_warning.is_some(),
        };
        if class_auto_approved(class_value, mode) && !warning_blocks_auto {
            return ShadowDecision {
                class,
                outcome: Outcome::Allow,
                reason: format!("{}（{}档自动放行）", class_value.label(), mode.label()),
            };
        }
        // 原因只说"为什么要问你"（例如"跑命令"）。
        // 现有守卫的警告文案是**补充提示**，由调用方单独记录并在报告里单独成段，
        // 不能拼进原因里——那会把"问你的理由"和"附加说明"混成一团（首次实测踩到过）。
        return ShadowDecision {
            class,
            outcome: Outcome::Ask,
            reason: class_value.label().to_string(),
        };
    }

    // 5. 危险命令警示（这次询问**每次都要问**，不能被"本次会话都允许"吞掉）。
    //    命令类在规则 4（always ask）就已 Ask，走不到这里；本条对命令类是防御性一致。
    //    覆盖已有文件已摘出本条（2026-09-18 沐拍板）：与普通编辑同权，走规则 7 按档位判定。
    let repeated_warning = match mode {
        ApprovalMode::FullApprove => input.destructive_warning,
        _ => input.existing_warning.is_some(),
    };
    if ask_always_repeated(repeated_warning) {
        return ShadowDecision {
            class,
            outcome: Outcome::Ask,
            reason: "危险操作警示".to_string(),
        };
    }

    // 6. 批量
    // 批量（本次调用 ≥3 个文件 / 本轮第 3 个文件）**可以**被"本次会话都允许"吞掉：
    // 用户已经说过"这个目录里的改动我放心"，此时还要他第 3、4、5… 个文件各点一次，
    // 只会把人逼回"每次都点允许"，反而失去护栏的意义。
    // 完全批准档（FullApprove）连批量也不再问：该档的语义就是"批量也别拦"（改动仍有快照可回滚）。
    if class_value.mutates_project() && mode != ApprovalMode::FullApprove {
        if input.files_in_call >= BATCH_FILE_THRESHOLD {
            return ShadowDecision {
                class,
                outcome: Outcome::Ask,
                reason: format!("一次调用影响 {} 个文件（批量）", input.files_in_call),
            };
        }
        if input.files_in_turn >= BATCH_FILE_THRESHOLD {
            return ShadowDecision {
                class,
                outcome: Outcome::Ask,
                reason: format!(
                    "本轮第 {} 个文件（批量阈值 {}）",
                    input.files_in_turn, BATCH_FILE_THRESHOLD
                ),
            };
        }
    }

    // 7. 按档位
    match (class_value, mode) {
        (ToolClass::CreateFile | ToolClass::ModifyContent, ApprovalMode::AutoApprove) => {
            ShadowDecision {
                class,
                outcome: Outcome::Allow,
                reason: format!("{}（帮我批准档自动放行）", class_value.label()),
            }
        }
        (ToolClass::CreateFile | ToolClass::ModifyContent, ApprovalMode::FullApprove) => {
            ShadowDecision {
                class,
                outcome: Outcome::Allow,
                reason: format!("{}（完全批准档自动放行）", class_value.label()),
            }
        }
        (ToolClass::CreateFile | ToolClass::ModifyContent, ApprovalMode::RequestApproval) => {
            ShadowDecision {
                class,
                outcome: Outcome::Ask,
                reason: format!("{}（请求审批档）", class_value.label()),
            }
        }
        // 命令类在"帮我批准"档放行（规则 4 已拦截含警示的命令；此处是命令类无警示时的放行路径，
        // 与规则 4 的放行分支互为兜底——规则 4 早返回，这里是语义完整性）。
        (ToolClass::RunCommand, ApprovalMode::AutoApprove | ApprovalMode::FullApprove) => {
            ShadowDecision {
                class,
                outcome: Outcome::Allow,
                reason: format!("跑命令（{}档自动放行）", mode.label()),
            }
        }
        _ => ShadowDecision {
            class,
            outcome: Outcome::Allow,
            reason: class_value.label().to_string(),
        },
    }
}

/// 某类别在某档位下是否自动放行（规则 4 与规则 7 共用同一份判据）。
///
/// 为什么集中在一处：规则 4（默认要问的类别区）与规则 7（按档位收尾）都要回答
/// "这一档放不放它"，两处若各写一份，新增档位时必然漂移。
fn class_auto_approved(class: ToolClass, mode: ApprovalMode) -> bool {
    match mode {
        ApprovalMode::RequestApproval => false,
        ApprovalMode::AutoApprove => class.auto_approved_in_auto_mode(),
        ApprovalMode::FullApprove => class.auto_approved_in_full_mode(),
    }
}

/// 这次询问是不是"每次都要问"——即不能被"本次会话都允许"记下来免掉。
///
/// 现有一种：
/// **危险命令警示命中**（改造①）。`Remove-Item -Recurse`、`git reset --hard`
/// 这类不可恢复操作，登记"会话允许"等于说"本会话内随便递归强删"，语义不成立；
/// 卡片照弹、警示照显，但不给「本次会话都允许」按钮。
///
/// 覆盖已有文件**已摘出本名单**（2026-09-18 沐拍板）：覆盖与普通编辑完全同权——
/// WriteFile/EditFile 都走"补丁暂存 + 快照回滚"链路（file_tools/workspace.rs），
/// 旧内容找得回来，语义上不需要每次亲手批。
///
/// **唯一口径**：`judge()` 规则 5 与 `policy_guard::enforce()` 的会话允许检查都调用这里，
/// 避免"什么时候问"和"问了之后能不能记住"两套判据各说各话。
pub fn ask_always_repeated(has_warning: bool) -> bool {
    has_warning
}

/// 从补丁文本里数出涉及的文件（`*** Update File:` / `*** Add File:` / `*** Delete File:` / `+++ b/x`）。
///
/// 只用于"观察"阶段统计批量规模，不参与拦截判定——所以允许它粗略。
pub fn files_in_patch(patch: &str) -> Vec<String> {
    let mut files: Vec<String> = Vec::new();
    let mut push = |path: &str| {
        let cleaned = path.trim().trim_start_matches("b/").trim();
        if !cleaned.is_empty() && cleaned != "/dev/null" {
            let owned = cleaned.to_string();
            if !files.contains(&owned) {
                files.push(owned);
            }
        }
    };
    for line in patch.lines() {
        let trimmed = line.trim_start();
        for marker in ["*** Update File:", "*** Add File:", "*** Delete File:"] {
            if let Some(rest) = trimmed.strip_prefix(marker) {
                push(rest);
            }
        }
        if let Some(rest) = trimmed.strip_prefix("+++ ") {
            push(rest);
        }
    }
    files
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::tools::framework::registry::ToolRegistry;

    fn allow_input() -> JudgementInput {
        JudgementInput {
            targets: vec!["src/app.js".to_string()],
            target_exists: Some(false),
            files_in_call: 1,
            files_in_turn: 1,
            ..Default::default()
        }
    }

    /// 全部档位。新档位加进来时，所有"各档都要成立"的用例自动覆盖到。
    fn all_modes() -> [ApprovalMode; 3] {
        [
            ApprovalMode::RequestApproval,
            ApprovalMode::AutoApprove,
            ApprovalMode::FullApprove,
        ]
    }

    #[test]
    fn every_registered_tool_is_classified() {
        // 防漏：新增工具必须登记分类，否则这里直接失败
        let mut missing: Vec<&str> = Vec::new();
        for name in ToolRegistry::global().all_tool_names() {
            if policy_for(name).is_none() {
                missing.push(name);
            }
        }
        assert!(
            missing.is_empty(),
            "以下工具没有登记权限策略分类：{:?}",
            missing
        );
    }

    #[test]
    fn read_only_work_is_allowed_in_all_modes() {
        for mode in all_modes() {
            let decision = judge("ReadFile", mode, &allow_input());
            assert_eq!(decision.outcome, Outcome::Allow, "mode={:?}", mode);
        }
    }

    #[test]
    fn delete_asks_until_full_approve() {
        // 2026-11 收紧后：删除进回收站可恢复，完全批准档放行；
        // 请求审批 / 帮我批准两档仍要用户亲自批。
        for mode in [ApprovalMode::RequestApproval, ApprovalMode::AutoApprove] {
            let decision = judge("DeleteFile", mode, &allow_input());
            assert_eq!(decision.outcome, Outcome::Ask, "mode={:?}", mode);
            assert_eq!(decision.reason, "删除文件");
        }
        assert_eq!(
            judge("DeleteFile", ApprovalMode::FullApprove, &allow_input()).outcome,
            Outcome::Allow
        );
    }

    #[test]
    fn command_asks_in_request_mode_and_silent_in_auto_mode() {
        // 2026-09-18 沐拍板 A：AutoApprove 档 = "别烦我，我看着结果就行"——
        // 非危险命令自动放行；请求审批档照旧问。
        assert_eq!(
            judge("RunCommand", ApprovalMode::RequestApproval, &allow_input()).outcome,
            Outcome::Ask
        );
        assert_eq!(
            judge("RunCommand", ApprovalMode::AutoApprove, &allow_input()).outcome,
            Outcome::Allow
        );
    }

    #[test]
    fn background_asks_in_request_mode_and_silent_in_auto_mode() {
        // 2026-09-19 沐拍板调整；2026-11-20 合并独立后台工具后，由
        // RunCommand + run_in_background=true 派生同一口径：
        // 请求审批档后台恒问（每次人确认）；帮我批准档后台放行（"别烦我"语义）。
        let input = JudgementInput {
            background_run: true,
            ..allow_input()
        };
        assert_eq!(
            judge("RunCommand", ApprovalMode::RequestApproval, &input).outcome,
            Outcome::Ask
        );
        assert_eq!(
            judge("RunCommand", ApprovalMode::AutoApprove, &input).outcome,
            Outcome::Allow
        );
    }

    #[test]
    fn dangerous_background_is_never_auto_approved() {
        // A 的硬边界同样覆盖后台：带危险警示的后台命令在帮我批准档也必须问
        let input = JudgementInput {
            background_run: true,
            existing_warning: Some("检测到递归强制删除".to_string()),
            ..allow_input()
        };
        assert_eq!(
            judge("RunCommand", ApprovalMode::AutoApprove, &input).outcome,
            Outcome::Ask
        );
    }

    #[test]
    fn dangerous_command_is_never_auto_approved() {
        // 硬边界：破坏性操作在任何档位（含完全批准）都必须问。
        // 注意 `destructive_warning` 必为 true——真实采集来自
        // `security::get_destructive_warning`，普通提醒（长周期等）不置此位。
        let input = JudgementInput {
            existing_warning: Some("检测到递归强制删除".to_string()),
            destructive_warning: true,
            ..allow_input()
        };
        for mode in all_modes() {
            assert_eq!(
                judge("RunCommand", mode, &input).outcome,
                Outcome::Ask,
                "mode={:?}",
                mode
            );
        }
    }

    // ── 完全批准（FullApprove）档 ──

    #[test]
    fn approval_mode_strings_are_parsed_centrally() {
        // as_str / label / from_str 必须一一对应（from_str 是唯一的字符串解析入口）
        assert_eq!(ApprovalMode::RequestApproval.as_str(), "request_approval");
        assert_eq!(ApprovalMode::AutoApprove.as_str(), "auto_approve");
        assert_eq!(ApprovalMode::FullApprove.as_str(), "full_approve");
        assert_eq!(ApprovalMode::FullApprove.label(), "完全批准");
        for mode in all_modes() {
            assert_eq!(ApprovalMode::from_str(mode.as_str()), mode);
        }
        // 未知值 / 空串一律退化为最保守的请求审批档（脏数据不能换来更宽权限）
        assert_eq!(
            ApprovalMode::from_str("whatever"),
            ApprovalMode::RequestApproval
        );
        assert_eq!(ApprovalMode::from_str(""), ApprovalMode::RequestApproval);
    }

    #[test]
    fn full_approve_silences_file_writes_and_commands() {
        let input = allow_input();
        // 新建（WriteFile）与改内容（EditFile / ApplyPatch）都直接放行
        for tool in ["WriteFile", "EditFile", "ApplyPatch"] {
            let decision = judge(tool, ApprovalMode::FullApprove, &input);
            assert_eq!(decision.outcome, Outcome::Allow, "tool={}", tool);
        }
        // 普通命令（无警示）也直接放行
        let decision = judge("RunCommand", ApprovalMode::FullApprove, &input);
        assert_eq!(decision.outcome, Outcome::Allow);
        assert!(
            decision.reason.contains("完全批准"),
            "{}",
            decision.reason
        );
        // 覆盖已有文件同理（回滚系统兜底）
        let overwrite = JudgementInput {
            target_exists: Some(true),
            ..allow_input()
        };
        assert_eq!(
            judge("WriteFile", ApprovalMode::FullApprove, &overwrite).outcome,
            Outcome::Allow
        );
    }

    #[test]
    fn full_approve_ignores_batch_threshold() {
        // 批量不再拦：一次调用 5 个文件、本轮第 3 个文件都直接放行
        let by_call = JudgementInput {
            files_in_call: 5,
            ..allow_input()
        };
        assert_eq!(
            judge("ApplyPatch", ApprovalMode::FullApprove, &by_call).outcome,
            Outcome::Allow
        );
        let by_turn = JudgementInput {
            files_in_turn: BATCH_FILE_THRESHOLD,
            ..allow_input()
        };
        assert_eq!(
            judge("EditFile", ApprovalMode::FullApprove, &by_turn).outcome,
            Outcome::Allow
        );
        // 对照：同样入参在帮我批准档仍会问（确认阈值只是被完全批准档跳过，而不是失效）
        assert_eq!(
            judge("EditFile", ApprovalMode::AutoApprove, &by_turn).outcome,
            Outcome::Ask
        );
    }

    #[test]
    fn full_approve_keeps_hard_boundaries() {
        // 1) 现有运行时检查要拒绝 → 仍 Deny
        let deny = JudgementInput {
            existing_deny_reason: Some("检测到危险 PowerShell 命令".to_string()),
            ..allow_input()
        };
        let decision = judge("RunCommand", ApprovalMode::FullApprove, &deny);
        assert_eq!(decision.outcome, Outcome::Deny);
        assert_eq!(decision.reason, "检测到危险 PowerShell 命令");

        // 2) 越界路径 → 仍 Deny
        let outside = JudgementInput {
            out_of_workspace: vec!["C:/Windows/System32".to_string()],
            ..allow_input()
        };
        for tool in ["ReadFile", "WriteFile", "RunCommand"] {
            assert_eq!(
                judge(tool, ApprovalMode::FullApprove, &outside).outcome,
                Outcome::Deny,
                "tool={}",
                tool
            );
        }

        // 3) 未登记分类 → 仍 Ask（并提醒补表）
        let decision = judge("SomethingNew", ApprovalMode::FullApprove, &allow_input());
        assert_eq!(decision.outcome, Outcome::Ask);
        assert!(decision.class.is_none());
        assert!(decision.reason.contains("未登记分类"));

        // 4) 破坏性操作警示 → 仍 Ask（每次都要问的硬边界，完全批准也不豁免）
        let destructive = JudgementInput {
            existing_warning: Some("检测到递归强制删除".to_string()),
            destructive_warning: true,
            ..allow_input()
        };
        for tool in ["RunCommand", "WriteFile"] {
            assert_eq!(
                judge(tool, ApprovalMode::FullApprove, &destructive).outcome,
                Outcome::Ask,
                "tool={}",
                tool
            );
        }

        // 5) 普通提醒（长周期 / sleep / 命令替换）→ 完全批准档放行，审计留痕即可
        let ordinary = JudgementInput {
            existing_warning: Some("检测到长周期命令，建议后台执行".to_string()),
            destructive_warning: false,
            ..allow_input()
        };
        for tool in ["RunCommand", "WriteFile"] {
            assert_eq!(
                judge(tool, ApprovalMode::FullApprove, &ordinary).outcome,
                Outcome::Allow,
                "tool={}",
                tool
            );
        }
        // 对照：同样普通提醒在帮我批准档仍问（收紧只作用于完全批准档）
        assert_eq!(
            judge("RunCommand", ApprovalMode::AutoApprove, &ordinary).outcome,
            Outcome::Ask
        );
    }

    #[test]
    fn full_approve_covers_delete_rename_and_background() {
        // 2026-11 收紧后的最终口径：完全批准覆盖删除 / 改名 / 后台服务——
        // 档位本身要求绑定项目，删除进回收站、改名与编辑同权、后台有进程树守卫与审计。
        assert_eq!(
            judge("RenameFile", ApprovalMode::FullApprove, &allow_input()).outcome,
            Outcome::Allow
        );
        assert_eq!(
            judge("DeleteFile", ApprovalMode::FullApprove, &allow_input()).outcome,
            Outcome::Allow
        );
        let background = JudgementInput {
            background_run: true,
            ..allow_input()
        };
        assert_eq!(
            judge("RunCommand", ApprovalMode::FullApprove, &background).outcome,
            Outcome::Allow
        );
        // 但破坏性操作在后台模式下仍要问
        let destructive_bg = JudgementInput {
            background_run: true,
            existing_warning: Some("检测到递归强制删除".to_string()),
            destructive_warning: true,
            ..allow_input()
        };
        assert_eq!(
            judge("RunCommand", ApprovalMode::FullApprove, &destructive_bg).outcome,
            Outcome::Ask
        );
    }

    #[test]
    fn rename_asks_in_request_mode_and_silent_in_auto_mode() {
        // 2026-09-19 沐拍板"改名也降级，和 A 同逻辑"：改名与编辑/覆盖完全同权
        // （Patch::RenameFile 回滚链完整——snapshot/replay/patch 三处；共享 edit_project 键）。
        assert_eq!(
            judge("RenameFile", ApprovalMode::RequestApproval, &allow_input()).outcome,
            Outcome::Ask
        );
        assert_eq!(
            judge("RenameFile", ApprovalMode::AutoApprove, &allow_input()).outcome,
            Outcome::Allow
        );
    }

    #[test]
    fn ask_always_repeated_covers_warning_only() {
        // 改造①：危险命令警示命中 → 无论类别都"每次都要问"（不给会话允许键）
        assert!(ask_always_repeated(true));
        // 无警示 → 可以被"本次会话都允许"吞掉
        assert!(!ask_always_repeated(false));
        // 2026-09-18 沐拍板：覆盖已有文件已摘出"每次必问"名单，与普通编辑同权
        // （WriteFile/EditFile 同走补丁暂存 + 快照回滚兜底，见 file_tools/workspace.rs）。
    }

    #[test]
    fn new_file_is_asked_in_request_mode_but_silent_in_auto_mode() {
        let input = allow_input();
        assert_eq!(
            judge("WriteFile", ApprovalMode::RequestApproval, &input).outcome,
            Outcome::Ask
        );
        assert_eq!(
            judge("WriteFile", ApprovalMode::AutoApprove, &input).outcome,
            Outcome::Allow
        );
    }

    #[test]
    fn overwriting_existing_file_same_as_edit() {
        // 2026-09-18 沐拍板：覆盖与编辑完全同权（回滚系统兜底）——覆盖不再触发
        // "每次必问"，走普通类别判定：请求审批档照常问，帮我批准档放行。
        let input = JudgementInput {
            target_exists: Some(true),
            ..allow_input()
        };
        assert_eq!(
            judge("WriteFile", ApprovalMode::RequestApproval, &input).outcome,
            Outcome::Ask
        );
        assert_eq!(
            judge("WriteFile", ApprovalMode::AutoApprove, &input).outcome,
            Outcome::Allow
        );
    }

    #[test]
    fn batch_by_single_call_asks() {
        let input = JudgementInput {
            files_in_call: 5,
            ..allow_input()
        };
        let decision = judge("ApplyPatch", ApprovalMode::AutoApprove, &input);
        assert_eq!(decision.outcome, Outcome::Ask);
        assert!(decision.reason.contains("批量"), "{}", decision.reason);
    }

    #[test]
    fn batch_by_third_file_in_turn_asks() {
        let input = JudgementInput {
            files_in_turn: 3,
            ..allow_input()
        };
        let decision = judge("EditFile", ApprovalMode::AutoApprove, &input);
        assert_eq!(decision.outcome, Outcome::Ask);
        assert!(
            decision.reason.contains("第 3 个文件"),
            "{}",
            decision.reason
        );

        let second = JudgementInput {
            files_in_turn: 2,
            ..allow_input()
        };
        assert_eq!(
            judge("EditFile", ApprovalMode::AutoApprove, &second).outcome,
            Outcome::Allow
        );
    }

    #[test]
    fn outside_workspace_is_denied_in_all_modes() {
        let input = JudgementInput {
            out_of_workspace: vec!["C:/Windows/System32".to_string()],
            ..allow_input()
        };
        for tool in ["ReadFile", "WriteFile", "DeleteFile", "RunCommand"] {
            for mode in all_modes() {
                let decision = judge(tool, mode, &input);
                assert_eq!(decision.outcome, Outcome::Deny, "tool={}", tool);
            }
        }
    }

    #[test]
    fn existing_runtime_deny_wins() {
        let input = JudgementInput {
            existing_deny_reason: Some("检测到危险 PowerShell 命令".to_string()),
            ..allow_input()
        };
        let decision = judge("RunCommand", ApprovalMode::AutoApprove, &input);
        assert_eq!(decision.outcome, Outcome::Deny);
        assert_eq!(decision.reason, "检测到危险 PowerShell 命令");
    }

    #[test]
    fn unclassified_tool_is_asked_and_flagged() {
        let decision = judge("SomethingNew", ApprovalMode::AutoApprove, &allow_input());
        assert_eq!(decision.outcome, Outcome::Ask);
        assert!(decision.class.is_none());
        assert!(decision.reason.contains("未登记分类"));
    }

    #[test]
    fn orchestration_and_session_tools_are_allowed() {
        for tool in [
            "CreateTask",
            "RunSubagent",
            "UpdateTodos",
            "CompactConversation",
            "UpdateMemory",
            "SwitchWorkMode",
        ] {
            assert_eq!(
                judge(tool, ApprovalMode::AutoApprove, &allow_input()).outcome,
                Outcome::Allow,
                "tool={}",
                tool
            );
        }
    }

    #[test]
    fn readonly_command_is_allowed_not_asked() {
        let input = JudgementInput {
            command_is_readonly: true,
            ..allow_input()
        };
        for mode in [ApprovalMode::RequestApproval, ApprovalMode::AutoApprove] {
            let decision = judge("RunCommand", mode, &input);
            assert_eq!(decision.outcome, Outcome::Allow, "mode={:?}", mode);
            assert_eq!(decision.reason, "只读命令（免询问）");
        }
    }

    #[test]
    fn ask_reason_is_not_polluted_by_existing_warning() {
        // 现有守卫的警告只作为补充提示，不能混进"为什么要问你"
        let input = JudgementInput {
            existing_warning: Some(
                "命令包含 .NET 静态方法调用 [Type]::Method()。请确认调用安全。".to_string(),
            ),
            ..allow_input()
        };
        let decision = judge("RunCommand", ApprovalMode::AutoApprove, &input);
        assert_eq!(decision.outcome, Outcome::Ask);
        assert_eq!(decision.reason, "跑命令");
    }

    #[test]
    fn patch_files_are_counted() {
        let patch = "*** Begin Patch\n*** Update File: src/a.ts\n@@\n*** Add File: src/b.ts\n+hello\n*** Delete File: src/old.ts\n*** End Patch\n";
        let files = files_in_patch(patch);
        assert_eq!(files.len(), 3, "{:?}", files);
        assert!(files.contains(&"src/a.ts".to_string()));
    }

    #[test]
    fn unified_diff_files_are_counted() {
        let patch = "--- a/src/x.rs\n+++ b/src/x.rs\n@@\n--- a/src/y.rs\n+++ b/src/y.rs\n@@\n";
        let files = files_in_patch(patch);
        assert_eq!(files, vec!["src/x.rs".to_string(), "src/y.rs".to_string()]);
    }
}
