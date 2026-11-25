//! # repository/projects.rs — 项目元数据仓储
//!
//! 封装 `projects` 表的创建、查询、路径匹配、归档判定与重命名操作。
//!
//! ## Key Exports
//! - `create_project()` / `get_project()` / `get_project_by_path()` / `list_projects()`: 项目 CRUD
//! - `get_project_path()` / `is_project_archived()` / `rename_project()`: 项目属性查询与更新
//!
//! ## Dependencies
//! - Internal: `crate::core::session`, `crate::infra::db`
//! - External: `rusqlite`

use rusqlite::{OptionalExtension, Row};
// ── Project operations ──

/// `ProjectMeta` 的 SELECT 列清单（两处查询共用一份；与 `project_meta_from_row` 读的列名对应）。
///
/// 与 `SESSION_MESSAGE_COLUMNS` / `SESSION_META_COLUMNS` 同一套做法。
pub(super) const PROJECT_META_COLUMNS: &str = "p.id, p.name, p.path, p.created_at, p.updated_at, \
     (SELECT COUNT(*) FROM sessions s WHERE s.project_id = p.id AND s.deleted_at IS NULL) as session_count";

pub fn get_project_path(project_id: &str) -> Result<Option<String>, String> {
    crate::infra::db::with_connection(|conn| {
        conn.query_row(
            "SELECT path FROM projects WHERE id = ?1",
            [project_id],
            |row| row.get::<_, String>(0),
        )
        .optional()
        .map_err(|e| e.to_string())
    })
}

/// 按 id 读单个项目元数据。
///
/// 与 [`get_project_by_path`] 共用同一份列清单与行映射，保证「重命名后返回的对象」
/// 与列表页拿到的字段口径完全一致（含 `session_count`）。
pub fn get_project(id: &str) -> Result<Option<crate::core::session::ProjectMeta>, String> {
    crate::infra::db::with_connection(|conn| {
        conn.query_row(
            &format!(
                "SELECT {} FROM projects p WHERE p.id = ?1",
                PROJECT_META_COLUMNS
            ),
            [id],
            project_meta_from_row,
        )
        .optional()
        .map_err(|e| e.to_string())
    })
}

pub fn get_project_by_path(
    path: &str,
) -> Result<Option<crate::core::session::ProjectMeta>, String> {
    crate::infra::db::with_connection(|conn| {
        conn.query_row(
            &format!(
                "SELECT {} FROM projects p WHERE p.path = ?1",
                PROJECT_META_COLUMNS
            ),
            [path],
            project_meta_from_row,
        )
        .optional()
        .map_err(|e| e.to_string())
    })
}

pub fn create_project(name: &str, path: &str) -> Result<crate::core::session::ProjectMeta, String> {
    let id = uuid::Uuid::new_v4().to_string()[..8].to_string();
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as i64;
    crate::infra::db::with_connection(|conn| {
        conn.execute(
            "INSERT INTO projects (id, name, path, created_at, updated_at) VALUES (?1, ?2, ?3, ?4, ?4)",
            rusqlite::params![id, name, path, now],
        )
        .map_err(|e| e.to_string())?;
        Ok(crate::core::session::ProjectMeta {
            id,
            name: name.to_string(),
            path: path.to_string(),
            created_at: now as u64,
            updated_at: now as u64,
            session_count: 0,
        })
    })
}

pub fn list_projects() -> Result<Vec<crate::core::session::ProjectMeta>, String> {
    crate::infra::db::with_connection(list_projects_in)
}

/// [`list_projects`] 的 connection 版（测试直接跑真实 SQL）。
///
/// 只列**未归档**项目：归档是“从界面移除但可撤销”，归档项统一在「归档」区展示。
/// 注意 `PROJECT_META_COLUMNS` 里的 `session_count` 子查询自己带了会话级过滤
/// （`s.deleted_at IS NULL`），不要把它改成跟着项目过滤——两者口径不同：
/// 一个未归档项目里可能全是已归档会话，此时 session_count 就是 0。
pub(super) fn list_projects_in(
    conn: &rusqlite::Connection,
) -> Result<Vec<crate::core::session::ProjectMeta>, String> {
    let mut stmt = conn
        .prepare(&format!(
            "SELECT {} FROM projects p
             WHERE p.deleted_at IS NULL
             ORDER BY p.updated_at DESC",
            PROJECT_META_COLUMNS
        ))
        .map_err(|e| e.to_string())?;
    let rows = stmt
        .query_map([], project_meta_from_row)
        .map_err(|e| e.to_string())?;
    let mut projects = Vec::new();
    for row in rows {
        projects.push(row.map_err(|e| e.to_string())?);
    }
    Ok(projects)
}

