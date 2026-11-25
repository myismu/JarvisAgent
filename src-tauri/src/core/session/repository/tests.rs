//! # repository/tests.rs — 会话仓储集成与列一致性单元测试
//!
//! 覆盖元数据列映射、消息分页与回合边界、回收站归档恢复以及 token 用量明细聚合的 SQLite 实库测试。
//!
//! ## Key Exports
//! - 无（仅单元测试模块）
//!
//! ## Dependencies
//! - Internal: `super` (`repository`), `crate::infra::db::schema`
//! - External: `rusqlite`

use super::*;
use rusqlite::params;
#[cfg(test)]
mod meta_columns_tests {
    //! `sessions` / `projects` 两张表的列一致性护栏。
    //!
    //! 由来：v11 引入 `thinking_mode` 时漏改了一条 SELECT，运行期直接抛
    //! `Invalid column index: 12`，导致"切换会话"整体失败。当时护栏是"数一数列数"。
    //!
    //! 2026-09-21 起映射器改为**按列名取值**，护栏口径随之改变：数不数列数已不重要，
    //! 要锁的是"映射器读的每个列名，在结果集里真的存在"。
    //!
    //! 这里刻意**不解析列清单文本**，而是把清单拼进 SELECT 交给 SQLite 报结果列名：
    //! 这两张表的清单里有表前缀（`s.id` / `p.path`）和子查询（`… as session_count`），
    //! 手写解析既容易错，也测不出"清单里的列在真实表结构上到底有没有"。
    //! 顺带：`prepare` 失败就等于清单里有不存在的列（迁移删列没跟上时会在这里红）。

    use super::*;
    use rusqlite::Connection;

    fn init_conn() -> Connection {
        let conn = Connection::open_in_memory().unwrap();
        crate::infra::db::schema::init_schema(&conn).expect("建表");
        conn
    }

    /// 让 SQLite 报出这条 SELECT 的结果集列名 —— 它就是映射器 `row.get("…")` 用的名字
    /// （SQLite 用引用名的最后一段作为结果列名，所以 `s.id` 的结果列名是 `id`）。
    fn result_column_names(conn: &Connection, sql: &str) -> Vec<String> {
        let stmt = conn
            .prepare(sql)
            .unwrap_or_else(|e| panic!("列清单无法在真实表结构上准备：{e}\nSQL: {sql}"));
        stmt.column_names().iter().map(|s| s.to_string()).collect()
    }

    /// 共享清单必须覆盖 `session_meta_from_row` 读的每一个列名。
    #[test]
    fn session_meta_columns_cover_every_name_the_mapper_reads() {
        let conn = init_conn();
        let names = result_column_names(
            &conn,
            &format!(
                "SELECT {} FROM sessions s LEFT JOIN projects p ON s.project_id = p.id",
                SESSION_META_COLUMNS
            ),
        );
        for name in [
            "id",
            "title",
            "created_at",
            "updated_at",
            "message_count",
            "is_smart_named",
            "profile_id",
            "total_input_tokens",
            "total_output_tokens",
            "title_source",
            "project_id",
            // working_directory 取自 p.path —— 结果列名叫 "path"
            "path",
            "thinking_mode",
            "total_cache_hit_tokens",
            "total_cache_miss_tokens",
            // 归档元数据：deleted_at 取自 s，project_name 取自 p.name（结果列名 "name"）
            "deleted_at",
            "name",
        ] {
            assert!(
                names.iter().any(|n| n == name),
                "SESSION_META_COLUMNS 的结果集里没有 {name}：{names:?}"
            );
        }
        assert_eq!(
            names.len(),
            17,
            "列数应为 17，实际 {}：{names:?}",
            names.len()
        );
    }

    /// 共享清单必须覆盖 `project_meta_from_row` 读的每一个列名。
    #[test]
    fn project_meta_columns_cover_every_name_the_mapper_reads() {
        let conn = init_conn();
        let names = result_column_names(
            &conn,
            &format!("SELECT {} FROM projects p", PROJECT_META_COLUMNS),
        );
        for name in [
            "id",
            "name",
            "path",
            "created_at",
            "updated_at",
            "session_count",
        ] {
            assert!(
                names.iter().any(|n| n == name),
                "PROJECT_META_COLUMNS 的结果集里没有 {name}：{names:?}"
            );
        }
        assert_eq!(
            names.len(),
            6,
            "列数应为 6，实际 {}：{names:?}",
            names.len()
        );
    }
}

#[cfg(test)]
mod message_columns_tests {
    //! `session_messages` 的列一致性护栏（2026-09-21 撤回故障的正面锁）。
    //!
    //! 背景：v17 加 `interrupt_kind` 时三处 SELECT 改了两处，漏掉
    //! `find_session_message_by_id`，而撤回是唯一走那条查询的路径 —— 于是撤回直接抛
    //! `Invalid column index: 10`。同一类错误 v11 在 sessions 表上已经发生过一次
    //! （见 `meta_columns_tests`），本模块把它在 `session_messages` 上也钉住。

