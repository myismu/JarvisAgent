//! # repository.rs — 会话 SQLite 仓储
//!
//! 封装会话元数据、完整记忆与消息展开索引的 SQLite 读写。
//!
//! ## Key Exports
//! - `upsert_session()`: 保存会话元数据和记忆
//! - `load_session()`: 读取完整会话记忆
//! - `list_sessions()`: 查询会话列表
//! - `upsert_context_snapshot()`: 保存最近一次上下文 token 快照
//! - `set_last_active_session_id()`: 持久化最后活跃会话
//!
//! ## Dependencies
//! - Internal: `crate::infra::db`, `crate::infra::types::models`
//! - External: `rusqlite`, `serde`

use rusqlite::{params, OptionalExtension, Row};
use std::collections::HashMap;

use crate::core::session::{
    BatchOpFailure, BatchOpResult, RequestUsageRecord, SessionMeta, SessionTokenTotals,
    UsageBreakdown, UsageSource, UsageSourceBreakdown,
};
// 父模块（session）的私有时间戳函数：子模块可以直接用，但需要显式导入。
// `insert_request_usage_in` 在“调用方没指定 created_at”时用它取当前毫秒。
use super::now_ts;
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
/// 落点：`core::session::usage::record_request_usage`（明细 + 累计同事务）。
/// 主循环与子 Agent 的每一次请求都走那条路径，回合收尾不再补交任何增量。
///
/// 返回 `None` 表示会话不存在或已归档（`deleted_at` 非空），调用方据此跳过事件推送。
#[deprecated(
    note = "生产路径请用 usage::record_request_usage（本函数只累加、不写明细）"
)]
pub fn accumulate_session_token_usage(
    session_id: &str,
    delta: SessionTokenTotals,
) -> Result<Option<SessionTokenTotals>, String> {
    crate::infra::db::with_connection(|conn| add_session_token_totals_in(conn, session_id, delta))
}

/// [`accumulate_session_token_usage`] 的 connection 版（可在测试里跑真实 SQL）。
///
/// 参数是 `&Connection` 而不是 `&Transaction`：`Transaction` 会 deref 到 `Connection`，
/// 因此也能在事务上下文里用（`record_request_usage` 就是这么复用它以保证
/// “明细与累计在同一个事务里”）。
///
/// 返回 `None` = 会话不存在或已归档（`deleted_at` 非空），调用方据此跳过事件推送。
pub(super) fn add_session_token_totals_in(
    conn: &rusqlite::Connection,
    session_id: &str,
    delta: SessionTokenTotals,
) -> Result<Option<SessionTokenTotals>, String> {
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
}

// ── 请求级用量明细（v28）──
//
// 本段的职责只是“把一行明细写进去 / 把明细汇总读出来”。
// ⚠️ **生产路径的唯一写入口是 `core::session::usage::record_request_usage`**：
// 它会把“插入明细”与“累加 sessions.total_*”放进**同一个事务**。
// 直接调 [insert_request_usage] 会造成“明细与累计不一致”的账（账对不平且不报错），
// 仅供应急/测试使用。

/// 向 `request_usage` 插入一行明细（**不要**在业务里直接调，用 `record_request_usage`）。
pub fn insert_request_usage(record: &RequestUsageRecord) -> Result<(), String> {
    crate::infra::db::with_connection(|conn| insert_request_usage_in(conn, record))
}

/// [`insert_request_usage`] 的 connection 版（事务与测试共用）。
///
/// 缓存两列**原样绑定 `Option`**：`None` 落 NULL（厂商未上报），`Some(0)` 落 0。
/// 这里刻意不写 `unwrap_or(0)` —— 那一行代码就会把“不知道”变成“确定为零”。
pub(super) fn insert_request_usage_in(
    conn: &rusqlite::Connection,
    record: &RequestUsageRecord,
) -> Result<(), String> {
    // 时间戳统一毫秒口径（v16 起全库一致）；不传则取写入时刻。
    let created_at = record.created_at.unwrap_or_else(now_ts) as i64;

    conn.execute(
        "INSERT INTO request_usage(
            session_id, run_id, loop_index, source, parent_tool_use_id,
            agent_role, task_id, model_id, input_tokens, output_tokens,
            cache_hit_tokens, cache_miss_tokens, cache_source, created_at
         ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14)",
        params![
            record.session_id,
            record.run_id,
            record.loop_index.map(|value| value as i64),
            record.source.as_str(),
            record.parent_tool_use_id,
            record.agent_role,
            record.task_id,
            record.model_id,
            record.input_tokens as i64,
            record.output_tokens as i64,
            record.cache_hit_tokens.map(|value| value as i64),
            record.cache_miss_tokens.map(|value| value as i64),
            record.cache_source,
            created_at,
        ],
    )
    .map_err(|e| e.to_string())?;
    Ok(())
}

