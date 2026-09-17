//! # state.rs — 状态管理模块
//!
//! 定义 Tauri 应用的全局状态管理器，包括会话管理、工作空间状态和快照注册表。
//! 使用 `Arc<Mutex<T>>` 和 `RwLock` 实现线程安全的状态共享。
//!
//! ## 关键导出
//! - `SessionManager`: 全局会话管理器，维护所有活跃会话的上下文
//! - `SessionContext`: 单个会话的上下文，包含记忆、取消令牌、待处理权限等
//! - `WorkspaceState`: 工作空间状态，记录当前工作目录
//! - `SnapshotRegistry`: 快照注册表，管理会话级快照
//! - `SessionCleanupResult`: 会话清理结果，用于返回删除和激活的会话 ID
//!
//! ## 依赖
//! - Internal: `crate::infra::types::models::SessionMemory`, `crate::core::session`, `crate::core::rollback::session_manager::SnapshotManagerRegistry`
//! - External: `tokio`, `std::sync::Arc`, `std::collections::HashMap`
//!
//! ## 约束
//! - 所有状态必须通过 Tauri 的 `.manage()` 注册
//! - 使用 `RwLock` 允许多读单写，`Mutex` 用于互斥访问
//! - `SessionManager::get_or_create()` 会自动从 SQLite 加载历史数据

use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::{Mutex, RwLock};

use crate::infra::types::models::SessionMemory;
use crate::core::rollback::session_manager::SnapshotManagerRegistry;

pub struct WorkspaceState(pub Mutex<Option<std::path::PathBuf>>);

#[derive(Clone, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SessionCleanupResult {
    pub deleted_session_id: Option<String>,
    pub active_session_id: Option<String>,
}

pub struct SnapshotRegistry(pub RwLock<SnapshotManagerRegistry>);

#[derive(Clone, Debug, serde::Serialize, serde::Deserialize)]
pub struct ToolDedupeCacheEntry {
    pub display: String,
    pub running: bool,
    pub suppressed_count: usize,
}

#[derive(Clone, Debug, serde::Serialize, serde::Deserialize)]
pub struct PendingPlanCacheEntry {
    pub display: String,
    pub title: String,
    pub id: String,
    pub suppressed_count: usize,
}

#[derive(Clone, Debug)]
pub struct PendingSnapshotPatch {
    pub run_id: String,
    pub seq: usize,
    pub patch: crate::core::rollback::Patch,
    pub message: Option<String>,
    pub trigger_user_memory_index: Option<usize>,
    pub trigger_user_message_id: Option<String>,
}

/// 本会话已允许的范围（用户点过"本次会话都允许"）
///
/// 粒度 = **操作类别 + 范围**（不是"工具 + 范围"）：
/// - 文件类：类别 = `edit_project`（新建/编辑/改名）或 `delete`，
///   范围 = **会话项目根**（`ctx.workspace`；无项目的会话退回目标所在目录）。
///   粒度刻意提到项目级：目录级会让一次脚手架搭建弹 N 张卡（每个目录各授权一次），
///   用户点完只能得出"这个按钮没用"。越界由沙箱边界（`policy::judge` 规则 2）挡住，
///   项目级授权不会让 agent 够到项目外。
/// - 命令类：类别 = `run_command`，范围 = 命令前缀（例如 `npm run`）
///
/// 类别刻意收敛成 3 个：类别越细，"本次会话都允许"就越等于每次都得点一遍，
/// 用户会用"其实还不如每次点允许"来回避它。类别与范围的口径见
/// `core::tools::framework::policy_guard::allowance_key_for`。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SessionAllowance {
    /// 操作类别（`edit_project` / `delete` / `run_command`）
    pub kind: String,
    /// 范围键（文件类 = 项目根或目标所在目录的绝对路径；命令类 = 命令前缀）
    pub scope: String,
    /// 展示给用户看的说明（弹窗与"已允许"面板共用）
    pub label: String,
}

