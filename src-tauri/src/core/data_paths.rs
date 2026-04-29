//! Runtime data layout helpers.
//!
//! New user-owned runtime data is grouped by owner:
//! - global data: `<data>/global`
//! - session data: `<data>/sessions/<session_id>`
//! - app logs: `<data>/logs`

use crate::core::constants;
use crate::get_agent_home;
use serde::{Deserialize, Serialize};
use std::collections::BTreeSet;
use std::fs;
use std::path::{Path, PathBuf};

pub const DATA_LAYOUT_VERSION: u32 = 1;

pub const DIR_GLOBAL: &str = "global";
pub const DIR_SESSIONS: &str = "sessions";
pub const DIR_LOGS: &str = "logs";
pub const DIR_CACHE: &str = "cache";
pub const DIR_TMP: &str = "tmp";

pub const DIR_ATTACHMENTS: &str = "attachments";
pub const DIR_AGENT_RUNS: &str = "agent_runs";
pub const DIR_CHECKPOINTS: &str = "checkpoints";
pub const DIR_IMAGES: &str = "images";
pub const DIR_PLANS: &str = "plans";
pub const DIR_SNAPSHOTS: &str = "snapshots";
pub const DIR_TASKS: &str = "tasks";
pub const DIR_TRANSCRIPTS: &str = "transcripts";

pub const FILE_SESSION: &str = "session.json";
pub const FILE_MANIFEST: &str = "manifest.json";

