//! # permission.rs — 权限管理模块
//!
//! 路径安全检查、沙箱边界校验、用户权限确认（通过 oneshot channel 阻塞等待前端决策）。
//!
//! ## 关键导出
//! - `is_path_safe()`: 检查路径是否包含 `..` 遍历
//! - `is_within_workspace()`: 检查路径是否在沙箱工作目录内
//! - `ensure_path_permission()`: 综合权限检查（安全 + 沙箱边界）
//! - `request_permission()`: 向前端发送权限确认请求，**无限期等待**用户决策
//! - `PermissionDecision`: 结构化决策结果（允许/会话级允许/明确拒绝/未取得结论）
//!
//! ## 约束
//! - 沙箱会话强制执行路径边界检查
//! - 非沙箱会话不做额外拦截，允许访问最大范围
//! - `AllowSession` 决策会被缓存，后续请求自动放行
//! - **没有超时**：权限等待只被三件事打断——用户给出决策、本轮被取消、权限通道被关闭。
//!   绝不允许"等太久就默认拒绝然后继续跑"：那等于系统替用户做了决定，
//!   模型还会拿着假的"拒绝"继续推理甚至换个写法重试。

use serde_json::json;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use tauri::{Emitter, Manager};
use tokio::sync::oneshot;

use crate::infra::state::state::{SessionContext, SessionManager};

/// 权限请求来源：让前端能把"工具调用确认"和"循环续跑确认"区分开
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PermissionKind {
    /// 工具调用的权限确认（如高风险 Shell 命令）
    Tool,
    /// Agent 循环达到上限后的续跑确认
    LoopContinuation,
    /// 方案审批通道（由 ProposePlan 占位，实际决议走产品层状态机）
    PlanApproval,
}

impl PermissionKind {
    pub fn as_str(&self) -> &'static str {
        match self {
            PermissionKind::Tool => "tool",
            PermissionKind::LoopContinuation => "loop_continuation",
            PermissionKind::PlanApproval => "plan_approval",
        }
    }

    /// 是否允许"本次会话始终允许"（只有工具确认才该有会话级授权语义）
    pub fn allows_session_wide_approval(&self) -> bool {
        matches!(self, PermissionKind::Tool)
    }
}

/// 权限决策结果。
///
/// 关键设计：**"用户拒绝"和"没等到结论"必须是两个不同的东西**。
/// 早期实现把两者都折叠成字符串 `"reject"`，于是 30 秒无人应答时系统替用户拒绝，
/// 模型拿着假的拒绝继续跑，前端卡片还挂着 —— 用户点"允许"已经无效。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PermissionDecision {
    /// 用户允许这一次操作
    Allow,
    /// 用户授权本会话后续同类操作（会话级放行）
    AllowSession,
    /// 用户明确拒绝，可带一句话说明/替代方案（会回灌给模型）
    Reject { feedback: Option<String> },
    /// 没有拿到用户结论：本轮被取消、权限通道被关闭等。**不等于拒绝**
    Interrupted { reason: String },
}

impl PermissionDecision {
    /// 是否放行本次操作
    pub fn is_allowed(&self) -> bool {
        matches!(self, PermissionDecision::Allow | PermissionDecision::AllowSession)
    }

    /// 是否为用户明确拒绝
    pub fn is_rejected(&self) -> bool {
        matches!(self, PermissionDecision::Reject { .. })
    }

    /// 回灌给模型的说明文本（允许类为空）
    pub fn model_note(&self) -> String {
        match self {
            PermissionDecision::Allow | PermissionDecision::AllowSession => String::new(),
            PermissionDecision::Reject { feedback } => {
                let head = "用户明确拒绝了这次操作。";
                let tail = "不要用等价写法重试（换参数、换命令别名、换等价工具都算重试）：\
                            改用不触犯该限制的方案，或向用户说明受阻点并询问下一步。";
                match feedback {
                    Some(text) if !text.trim().is_empty() => {
                        format!("{}\n用户说明：{}\n{}", head, text.trim(), tail)
                    }
                    _ => format!("{}\n{}", head, tail),
                }
            }
            PermissionDecision::Interrupted { reason } => format!(
                "权限确认未完成（{}），本次操作已跳过。这**不是**用户拒绝，\
                 但也不要重复发起同一请求，先向用户说明当前情况。",
                reason
            ),
        }
    }

    /// 审计/调试用的短标签
    pub fn status_label(&self) -> &'static str {
        match self {
            PermissionDecision::Allow => "allow",
            PermissionDecision::AllowSession => "allow_session",
            PermissionDecision::Reject { .. } => "reject",
            PermissionDecision::Interrupted { .. } => "interrupted",
        }
    }
}

pub fn is_path_safe(path_str: &str) -> bool {
    !path_str.contains("..")
}

fn normalize_path(path: &Path) -> PathBuf {
    // 剥掉 Windows 长路径前缀 \\?\，否则组件序列不匹配
    let path_str = path.to_string_lossy();
    let path = if path_str.starts_with(r"\\?\") {
        Path::new(&path_str[4..])
    } else {
        path
    };
    let mut components = Vec::new();
    for comp in path.components() {
        match comp {
            std::path::Component::ParentDir => {
                components.pop();
            }
            std::path::Component::CurDir => {}
            other => components.push(other),
        }
    }
    components.iter().collect()
}