/// 一条等待用户决策的权限请求
pub struct PendingPermission {
    pub created_at: std::time::Instant,
    /// 展示给用户的文案
    pub message: String,
    /// 请求来源（工具确认 / 循环续跑确认 / 方案审批），决定前端能提供哪些按钮
    pub kind: crate::core::tools::framework::permission::PermissionKind,
    /// 这条请求对应的"会话级允许"键 `(操作类别, 范围)`。
    ///
    /// 两个用途：
    /// 1. 前端据此决定要不要显示"本次会话都允许"——没有键时点了也没用，不该显示
    /// 2. 用户点了"本次会话都允许"之后，据此把**已经挂起**、会被同一条允许覆盖的
    ///    请求一次性放行，不用用户挨个点（口径见 `policy_guard::allowance_key_for`）
    pub allowance: Option<(String, String)>,
    /// 发起这条请求的工具名与原始入参（仅工具确认有）。
    ///
    /// 切换权限档位时，清扫逻辑（`policy_guard::sweep_pending_on_mode_change`）
    /// 用它按新档位**重放** `policy::judge`，自动消化"新档位下根本不用问"的挂起卡。
    /// 循环续跑确认、方案审批没有档位语义，为 `None`，不会被清扫重放。
    pub origin: Option<(String, serde_json::Value)>,
    /// 决策发送端（结构化决策，不是字符串）
    pub responder: tokio::sync::oneshot::Sender<
        crate::core::tools::framework::permission::PermissionDecision,
    >,
}

pub struct SessionContext {
    pub id: String,
    pub memory: Mutex<SessionMemory>,
    pub cancel_token: Mutex<Option<tokio_util::sync::CancellationToken>>,
    pub active_run_id: Mutex<Option<String>>,
    pub todos: Mutex<Vec<crate::infra::types::models::TodoItem>>,
    pub workspace: Mutex<Option<std::path::PathBuf>>,
    /// 待用户决策的权限请求。
    /// 发送端传的是结构化的 `PermissionDecision`，不是字符串——
    /// "用户拒绝"和"没等到结论"必须能区分（详见 framework::permission）
    pub pending_permissions: Mutex<HashMap<String, PendingPermission>>,
    pub pending_patches: Mutex<Vec<PendingSnapshotPatch>>,
    pub pending_plan_state: Mutex<HashMap<String, PendingPlanCacheEntry>>,
    /// 统一去重缓存：category → (key → entry)，替代原先分散的 compact / consolidate / skill / subagent 缓存
    pub dedupe_cache: Mutex<HashMap<String, HashMap<String, ToolDedupeCacheEntry>>>,
    /// 用户类型（"user" / "developer"），只有用户手动切换
    pub agent_audience: Mutex<String>,
    /// 工作模式（"edit" / "plan"）——用户可手动切换，Edit 下 Agent 可自动切 Plan。
    /// 权限档位（问得多严）另存于 `approval_mode`。
    pub agent_work_mode: Mutex<String>,
    /// 深度思考档位（会话级表态）：`None`/`"auto"` = 跟随预设默认，
    /// `"always"` = 本会话强制开启，`"never"` = 本会话强制关闭。
    ///
    /// 与 `sessions.thinking_mode` 同步；决策逻辑见 `core::session::thinking`。
    pub thinking_mode: Mutex<Option<String>>,
    /// **本轮**最终生效的思考状态（由主 Agent 的裁决层写入）。
    ///
    /// 子 Agent 用它**继承主 Agent 的档位**，而不是各自按全局 `agent_audience` 重新推导——
    /// 否则会出现"主 Agent 不开思考、子 Agent 却在思考"的不一致（设计文档 K3）。
    ///
    /// `None` = 本会话尚未跑过主 Agent（尚无裁决），子 Agent 此时回落 audience 默认值。
    /// 每轮开始时被主 Agent 覆盖为 `Some(..)`。
    pub turn_think: Mutex<Option<bool>>,
    /// 调度器事件接收端（异步模式）：RunSubagentsSequentially 存，pipeline 取
    pub scheduler_rx: Mutex<Option<tokio::sync::mpsc::UnboundedReceiver<crate::core::orchestration::scheduler::SchedulerEvent>>>,
    /// ReadFile 探索拦截：记录本会话已读取的文件路径，用于检测逐文件遍历模式
    pub read_file_paths: Mutex<Vec<String>>,
    /// 循环上限续跑标记：超时后用户仍可点"允许"来 resume
    pub loop_continuation_pending: Mutex<bool>,
    /// 工具调用的结构化标志 (break_loop, is_error)，按工具名索引
    /// 由 dispatch_tool_call 写入，tools_runner 读取后清除
    pub tool_result_flags: Mutex<HashMap<String, (bool, bool)>>,
    /// 权限判定的"本轮"状态（本轮改过哪些文件）——给批量规则用
    pub permission_turn:
        Mutex<crate::core::tools::framework::policy_guard::PermissionTurnState>,
    /// 权限档位："request_approval"（请求审批，默认）/ "auto_approve"（帮我批准）
    pub approval_mode: Mutex<String>,
    /// 本会话已允许的范围（内存态，会话结束即失效）
    pub session_allowances: Mutex<Vec<SessionAllowance>>,
    /// 只读保护（用户显式开关，默认关）。
    ///
    /// 开启后本会话禁止一切会改变工作区的动作：写 / 删 / 改名 / 打补丁文件、
    /// 执行非只读命令、起后台服务、派子代理、改工作目录。
    ///
    /// **与 `agent_work_mode` 的区别**：工作模式是模型也能自己切的正交通道
    /// （`SwitchWorkMode` 工具），而只读保护只看这个字段，切模式绕不过它。
    ///
    /// 刻意是**纯内存态**（与 `session_allowances` 同构）：放松的授权可以记住，
    /// 收紧的闸门记住容易变成"新会话里 agent 莫名其妙写不了文件"的幽灵故障。
    pub agent_read_only: Mutex<bool>,
}

