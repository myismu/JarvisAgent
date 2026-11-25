//! # repository/messages.rs — 会话消息与上下文快照仓储
//!
//! 封装 `session_messages` 的增删改查、分页回溯、软隐藏及 `session_context_snapshots` 读写。
//!
//! ## Key Exports
//! - `StoredSessionMessage`: 持久化消息行模型
//! - `ContextSnapshotUsageUpdate`: 上下文快照用量更新参数对象
//! - `append_or_upsert_session_messages()`: 批量写入或更新会话消息
//! - `list_visible_session_messages()` / `load_visible_turns_page()`: 查询可见消息与分页回合
//! - `load_session()`: 读取完整会话记忆
//! - `upsert_context_snapshot()` / `update_context_snapshot_usage()` / `get_context_snapshot()`: 上下文快照持久化
//!
//! ## Dependencies
//! - Internal: `crate::infra::db`, `crate::infra::types::models`
//! - External: `rusqlite`, `serde_json`

use rusqlite::{params, OptionalExtension, Row};
use std::collections::HashMap;
use crate::infra::types::models::{Message, MessageSource, SessionContextSnapshot, SessionMemory};
#[derive(Debug, Clone)]
pub struct StoredSessionMessage {
    pub message_id: String,
    pub seq: usize,
    pub role: String,
    pub content: Message,
    pub created_at: u64,
    pub updated_at: Option<u64>,
    pub recalled_at: Option<u64>,
    pub hidden_at: Option<u64>,
    /// 消息来源（可见域）。落库形态是字符串，此处在读边界经
    /// `MessageSource::from_db` 还原 —— 未知值会**报错**而不是静默降级。
    pub source: MessageSource,
    /// 中断类型（`None` = 非中断消息）。取值见 `InterruptKind::as_str`。
    pub interrupt_kind: Option<String>,
}

pub fn append_or_upsert_session_messages(
    session_id: &str,
    messages: &[Message],
    message_ids: &[String],
    sources: &[MessageSource],
    interrupt_kinds: &[Option<String>],
    now: u64,
) -> Result<(), String> {
    crate::infra::db::with_transaction(|tx| {
        let mut next_seq: i64 = tx
            .query_row(
                "SELECT COALESCE(MAX(seq), -1) + 1 FROM session_messages WHERE session_id = ?1",
                [session_id],
                |row| row.get(0),
            )
            .map_err(|e| e.to_string())?;

        let mut existing_by_content = tx
            .prepare(
                "SELECT message_id, seq, role, content_json
                 FROM session_messages
                 WHERE session_id = ?1 AND hidden_at IS NULL AND recalled_at IS NULL",
            )
            .map_err(|e| e.to_string())?
            .query_map([session_id], |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, i64>(1)?,
                    row.get::<_, String>(2)?,
                    row.get::<_, String>(3)?,
                ))
            })
            .map_err(|e| e.to_string())?
            .collect::<Result<Vec<_>, _>>()
            .map_err(|e| e.to_string())?;

        for (idx, message) in messages.iter().enumerate() {
            let Some(message_id) = message_ids.get(idx).filter(|id| !id.trim().is_empty()) else {
                continue;
            };
            // 与消息平行的来源；缺来源按 `Chat` 补（口径同 `normalize_message_ids`）
            let source = sources
                .get(idx)
                .copied()
                .unwrap_or(MessageSource::Chat)
                .as_db();
            // 中断类型（None = 非中断消息）；与 messages/sources 平行取值
            let interrupt_kind = interrupt_kinds.get(idx).and_then(|k| k.as_deref());
            let role = match message {
                Message::User { .. } => "user",
                Message::Assistant { .. } => "assistant",
            };
            let content_json = serde_json::to_string(message).map_err(|e| e.to_string())?;
            let existing_seq: Option<i64> = tx
                .query_row(
                    "SELECT seq FROM session_messages WHERE session_id = ?1 AND message_id = ?2",
                    params![session_id, message_id],
                    |row| row.get(0),
                )
                .optional()
                .map_err(|e| e.to_string())?;
            let seq = if let Some(seq) = existing_seq {
                seq
            } else if let Some(position) = existing_by_content.iter().position(
                |(existing_message_id, _, existing_role, existing_content_json)| {
                    existing_message_id != message_id
                        && existing_role == role
                        && existing_content_json == &content_json
                },
            ) {
                let (_, seq, _, _) = existing_by_content.remove(position);
                tx.execute(
                    "UPDATE session_messages
                     SET message_id = ?3, updated_at = ?4, source = ?5, interrupt_kind = ?6
                     WHERE session_id = ?1 AND seq = ?2",
                    params![
                        session_id,
                        seq,
                        message_id,
                        now as i64,
                        source,
                        interrupt_kind
                    ],
                )
                .map_err(|e| e.to_string())?;
                seq
            } else {
                let seq = next_seq;
                next_seq += 1;
                seq
            };

            tx.execute(
                "INSERT INTO session_messages(
                    session_id, message_id, seq, role, content_json, created_at, updated_at, source, interrupt_kind
                 ) VALUES(?1, ?2, ?3, ?4, ?5, ?6, ?6, ?7, ?8)
                 ON CONFLICT(session_id, message_id) DO UPDATE SET
                    role = excluded.role,
                    content_json = excluded.content_json,
                    updated_at = excluded.updated_at,
                    source = excluded.source,
                    interrupt_kind = excluded.interrupt_kind,
                    hidden_at = NULL,
                    recalled_at = NULL",
                params![
                    session_id,
                    message_id,
                    seq,
                    role,
                    content_json,
                    now as i64,
                    source,
                    interrupt_kind,
                ],
            )
            .map_err(|e| e.to_string())?;
        }

        Ok(())
    })
}