/// 检查路径是否在工作目录沙箱内（解析 `.` 和 `..` 后比较前缀）
pub fn is_within_workspace(path_str: &str, workspace_dir: Option<&Path>) -> bool {
    if !is_path_safe(path_str) {
        return false;
    }
    // 无沙箱限制时直接放行
    let ws = match workspace_dir {
        Some(d) => d,
        None => return true,
    };
    let path = Path::new(path_str);
    // 相对路径基于沙箱目录解析（而非进程 CWD）
    let resolved = if path.is_absolute() {
        normalize_path(path)
    } else {
        normalize_path(&ws.join(path))
    };
    let ws_normalized = normalize_path(ws);
    resolved.starts_with(&ws_normalized)
}

pub async fn ensure_path_permission(
    _app: &tauri::AppHandle,
    path_str: &str,
    _action: &str,
    workspace_dir: Option<&Path>,
) -> Result<(), String> {
    if !is_path_safe(path_str) {
        return Err("路径不安全：包含 '..' 遍历".to_string());
    }

    // 如果指定了工作目录（即沙箱会话：会话挂了项目），则强制执行边界检查
    if let Some(ws) = workspace_dir {
        if !is_within_workspace(path_str, Some(ws)) {
            return Err(format!(
                "沙箱边界：路径 '{}' 不在会话绑定的项目目录 '{}' 内，拒绝访问。\
                 需要操作其他目录，请新建不挂项目的会话（非沙箱会话，无目录边界）。",
                path_str,
                ws.display()
            ));
        }
    }

    // 非沙箱会话（workspace_dir 为 None）下，此处不做额外拦截，允许访问最大范围。
    Ok(())
}

/// 向前端发送权限确认请求，通过 oneshot channel **无限期**等待用户决策。
///
/// 返回结构化的 [`PermissionDecision`]，调用方必须区分三种情况：
/// - `is_allowed()` → 放行执行
/// - `Reject` → 本次操作不执行，把 `model_note()` 回灌给模型（loop 继续，让模型换方案）
/// - `Interrupted` → 没拿到用户结论（例如用户点了停止），也不执行，但不能当成"用户拒绝"
///
/// `allowance` 是这次请求对应的"会话级允许"范围键 `(工具, 范围)`；`None` 表示这次操作
/// 不支持会话级允许（例如"循环续跑确认"，照抄这套语义会顺带放行其它工具调用）。
///
/// 由于取消了超时，必须保证"等待这件事本身不会被无声丢弃"：
/// 如果调用方的 future 被取消/被打断（例如子代理任务撞上调度器 5 分钟超时被 drop），
/// 这里挂的 [`PendingPermissionGuard`] 会负责把等待条目清掉并通知前端撤卡，
/// 否则界面上会留下一张点了也没反应的僵尸卡片。
pub async fn request_permission(
    app: &tauri::AppHandle,
    session_id: &str,
    message: &str,
    kind: PermissionKind,
    allowance: Option<(String, String)>,
) -> PermissionDecision {
    let session_manager = app.state::<SessionManager>();
    let ctx = session_manager.get_or_create(session_id).await;

    // 已经被"本会话都允许"覆盖 → 不问用户，直接放行。
    // 这道前置检查还堵住了另一个窗口：调用方刚判定"未命中允许列表"、用户随后就点了
    // "本次会话都允许"，此时再插一张卡进去就是一张永远等不到点击的僵尸卡片。
    if let Some((tool, scope)) = &allowance {
        if ctx.allowance_covers(tool, scope).await {
            return PermissionDecision::Allow;
        }
    }

    // 生成唯一请求 ID，创建 oneshot channel 等待前端回调
    static REQ_ID: AtomicUsize = AtomicUsize::new(1);
    let id = REQ_ID.fetch_add(1, Ordering::SeqCst).to_string();

    let (tx, rx) = oneshot::channel();
    // 插入前清理"等待方已经不在了"的历史条目
    {
        let mut perms = ctx.pending_permissions.lock().await;
        let now = std::time::Instant::now();
        // 只清理 responder 已被丢弃的历史条目；不再按时间淘汰 pending 请求，
        // 等待多久不由系统替用户决定
        perms.retain(|_, entry| {
            now.duration_since(entry.created_at).as_secs() < 300 && !entry.responder.is_closed()
        });
        perms.insert(
            id.clone(),
            crate::infra::state::state::PendingPermission {
                created_at: std::time::Instant::now(),
                message: message.to_string(),
                kind,
                allowance: allowance.clone(),
                responder: tx,
            },
        );
    }

    // 兜底清理器：future 被 drop（取消/超时打断）时也能撤掉等待条目与界面卡片
    let _guard = PendingPermissionGuard {
        ctx: ctx.clone(),
        app: app.clone(),
        session_id: session_id.to_string(),
        id: id.clone(),
    };

    let _ = app.emit(
        "permission-request",
        json!({
            "id": id,
            "message": message,
            "sessionId": session_id,
            "kind": kind.as_str(),
            // 只有"工具确认"且这条操作**真的有范围键**时，会话级允许才有个明确的含义；
            // 否则按钮点了等于没点（旧实现里这是个会骗人的按钮）
            "allowSession": kind.allows_session_wide_approval() && allowance.is_some(),
        }),
    );

    // 无限期等待：只有"用户给出决策""本轮取消""权限通道关闭"能结束等待。
    // 这里刻意不设 tokio::time::timeout —— 超时不是用户的决定，系统不能替用户拒绝。
    let cancel = ctx.cancel_token.lock().await.clone();
    let decision = match cancel {
        Some(token) => tokio::select! {
            result = rx => result.unwrap_or_else(|_| PermissionDecision::Interrupted {
                reason: "权限通道已关闭".to_string(),
            }),
            _ = token.cancelled() => PermissionDecision::Interrupted {
                reason: "本轮执行已被取消".to_string(),
            },
        },
        None => rx.await.unwrap_or_else(|_| PermissionDecision::Interrupted {
            reason: "权限通道已关闭".to_string(),
        }),
    };

    // 正常路径下 resolve_permission 已经取走条目并广播了 permission-resolved；
    // 只有取消/通道关闭这类异常路径还留着条目，这里负责清理并通知前端撤卡，
    // 否则界面上会残留一张点不动的权限卡。
    {
        let removed = ctx.pending_permissions.lock().await.remove(&id);
        if removed.is_some() {
            let _ = app.emit(
                "permission-resolved",
                json!({
                    "id": id,
                    "sessionId": session_id,
                    "decision": decision.status_label(),
                    "decisionText": decision.model_note(),
                }),
            );
        }
    }

    decision
}

