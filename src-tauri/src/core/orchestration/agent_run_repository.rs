//! # repository.rs — Agent Run SQLite 仓储
//!
//! 持久化主 Agent 执行记录与「每轮一行」的运行事件，替代 run.json/events.jsonl/checkpoint.json。
//!
//! ## Key Exports
//! - `upsert_run()`: 保存运行记录
//! - `list_runs()`: 查询运行记录
//! - `upsert_loop_event()`: 写入/覆盖某轮的完整事件行
//! - `append_loop_delta()`: 帧级增量拼接（开关开启时）
//! - `load_loop_events()`: 按 `loop_index` 顺序取出某 run 的全部轮次
//!
//! ## Dependencies
//! - Internal: `crate::infra::db`, `crate::infra::types::models`
//! - External: `rusqlite`, `serde_json`

use rusqlite::{params, OptionalExtension, Row};

use crate::core::orchestration::agent_runs::{
    AgentRun, AgentRunLoopEvent, AgentRunStatus,
};

/// `agent_runs` 的列清单（SELECT / INSERT 共用，避免两处漂移）。
///
/// ⚠️ 列顺序与 [`run_from_row`] 的**下标硬编码**必须一一对应；
/// 任何增删列都要同时改这两处（v15 已删除三个 live 列；
/// v19 删除只写不读的 `user_message_preview` / `error`；
/// v20 追加 `interrupt_kind`——**追加在末尾**，故 0..=13 的下标不受影响）。
const RUN_COLUMNS: &str = "run_id, session_id, status, message_id, loop_count, \
     input_tokens, output_tokens, started_at, updated_at, finished_at, last_safe_point, \
     summary, resumable, resumed_from_run_id, interrupt_kind";

pub fn upsert_run(run: &AgentRun) -> Result<(), String> {
    crate::infra::db::with_connection(|conn| {
        conn.execute(
            "INSERT INTO agent_runs(
                run_id, session_id, status, message_id, loop_count, input_tokens, output_tokens,
                started_at, updated_at, finished_at, last_safe_point,
                summary, resumable, resumed_from_run_id, interrupt_kind
            ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15)
            ON CONFLICT(run_id) DO UPDATE SET
                session_id = excluded.session_id,
                status = excluded.status,
                message_id = excluded.message_id,
                loop_count = excluded.loop_count,
                input_tokens = excluded.input_tokens,
                output_tokens = excluded.output_tokens,
                started_at = excluded.started_at,
                updated_at = excluded.updated_at,
                finished_at = excluded.finished_at,
                last_safe_point = excluded.last_safe_point,
                summary = excluded.summary,
                resumable = excluded.resumable,
                resumed_from_run_id = excluded.resumed_from_run_id,
                interrupt_kind = excluded.interrupt_kind",
            params![
                run.run_id,
                run.session_id,
                status_to_str(&run.status),
                run.message_id,
                run.loop_count as i64,
                run.input_tokens as i64,
                run.output_tokens as i64,
                run.started_at as i64,
                run.updated_at as i64,
                run.finished_at.map(|v| v as i64),
                run.last_safe_point,
                run.summary,
                if run.resumable { 1 } else { 0 },
                run.resumed_from_run_id,
                run.interrupt_kind,
            ],
        )
        .map_err(|e| e.to_string())?;
        Ok(())
    })
}

pub fn list_runs(session_id: Option<&str>) -> Result<Vec<AgentRun>, String> {
    crate::infra::db::with_connection(|conn| {
        let sql = if session_id.is_some() {
            format!("SELECT {} FROM agent_runs WHERE session_id = ?1 ORDER BY started_at", RUN_COLUMNS)
        } else {
            format!("SELECT {} FROM agent_runs ORDER BY started_at", RUN_COLUMNS)
        };
        let mut stmt = conn.prepare(&sql).map_err(|e| e.to_string())?;
        let rows = if let Some(session_id) = session_id {
            stmt.query_map([session_id], run_from_row)
                .map_err(|e| e.to_string())?
                .collect::<Result<Vec<_>, _>>()
                .map_err(|e| e.to_string())?
        } else {
            stmt.query_map([], run_from_row)
                .map_err(|e| e.to_string())?
                .collect::<Result<Vec<_>, _>>()
                .map_err(|e| e.to_string())?
        };
        Ok(rows)
    })
}

