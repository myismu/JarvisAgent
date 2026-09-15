//! # policy_guard.rs — 执行前权限判定（真正生效的那一层）
//!
//! 第一步（`policy_shadow`）只记录"按新规则会怎么判"；这里负责**真的判、真的问、真的拦**。
//!
//! ## 流程
//!
//! ```text
//! 工具调用
//!   ↓
//! ① 采集事实（用哪个工具、目标路径、文件是否已存在、影响几个文件、现有守卫怎么说）
//!   ↓
//! ② 查"本会话已允许"（工具 + 范围）→ 命中 → 直接放行
//!   ↓ 未命中
//! ③ 判定器给结论
//!    ├─ 放行 → 执行
//!    ├─ 要问 → 弹窗；允许则执行（会话级允许会记进 ②）
//!    └─ 拒绝 → 不执行，把原因回灌给模型
//! ```
//!
//! ## Key Exports
//! - `enforce()`：执行前判定入口（返回 Some 表示不要执行，把文本回灌给模型）
//! - `prepare_facts()`：采集判定事实（工具/路径/覆盖/影响文件数/现有守卫结论）
//! - `PreparedFacts`：判定事实
//! - `PermissionTurnState`：本轮状态（本轮改过哪些文件，给批量规则用）
//!
//! ## 约束
//!
//! - 事实采集与观察层共用同一份实现（[`prepare_facts`]），避免"预判口径"和"真实口径"漂移
//! - 判定器是纯函数（见 `policy::judge`）；本模块只负责 IO 与弹窗
//! - 无工作区（没绑项目）时不判定越界——与现有行为一致

use super::permission::{PermissionDecision, PermissionKind};
use super::policy::{self, ApprovalMode, JudgementInput, Outcome, ToolClass};
use crate::infra::state::state::{SessionAllowance, SessionContext};
use std::collections::BTreeSet;
use serde_json::Value;
use tauri::{Emitter, Manager};

/// 会话内的"本轮"状态：只保留判定真正需要跨调用保持的东西。
///
/// 用途：批量规则——同一轮（一次用户请求）里改到第 3 个文件时要停下问用户。
#[derive(Debug, Default)]
pub struct PermissionTurnState {
    /// 当前轮的标识（用 run_id）；换了就重置
    pub turn_id: String,
    /// 本轮已经改动过的文件
    pub files: BTreeSet<String>,
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn existing_guard_blocks_dangerous_command() {
        let verdict = inspect_existing_guards(
            "RunCommand",
            &json!({ "command": "Invoke-Expression 'evil'" }),
            None,
        );
        assert!(
            verdict.deny_reason.is_some(),
            "危险命令应被现有检查判为拒绝"
        );
    }

    #[test]
    fn existing_guard_marks_readonly_command() {
        let verdict =
            inspect_existing_guards("RunCommand", &json!({ "command": "Get-ChildItem" }), None);
        assert!(verdict.deny_reason.is_none());
        assert!(verdict.is_readonly, "Get-ChildItem 应被识别为只读命令");
    }

    #[test]
    fn non_command_tools_have_no_guard_verdict() {
        let verdict = inspect_existing_guards("WriteFile", &json!({ "path": "a.txt" }), None);
        assert!(verdict.deny_reason.is_none() && verdict.warning.is_none());
        assert!(!verdict.is_readonly);
    }

    #[test]
    fn allowance_key_groups_edits_under_one_kind() {
        let key = allowance_key_for(
            Some(ToolClass::ModifyContent),
            &json!({ "path": "src/a.ts" }),
            &["src/a.ts".to_string()],
            Some(std::path::Path::new("E:/proj")),
        )
        .expect("编辑类应有会话允许键");
        assert_eq!(key.kind, ALLOWANCE_KIND_EDIT, "编辑类应归到同一档");
        assert!(
            key.scope.contains("proj"),
            "范围应是绝对目录：{}",
            key.scope
        );
        assert!(key.label.contains("改动项目文件"));

        // 新建走同一档：agent 先写文件、再改文件，不该让用户点两次
        let same_kind = allowance_key_for(
            Some(ToolClass::CreateFile),
            &json!({ "path": "src/b.ts" }),
            &["src/b.ts".to_string()],
            Some(std::path::Path::new("E:/proj")),
        )
        .expect("新建类应有会话允许键");
        assert_eq!(same_kind.kind, ALLOWANCE_KIND_EDIT);
        assert_eq!(same_kind.scope, key.scope, "同目录下范围键应相同");
    }

