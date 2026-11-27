//! # repository/sessions.rs — 会话元数据、回收站与运行偏好仓储
//!
//! 封装 `sessions` 与 `session_memory` 的元信息读写、软删除/恢复/批量清理及会话级运行偏好持久化。
//!
//! ## Key Exports
//! - `upsert_session()`: 保存会话元数据和记忆
//! - `get_session_meta()` / `list_sessions()`: 查询活跃会话元数据
//! - `delete_session()` / `restore_session()` / `restore_sessions()` / `purge_sessions()`: 归档与回收站操作
//! - `archive_project()` / `restore_project()`: 项目级归档与恢复
//! - `SessionRuntimePrefs` / `get_session_runtime_prefs()`: 会话级运行偏好读写
//! - `get_last_active_session_id()` / `set_last_active_session_id()`: 最后活跃会话状态
//!
//! ## Dependencies
//! - Internal: `crate::core::session`, `crate::infra::db`, `crate::infra::types::models`
//! - External: `rusqlite`, `serde_json`
use rusqlite::{params, OptionalExtension, Row};

use crate::core::session::{BatchOpFailure, BatchOpResult, SessionMeta};
use crate::infra::types::models::SessionMemory;
pub fn upsert_session(meta: &SessionMeta, memory: &SessionMemory) -> Result<(), String> {
    crate::infra::db::with_transaction(|tx| {
        tx.execute(
            "INSERT INTO sessions(
                id, title, created_at, updated_at, message_count, is_smart_named,
                profile_id, total_input_tokens, total_output_tokens,
                total_cache_hit_tokens, total_cache_miss_tokens,
                title_source, project_id, thinking_mode, deleted_at
            ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, NULL)
            ON CONFLICT(id) DO UPDATE SET
                title = excluded.title,
                created_at = excluded.created_at,
                updated_at = excluded.updated_at,
                message_count = excluded.message_count,
                is_smart_named = excluded.is_smart_named,
                profile_id = excluded.profile_id,
                total_input_tokens = excluded.total_input_tokens,
                total_output_tokens = excluded.total_output_tokens,
                total_cache_hit_tokens = excluded.total_cache_hit_tokens,
                total_cache_miss_tokens = excluded.total_cache_miss_tokens,
                title_source = excluded.title_source,
                project_id = excluded.project_id,
                thinking_mode = excluded.thinking_mode",
            // 注意：这里**故意不重置 `deleted_at`**（2026-09-21 沐拍板）。
            // 曾经的写法是 `deleted_at = NULL` —— 已删会话只要再被写一次就"自动复活"，
            // 用户会莫名其妙看到删掉的会话又回来了。删除应当是确定的：恢复只能走显式入口
            // （`restore_session`）。新行的 `deleted_at` 由 INSERT 里的 NULL 兜底。
            params![
                meta.id,
                meta.title,
                meta.created_at as i64,
                meta.updated_at as i64,
                meta.message_count as i64,
                if meta.is_smart_named { 1 } else { 0 },
                meta.profile_id,
                meta.total_input_tokens as i64,
                meta.total_output_tokens as i64,
                meta.total_cache_hit_tokens as i64,
                meta.total_cache_miss_tokens as i64,
                meta.title_source,
                meta.project_id,
                // 布尔列显式转 1/0（rusqlite 虽能直接绑 bool，但显式写死口径更清楚）
                if meta.thinking_mode { 1 } else { 0 },
            ],
        )
        .map_err(|e| e.to_string())?;

        let memory_json = serde_json::to_string(memory).map_err(|e| e.to_string())?;
        tx.execute(
            "INSERT INTO session_memory(session_id, memory_json) VALUES(?1, ?2)
             ON CONFLICT(session_id) DO UPDATE SET memory_json = excluded.memory_json",
            params![meta.id, memory_json],
        )
        .map_err(|e| e.to_string())?;

        Ok(())
    })
}