pub fn load_run(run_id: &str) -> Result<Option<AgentRun>, String> {
    crate::infra::db::with_connection(|conn| {
        conn.query_row(
            &format!("SELECT {} FROM agent_runs WHERE run_id = ?1", RUN_COLUMNS),
            [run_id],
            run_from_row,
        )
        .optional()
        .map_err(|e| e.to_string())
    })
}

/// 根据 message_id 查找 agent_run
pub fn find_by_message_id(message_id: &str) -> Result<Option<AgentRun>, String> {
    if message_id.is_empty() { return Ok(None); }
    crate::infra::db::with_connection(|conn| {
        conn.query_row(
            &format!(
                "SELECT {} FROM agent_runs WHERE message_id = ?1 ORDER BY started_at DESC LIMIT 1",
                RUN_COLUMNS
            ),
            [message_id],
            run_from_row,
        )
        .optional()
        .map_err(|e| e.to_string())
    })
}

/// 删除指定 run 的全部轮次事件
pub fn delete_events_by_run(run_id: &str) -> Result<(), String> {
    crate::infra::db::with_connection(|conn| {
        conn.execute("DELETE FROM agent_run_events WHERE run_id = ?1", [run_id])
            .map_err(|e| e.to_string())?;
        Ok(())
    })
}

/// 删除 agent_run 记录
pub fn delete_run(run_id: &str) -> Result<(), String> {
    crate::infra::db::with_connection(|conn| {
        conn.execute("DELETE FROM agent_runs WHERE run_id = ?1", [run_id])
            .map_err(|e| e.to_string())?;
        Ok(())
    })
}

/// 写入/覆盖某轮的完整事件行（按唯一键 `(run_id, loop_index)`）。
///
/// 用于 **loop 收尾** 的结构化落库：`resp_blocks` / `tool_results` 都是
/// `Vec<ContentBlock>` 的 JSON。同一轮重复调用是幂等的（整行覆盖）。
pub fn upsert_loop_event(event: &AgentRunLoopEvent) -> Result<(), String> {
    let resp_blocks = serde_json::to_string(&event.resp_blocks).map_err(|e| e.to_string())?;
    let tool_results = serde_json::to_string(&event.tool_results).map_err(|e| e.to_string())?;
    crate::infra::db::with_connection(|conn| {
        conn.execute(
            "INSERT INTO agent_run_events(
                event_id, run_id, session_id, loop_index, resp_blocks, tool_results,
                status, error, input_tokens, output_tokens, model, started_at, updated_at
            ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13)
            ON CONFLICT(run_id, loop_index) DO UPDATE SET
                resp_blocks = excluded.resp_blocks,
                tool_results = excluded.tool_results,
                status = excluded.status,
                error = excluded.error,
                input_tokens = excluded.input_tokens,
                output_tokens = excluded.output_tokens,
                model = excluded.model,
                updated_at = excluded.updated_at",
            params![
                event.event_id,
                event.run_id,
                event.session_id,
                event.loop_index as i64,
                resp_blocks,
                tool_results,
                event.status,
                event.error,
                event.input_tokens as i64,
                event.output_tokens as i64,
                event.model,
                event.started_at as i64,
                event.updated_at as i64,
            ],
        )
        .map_err(|e| e.to_string())?;
        Ok(())
    })
}

