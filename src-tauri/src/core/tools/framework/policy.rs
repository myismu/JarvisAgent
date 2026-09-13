//! # policy.rs — 工具权限策略表 + 判定器
//!
//! 这是"执行前权限判定"的唯一一张策略表，回答一个问题：
//! **这次工具调用，应该直接放行、先问用户一句，还是直接拒绝。**
//!
//! ## Key Exports
//! - `ApprovalMode`：权限档位（请求审批 / 帮我批准）
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

/// 用户的权限档位
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ApprovalMode {
    /// 请求审批：改动类操作一律先问用户
    RequestApproval,
    /// 帮我批准：只有风险操作才问
    AutoApprove,
}

impl ApprovalMode {
    pub fn as_str(&self) -> &'static str {
        match self {
            ApprovalMode::RequestApproval => "request_approval",
            ApprovalMode::AutoApprove => "auto_approve",
        }
    }

    /// 中文名（写进日志/汇总，便于人看）
    pub fn label(&self) -> &'static str {
        match self {
            ApprovalMode::RequestApproval => "请求审批",
            ApprovalMode::AutoApprove => "帮我批准",
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
    /// 改工作目录
    WorkspaceChange,
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
            ToolClass::WorkspaceChange => "workspace_change",
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
            ToolClass::WorkspaceChange => "改工作目录",
            ToolClass::Orchestrate => "任务编排",
            ToolClass::SessionMgmt => "会话与记忆",
            ToolClass::AppControl => "应用控制",
        }
    }

    /// 会不会改变用户项目里的东西（用于统计"这一轮改了几个文件"）
    pub fn mutates_project(&self) -> bool {
        matches!(
            self,
            ToolClass::CreateFile | ToolClass::ModifyContent | ToolClass::Delete | ToolClass::Rename
        )
    }

    /// 是否属于"默认就要问"的类别（两个档位都问）
    pub fn always_asks(&self) -> bool {
        matches!(
            self,
            ToolClass::Delete
                | ToolClass::Rename
                | ToolClass::RunCommand
                | ToolClass::Background
                | ToolClass::WorkspaceChange
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
    ("ReadFile", ToolPolicy { class: READ_ONLY, path_fields: &["path"], command_field: None, patch_field: None }),
    ("ReadFileSkeleton", ToolPolicy { class: READ_ONLY, path_fields: &["path"], command_field: None, patch_field: None }),
    ("ReadSymbol", ToolPolicy { class: READ_ONLY, path_fields: &["path"], command_field: None, patch_field: None }),
    ("FindSymbol", ToolPolicy { class: READ_ONLY, path_fields: &["dir"], command_field: None, patch_field: None }),
    ("FindReferences", ToolPolicy { class: READ_ONLY, path_fields: &["dir"], command_field: None, patch_field: None }),
    ("CodeSearch", ToolPolicy { class: READ_ONLY, path_fields: &["dir"], command_field: None, patch_field: None }),
    ("SearchRepo", ToolPolicy { class: READ_ONLY, path_fields: &["dir"], command_field: None, patch_field: None }),
    ("SearchText", ToolPolicy { class: READ_ONLY, path_fields: &["path", "dir"], command_field: None, patch_field: None }),
    ("FindFiles", ToolPolicy { class: READ_ONLY, path_fields: &["dir"], command_field: None, patch_field: None }),
    ("ListDirectory", ToolPolicy { class: READ_ONLY, path_fields: &["path"], command_field: None, patch_field: None }),
    ("RunGitCommand", ToolPolicy { class: READ_ONLY, path_fields: &[], command_field: None, patch_field: None }),
    ("CheckBackgroundCommand", ToolPolicy { class: READ_ONLY, path_fields: &[], command_field: None, patch_field: None }),
    ("ListTasks", ToolPolicy { class: READ_ONLY, path_fields: &[], command_field: None, patch_field: None }),
    ("GetTask", ToolPolicy { class: READ_ONLY, path_fields: &[], command_field: None, patch_field: None }),
    ("SummarizeTasks", ToolPolicy { class: READ_ONLY, path_fields: &[], command_field: None, patch_field: None }),
    ("ReadMemory", ToolPolicy { class: READ_ONLY, path_fields: &[], command_field: None, patch_field: None }),
    ("GetToolCatalog", ToolPolicy { class: READ_ONLY, path_fields: &[], command_field: None, patch_field: None }),
    ("DiscoverTools", ToolPolicy { class: READ_ONLY, path_fields: &[], command_field: None, patch_field: None }),
    ("LoadSkill", ToolPolicy { class: READ_ONLY, path_fields: &[], command_field: None, patch_field: None }),

    // ── 改文件 ──
    ("WriteFile", ToolPolicy { class: ToolClass::CreateFile, path_fields: &["path"], command_field: None, patch_field: None }),
    ("EditFile", ToolPolicy { class: ToolClass::ModifyContent, path_fields: &["path"], command_field: None, patch_field: None }),
    ("ApplyPatch", ToolPolicy { class: ToolClass::ModifyContent, path_fields: &[], command_field: None, patch_field: Some("patch") }),
    ("EditNotebook", ToolPolicy { class: ToolClass::ModifyContent, path_fields: &["notebook_path"], command_field: None, patch_field: None }),
    ("DeleteFile", ToolPolicy { class: ToolClass::Delete, path_fields: &["path"], command_field: None, patch_field: None }),
    ("RenameFile", ToolPolicy { class: ToolClass::Rename, path_fields: &["path", "new_path"], command_field: None, patch_field: None }),

    // ── 命令与工作区 ──
    ("RunCommand", ToolPolicy { class: ToolClass::RunCommand, path_fields: &[], command_field: Some("command"), patch_field: None }),
    ("StartBackgroundCommand", ToolPolicy { class: ToolClass::Background, path_fields: &["dir"], command_field: Some("command"), patch_field: None }),
    ("SetWorkspace", ToolPolicy { class: ToolClass::WorkspaceChange, path_fields: &["path"], command_field: None, patch_field: None }),

    // ── 编排 / 会话 / 应用 ──
    ("CreateTask", ToolPolicy { class: ToolClass::Orchestrate, path_fields: &[], command_field: None, patch_field: None }),
    ("UpdateTask", ToolPolicy { class: ToolClass::Orchestrate, path_fields: &[], command_field: None, patch_field: None }),
    ("DeleteTask", ToolPolicy { class: ToolClass::Orchestrate, path_fields: &[], command_field: None, patch_field: None }),
    ("UpdateTodos", ToolPolicy { class: ToolClass::Orchestrate, path_fields: &[], command_field: None, patch_field: None }),
    ("RunSubagent", ToolPolicy { class: ToolClass::Orchestrate, path_fields: &[], command_field: None, patch_field: None }),
    ("RunSubagentsSequentially", ToolPolicy { class: ToolClass::Orchestrate, path_fields: &[], command_field: None, patch_field: None }),
    ("ProposePlan", ToolPolicy { class: ToolClass::Orchestrate, path_fields: &[], command_field: None, patch_field: None }),
    ("CompactConversation", ToolPolicy { class: ToolClass::SessionMgmt, path_fields: &[], command_field: None, patch_field: None }),
    ("ConsolidateMemory", ToolPolicy { class: ToolClass::SessionMgmt, path_fields: &[], command_field: None, patch_field: None }),
    ("UpdateMemory", ToolPolicy { class: ToolClass::SessionMgmt, path_fields: &[], command_field: None, patch_field: None }),
    ("SwitchWorkMode", ToolPolicy { class: ToolClass::AppControl, path_fields: &[], command_field: None, patch_field: None }),
    ("ExecuteTool", ToolPolicy { class: ToolClass::AppControl, path_fields: &[], command_field: None, patch_field: None }),
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
    /// 命令是不是"只读命令"（今天这类命令本来就免弹窗，新规则也应当直接放行）
    pub command_is_readonly: bool,
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
/// 4. 默认就要问的类别（删/改名/跑命令/后台/改工作目录）→ 问
/// 5. 覆盖已有文件 → 问
/// 6. 批量（本次调用 ≥3 个文件，或本轮第 3 个文件）→ 问
/// 7. 其余按档位：请求审批档问，帮我批准档放行
pub fn judge(tool: &str, mode: ApprovalMode, input: &JudgementInput) -> ShadowDecision {
    let policy = policy_for(tool);
    let class = policy.map(|p| p.class);

    // 1. 现有运行时检查（命令安全检查等）已经要拒绝
    if let Some(reason) = &input.existing_deny_reason {
        return ShadowDecision {
            class,
            outcome: Outcome::Deny,
            reason: reason.clone(),
        };
    }

    // 2. 项目外路径
    if !input.out_of_workspace.is_empty() {
        return ShadowDecision {
            class,
            outcome: Outcome::Deny,
            reason: format!("路径在项目外：{}", input.out_of_workspace.join(", ")),
        };
    }

    // 3. 未登记分类
    let Some(policy) = policy else {
        return ShadowDecision {
            class: None,
            outcome: Outcome::Ask,
            reason: "工具未登记分类（需要补策略表）".to_string(),
        };
    };

    // 4. 默认就要问的类别
    if policy.class.always_asks() {
        // 只读命令（Get-ChildItem 这类）本来就免弹窗，新规则同样直接放行，
        // 否则观察数据会把"免询问的只读命令"算成打扰次数
        if input.command_is_readonly {
            return ShadowDecision {
                class,
                outcome: Outcome::Allow,
                reason: "只读命令（免询问）".to_string(),
            };
        }
        // 原因只说"为什么要问你"（例如"跑命令"）。
        // 现有守卫的警告文案是**补充提示**，由调用方单独记录并在报告里单独成段，
        // 不能拼进原因里——那会把"问你的理由"和"附加说明"混成一团（首次实测踩到过）。
        return ShadowDecision {
            class,
            outcome: Outcome::Ask,
            reason: policy.class.label().to_string(),
        };
    }

    // 5. 覆盖已有文件
    if matches!(policy.class, ToolClass::CreateFile) && input.target_exists == Some(true) {
        return ShadowDecision {
            class,
            outcome: Outcome::Ask,
            reason: "覆盖已有文件".to_string(),
        };
    }

    // 6. 批量
    if policy.class.mutates_project() {
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
    match (policy.class, mode) {
        (ToolClass::CreateFile | ToolClass::ModifyContent, ApprovalMode::AutoApprove) => {
            ShadowDecision {
                class,
                outcome: Outcome::Allow,
                reason: format!("{}（帮我批准档自动放行）", policy.class.label()),
            }
        }
        (ToolClass::CreateFile | ToolClass::ModifyContent, ApprovalMode::RequestApproval) => {
            ShadowDecision {
                class,
                outcome: Outcome::Ask,
                reason: format!("{}（请求审批档）", policy.class.label()),
            }
        }
        _ => ShadowDecision {
            class,
            outcome: Outcome::Allow,
            reason: policy.class.label().to_string(),
        },
    }
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
    fn read_only_work_is_allowed_in_both_modes() {
        for mode in [ApprovalMode::RequestApproval, ApprovalMode::AutoApprove] {
            let decision = judge("ReadFile", mode, &allow_input());
            assert_eq!(decision.outcome, Outcome::Allow, "mode={:?}", mode);
        }
    }

    #[test]
    fn delete_asks_in_both_modes() {
        for mode in [ApprovalMode::RequestApproval, ApprovalMode::AutoApprove] {
            let decision = judge("DeleteFile", mode, &allow_input());
            assert_eq!(decision.outcome, Outcome::Ask, "mode={:?}", mode);
            assert_eq!(decision.reason, "删除文件");
        }
    }

    #[test]
    fn command_asks_in_both_modes() {
        for mode in [ApprovalMode::RequestApproval, ApprovalMode::AutoApprove] {
            assert_eq!(judge("RunCommand", mode, &allow_input()).outcome, Outcome::Ask);
        }
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
    fn overwriting_existing_file_always_asks() {
        let input = JudgementInput {
            target_exists: Some(true),
            ..allow_input()
        };
        for mode in [ApprovalMode::RequestApproval, ApprovalMode::AutoApprove] {
            let decision = judge("WriteFile", mode, &input);
            assert_eq!(decision.outcome, Outcome::Ask);
            assert_eq!(decision.reason, "覆盖已有文件");
        }
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
        assert!(decision.reason.contains("第 3 个文件"), "{}", decision.reason);

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
    fn outside_workspace_is_denied_in_both_modes() {
        let input = JudgementInput {
            out_of_workspace: vec!["C:/Windows/System32".to_string()],
            ..allow_input()
        };
        for tool in ["ReadFile", "WriteFile", "DeleteFile", "RunCommand"] {
            for mode in [ApprovalMode::RequestApproval, ApprovalMode::AutoApprove] {
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
    fn workspace_change_always_asks() {
        for mode in [ApprovalMode::RequestApproval, ApprovalMode::AutoApprove] {
            assert_eq!(
                judge("SetWorkspace", mode, &allow_input()).outcome,
                Outcome::Ask
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
            existing_warning: Some("命令包含 .NET 静态方法调用 [Type]::Method()。请确认调用安全。".to_string()),
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
