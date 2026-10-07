//! 权限决策审计日志。
//!
//! 以 JSONL 格式 append-only 写入 `data/logs/permission_decisions/` 目录，按 session 隔离。
//! 记录权限系统每一次"真正做决策"的时刻：键命中后的自动放行、弹卡后用户的点击
//! （允许一次 / 会话允许 / 拒绝）。没有这些记录，"用户允许过什么"就无从谈起
//! （授权落盘的素材来源，见 `doc/权限拦截-授权持久化方案`），弹卡量统计也只能按
//! 判定规则推算。配套 HTML 查看器：`data/logs/log-viewer.html`（统一审计中心）。
//!
//! 刻意**不记** judge 直接放行（Allow，未弹卡）的调用：只读工具每轮几百条全是噪音。
//! 这里只关心"问没问、问了之后结果如何"。
//!
//! **唯一例外**：`auto_approve`（2026-09-18 A 改造）——"帮我批准"档自动放行**非危险命令**
//! 时记一条。理由：这是档位策略放行、用户从未逐条表态，不记就彻底无痕；
//! 只读命令（Get-ChildItem 这类）仍然不记（噪音）。判据见 `policy.rs::always_asks` 分支。

use std::collections::HashMap;
use std::fs::OpenOptions;
use std::io::Write;
use std::path::PathBuf;
use std::sync::{Mutex, OnceLock};

use serde::Serialize;

/// 决策动作：键命中自动放行（用户此前点过允许，本次未弹卡）
pub const ACTION_KEY_HIT: &str = "key_hit";
/// 决策动作：用户点「允许一次」（只放行本次，不留键）
pub const ACTION_ALLOW: &str = "allow";
/// 决策动作：用户点「本次会话都允许」（登记会话允许键）
pub const ACTION_ALLOW_SESSION: &str = "allow_session";
/// 决策动作：用户点「拒绝」（中止本次工具调用）
pub const ACTION_REJECT: &str = "reject";
/// 决策动作：「帮我批准」档自动放行，未弹卡（2026-09-18 A）
///
/// 与 `key_hit` 的区别：`key_hit` 是用户**点过账本**、本次命中；本动作是档位策略
/// 直接放行，用户从未就这条命令表过态。这是 A 改造的安全垫——AutoApprove 档下
/// 非危险命令静默执行，唯一的事后追溯手段就是这条流水。
pub const ACTION_AUTO_APPROVE: &str = "auto_approve";

/// 一条权限决策记录
#[derive(Serialize)]
struct PermissionDecisionRecord {
    /// 会话内序号（对齐 tool_calls 日志的 seq 字段）
    seq: u64,
    ts: String,
    session_id: String,
    agent_type: String,
    /// 工具名（RunCommand / WriteFile / DeleteFile / …）
    tool: String,
    /// 会话允许类别（edit_project / delete / run_command）；无键时为 null
    kind: Option<String>,
    /// 范围键（文件类 = 项目根；命令类 = 命令前缀或整条）；无键时为 null
    scope: Option<String>,
    /// 决策动作（ACTION_* 常量之一）
    action: String,
    /// 危险警示文案（命中危险名单时非空）
    warning: Option<String>,
}

pub struct PermissionAuditLogger {
    log_dir: PathBuf,
    policy: crate::infra::log_maintenance::LogPolicy,
    /// 会话内序号（对齐 tool_calls 日志的 seq 字段，便于按序排查）
    seqs: Mutex<HashMap<String, u64>>,
}

impl PermissionAuditLogger {
    pub fn new() -> Self {
        let log_dir = crate::infra::config::data_paths::logs_dir().join("permission_decisions");
        let _ = std::fs::create_dir_all(&log_dir);
        Self {
            log_dir,
            policy: crate::infra::log_maintenance::LogPolicy::from_env(),
            seqs: Mutex::new(HashMap::new()),
        }
    }

    /// 记录一条权限决策。`kind`/`scope` 为本次弹卡提供的会话允许键（无键则 None）。
    pub fn log_decision(
        &self,
        session_id: &str,
        agent_type: &str,
        tool: &str,
        kind: Option<&str>,
        scope: Option<&str>,
        action: &str,
        warning: Option<&str>,
    ) {
        let seq = {
            let mut seqs = self
                .seqs
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            let entry = seqs.entry(session_id.to_string()).or_insert(0);
            *entry += 1;
            *entry
        };

        let record = PermissionDecisionRecord {
            seq,
            ts: chrono::Utc::now().to_rfc3339(),
            session_id: session_id.to_string(),
            agent_type: agent_type.to_string(),
            tool: tool.to_string(),
            kind: kind.map(|s| s.to_string()),
            scope: scope.map(|s| s.to_string()),
            action: action.to_string(),
            warning: warning.map(|s| s.to_string()),
        };
        self.write_record(session_id, &record);
    }

    fn write_record(&self, session_id: &str, record: &PermissionDecisionRecord) {
        let date = chrono::Local::now().format("%Y-%m-%d").to_string();
        // 按大小分片：超过阈值自动写 <date>_<session>.2.jsonl、.3.jsonl …（与 tool_calls 同款）
        let filename = crate::infra::log_maintenance::resolve_part_file(
            &self.log_dir,
            &date,
            session_id,
            &self.policy,
        );
        let path = self.log_dir.join(filename);

        // 多个子代理会并发写同一个文件；Windows 上并发 append 会交错，
        // 把两条 JSON 粘成一行导致记录读不出来。进程级锁串行化"打开 + 写一行"。
        static APPEND_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());
        let _guard = APPEND_LOCK
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());

        if let Ok(mut file) = OpenOptions::new().create(true).append(true).open(&path) {
            if let Ok(line) = serde_json::to_string(record) {
                let _ = writeln!(file, "{}", line);
            }
        }
    }
}

impl Default for PermissionAuditLogger {
    fn default() -> Self {
        Self::new()
    }
}

static LOGGER: OnceLock<PermissionAuditLogger> = OnceLock::new();

/// 全局唯一审计日志实例（进程级，与 `tool_call_logger` 同模式）
pub fn permission_audit_logger() -> &'static PermissionAuditLogger {
    LOGGER.get_or_init(PermissionAuditLogger::new)
}
