//! # subagent_budget.rs — 子代理执行预算判据（无进展超时 / 硬上限）
//!
//! 子代理此前只有一个"整体超时"概念，且阈值 480 秒既当无进展判据又当整体上限用：
//! 一条正常的慢命令（`cargo build` 编译大工程 600 秒）会被判死，而真正卡死的
//! 空转却要等到 480 秒才被发现。本模块把这两件事拆开：
//!
//! - **无进展（idle）**：连续多久没有收到任何"链路仍在流动"的信号（SSE 帧、
//!   工具结果返回等）。只用于"等模型 / 流量静默"这类可能真失联的阶段。
//! - **硬上限（hard）**：整个子代理从启动到现在的总时长上限，只用于防真死循环。
//!
//! ## 设计要点
//! - [`judge`] 是**纯函数、无 IO、无时钟读取**：入参全是秒数，结论直接可断言，
//!   因此单测不需要 sleep（参考 `core::agent::stream::waiting_hint_secs` 的派生思路）。
//! - [`SubagentBudget`] 负责持有时间锚点；`last_progress_at` 私有，
//!   [`SubagentBudget::touch`] 是**唯一**能推进它的入口，杜绝外部偷偷改字段导致
//!   "永不过期"或"刚启动就过期"这类无法排查的错。
//! - 工具执行阶段**不参与 idle 判定**（见 [`judge_phase_aware`] 的注释）。
//!
//! ## 关于"进展信号"来自哪里（探查结论，2026-11-23）
//!
//! `StreamConfig.on_frame`（`core::agent::stream`）的触发时机是：`process_stream`
//! 的事件循环里，每成功解析出**一个 SSE 事件**后、在解析 payload 之前**无条件**
//! 调用一次（`stream.rs` 中 `if let Some(on_frame) = &config.on_frame { on_frame(); }`，
//! 该语句位于 `received_event = true` 之后、`parse_frame` 之前）。也就是说
//! **它确实是逐帧触发的**，而不是"只在文本增量时触发"——即使该帧是 ping、
//! 是空 delta、或是解析不出内容的未知事件，回调也照样执行。
//!
//! 因此它可以直接充当子代理的"有进展"信号源，**无需另造一套 activity 机制**。
//! 现状是 `subagent.rs` 约 957 行传的是 `on_frame: None`（理由是子代理有自己的
//! 监控面板，不需要主聊天流那种"仍在等待"提示），于是子代理侧目前**没有任何人
//! 观察进展**——这正是本模块要补的缺口：把 `on_frame` 指向
//! [`activity_callback`] 包出来的回调，预算对象就能逐帧被 `touch()`。
//!
//! ## 依赖
//! - Internal: `crate::infra::types::constants`
//! - External: 无（只用 `std`）

use std::sync::{Arc, Mutex};
use std::time::Instant;

use crate::infra::types::constants::{SUBAGENT_HARD_TIMEOUT_SECS, SUBAGENT_IDLE_TIMEOUT_SECS};

/// 预算判据的结论。
///
/// 只有 [`BudgetVerdict::Running`] 表示可以继续跑；另两种都是**终止**信号，
/// 但语义不同，调用方据此决定报错文案与监控面板上的阶段文案：
/// - `IdleTimeout`：链路静默太久，通常意味着流已死（"无进展"）。
/// - `HardTimeout`：总时长到顶，通常意味着陷入循环或反复重试（"整体超时"）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BudgetVerdict {
    /// 尚未越线，继续执行。
    Running,
    /// 无进展超时：连续 `idle_secs` 秒没有收到任何进展信号。
    IdleTimeout {
        /// 触发时的静默秒数（用于报错文案与日志，便于事后核对）。
        idle_secs: u64,
    },
    /// 硬上限超时：从启动至今已 `elapsed_secs` 秒。
    HardTimeout {
        /// 触发时的总耗时秒数。
        elapsed_secs: u64,
    },
}

/// 预算所处的阶段 —— **只区分一件事实：当前是否在跑工具**。
///
/// 刻意不用 `orchestration::subagents::SubAgentPhase`（那是给 UI 展示的 7 态枚举，
/// 带 serde）：预算判据只需要"跑工具 / 不在跑工具"这一个二值信息，
/// 且本模块要保持不依赖编排层，纯到可以单独断言。
///
/// 映射关系（供后续接线时对照）：
/// - `SubAgentPhase::WaitingModel` / `Streaming` / `Thinking` / `Starting`
///   → [`BudgetPhase::AwaitingModel`]
/// - `SubAgentPhase::CallingTool` / `ProcessingToolResult` → [`BudgetPhase::RunningTool`]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BudgetPhase {
    /// 等模型响应 / 流式生成中：链路上任何"零字节"都可能是真失联，受 idle 判据约束。
    AwaitingModel,
    /// 工具执行中：由工具自身的 timeout（≤ 600 秒）兜底，**不受 idle 判据约束**。
    RunningTool,
}