/// `SessionMeta` 的 SELECT 列清单（**必须与 `session_meta_from_row` 的列序号一一对应**）。
///
/// 抽成常量是为了从结构上杜绝一类反复出现的 bug：新增字段时只改了部分 SELECT，
/// 未改的那条查询就会在执行期抛 `Invalid column index`（例如 v11 引入
/// `thinking_mode` 时漏改 `get_session_meta`，直接导致切换会话失败）。
/// 以后只需要在这里加一列，并把 `session_meta_from_row` 的索引往后挪。
pub(super) const SESSION_META_COLUMNS: &str =
    "s.id, s.title, s.created_at, s.updated_at, s.message_count, \
     s.is_smart_named, s.profile_id, s.total_input_tokens, s.total_output_tokens, s.title_source, \
     s.project_id, p.path, s.thinking_mode, s.total_cache_hit_tokens, s.total_cache_miss_tokens, \
     s.deleted_at, p.name";

pub fn get_session_meta(id: &str) -> Result<SessionMeta, String> {
    crate::infra::db::with_read_connection(|conn| {
        let sql = format!(
            "SELECT {} FROM sessions s \
             LEFT JOIN projects p ON s.project_id = p.id \
             WHERE s.id = ?1 AND s.deleted_at IS NULL",
            SESSION_META_COLUMNS
        );
        conn.query_row(&sql, [id], session_meta_from_row)
            .optional()
            .map_err(|e| e.to_string())?
            .ok_or_else(|| format!("会话 {} 不存在", id))
    })
}

/// 列出未归档的会话（按最近更新倒序）。
///
/// 曾有一个 `Option<&SessionListFilter>` 参数与配套的 `matches_filter`，
/// 于 2026-10-07 整体删除 —— 那条筛选链路**三段全断**：前端面板被一个写死的
/// `false` 关着（连关键字搜索框也在里面）、后端命令 `list_sessions()` 不收参数
/// （条件被静默丢弃）、且其中两个查询查的 `agent_run_events.tool` 列在 v15
/// 换表时已不存在。功能建好第二天就被关掉且未留理由，半年多无人使用。
/// 需要"按工具/模型/E 筛选会话"时，工具名现在在 `resp_blocks` 的
/// `ContentBlock::ToolUse` 里（`tool_results` 只有 id 与内容，没有名字）。
pub fn list_sessions() -> Result<Vec<SessionMeta>, String> {
    crate::infra::db::with_read_connection(|conn| {
        let sql = format!(
            "SELECT {} FROM sessions s \
             LEFT JOIN projects p ON s.project_id = p.id \
             WHERE s.deleted_at IS NULL \
             ORDER BY s.updated_at DESC",
            SESSION_META_COLUMNS
        );
        let mut stmt = conn.prepare(&sql).map_err(|e| e.to_string())?;
        let rows = stmt
            .query_map([], session_meta_from_row)
            .map_err(|e| e.to_string())?;
        let mut sessions = Vec::new();
        for row in rows {
            sessions.push(row.map_err(|e| e.to_string())?);
        }
        Ok(sessions)
    })
}

pub fn ensure_session_exists(id: &str, title: Option<&str>, created_at: u64) -> Result<(), String> {
    crate::infra::db::with_connection(|conn| {
        let existing: Option<String> = conn
            .query_row(
                "SELECT id FROM sessions WHERE id = ?1 AND deleted_at IS NULL",
                [id],
                |row| row.get(0),
            )
            .optional()
            .map_err(|e| e.to_string())?;
        if existing.is_some() {
            return Ok(());
        }

        let title = title.unwrap_or("导入会话");
        conn.execute(
            "INSERT INTO sessions(
                id, title, created_at, updated_at, message_count, is_smart_named,
                profile_id, total_input_tokens, total_output_tokens, title_source, project_id, deleted_at
            ) VALUES(?1, ?2, ?3, ?3, 0, 0, NULL, 0, 0, 'default', NULL, NULL)",
            params![id, title, created_at as i64],
        )
        .map_err(|e| e.to_string())?;

        let memory_json =
            serde_json::to_string(&SessionMemory::default()).map_err(|e| e.to_string())?;
        conn.execute(
            "INSERT INTO session_memory(session_id, memory_json) VALUES(?1, ?2)
             ON CONFLICT(session_id) DO NOTHING",
            params![id, memory_json],
        )
        .map_err(|e| e.to_string())?;

        Ok(())
    })
}

