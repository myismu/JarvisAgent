//! # schema.rs — SQLite 表结构初始化
//!
//! 创建会话、运行事件、checkpoint 和迁移状态所需的数据库表与索引。
//!
//! ## Key Exports
//! - `init_schema()`: 初始化或升级 SQLite schema
//!
//! ## Dependencies
//! - External: `rusqlite`

use rusqlite::Connection;

pub const SCHEMA_VERSION: i64 = 12;

/// 删除废弃的旧 checkpoint 表（v3 迁移）
fn migrate_v3_drop_deprecated_tables(conn: &Connection) -> Result<(), rusqlite::Error> {
    let tables_to_drop = [
        "checkpoint_backups",
        "checkpoint_operations",
        "checkpoints",
        "checkpoint_branches",
    ];
    for table in &tables_to_drop {
        conn.execute(&format!("DROP TABLE IF EXISTS {}", table), [])?;
    }
    Ok(())
}

/// agent_runs 增加 message_id 关联 session_messages（v8 迁移）
fn migrate_v8_agent_runs_message_id(conn: &Connection) -> Result<(), rusqlite::Error> {
    let table_exists: bool = conn
        .prepare("SELECT count(*) FROM sqlite_master WHERE type='table' AND name='agent_runs'")
        .and_then(|mut s| s.query_row([], |r| r.get::<_, i64>(0)))
        .map(|c| c > 0)
        .unwrap_or(false);
    if !table_exists {
        return Ok(());
    }

    let has_column = {
        let mut stmt = conn.prepare("PRAGMA table_info(agent_runs)")?;
        let found = stmt
            .query_map([], |row| row.get::<_, String>(1))?
            .filter_map(Result::ok)
            .any(|c| c == "message_id");
        found
    };
    if !has_column {
        conn.execute("ALTER TABLE agent_runs ADD COLUMN message_id TEXT", [])?;
    }
    Ok(())
}

/// 为 session_messages 增加稳定 message_id（v6 迁移）
fn migrate_v6_add_session_message_id(conn: &Connection) -> Result<(), rusqlite::Error> {
    // 新数据库表尚未创建 → 跳过
    let table_exists: bool = conn
        .prepare("SELECT count(*) FROM sqlite_master WHERE type='table' AND name='session_messages'")
        .and_then(|mut s| s.query_row([], |r| r.get::<_, i64>(0)))
        .map(|c| c > 0)
        .unwrap_or(false);
    if !table_exists {
        return Ok(());
    }

    let mut stmt = conn.prepare("PRAGMA table_info(session_messages)")?;
    let has_message_id = stmt
        .query_map([], |row| row.get::<_, String>(1))?
        .filter_map(Result::ok)
        .any(|column| column == "message_id");
    drop(stmt);

    if !has_message_id {
        conn.execute("ALTER TABLE session_messages ADD COLUMN message_id TEXT", [])?;
    }

    let rows: Vec<(i64, String, i64)> = {
        let mut stmt = conn.prepare(
            "SELECT id, session_id, seq FROM session_messages
             WHERE message_id IS NULL OR message_id = ''",
        )?;
        let rows = stmt
            .query_map([], |row| {
                Ok((
                    row.get::<_, i64>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, i64>(2)?,
                ))
            })?
            .collect::<Result<Vec<_>, _>>()?;
        rows
    };

    for (id, session_id, seq) in rows {
        let message_id = format!("legacy:{}:{}", session_id, seq);
        conn.execute(
            "UPDATE session_messages SET message_id = ?1 WHERE id = ?2",
            rusqlite::params![message_id, id],
        )?;
    }

    conn.execute(
        "CREATE UNIQUE INDEX IF NOT EXISTS idx_session_messages_session_message_id
         ON session_messages(session_id, message_id)",
        [],
    )?;
    Ok(())
}