    #[test]
    fn delete_is_its_own_kind() {
        let key = allowance_key_for(
            Some(ToolClass::Delete),
            &json!({ "path": "src/a.ts" }),
            &["src/a.ts".to_string()],
            None,
        )
        .expect("删除类应有会话允许键");
        assert_eq!(key.kind, ALLOWANCE_KIND_DELETE, "删除必须与编辑分开");
        assert!(key.label.contains("删除文件"));
    }

    #[test]
    fn classes_without_session_allowance_have_no_key() {
        // 改工作目录是一次性动作且影响面大；未登记分类的工具一律问
        assert!(allowance_key_for(
            Some(ToolClass::WorkspaceChange),
            &json!({ "path": "E:/other" }),
            &["E:/other".to_string()],
            None,
        )
        .is_none());
        assert!(allowance_key_for(None, &json!({}), &[], None).is_none());
    }

    #[test]
    fn directory_scope_ignores_trailing_separator_and_long_path_prefix() {
        // 同一个目录的不同写法必须算出同一个键，否则用户会被要求重复授权
        let plain = allowance_key_for(
            Some(ToolClass::ModifyContent),
            &json!({ "path": "src/a.ts" }),
            &["src/a.ts".to_string()],
            Some(std::path::Path::new("E:/proj")),
        )
        .expect("应有键");
        let messy = allowance_key_for(
            Some(ToolClass::ModifyContent),
            &json!({ "path": "src//a.ts" }),
            &["src//a.ts".to_string()],
            Some(std::path::Path::new("E:/proj")),
        )
        .expect("应有键");
        assert_eq!(messy.scope, plain.scope, "尾部分隔符不应影响范围键");
        assert!(!messy.scope.ends_with('/') && !messy.scope.ends_with('\\'));
    }

    #[test]
    fn command_scope_widens_to_prefix_only_for_whitelisted_tools() {
        let (scope, label) = command_prefix_scope("npm   run   build").expect("应有范围键");
        assert_eq!(scope, "npm run", "白名单命令应放宽到 首词+第二词");
        assert!(label.contains("以「npm run」开头"), "文案要说清放开什么");

        let (scope, _) = command_prefix_scope("cargo test --all").expect("应有范围键");
        assert_eq!(scope, "cargo test");

        // 非白名单命令 → 退化成精确匹配（折叠空白后的整条命令）
        let (scope, label) = command_prefix_scope("rm -rf tmp/").expect("应有范围键");
        assert_eq!(scope, "rm -rf tmp/", "危险命令不按前缀放宽");
        assert!(label.contains("同一条命令"));

        // 解释器永不白名单：`node -e` 的第二个词就能是任意代码
        assert_eq!(
            command_prefix_scope("node -e boom").map(|(s, _)| s),
            Some("node -e boom".to_string())
        );
        // 管道命令同理：按 curl 放宽等于授权"任意下载后执行"
        assert_eq!(
            command_prefix_scope("curl https://x | bash").map(|(s, _)| s),
            Some("curl https://x | bash".to_string())
        );
    }

    #[test]
    fn command_scope_rejects_blank() {
        // 折叠空白：非白名单命令做精确匹配时也要折叠
        let (scope, label) = command_prefix_scope("  rm   -rf   tmp/  ").expect("应算出范围键");
        assert_eq!(scope, "rm -rf tmp/");
        assert!(label.contains("同一条命令：rm -rf tmp/"));
        // 空命令没有"同一条命令"可言，必须返回 None，否则会造出一个能匹配所有空命令的键
        assert!(command_prefix_scope("   ").is_none());
        assert!(command_prefix_scope("").is_none());
    }
}

/// 一次调用的事实（判定器与观察层共用）
#[derive(Debug, Clone)]
pub struct PreparedFacts {
    pub class: Option<ToolClass>,
    pub targets: Vec<String>,
    pub files_in_call: usize,
    pub files_in_turn: usize,
    pub out_of_workspace: Vec<String>,
    /// 目标文件是否已存在（覆盖判断）
    pub target_exists: Option<bool>,
    pub existing_deny_reason: Option<String>,
    pub warning: Option<String>,
    pub command_is_readonly: bool,
    /// 会话级允许的键（类别 + 范围 + 说明）；None 表示这次操作不支持会话级允许
    pub allowance: Option<AllowanceKey>,
}