/// `session_messages` 的 SELECT 列清单（**本表所有 SELECT 共用这一份，不许各自抄一份**）。
///
/// 由来与 `SESSION_META_COLUMNS` 相同，但这是**同一个坑第二次被踩**：
/// v17 给本表加 `interrupt_kind` 时，三处 SELECT 改了两处，漏掉
/// `find_session_message_by_id` —— 而撤回是唯一走那条查询的路径，于是撤回直接抛
/// `Invalid column index: 10`（撤回逻辑本身没有任何问题）。
/// v11 在 sessions 表上犯过一模一样的错，见 `meta_columns_tests` 的说明。
///
/// 抽成常量后"加列只改这一行"；再配合 `stored_session_message_from_row` 的
/// **按列名取值**，列序与列数都不再是隐式约定，这类错位在结构上不可能发生。
pub(super) const SESSION_MESSAGE_COLUMNS: &str = "message_id, seq, role, content_json, created_at, \
     updated_at, recalled_at, hidden_at, source, interrupt_kind";

pub fn list_visible_session_messages(
    session_id: &str,
) -> Result<Vec<StoredSessionMessage>, String> {
    crate::infra::db::with_connection(|conn| {
        let mut stmt = conn
            .prepare(&format!(
                "SELECT {} FROM session_messages
                 WHERE session_id = ?1
                   AND hidden_at IS NULL
                   AND recalled_at IS NULL
                   AND source != ?2
                 ORDER BY seq ASC",
                SESSION_MESSAGE_COLUMNS
            ))
            .map_err(|e| e.to_string())?;
        let rows = stmt
            .query_map(
                params![session_id, MessageSource::Compact.as_db()],
                stored_session_message_from_row,
            )
            .map_err(|e| e.to_string())?;
        let mut messages = Vec::new();
        for row in rows {
            messages.push(row.map_err(|e| e.to_string())?);
        }
        Ok(messages)
    })
}