    use super::*;
    use crate::infra::types::models::Content;
    use rusqlite::Connection;

    /// 共享列清单必须覆盖映射器读的每一个列名，且不多不少。
    #[test]
    fn message_columns_cover_every_name_the_mapper_reads() {
        let columns: Vec<&str> = SESSION_MESSAGE_COLUMNS
            .split(',')
            .map(|c| c.trim())
            .filter(|c| !c.is_empty())
            .collect();
        for name in [
            "message_id",
            "seq",
            "role",
            "content_json",
            "created_at",
            "updated_at",
            "recalled_at",
            "hidden_at",
            "source",
            "interrupt_kind",
        ] {
            assert!(
                columns.contains(&name),
                "共享列清单缺少 {name}：{columns:?}"
            );
        }
        assert_eq!(
            columns.len(),
            10,
            "列数应为 10，实际 {}：{columns:?}",
            columns.len()
        );
        assert!(
            !columns.contains(&"turn_id"),
            "turn_id 已于 v21 删除，不得回到共享列清单里"
        );
    }

    /// 一行样本消息的 JSON（用 serde 产出，保证形状一定合法）。
    fn sample_content_json() -> String {
        serde_json::to_string(&Message::User {
            content: Content::Single("hi".to_string()),
        })
        .unwrap()
    }

    /// 列序被打乱也能正确读回 —— 这是"按列名取值"的核心价值（列序不再有约束力）。
    #[test]
    fn mapper_is_order_independent() {
        let conn = Connection::open_in_memory().unwrap();
        let json = sample_content_json();
        // 故意把 interrupt_kind 挪到第一列，与 SESSION_MESSAGE_COLUMNS 的顺序不同
        let mut stmt = conn
            .prepare(
                "SELECT ?1 AS interrupt_kind, ?2 AS message_id, 7 AS seq, 'user' AS role, \
                        ?3 AS content_json, 1 AS created_at, 2 AS updated_at, 3 AS recalled_at, \
                        4 AS hidden_at, 'chat' AS source",
            )
            .unwrap();
        let stored = stmt
            .query_row(params!["ix", "m1", json], stored_session_message_from_row)
            .unwrap();
        assert_eq!(stored.message_id, "m1");
        assert_eq!(stored.seq, 7);
        assert_eq!(stored.role, "user");
        assert_eq!(stored.source, MessageSource::Chat);
        assert_eq!(stored.updated_at, Some(2));
        assert_eq!(stored.interrupt_kind.as_deref(), Some("ix"));
    }

    /// 缺列时的报错必须**指向列名**，而不是下标越界 —— 这把"不许改回下标取值"钉死。
    #[test]
    fn missing_column_reports_the_name_not_an_index() {
        let conn = Connection::open_in_memory().unwrap();
        let json = sample_content_json();
        // 少 interrupt_kind 一列：复现 v17 漏改的那条 SELECT
        let mut stmt = conn
            .prepare(
                "SELECT ?1 AS message_id, 0 AS seq, 'user' AS role, ?2 AS content_json, \
                        0 AS created_at, 0 AS updated_at, 0 AS recalled_at, 0 AS hidden_at, \
                        'chat' AS source",
            )
            .unwrap();
        let err = stmt
            .query_row(params!["m1", json], stored_session_message_from_row)
            .unwrap_err();
        let msg = err.to_string();
        assert!(
            msg.contains("interrupt_kind"),
            "报错应指出缺失的列名，实际：{msg}"
        );
        assert!(
            !msg.contains("column index"),
            "不应再是下标越界，实际：{msg}"
        );
    }

    /// 端到端：在**真实 schema** 上写入一条带 `interrupt_kind` 的消息，再按 id 查回。
    ///
    /// 这条锁的就是 v17 事故本身（撤回是 `find_session_message_by_id` 的唯一调用方）。
    /// 用 `init_schema` 建真实表结构而不是手写建表，是为了让"迁移加了列、某条查询没跟上"
    /// 这类问题在 `cargo test` 阶段就暴露。
    #[test]
    fn find_message_by_id_round_trips_interrupt_kind() {
        let conn = Connection::open_in_memory().unwrap();
        crate::infra::db::schema::init_schema(&conn).expect("建表");
        conn.execute(
            "INSERT INTO sessions(id, title, created_at, updated_at, message_count)
             VALUES('s-repo-test', 't', 0, 0, 1)",
            [],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO session_messages(session_id, message_id, seq, role, content_json,
                                          created_at, source, interrupt_kind)
             VALUES('s-repo-test', 'm-repo-test', 0, 'user', ?1, 0, 'chat', 'stream_timeout')",
            params![sample_content_json()],
        )
        .unwrap();

        let found = find_session_message_by_id_in(&conn, "s-repo-test", "m-repo-test")
            .expect("查询不应报错")
            .expect("应能按 id 找到这条消息");
        assert_eq!(found.message_id, "m-repo-test");
        assert_eq!(found.role, "user");
        assert_eq!(found.source, MessageSource::Chat);
        assert_eq!(found.interrupt_kind.as_deref(), Some("stream_timeout"));
    }
}

