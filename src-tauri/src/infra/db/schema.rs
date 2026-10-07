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

pub const SCHEMA_VERSION: i64 = 23;

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
fn migrate_v7_decouple_session_messages(conn: &Connection) -> Result<(), rusqlite::Error> {
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
///
/// ⚠️ **v14 已把该列改为布尔语义**（见 [`migrate_v14_thinking_mode_to_bool`]）。
/// 本函数保留原样是为了让"老库从任意版本升到最新"的迁移链完整——
/// v10 的库先跑 v11 建出 TEXT 列，再跑 v14 转成布尔，与"新库直接建布尔列"同构。
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
fn migrate_v14_thinking_mode_to_bool(conn: &Connection) -> Result<(), rusqlite::Error> {
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
fn migrate_v15_agent_run_events_per_loop(conn: &Connection) -> Result<(), rusqlite::Error> {
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

/// 秒级时间戳的判定上限：小于该值视为**秒**（需 ×1000），否则视为已是毫秒。
///
/// 两个量级天然不重叠：秒级最大值 1e11（公元 5138 年），毫秒级最小值 1e12（2001-09-09）。
const TIMESTAMP_SEC_LIMIT: i64 = 100_000_000_000;

/// 把全库时间戳从**秒**统一为**毫秒**（v16 迁移）
///
/// ## 为什么改
/// 此前两套口径并存：运行时表（agent_runs / agent_run_events / subagent_events）写毫秒，
/// 而会话与回滚体系（sessions / session_messages / projects / snapshot_* ...）写秒。后果：
/// - 侧边栏 `new Date(session.updatedAt)` 按毫秒解析秒值 → 会话时间显示成 1970 年；
/// - 排查落库时序（"停止那一刻写了几条"）时秒级精度把多条挤进同一秒，无法区分先后。
///
/// ## 为什么必须迁移存量而不是只改写入
/// 新旧值混存会让**比较逻辑**失真：回滚 GC 按 `now - snapshot.created_at` 算天数，
/// 旧值仍是秒而 `now` 变毫秒 → 差值放大约 1000 倍 → 存量快照被判成"几万年前"而遭误删。
/// 会话列表按 `updated_at` 排序同理（旧会话永久沉底）。故一次性把旧秒值 ×1000。
///
/// ## 不迁移的表
/// agent_runs / agent_run_events / subagent_events / session_context_snapshots
/// 本来就是毫秒，且其秒级判定阈值会跳过它们（毫秒值 ≥ 1e12 > 上限）。
fn migrate_v16_timestamps_to_millis(conn: &Connection) -> Result<(), rusqlite::Error> {
    // ── 1. 纯整数列：直接 UPDATE ×1000（SQLite 无多列批量语法，逐列执行）──
    let columns: [(&str, &str); 18] = [
        ("sessions", "created_at"),
        ("sessions", "updated_at"),
        ("sessions", "deleted_at"),
        ("session_messages", "created_at"),
        ("session_messages", "updated_at"),
        ("session_messages", "hidden_at"),
        ("session_messages", "recalled_at"),
        ("projects", "created_at"),
        ("projects", "updated_at"),
        ("agent_run_patches", "created_at"),
        ("checkpoint_user_message_links", "created_at"),
        ("checkpoint_user_message_links", "updated_at"),
        ("session_attachments", "created_at"),
        ("session_tasks", "updated_at"),
        ("session_transcripts", "created_at"),
        ("snapshot_trees", "updated_at"),
        ("snapshot_journal", "created_at"),
        ("snapshot_sandboxes", "updated_at"),
    ];
    for (table, column) in columns {
        if !db_table_exists(conn, table)? {
            continue;
        }
        conn.execute(
            &format!(
                "UPDATE {table} SET {column} = {column} * 1000
                 WHERE {column} IS NOT NULL AND {column} < {limit}",
                table = table,
                column = column,
                limit = TIMESTAMP_SEC_LIMIT,
            ),
            [],
        )?;
    }

    // ── 2. JSON 内嵌时间戳：SQL 表达式放大不了，读 → 改 → 写回 ──
    // 2.1 快照树：nodes / branches 两个对象映射里每个成员的 createdAt
    scale_json_map_timestamps(
        conn,
        "snapshot_trees",
        "tree_json",
        "session_id",
        &[("nodes", "createdAt"), ("branches", "createdAt")],
    )?;
    // 2.2 快照日志：create_snapshot 事件对象里的 timestamp（其余变体无时间字段）
    scale_json_root_timestamp(conn, "snapshot_journal", "event_json", "id", "timestamp")?;
    // 2.3 会话内存：plan_documents 数组里每条的 createdAt / updatedAt / decidedAt
    scale_json_array_timestamps(
        conn,
        "session_memory",
        "memory_json",
        "session_id",
        "plan_documents",
        &["createdAt", "updatedAt", "decidedAt"],
    )?;

    Ok(())
}

/// 判断表是否存在（旧库可能缺表，迁移需跳过而不是报错）
fn db_table_exists(conn: &Connection, name: &str) -> Result<bool, rusqlite::Error> {
    conn.query_row(
        "SELECT count(*) FROM sqlite_master WHERE type='table' AND name=?1",
        [name],
        |r| r.get::<_, i64>(0),
    )
    .map(|c| c > 0)
}

/// 把数字型的秒级时间戳放大为毫秒；已是毫秒或不是数字时返回 `None`。
fn scale_to_millis(value: &serde_json::Value) -> Option<serde_json::Value> {
    let seconds = value.as_i64()?;
    if seconds < TIMESTAMP_SEC_LIMIT {
        Some(serde_json::Value::from(seconds * 1000))
    } else {
        None
    }
}

/// 把 JSON 里若干**对象映射**（`{键: {…, 时间字段}}`）中的秒级时间字段放大为毫秒。
///
/// 用于 `snapshot_trees.tree_json` 的 `nodes` / `branches`——两者都是 id → 对象的映射。
fn scale_json_map_timestamps(
    conn: &Connection,
    table: &str,
    json_column: &str,
    key_column: &str,
    map_fields: &[(&str, &str)],
) -> Result<(), rusqlite::Error> {
    if !db_table_exists(conn, table)? {
        return Ok(());
    }
    let rows: Vec<(String, String)> = {
        let mut stmt = conn.prepare(&format!("SELECT {key_column}, {json_column} FROM {table}"))?;
        let collected = stmt
            .query_map([], |r| Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?)))?
            .collect::<Result<Vec<_>, _>>()?;
        collected
    };

    for (key, json) in rows {
        // 历史脏数据（非法 JSON）直接跳过：迁移不因个别行失败而中断
        let Ok(mut root) = serde_json::from_str::<serde_json::Value>(&json) else {
            continue;
        };
        let mut changed = false;
        for (map_name, time_field) in map_fields {
            let Some(map) = root.get_mut(*map_name).and_then(|m| m.as_object_mut()) else {
                continue;
            };
            for entry in map.values_mut() {
                let Some(current) = entry.get(*time_field) else {
                    continue;
                };
                if let Some(scaled) = scale_to_millis(current) {
                    entry[*time_field] = scaled;
                    changed = true;
                }
            }
        }
        if changed {
            if let Ok(new_json) = serde_json::to_string(&root) {
                conn.execute(
                    &format!(
                        "UPDATE {table} SET {json_column} = ?1 WHERE {key_column} = ?2",
                        table = table,
                        json_column = json_column,
                        key_column = key_column,
                    ),
                    rusqlite::params![new_json, key],
                )?;
            }
        }
    }
    Ok(())
}

/// 把 JSON **根对象**上的单个时间字段放大为毫秒。
///
/// 用于 `snapshot_journal.event_json`：`create_snapshot` 变体携带 `timestamp`
/// （其余变体无该字段，天然跳过）。
fn scale_json_root_timestamp(
    conn: &Connection,
    table: &str,
    json_column: &str,
    key_column: &str,
    time_field: &str,
) -> Result<(), rusqlite::Error> {
    if !db_table_exists(conn, table)? {
        return Ok(());
    }
    // 主键是 INTEGER，按 i64 读出
    let rows: Vec<(i64, String)> = {
        let mut stmt = conn.prepare(&format!("SELECT {key_column}, {json_column} FROM {table}"))?;
        let collected = stmt
            .query_map([], |r| Ok((r.get::<_, i64>(0)?, r.get::<_, String>(1)?)))?
            .collect::<Result<Vec<_>, _>>()?;
        collected
    };

    for (key, json) in rows {
        let Ok(mut root) = serde_json::from_str::<serde_json::Value>(&json) else {
            continue;
        };
        let Some(current) = root.get(time_field) else {
            continue;
        };
        let Some(scaled) = scale_to_millis(current) else {
            continue;
        };
        root[time_field] = scaled;
        if let Ok(new_json) = serde_json::to_string(&root) {
            conn.execute(
                &format!(
                    "UPDATE {table} SET {json_column} = ?1 WHERE {key_column} = ?2",
                    table = table,
                    json_column = json_column,
                    key_column = key_column,
                ),
                rusqlite::params![new_json, key],
            )?;
        }
    }
    Ok(())
}

/// 把 JSON 里某个**数组**中每条对象的时间字段放大为毫秒。
///
/// 用于 `session_memory.memory_json` 的 `plan_documents`
/// （SessionMemory 字段是 snake_case，PlanDocument 自身字段是 camelCase）。
fn scale_json_array_timestamps(
    conn: &Connection,
    table: &str,
    json_column: &str,
    key_column: &str,
    array_field: &str,
    time_fields: &[&str],
) -> Result<(), rusqlite::Error> {
    if !db_table_exists(conn, table)? {
        return Ok(());
    }
    let rows: Vec<(String, String)> = {
        let mut stmt = conn.prepare(&format!("SELECT {key_column}, {json_column} FROM {table}"))?;
        let collected = stmt
            .query_map([], |r| Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?)))?
            .collect::<Result<Vec<_>, _>>()?;
        collected
    };

    for (key, json) in rows {
        let Ok(mut root) = serde_json::from_str::<serde_json::Value>(&json) else {
            continue;
        };
        let Some(items) = root.get_mut(array_field).and_then(|v| v.as_array_mut()) else {
            continue;
        };
        let mut changed = false;
        for item in items.iter_mut() {
            for field in time_fields {
                let Some(current) = item.get(*field) else {
                    continue;
                };
                if let Some(scaled) = scale_to_millis(current) {
                    item[*field] = scaled;
                    changed = true;
                }
            }
        }
        if changed {
            if let Ok(new_json) = serde_json::to_string(&root) {
                conn.execute(
                    &format!(
                        "UPDATE {table} SET {json_column} = ?1 WHERE {key_column} = ?2",
                        table = table,
                        json_column = json_column,
                        key_column = key_column,
                    ),
                    rusqlite::params![new_json, key],
                )?;
            }
        }
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

/// 引入 `sessions.work_mode` / `approval_mode` / `agent_audience`（v13 迁移）
///
/// 三个会话级运行偏好的落库：`NULL` = 用户在本会话从未表态，
/// 恢复时回落到用户设置里的默认（app-config.json）；一旦在会话内切换即落库，
/// 此后该会话与设置默认解耦（切换会话/修改设置不再互相牵连）。
///
/// 刻意**不回填**：老会话一律 `NULL`（等价"跟随设置默认"），与 v11 的 thinking_mode 同语义。
fn migrate_v13_add_session_runtime_prefs(conn: &Connection) -> Result<(), rusqlite::Error> {
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

/// v17：为 `session_messages` / `agent_runs` 增加 `interrupt_kind`（中断类型结构化）。
///
/// 背景（阶段二改造，见 doc/状态标注符号统一与结构化改造方案.md）：
/// 中断收尾此前把 `**[回复被中断]** …` 标记**拼进 assistant 正文**（模型需要看到），
/// 界面侧只能靠正则从文本里"猜着剥"。kind 落库后：
/// - 界面侧直接读字段生成 notice 小字，正文保持干净；
/// - 发送给模型时按 kind 把标记拼回（模型行为不变）；
/// - ~~崩溃恢复重建时从 `agent_runs.interrupt_kind` 给重建消息打标~~
///   —— **该设想未落地**：重建路径从不读那一列，它也从无写入点，
///   已在 v18 删除（见 `migrate_v18_drop_agent_runs_interrupt_kind`）。
///   实际承担"哪一轮被打断"的是 `agent_run_events.status`。
///
/// 旧行留 `NULL`（= 非中断消息）。旧库的历史中断标记**仍留在正文里**——
/// 本次不做旧数据兼容层，由用户清理旧会话。
fn migrate_v17_add_interrupt_kind(conn: &Connection) -> Result<(), rusqlite::Error> {
    for table in ["session_messages", "agent_runs"] {
        // ⚠️ 表可能不存在：迁移测试用的是最小桩库（只建 sessions/app_state），
        // 早期版本库也未必有这两张表。`ALTER TABLE 不存在的表` 会直接报
        // "no such table"，所以必须先探测表存在性再探测列——
        // 与 v14 在空库上栽的坑同源（`PRAGMA table_info` 对不存在的表返回空集，
        // 看起来"列不存在"于是去 ALTER，结果炸在表上）。
        let table_exists = conn
            .prepare("SELECT count(*) FROM sqlite_master WHERE type='table' AND name = ?1")
            .and_then(|mut stmt| stmt.query_row([table], |row| row.get::<_, i64>(0)))
            .map(|count| count > 0)
            .unwrap_or(false);
        if !table_exists {
            continue;
        }
        let column_exists = {
            let mut stmt = conn.prepare(&format!("PRAGMA table_info({})", table))?;
            let columns: Vec<String> = stmt
                .query_map([], |row| row.get::<_, String>(1))?
                .filter_map(Result::ok)
                .collect();
            columns.iter().any(|c| c == "interrupt_kind")
        };
        if !column_exists {
            conn.execute(
                &format!("ALTER TABLE {} ADD COLUMN interrupt_kind TEXT", table),
                [],
            )?;
        }
    }
    Ok(())
}

/// v18：删除 `agent_runs.interrupt_kind`（设计要求了但从未接线，属死列）。
///
/// ## 为什么删
///
/// v17 给它设想的用途是"崩溃恢复重建时给重建消息打标"，但事实是：
/// - **无写入点**：`RUN_COLUMNS`（agent_run_repository.rs）与 `upsert_run` 都不含它，
///   全仓库没有任何一处写过这一列；
/// - **无读取点**：重建走 `rebuild_messages_from_events`（agent_runs.rs），
///   它只从 `agent_run_events` 重放，**从不读这一列**。
///
/// 真正承担"中断打标"的是另外两处载体：
/// - `session_messages.interrupt_kind` —— 渲染层读它出小字、发送层按它把标记拼回；
/// - `agent_run_events.status` / `.error` —— 判断"哪一轮被打断"。
///
/// 该列留着只会误导（注释声明的用途与实现不符），故删除。
/// ⚠️ **只删 agent_runs 这一列**：`session_messages.interrupt_kind` 有真实读写，
/// 必须保留。
///
/// ## 为什么可以直接用 DROP COLUMN
///
/// rusqlite 0.32（`bundled`）内置 SQLite 3.46，远高于该语法要求的 3.35；
/// 本文件既有迁移（`sessions.working_directory`、`sessions.thinking_mode`）
/// 已在用同一语法，所以无需走"重建表"路线。
fn migrate_v18_drop_agent_runs_interrupt_kind(conn: &Connection) -> Result<(), rusqlite::Error> {
    // ⚠️ 表可能不存在：迁移测试用的是最小桩库（只建 sessions/app_state），
    // 早期版本库也未必有 agent_runs。必须先探表、再探列，
    // 否则 `ALTER TABLE` 一个不存在的表会直接报 "no such table"
    // （与 v14 空库、v17 同源的坑）。
    let table_exists = conn
        .prepare("SELECT count(*) FROM sqlite_master WHERE type='table' AND name = 'agent_runs'")
        .and_then(|mut stmt| stmt.query_row([], |row| row.get::<_, i64>(0)))
        .map(|count| count > 0)
        .unwrap_or(false);
    if !table_exists {
        return Ok(());
    }

    let column_exists = {
        let mut stmt = conn.prepare("PRAGMA table_info(agent_runs)")?;
        let columns: Vec<String> = stmt
            .query_map([], |row| row.get::<_, String>(1))?
            .filter_map(Result::ok)
            .collect();
        columns.iter().any(|c| c == "interrupt_kind")
    };
    if column_exists {
        conn.execute("ALTER TABLE agent_runs DROP COLUMN interrupt_kind", [])?;
    }
    Ok(())
}

/// v19：删除 `agent_runs` 两个**只写不读**的列 —— `user_message_preview` 与 `error`。
///
/// ## 为什么删（2026-09-21 全链路核查）
///
/// 两列都**只写不读**：
/// - `user_message_preview`：`start_run` 写入用户消息前 120 字符；全仓库无任何
///   业务读取点，前端 `types/index.ts` 虽声明了类型却**零渲染**；
/// - `error`：`finish_run` 写入收尾原因；同为无业务读取、无前端渲染
///   （`AgentPanel.vue` 里渲染的 `run.error` 是**子代理**那一套数据，
///   与主 Agent 的 `agent_runs` 无关）。
///
/// 它们与 v18 删掉的 `interrupt_kind` 同源：都是"在主表上放一份摘要副本"，
/// 而真正的逐条事实在 `agent_run_events`（每轮一行的 error/status）里。
/// 冗余副本必然漂移，且本次核查证明它们连"被读"都没做到。
///
/// ⚠️ 影响提示：**run 级别的错误原因不再单独落库**。排查错误请查
/// `agent_run_events.error`（每轮一行、信息更详细）。
fn migrate_v19_drop_unused_run_columns(conn: &Connection) -> Result<(), rusqlite::Error> {
    // ⚠️ 表可能不存在（迁移测试的最小桩库只建 sessions/app_state），先探表再探列。
    let table_exists = conn
        .prepare("SELECT count(*) FROM sqlite_master WHERE type='table' AND name = 'agent_runs'")
        .and_then(|mut stmt| stmt.query_row([], |row| row.get::<_, i64>(0)))
        .map(|count| count > 0)
        .unwrap_or(false);
    if !table_exists {
        return Ok(());
    }

    for column in ["user_message_preview", "error"] {
        let column_exists = {
            let mut stmt = conn.prepare("PRAGMA table_info(agent_runs)")?;
            let columns: Vec<String> = stmt
                .query_map([], |row| row.get::<_, String>(1))?
                .filter_map(Result::ok)
                .collect();
            columns.iter().any(|c| c == column)
        };
        if column_exists {
            conn.execute(
                &format!("ALTER TABLE agent_runs DROP COLUMN {}", column),
                [],
            )?;
        }
    }
    Ok(())
}

/// v20：把"中断类型"从 `summary` 的魔法字符串里解放出来 ——
/// 为 `agent_runs` 加回 `interrupt_kind TEXT` 列（**这次是真接线**）。
///
/// ## 背景：为什么要加回来
///
/// v18 曾删掉这个列，理由是"它没有任何写入点与读取点"——当时属实：它只被设计为
/// "崩溃重建时给重建消息打标"，而重建路径根本不读它。
///
/// 但本次核查发现 `summary` 里藏着一个**真正的状态标志**：`"上次执行在应用关闭
/// 或进程结束时中断。"` 这句话被 `mark_run_recovered` 与 `keep_run_active` 拿去
/// 比对，用来判断"这个 run 是不是因应用关闭而中断"。**拿文案当枚举**——
/// 谁改一个字，恢复链的状态判断就静默失效。
///
/// 规范化的正确落点正是 `InterruptKind`（枚举已有 `AppClosed` 变体与
/// `as_str`/`from_db`）。因此本列回归，职责明确为**记录"本 run 因何中断"**：
/// - 退出/崩溃收尾 → `app_closed`
/// - 流空闲超时 → `stream_timeout`
/// - 用户取消 → `user_cancel`
/// - 执行错误 → `pipeline_error`
///
/// ## 与 `session_messages.interrupt_kind` 的分工
///
/// 前者是**消息级**（哪条消息被中断收尾写下 → 界面渲染小字、发送时拼回标记），
/// 本列是 **run 级**（整次运行为何结束 → 恢复链判定）。粒度不同，不是冗余副本。
///
/// 旧行留 `NULL`（= 未中断或早于本次迁移）。
fn migrate_v20_add_agent_runs_interrupt_kind(conn: &Connection) -> Result<(), rusqlite::Error> {
    // ⚠️ 表可能不存在（迁移测试的最小桩库只建 sessions/app_state），先探表再探列。
    let table_exists = conn
        .prepare("SELECT count(*) FROM sqlite_master WHERE type='table' AND name = 'agent_runs'")
        .and_then(|mut stmt| stmt.query_row([], |row| row.get::<_, i64>(0)))
        .map(|count| count > 0)
        .unwrap_or(false);
    if !table_exists {
        return Ok(());
    }

    let column_exists = {
        let mut stmt = conn.prepare("PRAGMA table_info(agent_runs)")?;
        let columns: Vec<String> = stmt
            .query_map([], |row| row.get::<_, String>(1))?
            .filter_map(Result::ok)
            .collect();
        columns.iter().any(|c| c == "interrupt_kind")
    };
    if !column_exists {
        conn.execute("ALTER TABLE agent_runs ADD COLUMN interrupt_kind TEXT", [])?;
    }
    Ok(())
}

/// v21：`session_messages.source` 取值收敛 + 删除死列 `turn_id`。
///
/// 详见 `doc/消息来源与轮次元数据重构方案.md`。两件事各自独立：
///
/// ## 一、source 取值搬迁（`internal` 一拆为二）
///
/// 旧的 `internal` 同时表示两种**相反**的东西，白名单按"不给模型看"处理，
/// 于是三条**写给模型看的指令**（崩溃恢复指令 / 方案重定向通知 / 反思修正建议）
/// 从未送达模型。新取值把两种语义分开：
///
/// | 旧值 | 新值 | 依据 |
/// |---|---|---|
/// | `internal`（正文是 `Context compressed.`） | `placeholder` | 对齐填充，两侧都不该看 |
/// | `internal`（其余） | `inject` | 写给模型看的系统注入 |
/// | `background` | `placeholder` | 旧白名单本就把它挡在模型与界面之外 |
/// | `context` | `inject` | 旧白名单**放行**它给模型、界面不看 —— 正是 `inject` 的语义 |
///
/// ⚠️ `Context compressed.` 的判定是**一次性数据搬迁**，不是长期内容匹配规则：
/// 它只在本次迁移里跑一遍，此后判定依据只有 `source` 列本身。
/// 将来若这句占位文案被改写，只是那条老数据留在 `inject`（无害），
/// **不需要**回来维护这个 LIKE 条件 —— 长期靠内容反推元信息正是本方案要废除的做法。
///
/// ## 二、删除 `turn_id`
///
/// 它自 v7（`a426209`，2026-05-06）引入起就是**死列**：建了列、建了索引、
/// 进了 SELECT 列清单、读进了结构体，但全仓**零写入点**，恒为 `NULL`。
/// 唯一的天然消费者（`7159b26` 的按轮凑页）当时用「`role = user` 且正文非空」
/// 推断轮边界，压根没用它。而"轮"的信息本已由每轮首条 `Chat` 用户消息的
/// `message_id` 承载（`agent_runs.message_id` 就关联到它），单独存列是重复设计。
///
/// 恒 NULL 的列不是"无害的预留"：它误导读者以为按轮查询可用，还被写进了
/// SELECT 列清单与测试断言，每次读取都要带着它。本迁移连同索引一并删除。
///
/// ⚠️ 还有一处**同名不同物**，别被名字带偏：`core/tools/framework/policy_guard.rs`
/// 的 `PermissionTurnState.turn_id` 装的是 **run_id**（用于批量审批规则"换一轮就重置
/// 文件计数"），是内存态、不落库，与本列无关。
///
/// 另注：`migrate_v7_decouple_session_messages` 里**仍然保留**着创建 `turn_id`
/// 与索引的语句 —— 迁移在本项目里是**只追加的历史记录**（参见 v18 删、
/// v20 又加回 `agent_runs.interrupt_kind` 的先例），不回改旧步骤。
/// 代价是老库升级会走一次"建列 → 删列"，一次性开销，可接受。
fn migrate_v21_message_source_and_drop_turn_id(conn: &Connection) -> Result<(), rusqlite::Error> {
    // ⚠️ 表可能不存在（迁移测试的最小桩库只建 sessions/app_state），先探表
    let table_exists = conn
        .prepare(
            "SELECT count(*) FROM sqlite_master WHERE type='table' AND name = 'session_messages'",
        )
        .and_then(|mut stmt| stmt.query_row([], |row| row.get::<_, i64>(0)))
        .map(|count| count > 0)
        .unwrap_or(false);
    if !table_exists {
        return Ok(());
    }

    // ── 一、source 取值搬迁（顺序敏感：先把例外挑走，再整体搬迁）──
    conn.execute(
        "UPDATE session_messages SET source = 'placeholder'
         WHERE source = 'internal' AND content_json LIKE '%\"content\":\"Context compressed.\"%'",
        [],
    )?;
    conn.execute(
        "UPDATE session_messages SET source = 'inject' WHERE source = 'internal'",
        [],
    )?;
    conn.execute(
        "UPDATE session_messages SET source = 'placeholder' WHERE source = 'background'",
        [],
    )?;
    conn.execute(
        "UPDATE session_messages SET source = 'inject' WHERE source = 'context'",
        [],
    )?;

    // ── 二、删除 turn_id 列与它的索引 ──
    // 索引先删：SQLite 的 DROP COLUMN 对"被索引引用的列"会直接报错。
    conn.execute("DROP INDEX IF EXISTS idx_session_messages_turn", [])?;

    let has_turn_id = {
        let mut stmt = conn.prepare("PRAGMA table_info(session_messages)")?;
        let columns: Vec<String> = stmt
            .query_map([], |row| row.get::<_, String>(1))?
            .filter_map(Result::ok)
            .collect();
        columns.iter().any(|c| c == "turn_id")
    };
    if has_turn_id {
        conn.execute("ALTER TABLE session_messages DROP COLUMN turn_id", [])?;
    }
    Ok(())
}

/// v22：给 `agent_run_events` 加反思审查三列。
///
/// 反思的判定（`ok` / `not_ok`）、原因、修正建议此前**只走 `agent-step` 事件**
/// （前端内存），重载即失 —— 数据库里查不到一次运行触发过几次反思、结论是什么、
/// 模型有没有采纳。而 `agent_run_events` 是"每轮一行"，反思也是**逐轮**的
/// （每个 loop 的工具执行后最多触发一次），归属天然吻合，不需要新表。
///
/// 旧行留 `NULL`（= 早于本次迁移，或该轮未触发反思）。
fn migrate_v22_add_reflection_columns(conn: &Connection) -> Result<(), rusqlite::Error> {
    // ⚠️ 表可能不存在（迁移测试的最小桩库只建 sessions/app_state），先探表再探列
    let table_exists = conn
        .prepare(
            "SELECT count(*) FROM sqlite_master WHERE type='table' AND name = 'agent_run_events'",
        )
        .and_then(|mut stmt| stmt.query_row([], |row| row.get::<_, i64>(0)))
        .map(|count| count > 0)
        .unwrap_or(false);
    if !table_exists {
        return Ok(());
    }

    for column in [
        "reflection_judgment",
        "reflection_reason",
        "reflection_suggestion",
    ] {
        let column_exists = {
            let mut stmt = conn.prepare("PRAGMA table_info(agent_run_events)")?;
            let columns: Vec<String> = stmt
                .query_map([], |row| row.get::<_, String>(1))?
                .filter_map(Result::ok)
                .collect();
            columns.iter().any(|c| c == column)
        };
        if !column_exists {
            conn.execute(
                &format!("ALTER TABLE agent_run_events ADD COLUMN {} TEXT", column),
                [],
            )?;
        }
    }
    Ok(())
}

/// v23：把老数据的 `interrupt_kind` 从正文标记文本里一次性回填。
///
/// ## 为什么需要它
///
/// 阶段二（2026-09-20）之前，中断原因是以**标记文本**形式拼进助手正文的
/// （`> ⚠️ **[回复被中断]** …` / `**[规划探索已到上限]** …` / `> ✕ **用户已取消执行…**`），
/// `interrupt_kind` 列在那之后才成为唯一载体。老行因此 kind 为 NULL，
/// 渲染侧只能靠 `interrupted_notice_text()` **从正文里清洗出**小字说明 ——
/// 典型的"从内容反推元信息"，与 `source` 字段那次是同一类问题。
///
/// 本次把老行按标记文本认出来、写回 kind，那条清洗路径随之删除。
///
/// ## 认不出的怎么办
///
/// 保持 NULL。渲染侧对 kind 缺失的 `interrupted` 消息**整轮跳过** ——
/// 宁可少显示一轮，也不把系统标记当模型正文渲染（那正是历史 bug）。
/// 回填只认下面三类**确定的**特征串，不做模糊猜测。
///
/// ⚠️ 这是一次性数据搬迁，**不是长期内容匹配规则**：跑完这一遍之后，
/// 判定依据只有 `interrupt_kind` 列本身。新代码不该再往这里加条件。
fn migrate_v23_backfill_interrupt_kind(conn: &Connection) -> Result<(), rusqlite::Error> {
    // ⚠️ 表可能不存在（迁移测试的最小桩库只建 sessions/app_state），先探表再探列
    let table_exists = conn
        .prepare(
            "SELECT count(*) FROM sqlite_master WHERE type='table' AND name = 'session_messages'",
        )
        .and_then(|mut stmt| stmt.query_row([], |row| row.get::<_, i64>(0)))
        .map(|count| count > 0)
        .unwrap_or(false);
    if !table_exists {
        return Ok(());
    }
    let has_kind = {
        let mut stmt = conn.prepare("PRAGMA table_info(session_messages)")?;
        let columns: Vec<String> = stmt
            .query_map([], |row| row.get::<_, String>(1))?
            .filter_map(Result::ok)
            .collect();
        columns.iter().any(|c| c == "interrupt_kind")
    };
    if !has_kind {
        return Ok(());
    }

    // 顺序敏感：先挑特征最强的，再兜泛化的。
    //
    // 1) 应用关闭占位（`INTERRUPT_PLACEHOLDER_NO_REPLY`）—— 特征唯一
    // 2) 规划上限（`PLAN_LIMIT_MARKER`）—— 特征唯一
    // 3) 用户取消（旧格式 `> ✕ **用户已取消执行…**`）—— 特征唯一
    // 4) 上面三条都不是、但带 `[回复被中断]` 的：这是**共用措辞**的
    //    "可续跑"三类（流超时 / 用户取消 / 执行报错），文本上无法区分。
    //    统一按 `pipeline_error` 回填 —— 它的文案是泛化的"本轮执行中断"，
    //    对这三种情形都成立；另两个变体的文案更具体，猜错反而说错。
    for (needle, kind) in [
        ("本次执行因应用关闭而中断", "app_closed"),
        ("[规划探索已到上限]", "plan_limit"),
        ("用户已取消执行", "user_cancel"),
        ("[回复被中断]", "pipeline_error"),
    ] {
        conn.execute(
            "UPDATE session_messages SET interrupt_kind = ?1
             WHERE interrupt_kind IS NULL
               AND source = 'interrupted'
               AND content_json LIKE '%' || ?2 || '%'",
            rusqlite::params![kind, needle],
        )?;
    }
    Ok(())
}

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
        CREATE INDEX IF NOT EXISTS idx_agent_runs_session_started ON agent_runs(session_id, started_at DESC);
        CREATE INDEX IF NOT EXISTS idx_agent_run_events_run_loop ON agent_run_events(run_id, loop_index);
        CREATE INDEX IF NOT EXISTS idx_agent_run_events_session_time ON agent_run_events(session_id, started_at DESC);
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
        // v10 形态的 sessions 表（无 thinking_mode，但 **有 project_id**——
        // project_id 正是 v10 迁引入的，任何真实的 v10 库都带这一列）
        conn.execute_batch(
            "CREATE TABLE sessions (
                id TEXT PRIMARY KEY,
                title TEXT NOT NULL,
                created_at INTEGER NOT NULL,
                updated_at INTEGER NOT NULL,
                message_count INTEGER NOT NULL,
                profile_id TEXT,
                deleted_at INTEGER,
                project_id TEXT
            );
            CREATE TABLE app_state (key TEXT PRIMARY KEY, value TEXT NOT NULL);
            INSERT INTO app_state(key, value) VALUES('schema_version', '10');
            INSERT INTO sessions(id, title, created_at, updated_at, message_count, profile_id, deleted_at)
                VALUES('s1', '老会话', 1, 1, 0, 'default', NULL);",
        )
        .expect("create legacy schema");

        assert!(!column_exists(&conn, "sessions", "thinking_mode"));

        init_schema(&conn).expect("upgrade v10 -> latest");

        assert!(
            column_exists(&conn, "sessions", "thinking_mode"),
            "v11 迁移必须补上 thinking_mode 列"
        );
        // 注意：init_schema 会一路升到最新版（含 v14），因此这里断言的是**最终形态**——
        // 布尔列，且老行的 NULL 被 v14 按保守口径转成 0。
        assert_eq!(
            column_type(&conn, "sessions", "thinking_mode"),
            "INTEGER",
            "升级到最新版后 thinking_mode 必须是布尔列（v14）"
        );
        let value: i64 = conn
            .query_row(
                "SELECT thinking_mode FROM sessions WHERE id = 's1'",
                [],
                |r| r.get(0),
            )
            .expect("read legacy row");
        assert_eq!(value, 0, "v10 老会话无有效表态，按保守口径转为关闭");

        // 版本号至少推进到 12（后续新增迁移会继续推进，不硬编码最新版）
        let version: i64 = conn
            .query_row(
                "SELECT CAST(value AS INTEGER) FROM app_state WHERE key = 'schema_version'",
                [],
                |r| r.get(0),
            )
            .expect("read version");
        assert!(
            version >= 12,
            "v11 升级后版本号至少到 12（实际 {}）",
            version
        );

        // 幂等：再次初始化不报错，且不破坏已有值
        conn.execute("UPDATE sessions SET thinking_mode = 1 WHERE id = 's1'", [])
            .expect("set mode");
        init_schema(&conn).expect("re-init idempotent");
        let after: i64 = conn
            .query_row(
                "SELECT thinking_mode FROM sessions WHERE id = 's1'",
                [],
                |r| r.get(0),
            )
            .expect("re-read");
        assert_eq!(after, 1, "重复初始化不得清掉用户表态");
    }

    /// 建表 DDL 必须自带 thinking_mode，且是**布尔（INTEGER）**列
    /// （v14 起会话档位不再有 NULL，保证新库与升级后的老库同构）
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
                thinking_mode INTEGER NOT NULL DEFAULT 0
            )",
            [],
        )
        .expect("create sessions");
        assert!(column_exists(&conn, "sessions", "thinking_mode"));
        assert_eq!(
            column_type(&conn, "sessions", "thinking_mode"),
            "INTEGER",
            "v14 起 thinking_mode 必须是布尔列（INTEGER），不再是 TEXT"
        );
    }

    /// v13 老库（thinking_mode 还是 TEXT 三态）升级到 v14：
    /// 明确开启的值转 1，其余（never / NULL / 脏值）一律转 0，且列为布尔。
    #[test]
    fn v14_migration_converts_thinking_mode_to_bool() {
        let conn = Connection::open_in_memory().expect("open memory db");
        conn.execute_batch(
            "CREATE TABLE sessions (
                id TEXT PRIMARY KEY,
                title TEXT NOT NULL,
                created_at INTEGER NOT NULL,
                updated_at INTEGER NOT NULL,
                message_count INTEGER NOT NULL,
                profile_id TEXT,
                deleted_at INTEGER,
                thinking_mode TEXT,
                work_mode TEXT,
                approval_mode TEXT,
                agent_audience TEXT,
                project_id TEXT
            );
            CREATE TABLE app_state (key TEXT PRIMARY KEY, value TEXT NOT NULL);
            INSERT INTO app_state(key, value) VALUES('schema_version', '13');
            INSERT INTO sessions(id, title, created_at, updated_at, message_count, thinking_mode)
                VALUES('s_on', '开的会话', 1, 1, 0, 'always');
            INSERT INTO sessions(id, title, created_at, updated_at, message_count, thinking_mode)
                VALUES('s_off', '关的会话', 1, 1, 0, 'never');
            INSERT INTO sessions(id, title, created_at, updated_at, message_count, thinking_mode)
                VALUES('s_null', '未表态会话', 1, 1, 0, NULL);
            INSERT INTO sessions(id, title, created_at, updated_at, message_count, thinking_mode)
                VALUES('s_dirty', '脏值会话', 1, 1, 0, 'bogus');",
        )
        .expect("create legacy schema");

        init_schema(&conn).expect("upgrade v13 -> v14");

        assert_eq!(
            column_type(&conn, "sessions", "thinking_mode"),
            "INTEGER",
            "v14 迁移后该列必须是布尔"
        );
        let read = |id: &str| -> i64 {
            conn.query_row(
                "SELECT thinking_mode FROM sessions WHERE id = ?1",
                [id],
                |r| r.get(0),
            )
            .expect("read thinking_mode")
        };
        assert_eq!(read("s_on"), 1, "'always' 应转成 1");
        assert_eq!(read("s_off"), 0, "'never' 应转成 0");
        assert_eq!(read("s_null"), 0, "NULL 按保守口径转成 0");
        assert_eq!(read("s_dirty"), 0, "脏值按保守口径转成 0");
    }

    /// 迁移幂等：已经是布尔形态的库，重复初始化不得改动用户值。
    #[test]
    fn v14_migration_is_idempotent() {
        let conn = Connection::open_in_memory().expect("open memory db");
        init_schema(&conn).expect("fresh schema");
        conn.execute(
            "INSERT INTO sessions(id, title, created_at, updated_at, message_count, thinking_mode)
             VALUES('s1', '会话', 1, 1, 0, 1)",
            [],
        )
        .expect("insert session");

        init_schema(&conn).expect("re-init");

        let value: i64 = conn
            .query_row(
                "SELECT thinking_mode FROM sessions WHERE id = 's1'",
                [],
                |r| r.get(0),
            )
            .expect("read thinking_mode");
        assert_eq!(value, 1, "重复初始化不得清掉用户表态");
    }

    /// 读取某列的声明类型（用于断言迁移后的列类型）。
    fn column_type(conn: &Connection, table: &str, column: &str) -> String {
        let mut stmt = conn
            .prepare(&format!("PRAGMA table_info({})", table))
            .expect("prepare table_info");
        let rows: Vec<(String, String)> = stmt
            .query_map([], |row| {
                Ok((row.get::<_, String>(1)?, row.get::<_, String>(2)?))
            })
            .expect("query table_info")
            .filter_map(Result::ok)
            .collect();
        rows.into_iter()
            .find(|(name, _)| name == column)
            .map(|(_, ty)| ty.to_uppercase())
            .unwrap_or_default()
    }

    /// v11 老库升级到 v12：补上两列缓存累计，且老行一律留在 0。
    ///
    /// 老行必须留在 0 而不是被回填：0 在展示层等价于"未报告"（显示 `--`），
    /// 回填任何具体值都等于给用户编一个假的命中率。
    #[test]
    fn v12_migration_adds_session_cache_tokens_to_legacy_db() {
        let conn = Connection::open_in_memory().expect("open memory db");
        // v11 形态的 sessions 表（无缓存两列，但 **有 project_id**——v10 起就存在）
        conn.execute_batch(
            "CREATE TABLE sessions (
                id TEXT PRIMARY KEY,
                title TEXT NOT NULL,
                created_at INTEGER NOT NULL,
                updated_at INTEGER NOT NULL,
                message_count INTEGER NOT NULL,
                profile_id TEXT,
                deleted_at INTEGER,
                thinking_mode TEXT,
                project_id TEXT
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

    /// v12 老库升级到 v13：补上工作模式/权限档位/用户类型三列，且老行保持 NULL。
    ///
    /// 老行必须留在 NULL 而不是被回填：NULL 等价于"用户在该会话从未表态"，
    /// 恢复会话时回落到设置默认值；回填任何具体值都等于替用户做了选择。
    #[test]
    fn v13_migration_adds_session_runtime_prefs_to_legacy_db() {
        let conn = Connection::open_in_memory().expect("open memory db");
        // v12 形态的 sessions 表（有 thinking_mode 与缓存两列，无三列运行时偏好）
        conn.execute_batch(
            "CREATE TABLE sessions (
                id TEXT PRIMARY KEY,
                title TEXT NOT NULL,
                created_at INTEGER NOT NULL,
                updated_at INTEGER NOT NULL,
                message_count INTEGER NOT NULL,
                profile_id TEXT,
                deleted_at INTEGER,
                thinking_mode TEXT,
                total_cache_hit_tokens INTEGER NOT NULL DEFAULT 0,
                total_cache_miss_tokens INTEGER NOT NULL DEFAULT 0,
                project_id TEXT
            );
            CREATE TABLE app_state (key TEXT PRIMARY KEY, value TEXT NOT NULL);
            INSERT INTO app_state(key, value) VALUES('schema_version', '12');
            INSERT INTO sessions(id, title, created_at, updated_at, message_count, profile_id, deleted_at, thinking_mode, total_cache_hit_tokens, total_cache_miss_tokens)
                VALUES('s1', '老会话', 1, 1, 0, 'default', NULL, NULL, 0, 0);",
        )
        .expect("create legacy schema");

        assert!(!column_exists(&conn, "sessions", "work_mode"));
        assert!(!column_exists(&conn, "sessions", "approval_mode"));
        assert!(!column_exists(&conn, "sessions", "agent_audience"));

        init_schema(&conn).expect("upgrade v12 -> v13");

        assert!(
            column_exists(&conn, "sessions", "work_mode"),
            "v13 迁移必须补上 work_mode 列"
        );
        assert!(
            column_exists(&conn, "sessions", "approval_mode"),
            "v13 迁移必须补上 approval_mode 列"
        );
        assert!(
            column_exists(&conn, "sessions", "agent_audience"),
            "v13 迁移必须补上 agent_audience 列"
        );
        // 老行不得被回填：NULL 才等价于"用户从未表态"，恢复时回落设置默认
        let (work, approval, audience): (Option<String>, Option<String>, Option<String>) = conn
            .query_row(
                "SELECT work_mode, approval_mode, agent_audience FROM sessions WHERE id = 's1'",
                [],
                |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
            )
            .expect("read legacy row");
        assert_eq!(
            (work, approval, audience),
            (None, None, None),
            "老会话三列必须保持 NULL，不得替用户选模式"
        );

        // 版本号至少推进到 13（后续迁移会继续推进，不硬编码最新版）
        let version: i64 = conn
            .query_row(
                "SELECT CAST(value AS INTEGER) FROM app_state WHERE key = 'schema_version'",
                [],
                |r| r.get(0),
            )
            .expect("read version");
        assert!(
            version >= 13,
            "v13 升级后版本号至少到 13（实际 {}）",
            version
        );

        // 幂等：再次初始化不报错，且不破坏已有值
        conn.execute_batch(
            "UPDATE sessions SET work_mode = 'plan', approval_mode = 'auto', agent_audience = 'normal' WHERE id = 's1'",
        )
        .expect("set prefs");
        init_schema(&conn).expect("re-init idempotent");
        let after: (Option<String>, Option<String>, Option<String>) = conn
            .query_row(
                "SELECT work_mode, approval_mode, agent_audience FROM sessions WHERE id = 's1'",
                [],
                |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
            )
            .expect("re-read");
        assert_eq!(
            (after.0.as_deref(), after.1.as_deref(), after.2.as_deref()),
            (Some("plan"), Some("auto"), Some("normal")),
            "重复初始化不得清掉用户表态"
        );
    }

    fn table_exists(conn: &Connection, table: &str) -> bool {
        conn.query_row(
            "SELECT count(*) FROM sqlite_master WHERE type='table' AND name=?1",
            [table],
            |r| r.get::<_, i64>(0),
        )
        .map(|c| c > 0)
        .unwrap_or(false)
    }

    /// v18：删掉 `agent_runs.interrupt_kind`（从未接线），但
    /// `session_messages.interrupt_kind` **必须保留**——那才是真正在用的落点。
    #[test]
    fn v18_drops_only_agent_runs_interrupt_kind() {
        let conn = Connection::open_in_memory().expect("open memory db");
        conn.execute_batch(
            "CREATE TABLE agent_runs (
                run_id TEXT PRIMARY KEY,
                session_id TEXT NOT NULL,
                interrupt_kind TEXT
            );
            CREATE TABLE session_messages (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                session_id TEXT NOT NULL,
                interrupt_kind TEXT
            );",
        )
        .expect("build v17-shaped stub");

        migrate_v18_drop_agent_runs_interrupt_kind(&conn).expect("v18 迁移应当成功");

        assert!(
            !column_exists(&conn, "agent_runs", "interrupt_kind"),
            "agent_runs.interrupt_kind 应被删除"
        );
        assert!(
            column_exists(&conn, "session_messages", "interrupt_kind"),
            "session_messages.interrupt_kind 必须保留（唯一在用的落点）"
        );

        // 幂等：重复执行不得报错（列已不存在时直接跳过）
        migrate_v18_drop_agent_runs_interrupt_kind(&conn).expect("重复执行应幂等");
    }

    /// 桩库没有 `agent_runs` 表时不得炸——与 v14 空库、v17 同源的坑。
    #[test]
    fn v18_migration_tolerates_missing_table() {
        let conn = Connection::open_in_memory().expect("open memory db");
        conn.execute_batch("CREATE TABLE sessions (id TEXT PRIMARY KEY);")
            .expect("minimal stub");
        migrate_v18_drop_agent_runs_interrupt_kind(&conn).expect("缺表时应直接返回");
    }

    /// v20：加回 `agent_runs.interrupt_kind`（这次真接线），且必须幂等。
    #[test]
    fn v20_adds_agent_runs_interrupt_kind_idempotently() {
        let conn = Connection::open_in_memory().expect("open memory db");
        conn.execute_batch(
            "CREATE TABLE agent_runs (
                run_id TEXT PRIMARY KEY,
                session_id TEXT NOT NULL,
                summary TEXT
            );",
        )
        .expect("build stub");

        assert!(!column_exists(&conn, "agent_runs", "interrupt_kind"));

        migrate_v20_add_agent_runs_interrupt_kind(&conn).expect("v20 迁移应当成功");
        assert!(
            column_exists(&conn, "agent_runs", "interrupt_kind"),
            "v20 必须加回 agent_runs.interrupt_kind"
        );

        // 幂等：重复执行不得报 duplicate column
        migrate_v20_add_agent_runs_interrupt_kind(&conn).expect("重复执行应幂等");
    }

    /// 桩库没有 `agent_runs` 表时 v20 也不得炸。
    #[test]
    fn v20_migration_tolerates_missing_table() {
        let conn = Connection::open_in_memory().expect("open memory db");
        conn.execute_batch("CREATE TABLE sessions (id TEXT PRIMARY KEY);")
            .expect("minimal stub");
        migrate_v20_add_agent_runs_interrupt_kind(&conn).expect("缺表时应直接返回");
    }

    /// v21 的 source 取值搬迁：`internal` 拆成 `inject` / `placeholder`，
    /// 另两个死值各归其位；`turn_id` 列与索引一并删除，其余数据不得丢。
    #[test]
    fn v21_migration_remaps_sources_and_drops_turn_id() {
        let conn = Connection::open_in_memory().expect("open memory db");
        // v20 形态的库：session_messages 带 turn_id 与旧的 source 取值
        conn.execute_batch(
            "CREATE TABLE session_messages (
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
                interrupt_kind TEXT,
                UNIQUE(session_id, seq)
            );
            CREATE INDEX idx_session_messages_turn ON session_messages(session_id, turn_id, seq);
            INSERT INTO session_messages(session_id, message_id, seq, role, content_json, created_at, source, turn_id) VALUES
                ('s1', 'm1', 0, 'user',      '{\"role\":\"user\",\"content\":\"你好\"}',                 1, 'chat',        'turn-a'),
                ('s1', 'm2', 1, 'assistant', '{\"role\":\"assistant\",\"content\":\"Context compressed.\"}', 2, 'internal', 'turn-a'),
                ('s1', 'm3', 2, 'user',      '{\"role\":\"user\",\"content\":\"请调用 ProposePlan\"}',  3, 'internal',    'turn-a'),
                ('s1', 'm4', 3, 'assistant', '{\"role\":\"assistant\",\"content\":\"后台结果\"}',          4, 'background',  NULL),
                ('s1', 'm5', 4, 'user',      '{\"role\":\"user\",\"content\":\"上下文快照\"}',            5, 'context',     NULL),
                ('s1', 'm6', 5, 'assistant', '{\"role\":\"assistant\",\"content\":\"压缩摘要\"}',          6, 'compact',     NULL);",
        )
        .expect("legacy v20 shape");

        migrate_v21_message_source_and_drop_turn_id(&conn).expect("v21 迁移");

        // 1. source 取值逐条搬迁
        let source_of = |message_id: &str| -> String {
            conn.query_row(
                "SELECT source FROM session_messages WHERE message_id = ?1",
                [message_id],
                |r| r.get(0),
            )
            .expect("row must survive")
        };
        assert_eq!(source_of("m1"), "chat", "无关取值不得被动到");
        assert_eq!(
            source_of("m2"),
            "placeholder",
            "`Context compressed.` 是对齐填充，必须迁到 placeholder 而不是 inject"
        );
        assert_eq!(
            source_of("m3"),
            "inject",
            "其余 internal 是写给模型看的注入，必须迁到 inject —— 这是 A 类缺陷的修复点"
        );
        assert_eq!(
            source_of("m4"),
            "placeholder",
            "background 旧口径两侧都不可见"
        );
        assert_eq!(
            source_of("m5"),
            "inject",
            "context 旧口径模型可见、界面不可见"
        );
        assert_eq!(source_of("m6"), "compact", "无关取值不得被动到");

        // 2. turn_id 列与索引都消失
        assert!(
            !column_exists(&conn, "session_messages", "turn_id"),
            "v21 必须删除 session_messages.turn_id"
        );
        let index_gone: bool = conn
            .query_row(
                "SELECT count(*) FROM sqlite_master WHERE type='index' AND name='idx_session_messages_turn'",
                [],
                |r| r.get::<_, i64>(0),
            )
            .map(|c| c == 0)
            .unwrap_or(false);
        assert!(index_gone, "v21 必须删除 idx_session_messages_turn");

        // 3. 其余列与行数完整保留
        let rows: i64 = conn
            .query_row("SELECT COUNT(*) FROM session_messages", [], |r| r.get(0))
            .unwrap();
        assert_eq!(rows, 6, "迁移不得丢行");
        let kind: Option<String> = conn
            .query_row(
                "SELECT interrupt_kind FROM session_messages WHERE message_id = 'm1'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(kind, None, "其余列仍在（示例行为 NULL）");
    }

    /// v21 必须幂等：重复跑不报错（迁移被无差别重跑是历史事故，见 init_schema 的 CAST 注释）
    #[test]
    fn v21_migration_is_idempotent() {
        let conn = Connection::open_in_memory().expect("open memory db");
        conn.execute_batch(
            "CREATE TABLE session_messages (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                session_id TEXT NOT NULL, message_id TEXT, seq INTEGER NOT NULL,
                role TEXT NOT NULL, content_json TEXT NOT NULL, created_at INTEGER NOT NULL,
                source TEXT NOT NULL DEFAULT 'chat', turn_id TEXT, interrupt_kind TEXT,
                UNIQUE(session_id, seq)
            );
            INSERT INTO session_messages(session_id, message_id, seq, role, content_json, created_at, source)
                VALUES ('s1','m1',0,'user','{}',1,'internal');",
        )
        .expect("legacy shape");

        migrate_v21_message_source_and_drop_turn_id(&conn).expect("第一次");
        migrate_v21_message_source_and_drop_turn_id(&conn).expect("第二次必须同样成功");
        let source: String = conn
            .query_row(
                "SELECT source FROM session_messages WHERE message_id='m1'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(source, "inject");
    }

    #[test]
    fn v21_migration_tolerates_missing_table() {
        let conn = Connection::open_in_memory().expect("open memory db");
        conn.execute_batch("CREATE TABLE sessions (id TEXT PRIMARY KEY);")
            .expect("minimal stub");
        migrate_v21_message_source_and_drop_turn_id(&conn).expect("缺表时应直接返回");
    }

    /// v22：`agent_run_events` 加反思三列，旧行留 NULL
    #[test]
    fn v22_migration_adds_reflection_columns() {
        let conn = Connection::open_in_memory().expect("open memory db");
        conn.execute_batch(
            "CREATE TABLE agent_run_events (
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
                started_at INTEGER NOT NULL,
                updated_at INTEGER NOT NULL,
                UNIQUE(run_id, loop_index)
            );
            INSERT INTO agent_run_events(event_id, run_id, session_id, loop_index, status, started_at, updated_at)
                VALUES ('e1','r1','s1',1,'complete',1,1);",
        )
        .expect("legacy shape");

        migrate_v22_add_reflection_columns(&conn).expect("v22 迁移");

        for col in [
            "reflection_judgment",
            "reflection_reason",
            "reflection_suggestion",
        ] {
            assert!(
                column_exists(&conn, "agent_run_events", col),
                "v22 必须补上 agent_run_events.{}",
                col
            );
        }
        let judgment: Option<String> = conn
            .query_row(
                "SELECT reflection_judgment FROM agent_run_events WHERE event_id = 'e1'",
                [],
                |r| r.get(0),
            )
            .expect("旧行必须保留");
        assert_eq!(
            judgment, None,
            "旧行留 NULL（= 早于本次迁移或该轮未触发反思）"
        );

        // 幂等
        migrate_v22_add_reflection_columns(&conn).expect("第二次必须同样成功");
    }

    #[test]
    fn v22_migration_tolerates_missing_table() {
        let conn = Connection::open_in_memory().expect("open memory db");
        conn.execute_batch("CREATE TABLE sessions (id TEXT PRIMARY KEY);")
            .expect("minimal stub");
        migrate_v22_add_reflection_columns(&conn).expect("缺表时应直接返回");
    }

    /// v14 老库升级到 v15：`agent_run_events` 改为每轮一行、`agent_runs` 去掉三个 live 列、
    /// `agent_run_checkpoints` 被删除，且 `agent_runs` 的其余列**完整保留**（不得丢数据）。
    #[test]
    fn v15_migration_rebuilds_events_and_drops_live_columns() {
        let conn = Connection::open_in_memory().expect("open memory db");
        // v14 形态的库：agent_runs 带三个 live 列 + 旧结构 events + checkpoints
        conn.execute_batch(
            "CREATE TABLE sessions (
                id TEXT PRIMARY KEY,
                title TEXT NOT NULL,
                created_at INTEGER NOT NULL,
                updated_at INTEGER NOT NULL,
                message_count INTEGER NOT NULL,
                profile_id TEXT,
                deleted_at INTEGER,
                thinking_mode INTEGER NOT NULL DEFAULT 0,
                work_mode TEXT,
                approval_mode TEXT,
                agent_audience TEXT,
                project_id TEXT
            );
            CREATE TABLE agent_runs (
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
                resumed_from_run_id TEXT
            );
            CREATE TABLE agent_run_events (
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
                model TEXT
            );
            CREATE TABLE agent_run_checkpoints (
                run_id TEXT PRIMARY KEY,
                session_id TEXT NOT NULL,
                loop_count INTEGER NOT NULL,
                messages_json TEXT NOT NULL,
                input_tokens INTEGER NOT NULL,
                output_tokens INTEGER NOT NULL,
                last_safe_point TEXT NOT NULL,
                updated_at INTEGER NOT NULL
            );
            CREATE TABLE app_state (key TEXT PRIMARY KEY, value TEXT NOT NULL);
            INSERT INTO app_state(key, value) VALUES('schema_version', '14');
            INSERT INTO sessions(id, title, created_at, updated_at, message_count)
                VALUES('s1', '老会话', 1, 1, 0);
            INSERT INTO agent_runs(run_id, session_id, status, user_message_preview, message_id,
                loop_count, input_tokens, output_tokens, started_at, updated_at, finished_at,
                last_safe_point, live_thinking, live_tool_buffer, live_content, error, summary,
                resumable, resumed_from_run_id)
                VALUES('ar_1', 's1', 'completed', '预览', 'msg_1',
                3, 1200, 340, 100, 200, 300,
                '模型已给出最终回复', '思考', '工具日志', '正文', NULL, '完成',
                0, NULL);
            INSERT INTO agent_run_events(event_id, run_id, session_id, event_type, message,
                tool, input_summary, output_summary, error, loop_count, input_tokens,
                output_tokens, timestamp, model)
                VALUES('are_1', 'ar_1', 's1', 'start', '主 Agent 开始执行', NULL, NULL, NULL,
                NULL, 0, 0, 0, 100, NULL);
            INSERT INTO agent_run_checkpoints(run_id, session_id, loop_count, messages_json,
                input_tokens, output_tokens, last_safe_point, updated_at)
                VALUES('ar_1', 's1', 3, '[]', 1200, 340, '模型已给出最终回复', 200);",
        )
        .expect("create legacy v14 schema");

        assert!(column_exists(&conn, "agent_runs", "live_content"));

        init_schema(&conn).expect("upgrade v14 -> v15");

        // 1. 三个 live 列消失
        for col in ["live_content", "live_thinking", "live_tool_buffer"] {
            assert!(
                !column_exists(&conn, "agent_runs", col),
                "v15 迁移必须删除 agent_runs.{}",
                col
            );
        }
        // 1b. 本测试跑的是**完整升级链**（到最新版），故 v19 的两个删除也应生效
        for col in ["user_message_preview", "error"] {
            assert!(
                !column_exists(&conn, "agent_runs", col),
                "v19 迁移必须删除 agent_runs.{}",
                col
            );
        }
        // 2. 其余列完整保留（不得丢数据）
        // ⚠️ `user_message_preview` 已在 v19 被删除（只写不读），故不再纳入本断言。
        let (status, loops, in_tok, out_tok, resumable): (String, i64, i64, i64, i64) = conn
            .query_row(
                // ⚠️ 不再选 user_message_preview：该列在 v19 被删除（只写不读）。
                // 本断言要验证的是"其余列完整搬移"，故只检查仍然存在的列。
                "SELECT status, loop_count, input_tokens, output_tokens, resumable
                 FROM agent_runs WHERE run_id = 'ar_1'",
                [],
                |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?, r.get(4)?)),
            )
            .expect("legacy run row must survive");
        assert_eq!(status, "completed");
        assert_eq!(loops, 3);
        assert_eq!((in_tok, out_tok, resumable), (1200, 340, 0));

        // 3. events 表重建为目标结构
        assert!(
            column_exists(&conn, "agent_run_events", "loop_index")
                && column_exists(&conn, "agent_run_events", "resp_blocks")
                && column_exists(&conn, "agent_run_events", "tool_results"),
            "v15 迁移后 events 表必须具备新结构列"
        );
        assert!(
            !column_exists(&conn, "agent_run_events", "event_type"),
            "旧 events 列应随重建表消失"
        );
        let old_rows: i64 = conn
            .query_row("SELECT count(*) FROM agent_run_events", [], |r| r.get(0))
            .expect("count events");
        assert_eq!(old_rows, 0, "旧 events 是文案转储，无保留价值，应直接丢弃");

        // 4. checkpoints 表被删除
        assert!(
            !table_exists(&conn, "agent_run_checkpoints"),
            "v15 迁移必须删除 agent_run_checkpoints"
        );

        // 5. 版本号推进
        let version: i64 = conn
            .query_row(
                "SELECT CAST(value AS INTEGER) FROM app_state WHERE key = 'schema_version'",
                [],
                |r| r.get(0),
            )
            .expect("read version");
        assert!(
            version >= 15,
            "v15 升级后版本号至少到 15（实际 {}）",
            version
        );

        // 6. 幂等：重复初始化不报错、不破坏数据
        init_schema(&conn).expect("re-init idempotent");
        // 用仍然存在的列验证（user_message_preview 已在 v19 删除）
        let after: String = conn
            .query_row(
                "SELECT status FROM agent_runs WHERE run_id = 'ar_1'",
                [],
                |r| r.get(0),
            )
            .expect("re-read run");
        assert_eq!(after, "completed", "重复初始化不得丢 run 数据");
    }

    /// v16：把存量秒级时间戳统一放大为毫秒（含 JSON 内嵌字段），且重复初始化不二次放大。
    #[test]
    fn v16_migration_scales_timestamps_to_millis() {
        let conn = Connection::open_in_memory().expect("open memory db");

        // 先用**当前 DDL** 建出完整表结构（避免手写桩漏列——v10+ 的索引会引用
        // sessions.profile_id / session_messages.turn_id 等列，桩缺列会导致建索引失败），
        // 再把版本号改回 15、塞入秒级数据，模拟"真实的 v15 库"。
        init_schema(&conn).expect("create fresh schema");
        conn.execute(
            "UPDATE app_state SET value = '15' WHERE key = 'schema_version'",
            [],
        )
        .expect("fake v15");

        conn.execute_batch(
            "INSERT INTO sessions(id, title, created_at, updated_at, message_count)
                VALUES('s1', '旧会话', 1700000000, 1700000100, 2);

             INSERT INTO session_messages(session_id, message_id, seq, role, content_json,
                created_at, updated_at, hidden_at)
                VALUES('s1', 'm1', 0, 'user', '{}', 1700000000, 1700000050, 1700000090);

             INSERT INTO snapshot_trees(session_id, tree_json, updated_at) VALUES(
                's1',
                '{\"nodes\":{\"snap_1\":{\"createdAt\":1700000000}},\
                  \"branches\":{\"main\":{\"createdAt\":1699000000}}}',
                1700000000);

             INSERT INTO snapshot_journal(session_id, event_json, created_at)
                VALUES('s1', '{\"type\":\"create_snapshot\",\"id\":\"snap_1\",\
                              \"timestamp\":1700000000}', 1700000000);

             INSERT INTO session_memory(session_id, memory_json) VALUES(
                's1',
                '{\"plan_documents\":[{\"createdAt\":1700000000,\"updatedAt\":1700000050,\
                                       \"decidedAt\":null}]}');",
        )
        .expect("insert legacy seconds data");

        init_schema(&conn).expect("upgrade v15 -> v16");

        // 1. 纯整数列 ×1000
        let (created, updated): (i64, i64) = conn
            .query_row(
                "SELECT created_at, updated_at FROM sessions WHERE id = 's1'",
                [],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .expect("read sessions");
        assert_eq!(created, 1_700_000_000_000, "sessions.created_at 应为毫秒");
        assert_eq!(updated, 1_700_000_100_000, "sessions.updated_at 应为毫秒");

        let (msg_created, hidden): (i64, i64) = conn
            .query_row(
                "SELECT created_at, hidden_at FROM session_messages WHERE message_id = 'm1'",
                [],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .expect("read session_messages");
        assert_eq!(msg_created, 1_700_000_000_000, "消息 created_at 应为毫秒");
        assert_eq!(hidden, 1_700_000_090_000, "hidden_at 应为毫秒");

        // 2. JSON 内嵌字段 ×1000
        let tree_json: String = conn
            .query_row(
                "SELECT tree_json FROM snapshot_trees WHERE session_id = 's1'",
                [],
                |r| r.get(0),
            )
            .expect("read tree");
        let tree: serde_json::Value = serde_json::from_str(&tree_json).expect("parse tree");
        assert_eq!(
            tree["nodes"]["snap_1"]["createdAt"],
            serde_json::json!(1_700_000_000_000i64),
            "快照节点 createdAt 应为毫秒"
        );
        assert_eq!(
            tree["branches"]["main"]["createdAt"],
            serde_json::json!(1_699_000_000_000i64),
            "分支 createdAt 应为毫秒"
        );

        let event_json: String = conn
            .query_row("SELECT event_json FROM snapshot_journal", [], |r| r.get(0))
            .expect("read journal");
        let event: serde_json::Value = serde_json::from_str(&event_json).expect("parse journal");
        assert_eq!(
            event["timestamp"],
            serde_json::json!(1_700_000_000_000i64),
            "日志事件 timestamp 应为毫秒"
        );

        let memory_json: String = conn
            .query_row(
                "SELECT memory_json FROM session_memory WHERE session_id = 's1'",
                [],
                |r| r.get(0),
            )
            .expect("read memory");
        let memory: serde_json::Value = serde_json::from_str(&memory_json).expect("parse memory");
        assert_eq!(
            memory["plan_documents"][0]["createdAt"],
            serde_json::json!(1_700_000_000_000i64),
            "方案文档 createdAt 应为毫秒"
        );
        assert_eq!(
            memory["plan_documents"][0]["updatedAt"],
            serde_json::json!(1_700_000_050_000i64),
            "方案文档 updatedAt 应为毫秒"
        );
        // null 的时间字段保持 null，不得被写成 0
        assert!(
            memory["plan_documents"][0]["decidedAt"].is_null(),
            "decidedAt 为 null 时不得被改写"
        );

        // 3. 版本号推进
        let version: i64 = conn
            .query_row(
                "SELECT CAST(value AS INTEGER) FROM app_state WHERE key = 'schema_version'",
                [],
                |r| r.get(0),
            )
            .expect("read version");
        assert!(
            version >= 16,
            "v16 升级后版本号至少到 16（实际 {}）",
            version
        );

        // 4. 幂等：重复初始化不得把毫秒再放大一次（秒级阈值守卫）
        init_schema(&conn).expect("re-init idempotent");
        let created_again: i64 = conn
            .query_row("SELECT created_at FROM sessions WHERE id = 's1'", [], |r| {
                r.get(0)
            })
            .expect("re-read sessions");
        assert_eq!(
            created_again, 1_700_000_000_000,
            "重复初始化不得二次放大时间戳"
        );
        let tree_again: String = conn
            .query_row(
                "SELECT tree_json FROM snapshot_trees WHERE session_id = 's1'",
                [],
                |r| r.get(0),
            )
            .expect("re-read tree");
        let tree_again_value: serde_json::Value =
            serde_json::from_str(&tree_again).expect("parse tree again");
        assert_eq!(
            tree_again_value["nodes"]["snap_1"]["createdAt"],
            serde_json::json!(1_700_000_000_000i64),
            "重复初始化不得二次放大 JSON 内时间字段"
        );
    }
}