impl PreparedFacts {
    pub fn to_input(&self) -> JudgementInput {
        JudgementInput {
            targets: self.targets.clone(),
            out_of_workspace: self.out_of_workspace.clone(),
            files_in_call: self.files_in_call,
            files_in_turn: self.files_in_turn,
            target_exists: self.target_exists,
            existing_deny_reason: self.existing_deny_reason.clone(),
            existing_warning: self.warning.clone(),
            command_is_readonly: self.command_is_readonly,
        }
    }
}

/// 采集判定事实（含更新"本轮改过哪些文件"）。观察层与执行前判定共用此函数。
pub async fn prepare_facts(
    ctx: &SessionContext,
    tool: &str,
    input: &Value,
) -> PreparedFacts {
    let policy = policy::policy_for(tool);
    let mut targets: Vec<String> = Vec::new();
    if let Some(policy) = policy {
        for field in policy.path_fields {
            if let Some(path) = input[*field].as_str() {
                targets.push(path.to_string());
            }
        }
        if let Some(patch_text) = policy.patch_field.and_then(|f| input[f].as_str()) {
            targets.extend(policy::files_in_patch(patch_text));
        }
    }
    targets.dedup();

    let workspace = ctx.workspace.lock().await.clone();
    let out_of_workspace: Vec<String> = targets
        .iter()
        .filter(|path| {
            workspace
                .as_deref()
                .map(|ws| !super::permission::is_within_workspace(path, Some(ws)))
                .unwrap_or(false)
        })
        .cloned()
        .collect();

    let class = policy.map(|p| p.class);
    let files_in_call = if targets.is_empty() { 0 } else { targets.len() };

    let target_exists = match class {
        Some(ToolClass::CreateFile) | Some(ToolClass::ModifyContent) => match targets.first() {
            Some(path) => {
                let resolved = resolve_target(path, workspace.as_deref());
                Some(tokio::fs::metadata(&resolved).await.is_ok())
            }
            None => None,
        },
        _ => None,
    };

    let command_info = inspect_existing_guards(tool, input, workspace.as_deref());

    // 本轮计数（换一轮就重置）
    let turn_id = ctx
        .active_run_id
        .lock()
        .await
        .clone()
        .unwrap_or_else(|| "unknown_turn".to_string());
    let files_in_turn = {
        let mut state = ctx.permission_turn.lock().await;
        if state.turn_id != turn_id {
            state.turn_id = turn_id;
            state.files.clear();
        }
        if class.map(|c| c.mutates_project()).unwrap_or(false) {
            for target in &targets {
                state.files.insert(target.clone());
            }
        }
        state.files.len()
    };

    let allowance = allowance_key_for(class, input, &targets, workspace.as_deref());

    PreparedFacts {
        class,
        targets,
        files_in_call,
        files_in_turn,
        out_of_workspace,
        target_exists,
        existing_deny_reason: command_info.deny_reason,
        warning: command_info.warning,
        command_is_readonly: command_info.is_readonly,
        allowance,
    }
}

fn resolve_target(path: &str, workspace: Option<&std::path::Path>) -> std::path::PathBuf {
    let p = std::path::Path::new(path);
    match (p.is_absolute(), workspace) {
        (true, _) => p.to_path_buf(),
        (false, Some(ws)) => ws.join(p),
        (false, None) => std::env::current_dir().unwrap_or_default().join(p),
    }
}

struct ExistingGuardVerdict {
    deny_reason: Option<String>,
    warning: Option<String>,
    is_readonly: bool,
}

