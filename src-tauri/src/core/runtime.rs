//! # runtime.rs — 会话级运行时上下文与全局状态管理器
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
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex as StdMutex, MutexGuard as StdMutexGuard};
use std::time::{SystemTime, UNIX_EPOCH};
use tokio::sync::{watch, Mutex, RwLock};

use crate::core::rollback::session_manager::SnapshotManagerRegistry;
use crate::core::tools::framework::policy::ApprovalMode;
use crate::infra::types::models::SessionMemory;

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
    /// 触发本轮补丁的用户消息 UUID（撤回关联的唯一锚点）。
    ///
    /// v29 起不再保存内存下标：它会被压缩/撤回改变并被后续轮次复用。
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
    /// 这条请求对应的"会话级允许"键列表 `(操作类别, 范围)`。
    ///
    /// 两个用途：
    /// 1. 前端据此决定要不要显示"本次会话都允许"——列表为空时点了也没用，不该显示
    /// 2. 用户点了"本次会话都允许"之后，据此把**已经挂起**、会被同一条允许覆盖的
    ///    请求一次性放行，不用用户挨个点（口径见 `policy_guard::allowance_keys_for`）
    ///
    /// 为什么是列表：链式命令（`npm install; npm run dev`）要拆段验权，
    /// 一段一把键——**全部段都拿到允许**这条请求才算被覆盖（2026-09-19 沐拍板 B 方案）。
    /// 单键操作（文件编辑等）就是长度为 1 的列表；不支持会话级允许的操作是空列表。
    pub allowance: Vec<(String, String)>,
    /// 发起这条请求的工具名与原始入参（仅工具确认有）。
    ///
    /// 切换权限档位时，清扫逻辑（`policy_guard::sweep_pending_on_mode_change`）
    /// 用它按新档位**重放** `policy::judge`，自动消化"新档位下根本不用问"的挂起卡。
    /// 循环续跑确认、方案审批没有档位语义，为 `None`，不会被清扫重放。
    pub origin: Option<(String, serde_json::Value)>,
    /// 命中"危险命令警示"时的警示文案（仅工具确认可能有）。
    /// 只用于前端把风险行做弱化的视觉分级（琥珀色小字），不参与任何判定逻辑。
    pub warning: Option<String>,
    /// 决策发送端（结构化决策，不是字符串）
    pub responder:
        tokio::sync::oneshot::Sender<crate::core::tools::framework::permission::PermissionDecision>,
}

/// 运行期咨询框的用户决策结果（与权限决策同构：区分正常回答、跳过与中断）。
#[derive(Debug, Clone)]
pub enum UserQuestionDecision {
    /// 用户提交了各题答复
    Answered(Vec<crate::infra::types::events::UserQuestionAnswerItem>),
    /// 用户点击“跳过”，由 Agent 按推荐方案自行决策
    Skipped,
    /// 本轮被取消或通道关闭（不等于用户跳过）
    Interrupted { reason: String },
}

/// 一条等待用户回复的运行期咨询请求（纯内存态，与 `PendingPermission` 同构）。
pub struct PendingUserQuestion {
    pub title: Option<String>,
    pub questions: Vec<crate::infra::types::events::UserQuestionItem>,
    pub responder: tokio::sync::oneshot::Sender<UserQuestionDecision>,
}

/// 当前会话中正在运行的一次主 Agent 执行。
///
/// `id` 标识这一轮执行，不是会话 ID；完成通知供停止命令和方案审批等待真正收尾。
pub struct ActiveSessionExecution {
    pub id: String,
    pub cancel_token: tokio_util::sync::CancellationToken,
    finished: watch::Sender<bool>,
}