/// 帧级增量：把文本片段**单语句拼接**到 `resp_blocks` 尾部，不回读整行。
///
/// ## 为什么必须是单语句 `||`
///
/// 旧实现（`live_content`）是"每帧 SELECT 整行 → 改字段 → UPDATE 整行"，
/// 一次 2000 字回复 ≈ 300 次整行读写 + fsync。搬到 events 表后若照抄，
/// **开销一点不省**（只是把写放大从一张表挪到另一张表）——那是假优化。
/// 这里改为 `resp_blocks = resp_blocks || ?`：一条语句、不回读、不写整行，
/// 写放大从 ~300x 降到 ~1x。
///
/// 仅用于 `status = 'streaming'` 期间**纯文本累积**（此时 `resp_blocks` 是原始
/// 文本片段，不是 JSON）。loop 收尾时会被结构化 JSON 整行覆盖（见 `upsert_loop_event`）。
pub fn append_loop_delta(
    run_id: &str,
    session_id: &str,
    loop_index: usize,
    delta: &str,
    model: Option<&str>,
) -> Result<(), String> {
    let now = now_millis();
    crate::infra::db::with_connection(|conn| {
        // 先尝试"行已存在则拼接"；行不存在时插入一条初始行再拼接。
        // 用 INSERT ... ON CONFLICT DO UPDATE 一次完成，避免"SELECT 判断"这种回读。
        conn.execute(
            "INSERT INTO agent_run_events(
                event_id, run_id, session_id, loop_index, resp_blocks, tool_results,
                status, error, input_tokens, output_tokens, model, started_at, updated_at
            ) VALUES (?1, ?2, ?3, ?4, ?5, '', 'streaming', NULL, 0, 0, ?6, ?7, ?7)
            ON CONFLICT(run_id, loop_index) DO UPDATE SET
                resp_blocks = agent_run_events.resp_blocks || ?5,
                status      = 'streaming',
                updated_at  = ?7",
            params![
                format!("are_{}", &uuid::Uuid::new_v4().to_string()[..8]),
                run_id,
                session_id,
                loop_index as i64,
                delta,
                model,
                now as i64,
            ],
        )
        .map_err(|e| e.to_string())?;
        Ok(())
    })
}

/// 把某轮的 `tool_results` 以单语句拼接写入（工具结果 JSON 分片累积）。
///
/// 与 [`append_loop_delta`] 同源：结构事件（工具结果）也要能帧级落盘而不回读整行。
pub fn append_loop_tool_results(
    run_id: &str,
    session_id: &str,
    loop_index: usize,
    fragment: &str,
    model: Option<&str>,
) -> Result<(), String> {
    let now = now_millis();
    crate::infra::db::with_connection(|conn| {
        conn.execute(
            "INSERT INTO agent_run_events(
                event_id, run_id, session_id, loop_index, resp_blocks, tool_results,
                status, error, input_tokens, output_tokens, model, started_at, updated_at
            ) VALUES (?1, ?2, ?3, ?4, '', ?5, 'streaming', NULL, 0, 0, ?6, ?7, ?7)
            ON CONFLICT(run_id, loop_index) DO UPDATE SET
                tool_results = agent_run_events.tool_results || ?5,
                updated_at   = ?7",
            params![
                format!("are_{}", &uuid::Uuid::new_v4().to_string()[..8]),
                run_id,
                session_id,
                loop_index as i64,
                fragment,
                model,
                now as i64,
            ],
        )
        .map_err(|e| e.to_string())?;
        Ok(())
    })
}

/// 确保某轮事件行存在（不存在则以空内容 + `streaming` 状态建一行）。
///
/// 供中断标记使用：`mark_loop_event_interrupted` 要先保证有行可改，
/// 否则"第一轮刚起就被取消"这种情况会连痕迹都留不下。
pub fn ensure_loop_event(run_id: &str, session_id: &str, loop_index: usize) -> Result<(), String> {
    let now = now_millis();
    crate::infra::db::with_connection(|conn| {
        conn.execute(
            "INSERT INTO agent_run_events(
                event_id, run_id, session_id, loop_index, resp_blocks, tool_results,
                status, error, input_tokens, output_tokens, model, started_at, updated_at
            ) VALUES (?1, ?2, ?3, ?4, '', '', 'streaming', NULL, 0, 0, NULL, ?5, ?5)
            ON CONFLICT(run_id, loop_index) DO NOTHING",
            params![
                format!("are_{}", &uuid::Uuid::new_v4().to_string()[..8]),
                run_id,
                session_id,
                loop_index as i64,
                now as i64,
            ],
        )
        .map_err(|e| e.to_string())?;
        Ok(())
    })
}

