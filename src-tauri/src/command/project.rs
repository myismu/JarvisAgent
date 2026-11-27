//! # project.rs — 项目管理与工作目录 Tauri 命令
//!
//! 承载项目打开（含归档项目自动复活）、项目列表、项目归档、重命名及会话工作目录查询。
//!
//! ## Key Exports
//! - `open_project()` / `list_projects()` / `archive_project()` / `rename_project()`: 项目 CRUD 与归档命令
//! - `get_workspace_dir()`: 查询会话绑定的工作目录沙箱路径
//!
//! ## Dependencies
//! - Internal: `crate::core::session`, `crate::core::session::lifecycle::switch_away`, `crate::core::runtime::SessionManager`
//! - External: `tauri`

use crate::core::runtime::SessionManager;
use crate::core::session;
use crate::core::session::lifecycle::switch_away;
use crate::infra::types::error::CommandError;
use tauri::Emitter;

#[tauri::command]
pub async fn open_project(path: String) -> Result<session::ProjectMeta, CommandError> {
    let normalized = std::path::Path::new(&path);
    if !normalized.exists() || !normalized.is_dir() {
        return Err(CommandError::invalid_input(format!(
            "目录不存在或不是文件夹: {}",
            path
        )));
    }
    let abs_canonical = normalized
        .canonicalize()
        .map_err(|e| CommandError::invalid_input(format!("解析路径失败: {}", e)))?
        .to_string_lossy()
        .to_string();
    // 剥离 \\?\ 前缀后再入库：展示美观，且与用户输入的普通路径一致
    let abs = session::strip_extended_path_prefix(&abs_canonical).to_string();

    // 已存在（含已归档项目）则直接复用并自动复活，绝不重复 INSERT 产生孤儿会话
    if let Some(project) = find_project_by_path(&abs, &abs_canonical)? {
        return revive_project_if_archived(project).map_err(CommandError::from);
    }

    let name = std::path::Path::new(&abs)
        .file_name()
        .map(|n| n.to_string_lossy().to_string())
        .unwrap_or_else(|| abs.clone());

    session::repository::create_project(&name, &abs).map_err(CommandError::from)
}

#[tauri::command]
pub async fn list_projects() -> Result<Vec<session::ProjectMeta>, CommandError> {
    tokio::task::spawn_blocking(session::repository::list_projects)
        .await
        .map_err(|e| CommandError::internal(format!("查询项目列表后台任务异常: {}", e)))?
        .map_err(CommandError::from)
}

/// 按「普通路径优先、含 `\\?\` 前缀的存量路径兜底」的顺序查项目。
fn find_project_by_path(
    abs: &str,
    abs_canonical: &str,
) -> Result<Option<session::ProjectMeta>, String> {
    if let Some(project) = session::repository::get_project_by_path(abs)? {
        return Ok(Some(project));
    }
    if abs != abs_canonical {
        return session::repository::get_project_by_path(abs_canonical);
    }
    Ok(None)
}

/// 打开项目时，如果该项目处于**归档**状态，先取消归档并重读真实 `session_count` 再返回。
fn revive_project_if_archived(
    project: session::ProjectMeta,
) -> Result<session::ProjectMeta, String> {
    if !session::repository::is_project_archived(&project.id)? {
        return Ok(project);
    }
    session::repository::restore_project(&project.id)?;
    session::repository::get_project(&project.id)?.ok_or_else(|| "项目不存在".to_string())
}

/// 归档项目（软删）：项目本身 + 名下所有未归档会话，在同一事务里打上同一时间戳。
#[tauri::command]
pub async fn archive_project(
    id: String,
    session_manager: tauri::State<'_, SessionManager>,
    app: tauri::AppHandle,
) -> Result<u64, CommandError> {
    // 必须在归档之前记下当前活动会话与其所属项目，归档后常规读路径查不到已归档会话
    let last_active_id = session::get_last_active_session_id();
    let last_active_project = last_active_id
        .as_deref()
        .and_then(|sid| session::get_session_meta(sid).ok())
        .and_then(|meta| meta.project_id);

    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as u64;
    let archived_sessions = session::archive_project(&id, now)?;

    let _ = app.emit("session-updated", ());

    if let (Some(sid), Some(pid)) = (last_active_id, last_active_project) {
        if pid == id {
            session_manager.remove(&sid).await;
            switch_away(&sid, &app).await?;
        }
    }

    Ok(archived_sessions)
}

#[tauri::command]
pub async fn rename_project(
    id: String,
    name: String,
) -> Result<session::ProjectMeta, CommandError> {
    session::repository::rename_project(&id, &name).map_err(CommandError::from)
}

#[tauri::command]
pub async fn get_workspace_dir(
    session_id: String,
    session_manager: tauri::State<'_, SessionManager>,
) -> Result<Option<String>, CommandError> {
    let ctx = session_manager.get_or_create(&session_id).await;
    let ws = ctx.workspace.lock().await.clone();
    if let Some(path) = ws {
        Ok(Some(path.to_string_lossy().to_string()))
    } else {
        // 非沙箱会话返回 None，避免前端或 Agent 误以为存在沙箱限制
        Ok(None)
    }
}