pub struct SessionContext {
    pub id: String,
    pub memory: Mutex<SessionMemory>,
    /// 同一会话的执行占用、执行编号和取消令牌合并在同一把锁内，避免检查与占用分离。
    pub active_execution: StdMutex<Option<ActiveSessionExecution>>,
    /// 串行化执行登记与会话历史撤回，避免撤回过程中启动新执行。
    pub execution_gate: Mutex<()>,
    pub active_run_id: Mutex<Option<String>>,
    /// 会话的待办清单**分组**。
    ///
    /// 一个会话可以有多个组（长任务里多轮规划），全内存态、不落库，关闭应用即清空。
    /// 为什么不存 `Vec<TodoItem>`：单层列表只能表达“当前那一份”，要么整体替换（历史丢失），
    /// 要么纯追加（状态更新变成重复条目），两种都不对。
    pub todos: Mutex<Vec<crate::infra::types::models::TodoGroup>>,
    pub workspace: Mutex<Option<std::path::PathBuf>>,
    /// 待用户决策的权限请求。
    /// 发送端传的是结构化的 `PermissionDecision`，不是字符串——
    /// "用户拒绝"和"没等到结论"必须能区分（详见 framework::permission）
    pub pending_permissions: Mutex<HashMap<String, PendingPermission>>,
    /// 待用户回复的运行期咨询请求（AskUser 工具挂起通道，纯内存态）
    pub pending_questions: Mutex<HashMap<String, PendingUserQuestion>>,
    pub pending_patches: Mutex<Vec<PendingSnapshotPatch>>,
    pub pending_plan_state: Mutex<HashMap<String, PendingPlanCacheEntry>>,
    /// 统一去重缓存：category → (key → entry)，替代原先分散的 compact / consolidate / skill / subagent 缓存
    pub dedupe_cache: Mutex<HashMap<String, HashMap<String, ToolDedupeCacheEntry>>>,
    /// 用户类型（"user" / "developer"），只有用户手动切换
    pub agent_audience: Mutex<String>,
    /// 工作模式（"edit" / "plan"）——用户可手动切换，Edit 下 Agent 可自动切 Plan。
    /// 权限档位（问得多严）另存于 `approval_mode`。
    pub agent_work_mode: Mutex<String>,
    /// 深度思考档位（会话级，**布尔**）：`true` = 本会话开启，`false` = 关闭。
    ///
    /// 与 `sessions.thinking_mode` 同步；决策逻辑见 `core::session::thinking`。
    /// v14 起没有"未表态"状态——初值在会话第一条消息时由 pipeline 按
    /// "设置默认 + 模型能力"解析并落库，此后只由用户在界面上拨动。
    pub thinking_mode: Mutex<bool>,
    /// **本轮**最终生效的思考状态（由主 Agent 的裁决层写入）。
    ///
    /// 子 Agent 用它**继承主 Agent 的档位**，而不是各自按全局 `agent_audience` 重新推导——
    /// 否则会出现"主 Agent 不开思考、子 Agent 却在思考"的不一致（设计文档 K3）。
    ///
    /// `None` = 本会话尚未跑过主 Agent（尚无裁决），子 Agent 此时回落 audience 默认值。
    /// 每轮开始时被主 Agent 覆盖为 `Some(..)`。
    pub turn_think: Mutex<Option<bool>>,
    /// ReadFile 探索拦截：记录本会话已读取的文件路径，用于检测逐文件遍历模式
    pub read_file_paths: Mutex<Vec<String>>,
    /// 「先读后改」记录：本会话内模型已看到过的文件版本（路径 → 内容指纹）。
    ///
    /// - key = `resolve_exec_path` 之后的路径字符串（与读写工具执行时用的同一种形式）；
    ///   模型对同一文件给出不同写法的绝对路径（正/反斜杠）会被当成两个 key，
    ///   误报方向是"要求重读"，安全，不做路径归一化。
    /// - value = 解码后文本内容的 SHA-256 指纹（`file_tools::common::content_fingerprint`）。
    /// - 写入方：ReadFile / SymbolSearch(block) 成功返回前；EditFile / WriteFile / ApplyPatch
    ///   成功落盘后（刷新为写入后的新版本）。
    /// - 校验方：EditFile / WriteFile / ApplyPatch 落盘前，唯一实现在
    ///   `file_tools::common::ensure_fresh_read`，勿在别处另建第二套判据。
    ///
    /// 刻意**纯内存态**（与 `session_allowances` 同理）：语义是"模型的认知"，
    /// 认知随会话终结而失效。恢复历史会话后此表为空，第一次修改会被要求重读——
    /// 这是安全方向的代价，换来"每次修改依据的都是本会话内建立的认知"这条不变量。
    pub read_file_fingerprints: Mutex<HashMap<String, String>>,
    /// 循环上限续跑标记：超时后用户仍可点"允许"来 resume
    pub loop_continuation_pending: Mutex<bool>,
    /// 工具调用的结构化标志 (break_loop, is_error)，按工具名索引
    /// 由 dispatch_tool_call 写入，tools_runner 读取后清除
    pub tool_result_flags: Mutex<HashMap<String, (bool, bool)>>,
    /// 本轮工具调用（如 MCP 浏览器截图）产生的待附加视觉图片 `(media_type, attachment_filename)`
    /// 由工具执行层写入，main_loop 汇总 tool_results 时消费并清空（仅视觉模型附加）
    pub pending_tool_images: Mutex<Vec<(String, String)>>,
    /// 权限判定的"本轮"状态（本轮改过哪些文件）——给批量规则用
    pub permission_turn: Mutex<crate::core::tools::framework::policy_guard::PermissionTurnState>,
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
    /// 会话级 system 提示词缓存（提示词磁盘化后保证"会话内字节恒定"的强制闸门）。
    ///
    /// `None` = 尚未组装；首次发消息时由 pipeline 组装一次并存入，此后本会话
    /// 所有 turn 一律复用。没有它，用户在设置里改了提示词文件后，同一会话的
    /// 下一个 turn 会重组 system → 缓存前缀整体失效（约 1.5k token 全量重算）。
    /// 有了它，"改动只对新会话生效"从文档约定升级为代码保证。
    pub system_prompt_cache: Mutex<Option<String>>,
    /// 本会话的**工具开关快照**（用户在设置里启停工具的结果）。
    ///
    /// `None` = 尚未快照；首次要用时从 `app-config.json` 读一次并固化，
    /// 此后本会话所有 turn 一律复用。没有它，用户改完开关后同一会话的下一轮
    /// `tools` 参数就会变 → prompt cache 前缀整体失效（与 `system_prompt_cache` 同理，
    /// 两者守的是同一条约束：请求体前缀必须会话内字节恒定）。
    ///
    /// 清空它 = 让当前会话**立刻**用上最新开关，代价是掉一次 cache ——
    /// 由用户显式点"应用到当前会话"触发，所以那个代价是知情的、可接受的。
    pub tool_filter: Mutex<Option<crate::core::tools::framework::registry::ToolFilter>>,
    /// 本 run 里**等用户做决策**的时间区间：每次弹出权限卡记一段（弹卡 → 拿到结论 / 被打断）。
    ///
    /// 为什么用 `Instant`：单调时钟，不受系统时间被调整的影响；用毫秒时间戳做差会在
    /// 改系统时间的机器上算出负数/巨值。
    ///
    /// 为什么用**同步锁** `StdMutex` 而不是 tokio 的 `Mutex`：区间的关闭点有可能落在
    /// `Drop` 里（future 被取消、子代理任务超时被 drop），`Drop` 是同步上下文，
    /// 拿不到异步锁（`.await` 不可用）。区间只在收尾时被读走，锁竞争极低。
    ///
    /// 为什么存**区间**而不是一个累加标量：并行工具可能同时弹出多张权限卡，
    /// 那几段时间是**重叠**的（用户一次点击同时放行多张卡），按请求累加会把同一段
    /// 时间算两遍。收尾时对这些区间求**并集**才是真实的人类确认耗时
    /// （合并逻辑见 `agent_runs::merged_wait_ms`）。
    pub run_wait_intervals: StdMutex<Vec<(std::time::Instant, std::time::Instant)>>,
}

