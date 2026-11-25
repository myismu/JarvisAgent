//! # schema/mod.rs — SQLite 表结构初始化入口
//!
//! 创建会话、运行事件、checkpoint 和迁移状态所需的数据库表与索引，并按版本号编排历史增量迁移。
//!
//! ## Key Exports
//! - `init_schema()`: 初始化或升级 SQLite schema
//! - `SCHEMA_VERSION`: 当前数据库结构版本
//!
//! ## Dependencies
//! - Internal: `migrations_v1_v15`, `migrations_v16_v29`
//! - External: `rusqlite`
mod migrations_v16_v29;
mod migrations_v1_v15;

#[cfg(test)]
mod tests;

use rusqlite::Connection;

use migrations_v16_v29::{
    migrate_v16_timestamps_to_millis, migrate_v17_add_interrupt_kind,
    migrate_v18_drop_agent_runs_interrupt_kind, migrate_v19_drop_unused_run_columns,
    migrate_v20_add_agent_runs_interrupt_kind, migrate_v21_message_source_and_drop_turn_id,
    migrate_v22_add_reflection_columns, migrate_v23_backfill_interrupt_kind,
    migrate_v24_subagent_trace, migrate_v25_subagent_trace_run_label, migrate_v26_run_wait_ms,
    migrate_v27_project_archive, migrate_v28_request_usage,
    migrate_v29_checkpoint_links_message_id,
};
use migrations_v1_v15::{
    migrate_v10_add_projects, migrate_v11_add_session_thinking_mode,
    migrate_v12_add_session_cache_tokens, migrate_v13_add_session_runtime_prefs,
    migrate_v14_thinking_mode_to_bool, migrate_v15_agent_run_events_per_loop,
    migrate_v3_drop_deprecated_tables, migrate_v6_add_session_message_id,
    migrate_v7_decouple_session_messages, migrate_v8_agent_runs_message_id,
};
/// 当前 schema 版本。改这个常量时必须同时做三件事：
/// 1. 写一个 `migrate_vNN_*` 迁移函数（老库靠它补结构）；
/// 2. 在 `init_schema` 的版本分支链末尾追加 `current_version < NN` 分支；
/// 3. 把新结构同步进 `init_schema` 末尾的建表 DDL（新库不跑迁移，DDL 就是它的全部结构）。
///
/// 三者缺一，新库与升级库的结构就会分叉。
pub const SCHEMA_VERSION: i64 = 29;