#[cfg(test)]
mod turn_start_tests {
    //! 轮起点判定。
    //!
    //! `load_visible_turns_page` 靠它凑页：判错会让**真实的一轮被切在中间**，
    //! 首条渲染成"无 user 开头"的半轮（该函数注释开头写明要避免的情况）。
    //!
    //! 背景：`Inject`（崩溃恢复指令 / 方案重定向通知 / 反思修正建议）也是
    //! **user role + 非空正文** —— 它们是"伪装成一轮用户发言喂给模型的系统指令"。
    //! 判定若只看 `role`，这些注入会被当成新的一轮。

    use super::*;
    use crate::infra::types::models::Content;

    fn stored(role: &str, source: MessageSource, text: &str) -> StoredSessionMessage {
        let content = Content::Single(text.to_string());
        StoredSessionMessage {
            message_id: "m".to_string(),
            seq: 0,
            role: role.to_string(),
            content: Message::User { content },
            created_at: 0,
            updated_at: None,
            recalled_at: None,
            hidden_at: None,
            source,
            interrupt_kind: None,
        }
    }

    /// 用户真实输入（`Chat` + 非空正文）才算轮起点
    #[test]
    fn chat_user_message_starts_a_turn() {
        assert!(is_turn_start_message(&stored(
            "user",
            MessageSource::Chat,
            "帮我看一下这个 bug"
        )));
    }

    /// 回归防护：系统注入不得被当成轮起点 —— 否则按轮凑页会切出半轮
    #[test]
    fn inject_user_message_does_not_start_a_turn() {
        assert!(
            !is_turn_start_message(&stored(
                "user",
                MessageSource::Inject,
                "【系统拦截通知】请调用 ProposePlan 工具提交方案"
            )),
            "Inject 是伪装成用户发言的系统指令，不是新的一轮"
        );
    }

    /// 空正文的 user 消息（纯 tool_result 型）仍不算轮起点
    #[test]
    fn empty_user_message_does_not_start_a_turn() {
        assert!(!is_turn_start_message(&stored(
            "user",
            MessageSource::Chat,
            "   "
        )));
    }

    /// assistant 消息永远不是轮起点
    #[test]
    fn assistant_never_starts_a_turn() {
        assert!(!is_turn_start_message(&stored(
            "assistant",
            MessageSource::Chat,
            "好的"
        )));
    }
}

#[cfg(test)]
mod message_stats_tests {
    //! `session_message_stats` 的可见性口径护栏。
    //!
    //! 这个查询是分页接口 `totalCount` / `newestSeq` 的唯一数据源，前端拿它判断
    //! 「本地窗口是否落后于后端」。口径一旦与分页查询不一致（比如漏了
    //! `hidden_at` / `recalled_at` / `compact` 过滤），前端就会看到
    //! 「总数比窗口多但怎么翻页都翻不出来」的假落后，反复重拉。

    use super::*;
    use crate::infra::types::models::Content;
    use rusqlite::Connection;

    fn init_conn() -> Connection {
        let conn = Connection::open_in_memory().unwrap();
        crate::infra::db::schema::init_schema(&conn).expect("建表");
        conn.execute(
            "INSERT INTO sessions(id, title, created_at, updated_at, message_count)
             VALUES('s-stats', 't', 0, 0, 0)",
            [],
        )
        .unwrap();
        conn
    }