fn inspect_existing_guards(
    tool: &str,
    input: &Value,
    workspace: Option<&std::path::Path>,
) -> ExistingGuardVerdict {
    use crate::core::tools::shell_tools::{execution, readonly, security, types::SafetyResult, utils};

    let command = input["command"].as_str().unwrap_or("").to_string();
    if command.trim().is_empty() {
        return ExistingGuardVerdict {
            deny_reason: None,
            warning: None,
            is_readonly: false,
        };
    }
    let mut warning = match security::check_command_safety(&command) {
        SafetyResult::Block(msg) => {
            return ExistingGuardVerdict {
                deny_reason: Some(format!("现有命令安全检查会拦下：{}", msg)),
                warning: None,
                is_readonly: false,
            };
        }
        SafetyResult::Warn(msg) => Some(msg),
        SafetyResult::Safe => None,
    };
    let is_readonly = readonly::is_readonly_command(&command);
    if let Some(ws) = workspace {
        if let Err(err) = utils::check_command_paths(&command, ws) {
            return ExistingGuardVerdict {
                deny_reason: Some(format!("命令里的路径越界：{}", err)),
                warning,
                is_readonly,
            };
        }
    }
    if let Some(hint) = execution::is_file_mutation_command(&command) {
        return ExistingGuardVerdict {
            deny_reason: Some(format!("shell 改文件会被拦：{}", hint)),
            warning,
            is_readonly,
        };
    }
    if warning.is_none() {
        warning = security::get_destructive_warning(&command);
    }
    let _ = tool;
    ExistingGuardVerdict {
        deny_reason: None,
        warning,
        is_readonly,
    }
}

/// 会话级允许的键：`(操作类别, 范围)` + 给用户看的一句话说明。
///
/// 只有拿得到这个键的操作，"本次会话都允许"按钮才有意义。
#[derive(Debug, Clone)]
pub struct AllowanceKey {
    /// 操作类别（见 [`allowance_kind`]）
    pub kind: &'static str,
    /// 范围键（文件类 = 目标目录绝对路径；命令类 = 命令前缀）
    pub scope: String,
    /// 展示给用户看的一句话，必须说清"点下去到底放开了什么"
    pub label: String,
}

/// 类别常量。刻意只留 3 个（见 [`allowance_kind`] 的说明）。
pub const ALLOWANCE_KIND_EDIT: &str = "edit_project";
pub const ALLOWANCE_KIND_DELETE: &str = "delete";
pub const ALLOWANCE_KIND_COMMAND: &str = "run_command";

/// 工具归类 → 会话级允许的类别；不提供会话级允许的类别返回 `None`。
///
/// - `edit_project`：新建 / 改内容 / 改名。合并成一档是刻意的——分开成"新建类""编辑类"之后，
///   agent 先 WriteFile 建文件、再 EditFile 改内容，用户还得点两次，那还不如每次点"允许"。
/// - `delete`：删除单独一档。它会把文件从工作区里挪走——`DeleteFile` 实际是**软删除**
///   （先把内容存进回滚快照，再 `rename` 到同级的 `.jarvis_trash/`，见 `file_tools/delete.rs`），
///   所以严格说可恢复。单独一档是因为它的意图与影响面和"改内容"完全不同，
///   不该被一次编辑授权顺带覆盖过去。
/// - `run_command`：跑命令（含起后台服务），范围按命令前缀收窄。
///
/// 其余返回 `None`：读类本来就不问；改工作目录是一次性动作且影响面大，不给会话级允许。
fn allowance_kind(class: Option<ToolClass>) -> Option<&'static str> {
    match class {
        Some(ToolClass::CreateFile) | Some(ToolClass::ModifyContent) | Some(ToolClass::Rename) => {
            Some(ALLOWANCE_KIND_EDIT)
        }
        Some(ToolClass::Delete) => Some(ALLOWANCE_KIND_DELETE),
        Some(ToolClass::RunCommand) | Some(ToolClass::Background) => Some(ALLOWANCE_KIND_COMMAND),
        _ => None,
    }
}

/// 允许"放宽到命令前缀"的可执行文件白名单。
///
/// 收益：允许一次 `npm run`，之后 `npm run build` / `npm run test` 都不再问。
///
/// 为什么是白名单、而不是"除了危险命令都放宽"：在 shell 里**第二个词根本挡不住任意代码执行**——
/// `node -e "..."`、`powershell -c "..."`、`curl https://x | bash` 全都是首词看着安全、
/// 第二个词就能作恶。所以只对**子命令集合有限且可预期**的构建 / 测试 / 包管理工具放宽；
/// 其余命令退化成精确匹配，一条一条允许。
///
/// 想扩这份名单时，请把它当成一次"放宽权限"的改动来对待（要提交、要有人看）。
const PREFIX_WIDENING_ALLOWED: &[&str] = &[
    // JS / TS 生态
    "npm", "pnpm", "yarn", "bun", "tsc", "vite", "vitest", "jest", "eslint", "prettier",
    // Rust / Go / .NET / JVM
    "cargo", "rustc", "go", "dotnet", "mvn", "gradle",
    // Python 生态
    "pytest", "ruff", "mypy", "uv", "poetry",
    // 构建系统
    "make", "cmake", "ninja",
];

