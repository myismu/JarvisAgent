//! # constants.rs — 常量定义模块
//!
//! 集中定义目录名、文件名、限制阈值等常量。确保整个应用使用一致的命名和限制。
//!
//! ## 关键导出
//! - 目录常量: `DIR_IMAGES`, `DIR_TASKS`, `DIR_LOGS` 等
//! - 文件常量: `FILE_WORKSPACE`, `FILE_CONFIG`, `FILE_GLOBAL_MEMORY` 等
//! - 限制常量: `MAX_TOKENS_CONTEXT`, `MAX_AGENT_LOOP_BEFORE_CONFIRM`, `MAX_SESSION_TITLE_LEN` 等
//!
//! ## 依赖
//! - 无外部依赖
//!
//! ## 约束
//! - 所有常量均为 `pub const`，可直接访问
//! - 修改限制常量可能影响系统稳定性和性能

// --- Directory Names ---
pub const DIR_IMAGES: &str = "images";
pub const DIR_TASKS: &str = "tasks";
pub const DIR_LOGS: &str = "logs";
pub const DIR_PLANS: &str = "plans";
pub const DIR_AGENT_RUNS: &str = "agent_runs";
pub const DIR_SKILLS: &str = "skills";
pub const DIR_TRANSCRIPTS: &str = "transcripts";

// --- File Names ---
pub const FILE_WORKSPACE: &str = ".jarvis_workspace";
pub const FILE_CONFIG: &str = "config.json";
pub const FILE_GLOBAL_MEMORY: &str = "global_memory.md";
pub const FILE_LAST_ACTIVE_SESSION: &str = "_last_active.txt";

// --- Limits & Thresholds ---
pub const MAX_TOKENS_CONTEXT: i32 = 8192;
// 上下文压缩的触发阈值已不再是一个固定常量：它 =（模型窗口 − 输出预算）× 85%，
// 见 `infra::llm::context_budget`（主 Agent 与子代理共用）。
pub const MAX_AGENT_LOOP_BEFORE_CONFIRM: usize = 30;
pub const MAX_AGENT_LOOP_ABSOLUTE: usize = 200;
pub const PLAN_WATCHDOG_MAX_CONSECUTIVE_STALLS: usize = 6;
pub const PLAN_WATCHDOG_MAX_LOOPS_WITHOUT_PLAN: usize = 10;
pub const MAX_SESSION_TITLE_LEN: usize = 30;
pub const MAX_BACKGROUND_OUTPUT_LEN: usize = 50000;
pub const MAX_BACKGROUND_NOTIFY_LEN: usize = 500;
/// 上下文快照里保留的"逐 loop 缓存命中"记录条数（用于渲染趋势曲线）
pub const CACHE_HISTORY_MAX_POINTS: usize = 12;
/// 手动压缩的**消息条数下限**：少于这个条数不值得压（压了等于没压）。
///
/// ⚠️ 2026-09-17 改名 + 改语义。旧名 `COMPACT_KEEP_RECENT_MESSAGES` 兼管两件事：
/// ① 自动压缩后保留几条第尾（**已撤销** —— 现在"不留尾巴"，压缩后只剩 2 条）；
/// ② 手动压缩低于几条直接拒绝（**保留** —— 就是本常量现在的唯一含义）。
/// 前端 `src/utils/contextUsage.ts` 有一份同值镜像，改一侧必须同步另一侧。
pub const COMPACT_MIN_MESSAGES: usize = 6;
pub const SUBAGENT_TIMEOUT_SECS: u64 = 480;

// --- HTTP 超时（分层，覆盖不同的沉默形态，详见 doc/流式请求超时与中断落库机制方案.md 第 11 节）---

/// ① TCP 建连超时：对方端口不通／网络不可达时快速失败。
///
/// **与重试退避的关系（重要）**：这就是用户感知的"读秒 15 秒"。
/// 请求发出后若对端毫无反应，最迟在本超时点失败，随即进入
/// `RETRY_BACKOFF_SECS = [1, 3, 5]` 的三次重试。
/// 因此"首次尝试最多等 15s，之后 1+3+5=9s 内重试三次"，
/// 总时间上限约 24 秒（不含各次重试自身的建连耗时）。
///
/// 注意：这**不是**响应头超时（②，120s）——真实模型在思考阶段可能长时间
/// 不吐首字节，属正常；建连阶段 15 秒无反应才是异常。
pub const HTTP_CONNECT_TIMEOUT_SECS: u64 = 15;

/// ④ TCP keepalive 探测间隔：连接空闲多久后开始发探测包。
///
/// 注意它**不是**客户端超时——探测用于让内核发现"半开连接"（对方进程已死、
/// 中间设备静默丢包），检测完成还需依赖 OS 的探测次数与间隔，通常是分钟级。
/// 因此它只是最后一道防线，流式请求的主力兜底是 `stream.rs` 的流内空闲超时。
pub const HTTP_TCP_KEEPALIVE_SECS: u64 = 30;

/// ② 响应头超时（流式）：请求已发出 → 收到响应头。
///
/// 覆盖"对方连上了但迟迟不吐响应头"。注意它**只到响应头为止**，
/// 之后由 `stream.rs` 的流内空闲超时接管。
pub const API_RESPONSE_HEADER_TIMEOUT_SECS: u64 = 120;

/// ③ 非流式辅助调用的整体超时：请求 → 完整响应体。
///
/// 适用于标题生成、消息压缩、全局记忆整理、反思审查这类"顺手做的事"。
/// 取值较短是有意的：这些调用失败不影响主流程（都有兜底），
/// 但若长时间挂起会拖住调用它的命令，用户界面会莫名卡住。
pub const API_NON_STREAM_TIMEOUT_SECS: u64 = 60;

/// 等待上游响应时，超过该秒数先在界面给一句"仍在等待"的提示。
///
/// 纯粹是交互体验：深度思考／网络迟滞时可能长时间无任何事件，
/// 界面只转圈会让用户无法区分"模型在思考"与"服务已经死了"。
/// 它不改变任何超时判定，真正的失联由空闲超时 / 响应头超时收尾。
pub const API_WAITING_HINT_SECS: u64 = 30;

/// 断流后"自动重试一次"的整体预算（秒），**覆盖请求头 + 读流全过程**。
///
/// 取值刻意很短：首次已经等满了一个空闲周期（90s），而上游刚静默 90s 的情况下
/// 立刻恢复的概率本来就低。与其让用户再等一个完整周期，不如让重试**快速失败**，
/// 尽早把决策权交还用户（服务恢复后一句"继续"即可接上）。
pub const API_STREAM_RETRY_BUDGET_SECS: u64 = 15;