/// 预算判据的**唯一**事实源：纯函数，不读时钟、不做 IO。
///
/// ## 语义（顺序与边界都很重要）
///
/// 1. **先查硬上限**：总时长超过 `hard_limit` 一律判 `HardTimeout`。
///    即使同时满足 idle 条件，也报硬上限——它描述的是"整体失控"这一更根本的原因。
/// 2. **再查无进展**：静默超过 `idle_limit` 判 `IdleTimeout`。
/// 3. 两者都要**严格大于**阈值才终止：秒数正好等于阈值时仍返回 `Running`。
///    这是为了给"刚好卡在整秒边界"留出信息，也避免测试里出现 `>=` / `>` 的歧义
///    （历史上 480 秒就是在这种模糊语义里既当 idle 又当 hard 用）。
///
/// 判据不关心"现在几点"，只比较三个秒数，因此调用方可以注入任意时间点做单测。
pub fn judge(elapsed_secs: u64, idle_secs: u64, idle_limit: u64, hard_limit: u64) -> BudgetVerdict {
    // 硬上限优先：它代表整体失控，比"这一段时间没动静"更根本。
    if elapsed_secs > hard_limit {
        return BudgetVerdict::HardTimeout { elapsed_secs };
    }
    // 无进展判据：连续静默超过阈值。
    if idle_secs > idle_limit {
        return BudgetVerdict::IdleTimeout { idle_secs };
    }
    BudgetVerdict::Running
}

/// 阶段感知版判据：**工具执行阶段只查硬上限，不查 idle**。
///
/// ## 为什么工具阶段必须豁免 idle（本设计最容易做错的一点）
///
/// idle 判据回答的是"链路是不是死了"。工具执行时，链路本来就没有数据：
/// 一条 `cargo build` 编译大工程 600 秒，期间零 SSE 帧、零进展回调都是**正常的**。
/// 若让 180 秒的 idle 判据笼罩工具执行，这条命令照样会被误杀——
/// 等于把"480 秒一刀切"的老问题原样搬进新机制，只是把误杀阈值从 480 降到 180。
///
/// 工具本身已有自己的 timeout 兜底（`RunCommand` 等 ≤ 600 秒），它比任何外部
/// 猜测都更清楚"这个操作该多久"。因此这里只保留硬上限（防死循环），
/// 把"多久算超时"的决定权交还给工具自己。
///
/// 实现上就是在工具阶段把 `idle_secs` 视作 0 再交给 [`judge`]：
/// 0 永远大于不了正的 `idle_limit`，于是 idle 分支不可能命中，而硬上限仍照常生效。
/// 这样两条路径共用同一个纯函数，不存在"两份判据语义漂移"的风险。
pub fn judge_phase_aware(
    phase: BudgetPhase,
    elapsed_secs: u64,
    idle_secs: u64,
    idle_limit: u64,
    hard_limit: u64,
) -> BudgetVerdict {
    match phase {
        // 工具执行中：把"无进展"这一输入掐掉（置 0），只留硬上限。
        BudgetPhase::RunningTool => judge(elapsed_secs, 0, idle_limit, hard_limit),
        BudgetPhase::AwaitingModel => judge(elapsed_secs, idle_secs, idle_limit, hard_limit),
    }
}

/// 一个子代理的预算状态：两个时间锚点 + 两条阈值。
///
/// 字段全部私有：调用方只能通过方法读时间/阈值、通过 [`SubagentBudget::touch`]
/// 推进进展锚点。**不要**提供 `&mut` 字段访问的 setter——一旦允许外部直接改
/// `last_progress_at`，就等于允许"让超时永不触发"，而这类 bug 在线上表现为
/// "子代理卡了半小时也不退出"，极难定位。
#[derive(Debug, Clone)]
pub struct SubagentBudget {
    /// 启动时刻（用于计算总耗时 / 硬上限）。创建后不再变化。
    started_at: Instant,
    /// 最近一次"有进展"的时刻。**只能**由 [`SubagentBudget::touch`] 推进。
    last_progress_at: Instant,
    /// 无进展阈值（秒）。
    idle_limit: u64,
    /// 硬上限（秒）。
    hard_limit: u64,
}

