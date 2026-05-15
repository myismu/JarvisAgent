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

pub const SCHEMA_VERSION: i64 = 9;

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

    conn.execute_batch(
        r#"
        CREATE TABLE IF NOT EXISTS app_state (
            key TEXT PRIMARY KEY,
            value TEXT NOT NULL
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
            title_source TEXT NOT NULL DEFAULT 'default',
            working_directory TEXT,
            deleted_at INTEGER
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
