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
    fn allowance_scope_uses_directory_for_files() {
        let (scope, label) = allowance_scope_for(
            Some(ToolClass::Delete),
            &json!({ "path": "src/a.ts" }),
            &["src/a.ts".to_string()],
            Some(std::path::Path::new("E:/proj")),
        );
        let scope = scope.expect("文件类应有范围键");
        assert!(scope.contains("proj"), "范围应是绝对目录：{}", scope);
        assert!(label.unwrap().contains("删除文件"));
    }

    #[test]
    fn allowance_scope_uses_command_fingerprint_for_commands() {
        let (scope, label) = allowance_scope_for(
            Some(ToolClass::RunCommand),
            &json!({ "command": "npm   run   build" }),
            &[],
            None,
        );
        assert_eq!(scope.as_deref(), Some("npm run build"), "命令指纹应折叠空白");
        assert!(label.unwrap().contains("同一命令"));
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
    /// 会话级允许的范围键（文件类=目录，命令类=命令指纹）；None 表示不支持会话级允许
    pub allowance_scope: Option<String>,
    /// 展示给用户的范围说明
    pub allowance_label: Option<String>,
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

    let (allowance_scope, allowance_label) =
        allowance_scope_for(class, input, &targets, workspace.as_deref());

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
        allowance_scope,
        allowance_label,
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

/// 会话级允许的范围键：
/// - 文件类：目标所在目录（相对工作区，便于展示成"在此目录里…"）
/// - 命令类：命令指纹（去掉多余空白；换命令会重新问）
/// - 其它类别（读、编排等本来就不问）：None
fn allowance_scope_for(
    class: Option<ToolClass>,
    input: &Value,
    targets: &[String],
    workspace: Option<&std::path::Path>,
) -> (Option<String>, Option<String>) {
    match class {
        Some(ToolClass::RunCommand) | Some(ToolClass::Background) => {
            let command = input["command"].as_str().unwrap_or("").trim();
            if command.is_empty() {
                return (None, None);
            }
            let fingerprint = command.split_whitespace().collect::<Vec<_>>().join(" ");
            let short: String = fingerprint.chars().take(60).collect();
            (
                Some(fingerprint),
                Some(format!("同一命令：{}", short)),
            )
        }
        Some(ToolClass::CreateFile)
        | Some(ToolClass::ModifyContent)
        | Some(ToolClass::Delete)
        | Some(ToolClass::Rename) => {
            let Some(first) = targets.first() else {
                return (None, None);
            };
            let dir = std::path::Path::new(first)
                .parent()
                .map(|p| p.to_path_buf())
                .unwrap_or_default();
            let resolved = if dir.is_absolute() {
                dir
            } else {
                workspace.map(|ws| ws.join(&dir)).unwrap_or(dir)
            };
            let display = resolved
                .to_string_lossy()
                .trim_start_matches(r"\\?\")
                .to_string();
            let action = class.map(|c| c.label()).unwrap_or("操作");
            (
                Some(display.clone()),
                Some(format!("在 {} 里{}", display, action)),
            )
        }
        _ => (None, None),
    }
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
            // ① 本会话已允许过"这个工具 + 这个范围" → 直接放行
            if let (Some(scope), _) = (&facts.allowance_scope, &facts.allowance_label) {
                let allowed = ctx
                    .session_allowances
                    .lock()
                    .await
                    .iter()
                    .any(|a| a.tool == tool && &a.scope == scope);
                if allowed {
                    return None;
                }
            }

            // ② 弹窗问用户
            let message = build_permission_message(tool, &facts, &decision.reason, agent_type);
            let decision = super::permission::request_permission(
                app,
                session_id,
                &message,
                PermissionKind::Tool,
            )
            .await;
            match decision {
                PermissionDecision::Allow => None,
                PermissionDecision::AllowSession => {
                    if let Some(scope) = facts.allowance_scope.clone() {
                        let label = facts
                            .allowance_label
                            .clone()
                            .unwrap_or_else(|| format!("{}（{}）", tool, scope));
                        ctx.session_allowances.lock().await.push(SessionAllowance {
                            tool: tool.to_string(),
                            scope,
                            label,
                        });
                        // 让界面的"已允许"面板立刻刷新
                        let _ = app.emit(
                            "session-allowances-changed",
                            serde_json::json!({ "sessionId": session_id }),
                        );
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
/// 删除文件
/// C:\...\permission-test\trash.txt
/// 工具：DeleteFile
/// 风险提示：⚠ 检测到递归删除操作
/// 允许范围：在 C:\...\frontend 里删除文件
/// ```
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
    if let Some(label) = &facts.allowance_label {
        msg.push_str(&format!("\n允许范围：{}", label));
    }
    if agent_type == "subagent" {
        msg.push_str("\n来源：子代理（并行任务）发起");
    }
    msg
}