impl SubagentBudget {
    /// 用当前时刻作启动时刻创建预算。
    pub fn new(idle_limit: u64, hard_limit: u64) -> Self {
        Self::with_started_at(Instant::now(), idle_limit, hard_limit)
    }

    /// 以**指定**启动时刻创建预算。
    ///
    /// 生产代码用 [`SubagentBudget::new`]；这里显式暴露 `started_at` 是为了单测能
    /// 直接构造"已经跑了 200 秒"的状态，从而在不 sleep 的前提下断言判据，
    /// 也便于崩溃重建时用真实的"本轮开始时间"恢复计时。
    pub fn with_started_at(started_at: Instant, idle_limit: u64, hard_limit: u64) -> Self {
        Self {
            started_at,
            // 启动即视为一次进展：否则"启动后立刻长时间排队"会被误判成静默。
            last_progress_at: started_at,
            idle_limit,
            hard_limit,
        }
    }

    /// 用 `constants` 里的默认阈值创建预算（idle 180s / hard 3600s）。
    pub fn from_defaults() -> Self {
        Self::new(SUBAGENT_IDLE_TIMEOUT_SECS, SUBAGENT_HARD_TIMEOUT_SECS)
    }

    /// 记录一次"有进展"。**这是唯一能推进 `last_progress_at` 的方法。**
    ///
    /// 调用点应当是真实的进展证据：收到一个 SSE 帧（`StreamConfig.on_frame`）、
    /// 一个工具返回、一段文本落地。不要拿"心跳定时器"冒充进展——
    /// 那样 idle 判据等于失效。
    pub fn touch(&mut self) {
        self.last_progress_at = Instant::now();
    }

    /// 启动时刻。
    pub fn started_at(&self) -> Instant {
        self.started_at
    }

    /// 最近一次进展时刻（只读，用于调试/监控展示）。
    pub fn last_progress_at(&self) -> Instant {
        self.last_progress_at
    }

    /// 无进展阈值（秒）。
    pub fn idle_limit(&self) -> u64 {
        self.idle_limit
    }

    /// 硬上限阈值（秒）。
    pub fn hard_limit(&self) -> u64 {
        self.hard_limit
    }

    /// 按当前时钟求判据（默认视为"等模型"阶段）。
    pub fn verdict(&self) -> BudgetVerdict {
        self.verdict_in_phase(BudgetPhase::AwaitingModel)
    }

    /// 按当前时钟 + 指定阶段求判据。
    pub fn verdict_in_phase(&self, phase: BudgetPhase) -> BudgetVerdict {
        self.judge_at(Instant::now(), phase)
    }

    /// 以**指定**的"现在"求判据：单测用这个注入时间点，避免真的等待。
    ///
    /// 注意 `now` 早于启动时刻时，`duration_since` 会 panic —— 这属于调用方错误
    /// （传了错误的时间源），让它早点炸出来比静默算出 0 更好。
    pub fn judge_at(&self, now: Instant, phase: BudgetPhase) -> BudgetVerdict {
        let elapsed_secs = now.duration_since(self.started_at).as_secs();
        let idle_secs = now.duration_since(self.last_progress_at).as_secs();
        judge_phase_aware(
            phase,
            elapsed_secs,
            idle_secs,
            self.idle_limit,
            self.hard_limit,
        )
    }
}

/// 共享预算：子代理主循环持有它算判据，SSE 流打帧时回调它做 `touch()`。
pub type SharedSubagentBudget = Arc<Mutex<SubagentBudget>>;