/// 等待条目的生命周期守卫。
///
/// 正常情况下 `request_permission` 会在返回前自己把条目取走，守卫 drop 时已无事可做；
/// 只有"调用方被打断"（子代理任务超时被 drop、任务取消等）才会由守卫完成清理。
struct PendingPermissionGuard {
    ctx: Arc<SessionContext>,
    app: tauri::AppHandle,
    session_id: String,
    id: String,
}

impl Drop for PendingPermissionGuard {
    fn drop(&mut self) {
        let ctx = self.ctx.clone();
        let app = self.app.clone();
        let session_id = self.session_id.clone();
        let id = self.id.clone();

        let cleanup = async move {
            let removed = ctx.pending_permissions.lock().await.remove(&id);
            if removed.is_some() {
                let _ = app.emit(
                    "permission-resolved",
                    json!({
                        "id": id,
                        "sessionId": session_id,
                        "decision": "interrupted",
                        "decisionText": "权限确认未完成：本轮执行已被中断",
                    }),
                );
            }
        };

        match tokio::runtime::Handle::try_current() {
            Ok(handle) => {
                handle.spawn(cleanup);
            }
            Err(_) => {
                // 没有异步运行时（进程收尾阶段）：同步尽力清理，至少不留脏条目
                if let Ok(mut perms) = self.ctx.pending_permissions.try_lock() {
                    perms.remove(&self.id);
                }
            }
        }
    }
}

#[cfg(test)]
mod decision_tests {
    use super::PermissionDecision;

    #[test]
    fn allow_variants_are_allowed() {
        assert!(PermissionDecision::Allow.is_allowed());
        assert!(PermissionDecision::AllowSession.is_allowed());
        assert!(!PermissionDecision::Allow.is_rejected());
    }

    #[test]
    fn reject_and_interrupted_are_not_allowed() {
        let rejected = PermissionDecision::Reject { feedback: None };
        let interrupted = PermissionDecision::Interrupted {
            reason: "本轮执行已被取消".to_string(),
        };
        assert!(!rejected.is_allowed());
        assert!(rejected.is_rejected());

        // 关键回归点：中断 ≠ 用户拒绝，不能被当成"用户拒绝了，换个写法继续"
        assert!(!interrupted.is_allowed());
        assert!(!interrupted.is_rejected());
        assert!(interrupted.model_note().contains("不是"));
    }

    #[test]
    fn reject_note_forbids_equivalent_retry() {
        let with_feedback = PermissionDecision::Reject {
            feedback: Some("这个文件保留".to_string()),
        };
        let note = with_feedback.model_note();
        assert!(note.contains("这个文件保留"));
        assert!(note.contains("不要用等价写法重试"));
        assert!(note.contains("改用不触犯该限制的方案"));
    }

    #[test]
    fn status_labels_are_stable_for_audit() {
        assert_eq!(PermissionDecision::Allow.status_label(), "allow");
        assert_eq!(
            PermissionDecision::AllowSession.status_label(),
            "allow_session"
        );
        assert_eq!(
            PermissionDecision::Reject { feedback: None }.status_label(),
            "reject"
        );
        assert_eq!(
            PermissionDecision::Interrupted {
                reason: "x".to_string()
            }
            .status_label(),
            "interrupted"
        );
    }
}
