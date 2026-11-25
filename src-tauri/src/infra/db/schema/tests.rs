//! # schema/tests.rs — SQLite schema 初始化与历史迁移回归测试
//!
//! 覆盖从早期 schema 版本升级到最新版本的幂等性、列迁移与数据回填验证。
//!
//! ## Key Exports
//! - 无（仅 `#[cfg(test)]` 单元测试）
//!
//! ## Dependencies
//! - Internal: `super` (`schema`)
//! - External: `rusqlite`
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

/// v26：v25 老库（`agent_runs` 无 `wait_ms`）升级后必须补上可空列，
/// 且旧行留 NULL（= 未统计等待，不得回填 0 伪装成“没等过”）。
#[test]
fn v26_migration_adds_agent_runs_wait_ms_nullable() {
    let conn = Connection::open_in_memory().expect("open memory db");
    // v25 形态的 agent_runs（除 wait_ms 外列齐全）：
    // 必须带 session_id / started_at —— 建表 DDL 里的索引会引用它们。
    conn.execute_batch(
        "CREATE TABLE agent_runs (
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
            interrupt_kind TEXT
        );
        CREATE TABLE app_state (key TEXT PRIMARY KEY, value TEXT NOT NULL);
        INSERT INTO app_state(key, value) VALUES('schema_version', '25');
        INSERT INTO agent_runs(run_id, session_id, status, loop_count, input_tokens,
            output_tokens, started_at, updated_at)
            VALUES('ar_old', 's1', 'completed', 2, 100, 50, 1, 2);",
    )
    .expect("create legacy v25 schema");

    assert!(
        !column_exists(&conn, "agent_runs", "wait_ms"),
        "前提：v25 库不应有 wait_ms 列"
    );

    init_schema(&conn).expect("upgrade v25 -> v26");

    assert!(
        column_exists(&conn, "agent_runs", "wait_ms"),
        "v26 迁移必须补上 agent_runs.wait_ms"
    );
    // 旧行不得被回填：NULL 才能表达“未统计等待”
    let wait: Option<i64> = conn
        .query_row(
            "SELECT wait_ms FROM agent_runs WHERE run_id = 'ar_old'",
            [],
            |r| r.get(0),
        )
        .expect("旧行必须保留");
    assert_eq!(wait, None, "旧行 wait_ms 必须保持 NULL（未统计等待）");

    // 版本号推进到 26
    let version: i64 = conn
        .query_row(
            "SELECT CAST(value AS INTEGER) FROM app_state WHERE key = 'schema_version'",
            [],
            |r| r.get(0),
        )
        .expect("read version");
    assert!(version >= 26, "v26 升级后版本号至少到 26（实际 {}）", version);

    // 幂等：再次初始化不得报 duplicate column，且不破坏已有值
    conn.execute("UPDATE agent_runs SET wait_ms = 1234 WHERE run_id = 'ar_old'", [])
        .expect("set wait_ms");
    init_schema(&conn).expect("re-init idempotent");
    let after: Option<i64> = conn
        .query_row(
            "SELECT wait_ms FROM agent_runs WHERE run_id = 'ar_old'",
            [],
            |r| r.get(0),
        )
        .expect("re-read");
    assert_eq!(after, Some(1234), "重复初始化不得清掉 wait_ms");
}

/// v26 的建表 DDL 必须自带 `wait_ms`（新库不跑迁移也要与升级后同构）
#[test]
fn fresh_ddl_declares_agent_runs_wait_ms() {
    let conn = Connection::open_in_memory().expect("open memory db");
    init_schema(&conn).expect("fresh schema");
    assert!(
        column_exists(&conn, "agent_runs", "wait_ms"),
        "新建库的 agent_runs 必须包含 wait_ms"
    );
    assert_eq!(
        column_type(&conn, "agent_runs", "wait_ms"),
        "INTEGER",
        "wait_ms 必须是毫秒整数列"
    );
}