/// 读取会话的用量账本（按来源分组 + 总计）。
pub fn get_session_usage_breakdown(session_id: &str) -> Result<UsageBreakdown, String> {
    crate::infra::db::with_connection(|conn| session_usage_breakdown_in(conn, session_id))
}

/// 汇总查询的一行（`GROUP BY source` 的结果）。
///
/// 用带字段名的结构体而不是八元组：全是 `i64`，元组传错顺序编译器不拦。
struct UsageAggregateRow {
    source: String,
    request_count: i64,
    input_tokens: i64,
    output_tokens: i64,
    cache_hit_tokens: i64,
    cache_miss_tokens: i64,
    unreported_requests: i64,
    unreported_input_tokens: i64,
}

/// [`get_session_usage_breakdown`] 的 connection 版。
///
/// ## 为什么在 SQL 里汇总
/// 一个长会话的明细行可以上万；拉到内存再 `fold` 等于把整张明细表搬一遍，
/// 而这里只需要十几个数字。`GROUP BY source` + `SUM/COUNT` 一次拿完。
///
/// ## “未上报”的判定口径
/// 任一个缓存列为 `NULL` 即算“本次未上报”：两个值本来就应同真同假，
/// 出现半截数据（只有命中、没有未命中）时宁可归为未上报，也不拿半截数去算命中率。
/// 未上报的请求**不参与** `cache_*_tokens` 求和（`SUM` 自带忽略 NULL），
/// 也**不**按 0 计入——它们的输入量单独记在 `unreported_input_tokens` 里。
///
/// 分组顺序固定为 `source` 字母序（结果确定、可测）；展示顺序由前端决定。
pub(super) fn session_usage_breakdown_in(
    conn: &rusqlite::Connection,
    session_id: &str,
) -> Result<UsageBreakdown, String> {
    let mut stmt = conn
        .prepare(
            "SELECT source,
                    COUNT(*) AS request_count,
                    COALESCE(SUM(input_tokens), 0) AS input_tokens,
                    COALESCE(SUM(output_tokens), 0) AS output_tokens,
                    COALESCE(SUM(cache_hit_tokens), 0) AS cache_hit_tokens,
                    COALESCE(SUM(cache_miss_tokens), 0) AS cache_miss_tokens,
                    SUM(CASE WHEN cache_hit_tokens IS NULL OR cache_miss_tokens IS NULL
                             THEN 1 ELSE 0 END) AS unreported_requests,
                    COALESCE(SUM(CASE WHEN cache_hit_tokens IS NULL OR cache_miss_tokens IS NULL
                                      THEN input_tokens ELSE 0 END), 0) AS unreported_input_tokens
             FROM request_usage
             WHERE session_id = ?1
             GROUP BY source
             ORDER BY source",
        )
        .map_err(|e| e.to_string())?;

    let rows = stmt
        .query_map([session_id], |row| {
            Ok(UsageAggregateRow {
                source: row.get("source")?,
                request_count: row.get("request_count")?,
                input_tokens: row.get("input_tokens")?,
                output_tokens: row.get("output_tokens")?,
                cache_hit_tokens: row.get("cache_hit_tokens")?,
                cache_miss_tokens: row.get("cache_miss_tokens")?,
                unreported_requests: row.get("unreported_requests")?,
                unreported_input_tokens: row.get("unreported_input_tokens")?,
            })
        })
        .map_err(|e| e.to_string())?;

    let mut breakdown = UsageBreakdown::empty(session_id);
    for row in rows {
        let row = row.map_err(|e| e.to_string())?;
        // 未知取值直接报错（理由见 `UsageSource::from_str` 的注释）
        let source = UsageSource::from_str(&row.source)?;

        let item = UsageSourceBreakdown {
            source,
            request_count: row.request_count.max(0) as u64,
            input_tokens: row.input_tokens.max(0) as u64,
            output_tokens: row.output_tokens.max(0) as u64,
            cache_hit_tokens: row.cache_hit_tokens.max(0) as u64,
            cache_miss_tokens: row.cache_miss_tokens.max(0) as u64,
            unreported_requests: row.unreported_requests.max(0) as u64,
            unreported_input_tokens: row.unreported_input_tokens.max(0) as u64,
        };

        // 总计由分组结果累加得出，不再发第二条 `SUM` 查询：
        // 两处口径一旦分叉（比如过滤条件改了一处漏了一处）就会对不平。
        breakdown.request_count += item.request_count;
        breakdown.input_tokens += item.input_tokens;
        breakdown.output_tokens += item.output_tokens;
        breakdown.cache_hit_tokens += item.cache_hit_tokens;
        breakdown.cache_miss_tokens += item.cache_miss_tokens;
        breakdown.unreported_requests += item.unreported_requests;
        breakdown.unreported_input_tokens += item.unreported_input_tokens;
        breakdown.sources.push(item);
    }

    // 没有任何分组行 = 没有任何明细 = 历史会话（早于 v28）。
    // 调用方必须靠这个标志区分“确实没用过 token”与“没明细可拆”。
    breakdown.has_detail = !breakdown.sources.is_empty();
    Ok(breakdown)
}