/// **归档**会话：只打 `deleted_at` 标记（列名是"删除"时代的遗留，语义已是归档）。
///
/// 为什么归档必须能撤销：撤销入口见 [`restore_session`]。所以连带地 ——
/// 消息、快照数据、回收站里的本体**全部保留**，否则取消归档之后回滚会缺数据。
/// 真正清掉这些回滚侧产物的只有硬删除路径（见 [`hard_delete_session`] 与
/// `core::session::purge_rollback_artifacts`）。
///
/// 已归档会话不再出现在任何列表/统计里（各读路径都带 `AND deleted_at IS NULL`），
/// 也**不会**因为再次被 upsert 而复活（见 [`upsert_session`] 的 ON CONFLICT）。
pub fn delete_session(id: &str) -> Result<(), String> {
    crate::infra::db::with_connection(|conn| {
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_millis() as i64;
        let changed = conn
            .execute(
                "UPDATE sessions SET deleted_at = ?2 WHERE id = ?1 AND deleted_at IS NULL",
                params![id, now],
            )
            .map_err(|e| e.to_string())?;
        if changed == 0 {
            return Err(format!("会话 {} 不存在或已在归档列表里", id));
        }
        Ok(())
    })
}

/// **硬删除**会话（真删行）。
///
/// 只给"用户明确要求彻底删除"的场景用（`purge_session`，入口在「归档」区）。
/// 界面上的归档走的是 [`delete_session`] —— 只打标记，随时可撤销。
pub fn hard_delete_session(id: &str) -> Result<(), String> {
    crate::infra::db::with_connection(|conn| {
        conn.execute("DELETE FROM sessions WHERE id = ?1", [id])
            .map_err(|e| e.to_string())?;
        Ok(())
    })
}

/// 取消归档（清掉 `deleted_at`），把会话放回活跃列表。
pub fn restore_session(id: &str) -> Result<(), String> {
    crate::infra::db::with_connection(|conn| {
        let changed = conn
            .execute(
                "UPDATE sessions SET deleted_at = NULL WHERE id = ?1 AND deleted_at IS NOT NULL",
                [id],
            )
            .map_err(|e| e.to_string())?;
        if changed == 0 {
            return Err(format!("会话 {} 不在归档列表里", id));
        }
        Ok(())
    })
}

/// 已归档的会话列表（供界面的「归档」区入口用）。
///
/// 与 `list_sessions` 的唯一区别是过滤条件取反；列清单复用同一份
/// [`SESSION_META_COLUMNS`]，所以不会出现"两条查询列不一致"的老问题。
pub fn list_deleted_sessions() -> Result<Vec<SessionMeta>, String> {
    crate::infra::db::with_read_connection(list_deleted_sessions_in)
}

/// [`list_deleted_sessions`] 的 connection 版（测试直接跑真实 SQL）。
///
/// LEFT JOIN `projects` **不过滤 `p.deleted_at`**：归档页要同时展示会话与它的项目名；
/// 项目归档时两者是一起归档的，过滤掉项目就只能看到一条没头没尾的会话。
pub(super) fn list_deleted_sessions_in(
    conn: &rusqlite::Connection,
) -> Result<Vec<SessionMeta>, String> {
    let sql = format!(
        "SELECT {} FROM sessions s \
         LEFT JOIN projects p ON s.project_id = p.id \
         WHERE s.deleted_at IS NOT NULL \
         ORDER BY s.deleted_at DESC",
        SESSION_META_COLUMNS
    );
    let mut stmt = conn.prepare(&sql).map_err(|e| e.to_string())?;
    let rows = stmt
        .query_map([], session_meta_from_row)
        .map_err(|e| e.to_string())?;
    let mut sessions = Vec::new();
    for row in rows {
        sessions.push(row.map_err(|e| e.to_string())?);
    }
    Ok(sessions)
}

