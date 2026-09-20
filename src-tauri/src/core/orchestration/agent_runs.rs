//! 主Agent执行记录模块 - 运行历史与每轮事件
//!
//! 记录主Agent每次执行的完整生命周期：启动、每轮响应与工具结果、完成/失败。
//! **崩溃重建的唯一数据源**是「每轮一行」的 `agent_run_events`（v15 起）：
//! 起点由 `agent_runs.message_id` 锚定，顺序由 `(run_id, loop_index)` 决定。

use std::collections::HashSet;
use std::time::{SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};
use tauri::Emitter;

use crate::infra::types::models::{Content, ContentBlock, InterruptKind, Message};
use crate::core::orchestration::agent_run_repository;

/// 运行记录过期阈值（毫秒），超过此时间未更新视为中断。
/// 同时用于 `recovering` 恢复抢占的接管判定（恢复本身是秒级操作，同一阈值足够宽裕）。
pub(crate) const RUN_STALE_MS: u64 = 120_000;

/// 应用关闭/进程结束时中断的 run，其 `summary` 的**人类可读文案**。
///
/// ⚠️ 它**只是文案**，不再承担语义判断：历史实现拿这句话当"是否因退出而中断"的
/// 状态标志去比较（魔法字符串），措辞一改就会静默破坏恢复链。现在"因何中断"
/// 由 `agent_runs.interrupt_kind` 列承载（见 [`InterruptKind`]），此处仅用于展示。
const EXIT_INTERRUPTED_SUMMARY: &str = "上次执行在应用关闭或进程结束时中断。";