/// 统计某列在表里出现几次：用于证明重复迁移没有靠 `ADD COLUMN` 堆出第二列。
fn column_occurrences(conn: &Connection, table: &str, column: &str) -> usize {
    let mut stmt = conn
        .prepare(&format!("PRAGMA table_info({})", table))
        .expect("prepare table_info");
    stmt.query_map([], |row| row.get::<_, String>(1))
        .expect("query table_info")
        .filter_map(Result::ok)
        .filter(|name| name == column)
        .count()
}

/// v27：迁移测试的最小桩库（只有 `app_state`，**没有 projects 表**）必须放行。
///
/// 真实痛点：测试桩库与早期库都可能缺 `projects`（v10 才引入），
/// 无条件 `ALTER TABLE projects` 会直接炸 "no such table"，
/// 让整条迁移链在新库/桩库上寸步难行。
#[test]
fn v27_migration_tolerates_missing_projects_table() {
    let conn = Connection::open_in_memory().expect("open memory db");
    conn.execute_batch(
        "CREATE TABLE app_state (key TEXT PRIMARY KEY, value TEXT NOT NULL);
         INSERT INTO app_state(key, value) VALUES('schema_version', '26');",
    )
    .expect("create minimal stub db");

    // 1) 直接调用迁移函数：缺表必须返回 Ok
    migrate_v27_project_archive(&conn).expect("缺 projects 表时必须跳过而不是报错");

    // 2) 走真实入口：v27 在建表 DDL **之前**执行，此刻表仍不存在，同样不得报错
    init_schema(&conn).expect("最小桩库升级不得报错");
    assert!(
        column_exists(&conn, "projects", "deleted_at"),
        "升级完成后 projects 必须有 deleted_at 列"
    );
}

/// v27：v26 老库（`projects` 无 `deleted_at`）升级后必须补上可空列，
/// 且已有项目留 NULL（= 从未归档，不得回填任何值伪造归档动作）。
#[test]
fn v27_migration_adds_projects_deleted_at_nullable() {
    let conn = Connection::open_in_memory().expect("open memory db");
    // v26 形态的 projects（无 deleted_at），并预置一条存量项目
    conn.execute_batch(
        "CREATE TABLE projects (
            id TEXT PRIMARY KEY,
            name TEXT NOT NULL,
            path TEXT NOT NULL UNIQUE,
            created_at INTEGER NOT NULL,
            updated_at INTEGER NOT NULL
        );
        CREATE TABLE app_state (key TEXT PRIMARY KEY, value TEXT NOT NULL);
        INSERT INTO app_state(key, value) VALUES('schema_version', '26');
        INSERT INTO projects(id, name, path, created_at, updated_at)
            VALUES('p_old', '老项目', 'C:/tmp/old', 1, 2);",
    )
    .expect("create legacy v26 schema");

    assert!(
        !column_exists(&conn, "projects", "deleted_at"),
        "前提：v26 库不应有 deleted_at 列"
    );

    init_schema(&conn).expect("upgrade v26 -> v27");

    assert!(
        column_exists(&conn, "projects", "deleted_at"),
        "v27 迁移必须补上 projects.deleted_at"
    );
    assert_eq!(
        column_type(&conn, "projects", "deleted_at"),
        "INTEGER",
        "deleted_at 必须是毫秒整数列"
    );
    // 存量行不得被回填：NULL 才能表达「从未归档」
    let archived: Option<i64> = conn
        .query_row(
            "SELECT deleted_at FROM projects WHERE id = 'p_old'",
            [],
            |r| r.get(0),
        )
        .expect("老项目必须保留");
    assert_eq!(archived, None, "存量项目 deleted_at 必须保持 NULL");

    // 版本号推进到 27
    let version: i64 = conn
        .query_row(
            "SELECT CAST(value AS INTEGER) FROM app_state WHERE key = 'schema_version'",
            [],
            |r| r.get(0),
        )
        .expect("read version");
    assert!(
        version >= 27,
        "v27 升级后版本号至少到 27（实际 {}）",
        version
    );

    // 幂等：直接重复调用迁移函数，探列逻辑必须挡住第二次 ADD COLUMN
    // （只测 init_schema 不够——版本号已推进，第二次根本不会走迁移分支）
    migrate_v27_project_archive(&conn).expect("重复迁移不得报 duplicate column");
    migrate_v27_project_archive(&conn).expect("重复迁移仍然幂等");
    assert_eq!(
        column_occurrences(&conn, "projects", "deleted_at"),
        1,
        "重复迁移不得堆出第二个 deleted_at 列"
    );

    // 真实建库入口再跑一遍：不报错，也不清掉归档时间
    conn.execute(
        "UPDATE projects SET deleted_at = 1700000000000 WHERE id = 'p_old'",
        [],
    )
    .expect("set archived");
    init_schema(&conn).expect("re-init idempotent");
    assert_eq!(
        column_occurrences(&conn, "projects", "deleted_at"),
        1,
        "重复初始化不得重复加列"
    );
    let after: Option<i64> = conn
        .query_row(
            "SELECT deleted_at FROM projects WHERE id = 'p_old'",
            [],
            |r| r.get(0),
        )
        .expect("re-read");
    assert_eq!(after, Some(1_700_000_000_000), "重复初始化不得清掉归档时间");
}