/// 分页读取可见消息（懒加载）：以 `before_seq` 为游标向**更早**方向取 `limit` 条，
/// 取出后按 seq 翻正返回。
///
/// - `before_seq = None`：从最新开始（首屏）；
/// - 游标用 seq 而非 offset：seq 会话内单调且唯一（有索引），插入新数据不会漂移；
/// - 与 [`list_visible_session_messages`] 同一套可见性过滤（hidden/recalled/compact）。
pub fn list_visible_session_messages_paged(
    session_id: &str,
    before_seq: Option<i64>,
    limit: usize,
) -> Result<Vec<StoredSessionMessage>, String> {
    crate::infra::db::with_connection(|conn| {
        let mut stmt = conn
            .prepare(&format!(
                "SELECT {} FROM session_messages
                 WHERE session_id = ?1
                   AND hidden_at IS NULL
                   AND recalled_at IS NULL
                   AND source != ?4
                   AND (?2 IS NULL OR seq < ?2)
                 ORDER BY seq DESC
                 LIMIT ?3",
                SESSION_MESSAGE_COLUMNS
            ))
            .map_err(|e| e.to_string())?;
        let rows = stmt
            .query_map(
                rusqlite::params![
                    session_id,
                    before_seq,
                    limit as i64,
                    MessageSource::Compact.as_db()
                ],
                stored_session_message_from_row,
            )
            .map_err(|e| e.to_string())?;
        let mut messages = Vec::new();
        for row in rows {
            messages.push(row.map_err(|e| e.to_string())?);
        }
        // 倒序取出（最新在前），翻正为 ASC 供渲染层直接使用
        messages.reverse();
        Ok(messages)
    })
}

/// 判断一条存储消息是否为"轮的起点"——即用户真实输入（可渲染的 user 消息）。
///
/// 懒加载的窗口必须从轮的起点开始：turn 聚合（多条 assistant/tool 消息合成一个
/// 气泡）以 user 消息开轮；窗口切在轮中间会让首条渲染成"无 user 开头"的半轮。
/// 渲染层 `user_display_content` 在 command 层，此处为避免反向依赖用同样判定内联实现。
/// ⚠️ `source` 判断不可省：`Inject`（崩溃恢复指令 / 方案重定向通知 / 反思修正建议）
/// 也是 **user role + 非空正文** —— 它们是"伪装成一轮用户发言喂给模型的系统指令"。
/// 只按 role 判断会把它们误认成轮起点，于是按轮凑页会把**真实的一轮切在中间**，
/// 正是本函数注释开头要避免的"半轮"。判定改为「真实用户输入」：
/// 来源是 `Chat` 且正文可渲染。
pub(super) fn is_turn_start_message(stored: &StoredSessionMessage) -> bool {
    if stored.role != "user" || stored.source != MessageSource::Chat {
        return false;
    }
    if let Message::User { content } = &stored.content {
        let display = user_display_content_for_paging(content);
        !display.trim().is_empty()
    } else {
        false
    }
}

/// 与渲染层 `user_display_content` 等价的轻量判定：用户消息里是否有可渲染内容
/// （Text 文本或 Image 图片——带图消息可能只有 Image 块）。
/// （tool_result 型 user 消息只携带工具结果块，没有用户输入内容，不构成轮起点。）
fn user_display_content_for_paging(content: &crate::infra::types::models::Content) -> String {
    use crate::infra::types::models::ContentBlock;
    match content {
        crate::infra::types::models::Content::Single(text) => text.clone(),
        crate::infra::types::models::Content::Multiple(blocks) => {
            let mut parts = String::new();
            for b in blocks {
                match b {
                    ContentBlock::Text { text }
                        if !text.trim().is_empty() => {
                            parts.push_str(text.trim());
                            parts.push('\n');
                        }
                    // 图片是可渲染内容：有图即视为有用户输入（与渲染层一致）
                    ContentBlock::Image { .. } => {
                        parts.push_str("[image]");
                    }
                    _ => {}
                }
            }
            parts
        }
    }
}