impl SessionContext {
    pub fn new(id: String) -> Self {
        Self {
            id,
            memory: Mutex::new(SessionMemory::default()),
            active_execution: StdMutex::new(None),
            execution_gate: Mutex::new(()),
            active_run_id: Mutex::new(None),
            todos: Mutex::new(Vec::new()),
            workspace: Mutex::new(None),
            pending_permissions: Mutex::new(HashMap::new()),
            pending_questions: Mutex::new(HashMap::new()),
            pending_patches: Mutex::new(Vec::new()),
            pending_plan_state: Mutex::new(HashMap::new()),
            dedupe_cache: Mutex::new(HashMap::new()),
            agent_audience: Mutex::new("developer".to_string()),
            agent_work_mode: Mutex::new("edit".to_string()),
            thinking_mode: Mutex::new(false),
            turn_think: Mutex::new(None),
            read_file_paths: Mutex::new(Vec::new()),
            read_file_fingerprints: Mutex::new(HashMap::new()),
            loop_continuation_pending: Mutex::new(false),
            tool_result_flags: Mutex::new(HashMap::new()),
            pending_tool_images: Mutex::new(Vec::new()),
            permission_turn: Mutex::new(Default::default()),
            approval_mode: Mutex::new("request_approval".to_string()),
            session_allowances: Mutex::new(Vec::new()),
            agent_read_only: Mutex::new(false),
            system_prompt_cache: Mutex::new(None),
            tool_filter: Mutex::new(None),
            run_wait_intervals: StdMutex::new(Vec::new()),
        }
    }

    /// 记录一段"等待用户决策"的区间（弹卡 → 拿到结论）。
    ///
    /// 同步方法：调用方可能是 `WaitIntervalGuard::drop`（future 被取消、子代理超时、
    /// 进程收尾），那里没有异步运行时，只能走同步锁。
    /// 中毒时取回内部值继续（丢一段统计好过让收尾 panic）。
    pub fn push_wait_interval(&self, start: std::time::Instant, end: std::time::Instant) {
        let mut guard = self
            .run_wait_intervals
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        guard.push((start, end));
    }

    /// 取走本 run 已记录的等待区间并清空，返回**并集**毫秒数。
    ///
    /// "取走"是刻意的：同一次 run 的收尾函数可能被调用多次
    /// （例如 `cancel_run` 落状态之后，finalize 还会盖 `closed` 图章），
    /// 区间若不清空就会被重复计入等待耗时。
    pub async fn take_run_wait_ms(&self) -> u64 {
        let intervals = {
            let mut guard = self
                .run_wait_intervals
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            std::mem::take(&mut *guard)
        }; // 锁在此释放：区间合并是纯计算，没必要占着锁
        crate::core::orchestration::agent_runs::merged_wait_ms(intervals)
    }

    /// 丢弃已累计的等待区间。
    ///
    /// 新一轮 run 启动前调用：区间表挂在**会话**上（`SessionContext`），
    /// 若不清空，上一轮没被收尾取走的残留会被算进本轮（等待耗时虚高）。
    pub async fn clear_run_wait(&self) {
        let mut guard = self
            .run_wait_intervals
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        guard.clear();
    }

    /// 原子登记当前执行；`Err(true)` 表示上一轮正在停止，`Err(false)` 表示仍在运行。
    pub async fn try_start_execution(
        self: &Arc<Self>,
        id: String,
        cancel_token: tokio_util::sync::CancellationToken,
    ) -> Result<ExecutionLease, bool> {
        let _gate = self.execution_gate.lock().await;
        let mut active = self.lock_active_execution();
        if let Some(current) = active.as_ref() {
            return Err(current.cancel_token.is_cancelled());
        }

        let (finished, _) = watch::channel(false);
        *active = Some(ActiveSessionExecution {
            id: id.clone(),
            cancel_token,
            finished,
        });
        Ok(ExecutionLease {
            ctx: self.clone(),
            id,
            released: false,
        })
    }

    /// 只允许本轮执行释放自己的占用，避免旧轮结束时清掉新轮状态。
    pub fn finish_execution(&self, id: &str) -> bool {
        let finished = {
            let mut active = self.lock_active_execution();
            if active.as_ref().map(|execution| execution.id.as_str()) != Some(id) {
                return false;
            }
            active.take().map(|execution| execution.finished)
        };

        if let Some(finished) = finished {
            finished.send_replace(true);
            true
        } else {
            false
        }
    }

    /// 请求停止当前执行并返回其编号；实际释放占用由执行管线收尾负责。
    pub fn request_execution_cancel(&self) -> Option<String> {
        let active = self.lock_active_execution();
        active.as_ref().map(|execution| {
            execution.cancel_token.cancel();
            execution.id.clone()
        })
    }