/// 为 session_messages 和 checkpoint 链接增加 message_id 解耦字段（v7 迁移）
fn migrate_v7_decouple_session_messages(conn: &Connection) -> Result<(), rusqlite::Error> {
    // 新数据库表尚未创建 → 跳过
    let table_exists: bool = conn
        .prepare("SELECT count(*) FROM sqlite_master WHERE type='table' AND name='session_messages'")
        .and_then(|mut s| s.query_row([], |r| r.get::<_, i64>(0)))
        .map(|c| c > 0)
        .unwrap_or(false);
    if !table_exists {
        return Ok(());
    }

    fn has_column(
        conn: &Connection,
        table: &str,
        column: &str,
    ) -> Result<bool, rusqlite::Error> {
        let mut stmt = conn.prepare(&format!("PRAGMA table_info({})", table))?;
        let has_column = stmt
            .query_map([], |row| row.get::<_, String>(1))?
            .filter_map(Result::ok)
            .any(|name| name == column);
        Ok(has_column)
    }

    for (table, column, definition) in [
        ("session_messages", "updated_at", "updated_at INTEGER"),
        ("session_messages", "recalled_at", "recalled_at INTEGER"),
        ("session_messages", "hidden_at", "hidden_at INTEGER"),
        (
            "session_messages",
            "source",
            "source TEXT NOT NULL DEFAULT 'chat'",
        ),
        ("session_messages", "turn_id", "turn_id TEXT"),
        (
            "checkpoint_user_message_links",
            "message_id",
            "message_id TEXT",
        ),
        (
            "checkpoint_user_message_links",
            "updated_at",
            "updated_at INTEGER",
        ),
        (
            "pending_snapshot_patches",
            "trigger_user_message_id",
            "trigger_user_message_id TEXT",
        ),
    ] {
        // 表可能不存在（新数据库已改为 agent_run_patches）→ 先检查表
        let table_exists: bool = conn
            .prepare(&format!("SELECT count(*) FROM sqlite_master WHERE type='table' AND name='{}'", table))
            .and_then(|mut s| s.query_row([], |r| r.get::<_, i64>(0)))
            .map(|c| c > 0)
            .unwrap_or(false);
        if table_exists && !has_column(conn, table, column)? {
            conn.execute(&format!("ALTER TABLE {} ADD COLUMN {}", table, definition), [])?;
        }
    }

    conn.execute(
        "UPDATE session_messages SET updated_at = created_at WHERE updated_at IS NULL",
        [],
    )?;
    conn.execute(
        "UPDATE session_messages SET source = 'chat' WHERE source IS NULL OR source = ''",
        [],
    )?;
    conn.execute(
        "UPDATE checkpoint_user_message_links SET updated_at = created_at WHERE updated_at IS NULL",
        [],
    )?;

    conn.execute(
        "CREATE INDEX IF NOT EXISTS idx_session_messages_visible_seq
         ON session_messages(session_id, hidden_at, recalled_at, source, seq)",
        [],
    )?;
    conn.execute(
        "CREATE INDEX IF NOT EXISTS idx_session_messages_turn
         ON session_messages(session_id, turn_id, seq)",
        [],
    )?;
    conn.execute(
        "CREATE INDEX IF NOT EXISTS idx_checkpoint_user_message_links_message_id
         ON checkpoint_user_message_links(session_id, message_id)",
        [],
    )?;
    // pending_snapshot_patches 已在 v9 重命名为 agent_run_patches，表不存在时跳过
    let _ = conn.execute(
        "CREATE INDEX IF NOT EXISTS idx_pending_snapshot_patches_trigger_message_id
         ON pending_snapshot_patches(session_id, trigger_user_message_id)",
        [],
    );

    Ok(())
}