/// v27 的建表 DDL 必须自带 `deleted_at`（新库不跑迁移也要与升级后同构）
#[test]
fn fresh_ddl_declares_projects_deleted_at() {
    let conn = Connection::open_in_memory().expect("open memory db");
    init_schema(&conn).expect("fresh schema");
    assert!(
        column_exists(&conn, "projects", "deleted_at"),
        "新建库的 projects 必须包含 deleted_at"
    );
    assert_eq!(
        column_type(&conn, "projects", "deleted_at"),
        "INTEGER",
        "deleted_at 必须是毫秒时间戳列"
    );
}

// ── v28：request_usage 逐请求用量明细 ──

/// 某列是否允许 NULL（`PRAGMA table_info` 第 4 列 notnull：0 = 可空）。
///
/// v28 的核心契约就是「缓存两列**必须可空**」—— 可空才能区分「厂商未上报」与「0」。
fn column_nullable(conn: &Connection, table: &str, column: &str) -> bool {
    let mut stmt = conn
        .prepare(&format!("PRAGMA table_info({})", table))
        .expect("prepare table_info");
    let rows: Vec<(String, i64)> = stmt
        .query_map([], |row| {
            Ok((row.get::<_, String>(1)?, row.get::<_, i64>(3)?))
        })
        .expect("query table_info")
        .filter_map(Result::ok)
        .collect();
    rows.into_iter()
        .find(|(name, _)| name == column)
        .map(|(_, not_null)| not_null == 0)
        .unwrap_or_else(|| panic!("列 {} 不存在于表 {}", column, table))
}

/// 索引是否存在（按名字查 `sqlite_master`）。
fn index_exists(conn: &Connection, index: &str) -> bool {
    conn.query_row(
        "SELECT count(*) FROM sqlite_master WHERE type='index' AND name=?1",
        [index],
        |r| r.get::<_, i64>(0),
    )
    .map(|c| c > 0)
    .unwrap_or(false)
}