    /// 等待指定执行完成。若它已经结束或被后续执行替代，不会等待其他轮次。
    pub async fn wait_for_execution(&self, id: &str) {
        let mut finished = {
            let active = self.lock_active_execution();
            match active.as_ref().filter(|execution| execution.id == id) {
                Some(execution) => execution.finished.subscribe(),
                None => return,
            }
        };

        loop {
            if *finished.borrow_and_update() {
                return;
            }
            if finished.changed().await.is_err() {
                return;
            }
        }
    }

    /// 等待当前执行收尾，供方案审批后续跑等必须串行的入口使用。
    pub async fn wait_for_active_execution(&self) {
        let execution_id = self
            .lock_active_execution()
            .as_ref()
            .map(|execution| execution.id.clone());
        if let Some(execution_id) = execution_id {
            self.wait_for_execution(&execution_id).await;
        }
    }

    /// 克隆当前执行的取消令牌，供工具与权限等待逻辑共享本轮取消信号。
    pub fn current_cancel_token(&self) -> Option<tokio_util::sync::CancellationToken> {
        self.lock_active_execution()
            .as_ref()
            .map(|execution| execution.cancel_token.clone())
    }

    /// 查询当前会话是否有执行占用。
    pub fn has_active_execution(&self) -> bool {
        self.lock_active_execution().is_some()
    }

    /// 执行占用锁只保护短小的状态读写；若异常时中毒，取回内部状态继续收尾。
    fn lock_active_execution(&self) -> StdMutexGuard<'_, Option<ActiveSessionExecution>> {
        self.active_execution
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }

    /// 取本会话的工具开关快照；尚未快照时从配置读一次并固化。
    ///
    /// 首次调用通常发生在"本会话第一次准备发消息"那一刻，正好是要的时机：
    /// 此后整个会话复用同一份，`tools` 参数保持字节恒定。
    pub async fn tool_filter(&self) -> crate::core::tools::framework::registry::ToolFilter {
        let mut guard = self.tool_filter.lock().await;
        if let Some(filter) = guard.as_ref() {
            return filter.clone();
        }
        let filter = crate::core::tools::framework::registry::ToolFilter::from_states(
            &crate::infra::config::app_config::get_all_tool_states(),
        );
        *guard = Some(filter.clone());
        filter
    }

    /// 丢弃快照 —— 下一次构建请求时会重新从配置读。
    ///
    /// 这是「应用到当前会话」的实现。**只应由用户显式操作触发**：
    /// 它会改变 `tools` 参数、掉一次 prompt cache；日常轮次绝不能顺手调用。
    pub async fn reset_tool_filter(&self) {
        *self.tool_filter.lock().await = None;
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
    ///
    /// 多把键是否**全部**已被允许（链式命令用：一段一把键，缺一把都不算放行）。
    ///
    /// 空列表返回 false（"没有任何键"不该被当成"全都被覆盖"）——调用方要先判空。
    pub async fn allowance_covers_all(&self, keys: &[(String, String)]) -> bool {
        if keys.is_empty() {
            return false;
        }
        let list = self.session_allowances.lock().await;
        keys.iter().all(|(kind, scope)| {
            if kind == crate::core::tools::framework::policy_guard::ALLOWANCE_KIND_COMMAND {
                // 命令类：范围键是前缀，同名即同权（精确相等，见 command_prefix_scope 的说明）
                list.iter().any(|a| a.kind == *kind && a.scope == *scope)
            } else {
                // 文件类：上级目录的授权覆盖下级（scope_covers）
                list.iter().any(|a| {
                    a.kind == *kind
                        && crate::core::tools::framework::policy_guard::scope_covers(
                            &a.scope, scope,
                        )
                })
            }
        })
    }

    pub async fn allowance_covers(&self, kind: &str, scope: &str) -> bool {
        let file_kind = kind != crate::core::tools::framework::policy_guard::ALLOWANCE_KIND_COMMAND;
        self.session_allowances.lock().await.iter().any(|a| {
            a.kind == kind
                && if file_kind {
                    crate::core::tools::framework::policy_guard::scope_covers(&a.scope, scope)
                } else {
                    a.scope == scope
                }
        })
    }
}

/// 执行生命周期守卫：正常收尾显式释放，异常退出时由 Drop 取消并释放本轮占用。
pub struct ExecutionLease {
    ctx: Arc<SessionContext>,
    id: String,
    released: bool,
}

impl ExecutionLease {
    /// 正常收尾时释放占用；Drop 是异常退出时的兜底。
    pub fn finish(&mut self) {
        if !self.released {
            self.ctx.finish_execution(&self.id);
            self.released = true;
        }
    }
}

impl Drop for ExecutionLease {
    fn drop(&mut self) {
        if self.released {
            return;
        }
        // 异常退出时先广播取消，通知共享此令牌的子任务收束，再释放本轮槽位。
        if let Some(execution) = self
            .ctx
            .lock_active_execution()
            .as_ref()
            .filter(|execution| execution.id == self.id)
        {
            execution.cancel_token.cancel();
        }
        self.ctx.finish_execution(&self.id);
        self.released = true;
    }
}

/// 冷会话同步初始化载荷（在 `spawn_blocking` 阻塞线程池中采集，绝不持全局会话表锁）。
struct SessionBootstrapData {
    memory: Option<SessionMemory>,
    work_mode: String,
    approval_mode: String,
    audience: String,
    workspace: Option<std::path::PathBuf>,
    thinking_mode: bool,
}