    /// 插一条消息（hidden / recalled 用布尔控制，便于逐条构造可见性组合）
    fn insert(
        conn: &Connection,
        message_id: &str,
        seq: i64,
        source: MessageSource,
        hidden: bool,
        recalled: bool,
    ) {
        let json = serde_json::to_string(&Message::User {
            content: Content::Single(message_id.to_string()),
        })
        .unwrap();
        conn.execute(
            "INSERT INTO session_messages(
                session_id, message_id, seq, role, content_json,
                created_at, updated_at, recalled_at, hidden_at, source
             ) VALUES('s-stats', ?1, ?2, 'user', ?3, 0, 0, ?4, ?5, ?6)",
            params![
                message_id,
                seq,
                json,
                if recalled { Some(1i64) } else { None },
                if hidden { Some(1i64) } else { None },
                source.as_db(),
            ],
        )
        .unwrap();
    }

    /// 空会话：0 条、无最新 seq（`MAX(seq)` 是 SQL NULL）
    #[test]
    fn empty_session_reports_zero_and_no_newest_seq() {
        let conn = init_conn();
        assert_eq!(
            session_message_stats_in(&conn, "s-stats").unwrap(),
            (0, None)
        );
    }

    /// 只统计可见消息：hidden / recalled / compact 三类都不计入
    #[test]
    fn only_visible_messages_are_counted() {
        let conn = init_conn();
        insert(&conn, "m0", 0, MessageSource::Chat, false, false);
        insert(&conn, "m1", 1, MessageSource::Chat, true, false); // 被隐藏
        insert(&conn, "m2", 2, MessageSource::Chat, false, true); // 被撤回
        insert(&conn, "m3", 3, MessageSource::Compact, false, false); // 压缩摘要

        let (count, newest) = session_message_stats_in(&conn, "s-stats").unwrap();
        assert_eq!(count, 1, "只有 m0 可见");
        assert_eq!(newest, Some(0), "最新可见 seq 不应被隐藏/撤回/压缩行抬高");
    }

    /// 正常会话：total = 可见条数，newest = 可见消息里的最大 seq
    #[test]
    fn reports_count_and_newest_seq_of_visible_messages() {
        let conn = init_conn();
        insert(&conn, "m0", 5, MessageSource::Chat, false, false);
        insert(&conn, "m1", 6, MessageSource::Interrupted, false, false);

        assert_eq!(
            session_message_stats_in(&conn, "s-stats").unwrap(),
            (2, Some(6))
        );
    }
}

#[cfg(test)]
mod archive_tests {
    //! 归档原语（v27）：批量恢复、批量彻底删除、项目归档、归档计数。
    //!
    //! 全部走 `*_in` 版本直接在 connection 上跑**真实 SQL**（不进全局 DB 单例），
    //! 与 `message_stats_tests` 同一做法。批量原语的全部风险都在 SQL 条件上
    //! （`deleted_at IS NULL` / `IS NOT NULL` 写反就变成“删掉活跃会话”），
    //! 所以这些用例断言的是"库里最终是什么样"，而不是函数返回值本身。

    use super::*;
    use rusqlite::Connection;

    /// 测试用连接：真实 schema + 运行期 PRAGMA。
    ///
    /// 可直接用 `init_schema` 是因为 v27 迁移（`projects.deleted_at`）已由 schema 层落地；
    /// 仓储层测试**不**自己补列——那样会把「仓储逻辑错」与「迁移没跟上」混成同一个红点。
    fn init_conn() -> Connection {
        let conn = Connection::open_in_memory().unwrap();
        crate::infra::db::schema::init_schema(&conn).expect("建表");
        // `session_messages` 的 ON DELETE CASCADE 依赖这个开关
        // （运行期由 `infra::db::configure_connection` 打开，测试连接要自己开）
        conn.pragma_update(None, "foreign_keys", "ON").unwrap();
        conn
    }

    fn insert_project(conn: &Connection, id: &str, name: &str, deleted_at: Option<i64>) {
        conn.execute(
            "INSERT INTO projects(id, name, path, created_at, updated_at, deleted_at)
             VALUES(?1, ?2, ?3, 1000, 1000, ?4)",
            params![id, name, format!("/tmp/{}", id), deleted_at],
        )
        .unwrap();
    }