// ── 项目 / 会话的归档与批量原语（v27）──
//
// 本段函数的共同约束：**数据库改动一律在同一次事务里完成**，
// 文件系统（快照表以外的磁盘目录）清理一律在事务**外**做。
// 原因：事务只能回滚数据库；目录已删、事务再回滚，就变成“行还在、数据没了”的不可恢复态。

/// 归档项目：项目本身 + 名下所有**未归档**会话，在同一个事务里打上同一时间戳。
///
/// 返回被归档的会话数。
///
/// - 已自行归档的会话不改 `deleted_at`：归档时间要保留「用户当时手动归档那一刻」，
///   不能被后来的项目归档覆盖（它已经是用户见过的时间）；
/// - 项目不存在或已归档 → 返回错误。此时事务整体回滚，会话的归档也一并撤销，
///   不会留下「会话已归档、项目还在」的半成品状态（这正是必须单事务的原因）。
///
/// 不清快照与回收站：归档可撤销（见 [`restore_project`]），回滚还要用它们。
pub fn archive_project(project_id: &str, now: i64) -> Result<u64, String> {
    crate::infra::db::with_transaction(|tx| archive_project_in(tx, project_id, now))
}

/// [`archive_project`] 的 connection 版（可在测试里跑真实 SQL，不必依赖全局 DB 单例）。
///
/// 参数是 `&Connection` 而不是 `&Transaction`：`Transaction` 会 deref 到 `Connection`，
/// 同样能在事务上下文里执行；反过来（拿 `&Transaction` 做参数）则无法在测试里不建事务直接用。
pub(super) fn archive_project_in(
    conn: &rusqlite::Connection,
    project_id: &str,
    now: i64,
) -> Result<u64, String> {
    let archived_sessions = conn
        .execute(
            "UPDATE sessions SET deleted_at = ?2 WHERE project_id = ?1 AND deleted_at IS NULL",
            params![project_id, now],
        )
        .map_err(|e| e.to_string())?;

    let changed_project = conn
        .execute(
            "UPDATE projects SET deleted_at = ?2 WHERE id = ?1 AND deleted_at IS NULL",
            params![project_id, now],
        )
        .map_err(|e| e.to_string())?;
    if changed_project == 0 {
        // 返回 Err 会让 `with_transaction` 丢掉 tx、不 commit，上面的会话 UPDATE 随之回滚
        return Err(format!("项目 {} 不存在或已在归档列表里", project_id));
    }

    Ok(archived_sessions as u64)
}

/// 取消归档项目：只清 `projects.deleted_at`。
///
/// 名下会话**不跟着复活**：它们是各自被归档的，应该逐条恢复（见 [`restore_sessions`]）。
/// 这里不做“级联复活”是因为项目可能归档了半年、用户只想找回其中一条会话。
pub fn restore_project(project_id: &str) -> Result<(), String> {
    crate::infra::db::with_connection(|conn| restore_project_in(conn, project_id))
}

/// [`restore_project`] 的 connection 版。不在归档列表时报错（与 `restore_session` 同口径）。
pub(super) fn restore_project_in(
    conn: &rusqlite::Connection,
    project_id: &str,
) -> Result<(), String> {
    let changed = conn
        .execute(
            "UPDATE projects SET deleted_at = NULL WHERE id = ?1 AND deleted_at IS NOT NULL",
            [project_id],
        )
        .map_err(|e| e.to_string())?;
    if changed == 0 {
        return Err(format!("项目 {} 不在归档列表里", project_id));
    }
    Ok(())
}