/// 取一页消息并按**轮**凑页：一轮 = 一条可渲染 user 消息（轮起点）+ 其后所有消息
/// （assistant / tool_result 等，直到下一条轮起点前）。
///
/// 与按条数分页的区别：长任务一轮可能产生几十条消息（每次工具调用 2 条），
/// 按条数切割会把一轮切成多页（页内出现"无 user 开头"的半轮）甚至一页装不下
/// 一整轮；按轮凑页保证**每页都是完整的 N 轮**，页大小随轮的密度自适应。
///
/// 算法：
/// 1. 从游标向前按批取（BUFFER 条/批），累计到 `collected`（保持 ASC）；
/// 2. 数 `collected` 里的轮起点个数，超过 `max_turns` 即停止；
/// 3. 裁剪：从最新端往前保留 `max_turns` 个完整轮，更早的轮裁掉（has_more=true）；
///    到达会话最老处仍未凑满则全保留（has_more=false）。
///
/// 返回 (消息, 是否还有更早的轮)。
pub fn load_visible_turns_page(
    session_id: &str,
    before_seq: Option<i64>,
    max_turns: usize,
) -> Result<(Vec<StoredSessionMessage>, bool), String> {
    const BUFFER: usize = 200; // 每批缓冲：覆盖 max_turns 轮 × 平均每轮条数
    const MAX_BATCHES: usize = 50; // 防御上限：极端长轮最多向前补 50 批（1 万条）

    let mut collected: Vec<StoredSessionMessage> = Vec::new();
    let mut cursor = before_seq;
    let mut reached_oldest = false;
    let mut enough_turns = false;
    let mut batches = 0usize;

    loop {
        batches += 1;
        if batches > MAX_BATCHES {
            reached_oldest = true; // 防御上限触发：停止补取，按现状裁剪
            break;
        }
        let batch = list_visible_session_messages_paged(session_id, cursor, BUFFER)?;
        if batch.is_empty() {
            reached_oldest = true; // 游标之前没有更早消息
            break;
        }
        let batch_len = batch.len(); // 先记录：splice 会 move batch
        collected.splice(0..0, batch); // batch 是 ASC，插到头部保持整体 ASC
        cursor = Some(collected[0].seq as i64);

        let turns = count_turn_starts(&collected);
        if turns > max_turns {
            enough_turns = true; // 轮数已凑够（且仍有更早轮未取）
            break;
        }
        if batch_len < BUFFER {
            reached_oldest = true; // 本批不满 = 已到会话最老
            break;
        }
    }

    // 轮起点下标（升序）
    let starts: Vec<usize> = collected
        .iter()
        .enumerate()
        .filter(|(_, m)| is_turn_start_message(m))
        .map(|(i, _)| i)
        .collect();

    if starts.len() > max_turns {
        // 从最新端往前保留 max_turns 个完整轮，更早的裁掉
        let cut = starts[starts.len() - max_turns];
        let page = collected.split_off(cut);
        // 被裁掉的部分非空 → 还有更早的轮；即使 reached_oldest 也以裁剪为准
        Ok((page, true))
    } else {
        // 轮数未超：全保留。collected[0] 可能不是轮起点（跨缓冲的半轮前半）——
        // 此时要么 reached_oldest（到头了，半轮前半就是最老历史），要么防御上限触发
        Ok((collected, !reached_oldest || enough_turns))
    }
}

/// 数一条消息序列里的轮起点个数
pub(super) fn count_turn_starts(messages: &[StoredSessionMessage]) -> usize {
    messages.iter().filter(|m| is_turn_start_message(m)).count()
}

pub fn find_session_message_by_id(
    session_id: &str,
    message_id: &str,
) -> Result<Option<StoredSessionMessage>, String> {
    crate::infra::db::with_connection(|conn| {
        find_session_message_by_id_in(conn, session_id, message_id)
    })
}

/// [`find_session_message_by_id`] 的 connection 版。
///
/// 抽出来是为了能在测试里**跑真实 SQL**（不必依赖全局 DB 单例）—— 见
/// `message_columns_tests::find_message_by_id_round_trips_interrupt_kind`。
/// 撤回（`command/session.rs` / `command/checkpoint.rs`）是这条查询的唯一调用方，
/// 2026-09-21 正是因为它的 SELECT 漏列而整体失败。
pub(super) fn find_session_message_by_id_in(
    conn: &rusqlite::Connection,
    session_id: &str,
    message_id: &str,
) -> Result<Option<StoredSessionMessage>, String> {
    conn.query_row(
        &format!(
            "SELECT {} FROM session_messages
             WHERE session_id = ?1 AND message_id = ?2
               AND hidden_at IS NULL
               AND recalled_at IS NULL",
            SESSION_MESSAGE_COLUMNS
        ),
        params![session_id, message_id],
        stored_session_message_from_row,
    )
    .optional()
    .map_err(|e| e.to_string())
}