/// 会话当前是否可写用量（存在且未归档）。
///
/// 与累加 SQL 的 `WHERE ... AND deleted_at IS NULL` 同口径：
/// 归档会话不再接受新账，否则归档区的旧账会被归档后的请求默默改掉。
pub(super) fn session_usage_writable_in(
    conn: &rusqlite::Connection,
    session_id: &str,
) -> Result<bool, String> {
    let count: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM sessions WHERE id = ?1 AND deleted_at IS NULL",
            [session_id],
            |row| row.get(0),
        )
        .map_err(|e| e.to_string())?;
    Ok(count > 0)
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
const SESSION_MESSAGE_COLUMNS: &str = "message_id, seq, role, content_json, created_at, \
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
fn is_turn_start_message(stored: &StoredSessionMessage) -> bool {
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
fn session_message_stats_in(
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
     s.project_id, p.path, s.thinking_mode, s.total_cache_hit_tokens, s.total_cache_miss_tokens, \
     s.deleted_at, p.name";

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

/// 列出未归档的会话（按最近更新倒序）。
///
/// 曾有一个 `Option<&SessionListFilter>` 参数与配套的 `matches_filter`，
/// 于 2026-10-07 整体删除 —— 那条筛选链路**三段全断**：前端面板被一个写死的
/// `false` 关着（连关键字搜索框也在里面）、后端命令 `list_sessions()` 不收参数
/// （条件被静默丢弃）、且其中两个查询查的 `agent_run_events.tool` 列在 v15
/// 换表时已不存在。功能建好第二天就被关掉且未留理由，半年多无人使用。
/// 需要"按工具/模型/E 筛选会话"时，工具名现在在 `resp_blocks` 的
/// `ContentBlock::ToolUse` 里（`tool_results` 只有 id 与内容，没有名字）。
pub fn list_sessions() -> Result<Vec<SessionMeta>, String> {
    crate::infra::db::with_connection(|conn| {
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
        let mut sessions = Vec::new();
        for row in rows {
            sessions.push(row.map_err(|e| e.to_string())?);
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

/// **归档**会话：只打 `deleted_at` 标记（列名是"删除"时代的遗留，语义已是归档）。
///
/// 为什么归档必须能撤销：撤销入口见 [`restore_session`]。所以连带地 ——
/// 消息、快照数据、回收站里的本体**全部保留**，否则取消归档之后回滚会缺数据。
/// 真正清掉这些回滚侧产物的只有硬删除路径（见 [`hard_delete_session`] 与
/// `core::session::purge_rollback_artifacts`）。
///
/// 已归档会话不再出现在任何列表/统计里（各读路径都带 `AND deleted_at IS NULL`），
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
            return Err(format!("会话 {} 不存在或已在归档列表里", id));
        }
        Ok(())
    })
}

/// **硬删除**会话（真删行）。
///
/// 只给"用户明确要求彻底删除"的场景用（`purge_session`，入口在「归档」区）。
/// 界面上的归档走的是 [`delete_session`] —— 只打标记，随时可撤销。
pub fn hard_delete_session(id: &str) -> Result<(), String> {
    crate::infra::db::with_connection(|conn| {
        conn.execute("DELETE FROM sessions WHERE id = ?1", [id])
            .map_err(|e| e.to_string())?;
        Ok(())
    })
}

/// 取消归档（清掉 `deleted_at`），把会话放回活跃列表。
pub fn restore_session(id: &str) -> Result<(), String> {
    crate::infra::db::with_connection(|conn| {
        let changed = conn
            .execute(
                "UPDATE sessions SET deleted_at = NULL WHERE id = ?1 AND deleted_at IS NOT NULL",
                [id],
            )
            .map_err(|e| e.to_string())?;
        if changed == 0 {
            return Err(format!("会话 {} 不在归档列表里", id));
        }
        Ok(())
    })
}

/// 已归档的会话列表（供界面的「归档」区入口用）。
///
/// 与 `list_sessions` 的唯一区别是过滤条件取反；列清单复用同一份
/// [`SESSION_META_COLUMNS`]，所以不会出现"两条查询列不一致"的老问题。
pub fn list_deleted_sessions() -> Result<Vec<SessionMeta>, String> {
    crate::infra::db::with_connection(list_deleted_sessions_in)
}

/// [`list_deleted_sessions`] 的 connection 版（测试直接跑真实 SQL）。
///
/// LEFT JOIN `projects` **不过滤 `p.deleted_at`**：归档页要同时展示会话与它的项目名；
/// 项目归档时两者是一起归档的，过滤掉项目就只能看到一条没头没尾的会话。
fn list_deleted_sessions_in(conn: &rusqlite::Connection) -> Result<Vec<SessionMeta>, String> {
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
}

// ── 项目 / 会话的归档与批量原语（v27）──
//
// 本段函数的共同约束：**数据库改动一律在同一次事务里完成**，
// 文件系统（快照表以外的磁盘目录）清理一律在事务**外**做。
// 原因：事务只能回滚数据库；目录已删、事务再回滚，就变成“行还在、数据没了”的不可恢复态。

/// 归档项目：项目本身 + 名下所有**未归档**会话，在同一个事务里打上同一时间戳。
///
/// 返回被归档的会话数。
///
/// - 已自行归档的会话不改 `deleted_at`：归档时间要保留「用户当时手动归档那一刻」，
///   不能被后来的项目归档覆盖（它已经是用户见过的时间）；
/// - 项目不存在或已归档 → 返回错误。此时事务整体回滚，会话的归档也一并撤销，
///   不会留下「会话已归档、项目还在」的半成品状态（这正是必须单事务的原因）。
///
/// 不清快照与回收站：归档可撤销（见 [`restore_project`]），回滚还要用它们。
pub fn archive_project(project_id: &str, now: i64) -> Result<u64, String> {
    crate::infra::db::with_transaction(|tx| archive_project_in(tx, project_id, now))
}

/// [`archive_project`] 的 connection 版（可在测试里跑真实 SQL，不必依赖全局 DB 单例）。
///
/// 参数是 `&Connection` 而不是 `&Transaction`：`Transaction` 会 deref 到 `Connection`，
/// 同样能在事务上下文里执行；反过来（拿 `&Transaction` 做参数）则无法在测试里不建事务直接用。
fn archive_project_in(
    conn: &rusqlite::Connection,
    project_id: &str,
    now: i64,
) -> Result<u64, String> {
    let archived_sessions = conn
        .execute(
            "UPDATE sessions SET deleted_at = ?2 WHERE project_id = ?1 AND deleted_at IS NULL",
            params![project_id, now],
        )
        .map_err(|e| e.to_string())?;

    let changed_project = conn
        .execute(
            "UPDATE projects SET deleted_at = ?2 WHERE id = ?1 AND deleted_at IS NULL",
            params![project_id, now],
        )
        .map_err(|e| e.to_string())?;
    if changed_project == 0 {
        // 返回 Err 会让 `with_transaction` 丢掉 tx、不 commit，上面的会话 UPDATE 随之回滚
        return Err(format!("项目 {} 不存在或已在归档列表里", project_id));
    }

    Ok(archived_sessions as u64)
}

/// 取消归档项目：只清 `projects.deleted_at`。
///
/// 名下会话**不跟着复活**：它们是各自被归档的，应该逐条恢复（见 [`restore_sessions`]）。
/// 这里不做“级联复活”是因为项目可能归档了半年、用户只想找回其中一条会话。
pub fn restore_project(project_id: &str) -> Result<(), String> {
    crate::infra::db::with_connection(|conn| restore_project_in(conn, project_id))
}

/// [`restore_project`] 的 connection 版。不在归档列表时报错（与 `restore_session` 同口径）。
fn restore_project_in(conn: &rusqlite::Connection, project_id: &str) -> Result<(), String> {
    let changed = conn
        .execute(
            "UPDATE projects SET deleted_at = NULL WHERE id = ?1 AND deleted_at IS NOT NULL",
            [project_id],
        )
        .map_err(|e| e.to_string())?;
    if changed == 0 {
        return Err(format!("项目 {} 不在归档列表里", project_id));
    }
    Ok(())
}

/// 批量取消归档会话，并把**连带复活**的归档项目回报在 `restored_project_ids` 里。
///
/// 为什么要连带复活项目：归档项目时是项目+会话一起打标的。若只复活会话，它会挂在一个
/// 归档项目下——项目列表里看不到它、`get_project_path` 仍能取值但界面无处可点，
/// 用户看到的是「会话回来了但没在任何项目里」，与他的操作预期不符。
///
/// 失败项（不存在 / 不在归档列表）进 `failed` 而不中断整批：批量操作里单条失效
/// （另一个窗口已恢复、会话已被彻底删除）不应该让其余勾选项全部白做。
///
/// ## 事务边界
/// 整批共用一个事务：会话与项目的复活要么一起生效，要么一起不生效。
/// ⚠️ 事务闭包里**只能**用手上的这把连接，不得再调 `with_connection` / `with_transaction`
/// —— 全局连接是 Mutex 串行化的，重入会直接死锁。
pub fn restore_sessions(ids: &[String]) -> Result<BatchOpResult, String> {
    crate::infra::db::with_transaction(|tx| restore_sessions_in(tx, ids))
}

/// [`restore_sessions`] 的 connection 版。
fn restore_sessions_in(
    conn: &rusqlite::Connection,
    ids: &[String],
) -> Result<BatchOpResult, String> {
    let mut succeeded_ids: Vec<String> = Vec::new();
    let mut failed: Vec<BatchOpFailure> = Vec::new();
    let mut restored_project_ids: Vec<String> = Vec::new();

    for id in ids {
        // 去重：同一个 id 在入参里出现两次时，第二次必然影响 0 行，
        // 若照常计入 failed 会误报「恢复失败」。
        if succeeded_ids.contains(id) {
            continue;
        }
        let changed = conn
            .execute(
                "UPDATE sessions SET deleted_at = NULL WHERE id = ?1 AND deleted_at IS NOT NULL",
                [id],
            )
            .map_err(|e| e.to_string())?;
        if changed == 0 {
            failed.push(BatchOpFailure {
                id: id.clone(),
                reason: "会话不存在或不在归档列表".to_string(),
            });
            continue;
        }

        // 会话已复活，再看它的项目是不是也还处于归档态。
        // 刚更新过 deleted_at，所以这里读到的 project_id 仍是归档时的绑定关系。
        let project_id: Option<String> = conn
            .query_row(
                "SELECT project_id FROM sessions WHERE id = ?1",
                [id],
                |row| row.get::<_, Option<String>>(0),
            )
            .optional()
            .map_err(|e| e.to_string())?
            .flatten();
        if let Some(project_id) = project_id {
            let project_changed = conn
                .execute(
                    "UPDATE projects SET deleted_at = NULL \
                     WHERE id = ?1 AND deleted_at IS NOT NULL",
                    [&project_id],
                )
                .map_err(|e| e.to_string())?;
            if project_changed > 0 && !restored_project_ids.contains(&project_id) {
                restored_project_ids.push(project_id);
            }
        }

        succeeded_ids.push(id.clone());
    }

    Ok(BatchOpResult {
        succeeded_ids,
        failed,
        restored_project_ids,
    })
}

/// 批量**彻底删除**归档会话（行 + 回滚侧产物）。
///
/// ## 两步式设计（为什么不能一步到底）
/// 第一步（事务内）：逐条校验「确实在归档列表里」并删行。`session_messages` 有
/// `ON DELETE CASCADE`，会随之清掉；但 `snapshot_trees` / `snapshot_content` 等快照表
/// 和回收站目录**没有 FK 级联**，必须显式清理。
/// 第二步（事务提交后）：逐条调 [`crate::core::session::purge_rollback_artifacts`] 清快照表与回收站目录。
///
/// ## 取舍：文件系统清理失败不算删除失败
/// 会话行已删是不可逆的，此时把该 id 报成 failed 是“误导”：用户再看列表它已经不在了，
/// 重试也只会得到「不在归档列表」。所以清理失败仅 `eprintln` 记录（留给运维排查残留），
/// 该 id 仍计入 `succeeded_ids`。若将来要做“残留产物自动清理”，应该另起扫描任务，
/// 而不是把这个函数变成半成功态。
///
/// 拒绝删除活跃（未归档）会话：那些 id 进 `failed`，原因「不在归档列表」。
/// 这是本函数唯一的防误删门——彻底删除不可撤销，不能因为前端传错 id 就清掉在用会话。
pub fn purge_sessions(ids: &[String]) -> Result<BatchOpResult, String> {
    // 事务内只做数据库部分，返回值里的 pending 是「待清产物的会话 id」（已删行）
    let (result, purged_ids) = crate::infra::db::with_transaction(|tx| purge_sessions_in(tx, ids))?;

    // 事务已提交，此处才碰文件系统 / 快照表：清理不可回滚，必须排在提交之后
    for session_id in &purged_ids {
        crate::core::session::purge_rollback_artifacts(session_id);
    }
    Ok(result)
}

/// [`purge_sessions`] 的事务内部分：校验 + 删行。
///
/// 第二个返回值是**待清理回滚产物的 id 列表**（与 `result.succeeded_ids` 内容相同，
/// 单列出来是为了让调用方在事务外循环时不必从结果结构里绕）。
fn purge_sessions_in(
    conn: &rusqlite::Connection,
    ids: &[String],
) -> Result<(BatchOpResult, Vec<String>), String> {
    let mut succeeded_ids: Vec<String> = Vec::new();
    let mut failed: Vec<BatchOpFailure> = Vec::new();
    let mut pending: Vec<String> = Vec::new();

    // 第一遍：只校验不删。先校验完再删，好处是入参里重复的 id 不会把第二条误判成失败。
    for id in ids {
        if pending.contains(id) {
            continue;
        }
        let archived: Option<String> = conn
            .query_row(
                "SELECT id FROM sessions WHERE id = ?1 AND deleted_at IS NOT NULL",
                [id],
                |row| row.get(0),
            )
            .optional()
            .map_err(|e| e.to_string())?;
        match archived {
            Some(_) => pending.push(id.clone()),
            None => failed.push(BatchOpFailure {
                id: id.clone(),
                reason: "不在归档列表".to_string(),
            }),
        }
    }

    // 第二遍：删行。`session_messages` / `session_memory` 有 ON DELETE CASCADE，会自动跟随删除。
    for id in pending {
        conn.execute("DELETE FROM sessions WHERE id = ?1", [&id])
            .map_err(|e| e.to_string())?;
        succeeded_ids.push(id);
    }

    Ok((
        BatchOpResult {
            succeeded_ids: succeeded_ids.clone(),
            failed,
            // 删除不会复活任何项目，字段保持空（结构体统一，调用方不必分分支处理）
            restored_project_ids: Vec::new(),
        },
        // 待清理回滚产物的 id（事务外循环用），与 result.succeeded_ids 内容一致
        succeeded_ids,
    ))
}

/// 归档会话总数（归档页角标，也是「清空归档」的前置读数）。
pub fn count_deleted_sessions() -> Result<usize, String> {
    crate::infra::db::with_connection(count_deleted_sessions_in)
}

/// [`count_deleted_sessions`] 的 connection 版。
fn count_deleted_sessions_in(conn: &rusqlite::Connection) -> Result<usize, String> {
    let count: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM sessions WHERE deleted_at IS NOT NULL",
            [],
            |row| row.get(0),
        )
        .map_err(|e| e.to_string())?;
    Ok(count.max(0) as usize)
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

pub fn update_session_profile(id: &str, profile_id: &str) -> Result<(), String> {
    crate::infra::db::with_connection(|conn| {
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
/// 不接外部输入，无注入面。会话不存在或已归档时报错（与 thinking_mode 同口径）。
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
        conn.execute(
            "DELETE FROM app_state WHERE key = 'last_active_session_id'",
            [],
        )
        .map_err(|e| e.to_string())?;
        Ok(())
    })
}

/// 从 `sessions`（LEFT JOIN `projects`）的一行还原会话元数据。
///
/// **按列名取值**（与 `stored_session_message_from_row` 同一口径，理由见那里的说明）。
/// 注意 `working_directory` 取自 `p.path`、`project_name` 取自 `p.name` —— 这是按列名之后
/// 才显式化的隐式约定，别"顺手"改成 `s` 的列（sessions 表根本没有这两列）。
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
        // NULL = 未归档；负值（脏数据）归一到 0，不把负数带进 u64
        deleted_at: row
            .get::<_, Option<i64>>("deleted_at")?
            .map(|value| value.max(0) as u64),
        // 无项目（或项目行缺失）时 LEFT JOIN 给 NULL → None
        project_name: row.get("name")?,
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

/// 按 id 读单个项目元数据。
///
/// 与 [`get_project_by_path`] 共用同一份列清单与行映射，保证「重命名后返回的对象」
/// 与列表页拿到的字段口径完全一致（含 `session_count`）。
pub fn get_project(id: &str) -> Result<Option<crate::core::session::ProjectMeta>, String> {
    crate::infra::db::with_connection(|conn| {
        conn.query_row(
            &format!(
                "SELECT {} FROM projects p WHERE p.id = ?1",
                PROJECT_META_COLUMNS
            ),
            [id],
            project_meta_from_row,
        )
        .optional()
        .map_err(|e| e.to_string())
    })
}

pub fn get_project_by_path(
    path: &str,
) -> Result<Option<crate::core::session::ProjectMeta>, String> {
    crate::infra::db::with_connection(|conn| {
        conn.query_row(
            &format!(
                "SELECT {} FROM projects p WHERE p.path = ?1",
                PROJECT_META_COLUMNS
            ),
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
    crate::infra::db::with_connection(list_projects_in)
}

/// [`list_projects`] 的 connection 版（测试直接跑真实 SQL）。
///
/// 只列**未归档**项目：归档是“从界面移除但可撤销”，归档项统一在「归档」区展示。
/// 注意 `PROJECT_META_COLUMNS` 里的 `session_count` 子查询自己带了会话级过滤
/// （`s.deleted_at IS NULL`），不要把它改成跟着项目过滤——两者口径不同：
/// 一个未归档项目里可能全是已归档会话，此时 session_count 就是 0。
fn list_projects_in(
    conn: &rusqlite::Connection,
) -> Result<Vec<crate::core::session::ProjectMeta>, String> {
    let mut stmt = conn
        .prepare(&format!(
            "SELECT {} FROM projects p
             WHERE p.deleted_at IS NULL
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
}

/// 项目是否处于**归档**状态（`projects.deleted_at IS NOT NULL`）。
///
/// 为什么单开一个查询，而不是把 `deleted_at` 加进 [`crate::core::session::ProjectMeta`]：
/// `PROJECT_META_COLUMNS` / `project_meta_from_row` 是列表页、单条查询与
/// `meta_columns_tests` 三方共用的一份列清单，而「打开项目时要不要复活」只有
/// `command::session::open_project` 这一条路径需要判断。为一条路径扩大公共契约，
/// 会把归档细节漏进所有读取项目的地方，收益不抵风险。
///
/// 项目不存在时返回 `false`（不是错误）：调用方已通过 `get_project_by_path` 拿到记录，
/// 这里只回答「要不要复活」，查不到就按未归档处理即可。
pub fn is_project_archived(project_id: &str) -> Result<bool, String> {
    crate::infra::db::with_connection(|conn| {
        let archived: Option<i64> = conn
            .query_row(
                "SELECT deleted_at FROM projects WHERE id = ?1",
                [project_id],
                |row| row.get::<_, Option<i64>>(0),
            )
            .optional()
            .map_err(|e| e.to_string())?
            .flatten();
        Ok(archived.is_some())
    })
}

/// 重命名项目：**只改展示名，不移动磁盘目录**。
///
/// 为什么只改 `name` 而不碰 `path`：
/// - 会话沙箱的工作目录是由 `projects.path` 派生出来的（见 `get_project_path`），
///   路径一改就等于把已有会话的工作根换掉，历史消息里的相对路径、工具调用记录
///   全都会指向旧位置，语义被悄悄改写；
/// - 快照 / 回收站里存的是按会话与文件路径拼出来的引用，移动目录会让这些引用
///   变成悬空指针，且没有迁移机制能把它们重新对上；
/// - 用户改名的真实诉求只是「列表里看着顺眼」，与磁盘上的目录名无关。
///
/// 同理这里**不更新 `updated_at`**：重命名不是实际工作进展，不该让项目在
/// 「按最近活跃排序」的列表里跳到最前面。只动 `name` 一列。
pub fn rename_project(id: &str, name: &str) -> Result<crate::core::session::ProjectMeta, String> {
    let trimmed = name.trim();
    if trimmed.is_empty() {
        return Err("项目名不能为空".into());
    }
    if trimmed.chars().count() > 40 {
        return Err("项目名不能超过 40 个字符".into());
    }
    crate::infra::db::with_connection(|conn| {
        let changed = conn
            .execute(
                "UPDATE projects SET name = ?2 WHERE id = ?1",
                rusqlite::params![id, trimmed],
            )
            .map_err(|e| e.to_string())?;
        if changed == 0 {
            return Err("项目不存在".into());
        }
        Ok(())
    })?;
    // 读回首元数据（含 session_count），口径与列表页一致；get_project 自己再拿一次连接，
    // 所以必须等上面的 with_connection 闭包结束之后再调，避免重入。
    get_project(id)?.ok_or_else(|| "项目不存在".into())
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
        assert_eq!(session_message_stats_in(&conn, "s-stats").unwrap(), (0, None));
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

    fn insert_session(conn: &Connection, id: &str, project_id: Option<&str>, deleted_at: Option<i64>) {
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
        assert_eq!(result.failed.len(), 2, "活跃会话与不存在的会话都应进 failed");
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
        assert_eq!(session_deleted_at(&conn, "s-archived"), None, "成功项应已清掉归档标记");
    }

    /// 恢复归档会话时，其归档项目在同事务内一并复活；同一项目只回报一次。
    #[test]
    fn restoring_sessions_revives_the_archived_project_once() {
        let conn = init_conn();
        insert_project(&conn, "p1", "项目甲", Some(500));
        insert_session(&conn, "s1", Some("p1"), Some(500));
        insert_session(&conn, "s2", Some("p1"), Some(500));

        let result =
            restore_sessions_in(&conn, &["s1".to_string(), "s2".to_string()]).unwrap();

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

        let (result, pending) = purge_sessions_in(
            &conn,
            &["s-archived".to_string(), "s-active".to_string()],
        )
        .unwrap();

        assert_eq!(result.succeeded_ids, vec!["s-archived".to_string()]);
        assert_eq!(pending, vec!["s-archived".to_string()], "只有删成功行的 id 才需要清产物");
        assert_eq!(result.failed.len(), 1);
        assert_eq!(result.failed[0].id, "s-active");
        assert_eq!(result.failed[0].reason, "不在归档列表");

        let sessions_left: i64 = conn
            .query_row("SELECT COUNT(*) FROM sessions WHERE id = 's-active'", [], |row| {
                row.get(0)
            })
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
        assert_eq!(breakdown.unreported_requests, 1, "只有子 Agent 那行没上报缓存");
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
            breakdown.sources.iter().map(|s| s.input_tokens).sum::<u64>()
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
            (totals.input, totals.output, totals.cache_hit, totals.cache_miss),
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