/// 批量取消归档会话，并把**连带复活**的归档项目回报在 `restored_project_ids` 里。
///
/// 为什么要连带复活项目：归档项目时是项目+会话一起打标的。若只复活会话，它会挂在一个
/// 归档项目下——项目列表里看不到它、`get_project_path` 仍能取值但界面无处可点，
/// 用户看到的是「会话回来了但没在任何项目里」，与他的操作预期不符。
///
/// 失败项（不存在 / 不在归档列表）进 `failed` 而不中断整批：批量操作里单条失效
/// （另一个窗口已恢复、会话已被彻底删除）不应该让其余勾选项全部白做。
///
/// ## 事务边界
/// 整批共用一个事务：会话与项目的复活要么一起生效，要么一起不生效。
/// ⚠️ 事务闭包里**只能**用手上的这把连接，不得再调 `with_connection` / `with_transaction`
/// —— 全局连接是 Mutex 串行化的，重入会直接死锁。
pub fn restore_sessions(ids: &[String]) -> Result<BatchOpResult, String> {
    crate::infra::db::with_transaction(|tx| restore_sessions_in(tx, ids))
}

/// [`restore_sessions`] 的 connection 版。
pub(super) fn restore_sessions_in(
    conn: &rusqlite::Connection,
    ids: &[String],
) -> Result<BatchOpResult, String> {
    let mut succeeded_ids: Vec<String> = Vec::new();
    let mut failed: Vec<BatchOpFailure> = Vec::new();
    let mut restored_project_ids: Vec<String> = Vec::new();

    for id in ids {
        // 去重：同一个 id 在入参里出现两次时，第二次必然影响 0 行，
        // 若照常计入 failed 会误报「恢复失败」。
        if succeeded_ids.contains(id) {
            continue;
        }
        let changed = conn
            .execute(
                "UPDATE sessions SET deleted_at = NULL WHERE id = ?1 AND deleted_at IS NOT NULL",
                [id],
            )
            .map_err(|e| e.to_string())?;
        if changed == 0 {
            failed.push(BatchOpFailure {
                id: id.clone(),
                reason: "会话不存在或不在归档列表".to_string(),
            });
            continue;
        }

        // 会话已复活，再看它的项目是不是也还处于归档态。
        // 刚更新过 deleted_at，所以这里读到的 project_id 仍是归档时的绑定关系。
        let project_id: Option<String> = conn
            .query_row(
                "SELECT project_id FROM sessions WHERE id = ?1",
                [id],
                |row| row.get::<_, Option<String>>(0),
            )
            .optional()
            .map_err(|e| e.to_string())?
            .flatten();
        if let Some(project_id) = project_id {
            let project_changed = conn
                .execute(
                    "UPDATE projects SET deleted_at = NULL \
                     WHERE id = ?1 AND deleted_at IS NOT NULL",
                    [&project_id],
                )
                .map_err(|e| e.to_string())?;
            if project_changed > 0 && !restored_project_ids.contains(&project_id) {
                restored_project_ids.push(project_id);
            }
        }

        succeeded_ids.push(id.clone());
    }

    Ok(BatchOpResult {
        succeeded_ids,
        failed,
        restored_project_ids,
    })
}