/// 在阻塞线程池内同步读取配置、历史消息与会话元数据。
fn load_session_bootstrap(session_id: &str) -> SessionBootstrapData {
    // 1. 新会话及冷会话的默认初始值 = 用户设置里的默认偏好
    let prefs = crate::infra::config::app_config::read_file().ui_preferences;
    let mut work_mode = if prefs.agent_work_mode == "plan" {
        "plan".to_string()
    } else {
        "edit".to_string()
    };
    let mut approval_mode = if prefs.agent_approval_mode == "auto_approve" {
        "auto_approve".to_string()
    } else {
        "request_approval".to_string()
    };
    let mut audience = if prefs.agent_audience == "user" {
        "user".to_string()
    } else {
        "developer".to_string()
    };

    // 2. 从 SQLite 加载历史记忆与元信息
    let memory = crate::core::session::load_session(session_id).ok();
    let meta = crate::core::session::get_session_meta(session_id).ok();
    let bound_to_project = meta
        .as_ref()
        .and_then(|meta| meta.project_id.as_deref())
        .is_some();

    // 3. 会话级运行偏好覆盖：落库非 NULL 则以落库值为准
    if let Ok(runtime_prefs) = crate::core::session::get_session_runtime_prefs(session_id) {
        if let Some(mode) = runtime_prefs.work_mode {
            work_mode = mode;
        }
        if let Some(mode) = runtime_prefs.approval_mode {
            // 「完全批准」要求会话绑定项目；无项目绑定的存量脏数据降级为「请求审批」并回写落库
            if crate::core::tools::framework::policy::mode_requires_project(&mode)
                && !bound_to_project
            {
                let downgraded = ApprovalMode::RequestApproval.as_str();
                approval_mode = downgraded.to_string();
                if let Err(e) =
                    crate::core::session::update_session_approval_mode(session_id, downgraded)
                {
                    eprintln!("[JARVIS] 会话 {} 权限档位降级回写失败：{}", session_id, e);
                }
                println!(
                    "[JARVIS] 会话 {} 恢复时发现「完全批准」但无项目绑定，已降级为「请求审批」",
                    session_id
                );
            } else {
                approval_mode = mode;
            }
        }
        if let Some(aud) = runtime_prefs.agent_audience {
            audience = aud;
        }
    }

    let (workspace, thinking_mode) = if let Some(meta) = meta {
        let ws = meta.working_directory.map(|ws| {
            std::path::PathBuf::from(crate::core::session::strip_extended_path_prefix(&ws))
        });
        (ws, meta.thinking_mode)
    } else {
        (None, false)
    };

    SessionBootstrapData {
        memory,
        work_mode,
        approval_mode,
        audience,
        workspace,
        thinking_mode,
    }
}

/// 会话运行时缓存默认上限（超过后按最近最少使用回收无活跃状态与外部引用的冷会话）
pub const MAX_CACHED_SESSIONS: usize = 32;
/// 空闲会话缓存回收阈值（30 分钟未访问且无执行/待办挂起时自动回收）
pub const SESSION_IDLE_TTL_MS: u64 = 30 * 60 * 1000;

fn now_epoch_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

struct CachedSession {
    ctx: Arc<SessionContext>,
    /// 最近访问毫秒时间戳（原子变量更新，保证读锁热路径零写锁开销）
    last_accessed_ms: AtomicU64,
}

impl CachedSession {
    fn new(ctx: Arc<SessionContext>) -> Self {
        Self {
            ctx,
            last_accessed_ms: AtomicU64::new(now_epoch_ms()),
        }
    }

    fn touch(&self) {
        self.last_accessed_ms
            .store(now_epoch_ms(), Ordering::Relaxed);
    }

    fn last_accessed(&self) -> u64 {
        self.last_accessed_ms.load(Ordering::Relaxed)
    }
}

/// 全局管理器，维护所有活跃会话的 SessionContext。
///
/// 并发与缓存模型：
/// - 热会话命中走 `sessions` 读锁快速路径，原子更新 `last_accessed_ms`，零阻塞；
/// - 冷会话加载按 `session_id` 粒度加 `init_locks` 异步锁，并在 `spawn_blocking` 中完成配置与 DB 读取，
///   彻底避免持全局写锁做 I/O 卡住其他会话；
/// - 仅在最终写入缓存表时短暂获取纳秒级全局写锁，并支持按空闲超时与容量上限（LRU）安全回收冷会话。
pub struct SessionManager {
    sessions: RwLock<HashMap<String, CachedSession>>,
    init_locks: StdMutex<HashMap<String, Arc<Mutex<()>>>>,
}

impl Default for SessionManager {
    fn default() -> Self {
        Self::new()
    }
}

impl SessionManager {
    pub fn new() -> Self {
        Self {
            sessions: RwLock::new(HashMap::new()),
            init_locks: StdMutex::new(HashMap::new()),
        }
    }

    /// 按可选 session_id 过滤返回当前内存中的会话上下文列表（短读锁快照）。
    pub async fn list_contexts(&self, session_id: Option<&str>) -> Vec<Arc<SessionContext>> {
        let sessions = self.sessions.read().await;
        sessions
            .iter()
            .filter(|(sid, _)| session_id.is_none_or(|target| target == sid.as_str()))
            .map(|(_, entry)| entry.ctx.clone())
            .collect()
    }

    /// 退出路径同步采集所有内存态会话记忆快照（仅在 `ExitRequested` 使用）。
    pub fn snapshot_all_memories_blocking(&self) -> Vec<(String, SessionMemory)> {
        let map = self.sessions.blocking_read();
        map.iter()
            .map(|(sid, entry)| {
                let memory = entry.ctx.memory.blocking_lock();
                (sid.clone(), memory.clone())
            })
            .collect()
    }