/// 空库建表：不跑任何迁移，仅靠建表 DDL 就必须建出 `request_usage` 及其索引。
///
/// 同时钉死 v28 最关键的列契约：缓存两列可空、其余统计列 NOT NULL。
#[test]
fn v28_fresh_db_creates_request_usage_table() {
    let conn = Connection::open_in_memory().expect("open memory db");
    init_schema(&conn).expect("fresh schema");

    assert!(table_exists(&conn, "request_usage"), "新库建表 DDL 必须建出 request_usage");
    for column in [
        "id",
        "session_id",
        "run_id",
        "loop_index",
        "source",
        "parent_tool_use_id",
        "agent_role",
        "task_id",
        "model_id",
        "input_tokens",
        "output_tokens",
        "cache_hit_tokens",
        "cache_miss_tokens",
        "cache_source",
        "created_at",
    ] {
        assert!(
            column_exists(&conn, "request_usage", column),
            "request_usage 缺少列 {column}"
        );
    }

    // 缓存两列可空：NULL = 厂商未上报，必须与 0（上报了就是零）严格区分
    assert!(
        column_nullable(&conn, "request_usage", "cache_hit_tokens"),
        "cache_hit_tokens 必须可空（NULL = 厂商未上报）"
    );
    assert!(
        column_nullable(&conn, "request_usage", "cache_miss_tokens"),
        "cache_miss_tokens 必须可空（NULL = 厂商未上报）"
    );
    // token 主列不可空：没拿到 usage 的路径不应该写明细
    assert!(!column_nullable(&conn, "request_usage", "input_tokens"));
    assert!(!column_nullable(&conn, "request_usage", "output_tokens"));
    assert!(!column_nullable(&conn, "request_usage", "source"));

    assert!(
        index_exists(&conn, "idx_request_usage_session_time"),
        "缺少 (session_id, created_at) 索引，汇总查询会全表扇描"
    );

    // 重复初始化幂等：不报错，且索引不会堆出第二份
    init_schema(&conn).expect("re-init idempotent");
    assert!(table_exists(&conn, "request_usage"));
}

/// v27 老库升级到 v28：必须补出 `request_usage` 表，版本号推进到 28，
/// 且升级后能真正插入一行（外键指向已存在的 sessions，不报约束错）。
#[test]
fn v28_migration_creates_request_usage_on_v27_db() {
    let conn = Connection::open_in_memory().expect("open memory db");
    // v27 形态的最小库：只有 app_state / sessions（request_usage 尚不存在）
    //
    // 注意：桩库必须带被索引引用的列（profile_id / project_id 等），否则
    // init_schema 建索引（idx_sessions_profile_updated / idx_sessions_project）
    // 时会报 no such column。这里的列清单按真实 v27 形态补齐（v14/v17 同源坑）。
    conn.execute_batch(
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
            total_cache_hit_tokens INTEGER NOT NULL DEFAULT 0,
            total_cache_miss_tokens INTEGER NOT NULL DEFAULT 0,
            title_source TEXT NOT NULL DEFAULT 'default',
            project_id TEXT,
            deleted_at INTEGER,
            thinking_mode INTEGER NOT NULL DEFAULT 0,
            work_mode TEXT,
            approval_mode TEXT,
            agent_audience TEXT
        );
        CREATE TABLE app_state (key TEXT PRIMARY KEY, value TEXT NOT NULL);
        INSERT INTO app_state(key, value) VALUES('schema_version', '27');
        INSERT INTO sessions(id, title, created_at, updated_at, message_count, profile_id)
            VALUES('s1', '老会话', 1, 1, 0, 'default');",
    )
    .expect("create legacy v27 schema");

    assert!(!table_exists(&conn, "request_usage"), "前提：v27 库不应有 request_usage");

    init_schema(&conn).expect("upgrade v27 -> v28");

    assert!(
        table_exists(&conn, "request_usage"),
        "v28 迁移必须补出 request_usage 表"
    );

    let version: i64 = conn
        .query_row(
            "SELECT CAST(value AS INTEGER) FROM app_state WHERE key = 'schema_version'",
            [],
            |r| r.get(0),
        )
        .expect("read version");
    assert!(
        version >= 28,
        "v28 升级后版本号至少到 28（实际 {}）",
        version
    );

    // 升级后可用：插一行带 NULL 缓存字段的明细（证明列真的可空）
    conn.execute(
        "INSERT INTO request_usage(session_id, source, input_tokens, output_tokens, created_at)
         VALUES('s1', 'main', 1000, 20, 1700000000000)",
        [],
    )
    .expect("升级后必须能写入明细行");
    let cache: (Option<i64>, Option<i64>) = conn
        .query_row(
            "SELECT cache_hit_tokens, cache_miss_tokens FROM request_usage WHERE session_id = 's1'",
            [],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )
        .expect("read row");
    assert_eq!(cache, (None, None), "未上报的缓存字段必须落 NULL，不得写成 0");

    // 重复初始化：表与数据都不得被动过
    init_schema(&conn).expect("re-init idempotent");
    let count: i64 = conn
        .query_row("SELECT count(*) FROM request_usage", [], |r| r.get(0))
        .expect("count rows");
    assert_eq!(count, 1, "重复初始化不得动明细数据");
}