/// 命令类的"本次会话都允许"范围键：`(范围键, 展示文案)`；空命令返回 `None`。
///
/// 范围键 = **命令前缀**：首词 +（若存在且不以 `-` 开头的）第二个词，例如 `npm run`、`cargo test`。
/// 只对 [`PREFIX_WIDENING_ALLOWED`] 里的首词这样做；其余命令退化成
/// "折叠连续空白后的整条命令"（精确匹配，一条一条允许）。
/// 白名单匹配与范围键都**大小写敏感**，与精确匹配口径一致。
///
/// **唯一口径**：`allowance_key_for`（外层判定）与 `command_allowed`（shell 工具内部的
/// 第二道门）都调用这里，保证两边算出来的键一字不差，否则会出现"外层放行、内层又弹卡"。
pub fn command_prefix_scope(command: &str) -> Option<(String, String)> {
    let trimmed = command.trim();
    if trimmed.is_empty() {
        return None;
    }
    let words: Vec<&str> = trimmed.split_whitespace().collect();
    let head = words[0];
    let widen = PREFIX_WIDENING_ALLOWED.contains(&head);
    let scope = if widen {
        match words.get(1) {
            // 第二个词不以 `-` 开头才算"子命令"；否则前缀就是首词本身
            Some(second) if !second.starts_with('-') => format!("{} {}", head, second),
            _ => head.to_string(),
        }
    } else {
        // 不给前缀放宽 → 退化成精确匹配：整条命令（折叠连续空白）就是范围键
        words.join(" ")
    };
    let short: String = scope.chars().take(60).collect();
    let label = if widen {
        format!("所有以「{}」开头的命令", short)
    } else {
        format!("同一条命令：{}", short)
    };
    Some((scope, label))
}

/// 会话级允许的类别与范围。
///
/// 文件类：类别 = `edit_project` / `delete`，范围 = 目标所在目录（绝对路径）
/// 命令类：类别 = `run_command`，范围 = 命令前缀（见 [`command_prefix_scope`]）
/// 其它类别（读、编排、改工作目录等）：`None`，即不提供"本次会话都允许"
fn allowance_key_for(
    class: Option<ToolClass>,
    input: &Value,
    targets: &[String],
    workspace: Option<&std::path::Path>,
) -> Option<AllowanceKey> {
    let kind = allowance_kind(class)?;

    if kind == ALLOWANCE_KIND_COMMAND {
        let (scope, label) = command_prefix_scope(input["command"].as_str().unwrap_or(""))?;
        return Some(AllowanceKey { kind, scope, label });
    }

    let first = targets.first()?;
    let dir = std::path::Path::new(first)
        .parent()
        .map(|p| p.to_path_buf())
        .unwrap_or_default();
    let resolved = if dir.is_absolute() {
        dir
    } else {
        workspace.map(|ws| ws.join(&dir)).unwrap_or(dir)
    };
    let display = normalize_dir_key(&resolved);
    let label = if kind == ALLOWANCE_KIND_DELETE {
        format!("在 {} 里删除文件", display)
    } else {
        format!("在 {} 里改动项目文件（新建 / 编辑 / 改名都算）", display)
    };
    Some(AllowanceKey {
        kind,
        scope: display,
        label,
    })
}