pub fn init_schema(conn: &Connection) -> Result<(), String> {
    // 获取当前 schema 版本。
    //
    // ⚠️ 必须 `CAST(value AS INTEGER)`：`app_state.value` 列声明为 TEXT，而 SQLite
    // 是动态类型——写入的其实是 TEXT（本函数末尾用 `SCHEMA_VERSION.to_string()` 绑定）。
    // rusqlite 的 `row.get::<_, i64>()` 对 TEXT **不做隐式转换**，会直接返回
    // `InvalidColumnType`；旧代码把该错误 `unwrap_or(0)` 吞掉，于是版本号**永远读成 0**。
    // 历史后果：迁移被无差别地重复执行（幂等迁移掩盖了症状）。加 CAST 后读取才可靠。
    let current_version: i64 = conn
        .query_row(
            "SELECT CAST(value AS INTEGER) FROM app_state WHERE key = 'schema_version'",
            [],
            |row| row.get(0),
        )
        .unwrap_or(0);

    // 全新库（version == 0）：**跳过所有增量迁移**，直接走下面的建表 DDL。
    //
    // 必须这样做：增量迁移写的是 `ALTER TABLE ...`，前提是表已存在，而建表 DDL
    // 在本函数末尾才执行。早期版本里 v10 迁移恰好对空库是幂等的（`CREATE TABLE IF
    // NOT EXISTS`），所以"空库跑一遍迁移"侥幸没炸；v14 用 `PRAGMA table_info(sessions)`
    // 探测，空库上直接 `no such table: sessions`，把这个隐患暴露了出来。
    //
    // 建表 DDL 始终反映**最新形态**（已包含 v10..v14 引入的全部列），
    // 因此新库不跑迁移也是同构的。
    if current_version > 0 {
        // 执行迁移
        if current_version < 3 {
            migrate_v3_drop_deprecated_tables(conn).map_err(|e| format!("v3 迁移失败: {}", e))?;
        }
        if current_version < 6 {
            migrate_v6_add_session_message_id(conn).map_err(|e| format!("v6 迁移失败: {}", e))?;
        }
        if current_version < 7 {
            migrate_v7_decouple_session_messages(conn)
                .map_err(|e| format!("v7 迁移失败: {}", e))?;
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
        if current_version < 13 {
            migrate_v13_add_session_runtime_prefs(conn)
                .map_err(|e| format!("v13 迁移失败: {}", e))?;
        }
        if current_version < 14 {
            migrate_v14_thinking_mode_to_bool(conn).map_err(|e| format!("v14 迁移失败: {}", e))?;
        }
        if current_version < 15 {
            migrate_v15_agent_run_events_per_loop(conn)
                .map_err(|e| format!("v15 迁移失败: {}", e))?;
        }
        if current_version < 16 {
            migrate_v16_timestamps_to_millis(conn).map_err(|e| format!("v16 迁移失败: {}", e))?;
        }
        if current_version < 17 {
            migrate_v17_add_interrupt_kind(conn).map_err(|e| format!("v17 迁移失败: {}", e))?;
        }
        if current_version < 18 {
            migrate_v18_drop_agent_runs_interrupt_kind(conn)
                .map_err(|e| format!("v18 迁移失败: {}", e))?;
        }
        if current_version < 19 {
            migrate_v19_drop_unused_run_columns(conn)
                .map_err(|e| format!("v19 迁移失败: {}", e))?;
        }
        if current_version < 20 {
            migrate_v20_add_agent_runs_interrupt_kind(conn)
                .map_err(|e| format!("v20 迁移失败: {}", e))?;
        }
        if current_version < 21 {
            migrate_v21_message_source_and_drop_turn_id(conn)
                .map_err(|e| format!("v21 迁移失败: {}", e))?;
        }
        if current_version < 22 {
            migrate_v22_add_reflection_columns(conn).map_err(|e| format!("v22 迁移失败: {}", e))?;
        }
        if current_version < 23 {
            migrate_v23_backfill_interrupt_kind(conn)
                .map_err(|e| format!("v23 迁移失败: {}", e))?;
        }
        if current_version < 24 {
            migrate_v24_subagent_trace(conn).map_err(|e| format!("v24 迁移失败: {}", e))?;
        }
        if current_version < 25 {
            migrate_v25_subagent_trace_run_label(conn)
                .map_err(|e| format!("v25 迁移失败: {}", e))?;
        }
        if current_version < 26 {
            migrate_v26_run_wait_ms(conn).map_err(|e| format!("v26 迁移失败: {}", e))?;
        }
        if current_version < 27 {
            migrate_v27_project_archive(conn).map_err(|e| format!("v27 迁移失败: {}", e))?;
        }
        if current_version < 28 {
            migrate_v28_request_usage(conn).map_err(|e| format!("v28 迁移失败: {}", e))?;
        }
        if current_version < 29 {
            migrate_v29_checkpoint_links_message_id(conn)
                .map_err(|e| format!("v29 迁移失败: {}", e))?;
        }
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
            updated_at INTEGER NOT NULL,
            -- v27：项目归档时间（毫秒）。NULL = 未归档（见 migrate_v27_project_archive）
            deleted_at INTEGER
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
            thinking_mode INTEGER NOT NULL DEFAULT 0,
            work_mode TEXT,
            approval_mode TEXT,
            agent_audience TEXT,
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
            interrupt_kind TEXT,
            FOREIGN KEY(session_id) REFERENCES sessions(id) ON DELETE CASCADE,
            UNIQUE(session_id, seq)
        );

        CREATE TABLE IF NOT EXISTS agent_runs (
            run_id TEXT PRIMARY KEY,
            session_id TEXT NOT NULL,
            status TEXT NOT NULL,
            message_id TEXT,
            loop_count INTEGER NOT NULL,
            input_tokens INTEGER NOT NULL,
            output_tokens INTEGER NOT NULL,
            started_at INTEGER NOT NULL,
            updated_at INTEGER NOT NULL,
            finished_at INTEGER,
            last_safe_point TEXT,
            summary TEXT,
            resumable INTEGER NOT NULL DEFAULT 0,
            resumed_from_run_id TEXT,
            interrupt_kind TEXT,
            -- v26：等待耗时（毫秒）。NULL = 未统计等待（见 migrate_v26_run_wait_ms）
            wait_ms INTEGER,
            FOREIGN KEY(session_id) REFERENCES sessions(id) ON DELETE CASCADE
        );

        -- 每个 loop 一行：本轮响应（结构化 ContentBlock JSON）+ 本轮工具执行结果。
        -- 崩溃重建的唯一数据源（起点锚 agent_runs.message_id，顺序由 loop_index 决定）。
        CREATE TABLE IF NOT EXISTS agent_run_events (
            event_id TEXT PRIMARY KEY,
            run_id TEXT NOT NULL,
            session_id TEXT NOT NULL,
            loop_index INTEGER NOT NULL,
            resp_blocks TEXT NOT NULL DEFAULT '',
            tool_results TEXT NOT NULL DEFAULT '',
            status TEXT NOT NULL,
            error TEXT,
            input_tokens INTEGER NOT NULL DEFAULT 0,
            output_tokens INTEGER NOT NULL DEFAULT 0,
            model TEXT,
            reflection_judgment TEXT,
            reflection_reason TEXT,
            reflection_suggestion TEXT,
            started_at INTEGER NOT NULL,
            updated_at INTEGER NOT NULL,
            UNIQUE(run_id, loop_index),
            FOREIGN KEY(run_id) REFERENCES agent_runs(run_id) ON DELETE CASCADE,
            FOREIGN KEY(session_id) REFERENCES sessions(id) ON DELETE CASCADE
        );

        -- v28：逐请求 token 用量明细（原始凭据；`sessions.total_*` 是它的派生汇总）。
        -- `cache_*_tokens` 可空：NULL = 厂商未上报（与 0 严格区分，见 migrate_v28_request_usage）。
        CREATE TABLE IF NOT EXISTS request_usage (
            id INTEGER PRIMARY KEY AUTOINCREMENT,
            session_id TEXT NOT NULL,
            run_id TEXT,
            loop_index INTEGER,
            source TEXT NOT NULL,
            parent_tool_use_id TEXT,
            agent_role TEXT,
            task_id TEXT,
            model_id TEXT,
            input_tokens INTEGER NOT NULL DEFAULT 0,
            output_tokens INTEGER NOT NULL DEFAULT 0,
            cache_hit_tokens INTEGER,
            cache_miss_tokens INTEGER,
            cache_source TEXT,
            created_at INTEGER NOT NULL,
            FOREIGN KEY(session_id) REFERENCES sessions(id) ON DELETE CASCADE
        );

        CREATE TABLE IF NOT EXISTS subagent_trace_runs (
            run_id TEXT PRIMARY KEY,
            session_id TEXT NOT NULL,
            parent_run_id TEXT,
            label TEXT NOT NULL DEFAULT '',
            started_at INTEGER NOT NULL,
            FOREIGN KEY(session_id) REFERENCES sessions(id) ON DELETE CASCADE
        );

        CREATE TABLE IF NOT EXISTS subagent_trace_events (
            event_id TEXT PRIMARY KEY,
            run_id TEXT NOT NULL,
            session_id TEXT NOT NULL,
            loop_index INTEGER NOT NULL,
            event_type TEXT NOT NULL,
            payload_json TEXT NOT NULL,
            model TEXT,
            input_tokens INTEGER,
            output_tokens INTEGER,
            timestamp INTEGER NOT NULL,
            FOREIGN KEY(run_id) REFERENCES subagent_trace_runs(run_id) ON DELETE CASCADE,
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
            message_id TEXT NOT NULL,
            checkpoint_id TEXT NOT NULL,
            has_file_edits INTEGER NOT NULL DEFAULT 0,
            created_at INTEGER NOT NULL,
            updated_at INTEGER,
            PRIMARY KEY(session_id, message_id),
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
        CREATE INDEX IF NOT EXISTS idx_agent_runs_session_started ON agent_runs(session_id, started_at DESC);
        CREATE INDEX IF NOT EXISTS idx_agent_runs_message_id ON agent_runs(message_id);
        CREATE INDEX IF NOT EXISTS idx_agent_run_events_run_loop ON agent_run_events(run_id, loop_index);
        CREATE INDEX IF NOT EXISTS idx_agent_run_events_session_time ON agent_run_events(session_id, started_at DESC);
        CREATE INDEX IF NOT EXISTS idx_request_usage_session_time ON request_usage(session_id, created_at);
        CREATE INDEX IF NOT EXISTS idx_subagent_trace_runs_session ON subagent_trace_runs(session_id, started_at);
        CREATE INDEX IF NOT EXISTS idx_subagent_trace_events_run_loop ON subagent_trace_events(run_id, loop_index);
        CREATE INDEX IF NOT EXISTS idx_session_context_snapshots_updated ON session_context_snapshots(updated_at DESC);

        CREATE INDEX IF NOT EXISTS idx_session_attachments_session ON session_attachments(session_id);
        CREATE INDEX IF NOT EXISTS idx_session_transcripts_session_time ON session_transcripts(session_id, created_at DESC);
        CREATE INDEX IF NOT EXISTS idx_checkpoint_user_message_links_session ON checkpoint_user_message_links(session_id, message_id);
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