    fn insert_session(
        conn: &Connection,
        id: &str,
        project_id: Option<&str>,
        deleted_at: Option<i64>,
    ) {
        conn.execute(
            "INSERT INTO sessions(
                id, title, created_at, updated_at, message_count, project_id, deleted_at
             ) VALUES(?1, ?2, 1000, 1000, 0, ?3, ?4)",
            params![id, id, project_id, deleted_at],
        )
        .unwrap();
    }

    fn session_deleted_at(conn: &Connection, id: &str) -> Option<i64> {
        conn.query_row(
            "SELECT deleted_at FROM sessions WHERE id = ?1",
            [id],
            |row| row.get::<_, Option<i64>>(0),
        )
        .unwrap()
    }

    fn project_deleted_at(conn: &Connection, id: &str) -> Option<i64> {
        conn.query_row(
            "SELECT deleted_at FROM projects WHERE id = ?1",
            [id],
            |row| row.get::<_, Option<i64>>(0),
        )
        .unwrap()
    }

    /// 批量恢复里，未归档 / 不存在的 id 不得让整批失败，但必须逐条回报。
    #[test]
    fn restore_sessions_reports_ids_that_are_not_archived() {
        let conn = init_conn();
        insert_session(&conn, "s-archived", None, Some(1_700_000_000_000));
        insert_session(&conn, "s-active", None, None);

        let result = restore_sessions_in(
            &conn,
            &[
                "s-archived".to_string(),
                "s-active".to_string(),
                "s-missing".to_string(),
            ],
        )
        .unwrap();

        assert_eq!(result.succeeded_ids, vec!["s-archived".to_string()]);
        assert_eq!(
            result.failed.len(),
            2,
            "活跃会话与不存在的会话都应进 failed"
        );
        assert!(
            result
                .failed
                .iter()
                .any(|f| f.id == "s-active" && f.reason.contains("不在归档列表")),
            "活跃会话不应被当作恢复成功：{:?}",
            result.failed
        );
        assert!(
            result
                .failed
                .iter()
                .any(|f| f.id == "s-missing" && f.reason.contains("不存在")),
            "不存在的会话应明确报不存在：{:?}",
            result.failed
        );
        assert_eq!(
            session_deleted_at(&conn, "s-archived"),
            None,
            "成功项应已清掉归档标记"
        );
    }

    /// 恢复归档会话时，其归档项目在同事务内一并复活；同一项目只回报一次。
    #[test]
    fn restoring_sessions_revives_the_archived_project_once() {
        let conn = init_conn();
        insert_project(&conn, "p1", "项目甲", Some(500));
        insert_session(&conn, "s1", Some("p1"), Some(500));
        insert_session(&conn, "s2", Some("p1"), Some(500));

        let result = restore_sessions_in(&conn, &["s1".to_string(), "s2".to_string()]).unwrap();

        assert_eq!(
            result.succeeded_ids,
            vec!["s1".to_string(), "s2".to_string()]
        );
        assert_eq!(
            result.restored_project_ids,
            vec!["p1".to_string()],
            "同一个项目只能回报一次"
        );
        assert_eq!(project_deleted_at(&conn, "p1"), None, "项目应已取消归档");
    }

    /// 彻底删除必须拒绝活跃会话（这是不可撤销操作唯一的防误删门），
    /// 并且成功项的消息行要跟着 CASCADE 一起消失。
    #[test]
    fn purge_sessions_rejects_active_sessions() {
        let conn = init_conn();
        insert_session(&conn, "s-archived", None, Some(9));
        insert_session(&conn, "s-active", None, None);
        conn.execute(
            "INSERT INTO session_messages(session_id, message_id, seq, role, content_json,
                                          created_at, source)
             VALUES('s-archived', 'm1', 0, 'user', '{}', 1000, 'chat')",
            [],
        )
        .unwrap();

        let (result, pending) =
            purge_sessions_in(&conn, &["s-archived".to_string(), "s-active".to_string()]).unwrap();

        assert_eq!(result.succeeded_ids, vec!["s-archived".to_string()]);
        assert_eq!(
            pending,
            vec!["s-archived".to_string()],
            "只有删成功行的 id 才需要清产物"
        );
        assert_eq!(result.failed.len(), 1);
        assert_eq!(result.failed[0].id, "s-active");
        assert_eq!(result.failed[0].reason, "不在归档列表");

        let sessions_left: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM sessions WHERE id = 's-active'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(sessions_left, 1, "活跃会话不得被彻底删除");
        let messages_left: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM session_messages WHERE session_id = 's-archived'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(messages_left, 0, "session_messages 应随会话行 CASCADE 删除");
    }

    /// 归档项目：项目从列表消失、名下未归档会话被打上同一时间戳，
    /// 已手动归档的会话保留原时间戳；归档页仍能拿到项目名。
    #[test]
    fn archiving_project_hides_it_and_stamps_its_sessions() {
        let conn = init_conn();
        insert_project(&conn, "p1", "项目甲", None);
        insert_project(&conn, "p2", "项目乙", None);
        insert_session(&conn, "s1", Some("p1"), None);
        insert_session(&conn, "s2", Some("p1"), None);
        insert_session(&conn, "s-manual", Some("p1"), Some(777));

        let archived = archive_project_in(&conn, "p1", 1234).unwrap();
        assert_eq!(archived, 2, "只归档此前未归档的会话");

        assert_eq!(session_deleted_at(&conn, "s1"), Some(1234));
        assert_eq!(session_deleted_at(&conn, "s2"), Some(1234));
        assert_eq!(
            session_deleted_at(&conn, "s-manual"),
            Some(777),
            "已归档会话的时间戳不应被项目归档覆盖"
        );

        // 项目列表不再含 p1
        let visible = list_projects_in(&conn).unwrap();
        assert_eq!(
            visible.iter().map(|p| p.id.as_str()).collect::<Vec<_>>(),
            vec!["p2"],
            "归档项目必须从项目列表里消失"
        );

        // 归档页要展示「所属项目名 + 归档时间」
        let archived_metas = list_deleted_sessions_in(&conn).unwrap();
        let s1_meta = archived_metas
            .iter()
            .find(|m| m.id == "s1")
            .expect("归档列表应包含 s1");
        assert_eq!(s1_meta.deleted_at, Some(1234));
        assert_eq!(
            s1_meta.project_name.as_deref(),
            Some("项目甲"),
            "归档会话必须能拿到项目名（LEFT JOIN 不得过滤归档项目）"
        );

        // 重复归档同一个项目应当报错（并且不会把时间戳刷成新的）
        assert!(
            archive_project_in(&conn, "p1", 9999).is_err(),
            "重复归档应报错而不是静默刷新时间"
        );
        assert_eq!(project_deleted_at(&conn, "p1"), Some(1234));
    }

    /// 归档计数只数 `deleted_at` 非空的会话。
    #[test]
    fn count_deleted_sessions_counts_only_archived_rows() {
        let conn = init_conn();
        insert_session(&conn, "s1", None, Some(1));
        insert_session(&conn, "s2", None, Some(2));
        insert_session(&conn, "s3", None, None);

        assert_eq!(count_deleted_sessions_in(&conn).unwrap(), 2);

        restore_sessions_in(&conn, &["s1".to_string()]).unwrap();
        assert_eq!(count_deleted_sessions_in(&conn).unwrap(), 1);
    }
}