/// 批量**彻底删除**归档会话（行 + 回滚侧产物）。
///
/// ## 两步式设计（为什么不能一步到底）
/// 第一步（事务内）：逐条校验「确实在归档列表里」并删行。`session_messages` 有
/// `ON DELETE CASCADE`，会随之清掉；但 `snapshot_trees` / `snapshot_content` 等快照表
/// 和回收站目录**没有 FK 级联**，必须显式清理。
/// 第二步（事务提交后）：逐条调 [`crate::core::session::purge_rollback_artifacts`] 清快照表与回收站目录。
///
/// ## 取舍：文件系统清理失败不算删除失败
/// 会话行已删是不可逆的，此时把该 id 报成 failed 是“误导”：用户再看列表它已经不在了，
/// 重试也只会得到「不在归档列表」。所以清理失败仅 `eprintln` 记录（留给运维排查残留），
/// 该 id 仍计入 `succeeded_ids`。若将来要做“残留产物自动清理”，应该另起扫描任务，
/// 而不是把这个函数变成半成功态。
///
/// 拒绝删除活跃（未归档）会话：那些 id 进 `failed`，原因「不在归档列表」。
/// 这是本函数唯一的防误删门——彻底删除不可撤销，不能因为前端传错 id 就清掉在用会话。
pub fn purge_sessions(ids: &[String]) -> Result<BatchOpResult, String> {
    // 事务内只做数据库部分，返回值里的 pending 是「待清产物的会话 id」（已删行）
    let (result, purged_ids) = crate::infra::db::with_transaction(|tx| purge_sessions_in(tx, ids))?;

    // 事务已提交，此处才碰文件系统 / 快照表：清理不可回滚，必须排在提交之后
    for session_id in &purged_ids {
        crate::core::session::purge_rollback_artifacts(session_id);
    }
    Ok(result)
}

/// [`purge_sessions`] 的事务内部分：校验 + 删行。
///
/// 第二个返回值是**待清理回滚产物的 id 列表**（与 `result.succeeded_ids` 内容相同，
/// 单列出来是为了让调用方在事务外循环时不必从结果结构里绕）。
pub(super) fn purge_sessions_in(
    conn: &rusqlite::Connection,
    ids: &[String],
) -> Result<(BatchOpResult, Vec<String>), String> {
    let mut succeeded_ids: Vec<String> = Vec::new();
    let mut failed: Vec<BatchOpFailure> = Vec::new();
    let mut pending: Vec<String> = Vec::new();

    // 第一遍：只校验不删。先校验完再删，好处是入参里重复的 id 不会把第二条误判成失败。
    for id in ids {
        if pending.contains(id) {
            continue;
        }
        let archived: Option<String> = conn
            .query_row(
                "SELECT id FROM sessions WHERE id = ?1 AND deleted_at IS NOT NULL",
                [id],
                |row| row.get(0),
            )
            .optional()
            .map_err(|e| e.to_string())?;
        match archived {
            Some(_) => pending.push(id.clone()),
            None => failed.push(BatchOpFailure {
                id: id.clone(),
                reason: "不在归档列表".to_string(),
            }),
        }
    }

    // 第二遍：删行。`session_messages` / `session_memory` 有 ON DELETE CASCADE，会自动跟随删除。
    for id in pending {
        conn.execute("DELETE FROM sessions WHERE id = ?1", [&id])
            .map_err(|e| e.to_string())?;
        succeeded_ids.push(id);
    }

    Ok((
        BatchOpResult {
            succeeded_ids: succeeded_ids.clone(),
            failed,
            // 删除不会复活任何项目，字段保持空（结构体统一，调用方不必分分支处理）
            restored_project_ids: Vec::new(),
        },
        // 待清理回滚产物的 id（事务外循环用），与 result.succeeded_ids 内容一致
        succeeded_ids,
    ))
}

/// 归档会话总数（归档页角标，也是「清空归档」的前置读数）。
pub fn count_deleted_sessions() -> Result<usize, String> {
    crate::infra::db::with_read_connection(count_deleted_sessions_in)
}

/// [`count_deleted_sessions`] 的 connection 版。
pub(super) fn count_deleted_sessions_in(conn: &rusqlite::Connection) -> Result<usize, String> {
    let count: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM sessions WHERE deleted_at IS NOT NULL",
            [],
            |row| row.get(0),
        )
        .map_err(|e| e.to_string())?;
    Ok(count.max(0) as usize)
}