pub fn hide_session_messages_from_seq(
    session_id: &str,
    seq: usize,
    recalled: bool,
) -> Result<(), String> {
    crate::infra::db::with_connection(|conn| {
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_millis() as u64;
        if recalled {
            conn.execute(
                "UPDATE session_messages
                 SET hidden_at = COALESCE(hidden_at, ?3), recalled_at = COALESCE(recalled_at, ?3)
                 WHERE session_id = ?1 AND seq >= ?2",
                params![session_id, seq as i64, now as i64],
            )
        } else {
            conn.execute(
                "UPDATE session_messages
                 SET hidden_at = COALESCE(hidden_at, ?3)
                 WHERE session_id = ?1 AND seq >= ?2",
                params![session_id, seq as i64, now as i64],
            )
        }
        .map_err(|e| e.to_string())?;
        Ok(())
    })
}

pub fn delete_session_messages_from_seq(session_id: &str, seq: usize) -> Result<(), String> {
    crate::infra::db::with_connection(|conn| {
        conn.execute(
            "DELETE FROM session_messages WHERE session_id = ?1 AND seq >= ?2",
            params![session_id, seq as i64],
        )
        .map_err(|e| e.to_string())?;
        Ok(())
    })
}

/// 删除 session_messages 中指定 source 的消息（压缩时清理 internal/background）
pub fn delete_session_messages_by_source(
    session_id: &str,
    sources: &[MessageSource],
) -> Result<usize, String> {
    crate::infra::db::with_connection(|conn| {
        let mut deleted = 0usize;
        for src in sources {
            deleted += conn
                .execute(
                    "DELETE FROM session_messages WHERE session_id = ?1 AND source = ?2",
                    params![session_id, src.as_db()],
                )
                .map_err(|e| e.to_string())?;
        }
        Ok(deleted)
    })
}

/// 隐藏 session_messages 中已不在 memory.message_ids 里的孤儿行
/// 压缩后 message_ids 被替换为新ID，旧行需要标记 hidden 以保持两表一致
pub fn hide_orphan_session_messages(
    session_id: &str,
    alive_message_ids: &[String],
) -> Result<usize, String> {
    crate::infra::db::with_connection(|conn| {
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_millis() as i64;
        // 将不在 alive_message_ids 中且未被隐藏的行标记 hidden_at
        let placeholders: Vec<String> = alive_message_ids
            .iter()
            .enumerate()
            .map(|(i, _)| format!("?{}", i + 3))
            .collect();
        let sql = if alive_message_ids.is_empty() {
            "UPDATE session_messages
             SET hidden_at = COALESCE(hidden_at, ?1), updated_at = ?2
             WHERE session_id = ?3 AND hidden_at IS NULL"
                .to_string()
        } else {
            format!(
                "UPDATE session_messages
                 SET hidden_at = COALESCE(hidden_at, ?1), updated_at = ?2
                 WHERE session_id = ?3 AND hidden_at IS NULL AND message_id NOT IN ({})",
                placeholders.join(",")
            )
        };
        let mut params: Vec<Box<dyn rusqlite::types::ToSql>> = vec![
            Box::new(now),
            Box::new(now),
            Box::new(session_id.to_string()),
        ];
        for id in alive_message_ids {
            params.push(Box::new(id.clone()));
        }
        let affected = conn
            .execute(
                &sql,
                rusqlite::params_from_iter(params.iter().map(|p| p.as_ref())),
            )
            .map_err(|e| e.to_string())?;
        Ok(affected)
    })
}

pub fn session_messages_count(session_id: &str) -> Result<usize, String> {
    crate::infra::db::with_connection(|conn| {
        let count: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM session_messages WHERE session_id = ?1",
                [session_id],
                |row| row.get(0),
            )
            .map_err(|e| e.to_string())?;
        Ok(count.max(0) as usize)
    })
}