/// 项目是否处于**归档**状态（`projects.deleted_at IS NOT NULL`）。
///
/// 为什么单开一个查询，而不是把 `deleted_at` 加进 [`crate::core::session::ProjectMeta`]：
/// `PROJECT_META_COLUMNS` / `project_meta_from_row` 是列表页、单条查询与
/// `meta_columns_tests` 三方共用的一份列清单，而「打开项目时要不要复活」只有
/// `command::session::open_project` 这一条路径需要判断。为一条路径扩大公共契约，
/// 会把归档细节漏进所有读取项目的地方，收益不抵风险。
///
/// 项目不存在时返回 `false`（不是错误）：调用方已通过 `get_project_by_path` 拿到记录，
/// 这里只回答「要不要复活」，查不到就按未归档处理即可。
pub fn is_project_archived(project_id: &str) -> Result<bool, String> {
    crate::infra::db::with_connection(|conn| {
        let archived: Option<i64> = conn
            .query_row(
                "SELECT deleted_at FROM projects WHERE id = ?1",
                [project_id],
                |row| row.get::<_, Option<i64>>(0),
            )
            .optional()
            .map_err(|e| e.to_string())?
            .flatten();
        Ok(archived.is_some())
    })
}

/// 重命名项目：**只改展示名，不移动磁盘目录**。
///
/// 为什么只改 `name` 而不碰 `path`：
/// - 会话沙箱的工作目录是由 `projects.path` 派生出来的（见 `get_project_path`），
///   路径一改就等于把已有会话的工作根换掉，历史消息里的相对路径、工具调用记录
///   全都会指向旧位置，语义被悄悄改写；
/// - 快照 / 回收站里存的是按会话与文件路径拼出来的引用，移动目录会让这些引用
///   变成悬空指针，且没有迁移机制能把它们重新对上；
/// - 用户改名的真实诉求只是「列表里看着顺眼」，与磁盘上的目录名无关。
///
/// 同理这里**不更新 `updated_at`**：重命名不是实际工作进展，不该让项目在
/// 「按最近活跃排序」的列表里跳到最前面。只动 `name` 一列。
pub fn rename_project(id: &str, name: &str) -> Result<crate::core::session::ProjectMeta, String> {
    let trimmed = name.trim();
    if trimmed.is_empty() {
        return Err("项目名不能为空".into());
    }
    if trimmed.chars().count() > 40 {
        return Err("项目名不能超过 40 个字符".into());
    }
    crate::infra::db::with_connection(|conn| {
        let changed = conn
            .execute(
                "UPDATE projects SET name = ?2 WHERE id = ?1",
                rusqlite::params![id, trimmed],
            )
            .map_err(|e| e.to_string())?;
        if changed == 0 {
            return Err("项目不存在".into());
        }
        Ok(())
    })?;
    // 读回首元数据（含 session_count），口径与列表页一致；get_project 自己再拿一次连接，
    // 所以必须等上面的 with_connection 闭包结束之后再调，避免重入。
    get_project(id)?.ok_or_else(|| "项目不存在".into())
}

/// 从 `projects` 的一行还原项目元数据（**按列名取值**，口径同上）。
pub(super) fn project_meta_from_row(row: &Row<'_>) -> rusqlite::Result<crate::core::session::ProjectMeta> {
    Ok(crate::core::session::ProjectMeta {
        id: row.get("id")?,
        name: row.get("name")?,
        path: row.get("path")?,
        created_at: row.get::<_, i64>("created_at")? as u64,
        updated_at: row.get::<_, i64>("updated_at")? as u64,
        session_count: row.get::<_, i64>("session_count")? as usize,
    })
}
