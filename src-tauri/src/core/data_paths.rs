//! Runtime data layout helpers.
//!
//! Persistent session data lives in SQLite (`jarvis.sqlite3`). This module only
//! creates non-session runtime directories such as global config, logs, cache,
//! and temporary workspaces.

use crate::core::constants;
use crate::get_agent_home;
use std::fs;
use std::path::PathBuf;

pub const DATA_LAYOUT_VERSION: u32 = 2;

pub const DIR_GLOBAL: &str = "global";
pub const DIR_LOGS: &str = "logs";
pub const DIR_CACHE: &str = "cache";
pub const DIR_TMP: &str = "tmp";

pub fn data_root() -> PathBuf {
    get_agent_home().clone()
}

pub fn global_dir() -> PathBuf {
    ensure_dir(data_root().join(DIR_GLOBAL))
}

pub fn logs_dir() -> PathBuf {
    ensure_dir(data_root().join(DIR_LOGS))
}

pub fn cache_dir() -> PathBuf {
    ensure_dir(data_root().join(DIR_CACHE))
}

pub fn tmp_dir() -> PathBuf {
    ensure_dir(data_root().join(DIR_TMP))
}

pub fn config_path() -> PathBuf {
    global_dir().join(constants::FILE_CONFIG)
}

pub fn sqlite_db_path() -> PathBuf {
    data_root().join("jarvis.sqlite3")
}

pub fn workspace_file_path() -> PathBuf {
    global_dir().join(constants::FILE_WORKSPACE)
}

pub fn global_memory_path() -> PathBuf {
    global_dir().join(constants::FILE_GLOBAL_MEMORY)
}

pub fn project_memory_path() -> PathBuf {
    ensure_dir(global_dir().join("memory")).join("GEMINI.md")
}

pub fn ensure_base_layout() {
    ensure_dir(data_root());
    ensure_dir(global_dir());
    ensure_dir(logs_dir());
    ensure_dir(cache_dir());
    ensure_dir(tmp_dir());
}

pub fn ensure_dir(path: PathBuf) -> PathBuf {
    let _ = fs::create_dir_all(&path);
    path
}

pub fn refresh_session_manifest(
    session_id: &str,
    title: Option<String>,
    created_at: Option<u64>,
    updated_at: Option<u64>,
) {
    let _ = (&session_id, &title, &created_at, &updated_at);
}