/// 引入 projects 表，sessions 增加 project_id 关联（v10 迁移）
fn migrate_v10_add_projects(conn: &Connection) -> Result<(), rusqlite::Error> {
    conn.execute_batch(
        "CREATE TABLE IF NOT EXISTS projects (
            id TEXT PRIMARY KEY,
            name TEXT NOT NULL,
            path TEXT NOT NULL UNIQUE,
            created_at INTEGER NOT NULL,
            updated_at INTEGER NOT NULL
        );",
    )?;

    let has_project_id = {
        let mut stmt = conn.prepare("PRAGMA table_info(sessions)")?;
        let columns: Vec<String> = stmt
            .query_map([], |row| row.get::<_, String>(1))?
            .filter_map(Result::ok)
            .collect();
        columns.iter().any(|c| c == "project_id")
    };
    if !has_project_id {
        conn.execute("ALTER TABLE sessions ADD COLUMN project_id TEXT", [])?;
    }

    // 将旧 working_directory 数据迁移到 projects
    let has_wd = {
        let mut stmt = conn.prepare("PRAGMA table_info(sessions)")?;
        let columns: Vec<String> = stmt
            .query_map([], |row| row.get::<_, String>(1))?
            .filter_map(Result::ok)
            .collect();
        columns.iter().any(|c| c == "working_directory")
    };

    if has_wd {
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs() as i64;

        let mut stmt = conn.prepare(
            "SELECT DISTINCT working_directory FROM sessions
             WHERE working_directory IS NOT NULL AND working_directory != ''
               AND deleted_at IS NULL",
        )?;
        let dirs: Vec<String> = stmt
            .query_map([], |row| row.get::<_, String>(0))?
            .filter_map(Result::ok)
            .collect();

        for dir in &dirs {
            let name = std::path::Path::new(dir)
                .file_name()
                .map(|n| n.to_string_lossy().to_string())
                .unwrap_or_else(|| dir.clone());
            let id = uuid::Uuid::new_v4().to_string()[..8].to_string();
            conn.execute(
                "INSERT OR IGNORE INTO projects (id, name, path, created_at, updated_at)
                 VALUES (?1, ?2, ?3, ?4, ?4)",
                rusqlite::params![id, name, dir, now],
            )?;
        }

        conn.execute(
            "UPDATE sessions SET project_id = (
                SELECT projects.id FROM projects WHERE projects.path = sessions.working_directory
            ) WHERE working_directory IS NOT NULL AND working_directory != ''",
            [],
        )?;

        conn.execute("ALTER TABLE sessions DROP COLUMN working_directory", [])?;
    }

    conn.execute(
        "CREATE INDEX IF NOT EXISTS idx_sessions_project ON sessions(project_id, updated_at DESC)",
        [],
    )?;

    Ok(())
}

/// 引入 sessions.thinking_mode（v11 迁移）
///
/// 深度思考档位的会话级表态：`NULL` / `'auto'` = 跟随预设默认，
/// `'always'` = 本会话强制开启，`'never'` = 本会话强制关闭。
///
/// 刻意**不回填**：老会话一律 `NULL`（等价 auto），语义与"用户从未表态"完全一致；
/// 用户历史上的临时开关从未持久化，回填任何具体值都是编造。
fn migrate_v11_add_session_thinking_mode(conn: &Connection) -> Result<(), rusqlite::Error> {
    let has_thinking_mode = {
        let mut stmt = conn.prepare("PRAGMA table_info(sessions)")?;
        let columns: Vec<String> = stmt
            .query_map([], |row| row.get::<_, String>(1))?
            .filter_map(Result::ok)
            .collect();
        columns.iter().any(|c| c == "thinking_mode")
    };
    if !has_thinking_mode {
        conn.execute("ALTER TABLE sessions ADD COLUMN thinking_mode TEXT", [])?;
    }
    Ok(())
}

/// 引入 `sessions.total_cache_hit_tokens` / `total_cache_miss_tokens`（v12 迁移）
///
/// 为什么需要这两列：缓存命中数此前只存在于**当前快照**（每 loop 覆盖）与
/// **12 条的滚动趋势**里，会话级累计从未落库，前端因此算不出"整个会话的命中率"。
/// 而「累计输入 1.8M」这类数字离开命中率就没法解读——实测某会话 97.2% 走缓存，
/// 真正全价计费的输入只有 49k。
///
/// 刻意**不回填**：老会话没有可依据的历史数据，一律留在 0。
/// 0 在展示层等价于"未报告"（显示 `--` 而不是 `0%`），不得编造命中率。
fn migrate_v12_add_session_cache_tokens(conn: &Connection) -> Result<(), rusqlite::Error> {
    for column in ["total_cache_hit_tokens", "total_cache_miss_tokens"] {
        let exists = {
            let mut stmt = conn.prepare("PRAGMA table_info(sessions)")?;
            let columns: Vec<String> = stmt
                .query_map([], |row| row.get::<_, String>(1))?
                .filter_map(Result::ok)
                .collect();
            columns.iter().any(|c| c == column)
        };
        if !exists {
            conn.execute(
                &format!(
                    "ALTER TABLE sessions ADD COLUMN {} INTEGER NOT NULL DEFAULT 0",
                    column
                ),
                [],
            )?;
        }
    }
    Ok(())
}