/// 一次查询取回**可见消息**的总条数与最新 seq：`(total_count, newest_seq)`。
///
/// 用途：分页接口要给前端补齐"窗口边界 + 后端总条数"，前端据此判断本地窗口
/// 是否已经落后于数据库（stale 判定）。如果分成两次查询（COUNT 一次、MAX 一次），
/// 期间若有新消息落库，两次数值可能对不上（count 已含新消息、max 却是旧的）。
///
/// 可见性条件与 [`list_visible_session_messages`] /
/// [`list_visible_session_messages_paged`] **完全一致**，不额外造过滤：
/// - `hidden_at IS NULL`：被隐藏的（回滚/压缩孤儿行）不算；
/// - `recalled_at IS NULL`：被撤回的不算；
/// - `source != Compact`：压缩摘要不进界面窗口，也不算可见消息。
///
/// 空会话返回 `(0, None)` —— `MAX(seq)` 在没有行时本身就是 NULL。
pub fn session_message_stats(session_id: &str) -> Result<(i64, Option<i64>), String> {
    crate::infra::db::with_connection(|conn| session_message_stats_in(conn, session_id))
}

/// [`session_message_stats`] 的 connection 版。
///
/// 抽出来是为了能在测试里跑真实 SQL（不必依赖全局 DB 单例），
/// 与 [`find_session_message_by_id_in`] 同一做法。
pub(super) fn session_message_stats_in(
    conn: &rusqlite::Connection,
    session_id: &str,
) -> Result<(i64, Option<i64>), String> {
    conn.query_row(
        "SELECT COUNT(*), MAX(seq) FROM session_messages
         WHERE session_id = ?1
           AND hidden_at IS NULL
           AND recalled_at IS NULL
           AND source != ?2",
        params![session_id, MessageSource::Compact.as_db()],
        |row| {
            let count: i64 = row.get(0)?;
            // 空结果集时 MAX(seq) 是 SQL NULL —— 正好映射成 None（= 本会话没有可见消息）
            let newest_seq: Option<i64> = row.get(1)?;
            Ok((count.max(0), newest_seq))
        },
    )
    .map_err(|e| e.to_string())
}

/// 从 `session_messages` 的一行还原消息。
///
/// **一律按列名取值，不用列序号**。列序号方案要求"每一条 SELECT 的列清单与顺序都和这里的
/// 下标严格对应"，而这个约定只能靠注释提醒 —— 2026-09-21 的撤回故障正是这么来的：
/// `find_session_message_by_id` 漏了 v17 新增的 `interrupt_kind`，映射器读第 11 列时直接抛
/// `Invalid column index: 10`（撤回逻辑本身毫无问题）。
/// 按列名之后：列序无关；万一某条 SELECT 漏了列，报的是
/// `Invalid column name: interrupt_kind`（直接指出缺哪个），而不是一个难定位的下标越界。
///
/// 列名集合必须与 [`SESSION_MESSAGE_COLUMNS`] 一致，
/// 由测试 `message_columns_cover_every_name_the_mapper_reads` 钉住。
pub(super) fn stored_session_message_from_row(row: &Row<'_>) -> rusqlite::Result<StoredSessionMessage> {
    let seq: i64 = row.get("seq")?;
    let content_json: String = row.get("content_json")?;
    let created_at: i64 = row.get("created_at")?;
    let updated_at: Option<i64> = row.get("updated_at")?;
    let recalled_at: Option<i64> = row.get("recalled_at")?;
    let hidden_at: Option<i64> = row.get("hidden_at")?;
    let content = serde_json::from_str(&content_json).map_err(|err| {
        // 错误上下文里的列号现查一次：取值本身已不依赖列序，这里只是让报错指向正确的列
        rusqlite::Error::FromSqlConversionFailure(
            row.as_ref().column_index("content_json").unwrap_or(0),
            rusqlite::types::Type::Text,
            Box::new(err),
        )
    })?;
    Ok(StoredSessionMessage {
        message_id: row.get("message_id")?,
        seq: seq.max(0) as usize,
        role: row.get("role")?,
        content,
        created_at: created_at.max(0) as u64,
        updated_at: updated_at.map(|value| value.max(0) as u64),
        recalled_at: recalled_at.map(|value| value.max(0) as u64),
        hidden_at: hidden_at.map(|value| value.max(0) as u64),
        // 读边界的唯一一处「字符串 → 枚举」转换。未知值报错而不是静默降级，
        // 并把出错的值与消息号一并带进错误文本，便于直接定位那一行数据。
        source: {
            let raw: String = row.get("source")?;
            MessageSource::from_db(&raw).map_err(|msg| {
                let column = row.as_ref().column_index("source").unwrap_or(0);
                rusqlite::Error::FromSqlConversionFailure(
                    column,
                    rusqlite::types::Type::Text,
                    Box::new(std::io::Error::new(
                        std::io::ErrorKind::InvalidData,
                        format!(
                            "{}（message_id={}）",
                            msg,
                            row.get::<_, String>("message_id").unwrap_or_default()
                        ),
                    )),
                )
            })?
        },
        interrupt_kind: row.get("interrupt_kind")?,
    })
}