/// 迁移函数自身幂等：反复调用不得报「table already exists」，也不得堆出第二个索引。
///
/// 只测 `init_schema` 不够——版本号一旦推进，第二次根本不会进迁移分支，
/// 重复执行的幂等性必须直接摁住迁移函数本身才算验过。
#[test]
fn v28_migration_is_repeatable() {
    let conn = Connection::open_in_memory().expect("open memory db");
    conn.execute_batch(
        "CREATE TABLE app_state (key TEXT PRIMARY KEY, value TEXT NOT NULL);
         INSERT INTO app_state(key, value) VALUES('schema_version', '27');",
    )
    .expect("create minimal stub db");

    migrate_v28_request_usage(&conn).expect("首次建表");
    migrate_v28_request_usage(&conn).expect("重复迁移不得报错");
    migrate_v28_request_usage(&conn).expect("重复迁移仍然幂等");

    assert!(table_exists(&conn, "request_usage"));
    assert_eq!(
        column_occurrences(&conn, "request_usage", "cache_hit_tokens"),
        1,
        "重复迁移不得堆出第二个 cache_hit_tokens 列"
    );
    assert!(index_exists(&conn, "idx_request_usage_session_time"));
    let index_count: i64 = conn
        .query_row(
            "SELECT count(*) FROM sqlite_master WHERE type='index' AND name='idx_request_usage_session_time'",
            [],
            |r| r.get(0),
        )
        .expect("count index");
    assert_eq!(index_count, 1, "重复迁移不得堆出第二个同名索引");
}

/// 会话彻底删除时明细必须级联清掉；归档（`deleted_at` 非空）不得删明细。
///
/// 这条契约直接决定「删了会话还留下残余 token 账」这类脏数据是否会发生。
#[test]
fn v28_request_usage_cascades_on_session_delete() {
    let conn = Connection::open_in_memory().expect("open memory db");
    // init_schema 不负责开外键（那是运行期 configure_connection 的事），测试里手动开
    conn.pragma_update(None, "foreign_keys", "ON")
        .expect("enable foreign_keys");
    init_schema(&conn).expect("fresh schema");
    conn.execute_batch(
        "INSERT INTO sessions(id, title, created_at, updated_at, message_count)
            VALUES('s_del', '待删会话', 1, 1, 0);
         INSERT INTO sessions(id, title, created_at, updated_at, message_count, deleted_at)
            VALUES('s_arch', '已归档会话', 1, 1, 0, 1700000000000);
         INSERT INTO request_usage(session_id, source, input_tokens, output_tokens, created_at)
            VALUES('s_del', 'main', 10, 1, 1), ('s_arch', 'main', 20, 2, 2);",
    )
    .expect("seed rows");

    // 归档只是打标记：明细必须留着（归档可撤销，账不能丢）
    conn.execute("UPDATE sessions SET deleted_at = 1700000000001 WHERE id = 's_arch'", [])
        .expect("archive session");
    let arch_rows: i64 = conn
        .query_row(
            "SELECT count(*) FROM request_usage WHERE session_id = 's_arch'",
            [],
            |r| r.get(0),
        )
        .expect("count archived rows");
    assert_eq!(arch_rows, 1, "归档不得清掉明细");

    // 彻底删除：明细必须跟着走
    conn.execute("DELETE FROM sessions WHERE id = 's_del'", [])
        .expect("delete session");
    let del_rows: i64 = conn
        .query_row(
            "SELECT count(*) FROM request_usage WHERE session_id = 's_del'",
            [],
            |r| r.get(0),
        )
        .expect("count deleted rows");
    assert_eq!(del_rows, 0, "会话彻底删除后明细必须级联清掉");
}