/// 把共享预算包成可直接塞进 `StreamConfig.on_frame` 的轻量回调。
///
/// ## 为什么需要它
///
/// `StreamConfig.on_frame` 是 `Option<Arc<dyn Fn() + Send + Sync>>`（见
/// `core::agent::stream`）：它按**每个 SSE 事件**触发一次，正是我们要的进展信号。
/// 但 `touch()` 需要 `&mut SubagentBudget`，而回调是无参 `Fn()`，
/// 因此这里用 `Arc<Mutex<_>>` 做桥接：
///
/// ```ignore
/// // 子代理侧（后续接线）：
/// let budget = Arc::new(Mutex::new(SubagentBudget::from_defaults()));
/// let on_frame = subagent_budget::activity_callback(budget.clone());
/// // ... StreamConfig { on_frame: Some(on_frame), .. } // 取代原来的 None
/// ```
///
/// 锁只在回调内部持有极短时间（一次字段赋值），且**不跨 await**，
/// 与 tokio 的调度没有交互风险；即使模型吐帧再快，也只是一次无竞争的赋值。
pub fn activity_callback(budget: SharedSubagentBudget) -> Arc<dyn Fn() + Send + Sync> {
    Arc::new(move || {
        // 单帧 touch 失败只可能是别处持锁时 panic（锁中毒）。此处不做恢复：
        // 预算判据失效不该静默吞掉，宁可让后面的 `lock()` 显式报错。
        if let Ok(mut guard) = budget.lock() {
            guard.touch();
        }
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    /// 无进展命中：静默超过 idle 阈值即终止，并带回实际静默秒数。
    #[test]
    fn idle_timeout_is_hit() {
        let verdict = judge(100, 181, SUBAGENT_IDLE_TIMEOUT_SECS, SUBAGENT_HARD_TIMEOUT_SECS);
        assert_eq!(verdict, BudgetVerdict::IdleTimeout { idle_secs: 181 });
    }

    /// 硬上限命中：总时长超过 hard 阈值即终止（即使刚刚才收到过进展）。
    #[test]
    fn hard_timeout_is_hit() {
        let verdict = judge(3601, 1, SUBAGENT_IDLE_TIMEOUT_SECS, SUBAGENT_HARD_TIMEOUT_SECS);
        assert_eq!(
            verdict,
            BudgetVerdict::HardTimeout {
                elapsed_secs: 3601
            }
        );
    }

    /// 两条都越线时报硬上限：它描述的是"整体失控"这一更根本的原因。
    #[test]
    fn hard_timeout_wins_when_both_exceeded() {
        let verdict = judge(4000, 4000, SUBAGENT_IDLE_TIMEOUT_SECS, SUBAGENT_HARD_TIMEOUT_SECS);
        assert_eq!(
            verdict,
            BudgetVerdict::HardTimeout {
                elapsed_secs: 4000
            }
        );
    }

    /// 边界：秒数**正好等于**阈值时不终止（判据用严格大于）。
    #[test]
    fn equal_to_threshold_keeps_running() {
        // 恰好等于 idle 阈值
        assert_eq!(
            judge(0, SUBAGENT_IDLE_TIMEOUT_SECS, SUBAGENT_IDLE_TIMEOUT_SECS, SUBAGENT_HARD_TIMEOUT_SECS),
            BudgetVerdict::Running,
            "idle 等于阈值不应终止"
        );
        // 恰好等于 hard 阈值
        assert_eq!(
            judge(SUBAGENT_HARD_TIMEOUT_SECS, 0, SUBAGENT_IDLE_TIMEOUT_SECS, SUBAGENT_HARD_TIMEOUT_SECS),
            BudgetVerdict::Running,
            "elapsed 等于阈值不应终止"
        );
        // 两条都恰好等于阈值
        assert_eq!(
            judge(
                SUBAGENT_HARD_TIMEOUT_SECS,
                SUBAGENT_IDLE_TIMEOUT_SECS,
                SUBAGENT_IDLE_TIMEOUT_SECS,
                SUBAGENT_HARD_TIMEOUT_SECS
            ),
            BudgetVerdict::Running
        );
        // 阈值 +1 立刻终止（确认边界两侧行为都正确）
        assert_eq!(
            judge(0, SUBAGENT_IDLE_TIMEOUT_SECS + 1, SUBAGENT_IDLE_TIMEOUT_SECS, SUBAGENT_HARD_TIMEOUT_SECS),
            BudgetVerdict::IdleTimeout {
                idle_secs: SUBAGENT_IDLE_TIMEOUT_SECS + 1
            }
        );
        assert_eq!(
            judge(SUBAGENT_HARD_TIMEOUT_SECS + 1, 0, SUBAGENT_IDLE_TIMEOUT_SECS, SUBAGENT_HARD_TIMEOUT_SECS),
            BudgetVerdict::HardTimeout {
                elapsed_secs: SUBAGENT_HARD_TIMEOUT_SECS + 1
            }
        );
    }

    /// **touch 后续命**：静默到越线 → `touch()` 立刻把 idle 归零，判据回到 Running。
    /// 硬上限不受 touch 影响（它看的是 started_at）。
    #[test]
    fn touch_resets_idle_but_not_elapsed() {
        let started = Instant::now();
        let mut budget = SubagentBudget::with_started_at(
            started,
            SUBAGENT_IDLE_TIMEOUT_SECS,
            SUBAGENT_HARD_TIMEOUT_SECS,
        );

        // 跑了 200 秒且期间毫无进展 → 无进展超时
        let t200 = started + Duration::from_secs(200);
        assert_eq!(
            budget.judge_at(t200, BudgetPhase::AwaitingModel),
            BudgetVerdict::IdleTimeout { idle_secs: 200 }
        );

        // 来了一次真实进展 → idle 归零，Running（总耗时仍是 200s，远未到硬上限）
        budget.touch();
        assert_eq!(
            budget.judge_at(Instant::now(), BudgetPhase::AwaitingModel),
            BudgetVerdict::Running
        );

        // 硬上限依旧按 started_at 计：从最近进展点再往后超过 3600 秒，
        // 总耗时必然 > 3600（多 1 秒是为了避开"恰好等于阈值不终止"的边界）
        let t_hard = budget.last_progress_at() + Duration::from_secs(SUBAGENT_HARD_TIMEOUT_SECS + 1);
        assert!(matches!(
            budget.judge_at(t_hard, BudgetPhase::AwaitingModel),
            BudgetVerdict::HardTimeout { .. }
        ));
    }

    /// **工具执行阶段不参与 idle 判定**：静默再久也不终止，但硬上限仍然生效。
    ///
    /// 场景：`cargo build` 编译 600 秒，期间没有任何 SSE 帧。
    /// 旧实现（480 秒一刀切）会误杀；这里必须放行。
    #[test]
    fn tool_phase_ignores_idle_but_keeps_hard_limit() {
        // 静默 9999 秒，远超 idle 阈值 → 工具阶段仍然 Running
        assert_eq!(
            judge_phase_aware(
                BudgetPhase::RunningTool,
                600,
                9999,
                SUBAGENT_IDLE_TIMEOUT_SECS,
                SUBAGENT_HARD_TIMEOUT_SECS
            ),
            BudgetVerdict::Running,
            "工具执行阶段的静默不应按无进展处理"
        );

        // 同样的输入在等模型阶段必须终止 —— 证明豁免是"阶段"带来的，不是判据失效
        assert_eq!(
            judge_phase_aware(
                BudgetPhase::AwaitingModel,
                600,
                9999,
                SUBAGENT_IDLE_TIMEOUT_SECS,
                SUBAGENT_HARD_TIMEOUT_SECS
            ),
            BudgetVerdict::IdleTimeout { idle_secs: 9999 }
        );

        // 工具阶段硬上限照常生效
        assert_eq!(
            judge_phase_aware(
                BudgetPhase::RunningTool,
                SUBAGENT_HARD_TIMEOUT_SECS + 1,
                0,
                SUBAGENT_IDLE_TIMEOUT_SECS,
                SUBAGENT_HARD_TIMEOUT_SECS
            ),
            BudgetVerdict::HardTimeout {
                elapsed_secs: SUBAGENT_HARD_TIMEOUT_SECS + 1
            }
        );
    }

    /// 阈值来自具名常量，且 idle 必须严格小于 hard（否则整套拆分失去意义）。
    #[test]
    fn defaults_come_from_named_constants() {
        let budget = SubagentBudget::from_defaults();
        assert_eq!(budget.idle_limit(), SUBAGENT_IDLE_TIMEOUT_SECS);
        assert_eq!(budget.hard_limit(), SUBAGENT_HARD_TIMEOUT_SECS);
        const {
            assert!(
                SUBAGENT_IDLE_TIMEOUT_SECS < SUBAGENT_HARD_TIMEOUT_SECS,
                "idle 必须严格小于 hard"
            );
        }
        // 新创建的预算两条锚点都等于启动时刻：启动瞬间必定 Running
        assert_eq!(budget.started_at(), budget.last_progress_at());
        assert_eq!(budget.verdict(), BudgetVerdict::Running);
    }

    /// 逐帧回调就是"推进进展"的接线方式：触发一次后 idle 归零。
    #[test]
    fn activity_callback_counts_as_progress() {
        let started = Instant::now() - Duration::from_secs(200);
        let budget = Arc::new(Mutex::new(SubagentBudget::with_started_at(
            started,
            SUBAGENT_IDLE_TIMEOUT_SECS,
            SUBAGENT_HARD_TIMEOUT_SECS,
        )));

        // 回调前：200 秒无进展 → 已越线
        assert!(matches!(
            budget.lock().unwrap().verdict(),
            BudgetVerdict::IdleTimeout { .. }
        ));

        // 模拟 stream.rs 收到一帧后调用 on_frame
        let on_frame = activity_callback(budget.clone());
        on_frame();

        // 回调后：idle 归零，Running
        assert_eq!(budget.lock().unwrap().verdict(), BudgetVerdict::Running);
    }
}