/// 把某轮标记为 `interrupted` 并记录原因（**不动 resp/tool 内容**）。
///
/// 帧级通道已落盘的半截文本必须保留——中断是"没跑完"，不是"没发生"。
pub fn set_loop_event_interrupted(
    run_id: &str,
    loop_index: usize,
    error: &str,
) -> Result<(), String> {
    let now = now_millis();
    crate::infra::db::with_connection(|conn| {
        conn.execute(
            "UPDATE agent_run_events SET status = 'interrupted', error = ?1, updated_at = ?2
             WHERE run_id = ?3 AND loop_index = ?4",
            params![error, now as i64, run_id, loop_index as i64],
        )
        .map_err(|e| e.to_string())?;
        Ok(())
    })
}

/// 按 `loop_index` 顺序取出某 run 的全部轮次（崩溃重建的唯一数据源）。
///
/// ⚠️ `ORDER BY loop_index` 必须显式写：不依赖 SQLite 的插入顺序。
pub fn load_loop_events(run_id: &str) -> Result<Vec<AgentRunLoopEvent>, String> {
    crate::infra::db::with_connection(|conn| {
        let mut stmt = conn
            .prepare(
                "SELECT event_id, run_id, session_id, loop_index, resp_blocks, tool_results,
                        status, error, input_tokens, output_tokens, model, started_at, updated_at
                 FROM agent_run_events WHERE run_id = ?1 ORDER BY loop_index",
            )
            .map_err(|e| e.to_string())?;
        let rows = stmt
            .query_map([run_id], loop_event_from_row)
            .map_err(|e| e.to_string())?
            .collect::<Result<Vec<_>, _>>()
            .map_err(|e| e.to_string())?;
        Ok(rows)
    })
}

fn run_from_row(row: &Row<'_>) -> rusqlite::Result<AgentRun> {
    // ⚠️ 下标与 `RUN_COLUMNS` 严格对应（v19 删掉 user_message_preview / error 后整体前移）
    Ok(AgentRun {
        run_id: row.get(0)?,
        session_id: row.get(1)?,
        status: status_from_str(row.get::<_, String>(2)?.as_str()),
        message_id: row.get(3)?,
        loop_count: row.get::<_, i64>(4)? as usize,
        input_tokens: row.get::<_, i64>(5)? as u64,
        output_tokens: row.get::<_, i64>(6)? as u64,
        started_at: row.get::<_, i64>(7)? as u64,
        updated_at: row.get::<_, i64>(8)? as u64,
        finished_at: row.get::<_, Option<i64>>(9)?.map(|value| value as u64),
        last_safe_point: row.get(10)?,
        summary: row.get(11)?,
        resumable: row.get::<_, i64>(12)? != 0,
        resumed_from_run_id: row.get(13)?,
        interrupt_kind: row.get(14)?,
    })
}

/// 解析 `resp_blocks` / `tool_results` 两个 JSON 列。
///
/// 帧级通道写入期间这两列可能是**原始文本片段**（非合法 JSON），
/// 解析失败时退化为"整段当作文本块"（`resp_blocks`）或"空"（`tool_results`），
/// 保证重建不 panic、不丢数据。
fn parse_blocks(raw: &str) -> Vec<crate::infra::types::models::ContentBlock> {
    use crate::infra::types::models::ContentBlock;
    if raw.trim().is_empty() {
        return Vec::new();
    }
    match serde_json::from_str::<Vec<ContentBlock>>(raw) {
        Ok(blocks) => blocks,
        // 降级：帧级拼接的半截文本（只有 resp_blocks 会这样）→ 当正文块
        Err(_) => vec![ContentBlock::Text {
            text: raw.to_string(),
        }],
    }
}