pub fn rename_session(
    id: &str,
    title: &str,
    is_smart_named: bool,
    title_source: &str,
) -> Result<SessionMeta, String> {
    crate::infra::db::with_connection(|conn| {
        let changed = conn
            .execute(
                "UPDATE sessions SET title = ?2, is_smart_named = ?3, title_source = ?4 WHERE id = ?1 AND deleted_at IS NULL",
                params![id, title, if is_smart_named { 1 } else { 0 }, title_source],
            )
            .map_err(|e| e.to_string())?;
        if changed == 0 {
            return Err(format!("会话 {} 不存在", id));
        }
        let sql = format!(
            "SELECT {} FROM sessions s \
             LEFT JOIN projects p ON s.project_id = p.id \
             WHERE s.id = ?1 AND s.deleted_at IS NULL",
            SESSION_META_COLUMNS
        );
        conn.query_row(&sql, [id], session_meta_from_row)
            .map_err(|e| e.to_string())
    })
}

pub fn update_session_profile(id: &str, profile_id: &str) -> Result<(), String> {
    crate::infra::db::with_connection(|conn| {
        let changed = conn
            .execute(
                "UPDATE sessions SET profile_id = ?2 WHERE id = ?1 AND deleted_at IS NULL",
                params![id, profile_id],
            )
            .map_err(|e| e.to_string())?;
        if changed == 0 {
            return Err(format!("会话 {} 不存在", id));
        }
        Ok(())
    })
}

/// 写入会话的深度思考档位（**布尔**）。
///
/// v14 起该列是 `INTEGER NOT NULL`，只写 `1` / `0`，**永不写 NULL**：
/// "跟随设置默认"这件事已在会话创建时被解析掉，库里只保留确定值——
/// 这正是"设置改动不倒灌已有会话"的实现基础（详见 `core::session::thinking`）。
pub fn update_session_thinking_mode(id: &str, enabled: bool) -> Result<(), String> {
    crate::infra::db::with_connection(|conn| {
        let changed = conn
            .execute(
                "UPDATE sessions SET thinking_mode = ?2 WHERE id = ?1 AND deleted_at IS NULL",
                params![id, if enabled { 1 } else { 0 }],
            )
            .map_err(|e| e.to_string())?;
        if changed == 0 {
            return Err(format!("会话 {} 不存在", id));
        }
        Ok(())
    })
}

/// 会话级运行偏好的恢复读：工作模式 / 权限档位 / 用户类型。
///
/// 与 thinking_mode 同语义：`None` = 用户在本会话从未表态，恢复时回落设置默认。
/// 刻意**不进** `SessionMeta` / `upsert_session` 全量链——这三列只由下面的
/// `update_session_*` 单列写入，避免 `save_session` 全量落库时被内存快照里的值覆盖。
#[derive(Debug, Clone, Default)]
pub struct SessionRuntimePrefs {
    pub work_mode: Option<String>,
    pub approval_mode: Option<String>,
    pub agent_audience: Option<String>,
}

pub fn get_session_runtime_prefs(id: &str) -> Result<SessionRuntimePrefs, String> {
    crate::infra::db::with_read_connection(|conn| {
        conn.query_row(
            "SELECT work_mode, approval_mode, agent_audience FROM sessions \
             WHERE id = ?1 AND deleted_at IS NULL",
            [id],
            |row| {
                Ok(SessionRuntimePrefs {
                    work_mode: row.get(0)?,
                    approval_mode: row.get(1)?,
                    agent_audience: row.get(2)?,
                })
            },
        )
        .map_err(|e| e.to_string())
    })
}

