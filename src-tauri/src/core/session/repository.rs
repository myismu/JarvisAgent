//! # repository.rs — 会话 SQLite 仓储
//!
//! 封装会话元数据、完整记忆、消息展开索引和列表筛选的 SQLite 读写。
//!
//! ## Key Exports
//! - `SessionListFilter`: 会话列表筛选条件
//! - `upsert_session()`: 保存会话元数据和记忆
//! - `load_session()`: 读取完整会话记忆
//! - `list_sessions()`: 查询会话列表并支持筛选
//! - `upsert_context_snapshot()`: 保存最近一次上下文 token 快照
//! - `set_last_active_session_id()`: 持久化最后活跃会话
//!
//! ## Dependencies
//! - Internal: `crate::infra::db`, `crate::infra::types::models`
//! - External: `rusqlite`, `serde`

use std::collections::HashMap;
use rusqlite::{params, OptionalExtension, Row};
use serde::{Deserialize, Serialize};

use crate::infra::types::models::{Message, SessionContextSnapshot, SessionMemory};
use crate::core::session::{SessionMeta, SessionTokenTotals};

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
    pub source: String,
    pub turn_id: Option<String>,
    /// 中断类型（`None` = 非中断消息）。取值见 `InterruptKind::as_str`。
    pub interrupt_kind: Option<String>,
}

#[derive(Serialize, Deserialize, Debug, Clone, Default)]
#[serde(rename_all = "camelCase")]
pub struct SessionListFilter {
    #[serde(default)]
    pub keyword: Option<String>,
    #[serde(default)]
    pub from_ts: Option<u64>,
    #[serde(default)]
    pub to_ts: Option<u64>,
    #[serde(default)]
    pub profile_id: Option<String>,
    #[serde(default)]
    pub model: Option<String>,
    #[serde(default)]
    pub tool: Option<String>,
    #[serde(default)]
    pub has_tool_calls: Option<bool>,
    #[serde(default)]
    pub limit: Option<usize>,
    #[serde(default)]
    pub offset: Option<usize>,
}

pub fn upsert_session(meta: &SessionMeta, memory: &SessionMemory) -> Result<(), String> {
    crate::infra::db::with_transaction(|tx| {
        tx.execute(
            "INSERT INTO sessions(
                id, title, created_at, updated_at, message_count, is_smart_named,
                profile_id, total_input_tokens, total_output_tokens,
                total_cache_hit_tokens, total_cache_miss_tokens,
                title_source, project_id, thinking_mode, deleted_at
            ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, NULL)
            ON CONFLICT(id) DO UPDATE SET
                title = excluded.title,
                created_at = excluded.created_at,
                updated_at = excluded.updated_at,
                message_count = excluded.message_count,
                is_smart_named = excluded.is_smart_named,
                profile_id = excluded.profile_id,
                total_input_tokens = excluded.total_input_tokens,
                total_output_tokens = excluded.total_output_tokens,
                total_cache_hit_tokens = excluded.total_cache_hit_tokens,
                total_cache_miss_tokens = excluded.total_cache_miss_tokens,
                title_source = excluded.title_source,
                project_id = excluded.project_id,
                thinking_mode = excluded.thinking_mode",
            // 注意：这里**故意不重置 `deleted_at`**（2026-09-21 沐拍板）。
            // 曾经的写法是 `deleted_at = NULL` —— 已删会话只要再被写一次就"自动复活"，
            // 用户会莫名其妙看到删掉的会话又回来了。删除应当是确定的：恢复只能走显式入口
            // （`restore_session`）。新行的 `deleted_at` 由 INSERT 里的 NULL 兜底。
            params![
                meta.id,
                meta.title,
                meta.created_at as i64,
                meta.updated_at as i64,
                meta.message_count as i64,
                if meta.is_smart_named { 1 } else { 0 },
                meta.profile_id,
                meta.total_input_tokens as i64,
                meta.total_output_tokens as i64,
                meta.total_cache_hit_tokens as i64,
                meta.total_cache_miss_tokens as i64,
                meta.title_source,
                meta.project_id,
                // 布尔列显式转 1/0（rusqlite 虽能直接绑 bool，但显式写死口径更清楚）
                if meta.thinking_mode { 1 } else { 0 },
            ],
        )
        .map_err(|e| e.to_string())?;

        let memory_json = serde_json::to_string(memory).map_err(|e| e.to_string())?;
        tx.execute(
            "INSERT INTO session_memory(session_id, memory_json) VALUES(?1, ?2)
             ON CONFLICT(session_id) DO UPDATE SET memory_json = excluded.memory_json",
            params![meta.id, memory_json],
        )
        .map_err(|e| e.to_string())?;

        Ok(())
    })
}