    /// 判定一个会话上下文是否满足“绝对安全可驱逐”条件：
    /// 必须无外部 `Arc` 持有者、无正在运行的 Agent 执行、无待审批权限/咨询/补丁/续跑挂起。
    fn is_evictable(ctx: &Arc<SessionContext>) -> bool {
        if Arc::strong_count(ctx) > 1 {
            return false;
        }
        if ctx.has_active_execution() {
            return false;
        }
        if ctx.execution_gate.try_lock().is_err() {
            return false;
        }
        if match ctx.active_run_id.try_lock() {
            Ok(guard) => guard.is_some(),
            Err(_) => true,
        } {
            return false;
        }
        if match ctx.pending_permissions.try_lock() {
            Ok(guard) => !guard.is_empty(),
            Err(_) => true,
        } {
            return false;
        }
        if match ctx.pending_questions.try_lock() {
            Ok(guard) => !guard.is_empty(),
            Err(_) => true,
        } {
            return false;
        }
        if match ctx.pending_patches.try_lock() {
            Ok(guard) => !guard.is_empty(),
            Err(_) => true,
        } {
            return false;
        }
        if match ctx.loop_continuation_pending.try_lock() {
            Ok(guard) => *guard,
            Err(_) => true,
        } {
            return false;
        }
        true
    }

    /// 在持有 `sessions` 写锁时执行空闲超期回收与容量上限 LRU 淘汰，返回回收数量。
    fn evict_locked(
        sessions: &mut HashMap<String, CachedSession>,
        now_ms: u64,
        max_idle_ms: u64,
        max_entries: usize,
        protect_session_id: Option<&str>,
    ) -> usize {
        let mut evicted = 0usize;

        // 阶段一：回收空闲超期且安全可驱逐的冷会话
        let expired_keys: Vec<String> = sessions
            .iter()
            .filter(|(sid, entry)| {
                protect_session_id != Some(sid.as_str())
                    && Self::is_evictable(&entry.ctx)
                    && now_ms.saturating_sub(entry.last_accessed()) >= max_idle_ms
            })
            .map(|(sid, _)| sid.clone())
            .collect();

        for sid in expired_keys {
            if sessions.remove(&sid).is_some() {
                evicted += 1;
            }
        }

        // 阶段二：若总数仍超过上限，按最近最少使用（LRU）淘汰可驱逐的冷会话
        if sessions.len() > max_entries {
            let need_evict = sessions.len() - max_entries;
            let mut candidates: Vec<(u64, String)> = sessions
                .iter()
                .filter(|(sid, entry)| {
                    protect_session_id != Some(sid.as_str()) && Self::is_evictable(&entry.ctx)
                })
                .map(|(sid, entry)| (entry.last_accessed(), sid.clone()))
                .collect();
            candidates.sort_by(|a, b| a.0.cmp(&b.0).then_with(|| a.1.cmp(&b.1)));

            for (_, sid) in candidates.into_iter().take(need_evict) {
                if sessions.remove(&sid).is_some() {
                    evicted += 1;
                }
            }
        }

        evicted
    }

    /// 回收空闲超时或超过容量上限的冷会话上下文（活跃、外部引用中或有挂起状态的会话绝不驱逐）。
    pub async fn evict_idle_sessions(&self, max_idle_ms: u64, max_entries: usize) -> usize {
        let mut sessions = self.sessions.write().await;
        Self::evict_locked(
            &mut sessions,
            now_epoch_ms(),
            max_idle_ms,
            max_entries,
            None,
        )
    }

    /// 按默认策略（30 分钟空闲 / 最多 32 个缓存会话）执行一次空闲回收。
    pub async fn evict_default_idle_sessions(&self) -> usize {
        self.evict_idle_sessions(SESSION_IDLE_TTL_MS, MAX_CACHED_SESSIONS)
            .await
    }

    /// 获取指定会话的专属初始化异步锁（不存在则创建）。
    fn acquire_init_lock(&self, session_id: &str) -> Arc<Mutex<()>> {
        let mut locks = self
            .init_locks
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        locks
            .entry(session_id.to_string())
            .or_insert_with(|| Arc::new(Mutex::new(())))
            .clone()
    }

    /// 当且仅当没有其他并发任务持有同一把 `init_lock` 时清理表项，防止 `init_locks` 长期累积。
    fn cleanup_init_lock(&self, session_id: &str, lock: &Arc<Mutex<()>>) {
        let mut locks = self
            .init_locks
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        if locks
            .get(session_id)
            .is_some_and(|existing| Arc::ptr_eq(existing, lock) && Arc::strong_count(existing) == 2)
        {
            locks.remove(session_id);
        }
    }

    /// 移除指定会话缓存。若该会话正好处于并发冷加载中，先等待加载完毕再移除，避免幽灵条目回写。
    pub async fn remove(&self, session_id: &str) -> Option<Arc<SessionContext>> {
        let maybe_lock = {
            let locks = self
                .init_locks
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            locks.get(session_id).cloned()
        };
        let removed = if let Some(lock) = maybe_lock {
            let guard = lock.lock().await;
            let val = self
                .sessions
                .write()
                .await
                .remove(session_id)
                .map(|entry| entry.ctx);
            drop(guard);
            self.cleanup_init_lock(session_id, &lock);
            val
        } else {
            self.sessions
                .write()
                .await
                .remove(session_id)
                .map(|entry| entry.ctx)
        };
        removed
    }