/// 三个运行偏好共用的单列写入：`column` 只接受本模块三个包装传入的字面量，
/// 不接外部输入，无注入面。会话不存在或已归档时报错（与 thinking_mode 同口径）。
fn update_session_text_column(id: &str, column: &str, value: &str) -> Result<(), String> {
    crate::infra::db::with_connection(|conn| {
        let changed = conn
            .execute(
                &format!(
                    "UPDATE sessions SET {} = ?2 WHERE id = ?1 AND deleted_at IS NULL",
                    column
                ),
                params![id, value],
            )
            .map_err(|e| e.to_string())?;
        if changed == 0 {
            return Err(format!("会话 {} 不存在", id));
        }
        Ok(())
    })
}

pub fn update_session_work_mode(id: &str, mode: &str) -> Result<(), String> {
    update_session_text_column(id, "work_mode", mode)
}

pub fn update_session_approval_mode(id: &str, mode: &str) -> Result<(), String> {
    update_session_text_column(id, "approval_mode", mode)
}

pub fn update_session_agent_audience(id: &str, audience: &str) -> Result<(), String> {
    update_session_text_column(id, "agent_audience", audience)
}

pub fn get_last_active_session_id() -> Option<String> {
    crate::infra::db::with_read_connection(|conn| {
        conn.query_row(
            "SELECT value FROM app_state WHERE key = 'last_active_session_id'",
            [],
            |row| row.get::<_, String>(0),
        )
        .optional()
        .map_err(|e| e.to_string())
    })
    .ok()
    .flatten()
}

pub fn set_last_active_session_id(id: &str) -> Result<(), String> {
    crate::infra::db::with_connection(|conn| {
        conn.execute(
            "INSERT INTO app_state(key, value) VALUES('last_active_session_id', ?1)
             ON CONFLICT(key) DO UPDATE SET value = excluded.value",
            [id],
        )
        .map_err(|e| e.to_string())?;
        Ok(())
    })
}

pub fn clear_last_active_session_id() -> Result<(), String> {
    crate::infra::db::with_connection(|conn| {
        conn.execute(
            "DELETE FROM app_state WHERE key = 'last_active_session_id'",
            [],
        )
        .map_err(|e| e.to_string())?;
        Ok(())
    })
}

/// 从 `sessions`（LEFT JOIN `projects`）的一行还原会话元数据。
///
/// **按列名取值**（与 `stored_session_message_from_row` 同一口径，理由见那里的说明）。
/// 注意 `working_directory` 取自 `p.path`、`project_name` 取自 `p.name` —— 这是按列名之后
/// 才显式化的隐式约定，别"顺手"改成 `s` 的列（sessions 表根本没有这两列）。
pub(super) fn session_meta_from_row(row: &Row<'_>) -> rusqlite::Result<SessionMeta> {
    Ok(SessionMeta {
        id: row.get("id")?,
        title: row.get("title")?,
        created_at: row.get::<_, i64>("created_at")? as u64,
        updated_at: row.get::<_, i64>("updated_at")? as u64,
        message_count: row.get::<_, i64>("message_count")? as usize,
        is_smart_named: row.get::<_, i64>("is_smart_named")? != 0,
        profile_id: row.get("profile_id")?,
        total_input_tokens: row.get::<_, i64>("total_input_tokens")? as u64,
        total_output_tokens: row.get::<_, i64>("total_output_tokens")? as u64,
        title_source: row.get("title_source")?,
        project_id: row.get("project_id")?,
        working_directory: row.get("path")?,
        thinking_mode: row.get::<_, i64>("thinking_mode")? != 0,
        total_cache_hit_tokens: row.get::<_, i64>("total_cache_hit_tokens")? as u64,
        total_cache_miss_tokens: row.get::<_, i64>("total_cache_miss_tokens")? as u64,
        // NULL = 未归档；负值（脏数据）归一到 0，不把负数带进 u64
        deleted_at: row
            .get::<_, Option<i64>>("deleted_at")?
            .map(|value| value.max(0) as u64),
        // 无项目（或项目行缺失）时 LEFT JOIN 给 NULL → None
        project_name: row.get("name")?,
    })
}
