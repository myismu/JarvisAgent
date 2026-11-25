//! # schema/migrations_v1_v15.rs — 数据库结构迁移（v1 至 v15）
//!
//! 承载早期数据库版本（v3、v6、v7、v8、v10、v11、v12、v13、v14、v15）的增量升级逻辑。
//!
//! ## Key Exports
//! - `migrate_v3_drop_deprecated_tables()` .. `migrate_v15_agent_run_events_per_loop()`: 早期版本迁移函数
//!
//! ## Dependencies
//! - External: `rusqlite`
use rusqlite::Connection;
/// 删除废弃的旧 checkpoint 表（v3 迁移）
pub(super) fn migrate_v3_drop_deprecated_tables(conn: &Connection) -> Result<(), rusqlite::Error> {
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
pub(super) fn migrate_v8_agent_runs_message_id(conn: &Connection) -> Result<(), rusqlite::Error> {
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
pub(super) fn migrate_v6_add_session_message_id(conn: &Connection) -> Result<(), rusqlite::Error> {
    // 新数据库表尚未创建 → 跳过
    let table_exists: bool = conn
        .prepare(
            "SELECT count(*) FROM sqlite_master WHERE type='table' AND name='session_messages'",
        )
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
        conn.execute(
            "ALTER TABLE session_messages ADD COLUMN message_id TEXT",
            [],
        )?;
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
pub(super) fn migrate_v7_decouple_session_messages(conn: &Connection) -> Result<(), rusqlite::Error> {
    // 新数据库表尚未创建 → 跳过
    let table_exists: bool = conn
        .prepare(
            "SELECT count(*) FROM sqlite_master WHERE type='table' AND name='session_messages'",
        )
        .and_then(|mut s| s.query_row([], |r| r.get::<_, i64>(0)))
        .map(|c| c > 0)
        .unwrap_or(false);
    if !table_exists {
        return Ok(());
    }

    fn has_column(conn: &Connection, table: &str, column: &str) -> Result<bool, rusqlite::Error> {
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
            .prepare(&format!(
                "SELECT count(*) FROM sqlite_master WHERE type='table' AND name='{}'",
                table
            ))
            .and_then(|mut s| s.query_row([], |r| r.get::<_, i64>(0)))
            .map(|c| c > 0)
            .unwrap_or(false);
        if table_exists && !has_column(conn, table, column)? {
            conn.execute(
                &format!("ALTER TABLE {} ADD COLUMN {}", table, definition),
                [],
            )?;
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
pub(super) fn migrate_v10_add_projects(conn: &Connection) -> Result<(), rusqlite::Error> {
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
///
/// ⚠️ **v14 已把该列改为布尔语义**（见 [`migrate_v14_thinking_mode_to_bool`]）。
/// 本函数保留原样是为了让"老库从任意版本升到最新"的迁移链完整——
/// v10 的库先跑 v11 建出 TEXT 列，再跑 v14 转成布尔，与"新库直接建布尔列"同构。
pub(super) fn migrate_v11_add_session_thinking_mode(conn: &Connection) -> Result<(), rusqlite::Error> {
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

/// 把 `sessions.thinking_mode` 从**三态字符串**改为**布尔**（v14 迁移）
///
/// ## 为什么改
///
/// 旧设计里 `NULL` 表示"未表态"，由裁决层**每轮现读**设置默认值解析。后果是：
/// 改一个预设的默认档位，**所有 `NULL` 会话下次发消息时全部跟着变**——
/// `NULL` 不是一个值，而是"每次都去外面问一句"，于是设置改动会无差别倒灌已有会话。
///
/// 现设计把"跟随"的解析**提前到会话创建那一刻**（`pipeline::start_run` 首次解析）：
/// 库里只存确定布尔，设置改动就再也无法回溯修改任何已有会话。
///
/// ## 迁移动作
///
/// 1. 存量的 `'always'` / `'on'` / `'true'` → `1`；
/// 2. 存量的 `'never'` / `'off'` / `'false'` → `0`；
/// 3. 存量的 `NULL` / 其它脏值 → `0`（保守关闭）。
///
/// 第 3 条是**刻意的保守选择**：老会话没有可靠的"当时想开还是想关"的历史信息，
/// 与其猜一个值，不如统一按"关"处理——用户在界面上拨一下即可覆盖。
/// 开发阶段数据无价，不做更精细的区分。
///
/// SQLite 不支持 `ALTER COLUMN`，因此用"加新列 → 回填 → 删旧列 → 重命名"完成。
pub(super) fn migrate_v14_thinking_mode_to_bool(conn: &Connection) -> Result<(), rusqlite::Error> {
    let columns: Vec<String> = {
        let mut stmt = conn.prepare("PRAGMA table_info(sessions)")?;
        let rows = stmt.query_map([], |row| row.get::<_, String>(1))?;
        let mut out = Vec::new();
        for row in rows {
            out.push(row?);
        }
        out
    };
    if !columns.iter().any(|c| c == "thinking_mode") {
        // 极端情况：连列都没有（不该发生，v11 已保证）——直接补一个布尔列
        conn.execute(
            "ALTER TABLE sessions ADD COLUMN thinking_mode INTEGER NOT NULL DEFAULT 0",
            [],
        )?;
        return Ok(());
    }

    conn.execute(
        "ALTER TABLE sessions ADD COLUMN thinking_mode_bool INTEGER NOT NULL DEFAULT 0",
        [],
    )?;
    // 只有明确的开启语义才置 1，其余（含 NULL 与脏值）一律 0
    conn.execute(
        "UPDATE sessions SET thinking_mode_bool = CASE \
             WHEN thinking_mode IN ('always', 'on', 'true', '1') THEN 1 \
             ELSE 0 END",
        [],
    )?;
    conn.execute("ALTER TABLE sessions DROP COLUMN thinking_mode", [])?;
    conn.execute(
        "ALTER TABLE sessions RENAME COLUMN thinking_mode_bool TO thinking_mode",
        [],
    )?;
    Ok(())
}

/// `agent_run_events` 改为「每轮一行的消息表」+ 删除三个 live 列与 checkpoints 表（v15 迁移）
///
/// ## 为什么要重构
///
/// 旧设计里"一个 run 发生了什么"散在四张载体上，**没有一条承担"每轮记录"的职责**：
/// - `agent_runs.live_content` / `live_thinking` / `live_tool_buffer`：每 SSE event
///   一次 `push_str` + 整行 UPDATE（无分隔符），把整个 run 的所有 loop 糊成一根字符串，
///   写放大 30-60x，且跨轮无边界、异常时无法判断哪段属于第几轮；
/// - `agent_run_events`：只装 `start`/`checkpoint`/`complete` 的**固定文案** + 最终正文副本，
///   是日志转储而非消息表，装不下请求/响应，无法重建；
/// - `agent_run_checkpoints`：每 loop 一次**全量 messages 快照**，O(n²) 膨胀。
///
/// 新设计：`agent_run_events` 一行 = 一个 loop，装下"这一轮发生了什么"（响应 + 工具结果），
/// 成为崩溃重建的**唯一**数据源（详见 `doc/agent_run_events-重构方案*.md`）。
///
/// ## 迁移动作
///
/// 1. 删除旧 `agent_run_events`（内容全是文案转储，**无保留价值，直接丢弃**）；
/// 2. 重建 `agent_run_events` 为目标结构（`loop_index` / `resp_blocks` / `tool_results` …）；
/// 3. 重建 `agent_runs` 去掉三个 live 列（其余列**完整搬移**）；
/// 4. `DROP TABLE agent_run_checkpoints`。
///
/// ⚠️ **以下"不支持 DROP COLUMN"的结论已过时**（2026-09-21 更正）：
/// 本迁移写于 rusqlite 升级之前，当时内置 SQLite 版本不支持该语法，故走
/// "建新表 → 搬数据 → 删旧表 → 改名"。现在用的是 rusqlite 0.32（`bundled`，
/// 内置 SQLite **3.46**），`ALTER TABLE ... DROP COLUMN`（3.35+ 起支持）完全可用
/// —— 同文件的 `sessions.working_directory`、`sessions.thinking_mode`
/// 以及 v18 删除 `agent_runs.interrupt_kind` 都已在用。
///
/// ⇒ **新增迁移若只是删列，直接用 `DROP COLUMN` 即可，不必重建表。**
/// 本 v15 迁移保留"重建表"写法不动：它同时还要改列约束与表结构（DROP COLUMN
/// 做不到），且既有库早已执行过，重写无收益且有风险。
pub(super) fn migrate_v15_agent_run_events_per_loop(conn: &Connection) -> Result<(), rusqlite::Error> {
    let table_exists = |name: &str| -> Result<bool, rusqlite::Error> {
        conn.query_row(
            "SELECT count(*) FROM sqlite_master WHERE type='table' AND name=?1",
            [name],
            |r| r.get::<_, i64>(0),
        )
        .map(|c| c > 0)
    };

    // ── 1. 旧 events 表直接丢弃（文案转储，无重建价值）──
    if table_exists("agent_run_events")? {
        conn.execute("DROP TABLE agent_run_events", [])?;
    }

    // ── 2. 重建 agent_runs：去掉 live_thinking / live_tool_buffer / live_content ──
    if table_exists("agent_runs")? {
        conn.execute_batch(
            "CREATE TABLE agent_runs_new (
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
                error TEXT,
                summary TEXT,
                resumable INTEGER NOT NULL DEFAULT 0,
                resumed_from_run_id TEXT,
                FOREIGN KEY(session_id) REFERENCES sessions(id) ON DELETE CASCADE
            );
            INSERT INTO agent_runs_new(
                run_id, session_id, status, user_message_preview, message_id, loop_count,
                input_tokens, output_tokens, started_at, updated_at, finished_at,
                last_safe_point, error, summary, resumable, resumed_from_run_id
            )
            SELECT
                run_id, session_id, status, user_message_preview, message_id, loop_count,
                input_tokens, output_tokens, started_at, updated_at, finished_at,
                last_safe_point, error, summary, resumable, resumed_from_run_id
            FROM agent_runs;
            DROP TABLE agent_runs;
            ALTER TABLE agent_runs_new RENAME TO agent_runs;",
        )?;
    }

    // ── 3. checkpoints 全量快照不再需要 ──
    conn.execute("DROP TABLE IF EXISTS agent_run_checkpoints", [])?;

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
pub(super) fn migrate_v12_add_session_cache_tokens(conn: &Connection) -> Result<(), rusqlite::Error> {
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

/// 引入 `sessions.work_mode` / `approval_mode` / `agent_audience`（v13 迁移）
///
/// 三个会话级运行偏好的落库：`NULL` = 用户在本会话从未表态，
/// 恢复时回落到用户设置里的默认（app-config.json）；一旦在会话内切换即落库，
/// 此后该会话与设置默认解耦（切换会话/修改设置不再互相牵连）。
///
/// 刻意**不回填**：老会话一律 `NULL`（等价"跟随设置默认"），与 v11 的 thinking_mode 同语义。
pub(super) fn migrate_v13_add_session_runtime_prefs(conn: &Connection) -> Result<(), rusqlite::Error> {
    for column in ["work_mode", "approval_mode", "agent_audience"] {
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
                &format!("ALTER TABLE sessions ADD COLUMN {} TEXT", column),
                [],
            )?;
        }
    }
    Ok(())
}