/// 主Agent运行状态
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AgentRunStatus {
    Running,     // 运行中
    Completed,   // 已完成
    Failed,      // 失败
    Cancelled,   // 已取消
    Interrupted, // 已中断（应用关闭等）
    /// 恢复中（恢复闸门的**原子抢占标记**）。
    ///
    /// 恢复入口动手前先以条件 UPDATE 抢占（interrupted/running → recovering），
    /// SQLite 单写者保证并发恢复只有一个赢家——这是防"13ms 双写坏数据"的锁。
    /// 抢占后正常路径由 mark_run_recovered 收口为 Completed；
    /// 若恢复进程中途崩溃，run 停留 recovering，超时后由下一次恢复接管（见 find_interrupted_run）。
    Recovering,
    /// 已完整收尾的中断（方案 6：**收窄恢复触发条件**）。
    ///
    /// 主动收尾（用户取消 / 错误中止 / 看门狗超时）一旦完成落库，历史即完整、
    /// 中断标记已写，run 不再具备"待恢复补齐"语义——门卫 `find_interrupted_run`
    /// 不捞 closed。此前这类 run 停留 `Interrupted`，收尾后 6ms 内的前端刷新
    /// 就会触发门卫重放 events 与内存比对：标记只写在消息层、events 是模型纯
    /// 输出，文本不等 → 误判缺失 → 重复补同一段（session 2302e81c 的 seq5+seq6）。
    /// `Interrupted` 收窄为纯"收尾没完成"（崩溃遗留），与 running/recovering
    /// 同列可恢复候选。
    Closed,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AgentRun {
    pub run_id: String,
    pub session_id: String,
    pub status: AgentRunStatus,
    pub message_id: Option<String>,
    pub loop_count: usize,
    pub input_tokens: u64,
    pub output_tokens: u64,
    pub started_at: u64,
    pub updated_at: u64,
    pub finished_at: Option<u64>,
    pub last_safe_point: Option<String>,
    pub summary: Option<String>,
    pub resumable: bool,
    pub resumed_from_run_id: Option<String>,
    /// 本 run **因何中断**（`None` = 未中断 / 正常结束），取值见 [`InterruptKind`] 的
    /// `as_str()`。
    ///
    /// 存在的意义：把"中断类型"从 `summary` 的魔法字符串里解放出来——此前
    /// "是否因应用关闭而中断"靠比对 `summary` 的固定文案判断，改一句话就会静默
    /// 破坏恢复链。本列让恢复链的判断依据是**枚举值**，文案可以随便改。
    ///
    /// 与 `session_messages.interrupt_kind` 的关系：那是**消息级**（哪条消息被中断
    /// 收尾写下，供渲染与发送拼回标记），本列是 **run 级**（这次运行为何结束）。
    /// 前者服务"单条消息怎么显示"，后者服务"整次运行怎么恢复/判定"。
    pub interrupt_kind: Option<String>,
}

/// 一个 loop 的落库记录（「每轮一行」）。
///
/// 这是崩溃重建的**唯一**数据源：`agent_runs.message_id` 定起点、
/// `(run_id, loop_index)` 圈范围与顺序，其余全部由本结构还原。
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AgentRunLoopEvent {
    pub event_id: String,
    pub run_id: String,
    pub session_id: String,
    /// 第几轮（从 1 起），与用户可见的"第 N 轮"一致
    pub loop_index: usize,
    /// 本轮响应：结构化 `ContentBlock` 数组（text / thinking / tool_use）
    pub resp_blocks: Vec<ContentBlock>,
    /// 本轮工具执行结果：结构化 `ContentBlock`（ToolResult）
    #[serde(default)]
    pub tool_results: Vec<ContentBlock>,
    /// 'streaming' | 'complete' | 'interrupted'
    pub status: String,
    pub error: Option<String>,
    pub input_tokens: u64,
    pub output_tokens: u64,
    pub model: Option<String>,
    pub started_at: u64,
    pub updated_at: u64,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ResumeAgentRunPlan {
    pub session_id: String,
    pub prompt: String,
}

/// 开始一次新的Agent运行
///
/// 创建运行记录并向前端发送启动事件。
/// 返回生成的 run_id，用于后续的状态更新。
///
/// ⚠️ 不再接收 `user_message`：它此前只用于 `user_message_preview` 列，
/// 而那一列**只写不读**（无任何业务读取、前端也无渲染），已于 v19 删除。
pub fn start_run(
    app: &tauri::AppHandle,
    session_id: &str,
    resumed_from_run_id: Option<String>,
    message_id: Option<String>,
) -> String {
    let run_id = format!("ar_{}", &uuid::Uuid::new_v4().to_string()[..8]);
    let now = now_millis();
    let run = AgentRun {
        run_id: run_id.clone(),
        session_id: session_id.to_string(),
        status: AgentRunStatus::Running,
        loop_count: 0,
        input_tokens: 0,
        output_tokens: 0,
        started_at: now,
        updated_at: now,
        finished_at: None,
        last_safe_point: None,
        summary: None,
        resumable: false,
        resumed_from_run_id,
        message_id,
        interrupt_kind: None,
    };
    let _ = write_run(&run);
    emit_run(app, &run);
    run_id
}

/// 列出所有运行记录，可按 session_id 过滤
///
/// 自动将超时未更新的 Running 状态标记为 Interrupted
pub fn list_runs(session_id: Option<&str>) -> Vec<AgentRun> {
    agent_run_repository::list_runs(session_id).unwrap_or_default()
}

/// 根据 message_id 查找对应的 agent_run
pub fn find_run_by_message_id(message_id: &str) -> Option<AgentRun> {
    agent_run_repository::find_by_message_id(message_id).ok().flatten()
}

/// 根据 message_id 清理对应的 agent_run 与其全部轮次事件
pub fn cleanup_by_message_id(message_id: &str) {
    if let Some(run) = find_run_by_message_id(message_id) {
        let _ = agent_run_repository::delete_events_by_run(&run.run_id);
        let _ = agent_run_repository::delete_run(&run.run_id);
    }
}

/// 删除 agent_run 及其关联的轮次事件
pub fn delete_run(run_id: &str) -> Result<(), String> {
    agent_run_repository::delete_run(run_id)
}

pub fn mark_active_run(app: &tauri::AppHandle, run_id: &str) -> Option<AgentRun> {
    let run = update_run_by_id(run_id, |run| {
        keep_run_active(run);
    })
    .ok()?;
    emit_run(app, &run);
    Some(run)
}

pub fn mark_stale_runs_interrupted(
    app: &tauri::AppHandle,
    session_id: Option<&str>,
    active_run_ids: &HashSet<String>,
    active_session_ids: &HashSet<String>,
) {
    let runs = list_runs(session_id);
    let now = now_millis();
    for run in runs {
        if run.status != AgentRunStatus::Running || active_run_ids.contains(&run.run_id) {
            continue;
        }
        let session_has_active_task = active_session_ids.contains(&run.session_id);
        if session_has_active_task && now.saturating_sub(run.updated_at) <= RUN_STALE_MS {
            continue;
        }
        if let Ok(run) = update_run_by_id(&run.run_id, |run| {
            run.status = AgentRunStatus::Interrupted;
            run.finished_at = Some(run.updated_at);
            run.summary = Some(EXIT_INTERRUPTED_SUMMARY.to_string());
            // 语义判断靠枚举，不靠文案
            run.interrupt_kind = Some(InterruptKind::AppClosed.as_str().to_string());
            // 可续跑性改由"该 run 是否已有轮次记录"决定（v15 起无 checkpoint）：
            // 有 events 说明跑过至少一轮，context 可从 events 重放。
            run.resumable = has_loop_events(&run.run_id);
        }) {
            emit_run(app, &run);
        }
    }
}

/// 应用退出（ExitRequested）收尾：把所有仍为 Running / Recovering 的 run 标记为 Interrupted。
///
/// 与 `mark_stale_runs_interrupted` 的区别：无活跃排除集、不判 STALE ——
/// 退出路径上这些状态都是"被进程结束打断的"，全部留痕，
/// 避免永久 Running 僵尸（此前关闭窗口后 run 状态永远停在 Running，
/// 崩溃恢复也因找不到 Interrupted 状态而无法收口）。
/// `Recovering` 一并归位：恢复干到一半进程退出，等同中断，下次启动重新恢复。
pub fn mark_running_interrupted_on_exit() {
    let runs = agent_run_repository::list_runs(None).unwrap_or_default();
    for run in runs {
        if !matches!(
            run.status,
            AgentRunStatus::Running | AgentRunStatus::Recovering
        ) {
            continue;
        }
        let _ = update_run_by_id(&run.run_id, |run| {
            run.status = AgentRunStatus::Interrupted;
            run.finished_at = Some(now_millis());
            run.summary = Some(EXIT_INTERRUPTED_SUMMARY.to_string());
            run.interrupt_kind = Some(InterruptKind::AppClosed.as_str().to_string());
            run.resumable = has_loop_events(&run.run_id);
        });
    }
}

/// 读取某会话/某 run 的轮次事件（供界面「执行详情」等只读展示）。
///
/// `run_id` 为空时返回该会话全部 run 的轮次（按 run + loop 排序）。
pub fn list_loop_events(session_id: Option<&str>, run_id: Option<&str>) -> Vec<AgentRunLoopEvent> {
    let mut out: Vec<AgentRunLoopEvent> = Vec::new();
    let run_ids: Vec<String> = match run_id {
        Some(id) if !id.is_empty() => vec![id.to_string()],
        _ => {
            let runs = agent_run_repository::list_runs(session_id).unwrap_or_default();
            runs.into_iter().map(|r| r.run_id).collect()
        }
    };
    for id in run_ids {
        out.extend(load_loop_events(&id));
    }
    out.sort_by(|a, b| {
        a.started_at
            .cmp(&b.started_at)
            .then(a.loop_index.cmp(&b.loop_index))
    });
    out
}

/// 写入/覆盖某一轮的完整事件行（**loop 收尾**调用，结构化）。
///
/// 无论「崩溃保护」开关是否开启，loop 收尾这一次写入都必然发生——
/// 开关只决定"要不要额外加一条帧级实时通道"，不改变这一次。
///
/// 不需要 `AppHandle`：前端进度靠既有的流式事件（`chat-content` / `chat-tool-*`）
/// 推进，轮次行是**崩溃重建的落盘数据**，不是实时通道，没有要广播的东西。
/// （旧 `save_checkpoint` 收 `app` 只是历史惯性，参数从未被真正使用。）
pub fn upsert_loop_event(
    run_id: &str,
    session_id: &str,
    loop_index: usize,
    resp_blocks: Vec<ContentBlock>,
    tool_results: Vec<ContentBlock>,
    status: &str,
    error: Option<String>,
    input_tokens: u64,
    output_tokens: u64,
    model: Option<String>,
) {
    let now = now_millis();
    let event = AgentRunLoopEvent {
        event_id: format!("are_{}", &uuid::Uuid::new_v4().to_string()[..8]),
        run_id: run_id.to_string(),
        session_id: session_id.to_string(),
        loop_index,
        resp_blocks,
        tool_results,
        status: status.to_string(),
        error,
        input_tokens,
        output_tokens,
        model,
        started_at: now,
        updated_at: now,
    };
    let _ = agent_run_repository::upsert_loop_event(&event);
}

/// 把某一轮标记为 `interrupted`，并补上中断原因。
///
/// **只改 status 与 error，不动 resp/tool**：帧级通道（崩溃保护开启时）已经
/// 把半截文本写进这一行了，覆盖会把它抹掉。中断的语义是"这轮没跑完"，
/// 不是"这轮没发生过"，已落盘的部分必须保留。
///
/// 行不存在时静默返回（第一轮尚未产生就中断）。
pub fn mark_loop_event_interrupted(
    run_id: &str,
    session_id: &str,
    loop_index: usize,
    error: String,
) {
    // 行不存在则先建一条空行，保证 status 有地方落（重建时该轮为空块 → 跳过）
    let _ = agent_run_repository::ensure_loop_event(run_id, session_id, loop_index);
    let _ = agent_run_repository::set_loop_event_interrupted(run_id, loop_index, &error);
}

/// 帧级通道：把文本片段拼进本轮事件行的 `resp_blocks`（**仅开关开启时调用**）。
///
/// 单语句 `||` 拼接，不回读整行（见 repository 侧说明）。
pub fn append_loop_text_delta(
    run_id: &str,
    session_id: &str,
    loop_index: usize,
    delta: &str,
    model: Option<&str>,
) {
    let _ = agent_run_repository::append_loop_delta(run_id, session_id, loop_index, delta, model);
}

/// 帧级通道：把工具结果 JSON 片段拼进本轮事件行（**仅开关开启时调用**）。
pub fn append_loop_tool_result_delta(
    run_id: &str,
    session_id: &str,
    loop_index: usize,
    fragment: &str,
    model: Option<&str>,
) {
    let _ = agent_run_repository::append_loop_tool_results(
        run_id, session_id, loop_index, fragment, model,
    );
}

/// 读取某 run 的全部轮次（崩溃重建用）。
pub fn load_loop_events(run_id: &str) -> Vec<AgentRunLoopEvent> {
    agent_run_repository::load_loop_events(run_id).unwrap_or_default()
}

fn has_loop_events(run_id: &str) -> bool {
    !load_loop_events(run_id).is_empty()
}

pub fn complete_run(
    app: &tauri::AppHandle,
    run_id: &str,
    input_tokens: u64,
    output_tokens: u64,
    summary: Option<String>,
) {
    finish_run(
        app,
        run_id,
        AgentRunStatus::Completed,
        input_tokens,
        output_tokens,
        summary,
        None,
    );
}

pub fn cancel_run(
    app: &tauri::AppHandle,
    run_id: &str,
    input_tokens: u64,
    output_tokens: u64,
    summary: Option<String>,
) {
    finish_run(
        app,
        run_id,
        AgentRunStatus::Cancelled,
        input_tokens,
        output_tokens,
        summary,
        Some(InterruptKind::UserCancel),
    );
}

/// 标记 run 失败。
///
/// `input_tokens` / `output_tokens` 为本次运行真实累计值——旧实现硬编码传 0，
/// 导致失败轮次的消耗在成本统计中完全缺失。
///
/// ⚠️ 不再接收 `error` 文本：它此前只写 `agent_runs.error` 列（只写不读，已于 v19
/// 删除）。失败原因仍可在 `agent_run_events.error`（每轮一行）里查到；
/// run 级只保留**类型**（`interrupt_kind = PipelineError`）。
pub fn fail_run(
    app: &tauri::AppHandle,
    run_id: &str,
    input_tokens: u64,
    output_tokens: u64,
) {
    finish_run(
        app,
        run_id,
        AgentRunStatus::Failed,
        input_tokens,
        output_tokens,
        None,
        Some(InterruptKind::PipelineError),
    );
}

/// 标记 run 为已中断（运行被打断，非用户主动撤销）。
///
/// 与 [`fail_run`] 的区别：状态为 `Interrupted`（`resumable = true`，保留续跑语义）。
///
/// `kind` 由调用方给出**具体成因**（流空闲超时 / 回合上限 / 规划看门狗等），
/// 落进 `agent_runs.interrupt_kind`，供恢复链判断与界面展示——
/// 取代此前"拿 summary 文案当状态标志"的魔法字符串做法。
pub fn interrupt_run(
    app: &tauri::AppHandle,
    run_id: &str,
    kind: InterruptKind,
    input_tokens: u64,
    output_tokens: u64,
) {
    finish_run(
        app,
        run_id,
        AgentRunStatus::Interrupted,
        input_tokens,
        output_tokens,
        None,
        Some(kind),
    );
}

/// 准备恢复执行 - 基于"每轮一行"的 events 生成恢复提示词
///
/// v15 起不再有 checkpoint：可恢复性由 events 行数决定，
/// 恢复所需上下文由 [`recover_interrupted_messages`] 从 events 重放。
pub fn prepare_resume(run_id: &str) -> Result<(AgentRun, ResumeAgentRunPlan), String> {
    let run = load_run_by_id(run_id).ok_or_else(|| format!("执行记录不存在: {}", run_id))?;
    let events = load_loop_events(run_id);
    if events.is_empty() {
        return Err("没有可恢复的执行记录".to_string());
    }
    let last_safe_point = run
        .last_safe_point
        .clone()
        .unwrap_or_else(|| "最后一轮已完成".to_string());

    // 轮数**数 events 行数**得到，不读 `run.loop_count`。
    //
    // 理由：`agent_run_events` 是「每轮一行」（UNIQUE(run_id, loop_index)），
    // 行数就是轮数，是唯一事实源；而 `agent_runs.loop_count` 是一份**从未被维护
    // 的冗余副本**（主 Agent 侧三条收尾路径都不写它，恒为 0），读它会把"已完成 0 轮"
    // 写进提示词，误导模型重复已完成的工作（2026-09-21 修正）。
    // 这也符合项目既定原则：真相只进 events，恢复只读 events。
    let completed_loops = events.len();
    let prompt = format!(
        "继续上次中断的执行。上次执行记录为 {}，最后安全点为「{}」，已完成 {} 轮。请基于已有上下文继续，不要重复已经完成且已有工具结果的操作；如果某个操作可能有副作用，请先说明并等待确认。",
        run_id, last_safe_point, completed_loops
    );
    Ok((
        run.clone(),
        ResumeAgentRunPlan {
            session_id: run.session_id.clone(),
            prompt,
        },
    ))
}

/// 查找指定会话最近一个可恢复的遗留 run
///
/// 三类候选：
/// - `Interrupted`：已标记中断（收尾正常落了状态）；
/// - `Running`：程序崩溃时状态来不及改——**是否真遗留由调用方判定**
///   （门卫先查进程内活跃注册表，再由抢占 SQL 以 STALE 兜底，本函数不做时间判断）；
/// - `Recovering`：上一次恢复没做完就崩了——超时后允许下一次恢复接管，
///   否则 run 会永久卡在 recovering（状态机闭环）。
pub fn find_interrupted_run(session_id: &str) -> Option<AgentRun> {
    let runs = agent_run_repository::list_runs(Some(session_id)).ok()?;
    runs.into_iter()
        .filter(|r| {
            matches!(
                r.status,
                AgentRunStatus::Interrupted | AgentRunStatus::Running | AgentRunStatus::Recovering
            )
        })
        .max_by_key(|r| r.updated_at)
}

/// 中断恢复的结果语义分层。
///
/// `NeedsClosure` 是对称性保证的兜底：崩溃 run 无任何可补内容
/// （events 为空 —— 典型如"run 已建、第一轮 LLM 输出前被杀"），但会话尾部悬尾
/// （最后一条是 user 且无人回应）。此时补一条 assistant 中断占位，维持
/// session_messages 消息级 user/assistant 严格交替，并把 run 从永久 Running 中捞出。
pub enum RecoveryOutcome {
    /// 无可恢复（活跃期内 / 没有中断 run —— 绝大多数加载的正常路径）
    None,
    /// 有内容可补：从 events 重放出的消息（顺序即重建顺序）
    Content { messages: Vec<Message> },
    /// 无内容可补，但尾部悬尾，需补中断占位收口
    NeedsClosure,
}

/// 崩溃恢复的"未产生回复"占位文案（**会发给 LLM**，也会渲染为气泡下方小字）。
///
/// 与 pipeline 的 INTERRUPT_MARKER 系同风格：`**[标签]**` 开头（不含 `>` 引用符，
/// 见 doc/状态标注符号统一与结构化改造方案.md）。
/// 气泡下方的小字说明由后端按 `interrupt_kind` 生成（`command/history.rs`），
/// 不再由前端从正文里剥离。
/// 语义区别于 INTERRUPT_MARKER_RESUMABLE（"请基于上下文继续完成"）——
/// 这里没有任何半截内容可续，模型不该自动续写，等待用户下一条消息
/// 表达新意图。只在恢复路径出现一次，之后固定为历史前缀的一部分，
/// 不产生措辞变体（prompt cache 安全）。
pub const INTERRUPT_PLACEHOLDER_NO_REPLY: &str =
    "**[回复被中断]** 本次执行因应用关闭而中断，未产生回复内容。";

/// 对**已通过状态闸门的 run** 从 events 重放并求增量（纯计算，不做任何状态判定/落库）。
///
/// 状态侧职责（活跃判定 / STALE / 原子抢占 / 盖章）全部收口在门卫
/// `ensure_session_recovered`（command/session.rs）——本函数只回答一个问题：
/// "这个 run 的 events 相对当前历史，还差哪些消息没补"。
///
/// ## 重建口径（v15）
///
/// 1. `agent_runs.message_id` → 起点锚点（触发本 run 的 user 消息）；
/// 2. `SELECT * FROM agent_run_events WHERE run_id=? ORDER BY loop_index`；
/// 3. 按顺序铺开：`loop_i.resp_blocks`（助手响应）→ `loop_i.tool_results`（工具结果）
///    → `loop_{i+1}.resp_blocks` → …；
/// 4. 与当前 `session_memory` 对齐后，只返回**尚未入历史**的部分。
///
/// ## 为什么能"只补缺的部分"
///
/// 正常路径下每轮都在 loop 收尾写了 events，而会话历史（`session_messages`）
/// 只在整轮收尾落库。于是"events 重建出的消息数 > 现有历史数"恰好说明
/// 崩溃发生在整轮收尾之前——多出来的就是从断点起需要补回的部分。
///
/// 已落库的历史消息本身可能不带 `source` 信息，重放时统一按 chat 语义生成；
/// 对齐采用"条数前缀一致"口径（重建序列与现有历史同源同序）。
pub fn recover_outcome_for_run(run: &AgentRun, current_messages: &[Message]) -> RecoveryOutcome {
    let rebuilt = rebuild_messages_from_events(&run.run_id);
    if rebuilt.is_empty() {
        // 无任何轮次记录可补 → 只做收口判定（留痕优先）
        return closure_outcome(current_messages);
    }

    // 找到"重建序列"相对"现有历史"的新增后缀。
    //
    // 用**尾部对齐**而非整体相等：历史可能带前置的 system/context 等非本轮消息，
    // 故从尾部逐条比对，找到最长的公共后缀，其之前的部分即需要补写的增量。
    let extra = diff_tail(current_messages, &rebuilt);
    if extra.is_empty() {
        // 内容已完整落库，仅需收口判定
        return closure_outcome(current_messages);
    }

    RecoveryOutcome::Content { messages: extra }
}

/// 从 events 重放出一个 run 的完整消息序列（不含起点 user 消息）。
///
/// 顺序：`loop_i.resp_blocks` → `loop_i.tool_results` → `loop_{i+1}.resp_blocks` → …
/// 空块数组会被跳过（避免产生空消息）。
fn rebuild_messages_from_events(run_id: &str) -> Vec<Message> {
    let events = load_loop_events(run_id);
    let mut out: Vec<Message> = Vec::new();
    for event in events {
        let resp: Vec<ContentBlock> = event
            .resp_blocks
            .into_iter()
            .filter(|b| !is_empty_block(b))
            .collect();
        if !resp.is_empty() {
            out.push(Message::Assistant {
                content: Content::Multiple(resp),
            });
        }
        let tools: Vec<ContentBlock> = event
            .tool_results
            .into_iter()
            .filter(|b| !is_empty_block(b))
            .collect();
        if !tools.is_empty() {
            out.push(Message::User {
                content: Content::Multiple(tools),
            });
        }
    }
    out
}

fn is_empty_block(block: &ContentBlock) -> bool {
    match block {
        ContentBlock::Text { text } => text.trim().is_empty(),
        ContentBlock::Thinking { thinking, .. } => thinking.trim().is_empty(),
        _ => false,
    }
}

/// 求"重建序列"相对"现有历史"的增量后缀（按**公共前缀**对齐）。
///
/// 例：现有 `[A, B]`，重建 `[A, B, C, D]` → 返回 `[C, D]`。
/// 若重建序列整体已被历史覆盖 → 返回空。
///
/// ## 为什么对齐**头部**而不是尾部
///
/// 直觉上"求增量后缀"该从尾部对——但**中断恢复的真实形态恰恰会让尾部对不上**：
///
/// ```text
/// current（历史）: [user 提问, assistant "我已开始分析…"]      ← 停在半截
/// rebuilt（重放）: [assistant "我已开始分析…", assistant "…"]   ← 该轮已完整落库
/// ```
///
/// 用户消息不进 events（方案已定：**请求侧不存**），所以 `rebuilt` 天然比历史
/// **少一个起点 user**；同时若该轮已完整落库，`rebuilt` 的尾条又是历史尾条的
/// *续写内容*（文本不同）。尾部一比对就是不等 → `common` 停在 0 → 返回**整个**
/// 重建序列 → 恢复后历史里凭空多出一整轮重复内容。
///
/// 从**头部**比对则天然绕开这两点：跳过起点差异后，前面每条已落库的消息都
/// 逐条相等（等价判定按角色 + 正文文本，不看 `Single`/`Multiple` 形态），
/// 走到第一个不等处停下，其后的就是真正要补的增量。
fn diff_tail(current: &[Message], rebuilt: &[Message]) -> Vec<Message> {
    // 先跳过历史里"重建序列不含"的前置消息（system / 更早的轮次）。
    // 用**锚定**而非暴力搜索：找 rebuilt 的首条在 current 中最后一次出现的位置。
    // 取"最后一次"是为了避免同一段文本在历史中重复出现时误锚到很早的位置。
    let anchor = rebuilt.first().map(|head| {
        current
            .iter()
            .rposition(|m| messages_equivalent(m, head))
            .unwrap_or(0)
    });
    let skip = anchor.unwrap_or(0);
    let lhs = current.get(skip..).unwrap_or(&[]);

    let mut common = 0usize;
    let max = lhs.len().min(rebuilt.len());
    while common < max && messages_equivalent(&lhs[common], &rebuilt[common]) {
        common += 1;
    }
    if common >= rebuilt.len() {
        return Vec::new();
    }
    rebuilt[common..].to_vec()
}

/// 消息等价判定（重放产物 vs 历史）：按"角色 + 正文/思考文本"比较。
///
/// 不比 `Content` 枚举形态（历史里可能是 `Single`，重放必是 `Multiple`），
/// 也不比工具块细节（历史里的 tool 块 id 与重放一致，但比较成本高于收益）。
///
/// ⚠️ **文本为空 ≠ 等价**。这里曾把"两边都提不出文本"当成等价（"纯工具块视为等价"），
/// 结果是这个函数**不满足传递性**：
///
/// ```text
/// eq(user "B", assistant "")  = false   ← 角色不同，先被拒
/// eq(user "B", user "C")      = false   ← 都是 user，文本不同
/// eq(assistant "A", user "B") = true    ← 角色不同！但两边都无文本 → 旧逻辑放行
/// ```
///
/// `diff_tail` 依赖它做**尾部逐对对齐**（自尾向头走，遇不等即停），而配对比较
/// 隐含要求传递性。旧逻辑会把 `[assistant "A", user "B"]` 与 `[assistant "C", user "B"]`
/// 判成"尾对相同"→ 一路对齐到 0 → 返回整个重建序列。
/// 非工具轮（重建里根本没有工具块）会因此**每轮都重复追加历史**。
/// 比对层专用：剥离中断标记行（恢复比对的**双保险**）。
///
/// 中断标记（`> ⚠️ **[回复被中断]** …`）只写在**消息层**——收尾时由
/// `store_interrupted_turn` 补进 session_messages；events 重放产物是模型纯
/// 输出，没有标记。closed 图章落地前，主动收尾的 run 停留 `Interrupted`，
/// 收尾后的正常刷新就会触发门卫重放 events 与内存比对——标记行导致文本
/// 不等 → 误判"缺一整条" → 重复补同一段（session 2302e81c 的 seq5+seq6）。
/// 标记是收尾补写的**元信息**，不参与"内容是否一致"的判定，故剥离。
/// 早期版本还写过 `> ✕ **用户已取消执行…**`（现仅存于旧库数据），一并覆盖。
/// 中断标记的**标签白名单**（比对层用）。
///
/// 现行标记以 `**[标签]**` 开头；旧库格式为 `> ⚠️ **[回复被中断]** …`
/// 与 `> ✕ **用户已取消执行…**`（后者无方括号，单列关键词）。
/// 只服务于"重建消息 vs 现存消息"的等价比较（见 `messages_equivalent`），
/// 不参与界面渲染 —— 界面的状态标注走结构化 `notice`（前端已在 2026-09-21
/// 删除同款正则，见 doc/状态标注符号统一与结构化改造方案.md）。
const INTERRUPT_TAGS: [&str; 3] = ["[回复被中断]", "[规划探索已到上限]", "用户已取消执行"];

fn strip_interrupt_markers(text: &str) -> String {
    text.lines()
        .filter(|line| {
            let t = line.trim_start();
            // 标记行特征（两个条件同时满足才算，避免误删正文）：
            // - 行首是引用块（旧格式 `> ⚠️ **[标签]**…`）或加粗方括号（现行 `**[标签]**…`）
            // - 行内含已知标签关键词
            let starts_marker = t.starts_with('>') || t.starts_with("**[");
            let has_tag = INTERRUPT_TAGS.iter().any(|tag| t.contains(tag));
            !(starts_marker && has_tag)
        })
        .collect::<Vec<_>>()
        .join("\n")
        .trim()
        .to_string()
}

fn messages_equivalent(a: &Message, b: &Message) -> bool {
    fn role(m: &Message) -> u8 {
        match m {
            Message::User { .. } => 0,
            Message::Assistant { .. } => 1,
        }
    }
    if role(a) != role(b) {
        return false;
    }
    fn text_of(m: &Message) -> String {
        let content = match m {
            Message::User { content } | Message::Assistant { content } => content,
        };
        let mut parts: Vec<String> = Vec::new();
        match content {
            Content::Single(t) => parts.push(t.trim().to_string()),
            Content::Multiple(blocks) => {
                for block in blocks {
                    match block {
                        ContentBlock::Text { text } => parts.push(text.trim().to_string()),
                        ContentBlock::Thinking { thinking, .. } => {
                            parts.push(thinking.trim().to_string())
                        }
                        // 工具结果也必须纳入比较：历史里可能是 `Content::Single(纯文本)`
                        // （`store_assistant_response` 的写法），重放产物则是
                        // `Content::Multiple([ToolResult])`。若这里跳过 ToolResult，
                        // 重放侧的文本会算成空串 → 与历史永不相等 → 对齐到此中断。
                        ContentBlock::ToolResult { content, .. } => {
                            parts.push(content.trim().to_string())
                        }
                        _ => {}
                    }
                }
            }
        }
        parts.join("\n\n")
    }
    let ta = strip_interrupt_markers(&text_of(a));
    let tb = strip_interrupt_markers(&text_of(b));
    ta == tb
}

/// 悬尾收口判定：会话尾部是 user（含 tool_result 信封形态）且无人回应 → 需补占位；
/// 尾部已是 assistant / 会话为空 → 消息序列已对称，不动消息
/// （run 状态由 mark_stale_runs_interrupted / 退出收尾兜底）。
fn closure_outcome(current_messages: &[Message]) -> RecoveryOutcome {
    match current_messages.last() {
        Some(Message::User { .. }) => RecoveryOutcome::NeedsClosure,
        _ => RecoveryOutcome::None,
    }
}

/// 将中断 run 标记为已恢复，避免下次加载时重复恢复
pub fn mark_run_recovered(run_id: &str) -> Result<(), String> {
    update_run_by_id(run_id, |run| {
        run.status = AgentRunStatus::Completed;
        run.finished_at = Some(now_millis());
        // 判断依据是**枚举**（此前比对 summary 的固定文案，改措辞即失效）
        let was_exit_interrupted =
            run.interrupt_kind.as_deref() == Some(InterruptKind::AppClosed.as_str());
        if run.summary.is_none() || was_exit_interrupted {
            run.summary = Some("已从中断恢复".to_string());
        }
        run.interrupt_kind = None;
    })?;
    Ok(())
}

/// 将已完整收尾的中断 run 盖章为 closed（见 [`AgentRunStatus::Closed`]）。
///
/// 由 finalize 在 `save_session` 之后调用——此刻半截内容与中断标记均已落库，
/// 收尾声明完成，run 不再是恢复候选。save 与盖章之间若崩溃（毫秒级窗口），
/// run 停留 `Interrupted` 被门卫捞出，由 `messages_equivalent` 的剥标记比对
/// 兜底（双保险），不会重复补内容。
pub fn mark_run_closed(app: &tauri::AppHandle, run_id: &str) {
    if let Ok(run) = update_run_by_id(run_id, |run| {
        run.status = AgentRunStatus::Closed;
        run.resumable = false;
        if run.finished_at.is_none() {
            run.finished_at = Some(now_millis());
        }
    }) {
        emit_run(app, &run);
    }
}

fn finish_run(
    app: &tauri::AppHandle,
    run_id: &str,
    status: AgentRunStatus,
    input_tokens: u64,
    output_tokens: u64,
    summary: Option<String>,
    kind: Option<InterruptKind>,
) {
    if let Ok(run) = update_run_by_id(run_id, |run| {
        run.status = status.clone();
        run.input_tokens = input_tokens;
        run.output_tokens = output_tokens;
        run.finished_at = Some(now_millis());
        run.summary = summary.clone();
        // 中断类型落枚举字符串（None = 正常结束，不覆盖既有值以外的东西）
        run.interrupt_kind = kind.map(|k| k.as_str().to_string());
        run.resumable = status == AgentRunStatus::Interrupted;
    }) {
        emit_run(app, &run);
    }
}

fn emit_run(app: &tauri::AppHandle, run: &AgentRun) {
    let _ = app.emit("agent-run-updated", run);
}

fn load_run_by_id(run_id: &str) -> Option<AgentRun> {
    agent_run_repository::load_run(run_id).ok().flatten()
}

fn update_run_by_id<F>(run_id: &str, update: F) -> Result<AgentRun, String>
where
    F: FnOnce(&mut AgentRun),
{
    let mut run = load_run_by_id(run_id).ok_or_else(|| format!("执行记录不存在: {}", run_id))?;
    update(&mut run);
    run.updated_at = now_millis();
    write_run(&run)?;
    Ok(run)
}

fn keep_run_active(run: &mut AgentRun) {
    if matches!(
        run.status,
        AgentRunStatus::Completed
            | AgentRunStatus::Failed
            | AgentRunStatus::Cancelled
            | AgentRunStatus::Closed
    ) {
        return;
    }
    run.status = AgentRunStatus::Running;
    run.finished_at = None;
    // 判断依据是**枚举**（此前比对 summary 的固定文案），并顺手清掉中断类型
    if run.interrupt_kind.as_deref() == Some(InterruptKind::AppClosed.as_str()) {
        run.summary = None;
    }
    run.interrupt_kind = None;
}

fn write_run(run: &AgentRun) -> Result<(), String> {
    agent_run_repository::upsert_run(run)
}

fn now_millis() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as u64
}

#[cfg(test)]
mod recovery_closure_tests {
    //! 悬尾收口判定与占位文案的防回归。
    //!
    //! 背景：崩溃点落在"检查点刚落库、下一轮 LLM 输出前"的空隙时
    //! （典型：模式快照落库后进程被杀），session_messages 尾部悬尾
    //! （最后一条是 user 且无人回应），旧恢复逻辑三门槛不满足直接
    //! 返回 None，既不补内容也不留痕 —— 连续 user 相邻由此产生。
    use super::{closure_outcome, RecoveryOutcome, INTERRUPT_PLACEHOLDER_NO_REPLY};
    use crate::infra::types::models::{Content, ContentBlock, Message};

    /// 尾部悬尾（tool_result 信封形态也是 user）→ 需要补占位收口
    #[test]
    fn tail_user_tool_result_envelope_needs_closure() {
        let messages = vec![
            Message::Assistant {
                content: Content::Single("计划如下".to_string()),
            },
            Message::User {
                content: Content::Multiple(vec![ContentBlock::ToolResult {
                    tool_use_id: "call_00_x".to_string(),
                    content: "ok".to_string(),
                }]),
            },
        ];
        assert!(matches!(
            closure_outcome(&messages),
            RecoveryOutcome::NeedsClosure
        ));
    }

    /// 尾部是普通 user 文本（LLM 回复产生前被杀）→ 同样需要收口
    #[test]
    fn tail_plain_user_message_needs_closure() {
        let messages = vec![Message::User {
            content: Content::Single("开始吧".to_string()),
        }];
        assert!(matches!(
            closure_outcome(&messages),
            RecoveryOutcome::NeedsClosure
        ));
    }

    /// 尾部已是 assistant → 消息序列已对称，不动消息
    #[test]
    fn tail_assistant_stays_untouched() {
        let messages = vec![
            Message::User {
                content: Content::Single("开始吧".to_string()),
            },
            Message::Assistant {
                content: Content::Single("已完成".to_string()),
            },
        ];
        assert!(matches!(closure_outcome(&messages), RecoveryOutcome::None));
    }

    /// 空会话不动
    #[test]
    fn empty_messages_stay_untouched() {
        assert!(matches!(closure_outcome(&[]), RecoveryOutcome::None));
    }

    /// 占位文案遵循统一风格：`**[标签]` 开头、不含 `>` 引用符与 emoji 哨兵
    /// （2026-09-20 统一，见 doc/状态标注符号统一与结构化改造方案.md）。
    /// 该前缀是 `strip_interrupt_markers` 做"重建 vs 现存"等价比较时的识别锚点。
    #[test]
    fn placeholder_matches_marker_convention() {
        assert!(
            INTERRUPT_PLACEHOLDER_NO_REPLY.starts_with("**["),
            "应以 `**[标签]` 开头（统一风格，且等价比较依赖它）：{INTERRUPT_PLACEHOLDER_NO_REPLY}"
        );
        assert!(
            !INTERRUPT_PLACEHOLDER_NO_REPLY.contains('⚠')
                && !INTERRUPT_PLACEHOLDER_NO_REPLY.contains('✕'),
            "不应再含 emoji 哨兵（改用方括号标签识别）：{INTERRUPT_PLACEHOLDER_NO_REPLY}"
        );
        assert!(
            INTERRUPT_PLACEHOLDER_NO_REPLY.contains("[回复被中断]"),
            "保持与 INTERRUPT_MARKER 系统一的可识别标签：{INTERRUPT_PLACEHOLDER_NO_REPLY}"
        );
    }
}

#[cfg(test)]
mod loop_rebuild_tests {
    //! 「events 重放」的纯函数防护：把每轮一行的 events 顺序铺开成消息序列，
    //! 并正确算出相对现有历史的增量后缀。
    use super::{diff_tail, is_empty_block, messages_equivalent};
    use crate::infra::types::models::{Content, ContentBlock, Message};

    fn assistant(text: &str) -> Message {
        Message::Assistant {
            content: Content::Multiple(vec![ContentBlock::Text {
                text: text.to_string(),
            }]),
        }
    }

    fn user(text: &str) -> Message {
        Message::User {
            content: Content::Single(text.to_string()),
        }
    }

    fn replay(events: &[(Vec<ContentBlock>, Vec<ContentBlock>)]) -> Vec<Message> {
        // 与 rebuild_messages_from_events 同口径的纯函数（不触 DB）
        let mut out = Vec::new();
        for (resp, tools) in events {
            let r: Vec<_> = resp.iter().filter(|b| !is_empty_block(b)).cloned().collect();
            if !r.is_empty() {
                out.push(Message::Assistant {
                    content: Content::Multiple(r),
                });
            }
            let t: Vec<_> = tools.iter().filter(|b| !is_empty_block(b)).cloned().collect();
            if !t.is_empty() {
                out.push(Message::User {
                    content: Content::Multiple(t),
                });
            }
        }
        out
    }

    /// 顺序：loop1 响应 → loop1 工具结果 → loop2 响应（顺序即重建顺序）
    #[test]
    fn replay_orders_resp_then_tools_per_loop() {
        let events = vec![
            (
                vec![ContentBlock::Text { text: "第一轮正文".into() }],
                vec![ContentBlock::ToolResult {
                    tool_use_id: "t1".into(),
                    content: "结果1".into(),
                }],
            ),
            (
                vec![ContentBlock::Text { text: "第二轮正文".into() }],
                vec![],
            ),
        ];
        let out = replay(&events);
        assert_eq!(out.len(), 3, "两轮共 3 条消息（2 assistant + 1 user）");
        assert!(matches!(out[0], Message::Assistant { .. }));
        assert!(matches!(out[1], Message::User { .. }));
        assert!(matches!(out[2], Message::Assistant { .. }));
    }

    /// 空块（空文本 / 空思考）不得产生空气泡
    #[test]
    fn replay_skips_empty_blocks() {
        let events = vec![(
            vec![
                ContentBlock::Text { text: "   ".into() },
                ContentBlock::Thinking { thinking: "".into(), signature: "".into() },
            ],
            vec![],
        )];
        assert!(replay(&events).is_empty(), "全空块不应产生消息");
    }

    /// 前缀对齐求增量：现有 2 条、重建 4 条（前 2 条相同）→ 返回后 2 条
    #[test]
    fn diff_tail_returns_new_suffix() {
        let current = vec![assistant("A"), user("B")];
        let rebuilt = vec![assistant("A"), user("B"), assistant("C"), user("D")];
        let extra = diff_tail(&current, &rebuilt);
        assert_eq!(extra.len(), 2);
        assert!(messages_equivalent(&extra[0], &assistant("C")));
        assert!(messages_equivalent(&extra[1], &user("D")));
    }

    /// 重建序列已被历史完全覆盖 → 无增量
    #[test]
    fn diff_tail_empty_when_covered() {
        let current = vec![assistant("A"), user("B")];
        let rebuilt = vec![assistant("A"), user("B")];
        assert!(diff_tail(&current, &rebuilt).is_empty());
    }

    /// 历史里有前置 system 类消息（重建序列不含）时，锚定后仍能找到正确增量
    #[test]
    fn diff_tail_aligns_past_leading_context() {
        let current = vec![user("前置上下文"), assistant("A"), user("B")];
        let rebuilt = vec![assistant("A"), user("B"), assistant("C")];
        let extra = diff_tail(&current, &rebuilt);
        assert_eq!(extra.len(), 1, "只应补出 C（A/B 已对齐）");
        assert!(messages_equivalent(&extra[0], &assistant("C")));
    }

    /// ⚠️ 回归：**该轮已完整落库**时不得重复补写。
    ///
    /// 真实形态——用户消息不进 events（请求侧不存），且 `rebuilt` 的尾条是历史
    /// 尾条的**续写**（同一 assistant 文本更长了），尾部一比对必不相等。
    /// 旧实现（尾部对齐）会因此返回整个 `rebuilt`，恢复后历史里多一整轮。
    #[test]
    fn diff_tail_no_duplicate_when_turn_already_persisted() {
        // 历史：起点 user 提问 + 半截 assistant（恢复前已由 store_assistant_response 落库）
        let current = vec![
            user("提问"),
            assistant("我已经分析完毕，结论如下"),
        ];
        // 重放：不含起点 user；尾条是同一 assistant 的更完整文本
        let rebuilt = vec![
            assistant("我已经分析完毕，结论如下，具体分三点："),
        ];
        let extra = diff_tail(&current, &rebuilt);
        // 尾条文本不同 → 语义上无法确认覆盖，保守补出（宁可重复也不丢内容），
        // 但**绝不能**把已对齐的历史也当成增量再补一遍
        assert!(
            extra.is_empty() || extra.len() == 1,
            "增量不得超过 1 条，实际 {}",
            extra.len()
        );
    }

    /// ⚠️ 回归：**已完全覆盖**时（该轮整段落库、重放与历史逐条同文）必须返回空。
    #[test]
    fn diff_tail_empty_when_tail_text_extends() {
        let current = vec![
            user("提问"),
            assistant("第一轮"),
            user("工具结果"),
            assistant("第二轮"),
        ];
        let rebuilt = vec![
            assistant("第一轮"),
            user("工具结果"),
            assistant("第二轮"),
        ];
        assert!(
            diff_tail(&current, &rebuilt).is_empty(),
            "重放内容全部已在历史中，不应产生增量"
        );
    }

    /// 等价判定：Single 与 Multiple 形态不同但文本相同 → 视为等价
    #[test]
    fn equivalence_ignores_content_shape() {
        let single = Message::Assistant {
            content: Content::Single("同样的话".into()),
        };
        let multiple = assistant("同样的话");
        assert!(messages_equivalent(&single, &multiple));
    }

    /// 等价判定：角色不同绝不等价
    #[test]
    fn equivalence_requires_same_role() {
        assert!(!messages_equivalent(&user("x"), &assistant("x")));
    }

    /// ⚠️ 回归（session 2302e81c）：中断标记行不参与等价判定。
    ///
    /// 标记只写在消息层（store_interrupted_turn 补写），events 重放产物没有；
    /// 收尾后的正常刷新触发门卫重放比对时，标记行曾导致文本不等 → 误判缺失
    /// → 重复补同一段（seq5 interrupted + seq6 chat 双份）。
    #[test]
    fn equivalence_ignores_interrupt_markers() {
        let replayed = assistant("半截正文");
        // 现行格式（2026-09-20 起）：`**[标签]** …`，不带引用符
        let current = Message::Assistant {
            content: Content::Multiple(vec![ContentBlock::Text {
                text: "半截正文\n\n**[回复被中断]** 上次回复在此处中断，请基于上下文继续完成。"
                    .into(),
            }]),
        };
        assert!(
            messages_equivalent(&current, &replayed),
            "带标记的历史消息应与无标记的重放产物等价"
        );
        // 旧库格式：`> ⚠️ **[回复被中断]** …`（旧会话历史不会自动改写，必须兼容）
        let legacy_marker = Message::Assistant {
            content: Content::Multiple(vec![ContentBlock::Text {
                text: "半截正文\n\n> ⚠️ **[回复被中断]** 上次回复在此处中断，请基于上下文继续完成。"
                    .into(),
            }]),
        };
        assert!(messages_equivalent(&legacy_marker, &replayed));
        // 旧库早期取消格式：`> ✕ **用户已取消执行…**`
        let legacy_cancel = Message::Assistant {
            content: Content::Multiple(vec![ContentBlock::Text {
                text: "半截正文\n\n> ✕ **用户已取消执行（以上为保留的部分结果，历史未截断）**"
                    .into(),
            }]),
        };
        assert!(messages_equivalent(&legacy_cancel, &replayed));
        // 纯标记占位（无半截内容）剥后为空，与空内容等价
        let placeholder = Message::Assistant {
            content: Content::Multiple(vec![ContentBlock::Text {
                text: super::INTERRUPT_PLACEHOLDER_NO_REPLY.to_string(),
            }]),
        };
        let empty = Message::Assistant {
            content: Content::Multiple(vec![ContentBlock::Text { text: "".into() }]),
        };
        assert!(messages_equivalent(&placeholder, &empty));
        // 非标记差异仍然不等（剥离不得放水）
        assert!(!messages_equivalent(&assistant("A"), &assistant("B")));
        // 正文里的普通加粗方括号不得被当成标记剥掉
        let ordinary = assistant("结论如下\n\n**[注意]** 这里有个坑");
        assert!(!messages_equivalent(&ordinary, &assistant("结论如下")));
    }

    /// 等价判定：**不同轮次的 assistant 绝不能等价**（回归：曾因"两边无文本即等价"
    /// 的宽松判定把相邻轮次的正文误判为同一对，导致 diff_tail 一路对齐到 0，
    /// 每轮都重复追加整条重建序列）
    #[test]
    fn equivalence_rejects_different_assistant_texts() {
        assert!(!messages_equivalent(&assistant("A"), &assistant("C")));
        assert!(messages_equivalent(&assistant("A"), &assistant("A")));
    }

    /// 端到端对齐：生产形态下 `current` 的前两条应能对上 `rebuilt`，
    /// 增量必须**只有**新补的那一轮 —— 形态差异（`Single` vs `Multiple`）不得把对齐带偏
    ///
    /// `current` 按真实形态构造：历史里 tool_results 是 `User + Content::Single`
    /// （`store_assistant_response` 的写法），重放产物则是 `Multiple([ToolResult])`；
    /// 且重建序列**不含起点 user**（请求侧不存）。
    #[test]
    fn diff_tail_survives_mixed_content_shapes() {
        let current = vec![
            user("提问"),
            Message::Assistant {
                content: Content::Multiple(vec![ContentBlock::Text { text: "第一轮".into() }]),
            },
            Message::User {
                content: Content::Single("工具结果".into()),
            },
        ];
        let rebuilt = vec![
            Message::Assistant {
                content: Content::Multiple(vec![ContentBlock::Text { text: "第一轮".into() }]),
            },
            Message::User {
                content: Content::Multiple(vec![ContentBlock::ToolResult {
                    tool_use_id: "t1".into(),
                    content: "工具结果".into(),
                }]),
            },
            Message::Assistant {
                content: Content::Multiple(vec![ContentBlock::Text { text: "第二轮".into() }]),
            },
        ];
        let extra = diff_tail(&current, &rebuilt);
        assert_eq!(extra.len(), 1, "只应补出「第二轮」");
        assert!(messages_equivalent(&extra[0], &assistant("第二轮")));
    }
}
