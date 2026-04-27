// --- Constants Module ---
// Centralized definitions for directory names, file names, limits, and thresholds.

// --- Directory Names ---
pub const DIR_SESSIONS: &str = ".sessions";
pub const DIR_IMAGES: &str = ".images";
pub const DIR_TASKS: &str = ".tasks";
pub const DIR_LOGS: &str = ".logs";
pub const DIR_PLANS: &str = ".plans";
pub const DIR_AGENT_RUNS: &str = ".agent_runs";
pub const DIR_SKILLS: &str = "skills";
pub const DIR_TRANSCRIPTS: &str = ".transcripts";

// --- File Names ---
pub const FILE_WORKSPACE: &str = ".jarvis_workspace";
pub const FILE_CONFIG: &str = "config.json";
pub const FILE_GLOBAL_MEMORY: &str = "global_memory.md";
pub const FILE_LAST_ACTIVE_SESSION: &str = "_last_active.txt";
pub const FILE_AGENT_LOOP_DEBUG: &str = "agent_loop_debug.txt";
pub const FILE_THOUGHTS_LOG: &str = "thoughts_and_plans.md";

// --- Limits & Thresholds ---
pub const MAX_TOKENS_CONTEXT: i32 = 8192;
pub const MAX_TOKENS_COMPACT_TRIGGER: usize = 50000;
pub const MAX_AGENT_LOOP_BEFORE_CONFIRM: usize = 30;
pub const MAX_AGENT_LOOP_ABSOLUTE: usize = 500;
pub const MAX_SESSION_TITLE_LEN: usize = 30;
pub const MAX_BACKGROUND_OUTPUT_LEN: usize = 50000;
pub const MAX_BACKGROUND_NOTIFY_LEN: usize = 500;
