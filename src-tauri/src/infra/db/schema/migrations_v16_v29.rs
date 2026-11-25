//! # schema/migrations_v16_v29.rs — 数据库结构迁移（v16 至 v29）
//!
//! 承载近期数据库版本（v16 毫秒时间戳统一至 v29 checkpoint 主键迁移）的增量升级逻辑与时间归一化辅助函数。
//!
//! ## Key Exports
//! - `migrate_v16_timestamps_to_millis()` .. `migrate_v29_checkpoint_links_message_id()`: 近期版本迁移函数
//!
//! ## Dependencies
//! - External: `rusqlite`, `serde_json`
use rusqlite::Connection;
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
pub(super) fn migrate_v16_timestamps_to_millis(conn: &Connection) -> Result<(), rusqlite::Error> {
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
pub(super) fn db_table_exists(conn: &Connection, name: &str) -> Result<bool, rusqlite::Error> {
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
pub(super) fn migrate_v17_add_interrupt_kind(conn: &Connection) -> Result<(), rusqlite::Error> {
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
pub(super) fn migrate_v18_drop_agent_runs_interrupt_kind(conn: &Connection) -> Result<(), rusqlite::Error> {
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
pub(super) fn migrate_v19_drop_unused_run_columns(conn: &Connection) -> Result<(), rusqlite::Error> {
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
pub(super) fn migrate_v20_add_agent_runs_interrupt_kind(conn: &Connection) -> Result<(), rusqlite::Error> {
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
pub(super) fn migrate_v21_message_source_and_drop_turn_id(conn: &Connection) -> Result<(), rusqlite::Error> {
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
pub(super) fn migrate_v22_add_reflection_columns(conn: &Connection) -> Result<(), rusqlite::Error> {
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
pub(super) fn migrate_v23_backfill_interrupt_kind(conn: &Connection) -> Result<(), rusqlite::Error> {
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

/// v24：用子 Agent 逐轮轨迹替代仅归档溢出事件的旧表。
pub(super) fn migrate_v24_subagent_trace(conn: &Connection) -> Result<(), rusqlite::Error> {
    conn.execute_batch(
        "DROP TABLE IF EXISTS subagent_events;
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
         CREATE INDEX IF NOT EXISTS idx_subagent_trace_runs_session
            ON subagent_trace_runs(session_id, started_at);
         CREATE INDEX IF NOT EXISTS idx_subagent_trace_events_run_loop
            ON subagent_trace_events(run_id, loop_index);",
    )?;
    Ok(())
}

/// v25：为子 Agent 轨迹索引保存稳定显示名称，便于按会话恢复子 Agent 列表。
pub(super) fn migrate_v25_subagent_trace_run_label(conn: &Connection) -> Result<(), rusqlite::Error> {
    let mut stmt = conn.prepare("PRAGMA table_info(subagent_trace_runs)")?;
    let has_label = stmt
        .query_map([], |row| row.get::<_, String>(1))?
        .filter_map(Result::ok)
        .any(|column| column == "label");
    if !has_label {
        conn.execute(
            "ALTER TABLE subagent_trace_runs ADD COLUMN label TEXT NOT NULL DEFAULT ''",
            [],
        )?;
    }
    Ok(())
}

/// v26：为 `agent_runs` 增加可空列 `wait_ms`（本 run 的**等待耗时**，毫秒）。
///
/// ## 语义
/// 记录这一次运行里「工具执行前等用户审批」累计消耗的时间，用于把模型耗时与
/// 人类确认耗时区分开（否则审批慢会被算进执行耗时，评测结论失真）。
///
/// ## 为什么可空而不是 `NOT NULL DEFAULT 0`
/// `0` 会被读成「等待耗时为零」，与「根本没有统计过」混为一谈；
/// 可空才能让上层区分两种情况：
/// - `NULL` = **未统计等待**（早于本次迁移的旧行，或收尾路径没走统计逻辑）；
/// - `Some(ms)` = 本次运行的真实等待毫秒数（可能是 0：全程无需审批）。
///
/// 刻意**不回填**：旧行没有可依据的历史数据，回填 0 等于编造「没等过」。
///
/// ⚠️ 列**追加在末尾**，不改动既有列顺序（`RUN_COLUMNS` 的既有下标语义不受影响）。
pub(super) fn migrate_v26_run_wait_ms(conn: &Connection) -> Result<(), rusqlite::Error> {
    // ⚠️ 表可能不存在（迁移测试的最小桩库只建 sessions/app_state；
    // 早期版本库也未必有 agent_runs），先探表再探列，
    // 否则 `ALTER TABLE` 一个不存在的表会直接报 "no such table"（与 v17/v20 同源的坑）。
    if !db_table_exists(conn, "agent_runs")? {
        return Ok(());
    }

    let has_wait_ms = {
        let mut stmt = conn.prepare("PRAGMA table_info(agent_runs)")?;
        let found = stmt
            .query_map([], |row| row.get::<_, String>(1))?
            .filter_map(Result::ok)
            .any(|column| column == "wait_ms");
        found
    };
    if !has_wait_ms {
        conn.execute("ALTER TABLE agent_runs ADD COLUMN wait_ms INTEGER", [])?;
    }

    // 历史消息重建用时需要按 message_id 反查 run（`list_runs_by_message_id`）。
    // 这条路径在每次翻页/重载时对每一轮都会跑一次，没索引就是全表扇描；
    // `IF NOT EXISTS` 保证重复执行安全（与列存在性检查同为幂等）。
    conn.execute(
        "CREATE INDEX IF NOT EXISTS idx_agent_runs_message_id ON agent_runs(message_id)",
        [],
    )?;
    Ok(())
}

/// v27：为 `projects` 增加可空列 `deleted_at`（项目**归档**标记，毫秒时间戳）。
///
/// ## 语义
/// `NULL` = 正常项目；`Some(ms)` = 该项目已被用户归档，值是归档那一刻的时间戳。
/// 归档属于**软删**：只打标记、不删行。物理 `DELETE FROM projects` 会把
/// `sessions.project_id` 按外键 `ON DELETE SET NULL` 打空，
/// 「哪个会话原本属于哪个项目」这段历史从此不可恢复——而快照、回收站都需要它。
///
/// ## 为什么可空而不是 `NOT NULL DEFAULT 0`
/// 与 v26 的 `agent_runs.wait_ms` 同理：`0` 会被读成「1970 年归档」这样一个具体时刻，
/// 与「从未归档」混为一谈；只有可空才能让上层干净地区分「在用的项目」与「已归档的项目」。
///
/// 刻意**不回填**：历史项目没有发生过归档动作，回填任何值都是编造，一律留 `NULL`。
///
/// ⚠️ 列**追加在末尾**，不改动既有列顺序。
pub(super) fn migrate_v27_project_archive(conn: &Connection) -> Result<(), rusqlite::Error> {
    // ⚠️ 必须先探表：迁移测试的最小桩库只有 `app_state`（没有 `projects`），
    // 早期版本库也未必有 `projects`（v10 才引入该表），
    // 直接 `ALTER TABLE projects` 一个不存在的表会报 "no such table"（与 v17/v20/v26 同源的坑）。
    if !db_table_exists(conn, "projects")? {
        return Ok(());
    }

    let has_deleted_at = {
        let mut stmt = conn.prepare("PRAGMA table_info(projects)")?;
        let found = stmt
            .query_map([], |row| row.get::<_, String>(1))?
            .filter_map(Result::ok)
            .any(|column| column == "deleted_at");
        found
    };
    // 探列而不是无条件 ALTER：迁移可能被重复执行（版本号读取异常、手工重跑），
    // 重复 `ADD COLUMN` 会直接报 "duplicate column name"。
    if !has_deleted_at {
        conn.execute("ALTER TABLE projects ADD COLUMN deleted_at INTEGER", [])?;
    }
    Ok(())
}

/// v28：新增 `request_usage` 表——**逐请求**的 token 用量明细。
///
/// ## 为什么需要这张表
/// `sessions.total_*` 只是四个累加计数器，一旦累加就没有任何维度可拆：
/// 分不清哪部分 token 是子 Agent 烧掉的、哪部分是压缩/反思/自动命名烧掉的，
/// 也分不清某次请求厂商到底报没报缓存字段。要回答这些问题只能把**每一次请求**落一行。
///
/// ## 与 `sessions.total_*` 的关系
/// 明细表是**原始凭据**，`sessions.total_*` 是**派生汇总**。两者由
/// `core::session::usage::record_request_usage` 在**同一个事务**里一起写，
/// 不允许任何路径只写其中一边（否则「明细加起来 ≠ 累计值」这种账永远对不平）。
///
/// ## `cache_hit_tokens` / `cache_miss_tokens` 为什么可空
/// `NULL` = **厂商这次没上报缓存字段**，与 `0`（上报了、就是零）是两件事：
/// 前者在展示层应显示「未报告」，后者应显示「0%」。合并成 `NOT NULL DEFAULT 0`
/// 等于把「不知道」伪装成「确定为零」，命中率会被系统性低估。
/// 汇总查询里「未上报请求数 / 未上报输入 token」正是靠 `IS NULL` 判出来的。
pub(super) fn migrate_v28_request_usage(conn: &Connection) -> Result<(), rusqlite::Error> {
    // 全部 `IF NOT EXISTS`：迁移可能被重复执行（版本号读取异常、手工重跑），
    // 重复建表/建索引必须无副作用。
    //
    // 外键指向 `sessions(id)`：会话**彻底删除**时明细随 `ON DELETE CASCADE` 一起清掉；
    // 归档（`deleted_at` 非空）只打标记，明细保留——归档可撤销，账不能丢。
    // SQLite 允许先建带外键的表、后建被引用表（外键在 DML 时才校验），
    // 因此本迁移在建表 DDL 之前执行也不会报错。
    conn.execute_batch(
        "CREATE TABLE IF NOT EXISTS request_usage (
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
         CREATE INDEX IF NOT EXISTS idx_request_usage_session_time
            ON request_usage(session_id, created_at);",
    )?;
    Ok(())
}

/// v29：撤回关联从**易变的内存消息下标**改为**稳定的 message_id**。
///
/// ## 为什么必须换主键
/// 旧表的 `user_message_index` 记录的是"写补丁那一刻内存 `messages` 数组的下标"。
/// 自动压缩会把前缀压成两条消息、撤回会截断历史，下标随之前移并被后续轮次复用；
/// 而撤回解析时又拿它与 `session_messages.seq`（全局递增、不回退）比较，
/// 两者不在同一个坐标系。实测后果：
/// - 点第 3 轮却解析到第 6 轮的快照；
/// - 点第 6 轮解析回当前 HEAD，代码完全没退；
/// - 解析不到时退化为"初始状态"，预览/执行变成撤销整个会话。
///
/// `message_id` 是消息落库时分配的 UUID，跨压缩、撤回、分页都不变，是唯一正确的锚点。
///
/// ## 旧数据处理
/// - `message_id` 为空的历史行无法可靠还原身份，**丢弃**（它们正是会误配的脏数据）；
/// - 快照树里的 `trigger_user_message_id` 元数据会在会话打开时由
///   `SessionSnapshotManager` 回填，因此旧会话仍能恢复出正确的关联。
///
/// 迁移可能被重复执行（版本号读取异常、手工重跑），因此先探列、整段用事务包住。
pub(super) fn migrate_v29_checkpoint_links_message_id(conn: &Connection) -> Result<(), rusqlite::Error> {
    if !db_table_exists(conn, "checkpoint_user_message_links")? {
        return Ok(());
    }

    let has_index_column = {
        let mut stmt = conn.prepare("PRAGMA table_info(checkpoint_user_message_links)")?;
        let found = stmt
            .query_map([], |row| row.get::<_, String>(1))?
            .filter_map(Result::ok)
            .any(|column| column == "user_message_index");
        found
    };
    if !has_index_column {
        return Ok(());
    }

    conn.execute_batch(
        "BEGIN IMMEDIATE;
         DROP TABLE IF EXISTS checkpoint_user_message_links_v29;
         CREATE TABLE checkpoint_user_message_links_v29 (
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
         INSERT OR IGNORE INTO checkpoint_user_message_links_v29(
            session_id, message_id, checkpoint_id, has_file_edits, created_at, updated_at
         )
         SELECT l.session_id, l.message_id, l.checkpoint_id, l.has_file_edits,
                l.created_at, l.updated_at
         FROM checkpoint_user_message_links l
         JOIN (
            SELECT session_id, message_id, MIN(created_at) AS min_created
            FROM checkpoint_user_message_links
            WHERE message_id IS NOT NULL AND trim(message_id) <> ''
            GROUP BY session_id, message_id
         ) earliest
           ON earliest.session_id = l.session_id
          AND earliest.message_id = l.message_id
          AND earliest.min_created = l.created_at;
         DROP TABLE checkpoint_user_message_links;
         ALTER TABLE checkpoint_user_message_links_v29 RENAME TO checkpoint_user_message_links;
         CREATE INDEX IF NOT EXISTS idx_checkpoint_user_message_links_session
            ON checkpoint_user_message_links(session_id, message_id);
         COMMIT;",
    )?;
    Ok(())
}