/// v29：撤回关联表必须从"内存下标主键"重建为"message_id 主键"。
///
/// 锁三件事：
/// 1. 同一个 message_id 的多条历史行只保留**最早**一条（本轮第一个快照才是撤回锚点）；
/// 2. `message_id` 为空的下标脏数据被丢弃（它们正是误配的来源）；
/// 3. 迁移可重复执行，且新主键确实是 `(session_id, message_id)`。
#[test]
fn v29_checkpoint_links_rebuilds_on_message_id() {
    let conn = Connection::open_in_memory().expect("open memory db");
    conn.execute_batch(
        "CREATE TABLE sessions(id TEXT PRIMARY KEY);
         INSERT INTO sessions(id) VALUES('s1');
         CREATE TABLE checkpoint_user_message_links (
            session_id TEXT NOT NULL,
            user_message_index INTEGER NOT NULL,
            checkpoint_id TEXT NOT NULL,
            has_file_edits INTEGER NOT NULL DEFAULT 0,
            created_at INTEGER NOT NULL,
            message_id TEXT,
            updated_at INTEGER,
            PRIMARY KEY(session_id, user_message_index),
            UNIQUE(session_id, checkpoint_id)
         );
         INSERT INTO checkpoint_user_message_links
            (session_id, user_message_index, checkpoint_id, has_file_edits, created_at, message_id, updated_at)
         VALUES
            ('s1', 2,  'snap_old',   1, 100, 'msg-a', 100),
            ('s1', 6,  'snap_new',   1, 200, 'msg-a', 200),
            ('s1', 10, 'snap_b',     1, 300, 'msg-b', 300),
            ('s1', 12, 'snap_dirty', 1, 400, NULL,    400);",
    )
    .expect("seed legacy links");

    migrate_v29_checkpoint_links_message_id(&conn).expect("首次迁移");
    migrate_v29_checkpoint_links_message_id(&conn).expect("重复迁移必须幂等");

    assert_eq!(
        column_occurrences(&conn, "checkpoint_user_message_links", "user_message_index"),
        0,
        "旧的内存下标列必须消失"
    );
    assert_eq!(
        column_occurrences(&conn, "checkpoint_user_message_links", "message_id"),
        1,
        "message_id 列必须存在且只有一列"
    );

    let rows: Vec<(String, String)> = {
        let mut stmt = conn
            .prepare(
                "SELECT message_id, checkpoint_id FROM checkpoint_user_message_links
                 ORDER BY message_id",
            )
            .expect("prepare links");
        stmt.query_map([], |row| Ok((row.get(0)?, row.get(1)?)))
            .expect("query links")
            .filter_map(Result::ok)
            .collect()
    };
    assert_eq!(
        rows,
        vec![
            ("msg-a".to_string(), "snap_old".to_string()),
            ("msg-b".to_string(), "snap_b".to_string()),
        ],
        "同一消息只保留最早关联；message_id 为空的脏行必须丢弃"
    );

    // 新主键必须是 (session_id, message_id)：同一消息再次写入应覆盖而不是新增行。
    conn.execute(
        "INSERT INTO checkpoint_user_message_links
            (session_id, message_id, checkpoint_id, has_file_edits, created_at, updated_at)
         VALUES('s1', 'msg-a', 'snap_newer', 1, 500, 500)
         ON CONFLICT(session_id, message_id) DO UPDATE SET
            checkpoint_id = excluded.checkpoint_id",
        [],
    )
    .expect("upsert by message_id");
    let checkpoint: String = conn
        .query_row(
            "SELECT checkpoint_id FROM checkpoint_user_message_links
             WHERE session_id = 's1' AND message_id = 'msg-a'",
            [],
            |row| row.get(0),
        )
        .expect("read linked checkpoint");
    assert_eq!(checkpoint, "snap_newer");
    let total: i64 = conn
        .query_row("SELECT count(*) FROM checkpoint_user_message_links", [], |row| {
            row.get(0)
        })
        .expect("count links");
    assert_eq!(total, 2, "按 message_id 覆盖后仍应是两条");
}