#[cfg(test)]
mod usage_tests {
    //! 请求级用量明细（v28）：插入原语、累计原语、按来源汇总。
    //!
    //! 全部直接在 connection 上跑**真实 SQL**（进不了全局 DB 单例），
    //! 与 `archive_tests` 同一做法；`record_request_usage`（事务 + 事件）
    //! 由这三个原语拼成，原语对了那层就只剩“把三句串起来”。

    use super::{
        add_session_token_totals_in, insert_request_usage_in, session_usage_breakdown_in,
        session_usage_writable_in,
    };
    use crate::core::session::{RequestUsageRecord, SessionTokenTotals, UsageSource};
    use rusqlite::Connection;

    /// 测试连接：真实 schema + 运行期 PRAGMA（外键与生产一致）。
    fn init_conn() -> Connection {
        let conn = Connection::open_in_memory().unwrap();
        // `init_schema` 不负责 PRAGMA，生产由 `db::configure_connection` 开外键
        conn.pragma_update(None, "foreign_keys", "ON").unwrap();
        crate::infra::db::schema::init_schema(&conn).expect("建表");
        conn
    }

    fn insert_session(conn: &Connection, id: &str, deleted_at: Option<i64>) {
        conn.execute(
            "INSERT INTO sessions(id, title, created_at, updated_at, message_count, deleted_at)
             VALUES(?1, '测试会话', 1, 1, 0, ?2)",
            rusqlite::params![id, deleted_at],
        )
        .expect("插入会话");
    }

    fn raw_usage(conn: &Connection, session_id: &str) -> Vec<(String, Option<i64>, Option<i64>)> {
        let mut stmt = conn
            .prepare(
                "SELECT source, cache_hit_tokens, cache_miss_tokens
                 FROM request_usage WHERE session_id = ?1 ORDER BY id",
            )
            .unwrap();
        stmt.query_map([session_id], |row| {
            Ok((row.get(0)?, row.get(1)?, row.get(2)?))
        })
        .unwrap()
        .filter_map(Result::ok)
        .collect()
    }

    /// 核心汇总契约：按来源分组、总计等于各组之和、
    /// 未上报的请求单独计数且不拉低命中率。
    #[test]
    fn breakdown_groups_by_source_and_flags_unreported() {
        let conn = init_conn();
        insert_session(&conn, "s1", None);

        // 主循环第 1 次：厂商报了缓存
        insert_request_usage_in(
            &conn,
            &RequestUsageRecord::new("s1", UsageSource::Main)
                .run("run-1", Some(0))
                .model_id("deepseek-chat")
                .tokens(1000, 100)
                .cache(Some(900), Some(100), Some("deepseek"))
                .at(1_000),
        )
        .unwrap();
        // 主循环第 2 次：报了缓存，命中真的是 0（不得被当成“未上报”）
        insert_request_usage_in(
            &conn,
            &RequestUsageRecord::new("s1", UsageSource::Main)
                .run("run-1", Some(1))
                .tokens(2000, 200)
                .cache(Some(0), Some(2000), Some("deepseek"))
                .at(2_000),
        )
        .unwrap();
        // 子 Agent：厂商没报缓存（两个列必须落 NULL）
        insert_request_usage_in(
            &conn,
            &RequestUsageRecord::new("s1", UsageSource::Subagent)
                .parent_tool_use_id("toolu_1")
                .tokens(500, 50)
                .at(3_000),
        )
        .unwrap();

        let breakdown = session_usage_breakdown_in(&conn, "s1").unwrap();
        assert!(breakdown.has_detail, "有明细时 has_detail 必须为 true");
        assert_eq!(breakdown.request_count, 3);
        assert_eq!(breakdown.input_tokens, 3500);
        assert_eq!(breakdown.output_tokens, 350);
        // 已上报的命中/未命中：900 / 2100（未上报那行不按 0 计入，但也不影响这两个和）
        assert_eq!(breakdown.cache_hit_tokens, 900);
        assert_eq!(breakdown.cache_miss_tokens, 2100);
        assert_eq!(
            breakdown.unreported_requests, 1,
            "只有子 Agent 那行没上报缓存"
        );
        assert_eq!(
            breakdown.unreported_input_tokens, 500,
            "未上报的输入量要单独记，才能说清“多少 token 不知道”"
        );

        // 分组：字母序 compact < main < subagent（这里只有 main 与 subagent）
        assert_eq!(breakdown.sources.len(), 2);
        let main = breakdown
            .sources
            .iter()
            .find(|s| s.source == UsageSource::Main)
            .expect("应有 main 分组");
        assert_eq!(main.request_count, 2);
        assert_eq!(main.cache_hit_tokens, 900);
        assert_eq!(main.unreported_requests, 0, "命中为 0 不等于未上报");

        let sub = breakdown
            .sources
            .iter()
            .find(|s| s.source == UsageSource::Subagent)
            .expect("应有 subagent 分组");
        assert_eq!(sub.request_count, 1);
        assert_eq!(sub.unreported_requests, 1);
        assert_eq!(sub.unreported_input_tokens, 500);
        assert_eq!(sub.cache_hit_tokens, 0);

        // 总计 = 各组之和（不允许总分另算一套而少加一组）
        assert_eq!(
            breakdown.input_tokens,
            breakdown
                .sources
                .iter()
                .map(|s| s.input_tokens)
                .sum::<u64>()
        );

        // 原始行：未上报必须真 NULL，上报为零必须真 0
        assert_eq!(
            raw_usage(&conn, "s1"),
            vec![
                ("main".to_string(), Some(900), Some(100)),
                ("main".to_string(), Some(0), Some(2000)),
                ("subagent".to_string(), None, None),
            ]
        );
    }

