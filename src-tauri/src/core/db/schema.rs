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

pub const SCHEMA_VERSION: i64 = 5;

/// 删除废弃的旧 checkpoint 表（v3 迁移）
fn migrate_v3_drop_deprecated_tables(conn: &Connection) -> Result<(), rusqlite::Error> {
    let tables_to_drop = [
        "checkpoint_backups",
        "checkpoint_operations",
        "checkpoints",
        "checkpoint_branches",
    ];
    for table in &tables_to_drop {
        conn.execute(
            &format!("DROP TABLE IF EXISTS {}", table),
            [],
        )?;
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
        migrate_v3_drop_deprecated_tables(conn)
            .map_err(|e| format!("v3 迁移失败: {}", e))?;
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
            seq INTEGER NOT NULL,
            role TEXT NOT NULL,
            content_json TEXT NOT NULL,
            created_at INTEGER NOT NULL,
            FOREIGN KEY(session_id) REFERENCES sessions(id) ON DELETE CASCADE,
            UNIQUE(session_id, seq)
        );

        CREATE TABLE IF NOT EXISTS agent_runs (
            run_id TEXT PRIMARY KEY,
            session_id TEXT NOT NULL,
            status TEXT NOT NULL,
            user_message_preview TEXT NOT NULL,
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

        CREATE TABLE IF NOT EXISTS snapshots (
            session_id TEXT NOT NULL,
            snapshot_id TEXT NOT NULL,
            branch_name TEXT NOT NULL,
            snapshot_json TEXT NOT NULL,
            created_at INTEGER NOT NULL,
            PRIMARY KEY(session_id, snapshot_id),
            FOREIGN KEY(session_id) REFERENCES sessions(id) ON DELETE CASCADE
        );

        CREATE TABLE IF NOT EXISTS checkpoint_user_message_links (
            session_id TEXT NOT NULL,
            user_message_index INTEGER NOT NULL,
            checkpoint_id TEXT NOT NULL,
            has_file_edits INTEGER NOT NULL DEFAULT 0,
            created_at INTEGER NOT NULL,
            PRIMARY KEY(session_id, user_message_index),
            UNIQUE(session_id, checkpoint_id),
            FOREIGN KEY(session_id) REFERENCES sessions(id) ON DELETE CASCADE,
            FOREIGN KEY(session_id, checkpoint_id) REFERENCES snapshots(session_id, snapshot_id) ON DELETE CASCADE
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
        CREATE INDEX IF NOT EXISTS idx_agent_runs_session_started ON agent_runs(session_id, started_at DESC);
        CREATE INDEX IF NOT EXISTS idx_agent_run_events_session_time ON agent_run_events(session_id, timestamp DESC);
        CREATE INDEX IF NOT EXISTS idx_agent_run_events_tool_time ON agent_run_events(tool, timestamp DESC);
        CREATE INDEX IF NOT EXISTS idx_session_context_snapshots_updated ON session_context_snapshots(updated_at DESC);

        CREATE INDEX IF NOT EXISTS idx_session_attachments_session ON session_attachments(session_id);
        CREATE INDEX IF NOT EXISTS idx_session_transcripts_session_time ON session_transcripts(session_id, created_at DESC);
        CREATE INDEX IF NOT EXISTS idx_snapshots_session_branch_time ON snapshots(session_id, branch_name, created_at DESC);
        CREATE INDEX IF NOT EXISTS idx_checkpoint_user_message_links_session ON checkpoint_user_message_links(session_id, user_message_index);
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