    /// 通用按会话细粒度加锁加载入口（便于单测注入自定义异步 loader 验证并发隔离）。
    async fn get_or_create_with<F, Fut>(&self, session_id: &str, loader: F) -> Arc<SessionContext>
    where
        F: FnOnce(String) -> Fut,
        Fut: std::future::Future<Output = Arc<SessionContext>>,
    {
        // 快速路径：热会话命中直接走短读锁并原子刷新访问时间，不碰写锁和初始化锁
        {
            let read_guard = self.sessions.read().await;
            if let Some(entry) = read_guard.get(session_id) {
                entry.touch();
                return entry.ctx.clone();
            }
        }

        // 慢速路径：按 session_id 粒度加初始化锁（同会话串行防重复加载，跨会话完全并行）
        let init_lock = self.acquire_init_lock(session_id);
        let init_guard = init_lock.lock().await;

        // 双检（Double-Check）：排在前面的同会话并发请求可能已经加载完毕
        {
            let read_guard = self.sessions.read().await;
            if let Some(entry) = read_guard.get(session_id) {
                entry.touch();
                let ctx = entry.ctx.clone();
                drop(read_guard);
                drop(init_guard);
                self.cleanup_init_lock(session_id, &init_lock);
                return ctx;
            }
        }

        // 锁外执行全部重 I/O（绝不阻塞全局 sessions 读写锁）
        let built = loader(session_id.to_string()).await;

        // 纳秒级短写锁插入，并在超限时顺手执行轻量 LRU 回收
        let arc_ctx = {
            let mut write_guard = self.sessions.write().await;
            let entry = write_guard
                .entry(session_id.to_string())
                .or_insert_with(|| CachedSession::new(built));
            entry.touch();
            let ctx = entry.ctx.clone();
            if write_guard.len() > MAX_CACHED_SESSIONS {
                Self::evict_locked(
                    &mut write_guard,
                    now_epoch_ms(),
                    SESSION_IDLE_TTL_MS,
                    MAX_CACHED_SESSIONS,
                    Some(session_id),
                );
            }
            ctx
        };

        drop(init_guard);
        self.cleanup_init_lock(session_id, &init_lock);
        arc_ctx
    }