/// 目录范围键的规范化：剥掉 Windows 长路径前缀与**结尾的分隔符**。
///
/// 为什么需要：范围键是**字符串精确比较**。工具参数里写成 `src/a.ts` 与 `src//a.ts`
/// 会算出带不带尾斜杠的两种目录串，于是"同一个目录"被当成两个范围，用户要重复授权。
fn normalize_dir_key(path: &std::path::Path) -> String {
    let text = path.to_string_lossy().trim_start_matches(r"\\?\").to_string();
    let trimmed = text.trim_end_matches(['\\', '/']);
    // 别把 `E:\` 修成 `E:`（后者是"当前目录"的意思，完全不同的路径）
    if trimmed.is_empty() || trimmed.ends_with(':') {
        text
    } else {
        trimmed.to_string()
    }
}

/// 用户点了"本次会话都允许"：登记这个"类别 + 范围"，并把**已经挂起**的同类请求一次性放行。
///
/// 为什么必须消化积压：工具是**并行**执行的（`tools_runner` 对一批工具调用逐个
/// `tokio::spawn` 后 `join_all`），所以一批里若有 3 个改动，就会同时挂出 3 张权限卡。
/// 用户在第一张上点"本次会话都允许"时，另外两张早就卡在 `request_permission` 里等自己的
/// 那个 oneshot 了 —— 它们不会回头复查允许列表，所以不消化就得挨个点。
///
/// 放行积压时发的是 [`PermissionDecision::Allow`] 而不是 `AllowSession`：键刚刚已经
/// 登记过了，再发一次会在"已允许"面板里留下重复条目。
///
/// shell 工具内部的第二道门（`shell_tools::execution`）也调用本函数，保证"点一次就够了"
/// 这件事在两道门上都成立。
pub async fn grant_session_allowance(
    app: &tauri::AppHandle,
    session_id: &str,
    kind: &str,
    scope: &str,
    label: String,
) {
    let manager = app.state::<crate::infra::state::state::SessionManager>();
    let ctx = manager.get_or_create(session_id).await;

    // 同一个"类别 + 范围"只登记一次：用户可能在两张并排的卡上先后点了"本次会话都允许"，
    // 不去重的话"已允许"面板里会出现两条一模一样的条目
    {
        let mut list = ctx.session_allowances.lock().await;
        if !list.iter().any(|a| a.kind == kind && a.scope == scope) {
            list.push(SessionAllowance {
                kind: kind.to_string(),
                scope: scope.to_string(),
                label,
            });
        }
    }
    // 让界面的"已允许"面板立刻刷新
    let _ = app.emit(
        "session-allowances-changed",
        serde_json::json!({ "sessionId": session_id }),
    );

    // 挑出会被这条允许覆盖的积压请求：同一个类别 + 同一个范围 + 是工具确认
    // （循环续跑确认和方案审批没有会话级允许语义，不能顺手放行）
    let swept: Vec<(String, tokio::sync::oneshot::Sender<PermissionDecision>)> = {
        let mut perms = ctx.pending_permissions.lock().await;
        let hit: Vec<String> = perms
            .iter()
            .filter(|(_, entry)| {
                entry.kind == PermissionKind::Tool
                    && matches!(&entry.allowance, Some((k, s)) if k.as_str() == kind && s.as_str() == scope)
            })
            .map(|(id, _)| id.clone())
            .collect();
        hit.into_iter()
            .filter_map(|id| perms.remove(&id).map(|entry| (id, entry.responder)))
            .collect()
    };

    // 先收锁再唤醒：responder.send 会立刻唤醒等待方，而对方醒来马上要锁同一张表
    for (id, responder) in swept {
        println!("[JARVIS] 会话级允许覆盖了积压请求 {}，自动放行", id);
        let _ = responder.send(PermissionDecision::Allow);
        let _ = app.emit(
            "permission-resolved",
            serde_json::json!({
                "id": id,
                "sessionId": session_id,
                "decision": "allow",
                "decisionText": "",
            }),
        );
    }
}

/// "这条命令是否已被本会话允许"——给 shell 工具内部的第二道门用。
///
/// 键口径与 [`allowance_key_for`] 共用 [`command_prefix_scope`]，所以外层判定放行了，
/// 内层就不会再弹一次卡。
pub async fn command_allowed(app: &tauri::AppHandle, session_id: &str, command: &str) -> bool {
    let Some((scope, _)) = command_prefix_scope(command) else {
        return false;
    };
    let manager = app.state::<crate::infra::state::state::SessionManager>();
    let ctx = manager.get_or_create(session_id).await;
    ctx.allowance_covers(ALLOWANCE_KIND_COMMAND, &scope).await
}

/// 执行前判定：返回 `Some(拒绝结果)` 表示**不要执行**，把这段文本作为工具结果回灌给模型。
///
/// 返回 `None` 表示放行。
pub async fn enforce(
    app: &tauri::AppHandle,
    session_id: &str,
    tool: &str,
    input: &Value,
    agent_type: &str,
) -> Option<super::ToolCallResult> {
    let manager = app.state::<crate::infra::state::state::SessionManager>();
    let ctx = manager.get_or_create(session_id).await;
    let facts = prepare_facts(&ctx, tool, input).await;

    let mode = match ctx.approval_mode.lock().await.as_str() {
        "auto_approve" => ApprovalMode::AutoApprove,
        _ => ApprovalMode::RequestApproval,
    };

    let decision = policy::judge(tool, mode, &facts.to_input());

    match decision.outcome {
        Outcome::Allow => None,
        Outcome::Deny => {
            println!(
                "[JARVIS] 权限判定拒绝：工具={} 原因={}",
                tool, decision.reason
            );
            Some(super::ToolCallResult::blocked(format!(
                "这个操作被系统拒绝，不能执行：{}\n不要换等价写法重试；改用其他方案，或向用户说明受阻点。",
                decision.reason
            )))
        }
        Outcome::Ask => {
            // "覆盖已有文件"这类询问刻意**每次都要问**（见 `policy::ask_always_repeated`）。
            // 这类卡片也**不给会话级允许**：键登记了也吞不掉下一次，显示按钮就是骗人。
            let key = if policy::ask_always_repeated(facts.class, facts.target_exists) {
                None
            } else {
                facts.allowance.clone()
            };

            // ① 本会话已允许过"这个操作类别 + 这个范围" → 直接放行
            if let Some(key) = &key {
                if ctx.allowance_covers(key.kind, &key.scope).await {
                    return None;
                }
            }

            // ② 弹窗问用户。把键一并交过去：一是让前端知道"本次会话都允许"是否真有
            //    明确含义（没键就别显示这个按钮），二是用户点了之后能据此消化积压。
            let message = build_permission_message(tool, &facts, &decision.reason, agent_type);
            let pending_key = key.as_ref().map(|k| (k.kind.to_string(), k.scope.clone()));
            let decision = super::permission::request_permission(
                app,
                session_id,
                &message,
                PermissionKind::Tool,
                pending_key,
            )
            .await;
            match decision {
                PermissionDecision::Allow => None,
                PermissionDecision::AllowSession => {
                    if let Some(k) = key {
                        grant_session_allowance(app, session_id, k.kind, &k.scope, k.label).await;
                    }
                    None
                }
                other => Some(super::ToolCallResult::blocked(format!(
                    "这个操作没有执行：{}",
                    other.model_note()
                ))),
            }
        }
    }
}

/// 弹窗文案：要做什么、对什么做、为什么问、以及"本次会话都允许"会允许到什么范围
///
/// 输出是"第一行=动作，其余=明细"的朴素结构，前端按行渲染，避免一坨长句：
///
/// ```text
/// 需要确认：删除文件
/// C:\...\permission-test\trash.txt
/// 工具：DeleteFile
/// 风险提示：⚠ 检测到递归删除操作
/// 允许范围：在 C:\...\permission-test 里删除文件
/// ```
///
/// "允许范围"这一行是给用户做决定用的，必须说清**类别 + 范围**，不能只写工具名：
/// 授权粒度是按类别合并的（例如"改项目文件"同时覆盖新建/编辑/改名）。
fn build_permission_message(
    tool: &str,
    facts: &PreparedFacts,
    reason: &str,
    agent_type: &str,
) -> String {
    let action = facts.class.map(|c| c.label()).unwrap_or("操作");
    let target = facts
        .targets
        .first()
        .cloned()
        .unwrap_or_else(|| "（无具体路径）".to_string());
    // 动作（第一行）
    let mut msg = format!("需要确认：{}", action);
    // 明细：对象 → 工具 → 风险提示 → 允许范围 → 来源
    msg.push_str(&format!("\n{}", target));
    msg.push_str(&format!("\n工具：{}", tool));
    let _ = reason;
    if let Some(warning) = &facts.warning {
        msg.push_str(&format!("\n风险提示：{}", warning));
    }
    if let Some(key) = &facts.allowance {
        msg.push_str(&format!("\n允许范围：{}", key.label));
    }
    if agent_type == "subagent" {
        msg.push_str("\n来源：子代理（并行任务）发起");
    }
    msg
}