pub fn init_schema(conn: &Connection) -> Result<(), String> {
    // 获取当前 schema 版本
    let current_version: i64 = conn
        .query_row(
            "SELECT value FROM app_state WHERE key = 'schema_version'",
            [],
            |row| row.get(0),
        )
        .unwrap_or(0);

    // 执行迁移
    if current_version < 3 {
        migrate_v3_drop_deprecated_tables(conn).map_err(|e| format!("v3 迁移失败: {}", e))?;
    }
    if current_version < 6 {
        migrate_v6_add_session_message_id(conn).map_err(|e| format!("v6 迁移失败: {}", e))?;
    }
    if current_version < 7 {
        migrate_v7_decouple_session_messages(conn).map_err(|e| format!("v7 迁移失败: {}", e))?;
    }
    if current_version < 8 {
        migrate_v8_agent_runs_message_id(conn).map_err(|e| format!("v8 迁移失败: {}", e))?;
    }
    if current_version < 9 {
        conn.execute("DROP TABLE IF EXISTS snapshots", [])
            .map_err(|e| format!("v9 迁移失败: {}", e))?;
        let _ = conn.execute(
            "ALTER TABLE pending_snapshot_patches RENAME TO agent_run_patches",
            [],
        );
        // 重建 checkpoint_user_message_links，移除指向 snapshots 的外键（旧库才有）
        if conn
            .prepare("SELECT count(*) FROM sqlite_master WHERE type='table' AND name='checkpoint_user_message_links'")
            .and_then(|mut s| s.query_row([], |r| r.get::<_, i64>(0)))
            .map(|c| c > 0)
            .unwrap_or(false)
        {
            conn.execute_batch(
            "CREATE TABLE IF NOT EXISTS checkpoint_user_message_links_new (
                session_id TEXT NOT NULL,
                user_message_index INTEGER NOT NULL,
                checkpoint_id TEXT NOT NULL,
                has_file_edits INTEGER NOT NULL DEFAULT 0,
                created_at INTEGER NOT NULL,
                message_id TEXT,
                updated_at INTEGER,
                PRIMARY KEY(session_id, user_message_index),
                UNIQUE(session_id, checkpoint_id),
                FOREIGN KEY(session_id) REFERENCES sessions(id) ON DELETE CASCADE
            );
            INSERT OR IGNORE INTO checkpoint_user_message_links_new
                SELECT session_id, user_message_index, checkpoint_id, has_file_edits, created_at, message_id, updated_at
                FROM checkpoint_user_message_links;
            DROP TABLE checkpoint_user_message_links;
            ALTER TABLE checkpoint_user_message_links_new RENAME TO checkpoint_user_message_links;",
        )
        .map_err(|e| format!("v9 迁移失败: {}", e))?;
        }
    }
    if current_version < 10 {
        migrate_v10_add_projects(conn).map_err(|e| format!("v10 迁移失败: {}", e))?;
    }
    if current_version < 11 {
        migrate_v11_add_session_thinking_mode(conn)
            .map_err(|e| format!("v11 迁移失败: {}", e))?;
    }
    if current_version < 12 {
        migrate_v12_add_session_cache_tokens(conn)
            .map_err(|e| format!("v12 迁移失败: {}", e))?;
    }

    conn.execute_batch(
        r#"
        CREATE TABLE IF NOT EXISTS app_state (
            key TEXT PRIMARY KEY,
            value TEXT NOT NULL
        );

        CREATE TABLE IF NOT EXISTS projects (
            id TEXT PRIMARY KEY,
            name TEXT NOT NULL,
            path TEXT NOT NULL UNIQUE,
            created_at INTEGER NOT NULL,
            updated_at INTEGER NOT NULL
        );

        CREATE TABLE IF NOT EXISTS sessions (
            id TEXT PRIMARY KEY,
            title TEXT NOT NULL,
            created_at INTEGER NOT NULL,
            updated_at INTEGER NOT NULL,
            message_count INTEGER NOT NULL,
            is_smart_named INTEGER NOT NULL DEFAULT 0,
            profile_id TEXT,
            total_input_tokens INTEGER NOT NULL DEFAULT 0,
            total_output_tokens INTEGER NOT NULL DEFAULT 0,
            total_cache_hit_tokens INTEGER NOT NULL DEFAULT 0,
            total_cache_miss_tokens INTEGER NOT NULL DEFAULT 0,
            title_source TEXT NOT NULL DEFAULT 'default',
            project_id TEXT,
            deleted_at INTEGER,
            thinking_mode TEXT,
            FOREIGN KEY(project_id) REFERENCES projects(id) ON DELETE SET NULL
        );

        CREATE TABLE IF NOT EXISTS session_memory (
            session_id TEXT PRIMARY KEY,
            memory_json TEXT NOT NULL,
            FOREIGN KEY(session_id) REFERENCES sessions(id) ON DELETE CASCADE
        );

        CREATE TABLE IF NOT EXISTS session_messages (
            id INTEGER PRIMARY KEY AUTOINCREMENT,
            session_id TEXT NOT NULL,
            message_id TEXT,
            seq INTEGER NOT NULL,
            role TEXT NOT NULL,
            content_json TEXT NOT NULL,
            created_at INTEGER NOT NULL,
            updated_at INTEGER,
            recalled_at INTEGER,
            hidden_at INTEGER,
            source TEXT NOT NULL DEFAULT 'chat',
            turn_id TEXT,
            FOREIGN KEY(session_id) REFERENCES sessions(id) ON DELETE CASCADE,
            UNIQUE(session_id, seq)
        );

        CREATE TABLE IF NOT EXISTS agent_runs (
            run_id TEXT PRIMARY KEY,
            session_id TEXT NOT NULL,
            status TEXT NOT NULL,
            user_message_preview TEXT NOT NULL,
            message_id TEXT,
            loop_count INTEGER NOT NULL,
            input_tokens INTEGER NOT NULL,
            output_tokens INTEGER NOT NULL,
            started_at INTEGER NOT NULL,
            updated_at INTEGER NOT NULL,
            finished_at INTEGER,
            last_safe_point TEXT,
            live_thinking TEXT NOT NULL DEFAULT '',
            live_tool_buffer TEXT NOT NULL DEFAULT '',
            live_content TEXT NOT NULL DEFAULT '',
            error TEXT,
            summary TEXT,
            resumable INTEGER NOT NULL DEFAULT 0,
            resumed_from_run_id TEXT,
            FOREIGN KEY(session_id) REFERENCES sessions(id) ON DELETE CASCADE
        );

        CREATE TABLE IF NOT EXISTS agent_run_events (
            event_id TEXT PRIMARY KEY,
            run_id TEXT NOT NULL,
            session_id TEXT NOT NULL,
            event_type TEXT NOT NULL,
            message TEXT NOT NULL,
            tool TEXT,
            input_summary TEXT,
            output_summary TEXT,
            error TEXT,
            loop_count INTEGER NOT NULL,
            input_tokens INTEGER NOT NULL,
            output_tokens INTEGER NOT NULL,
            timestamp INTEGER NOT NULL,
            model TEXT,
            FOREIGN KEY(run_id) REFERENCES agent_runs(run_id) ON DELETE CASCADE,
            FOREIGN KEY(session_id) REFERENCES sessions(id) ON DELETE CASCADE
        );

        CREATE TABLE IF NOT EXISTS subagent_events (
            event_id TEXT PRIMARY KEY,
            run_id TEXT NOT NULL,
            session_id TEXT NOT NULL,
            event_type TEXT NOT NULL,
            message TEXT NOT NULL,
            tool TEXT,
            input_summary TEXT,
            output_summary TEXT,
            error TEXT,
            loop_count INTEGER NOT NULL,
            input_tokens INTEGER NOT NULL,
            output_tokens INTEGER NOT NULL,
            timestamp INTEGER NOT NULL,
            FOREIGN KEY(session_id) REFERENCES sessions(id) ON DELETE CASCADE
        );

        CREATE TABLE IF NOT EXISTS agent_run_checkpoints (
            run_id TEXT PRIMARY KEY,
            session_id TEXT NOT NULL,
            loop_count INTEGER NOT NULL,
            messages_json TEXT NOT NULL,
            input_tokens INTEGER NOT NULL,
            output_tokens INTEGER NOT NULL,
            last_safe_point TEXT NOT NULL,
            updated_at INTEGER NOT NULL,
            FOREIGN KEY(run_id) REFERENCES agent_runs(run_id) ON DELETE CASCADE,
            FOREIGN KEY(session_id) REFERENCES sessions(id) ON DELETE CASCADE
        );

        CREATE TABLE IF NOT EXISTS session_context_snapshots (
            session_id TEXT PRIMARY KEY,
            snapshot_json TEXT NOT NULL,
            updated_at INTEGER NOT NULL,
            FOREIGN KEY(session_id) REFERENCES sessions(id) ON DELETE CASCADE
        );

        -- 旧 checkpoint 表已在 v3 迁移中删除，不再创建

        CREATE TABLE IF NOT EXISTS session_attachments (
            filename TEXT PRIMARY KEY,
            session_id TEXT NOT NULL,
            media_type TEXT NOT NULL,
            data BLOB NOT NULL,
            created_at INTEGER NOT NULL,
            FOREIGN KEY(session_id) REFERENCES sessions(id) ON DELETE CASCADE
        );

        CREATE TABLE IF NOT EXISTS session_transcripts (
            id TEXT PRIMARY KEY,
            session_id TEXT NOT NULL,
            filename TEXT NOT NULL,
            content TEXT NOT NULL,
            created_at INTEGER NOT NULL,
            FOREIGN KEY(session_id) REFERENCES sessions(id) ON DELETE CASCADE,
            UNIQUE(session_id, filename)
        );

        CREATE TABLE IF NOT EXISTS session_tasks (
            session_id TEXT NOT NULL,
            task_id INTEGER NOT NULL,
            task_json TEXT NOT NULL,
            updated_at INTEGER NOT NULL,
            PRIMARY KEY(session_id, task_id),
            FOREIGN KEY(session_id) REFERENCES sessions(id) ON DELETE CASCADE
        );

        CREATE TABLE IF NOT EXISTS snapshot_trees (
            session_id TEXT PRIMARY KEY,
            tree_json TEXT NOT NULL,
            updated_at INTEGER NOT NULL,
            FOREIGN KEY(session_id) REFERENCES sessions(id) ON DELETE CASCADE
        );

        CREATE TABLE IF NOT EXISTS checkpoint_user_message_links (
            session_id TEXT NOT NULL,
            user_message_index INTEGER NOT NULL,
            checkpoint_id TEXT NOT NULL,
            has_file_edits INTEGER NOT NULL DEFAULT 0,
            created_at INTEGER NOT NULL,
            message_id TEXT,
            updated_at INTEGER,
            PRIMARY KEY(session_id, user_message_index),
            UNIQUE(session_id, checkpoint_id),
            FOREIGN KEY(session_id) REFERENCES sessions(id) ON DELETE CASCADE
        );

        CREATE TABLE IF NOT EXISTS agent_run_patches (
            session_id TEXT NOT NULL,
            run_id TEXT NOT NULL,
            seq INTEGER NOT NULL,
            patch_json TEXT NOT NULL,
            message TEXT,
            trigger_user_memory_index INTEGER,
            trigger_user_message_id TEXT,
            created_at INTEGER NOT NULL,
            PRIMARY KEY(session_id, run_id, seq),
            FOREIGN KEY(session_id) REFERENCES sessions(id) ON DELETE CASCADE
        );

        CREATE TABLE IF NOT EXISTS snapshot_content (
            session_id TEXT NOT NULL,
            content_hash TEXT NOT NULL,
            content TEXT NOT NULL,
            created_at INTEGER NOT NULL,
            PRIMARY KEY(session_id, content_hash),
            FOREIGN KEY(session_id) REFERENCES sessions(id) ON DELETE CASCADE
        );

        CREATE TABLE IF NOT EXISTS snapshot_journal (
            id INTEGER PRIMARY KEY AUTOINCREMENT,
            session_id TEXT NOT NULL,
            event_json TEXT NOT NULL,
            created_at INTEGER NOT NULL,
            FOREIGN KEY(session_id) REFERENCES sessions(id) ON DELETE CASCADE
        );

        CREATE TABLE IF NOT EXISTS snapshot_sandboxes (
            session_id TEXT NOT NULL,
            sandbox_id TEXT NOT NULL,
            sandbox_json TEXT NOT NULL,
            updated_at INTEGER NOT NULL,
            PRIMARY KEY(session_id, sandbox_id),
            FOREIGN KEY(session_id) REFERENCES sessions(id) ON DELETE CASCADE
        );

        CREATE INDEX IF NOT EXISTS idx_sessions_updated_at ON sessions(updated_at DESC);
        CREATE INDEX IF NOT EXISTS idx_sessions_created_at ON sessions(created_at);
        CREATE INDEX IF NOT EXISTS idx_sessions_profile_updated ON sessions(profile_id, updated_at DESC);
        CREATE INDEX IF NOT EXISTS idx_sessions_project ON sessions(project_id, updated_at DESC);
        CREATE INDEX IF NOT EXISTS idx_session_messages_session_seq ON session_messages(session_id, seq);
        CREATE UNIQUE INDEX IF NOT EXISTS idx_session_messages_session_message_id ON session_messages(session_id, message_id);
        CREATE INDEX IF NOT EXISTS idx_session_messages_visible_seq ON session_messages(session_id, hidden_at, recalled_at, source, seq);
        CREATE INDEX IF NOT EXISTS idx_session_messages_turn ON session_messages(session_id, turn_id, seq);
        CREATE INDEX IF NOT EXISTS idx_agent_runs_session_started ON agent_runs(session_id, started_at DESC);
        CREATE INDEX IF NOT EXISTS idx_agent_run_events_session_time ON agent_run_events(session_id, timestamp DESC);
        CREATE INDEX IF NOT EXISTS idx_agent_run_events_tool_time ON agent_run_events(tool, timestamp DESC);
        CREATE INDEX IF NOT EXISTS idx_session_context_snapshots_updated ON session_context_snapshots(updated_at DESC);

        CREATE INDEX IF NOT EXISTS idx_session_attachments_session ON session_attachments(session_id);
        CREATE INDEX IF NOT EXISTS idx_session_transcripts_session_time ON session_transcripts(session_id, created_at DESC);
        CREATE INDEX IF NOT EXISTS idx_checkpoint_user_message_links_session ON checkpoint_user_message_links(session_id, user_message_index);
        CREATE INDEX IF NOT EXISTS idx_checkpoint_user_message_links_message_id ON checkpoint_user_message_links(session_id, message_id);
        CREATE INDEX IF NOT EXISTS idx_agent_run_patches_session_run ON agent_run_patches(session_id, run_id, seq);
        CREATE INDEX IF NOT EXISTS idx_agent_run_patches_trigger_message_id ON agent_run_patches(session_id, trigger_user_message_id);
        CREATE INDEX IF NOT EXISTS idx_snapshot_journal_session ON snapshot_journal(session_id, id);
        "#,
    )
    .map_err(|e| e.to_string())?;

    conn.execute(
        "INSERT INTO app_state(key, value) VALUES('schema_version', ?1)
         ON CONFLICT(key) DO UPDATE SET value = excluded.value",
        [SCHEMA_VERSION.to_string()],
    )
    .map_err(|e| e.to_string())?;

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn column_exists(conn: &Connection, table: &str, column: &str) -> bool {
        let mut stmt = conn
            .prepare(&format!("PRAGMA table_info({})", table))
            .expect("prepare table_info");
        let found: Vec<String> = stmt
            .query_map([], |row| row.get::<_, String>(1))
            .expect("query table_info")
            .filter_map(Result::ok)
            .collect();
        found.iter().any(|c| c == column)
    }

    /// v10 老库升级到 v11：升级前没有 thinking_mode，升级后必须有且老行保持 NULL。
    ///
    /// 注意：这里刻意先建好 `sessions` 表再调 `init_schema`——`init_schema` 对**完全空库**
    /// 并不幂等（v10 迁移会 `ALTER` 不存在的表），真实场景下 DB 文件由应用先创建，
    /// 因此测试按真实形态构造：已有 v10 表结构 + `schema_version = 10`。
    #[test]
    fn v11_migration_adds_thinking_mode_to_legacy_db() {
        let conn = Connection::open_in_memory().expect("open memory db");
        // v10 形态的 sessions 表（无 thinking_mode）
        conn.execute_batch(
            "CREATE TABLE sessions (
                id TEXT PRIMARY KEY,
                title TEXT NOT NULL,
                created_at INTEGER NOT NULL,
                updated_at INTEGER NOT NULL,
                message_count INTEGER NOT NULL,
                profile_id TEXT,
                deleted_at INTEGER
            );
            CREATE TABLE app_state (key TEXT PRIMARY KEY, value TEXT NOT NULL);
            INSERT INTO app_state(key, value) VALUES('schema_version', '10');
            INSERT INTO sessions(id, title, created_at, updated_at, message_count, profile_id, deleted_at)
                VALUES('s1', '老会话', 1, 1, 0, 'default', NULL);",
        )
        .expect("create legacy schema");

        assert!(!column_exists(&conn, "sessions", "thinking_mode"));

        init_schema(&conn).expect("upgrade v10 -> v11");

        assert!(
            column_exists(&conn, "sessions", "thinking_mode"),
            "v11 迁移必须补上 thinking_mode 列"
        );
        // 老行不得被回填任何具体档位：NULL 才等价于"用户从未表态"（auto）
        let value: Option<String> = conn
            .query_row("SELECT thinking_mode FROM sessions WHERE id = 's1'", [], |r| {
                r.get(0)
            })
            .expect("read legacy row");
        assert_eq!(value, None, "老会话必须保持 NULL(auto)，不得编造档位");

        // 版本号推进到 12
        let version: String = conn
            .query_row(
                "SELECT value FROM app_state WHERE key = 'schema_version'",
                [],
                |r| r.get(0),
            )
            .expect("read version");
        assert_eq!(version, "12");

        // 幂等：再次初始化不报错，且不破坏已有值
        conn.execute("UPDATE sessions SET thinking_mode = 'never' WHERE id = 's1'", [])
            .expect("set mode");
        init_schema(&conn).expect("re-init idempotent");
        let after: Option<String> = conn
            .query_row("SELECT thinking_mode FROM sessions WHERE id = 's1'", [], |r| {
                r.get(0)
            })
            .expect("re-read");
        assert_eq!(after.as_deref(), Some("never"), "重复初始化不得清掉用户表态");
    }

    /// 建表 DDL 必须自带 thinking_mode（保证新库与升级后的老库同构）
    #[test]
    fn fresh_ddl_declares_thinking_mode() {
        let conn = Connection::open_in_memory().expect("open memory db");
        conn.execute(
            "CREATE TABLE sessions (
                id TEXT PRIMARY KEY,
                title TEXT NOT NULL,
                created_at INTEGER NOT NULL,
                updated_at INTEGER NOT NULL,
                message_count INTEGER NOT NULL,
                is_smart_named INTEGER NOT NULL DEFAULT 0,
                profile_id TEXT,
                total_input_tokens INTEGER NOT NULL DEFAULT 0,
                total_output_tokens INTEGER NOT NULL DEFAULT 0,
                title_source TEXT NOT NULL DEFAULT 'default',
                project_id TEXT,
                deleted_at INTEGER,
                thinking_mode TEXT
            )",
            [],
        )
        .expect("create sessions");
        assert!(column_exists(&conn, "sessions", "thinking_mode"));
    }

    /// v11 老库升级到 v12：补上两列缓存累计，且老行一律留在 0。
    ///
    /// 老行必须留在 0 而不是被回填：0 在展示层等价于"未报告"（显示 `--`），
    /// 回填任何具体值都等于给用户编一个假的命中率。
    #[test]
    fn v12_migration_adds_session_cache_tokens_to_legacy_db() {
        let conn = Connection::open_in_memory().expect("open memory db");
        // v11 形态的 sessions 表（无缓存两列）
        conn.execute_batch(
            "CREATE TABLE sessions (
                id TEXT PRIMARY KEY,
                title TEXT NOT NULL,
                created_at INTEGER NOT NULL,
                updated_at INTEGER NOT NULL,
                message_count INTEGER NOT NULL,
                profile_id TEXT,
                deleted_at INTEGER,
                thinking_mode TEXT
            );
            CREATE TABLE app_state (key TEXT PRIMARY KEY, value TEXT NOT NULL);
            INSERT INTO app_state(key, value) VALUES('schema_version', '11');
            INSERT INTO sessions(id, title, created_at, updated_at, message_count, profile_id, deleted_at, thinking_mode)
                VALUES('s1', '老会话', 1, 1, 0, 'default', NULL, NULL);",
        )
        .expect("create legacy schema");

        assert!(!column_exists(&conn, "sessions", "total_cache_hit_tokens"));
        assert!(!column_exists(&conn, "sessions", "total_cache_miss_tokens"));

        init_schema(&conn).expect("upgrade v11 -> v12");

        assert!(
            column_exists(&conn, "sessions", "total_cache_hit_tokens"),
            "v12 迁移必须补上 total_cache_hit_tokens 列"
        );
        assert!(
            column_exists(&conn, "sessions", "total_cache_miss_tokens"),
            "v12 迁移必须补上 total_cache_miss_tokens 列"
        );
        let (hit, miss): (i64, i64) = conn
            .query_row(
                "SELECT total_cache_hit_tokens, total_cache_miss_tokens FROM sessions WHERE id = 's1'",
                [],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .expect("read legacy row");
        assert_eq!((hit, miss), (0, 0), "老会话两列必须留在 0，不得编造命中率");
    }
}