    pub async fn get_or_create(&self, session_id: &str) -> Arc<SessionContext> {
        self.get_or_create_with(session_id, |sid| async move {
            let sid_for_blocking = sid.clone();
            let bootstrap =
                tokio::task::spawn_blocking(move || load_session_bootstrap(&sid_for_blocking))
                    .await
                    .unwrap_or_else(|err| {
                        eprintln!(
                            "[SessionManager] spawn_blocking 加载会话 {} 失败: {}",
                            sid, err
                        );
                        SessionBootstrapData {
                            memory: None,
                            work_mode: "edit".to_string(),
                            approval_mode: "request_approval".to_string(),
                            audience: "developer".to_string(),
                            workspace: None,
                            thinking_mode: false,
                        }
                    });

            let ctx = SessionContext::new(sid);
            *ctx.agent_work_mode.lock().await = bootstrap.work_mode;
            *ctx.approval_mode.lock().await = bootstrap.approval_mode;
            *ctx.agent_audience.lock().await = bootstrap.audience;
            if let Some(memory) = bootstrap.memory {
                *ctx.memory.lock().await = memory;
            }
            *ctx.workspace.lock().await = bootstrap.workspace.clone();
            *ctx.thinking_mode.lock().await = bootstrap.thinking_mode;

            // 挂载项目后把该项目的授权账本注入内存（用户此前点过的「本项目允许」自动生效）
            if let Some(root) = bootstrap.workspace {
                crate::core::tools::framework::allowance_store::load_into(&ctx, &root).await;
            }

            Arc::new(ctx)
        })
        .await
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

#[cfg(test)]
mod session_manager_concurrency_tests {
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::time::{Duration, Instant};

    /// 同一冷会话高并发访问时，loader 只执行一次且返回同一实例，完成后 init_locks 自动清理
    #[tokio::test]
    async fn concurrent_cold_session_loads_once_and_cleans_init_lock() {
        let manager = Arc::new(SessionManager::new());
        let load_count = Arc::new(AtomicUsize::new(0));

        let mut handles = Vec::new();
        for _ in 0..8 {
            let mgr = Arc::clone(&manager);
            let counter = Arc::clone(&load_count);
            handles.push(tokio::spawn(async move {
                mgr.get_or_create_with("cold_s1", |sid| async move {
                    counter.fetch_add(1, Ordering::SeqCst);
                    tokio::time::sleep(Duration::from_millis(60)).await;
                    Arc::new(SessionContext::new(sid))
                })
                .await
            }));
        }

        let mut ptrs = Vec::new();
        for h in handles {
            ptrs.push(h.await.expect("并发任务不应 panic"));
        }

        assert_eq!(
            load_count.load(Ordering::SeqCst),
            1,
            "同一冷会话只应加载一次"
        );
        for ctx in &ptrs[1..] {
            assert!(
                Arc::ptr_eq(&ptrs[0], ctx),
                "并发请求应共享同一 SessionContext 实例"
            );
        }
        assert!(
            manager.init_locks.lock().unwrap().is_empty(),
            "加载完成后应清理无竞争的 init_locks 条目"
        );
    }

    /// 一个慢速冷会话在加载期间，绝不阻塞热会话命中和其他冷会话并行加载
    #[tokio::test]
    async fn slow_cold_session_does_not_block_hot_session() {
        let manager = Arc::new(SessionManager::new());

        // 预热热会话 hot_s1
        let hot = manager
            .get_or_create_with(
                "hot_s1",
                |sid| async move { Arc::new(SessionContext::new(sid)) },
            )
            .await;

        // 启动一个耗时 180ms 的慢速冷会话加载
        let mgr_slow = Arc::clone(&manager);
        let slow_task = tokio::spawn(async move {
            mgr_slow
                .get_or_create_with("slow_cold", |sid| async move {
                    tokio::time::sleep(Duration::from_millis(180)).await;
                    Arc::new(SessionContext::new(sid))
                })
                .await
        });

        // 稍等确保 slow_cold 已进入 loader 阶段
        tokio::time::sleep(Duration::from_millis(20)).await;

        // 此时读取热会话必须瞬间返回（远小于 180ms）
        let started = Instant::now();
        let got = manager.get_or_create("hot_s1").await;
        let elapsed = started.elapsed();

        assert!(Arc::ptr_eq(&hot, &got));
        assert!(
            elapsed < Duration::from_millis(60),
            "慢速冷会话不得阻塞热会话读取，实际耗时: {:?}",
            elapsed
        );

        let _ = slow_task.await.expect("慢速加载不应 panic");
    }

    /// 空闲会话回收绝不淘汰活跃执行中或外部仍持有引用的会话，但会按 LRU 回收冷会话
    #[tokio::test]
    async fn evict_idle_sessions_preserves_active_and_referenced_reclaims_cold_lru() {
        let manager = SessionManager::new();
        let now = now_epoch_ms();

        let s1 = manager
            .get_or_create_with(
                "s1",
                |sid| async move { Arc::new(SessionContext::new(sid)) },
            )
            .await;
        let held_s2 = manager
            .get_or_create_with(
                "s2",
                |sid| async move { Arc::new(SessionContext::new(sid)) },
            )
            .await;
        let s3 = manager
            .get_or_create_with(
                "s3",
                |sid| async move { Arc::new(SessionContext::new(sid)) },
            )
            .await;
        let s4 = manager
            .get_or_create_with(
                "s4",
                |sid| async move { Arc::new(SessionContext::new(sid)) },
            )
            .await;

        // s3 模拟运行态：带有 active_run_id
        *s3.active_run_id.lock().await = Some("run-active-1".to_string());

        // 释放 s1/s3/s4 外部引用，只保留 held_s2 外部引用
        drop(s1);
        drop(s3);
        drop(s4);

        {
            let guard = manager.sessions.read().await;
            guard["s1"]
                .last_accessed_ms
                .store(now.saturating_sub(20_000), Ordering::Relaxed);
            guard["s2"]
                .last_accessed_ms
                .store(now.saturating_sub(18_000), Ordering::Relaxed);
            guard["s3"]
                .last_accessed_ms
                .store(now.saturating_sub(15_000), Ordering::Relaxed);
            guard["s4"].last_accessed_ms.store(now, Ordering::Relaxed);
        }

        // TTL=60s，容量上限=3：共 4 条记录需淘汰 1 条；s2（外部强引用 > 1）与 s3（active_run_id = Some）均受保护，
        // 在可驱逐的 s1/s4 中按 LRU 淘汰最旧的 s1，保留 s2、s3 与最新的 s4
        let evicted = manager.evict_idle_sessions(60_000, 3).await;
        assert_eq!(evicted, 1, "只应淘汰最旧的无保护冷会话 s1");
        {
            let guard = manager.sessions.read().await;
            assert!(!guard.contains_key("s1"), "冷会话 s1 应被 LRU 淘汰");
            assert!(guard.contains_key("s2"), "外部引用的 s2 绝不可淘汰");
            assert!(guard.contains_key("s3"), "含活跃运行 ID 的 s3 绝不可淘汰");
            assert!(guard.contains_key("s4"), "最新访问的 s4 应保留");
        }

        // 即使 s2(18s) 与 s3(15s) 都超过 TTL=10s，因仍处于受保护状态，TTL 回收也必须返回 0
        let evicted_protected = manager.evict_idle_sessions(10_000, 32).await;
        assert_eq!(
            evicted_protected, 0,
            "外部引用中或运行中的会话即使超过 TTL 也绝不可被回收"
        );

        // 释放 held_s2 且清空 s3 活跃运行状态后，按容量上限=1 应淘汰 s2 与 s3，仅保留最新的 s4
        drop(held_s2);
        {
            let guard = manager.sessions.read().await;
            *guard["s3"].ctx.active_run_id.lock().await = None;
        }
        let evicted_second = manager.evict_idle_sessions(60_000, 1).await;
        assert_eq!(evicted_second, 2);
        {
            let guard = manager.sessions.read().await;
            assert_eq!(guard.len(), 1);
            assert!(guard.contains_key("s4"));
        }
    }

    /// 超过空闲 TTL 且无活跃挂起的会话会被回收，未超期会话保留
    #[tokio::test]
    async fn evict_idle_sessions_respects_ttl_expiration() {
        let manager = SessionManager::new();
        let now = now_epoch_ms();

        let _old = manager
            .get_or_create_with("ttl_old", |sid| async move {
                Arc::new(SessionContext::new(sid))
            })
            .await;
        let _fresh = manager
            .get_or_create_with("ttl_fresh", |sid| async move {
                Arc::new(SessionContext::new(sid))
            })
            .await;
        drop(_old);
        drop(_fresh);

        {
            let guard = manager.sessions.read().await;
            guard["ttl_old"]
                .last_accessed_ms
                .store(now.saturating_sub(10_000), Ordering::Relaxed);
            guard["ttl_fresh"]
                .last_accessed_ms
                .store(now, Ordering::Relaxed);
        }

        let evicted = manager.evict_idle_sessions(5_000, 32).await;
        assert_eq!(evicted, 1);
        let guard = manager.sessions.read().await;
        assert!(!guard.contains_key("ttl_old"));
        assert!(guard.contains_key("ttl_fresh"));
    }
}