pub fn load_session(id: &str) -> Result<SessionMemory, String> {
    crate::infra::db::with_connection(|conn| {
        let json = conn
            .query_row(
                "SELECT memory_json FROM session_memory
                 JOIN sessions ON sessions.id = session_memory.session_id
                 WHERE session_id = ?1 AND sessions.deleted_at IS NULL",
                [id],
                |row| row.get::<_, String>(0),
            )
            .optional()
            .map_err(|e| e.to_string())?
            .ok_or_else(|| format!("会话 {} 不存在", id))?;

        let mut memory: SessionMemory = serde_json::from_str(&json).map_err(|e| e.to_string())?;

        // 从 session_messages 表重建 messages 和 sources
        // 按 message_ids 的顺序加载，而非依赖 seq（seq 可能因 upsert 错位）
        if !memory.message_ids.is_empty() {
            let placeholders: Vec<String> = memory
                .message_ids
                .iter()
                .enumerate()
                .map(|(i, _)| format!("?{}", i + 2))
                .collect();
            let sql = format!(
                "SELECT message_id, content_json, source, interrupt_kind FROM session_messages
                 WHERE session_id = ?1 AND message_id IN ({})",
                placeholders.join(",")
            );
            let mut params: Vec<Box<dyn rusqlite::types::ToSql>> = vec![Box::new(id.to_string())];
            for mid in &memory.message_ids {
                params.push(Box::new(mid.clone()));
            }
            let mut stmt = conn.prepare(&sql).map_err(|e| e.to_string())?;
            let rows = stmt
                .query_map(
                    rusqlite::params_from_iter(params.iter().map(|p| p.as_ref())),
                    |row| {
                        Ok((
                            row.get::<_, String>(0)?,
                            row.get::<_, String>(1)?,
                            row.get::<_, String>(2)?,
                            row.get::<_, Option<String>>(3)?,
                        ))
                    },
                )
                .map_err(|e| e.to_string())?;
            // source 在**读边界**就还原成枚举（未知值报错），不把字符串带进内存：
            // 内存里的 `sources` 从此只有 `MessageSource` 一种形态，
            // "这条消息该被谁看见"不会再出现第二套字符串口径。
            let mut content_by_id: HashMap<String, (String, MessageSource, Option<String>)> =
                HashMap::new();
            for row in rows {
                let (mid, content_json, raw_source, kind) = row.map_err(|e| e.to_string())?;
                let source = MessageSource::from_db(&raw_source)
                    .map_err(|msg| format!("会话 {} 的消息 {} 读取失败：{}", id, mid, msg))?;
                content_by_id.insert(mid, (content_json, source, kind));
            }
            // 严格按 message_ids 数组顺序重建 messages / sources / interrupt_kinds，保证三数组平行
            for mid in &memory.message_ids {
                if let Some((content_json, source, kind)) = content_by_id.get(mid) {
                    if let Ok(msg) = serde_json::from_str::<Message>(content_json) {
                        memory.messages.push(msg);
                        memory.sources.push(*source);
                        memory.interrupt_kinds.push(kind.clone());
                    }
                }
            }
        }

        Ok(memory)
    })
}

pub fn upsert_context_snapshot(snapshot: &SessionContextSnapshot) -> Result<(), String> {
    crate::infra::db::with_connection(|conn| {
        let snapshot_json = serde_json::to_string(snapshot).map_err(|e| e.to_string())?;
        conn.execute(
            "INSERT INTO session_context_snapshots(session_id, snapshot_json, updated_at)
             VALUES(?1, ?2, ?3)
             ON CONFLICT(session_id) DO UPDATE SET
                snapshot_json = excluded.snapshot_json,
                updated_at = excluded.updated_at",
            params![
                snapshot.session_id,
                snapshot_json,
                snapshot.created_at as i64
            ],
        )
        .map_err(|e| e.to_string())?;
        Ok(())
    })
}