#[derive(Clone, Debug)]
pub struct SessionPaths {
    pub session_id: String,
    pub root: PathBuf,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SessionManifest {
    pub schema_version: u32,
    pub session_id: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub created_at: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub updated_at: Option<u64>,
    pub resources: SessionResources,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SessionResources {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub session: Option<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub transcripts: Vec<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub agent_runs: Vec<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub snapshots: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub checkpoints: Option<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub images: Vec<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub plans: Vec<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub tasks: Vec<String>,
}

impl SessionPaths {
    pub fn new(session_id: &str) -> Self {
        Self {
            session_id: session_id.to_string(),
            root: sessions_dir().join(safe_component(session_id)),
        }
    }

    pub fn ensure_root(&self) -> PathBuf {
        ensure_dir(self.root.clone());
        self.root.clone()
    }

    pub fn session_file(&self) -> PathBuf {
        self.ensure_root().join(FILE_SESSION)
    }

    pub fn manifest_file(&self) -> PathBuf {
        self.ensure_root().join(FILE_MANIFEST)
    }

    pub fn transcripts_dir(&self) -> PathBuf {
        ensure_dir(self.ensure_root().join(DIR_TRANSCRIPTS))
    }

    pub fn agent_runs_dir(&self) -> PathBuf {
        ensure_dir(self.ensure_root().join(DIR_AGENT_RUNS))
    }

    pub fn agent_run_dir(&self, run_id: &str) -> PathBuf {
        ensure_dir(self.agent_runs_dir().join(safe_component(run_id)))
    }

    pub fn snapshots_dir(&self) -> PathBuf {
        ensure_dir(self.ensure_root().join(DIR_SNAPSHOTS))
    }

    pub fn checkpoints_dir(&self) -> PathBuf {
        ensure_dir(self.ensure_root().join(DIR_CHECKPOINTS))
    }

    pub fn attachments_dir(&self) -> PathBuf {
        ensure_dir(self.ensure_root().join(DIR_ATTACHMENTS))
    }

    pub fn images_dir(&self) -> PathBuf {
        ensure_dir(self.attachments_dir().join(DIR_IMAGES))
    }

    pub fn plans_dir(&self) -> PathBuf {
        ensure_dir(self.ensure_root().join(DIR_PLANS))
    }

    pub fn tasks_dir(&self) -> PathBuf {
        ensure_dir(self.ensure_root().join(DIR_TASKS))
    }
}

pub fn session_paths(session_id: &str) -> SessionPaths {
    SessionPaths::new(session_id)
}

pub fn data_root() -> PathBuf {
    get_agent_home().clone()
}

pub fn sessions_dir() -> PathBuf {
    ensure_dir(data_root().join(DIR_SESSIONS))
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

pub fn workspace_file_path() -> PathBuf {
    global_dir().join(constants::FILE_WORKSPACE)
}

pub fn last_active_session_path() -> PathBuf {
    global_dir().join(constants::FILE_LAST_ACTIVE_SESSION)
}

pub fn global_memory_path() -> PathBuf {
    global_dir().join(constants::FILE_GLOBAL_MEMORY)
}

pub fn project_memory_path() -> PathBuf {
    ensure_dir(global_dir().join("memory")).join("GEMINI.md")
}

pub fn global_tasks_dir() -> PathBuf {
    ensure_dir(global_dir().join(DIR_TASKS))
}

pub fn ensure_base_layout() {
    ensure_dir(data_root());
    ensure_dir(global_dir());
    ensure_dir(sessions_dir());
    ensure_dir(logs_dir());
    ensure_dir(cache_dir());
    ensure_dir(tmp_dir());
}

pub fn ensure_dir(path: PathBuf) -> PathBuf {
    let _ = fs::create_dir_all(&path);
    path
}

pub fn session_ids_from_storage() -> BTreeSet<String> {
    let mut ids = BTreeSet::new();

    if let Ok(entries) = fs::read_dir(sessions_dir()) {
        for entry in entries.flatten() {
            let path = entry.path();
            if path.is_dir() && path.join(FILE_SESSION).exists() {
                if let Some(name) = path.file_name().and_then(|value| value.to_str()) {
                    ids.insert(name.to_string());
                }
            }
        }
    }

    ids
}

pub fn refresh_session_manifest(
    session_id: &str,
    title: Option<String>,
    created_at: Option<u64>,
    updated_at: Option<u64>,
) {
    let paths = session_paths(session_id);
    let root = paths.ensure_root();
    let resources = SessionResources {
        session: file_resource(&root, &root.join(FILE_SESSION)),
        transcripts: list_relative_files(&root, &root.join(DIR_TRANSCRIPTS)),
        agent_runs: list_relative_dirs(&root, &root.join(DIR_AGENT_RUNS)),
        snapshots: dir_resource(&root, &root.join(DIR_SNAPSHOTS)),
        checkpoints: dir_resource(&root, &root.join(DIR_CHECKPOINTS)),
        images: list_relative_files(&root, &root.join(DIR_ATTACHMENTS).join(DIR_IMAGES)),
        plans: list_relative_files(&root, &root.join(DIR_PLANS)),
        tasks: list_relative_files(&root, &root.join(DIR_TASKS)),
    };
    let manifest = SessionManifest {
        schema_version: DATA_LAYOUT_VERSION,
        session_id: session_id.to_string(),
        title,
        created_at,
        updated_at,
        resources,
    };
    if let Ok(json) = serde_json::to_string_pretty(&manifest) {
        let _ = fs::write(paths.manifest_file(), json);
    }
}

pub fn session_id_from_prefixed_filename(filename: &str) -> Option<String> {
    filename
        .split_once('_')
        .map(|(session_id, _)| session_id)
        .filter(|session_id| !session_id.is_empty())
        .map(|session_id| session_id.to_string())
}

fn safe_component(value: &str) -> String {
    let safe: String = value
        .chars()
        .map(|ch| {
            if ch.is_ascii_alphanumeric() || ch == '-' || ch == '_' {
                ch
            } else {
                '_'
            }
        })
        .collect();
    if safe.is_empty() {
        "unknown".to_string()
    } else {
        safe
    }
}

fn file_resource(root: &Path, file: &Path) -> Option<String> {
    if file.exists() {
        Some(relative_path(root, file))
    } else {
        None
    }
}

fn dir_resource(root: &Path, dir: &Path) -> Option<String> {
    if dir.exists() && has_entries(dir) {
        Some(relative_path(root, dir))
    } else {
        None
    }
}

fn has_entries(dir: &Path) -> bool {
    fs::read_dir(dir)
        .map(|mut entries| entries.next().is_some())
        .unwrap_or(false)
}

fn list_relative_files(root: &Path, dir: &Path) -> Vec<String> {
    let mut files = Vec::new();
    collect_relative_files(root, dir, &mut files);
    files.sort();
    files
}

fn collect_relative_files(root: &Path, dir: &Path, files: &mut Vec<String>) {
    let Ok(entries) = fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            collect_relative_files(root, &path, files);
        } else {
            files.push(relative_path(root, &path));
        }
    }
}

fn list_relative_dirs(root: &Path, dir: &Path) -> Vec<String> {
    let mut dirs = Vec::new();
    if let Ok(entries) = fs::read_dir(dir) {
        for entry in entries.flatten() {
            let path = entry.path();
            if path.is_dir() {
                dirs.push(relative_path(root, &path));
            }
        }
    }
    dirs.sort();
    dirs
}

fn relative_path(root: &Path, path: &Path) -> String {
    path.strip_prefix(root)
        .unwrap_or(path)
        .to_string_lossy()
        .replace('\\', "/")
}
