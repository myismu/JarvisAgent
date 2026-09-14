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
pub const MAX_TOKENS_COMPACT_TRIGGER: usize = 100_000;
pub const MAX_AGENT_LOOP_BEFORE_CONFIRM: usize = 30;
pub const MAX_AGENT_LOOP_ABSOLUTE: usize = 200;
pub const PLAN_WATCHDOG_MAX_CONSECUTIVE_STALLS: usize = 6;
pub const PLAN_WATCHDOG_MAX_LOOPS_WITHOUT_PLAN: usize = 10;
pub const MAX_SESSION_TITLE_LEN: usize = 30;
pub const MAX_BACKGROUND_OUTPUT_LEN: usize = 50000;
pub const MAX_BACKGROUND_NOTIFY_LEN: usize = 500;
/// 上下文快照里保留的"逐 loop 缓存命中"记录条数（用于渲染趋势曲线）
pub const CACHE_HISTORY_MAX_POINTS: usize = 12;
pub const COMPACT_KEEP_RECENT_MESSAGES: usize = 6;
pub const SUBAGENT_TIMEOUT_SECS: u64 = 480;
