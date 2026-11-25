//! # repository/usage.rs — 会话 token 用量与请求明细仓储
//!
//! 封装 `request_usage` 逐请求明细写入、按来源聚合查询及 `sessions.total_*` 原子累加逻辑。
//!
//! ## Key Exports
//! - `insert_request_usage()`: 写入单条请求 token 明细
//! - `get_session_usage_breakdown()`: 按主循环与子 Agent 来源汇总会话用量
//! - `accumulate_session_token_usage()`: 原子累加会话级 token 累计值
//!
//! ## Dependencies
//! - Internal: `crate::core::session`, `crate::infra::db`
//! - External: `rusqlite`

use rusqlite::{params, OptionalExtension};
use crate::core::session::{
    now_ts, RequestUsageRecord, SessionTokenTotals, UsageBreakdown, UsageSource,
    UsageSourceBreakdown,
};
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
pub(in crate::core::session) fn add_session_token_totals_in(
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
pub(in crate::core::session) fn insert_request_usage_in(
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
pub(in crate::core::session) fn session_usage_breakdown_in(
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
pub(in crate::core::session) fn session_usage_writable_in(
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