/// 逐请求累加会话的 token 用量，返回累加**之后**的累计值。
///
/// 为什么单独开一个函数而不是复用 `upsert_session`：
/// `upsert_session` 收的是完整 `SessionMeta`，它把四个 `total_*` 列**整值覆盖**写入。
/// 而逐请求路径手里只有"本次请求的增量"，没有（也不该为了写它去读一遍）完整 meta——
/// 读出来再写回去会和并发写入打架，也会把期间别人的累加抹掉。
/// 这里直接用 SQL 的 `x = x + ?` 做原子累加。
///
/// 落点：`pipeline::update_provider_usage_snapshot`，即每个 loop 拿到 usage 之后。
/// 回合收尾的 `save_session` 只再补**子代理**那部分增量——子代理走独立循环
/// （`tools/agent_tools/subagent.rs`），不经过上面那个函数。
///
/// 返回 `None` 表示会话不存在或已软删（`deleted_at` 非空），调用方据此跳过事件推送。
pub fn accumulate_session_token_usage(
    session_id: &str,
    delta: SessionTokenTotals,
) -> Result<Option<SessionTokenTotals>, String> {
    crate::infra::db::with_connection(|conn| {
        let affected = conn
            .execute(
                "UPDATE sessions SET
                    total_input_tokens = total_input_tokens + ?2,
                    total_output_tokens = total_output_tokens + ?3,
                    total_cache_hit_tokens = total_cache_hit_tokens + ?4,
                    total_cache_miss_tokens = total_cache_miss_tokens + ?5
                 WHERE id = ?1 AND deleted_at IS NULL",
                params![
                    session_id,
                    delta.input as i64,
                    delta.output as i64,
                    delta.cache_hit as i64,
                    delta.cache_miss as i64,
                ],
            )
            .map_err(|e| e.to_string())?;

        if affected == 0 {
            return Ok(None);
        }

        // 同一把连接里紧接着读回，拿到的是本次累加之后的值，
        // 供调用方原样推给前端（前端是整值覆盖，不是自己再累加一遍）。
        let totals = conn
            .query_row(
                "SELECT total_input_tokens, total_output_tokens,
                        total_cache_hit_tokens, total_cache_miss_tokens
                 FROM sessions WHERE id = ?1",
                [session_id],
                |row| {
                    Ok(SessionTokenTotals {
                        input: row.get::<_, i64>(0)?.max(0) as u64,
                        output: row.get::<_, i64>(1)?.max(0) as u64,
                        cache_hit: row.get::<_, i64>(2)?.max(0) as u64,
                        cache_miss: row.get::<_, i64>(3)?.max(0) as u64,
                    })
                },
            )
            .optional()
            .map_err(|e| e.to_string())?;

        Ok(totals)
    })
}