impl SessionContext {
    pub fn new(id: String) -> Self {
        Self {
            id,
            memory: Mutex::new(SessionMemory::default()),
            cancel_token: Mutex::new(None),
            active_run_id: Mutex::new(None),
            todos: Mutex::new(Vec::new()),
            workspace: Mutex::new(None),
            pending_permissions: Mutex::new(HashMap::new()),
            pending_patches: Mutex::new(Vec::new()),
            pending_plan_state: Mutex::new(HashMap::new()),
            dedupe_cache: Mutex::new(HashMap::new()),
            agent_audience: Mutex::new("developer".to_string()),
            agent_work_mode: Mutex::new("edit".to_string()),
            thinking_mode: Mutex::new(None),
            turn_think: Mutex::new(None),
            scheduler_rx: Mutex::new(None),
            read_file_paths: Mutex::new(Vec::new()),
            loop_continuation_pending: Mutex::new(false),
            tool_result_flags: Mutex::new(HashMap::new()),
            permission_turn: Mutex::new(Default::default()),
            approval_mode: Mutex::new("request_approval".to_string()),
            session_allowances: Mutex::new(Vec::new()),
            agent_read_only: Mutex::new(false),
        }
    }

    /// 只读保护是否开启。
    ///
    /// `enforce` 的最高优先级闸门与权限设置查询（`get_session_permission_settings`）都走这里，
    /// 不允许任何调用点自己读 `agent_read_only` 另搞一套。
    pub async fn read_only_enabled(&self) -> bool {
        *self.agent_read_only.lock().await
    }