    /// 读 `sessions` 的四个累计值：(input, output, cache_hit, cache_miss)。
    fn session_totals(conn: &Connection, session_id: &str) -> (u64, u64, u64, u64) {
        conn.query_row(
            "SELECT total_input_tokens, total_output_tokens,
                    total_cache_hit_tokens, total_cache_miss_tokens
             FROM sessions WHERE id = ?1",
            [session_id],
            |row| {
                Ok((
                    row.get::<_, i64>(0)?.max(0) as u64,
                    row.get::<_, i64>(1)?.max(0) as u64,
                    row.get::<_, i64>(2)?.max(0) as u64,
                    row.get::<_, i64>(3)?.max(0) as u64,
                ))
            },
        )
        .expect("读会话累计值")
    }

    /// 一次子 Agent 调用**只累加一次**（去重契约）。
    ///
    /// 复现子 Agent 的真实写入形态：每个 loop 一次请求 → 一行明细 + 一次累计，
    /// 两者在**同一个事务**里（与 `record_request_usage` 一致）。
    ///
    /// 这是双计事故的回归防护：历史上子 Agent 的用量先由子代理自己带回父层，
    /// 再由回合收尾的 `save_session` 补一次。一旦“每轮直写”与“收尾补交”并存，
    /// `sessions.total_*` 就会大于明细求和，而账本本身不会报任何错——只能靠断言拓。
    /// 子 Agent 改成每 loop 直写后，收尾一律传 `None`，本用例才能恒绿。
    #[test]
    fn subagent_usage_is_counted_once_per_loop() {
        let mut conn = init_conn();
        insert_session(&conn, "s1", None);

        // 两轮子 Agent 请求（输入/输出/命中/未命中）
        let loops = [(1200u64, 80u64, 1000u64, 200u64), (2400, 120, 2100, 300)];
        for (loop_index, (input, output, hit, miss)) in loops.into_iter().enumerate() {
            let record = RequestUsageRecord::new("s1", UsageSource::Subagent)
                .run("sub-run-1", Some(loop_index as u32))
                .agent_role("subagent:implementation")
                .model_id("deepseek-chat")
                .tokens(input, output)
                .cache(Some(hit), Some(miss), Some("deepseek"))
                .at(1_000 + loop_index as u64);
            let delta = SessionTokenTotals {
                input,
                output,
                cache_hit: hit,
                cache_miss: miss,
            };

            // 与 `record_request_usage` 同一个事务形状：明细 + 累计同生同死
            let tx = conn.transaction().unwrap();
            insert_request_usage_in(&tx, &record).unwrap();
            add_session_token_totals_in(&tx, "s1", delta)
                .unwrap()
                .expect("活跃会话必须可累加");
            tx.commit().unwrap();
        }

        let breakdown = session_usage_breakdown_in(&conn, "s1").unwrap();
        let totals = session_totals(&conn, "s1");

        assert_eq!(breakdown.request_count, 2);
        // 核心断言：累计 == 明细求和（差一分就是重复计或多计）
        assert_eq!(
            totals.0, breakdown.input_tokens,
            "sessions.total_input_tokens 必须等于明细求和（收了两次就是双计）"
        );
        assert_eq!(totals.1, breakdown.output_tokens, "输出 token 同理");
        assert_eq!(totals.2, breakdown.cache_hit_tokens);
        assert_eq!(totals.3, breakdown.cache_miss_tokens);
        assert_eq!(breakdown.input_tokens, 3600);
        assert_eq!(breakdown.output_tokens, 200);

        // 明细里的缓存读数必须是**实际值**，而不是写死的 0
        assert_eq!(
            raw_usage(&conn, "s1"),
            vec![
                ("subagent".to_string(), Some(1000), Some(200)),
                ("subagent".to_string(), Some(2100), Some(300)),
            ],
            "子 Agent 明细行要带真实缓存读数（未上报才是 NULL，不得固定写 0）"
        );
    }