fn loop_event_from_row(row: &Row<'_>) -> rusqlite::Result<AgentRunLoopEvent> {
    let resp_raw: String = row.get(4)?;
    let tool_raw: String = row.get(5)?;
    Ok(AgentRunLoopEvent {
        event_id: row.get(0)?,
        run_id: row.get(1)?,
        session_id: row.get(2)?,
        loop_index: row.get::<_, i64>(3)? as usize,
        resp_blocks: parse_blocks(&resp_raw),
        // 工具结果是结构化 JSON 数组；解析失败（半截）时退化为空，不误当正文
        tool_results: serde_json::from_str(&tool_raw).unwrap_or_default(),
        status: row.get(6)?,
        error: row.get(7)?,
        input_tokens: row.get::<_, i64>(8)? as u64,
        output_tokens: row.get::<_, i64>(9)? as u64,
        model: row.get(10)?,
        started_at: row.get::<_, i64>(11)? as u64,
        updated_at: row.get::<_, i64>(12)? as u64,
    })
}

fn status_to_str(status: &AgentRunStatus) -> &'static str {
    match status {
        AgentRunStatus::Running => "running",
        AgentRunStatus::Completed => "completed",
        AgentRunStatus::Failed => "failed",
        AgentRunStatus::Cancelled => "cancelled",
        AgentRunStatus::Interrupted => "interrupted",
        AgentRunStatus::Recovering => "recovering",
        AgentRunStatus::Closed => "closed",
    }
}

fn status_from_str(value: &str) -> AgentRunStatus {
    match value {
        "completed" => AgentRunStatus::Completed,
        "failed" => AgentRunStatus::Failed,
        "cancelled" => AgentRunStatus::Cancelled,
        "interrupted" => AgentRunStatus::Interrupted,
        "recovering" => AgentRunStatus::Recovering,
        "closed" => AgentRunStatus::Closed,
        // ⚠️ 新枚举必须加在 `_ => Running` 兜底之前，否则会被吞成 Running
        // （Running 会被门卫按 STALE 判定捞出， Closed run 将被错误恢复）。
        _ => AgentRunStatus::Running,
    }
}

/// **原子抢占**遗留 run（恢复闸门的锁）。
///
/// 把符合条件的 run 置为 `recovering`，返回是否抢到。可抢条件：
/// - `interrupted`：收尾已落状态的遗留，无条件可抢；
/// - `running` 且超 STALE：进程崩溃遗留（更新时间早于 `stale_cutoff`）；
/// - `recovering` 且超 STALE：上一次恢复干到一半崩了，本次接管（状态机闭环）。
///
/// ⚠️ 必须用**单条条件 UPDATE** 而非"读-改-写"：SQLite 单写者锁保证这条语句
/// 的判定与写入原子——并发恢复（同进程多命令 / 多实例）只有一个调用方
/// 能拿到 affected > 0，其余全部抢空退出。这就是防"13ms 双写坏数据"的锁本体。
pub fn try_claim_run_for_recovery(run_id: &str, stale_cutoff: i64) -> Result<bool, String> {
    crate::infra::db::with_connection(|conn| {
        let now = now_millis() as i64;
        let affected = conn
            .execute(
                "UPDATE agent_runs
                 SET status = 'recovering', updated_at = ?3, finished_at = NULL
                 WHERE run_id = ?1
                   AND (status = 'interrupted'
                        OR (status IN ('running', 'recovering') AND updated_at < ?2))",
                params![run_id, stale_cutoff, now],
            )
            .map_err(|e| e.to_string())?;
        Ok(affected > 0)
    })
}

fn now_millis() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as u64
}