    /// 本会话是否已经允许"这个操作类别 + 这个范围"。
    ///
    /// **唯一口径**：执行前判定（`policy_guard::enforce`）的放行检查、
    /// 权限请求（`permission::request_permission`）的"还要不要问用户"检查、
    /// 以及 shell 工具内部的第二道门（`policy_guard::command_allowed`）都走这里，
    /// 避免出现两套"已允许"标准。
    ///
    /// 匹配规则按类别分两套：
    /// - 文件类（`edit_project` / `delete`）：**上级覆盖下级**——范围键是项目根
    ///   （或无项目时的目标所在目录），所以"整个项目授权过"自然覆盖其子目录的请求
    ///   （继承判定见 `policy_guard::scope_covers`）。
    /// - 命令类（`run_command`）：字符串**精确相等**——命令前缀没有"包含"语义，
    ///   `npm run` 的授权不能放宽到 `npm run build` 以外的前缀。
    pub async fn allowance_covers(&self, kind: &str, scope: &str) -> bool {
        let file_kind = kind != crate::core::tools::framework::policy_guard::ALLOWANCE_KIND_COMMAND;
        self.session_allowances
            .lock()
            .await
            .iter()
            .any(|a| {
                a.kind == kind
                    && if file_kind {
                        crate::core::tools::framework::policy_guard::scope_covers(&a.scope, scope)
                    } else {
                        a.scope == scope
                    }
            })
    }
}

/// 全局管理器，维护所有活跃会话的 SessionContext
pub struct SessionManager(pub RwLock<HashMap<String, Arc<SessionContext>>>);

impl SessionManager {
    pub fn new() -> Self {
        Self(RwLock::new(HashMap::new()))
    }

    pub async fn remove(&self, session_id: &str) -> Option<Arc<SessionContext>> {
        self.0.write().await.remove(session_id)
    }

    pub async fn get_or_create(&self, session_id: &str) -> Arc<SessionContext> {
        let read_guard = self.0.read().await;
        if let Some(ctx) = read_guard.get(session_id) {
            return ctx.clone();
        }
        drop(read_guard);

        let mut write_guard = self.0.write().await;
        // Double check
        if let Some(ctx) = write_guard.get(session_id) {
            return ctx.clone();
        }

        let ctx = SessionContext::new(session_id.to_string());
        // 尝试从磁盘加载历史数据和工作目录
        if let Ok(memory) = crate::core::session::load_session(session_id) {
            *ctx.memory.lock().await = memory;
        }
        if let Ok(meta) = crate::core::session::get_session_meta(session_id) {
            // 存量记录可能带 \\?\ 前缀，绑定前剥离（下游报错文案与显示才干净）
            *ctx.workspace.lock().await = meta.working_directory.map(|ws| {
                std::path::PathBuf::from(
                    crate::core::session::strip_extended_path_prefix(&ws),
                )
            });
            // 深度思考档位随会话恢复（None = auto），与 profileId 的恢复路径对称
            *ctx.thinking_mode.lock().await =
                crate::core::session::thinking::normalize_session_mode(
                    meta.thinking_mode.as_deref(),
                )
                .map(|s| s.to_string());
        }

        let arc_ctx = Arc::new(ctx);
        write_guard.insert(session_id.to_string(), arc_ctx.clone());
        arc_ctx
    }
}

use tauri::Manager;

pub async fn active_run_scope_key(app: &tauri::AppHandle, session_id: &str) -> String {
    if let Some(manager) = app.try_state::<SessionManager>() {
        let ctx = manager.get_or_create(session_id).await;
        let run_id_lock = ctx.active_run_id.lock().await;
        if let Some(run_id) = run_id_lock.as_ref() {
            return format!("{}:{}", session_id, run_id);
        }
    }
    session_id.to_string()
}

pub fn stable_hash(text: &str) -> String {
    use std::collections::hash_map::DefaultHasher;
    use std::hash::{Hash, Hasher};
    let mut hasher = DefaultHasher::new();
    text.hash(&mut hasher);
    format!("{:x}", hasher.finish())
}

pub async fn effective_workspace(
    app: &tauri::AppHandle,
    session_id: &str,
) -> Option<std::path::PathBuf> {
    if let Some(manager) = app.try_state::<SessionManager>() {
        let ctx = manager.get_or_create(session_id).await;
        return ctx.workspace.lock().await.clone();
    }
    None
}