    /// 无明细的会话（早于 v28 的历史会话）：`has_detail = false`、数字全 0。
    ///
    /// 调用方必须靠这个标志区分“真没用过 token”与“没明细可拆”，
    /// 否则会把历史会话展示成 0。
    #[test]
    fn breakdown_without_detail_is_flagged() {
        let conn = init_conn();
        insert_session(&conn, "s-empty", None);

        let breakdown = session_usage_breakdown_in(&conn, "s-empty").unwrap();
        assert!(!breakdown.has_detail, "没有明细时必须 has_detail = false");
        assert_eq!(breakdown.request_count, 0);
        assert_eq!(breakdown.input_tokens, 0);
        assert_eq!(breakdown.output_tokens, 0);
        assert_eq!(breakdown.cache_hit_tokens, 0);
        assert_eq!(breakdown.cache_miss_tokens, 0);
        assert_eq!(breakdown.unreported_requests, 0);
        assert!(breakdown.sources.is_empty());

        // 会话 id 不存在时同样返回空账，不报错（展示层要能安全地问）
        let missing = session_usage_breakdown_in(&conn, "s-missing").unwrap();
        assert!(!missing.has_detail);
    }

    /// 库里的来源取值不可识别时必须报错，不得静默归入某个分组。
    #[test]
    fn unknown_source_in_db_is_rejected() {
        let conn = init_conn();
        insert_session(&conn, "s1", None);
        conn.execute(
            "INSERT INTO request_usage(session_id, source, input_tokens, output_tokens, created_at)
             VALUES('s1', 'ghost', 1, 1, 1)",
            [],
        )
        .unwrap();

        let err = session_usage_breakdown_in(&conn, "s1").expect_err("未知来源必须报错");
        assert!(err.contains("ghost"), "错误信息要带上原始取值：{err}");
    }

    /// 累计原语：活跃会话原子累加并读回；归档/不存在的会话返回 None。
    #[test]
    fn totals_accumulate_and_skip_archived_sessions() {
        let conn = init_conn();
        insert_session(&conn, "s1", None);
        insert_session(&conn, "s-arch", Some(1_700_000_000_000));

        let totals = add_session_token_totals_in(
            &conn,
            "s1",
            SessionTokenTotals {
                input: 100,
                output: 10,
                cache_hit: 80,
                cache_miss: 20,
            },
        )
        .unwrap()
        .expect("活跃会话必须累加成功");
        assert_eq!(
            (
                totals.input,
                totals.output,
                totals.cache_hit,
                totals.cache_miss
            ),
            (100, 10, 80, 20)
        );

        // 第二次是增量累加，不是整值覆盖
        let totals = add_session_token_totals_in(
            &conn,
            "s1",
            SessionTokenTotals {
                input: 5,
                output: 1,
                cache_hit: 0,
                cache_miss: 5,
            },
        )
        .unwrap()
        .expect("第二次累加");
        assert_eq!(totals.input, 105);
        assert_eq!(totals.cache_miss, 25);

        assert_eq!(
            add_session_token_totals_in(
                &conn,
                "s-arch",
                SessionTokenTotals {
                    input: 1,
                    output: 1,
                    cache_hit: 0,
                    cache_miss: 1,
                },
            )
            .unwrap(),
            None,
            "归档会话不再接受新账"
        );
        assert_eq!(
            add_session_token_totals_in(
                &conn,
                "s-missing",
                SessionTokenTotals {
                    input: 1,
                    output: 1,
                    cache_hit: 0,
                    cache_miss: 1,
                },
            )
            .unwrap(),
            None,
            "不存在的会话返回 None"
        );
    }

    /// 可写判定与累加 SQL 的条件同口径（存在且未归档）。
    #[test]
    fn writable_check_matches_accumulate_condition() {
        let conn = init_conn();
        insert_session(&conn, "s1", None);
        insert_session(&conn, "s-arch", Some(1_700_000_000_000));

        assert!(session_usage_writable_in(&conn, "s1").unwrap());
        assert!(!session_usage_writable_in(&conn, "s-arch").unwrap());
        assert!(!session_usage_writable_in(&conn, "s-missing").unwrap());
    }
}