/// 上下文快照用量更新参数（收拢多字段实测 token 与缓存统计，避免超长参数列表）。
pub struct ContextSnapshotUsageUpdate<'a> {
    pub provider_input_tokens: u64,
    pub provider_output_tokens: u64,
    pub provider_total_tokens: u64,
    pub drift_percent: Option<f32>,
    pub cache_hit_tokens: Option<u64>,
    pub cache_miss_tokens: Option<u64>,
    pub cache_source: Option<&'a str>,
    pub cache_point: Option<&'a crate::infra::types::models::CacheHitPoint>,
}

pub fn update_context_snapshot_usage(
    session_id: &str,
    usage: ContextSnapshotUsageUpdate<'_>,
) -> Result<Option<SessionContextSnapshot>, String> {
    crate::infra::db::with_connection(|conn| {
        let snapshot_json = conn
            .query_row(
                "SELECT snapshot_json FROM session_context_snapshots
                 JOIN sessions ON sessions.id = session_context_snapshots.session_id
                 WHERE session_context_snapshots.session_id = ?1 AND sessions.deleted_at IS NULL",
                [session_id],
                |row| row.get::<_, String>(0),
            )
            .optional()
            .map_err(|e| e.to_string())?;

        let Some(snapshot_json) = snapshot_json else {
            return Ok(None);
        };

        let mut snapshot: SessionContextSnapshot =
            serde_json::from_str(&snapshot_json).map_err(|e| e.to_string())?;
        snapshot.provider_input_tokens = Some(usage.provider_input_tokens);
        snapshot.provider_output_tokens = Some(usage.provider_output_tokens);
        snapshot.provider_total_tokens = Some(usage.provider_total_tokens);
        snapshot.drift_percent = usage.drift_percent;
        // 缓存命中：只在本次拿到值时覆盖（None 表示这家没报告，不要抹掉上一次已知的结果）
        if usage.cache_hit_tokens.is_some() || usage.cache_miss_tokens.is_some() {
            snapshot.cache_hit_tokens = usage.cache_hit_tokens;
            snapshot.cache_miss_tokens = usage.cache_miss_tokens;
            snapshot.cache_source = usage.cache_source.map(|s| s.to_string());
        }
        // 逐 loop 趋势：追加本次记录，只保留最近 N 条。
        // 同一 loop 重复上报（重试/补充统计）时覆盖而不是堆叠，避免趋势出现重复柱子。
        if let Some(point) = usage.cache_point {
            match snapshot.cache_history.last_mut() {
                Some(last) if last.loop_count == point.loop_count => *last = point.clone(),
                _ => snapshot.cache_history.push(point.clone()),
            }
            let keep = crate::infra::types::constants::CACHE_HISTORY_MAX_POINTS;
            if snapshot.cache_history.len() > keep {
                let overflow = snapshot.cache_history.len() - keep;
                snapshot.cache_history.drain(0..overflow);
            }
        }

        let snapshot_json = serde_json::to_string(&snapshot).map_err(|e| e.to_string())?;
        conn.execute(
            "UPDATE session_context_snapshots
             SET snapshot_json = ?2, updated_at = ?3
             WHERE session_id = ?1",
            params![session_id, snapshot_json, snapshot.created_at as i64],
        )
        .map_err(|e| e.to_string())?;

        Ok(Some(snapshot))
    })
}

pub fn get_context_snapshot(session_id: &str) -> Result<Option<SessionContextSnapshot>, String> {
    crate::infra::db::with_connection(|conn| {
        conn.query_row(
            "SELECT snapshot_json FROM session_context_snapshots
             JOIN sessions ON sessions.id = session_context_snapshots.session_id
             WHERE session_context_snapshots.session_id = ?1 AND sessions.deleted_at IS NULL",
            [session_id],
            |row| row.get::<_, String>(0),
        )
        .optional()
        .map_err(|e| e.to_string())?
        .map(|json| serde_json::from_str(&json).map_err(|e| e.to_string()))
        .transpose()
    })
}