pub fn append_or_upsert_session_messages(
    session_id: &str,
    messages: &[Message],
    message_ids: &[String],
    sources: &[String],
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
            let source = sources.get(idx).map(|s| s.as_str()).unwrap_or("chat");
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
                    params![session_id, seq, message_id, now as i64, source, interrupt_kind],
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
const SESSION_MESSAGE_COLUMNS: &str = "message_id, seq, role, content_json, created_at, \
     updated_at, recalled_at, hidden_at, source, turn_id, interrupt_kind";

pub fn list_visible_session_messages(session_id: &str) -> Result<Vec<StoredSessionMessage>, String> {
    crate::infra::db::with_connection(|conn| {
        let mut stmt = conn
            .prepare(&format!(
                "SELECT {} FROM session_messages
                 WHERE session_id = ?1
                   AND hidden_at IS NULL
                   AND recalled_at IS NULL
                   AND source != 'compact'
                 ORDER BY seq ASC",
                SESSION_MESSAGE_COLUMNS
            ))
            .map_err(|e| e.to_string())?;
        let rows = stmt
            .query_map([session_id], stored_session_message_from_row)
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
                   AND source != 'compact'
                   AND (?2 IS NULL OR seq < ?2)
                 ORDER BY seq DESC
                 LIMIT ?3",
                SESSION_MESSAGE_COLUMNS
            ))
            .map_err(|e| e.to_string())?;
        let rows = stmt
            .query_map(
                rusqlite::params![session_id, before_seq, limit as i64],
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
fn is_turn_start_message(stored: &StoredSessionMessage) -> bool {
    if stored.role != "user" {
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
fn user_display_content_for_paging(
    content: &crate::infra::types::models::Content,
) -> String {
    use crate::infra::types::models::ContentBlock;
    match content {
        crate::infra::types::models::Content::Single(text) => text.clone(),
        crate::infra::types::models::Content::Multiple(blocks) => {
            let mut parts = String::new();
            for b in blocks {
                match b {
                    ContentBlock::Text { text } => {
                        if !text.trim().is_empty() {
                            parts.push_str(text.trim());
                            parts.push('\n');
                        }
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
fn count_turn_starts(messages: &[StoredSessionMessage]) -> usize {
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
fn find_session_message_by_id_in(
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
    sources: &[&str],
) -> Result<usize, String> {
    crate::infra::db::with_connection(|conn| {
        let mut deleted = 0usize;
        for src in sources {
            deleted += conn.execute(
                "DELETE FROM session_messages WHERE session_id = ?1 AND source = ?2",
                params![session_id, *src],
            ).map_err(|e| e.to_string())?;
        }
        Ok(deleted)
    })
}

/// 隐藏 session_messages 中已不在 memory.message_ids 里的孤儿行
/// 压缩后 message_ids 被替换为新ID，旧行需要标记 hidden 以保持两表一致
pub fn hide_orphan_session_messages(session_id: &str, alive_message_ids: &[String]) -> Result<usize, String> {
    crate::infra::db::with_connection(|conn| {
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_millis() as i64;
        // 将不在 alive_message_ids 中且未被隐藏的行标记 hidden_at
        let placeholders: Vec<String> = alive_message_ids.iter().enumerate()
            .map(|(i, _)| format!("?{}", i + 3))
            .collect();
        let sql = if alive_message_ids.is_empty() {
            "UPDATE session_messages
             SET hidden_at = COALESCE(hidden_at, ?1), updated_at = ?2
             WHERE session_id = ?3 AND hidden_at IS NULL".to_string()
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
        let affected = conn.execute(
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
fn stored_session_message_from_row(row: &Row<'_>) -> rusqlite::Result<StoredSessionMessage> {
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
        source: row.get("source")?,
        turn_id: row.get("turn_id")?,
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

        let mut memory: SessionMemory =
            serde_json::from_str(&json).map_err(|e| e.to_string())?;

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
            let mut params: Vec<Box<dyn rusqlite::types::ToSql>> =
                vec![Box::new(id.to_string())];
            for mid in &memory.message_ids {
                params.push(Box::new(mid.clone()));
            }
            let mut stmt = conn.prepare(&sql).map_err(|e| e.to_string())?;
            let rows = stmt
                .query_map(
                    rusqlite::params_from_iter(params.iter().map(|p| p.as_ref())),
                    |row| Ok((
                        row.get::<_, String>(0)?,
                        row.get::<_, String>(1)?,
                        row.get::<_, String>(2)?,
                        row.get::<_, Option<String>>(3)?,
                    )),
                )
                .map_err(|e| e.to_string())?;
            let mut content_by_id: HashMap<String, (String, String, Option<String>)> = HashMap::new();
            for row in rows {
                let (mid, content_json, source, kind) = row.map_err(|e| e.to_string())?;
                content_by_id.insert(mid, (content_json, source, kind));
            }
            // 严格按 message_ids 数组顺序重建 messages / sources / interrupt_kinds，保证三数组平行
            for mid in &memory.message_ids {
                if let Some((content_json, source, kind)) = content_by_id.get(mid) {
                    if let Ok(msg) = serde_json::from_str::<Message>(content_json) {
                        memory.messages.push(msg);
                        memory.sources.push(source.clone());
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

pub fn update_context_snapshot_usage(
    session_id: &str,
    provider_input_tokens: u64,
    provider_output_tokens: u64,
    provider_total_tokens: u64,
    drift_percent: Option<f32>,
    cache_hit_tokens: Option<u64>,
    cache_miss_tokens: Option<u64>,
    cache_source: Option<&str>,
    cache_point: Option<&crate::infra::types::models::CacheHitPoint>,
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
        snapshot.provider_input_tokens = Some(provider_input_tokens);
        snapshot.provider_output_tokens = Some(provider_output_tokens);
        snapshot.provider_total_tokens = Some(provider_total_tokens);
        snapshot.drift_percent = drift_percent;
        // 缓存命中：只在本次拿到值时覆盖（None 表示这家没报告，不要抹掉上一次已知的结果）
        if cache_hit_tokens.is_some() || cache_miss_tokens.is_some() {
            snapshot.cache_hit_tokens = cache_hit_tokens;
            snapshot.cache_miss_tokens = cache_miss_tokens;
            snapshot.cache_source = cache_source.map(|s| s.to_string());
        }
        // 逐 loop 趋势：追加本次记录，只保留最近 N 条。
        // 同一 loop 重复上报（重试/补充统计）时覆盖而不是堆叠，避免趋势出现重复柱子。
        if let Some(point) = cache_point {
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

/// `SessionMeta` 的 SELECT 列清单（**必须与 `session_meta_from_row` 的列序号一一对应**）。
///
/// 抽成常量是为了从结构上杜绝一类反复出现的 bug：新增字段时只改了部分 SELECT，
/// 未改的那条查询就会在执行期抛 `Invalid column index`（例如 v11 引入
/// `thinking_mode` 时漏改 `get_session_meta`，直接导致切换会话失败）。
/// 以后只需要在这里加一列，并把 `session_meta_from_row` 的索引往后挪。
const SESSION_META_COLUMNS: &str = "s.id, s.title, s.created_at, s.updated_at, s.message_count, \
     s.is_smart_named, s.profile_id, s.total_input_tokens, s.total_output_tokens, s.title_source, \
     s.project_id, p.path, s.thinking_mode, s.total_cache_hit_tokens, s.total_cache_miss_tokens";

pub fn get_session_meta(id: &str) -> Result<SessionMeta, String> {
    crate::infra::db::with_connection(|conn| {
        let sql = format!(
            "SELECT {} FROM sessions s \
             LEFT JOIN projects p ON s.project_id = p.id \
             WHERE s.id = ?1 AND s.deleted_at IS NULL",
            SESSION_META_COLUMNS
        );
        conn.query_row(&sql, [id], session_meta_from_row)
            .optional()
            .map_err(|e| e.to_string())?
            .ok_or_else(|| format!("会话 {} 不存在", id))
    })
}

pub fn list_sessions(filter: Option<&SessionListFilter>) -> Result<Vec<SessionMeta>, String> {
    crate::infra::db::with_connection(|conn| {
        let mut sessions = Vec::new();
        let sql = format!(
            "SELECT {} FROM sessions s \
             LEFT JOIN projects p ON s.project_id = p.id \
             WHERE s.deleted_at IS NULL \
             ORDER BY s.updated_at DESC",
            SESSION_META_COLUMNS
        );
        let mut stmt = conn.prepare(&sql).map_err(|e| e.to_string())?;
        let rows = stmt
            .query_map([], session_meta_from_row)
            .map_err(|e| e.to_string())?;
        for row in rows {
            let meta = row.map_err(|e| e.to_string())?;
            if matches_filter(conn, &meta, filter)? {
                sessions.push(meta);
            }
        }

        if let Some(filter) = filter {
            let offset = filter.offset.unwrap_or(0);
            let limit = filter.limit.unwrap_or(sessions.len());
            sessions = sessions.into_iter().skip(offset).take(limit).collect();
        }
        Ok(sessions)
    })
}

pub fn ensure_session_exists(id: &str, title: Option<&str>, created_at: u64) -> Result<(), String> {
    crate::infra::db::with_connection(|conn| {
        let existing: Option<String> = conn
            .query_row(
                "SELECT id FROM sessions WHERE id = ?1 AND deleted_at IS NULL",
                [id],
                |row| row.get(0),
            )
            .optional()
            .map_err(|e| e.to_string())?;
        if existing.is_some() {
            return Ok(());
        }

        let title = title.unwrap_or("导入会话");
        conn.execute(
            "INSERT INTO sessions(
                id, title, created_at, updated_at, message_count, is_smart_named,
                profile_id, total_input_tokens, total_output_tokens, title_source, project_id, deleted_at
            ) VALUES(?1, ?2, ?3, ?3, 0, 0, NULL, 0, 0, 'default', NULL, NULL)",
            params![id, title, created_at as i64],
        )
        .map_err(|e| e.to_string())?;

        let memory_json =
            serde_json::to_string(&SessionMemory::default()).map_err(|e| e.to_string())?;
        conn.execute(
            "INSERT INTO session_memory(session_id, memory_json) VALUES(?1, ?2)
             ON CONFLICT(session_id) DO NOTHING",
            params![id, memory_json],
        )
        .map_err(|e| e.to_string())?;

        Ok(())
    })
}

/// **软删除**会话：只打 `deleted_at` 标记。
///
/// 为什么软删：软删除的意义就是**能恢复**（入口见 [`restore_session`]）。所以连带地 ——
/// 消息、快照数据、回收站里的本体**全部保留**，否则恢复之后回滚会缺数据。
/// 真正清掉这些回滚侧产物的只有硬删除路径（见 [`hard_delete_session`] 与
/// `core::session::purge_rollback_artifacts`）。
///
/// 已删会话不再出现在任何列表/统计里（各读路径都带 `AND deleted_at IS NULL`），
/// 也**不会**因为再次被 upsert 而复活（见 [`upsert_session`] 的 ON CONFLICT）。
pub fn delete_session(id: &str) -> Result<(), String> {
    crate::infra::db::with_connection(|conn| {
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_millis() as i64;
        let changed = conn
            .execute(
                "UPDATE sessions SET deleted_at = ?2 WHERE id = ?1 AND deleted_at IS NULL",
                params![id, now],
            )
            .map_err(|e| e.to_string())?;
        if changed == 0 {
            return Err(format!("会话 {} 不存在或已在已删除列表里", id));
        }
        Ok(())
    })
}

/// **硬删除**会话（真删行）。
///
/// 只给"确定没有挽留价值"的场景用：目前是自动清理**空会话**
/// （`switch_away_and_delete_session` 里"message_count == 0"那一支 —— 它没有任何消息，
/// 恢复了也只是一张空会话）。
/// 用户从界面上删会话走的是软删除 [`delete_session`]。
pub fn hard_delete_session(id: &str) -> Result<(), String> {
    crate::infra::db::with_connection(|conn| {
        conn.execute("DELETE FROM sessions WHERE id = ?1", [id])
            .map_err(|e| e.to_string())?;
        Ok(())
    })
}

/// 恢复一个被软删除的会话（清掉 `deleted_at`）。
pub fn restore_session(id: &str) -> Result<(), String> {
    crate::infra::db::with_connection(|conn| {
        let changed = conn
            .execute(
                "UPDATE sessions SET deleted_at = NULL WHERE id = ?1 AND deleted_at IS NOT NULL",
                [id],
            )
            .map_err(|e| e.to_string())?;
        if changed == 0 {
            return Err(format!("会话 {} 不在已删除列表里", id));
        }
        Ok(())
    })
}

/// 已删除的会话列表（供界面的「最近删除 / 恢复」入口用）。
///
/// 与 `list_sessions` 的唯一区别是过滤条件取反；列清单复用同一份
/// [`SESSION_META_COLUMNS`]，所以不会出现"两条查询列不一致"的老问题。
pub fn list_deleted_sessions() -> Result<Vec<SessionMeta>, String> {
    crate::infra::db::with_connection(|conn| {
        let sql = format!(
            "SELECT {} FROM sessions s \
             LEFT JOIN projects p ON s.project_id = p.id \
             WHERE s.deleted_at IS NOT NULL \
             ORDER BY s.deleted_at DESC",
            SESSION_META_COLUMNS
        );
        let mut stmt = conn.prepare(&sql).map_err(|e| e.to_string())?;
        let rows = stmt
            .query_map([], session_meta_from_row)
            .map_err(|e| e.to_string())?;
        let mut sessions = Vec::new();
        for row in rows {
            sessions.push(row.map_err(|e| e.to_string())?);
        }
        Ok(sessions)
    })
}

pub fn rename_session(
    id: &str,
    title: &str,
    is_smart_named: bool,
    title_source: &str,
) -> Result<SessionMeta, String> {
    crate::infra::db::with_connection(|conn| {
        let changed = conn
            .execute(
                "UPDATE sessions SET title = ?2, is_smart_named = ?3, title_source = ?4 WHERE id = ?1 AND deleted_at IS NULL",
                params![id, title, if is_smart_named { 1 } else { 0 }, title_source],
            )
            .map_err(|e| e.to_string())?;
        if changed == 0 {
            return Err(format!("会话 {} 不存在", id));
        }
        let sql = format!(
            "SELECT {} FROM sessions s \
             LEFT JOIN projects p ON s.project_id = p.id \
             WHERE s.id = ?1 AND s.deleted_at IS NULL",
            SESSION_META_COLUMNS
        );
        conn.query_row(&sql, [id], session_meta_from_row)
            .map_err(|e| e.to_string())
    })
}

pub fn update_session_profile(id: &str, profile_id: &str) -> Result<(), String> {    crate::infra::db::with_connection(|conn| {
        let changed = conn
            .execute(
                "UPDATE sessions SET profile_id = ?2 WHERE id = ?1 AND deleted_at IS NULL",
                params![id, profile_id],
            )
            .map_err(|e| e.to_string())?;
        if changed == 0 {
            return Err(format!("会话 {} 不存在", id));
        }
        Ok(())
    })
}

/// 写入会话的深度思考档位（**布尔**）。
///
/// v14 起该列是 `INTEGER NOT NULL`，只写 `1` / `0`，**永不写 NULL**：
/// "跟随设置默认"这件事已在会话创建时被解析掉，库里只保留确定值——
/// 这正是"设置改动不倒灌已有会话"的实现基础（详见 `core::session::thinking`）。
pub fn update_session_thinking_mode(id: &str, enabled: bool) -> Result<(), String> {
    crate::infra::db::with_connection(|conn| {
        let changed = conn
            .execute(
                "UPDATE sessions SET thinking_mode = ?2 WHERE id = ?1 AND deleted_at IS NULL",
                params![id, if enabled { 1 } else { 0 }],
            )
            .map_err(|e| e.to_string())?;
        if changed == 0 {
            return Err(format!("会话 {} 不存在", id));
        }
        Ok(())
    })
}

/// 会话级运行偏好的恢复读：工作模式 / 权限档位 / 用户类型。
///
/// 与 thinking_mode 同语义：`None` = 用户在本会话从未表态，恢复时回落设置默认。
/// 刻意**不进** `SessionMeta` / `upsert_session` 全量链——这三列只由下面的
/// `update_session_*` 单列写入，避免 `save_session` 全量落库时被内存快照里的值覆盖。
#[derive(Debug, Clone, Default)]
pub struct SessionRuntimePrefs {
    pub work_mode: Option<String>,
    pub approval_mode: Option<String>,
    pub agent_audience: Option<String>,
}

pub fn get_session_runtime_prefs(id: &str) -> Result<SessionRuntimePrefs, String> {
    crate::infra::db::with_connection(|conn| {
        conn.query_row(
            "SELECT work_mode, approval_mode, agent_audience FROM sessions \
             WHERE id = ?1 AND deleted_at IS NULL",
            [id],
            |row| {
                Ok(SessionRuntimePrefs {
                    work_mode: row.get(0)?,
                    approval_mode: row.get(1)?,
                    agent_audience: row.get(2)?,
                })
            },
        )
        .map_err(|e| e.to_string())
    })
}

/// 三个运行偏好共用的单列写入：`column` 只接受本模块三个包装传入的字面量，
/// 不接外部输入，无注入面。会话不存在或已软删时报错（与 thinking_mode 同口径）。
fn update_session_text_column(id: &str, column: &str, value: &str) -> Result<(), String> {
    crate::infra::db::with_connection(|conn| {
        let changed = conn
            .execute(
                &format!(
                    "UPDATE sessions SET {} = ?2 WHERE id = ?1 AND deleted_at IS NULL",
                    column
                ),
                params![id, value],
            )
            .map_err(|e| e.to_string())?;
        if changed == 0 {
            return Err(format!("会话 {} 不存在", id));
        }
        Ok(())
    })
}

pub fn update_session_work_mode(id: &str, mode: &str) -> Result<(), String> {
    update_session_text_column(id, "work_mode", mode)
}

pub fn update_session_approval_mode(id: &str, mode: &str) -> Result<(), String> {
    update_session_text_column(id, "approval_mode", mode)
}

pub fn update_session_agent_audience(id: &str, audience: &str) -> Result<(), String> {
    update_session_text_column(id, "agent_audience", audience)
}

pub fn get_last_active_session_id() -> Option<String> {
    crate::infra::db::with_connection(|conn| {
        conn.query_row(
            "SELECT value FROM app_state WHERE key = 'last_active_session_id'",
            [],
            |row| row.get::<_, String>(0),
        )
        .optional()
        .map_err(|e| e.to_string())
    })
    .ok()
    .flatten()
}

pub fn set_last_active_session_id(id: &str) -> Result<(), String> {
    crate::infra::db::with_connection(|conn| {
        conn.execute(
            "INSERT INTO app_state(key, value) VALUES('last_active_session_id', ?1)
             ON CONFLICT(key) DO UPDATE SET value = excluded.value",
            [id],
        )
        .map_err(|e| e.to_string())?;
        Ok(())
    })
}

pub fn clear_last_active_session_id() -> Result<(), String> {
    crate::infra::db::with_connection(|conn| {
        conn.execute("DELETE FROM app_state WHERE key = 'last_active_session_id'", [])
            .map_err(|e| e.to_string())?;
        Ok(())
    })
}

/// 从 `sessions`（LEFT JOIN `projects`）的一行还原会话元数据。
///
/// **按列名取值**（与 `stored_session_message_from_row` 同一口径，理由见那里的说明）。
/// 注意 `working_directory` 取自 `p.path` —— 这是按列名之后才显式化的隐式约定，
/// 别"顺手"改成 `s` 的列。
fn session_meta_from_row(row: &Row<'_>) -> rusqlite::Result<SessionMeta> {
    Ok(SessionMeta {
        id: row.get("id")?,
        title: row.get("title")?,
        created_at: row.get::<_, i64>("created_at")? as u64,
        updated_at: row.get::<_, i64>("updated_at")? as u64,
        message_count: row.get::<_, i64>("message_count")? as usize,
        is_smart_named: row.get::<_, i64>("is_smart_named")? != 0,
        profile_id: row.get("profile_id")?,
        total_input_tokens: row.get::<_, i64>("total_input_tokens")? as u64,
        total_output_tokens: row.get::<_, i64>("total_output_tokens")? as u64,
        title_source: row.get("title_source")?,
        project_id: row.get("project_id")?,
        working_directory: row.get("path")?,
        thinking_mode: row.get::<_, i64>("thinking_mode")? != 0,
        total_cache_hit_tokens: row.get::<_, i64>("total_cache_hit_tokens")? as u64,
        total_cache_miss_tokens: row.get::<_, i64>("total_cache_miss_tokens")? as u64,
    })
}

// ── Project operations ──

/// `ProjectMeta` 的 SELECT 列清单（两处查询共用一份；与 `project_meta_from_row` 读的列名对应）。
///
/// 与 `SESSION_MESSAGE_COLUMNS` / `SESSION_META_COLUMNS` 同一套做法。
const PROJECT_META_COLUMNS: &str = "p.id, p.name, p.path, p.created_at, p.updated_at, \
     (SELECT COUNT(*) FROM sessions s WHERE s.project_id = p.id AND s.deleted_at IS NULL) as session_count";

pub fn get_project_path(project_id: &str) -> Result<Option<String>, String> {
    crate::infra::db::with_connection(|conn| {
        conn.query_row(
            "SELECT path FROM projects WHERE id = ?1",
            [project_id],
            |row| row.get::<_, String>(0),
        )
        .optional()
        .map_err(|e| e.to_string())
    })
}

pub fn get_project_by_path(path: &str) -> Result<Option<crate::core::session::ProjectMeta>, String> {
    crate::infra::db::with_connection(|conn| {
        conn.query_row(
            &format!("SELECT {} FROM projects p WHERE p.path = ?1", PROJECT_META_COLUMNS),
            [path],
            project_meta_from_row,
        )
        .optional()
        .map_err(|e| e.to_string())
    })
}

pub fn create_project(name: &str, path: &str) -> Result<crate::core::session::ProjectMeta, String> {
    let id = uuid::Uuid::new_v4().to_string()[..8].to_string();
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as i64;
    crate::infra::db::with_connection(|conn| {
        conn.execute(
            "INSERT INTO projects (id, name, path, created_at, updated_at) VALUES (?1, ?2, ?3, ?4, ?4)",
            rusqlite::params![id, name, path, now],
        )
        .map_err(|e| e.to_string())?;
        Ok(crate::core::session::ProjectMeta {
            id,
            name: name.to_string(),
            path: path.to_string(),
            created_at: now as u64,
            updated_at: now as u64,
            session_count: 0,
        })
    })
}

pub fn list_projects() -> Result<Vec<crate::core::session::ProjectMeta>, String> {
    crate::infra::db::with_connection(|conn| {
        let mut stmt = conn
            .prepare(&format!(
                "SELECT {} FROM projects p
                 ORDER BY p.updated_at DESC",
                PROJECT_META_COLUMNS
            ))
            .map_err(|e| e.to_string())?;
        let rows = stmt
            .query_map([], project_meta_from_row)
            .map_err(|e| e.to_string())?;
        let mut projects = Vec::new();
        for row in rows {
            projects.push(row.map_err(|e| e.to_string())?);
        }
        Ok(projects)
    })
}

/// 列某项目名下的会话 id。
///
/// 给"删项目"用：那条路径会连带删掉名下所有会话（`DELETE FROM sessions WHERE project_id`），
/// 而回滚侧产物（快照表 / 回收站目录）没有 FK 级联 —— 必须**删之前**把 id 抓出来，
/// 删完就再也查不到了。
pub fn list_session_ids_of_project(project_id: &str) -> Result<Vec<String>, String> {
    crate::infra::db::with_connection(|conn| {
        let mut stmt = conn
            .prepare("SELECT id FROM sessions WHERE project_id = ?1")
            .map_err(|e| e.to_string())?;
        let rows = stmt
            .query_map([project_id], |row| row.get::<_, String>(0))
            .map_err(|e| e.to_string())?;
        let mut ids = Vec::new();
        for row in rows {
            ids.push(row.map_err(|e| e.to_string())?);
        }
        Ok(ids)
    })
}

pub fn delete_project(id: &str) -> Result<(), String> {
    crate::infra::db::with_connection(|conn| {
        conn.execute("DELETE FROM sessions WHERE project_id = ?1", [id])
            .map_err(|e| e.to_string())?;
        conn.execute("DELETE FROM projects WHERE id = ?1", [id])
            .map_err(|e| e.to_string())?;
        Ok(())
    })
}

/// 从 `projects` 的一行还原项目元数据（**按列名取值**，口径同上）。
fn project_meta_from_row(row: &Row<'_>) -> rusqlite::Result<crate::core::session::ProjectMeta> {
    Ok(crate::core::session::ProjectMeta {
        id: row.get("id")?,
        name: row.get("name")?,
        path: row.get("path")?,
        created_at: row.get::<_, i64>("created_at")? as u64,
        updated_at: row.get::<_, i64>("updated_at")? as u64,
        session_count: row.get::<_, i64>("session_count")? as usize,
    })
}

fn matches_filter(
    conn: &rusqlite::Connection,
    meta: &SessionMeta,
    filter: Option<&SessionListFilter>,
) -> Result<bool, String> {
    let Some(filter) = filter else {
        return Ok(true);
    };

    if let Some(keyword) = filter
        .keyword
        .as_ref()
        .map(|value| value.trim())
        .filter(|value| !value.is_empty())
    {
        let keyword = keyword.to_lowercase();
        if !meta.title.to_lowercase().contains(&keyword)
            && !meta.id.to_lowercase().contains(&keyword)
        {
            return Ok(false);
        }
    }
    if let Some(from_ts) = filter.from_ts {
        if meta.updated_at < from_ts {
            return Ok(false);
        }
    }
    if let Some(to_ts) = filter.to_ts {
        if meta.updated_at > to_ts {
            return Ok(false);
        }
    }
    if let Some(profile_id) = filter.profile_id.as_ref().filter(|value| !value.is_empty()) {
        if meta.profile_id.as_deref() != Some(profile_id.as_str()) {
            return Ok(false);
        }
    }
    if let Some(tool) = filter.tool.as_ref().filter(|value| !value.is_empty()) {
        let count: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM agent_run_events WHERE session_id = ?1 AND tool = ?2",
                params![meta.id, tool],
                |row| row.get(0),
            )
            .map_err(|e| e.to_string())?;
        if count == 0 {
            return Ok(false);
        }
    }
    if let Some(has_tool_calls) = filter.has_tool_calls {
        let count: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM agent_run_events WHERE session_id = ?1 AND tool IS NOT NULL",
                [meta.id.as_str()],
                |row| row.get(0),
            )
            .map_err(|e| e.to_string())?;
        if has_tool_calls != (count > 0) {
            return Ok(false);
        }
    }
    if let Some(model) = filter.model.as_ref().filter(|value| !value.is_empty()) {
        let count: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM agent_run_events WHERE session_id = ?1 AND model = ?2",
                params![meta.id, model],
                |row| row.get(0),
            )
            .map_err(|e| e.to_string())?;
        if count == 0 {
            return Ok(false);
        }
    }

    Ok(true)
}

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
        ] {
            assert!(
                names.iter().any(|n| n == name),
                "SESSION_META_COLUMNS 的结果集里没有 {name}：{names:?}"
            );
        }
        assert_eq!(
            names.len(),
            15,
            "列数应为 15，实际 {}：{names:?}",
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
        assert_eq!(names.len(), 6, "列数应为 6，实际 {}：{names:?}", names.len());
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
            "turn_id",
            "interrupt_kind",
        ] {
            assert!(columns.contains(&name), "共享列清单缺少 {name}：{columns:?}");
        }
        assert_eq!(
            columns.len(),
            11,
            "列数应为 11，实际 {}：{columns:?}",
            columns.len()
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
                        4 AS hidden_at, 'chat' AS source, 't1' AS turn_id",
            )
            .unwrap();
        let stored = stmt
            .query_row(params!["ix", "m1", json], stored_session_message_from_row)
            .unwrap();
        assert_eq!(stored.message_id, "m1");
        assert_eq!(stored.seq, 7);
        assert_eq!(stored.role, "user");
        assert_eq!(stored.source, "chat");
        assert_eq!(stored.turn_id.as_deref(), Some("t1"));
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
                        'chat' AS source, 't1' AS turn_id",
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
                                          created_at, source, turn_id, interrupt_kind)
             VALUES('s-repo-test', 'm-repo-test', 0, 'user', ?1, 0, 'chat', 't1', 'stream_timeout')",
            params![sample_content_json()],
        )
        .unwrap();

        let found = find_session_message_by_id_in(&conn, "s-repo-test", "m-repo-test")
            .expect("查询不应报错")
            .expect("应能按 id 找到这条消息");
        assert_eq!(found.message_id, "m-repo-test");
        assert_eq!(found.role, "user");
        assert_eq!(found.turn_id.as_deref(), Some("t1"));
        assert_eq!(found.interrupt_kind.as_deref(), Some("stream_timeout"));
    }
}
