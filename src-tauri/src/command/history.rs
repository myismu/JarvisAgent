//! # history.rs — 会话历史读取 Tauri 命令
//!
//! 把会话消息读成**结构化的 JSON**（`SessionMessage[]` + `AgentTurnSnapshot`），
//! 交给前端 Vue 组件渲染；并关联检查点信息以支持消息撤回按钮。
//!
//! ## 关键导出
//! - `get_session_messages()`: 全量结构化消息
//! - `get_session_messages_paged()`: 按轮懒加载
//!
//! ## 历史演进：HTML 渲染通道已删除（2026-10-07）
//!
//! 这里原本还有一条 `get_session_history()`：把消息**拼成 HTML 字符串**返回，
//! 前端存进 `jarvisResponse` 字段。它早已不被渲染（`ChatArea` 渲染的是
//! 结构化 `messages`），只剩 8 处 `catch` 兜底在调用它；而它拼出的 HTML 里
//! 还写死了中文（思考折叠标题 / token 消耗行 / 图片 alt）。
//! 整条通道连同前端的 `jarvisResponse` 字段一并删除 —— 兜底改为
//! "重试结构化接口 + 显示可翻译的错误提示"。
//!
//! ## 约束
//! - 过滤不可见来源（按 `MessageSource::rendered_in_ui()`）
//! - 助手多轮回复合并显示；思考过程由前端折叠
//! - 用户消息关联检查点 ID，支持前端回滚按钮

use crate::core::session;
use crate::infra::state::state::*;
use crate::infra::types::models::*;
use std::collections::HashMap;

#[derive(Clone)]
struct RollbackInfo {
    checkpoint_id: String,
    has_file_edits: bool,
    created_at: u64,
}

/// 一条**可渲染用户消息**的附加信息（编号 / 撤回入口）。
///
/// ⚠️ 它**不装正文** —— 正文在渲染循环里由 `user_display_content` 现算。
/// 此前它还带 `display` 与 `memory_index` 两个字段，只有已删除的 HTML 渲染器
/// （`render_user_message`）在读，故一并去掉。
struct UserDisplayMessage {
    message_id: Option<String>,
    seq: Option<usize>,
    rollback_info: Option<RollbackInfo>,
}

struct RollbackLookups {
    by_index: Vec<(usize, RollbackInfo)>,
    by_message_id: HashMap<String, RollbackInfo>,
}

use serde::Serialize;

// ═══════════════════════════════════════════════════════════════
//  新增：结构化消息类型（供前端 Vue 组件渲染）
// ═══════════════════════════════════════════════════════════════

#[derive(Serialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct SessionMessage {
    pub role: String,
    pub id: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub snapshot: Option<AgentTurnSnapshot>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub snapshot_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub message_id: Option<String>,
    // 用户消息的原始 HTML 内容
    #[serde(skip_serializing_if = "Option::is_none")]
    pub user_content: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub rollback_checkpoint_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub rollback_mode: Option<String>,
}

#[derive(Serialize, Clone, Default)]
#[serde(rename_all = "camelCase")]
pub struct AgentTurnTokens {
    input: u64,
    output: u64,
}

#[derive(Serialize, Clone, Default)]
#[serde(rename_all = "camelCase")]
pub struct AgentTurnSnapshot {
    version: u32,
    status: String,
    text_blocks: Vec<AgentTextBlock>,
    thinking_blocks: Vec<AgentThinkingBlock>,
    tool_calls: Vec<AgentToolCallView>,
    logs: Vec<AgentExecutionLog>,
    tokens: Option<AgentTurnTokens>,
    /// 气泡下方的小字说明 —— 只给 **i18n key**，文案由前端出。
    ///
    /// 与 `text_blocks` 的区别是展示位置：notice 渲染在回复气泡**之外**的下方，
    /// 属于状态标注；而 text_blocks 是模型正文，渲染在气泡内。
    /// 中断/取消这类"运行状态"信息一律走 notice，避免混进正文显得突兀。
    ///
    /// ⚠️ 这里曾经是 `notice: Option<String>`、装后端拼好的**中文句子** ——
    /// 界面切英文后中断轮次仍冒中文。现在后端只发 key（如 `notice.userCancel`），
    /// 文案统一由前端语言包提供，与运行期 `JarvisResult::notice_i18n_key` 同源。
    #[serde(skip_serializing_if = "Option::is_none")]
    notice_i18n_key: Option<String>,
    created_at: u64,
}

#[derive(Serialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct AgentTextBlock {
    id: String,
    #[serde(rename = "loop")]
    loop_: u32,
    kind: String,
    content: String,
    status: String,
    timestamp: u64,
}

#[derive(Serialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct AgentThinkingBlock {
    id: String,
    #[serde(rename = "loop")]
    loop_: u32,
    content: String,
    status: String,
    timestamp: u64,
}

#[derive(Serialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct AgentToolCallView {
    id: String,
    #[serde(rename = "loop")]
    loop_: u32,
    name: String,
    status: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    input: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    output: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    error: Option<String>,
    logs: Vec<String>,
    timestamp: u64,
    updated_at: u64,
}

#[derive(Serialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct AgentExecutionLog {
    id: String,
    #[serde(rename = "loop")]
    loop_: u32,
    content: String,
    timestamp: u64,
}

impl AgentTurnSnapshot {
    fn is_empty(&self) -> bool {
        self.text_blocks.is_empty()
            && self.thinking_blocks.is_empty()
            && self.tool_calls.is_empty()
            // notice 也是要展示的内容：只统计正文会让"仅有一条中断说明"的
            // 收尾轮次被判为空而整轮丢弃 —— 实测出现于"中断后没有后续消息"的场景
            // （最后一条就是 interrupted 消息，结果界面什么都不显示）。
            && self
                .notice_i18n_key
                .as_deref()
                .map(|n| n.trim().is_empty())
                .unwrap_or(true)
    }
}

/// 提取用户消息的展示文本。
///
/// 只取真实 Text 块；动态上下文是独立的 Context 块（工作目录 / 项目结构 /
/// 全局记忆），属于发给模型的运行时信息，不进 UI。
fn user_display_content(content: &Content) -> String {
    match content {
        Content::Single(s) => s.trim().to_string(),
        Content::Multiple(blocks) => {
            let mut parts = String::new();
            for block in blocks {
                match block {
                    ContentBlock::Text { text } => {
                        let t = text.trim();
                        if !t.is_empty() {
                            parts.push_str(t);
                            parts.push('\n');
                        }
                    }
                    ContentBlock::Image { source } => {
                        let data = if !source.data.is_empty() {
                            source.data.clone()
                        } else if let Some(ref fp) = source.file_path {
                            session::load_image_data(fp).unwrap_or_default()
                        } else {
                            String::new()
                        };
                        parts.push_str(&format!(
                            "<img src=\"data:{};base64,{}\" style=\"max-width: 200px; max-height: 200px; border-radius: 8px; margin: 4px 4px 4px 0; display: inline-block; vertical-align: middle;\" alt=\"图片\" />",
                            source.media_type, data
                        ));
                        parts.push('\n');
                    }
                    _ => {}
                }
            }
            parts.trim_end().to_string()
        }
    }
}

fn append_assistant_content(
    target: &mut AgentTurnSnapshot,
    content: &Content,
    loop_idx: u32,
    timestamp: u64,
) {
    match content {
        Content::Single(s) => {
            let trimmed = s.trim();
            if !trimmed.is_empty() {
                target.text_blocks.push(AgentTextBlock {
                    id: format!("text_{}", timestamp),
                    loop_: loop_idx,
                    kind: "assistant".to_string(),
                    content: trimmed.to_string(),
                    status: "done".to_string(),
                    timestamp,
                });
            }
        }
        Content::Multiple(blocks) => {
            for (i, block) in blocks.iter().enumerate() {
                let ts = timestamp + i as u64;
                match block {
                    ContentBlock::Text { text } => {
                        let trimmed = text.trim();
                        if !trimmed.is_empty() {
                            target.text_blocks.push(AgentTextBlock {
                                id: format!("text_{}", ts),
                                loop_: loop_idx,
                                kind: "assistant".to_string(),
                                content: trimmed.to_string(),
                                status: "done".to_string(),
                                timestamp: ts,
                            });
                        }
                    }
                    ContentBlock::Thinking { thinking, .. } => {
                        let trimmed = thinking.trim();
                        if !trimmed.is_empty() {
                            target.thinking_blocks.push(AgentThinkingBlock {
                                id: format!("thinking_{}", ts),
                                loop_: loop_idx,
                                content: trimmed.to_string(),
                                status: "done".to_string(),
                                timestamp: ts,
                            });
                        }
                    }
                    ContentBlock::ToolUse { id, name, input } => {
                        let input_json = serde_json::to_string_pretty(input).unwrap_or_default();
                        target.tool_calls.push(AgentToolCallView {
                            id: id.clone(),
                            loop_: loop_idx,
                            name: name.clone(),
                            status: "running".to_string(),
                            input: Some(input_json),
                            output: None,
                            error: None,
                            logs: vec![],
                            timestamp: ts,
                            updated_at: ts,
                        });
                    }
                    _ => {}
                }
            }
        }
    }
}

fn append_tool_result(
    target: &mut AgentTurnSnapshot,
    tool_use_id: &str,
    content: &str,
    timestamp: u64,
) {
    if let Some(tool) = target.tool_calls.iter_mut().find(|t| t.id == tool_use_id) {
        tool.status = "completed".to_string();
        tool.output = Some(content.trim().to_string());
        tool.updated_at = timestamp;
    }
}

fn build_linked_rollbacks(session_id: &str) -> RollbackLookups {
    let mut by_index = Vec::new();
    let mut by_message_id = HashMap::new();
    for link in crate::infra::db::list_checkpoint_user_message_links(session_id)
        .unwrap_or_default()
        .into_iter()
        .filter(|link| link.has_file_edits && !link.checkpoint_id.is_empty())
    {
        let info = RollbackInfo {
            checkpoint_id: link.checkpoint_id,
            has_file_edits: true,
            created_at: link.created_at,
        };
        if let Some(message_id) = link.message_id.filter(|value| !value.trim().is_empty()) {
            by_message_id.insert(message_id, info.clone());
        }
        by_index.push((link.user_message_index, info));
    }
    by_index.sort_by_key(|(trigger_index, info)| (*trigger_index, info.created_at));
    RollbackLookups {
        by_index,
        by_message_id,
    }
}

fn parse_snapshot_usize(snapshot: &crate::core::rollback::Snapshot, key: &str) -> Option<usize> {
    snapshot
        .metadata
        .get(key)
        .and_then(|value| value.parse::<usize>().ok())
}

fn parse_snapshot_string(snapshot: &crate::core::rollback::Snapshot, key: &str) -> Option<String> {
    snapshot
        .metadata
        .get(key)
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty())
}

fn find_rollback_info(
    linked_rollbacks: &RollbackLookups,
    metadata_rollbacks_by_index: &[(usize, u64, String)],
    metadata_rollbacks_by_message_id: &HashMap<String, RollbackInfo>,
    memory_index: usize,
    message_id: Option<&str>,
) -> Option<RollbackInfo> {
    if let Some(message_id) = message_id {
        if let Some(info) = linked_rollbacks
            .by_message_id
            .get(message_id)
            .or_else(|| metadata_rollbacks_by_message_id.get(message_id))
        {
            return Some(info.clone());
        }
    }

    let linked = linked_rollbacks
        .by_index
        .iter()
        .filter(|(trigger_index, _)| *trigger_index >= memory_index)
        .min_by_key(|(trigger_index, info)| (*trigger_index, info.created_at))
        .map(|(_, info)| info.clone());
    let metadata = metadata_rollbacks_by_index
        .iter()
        .filter(|(trigger_index, _, _)| *trigger_index >= memory_index)
        .min_by_key(|(trigger_index, created_at, _)| (*trigger_index, *created_at))
        .map(|(_, created_at, id)| RollbackInfo {
            checkpoint_id: id.clone(),
            has_file_edits: true,
            created_at: *created_at,
        });

    match (linked, metadata) {
        (Some(linked), Some(metadata)) if metadata.created_at < linked.created_at => Some(metadata),
        (Some(linked), _) => Some(linked),
        (None, Some(metadata)) => Some(metadata),
        (None, None) => None,
    }
}

/// 判断该 source 是否属于"中断收尾消息"。
///
/// ⚠️ 别据此断言它"不是模型正文" —— 新数据的正文块里装的恰恰是半截内容，
/// 该不该渲染见 [`interrupted_body_is_content`]。
fn is_interrupted_source(source: MessageSource) -> bool {
    source == MessageSource::Interrupted
}

/// `interrupted` 消息的正文块是否应当渲染进 `text_blocks`。
///
/// 同一条 `source = "interrupted"` 的消息承载两种截然不同的东西，只有 kind 能区分：
/// - **kind 有值**（新数据）：正文块里是**半截内容**（`pipeline.rs` 中断收尾的分支二
///   写明"正文即半截内容；正文块恒存在"），必须渲染 —— 否则用户取消后只剩一句
///   "用户已取消执行"，已经生成的部分凭空消失（实测 bug）；
/// - **kind 缺失**（v23 之前的老数据）：正文是 `**[回复被中断]** …` 这类标记文本，
///   已由 notice 承载，再渲染会把它当模型正文重复显示一遍。
///
/// v23 迁移已把老数据的 `interrupt_kind` 一次性回填（从那些标记文本里认出来的），
/// 所以"kind 缺失"现在只剩**认不出的极少数**（正文被人手改过、或标记格式前所未见）。
/// 那些轮次会被整轮跳过 —— 这是刻意的：宁可少显示，也不把系统标记当模型正文渲染。
fn interrupted_body_is_content(kind: Option<&str>) -> bool {
    kind.is_some()
}

/// 中断类型 → 前端 i18n key（文案在前端语言包的 `notice.*` 下）。
///
/// ## 为什么这里只出 key
///
/// 这里曾经是 `interrupt_notice_for()`，直接返回**中文句子**。后果是界面切成英文后，
/// 中断轮次的小字仍然是中文 —— 后端产出的用户可见文案绕开了前端语言包。
/// 现在后端只说"中断类型"，文案由前端出。
///
/// ## 与运行期必须同源
///
/// 运行期（`pipeline.rs` 的三处收尾）也会设一次 notice，用户先看到它，
/// 刷新后由本函数按 `interrupt_kind` 重新生成。**两处必须给出同一个 key** ——
/// 措辞若不一致，用户会看到"刷新前后说法变了"。
/// 旧实现正是两套文案并存（`handle_cancellation` 里还内联了一份
/// "用户已取消执行…"的副本），本次一并收敛到这里。
///
/// ## 旧库数据
///
/// 这里曾经还有一条 `interrupted_notice_text()` 回退：从正文里**清洗出**
/// 旧格式标记文本（`> ⚠️ **[回复被中断]** …`）当小字。那是"从内容反推元信息"，
/// 已在 v23 迁移里把老数据的 `interrupt_kind` 一次性回填，该回退随之删除。
fn interrupt_notice_key(kind: Option<&str>) -> Option<&'static str> {
    use crate::infra::types::models::InterruptKind;
    Some(match InterruptKind::from_db(kind?)? {
        InterruptKind::StreamTimeout => "notice.streamTimeout",
        InterruptKind::UserCancel => "notice.userCancel",
        InterruptKind::PipelineError => "notice.pipelineError",
        InterruptKind::AppClosed => "notice.appClosed",
        InterruptKind::LoopLimit => "notice.loopLimit",
        InterruptKind::PlanLimit => "notice.planLimit",
    })
}

/// 该来源的消息是否参与会话历史的界面渲染。
///
/// 判据直接取 `MessageSource::rendered_in_ui()`（白名单是 `Chat | Interrupted`），
/// 不再维护字符串名单 —— 曾经这里是一份独立的 `matches!(source, "chat" | "interrupted")`，
/// 与模型侧白名单（`pipeline.rs`）、压缩侧名单（`memory.rs`）各写各的，
/// 任一处漏改都是**静默**的消息消失或泄漏。见
/// `doc/消息来源与轮次元数据重构方案.md` §2.1。
///
/// - `Chat`：常规对话（用户输入 / 助手回复）；
/// - `Interrupted`：运行被打断（用户取消 / 上游失联 / 执行报错）时的收尾消息。
///   必须渲染，否则中断轮次会从界面消失，用户会以为「根本没执行」。
fn is_renderable_source(source: MessageSource) -> bool {
    source.rendered_in_ui()
}

// ═══════════════════════════════════════════════════════════════
//  新增：get_session_messages — 返回结构化 JSON 消息列表
// ═══════════════════════════════════════════════════════════════

/// 提取消息列表的公共逻辑（全量与分页两条命令共用）
async fn extract_session_messages(
    session_id: &str,
    session_manager: &SessionManager,
    registry: &SnapshotRegistry,
) -> Result<Vec<SessionMessage>, String> {
    // 全量语义：窗口不设限（分页元数据被丢弃）
    let (messages, _, _) =
        extract_session_messages_window(session_id, session_manager, registry, None, None).await?;
    Ok(messages)
}

/// 分页元数据
#[derive(Serialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct PagedSessionMessages {
    pub messages: Vec<SessionMessage>,
    /// 是否还有更早的历史可加载（向上翻页游标）
    pub has_more: bool,
    /// 本页最老消息的 seq（下次请求的游标）；页为空时为 None
    pub oldest_seq: Option<i64>,
}

/// 懒加载核心：从 events 恢复后，按窗口取消息并走与全量完全相同的加工逻辑
/// （turn 聚合 / checkpoint 关联 / snapshot 组装），只是消息来源换成
/// `load_visible_turns_page` 的窗口结果。
///
/// - `before_seq = None`：首屏（最新一页）；`Some(seq)`：以 seq 为游标向更早取
/// - `limit = None`：默认 50
async fn extract_session_messages_window(
    session_id: &str,
    session_manager: &SessionManager,
    registry: &SnapshotRegistry,
    before_seq: Option<i64>,
    max_turns: Option<usize>,
) -> Result<(Vec<SessionMessage>, bool, Option<i64>), String> {
    // max_turns = None 表示全量语义（不分页）；Some(n) 表示按轮凑页（n = 每页轮数，
    // 默认 5：2K 屏一屏约 2-3 轮，每页约 2 屏内容，翻页节奏适中）
    let paged = max_turns.is_some();
    let max_turns = max_turns.unwrap_or(5).max(1);
    let ctx = session_manager.get_or_create(session_id).await;
    // 注意：**不要**在这里把内存 flush 到 DB。
    //
    // 曾经的实现是"先把 ctx.memory 存库，再读回来"，注释说是"确保读到最新状态"，
    // 但实际效果相反：后端进程常驻，内存里可能残留**已被删除**的消息，
    // 这次 flush 会把它们全部复活 —— 实测表现为"手动删掉某条消息后 F5，
    // 它立刻回到数据库"，看起来阴魂不散。
    //
    // 本函数语义是**读取**，应以数据库为准；需要持久化的写入点各自负责落库。
    let mut memory = session::load_session(session_id)?;

    // 中断恢复：唯一闸门 `ensure_session_recovered`（含原子抢占锁 + 先落库后盖章）。
    //
    // 该闸门内含"半截内容是否已存在"的去重守卫，而它的判定需要处理
    // 「正文 / 中断标记分体存储」与「正文+标记合并」两种形态。
    // 曾在此内联复制过一份无去重的实现，导致**每加载一次就多一条合并副本**
    // （实测"删掉再刷新，副本又回来"，且因未走到守卫分支连日志都不打印）。
    let active_run_id = ctx.active_run_id.lock().await.clone();
    let _ = crate::command::session::ensure_session_recovered(
        session_id,
        &mut memory,
        active_run_id.as_deref(),
    );
    *ctx.memory.lock().await = memory.clone();

    if memory.messages.is_empty() && session::session_messages_count(session_id).unwrap_or(0) == 0 {
        // 空会话：窗口版返回空页 + 无更早数据
        return Ok((Vec::new(), false, None));
    }

    let linked_rollbacks = build_linked_rollbacks(session_id);
    let metadata_rollbacks_by_index;
    let metadata_rollbacks_by_message_id;
    {
        let manager = registry.0.read().await.get_or_create(session_id).await?;
        let mut by_index = Vec::new();
        let mut by_message_id = HashMap::new();
        for snapshot in manager
            .list_snapshots(None)
            .await
            .into_iter()
            .filter(|snapshot| snapshot.is_checkpoint)
        {
            let patch_count = parse_snapshot_usize(&snapshot, "patch_count").unwrap_or(0);
            if patch_count == 0 {
                continue;
            }
            let info = RollbackInfo {
                checkpoint_id: snapshot.id.clone(),
                has_file_edits: true,
                created_at: snapshot.created_at,
            };
            if let Some(message_id) = parse_snapshot_string(&snapshot, "trigger_user_message_id") {
                by_message_id.insert(message_id, info);
            }
            if let Some(trigger_index) =
                parse_snapshot_usize(&snapshot, "trigger_user_memory_index")
            {
                by_index.push((trigger_index, snapshot.created_at, snapshot.id));
            }
        }
        by_index.sort_by_key(|(trigger_index, created_at, _)| (*trigger_index, *created_at));
        metadata_rollbacks_by_index = by_index;
        metadata_rollbacks_by_message_id = by_message_id;
    };

    // 消息来源二选一：
    // - 按轮凑页：每页 max_turns 个完整轮（长任务轮密度自适应）
    // - 全量：原逻辑不变（max_turns = None）
    let (stored_messages, has_more) = if paged {
        let (page, more) = session::load_visible_turns_page(session_id, before_seq, max_turns)?;
        (page, Some(more))
    } else {
        (session::list_visible_session_messages(session_id)?, None)
    };
    let render_messages: Vec<_> = if stored_messages.is_empty() {
        memory
            .messages
            .iter()
            .enumerate()
            .map(|(idx, message)| {
                let source = memory
                    .sources
                    .get(idx)
                    .copied()
                    .unwrap_or(MessageSource::Chat);
                let kind = memory.interrupt_kinds.get(idx).cloned().flatten();
                (
                    idx,
                    memory.message_ids.get(idx).cloned(),
                    None,
                    message.clone(),
                    source,
                    kind,
                )
            })
            .collect()
    } else {
        stored_messages
            .into_iter()
            .enumerate()
            .map(|(idx, stored)| {
                (
                    idx,
                    Some(stored.message_id),
                    Some(stored.seq),
                    stored.content,
                    stored.source,
                    stored.interrupt_kind,
                )
            })
            .collect()
    };

    // 窗口最老消息的 seq（分页游标）；memory 回退分支无 seq，返回 None
    let oldest_seq = render_messages
        .first()
        .and_then(|(_, _, seq, _, _, _)| seq.map(|s| s as i64));

    let display_messages = render_messages
        .iter()
        .filter_map(|(memory_index, message_id, seq, msg, source, _kind)| {
            if let Message::User { content } = msg {
                let display = user_display_content(content);
                if is_renderable_source(*source) && !display.trim().is_empty() {
                    return Some(UserDisplayMessage {
                        message_id: message_id.clone(),
                        seq: *seq,
                        rollback_info: find_rollback_info(
                            &linked_rollbacks,
                            &metadata_rollbacks_by_index,
                            &metadata_rollbacks_by_message_id,
                            *memory_index,
                            message_id.as_deref(),
                        ),
                    });
                }
            }
            None
        })
        .collect::<Vec<_>>();

    let mut result = Vec::new();
    let mut pending_assistant = AgentTurnSnapshot::default();
    let mut visible_user_index = 0usize;
    let mut loop_idx = 1;
    let mut current_ts = 1000u64;
    // 当前正在处理的消息 seq（懒加载分页下用 seq 生成**跨页稳定唯一**的 id——
    // 旧的 result.len() 位置编号在每页都从 0 开始，头部插入后 key 大量重复，
    // Vue diff 整表重建导致翻页时界面闪烁）
    let mut last_seen_seq: Option<usize> = None;

    for (_, _, seq, msg, source, kind) in &render_messages {
        current_ts += 1;
        last_seen_seq = *seq;
        match msg {
            Message::User { content } => {
                let display = user_display_content(content);

                if let Content::Multiple(blocks) = content {
                    for block in blocks {
                        if let ContentBlock::ToolResult {
                            tool_use_id,
                            content: res_content,
                        } = block
                        {
                            append_tool_result(
                                &mut pending_assistant,
                                tool_use_id,
                                res_content,
                                current_ts,
                            );
                        }
                    }
                }

                if !is_renderable_source(*source) || display.trim().is_empty() {
                    continue;
                }
                let Some(message) = display_messages.get(visible_user_index) else {
                    continue;
                };

                // 先 finalize 之前的 assistant 消息
                if !pending_assistant.is_empty() {
                    pending_assistant.status = "FINISH".to_string();
                    pending_assistant.version = 1;
                    if pending_assistant.created_at == 0 {
                        pending_assistant.created_at = current_ts;
                    }
                    result.push(SessionMessage {
                        role: "agent".to_string(),
                        // seq 稳定唯一：分页/全量、跨页都不会撞 key
                        id: format!(
                            "agent_{}",
                            last_seen_seq
                                .map(|s| s.to_string())
                                .unwrap_or_else(|| format!("m{}", result.len()))
                        ),
                        snapshot: Some(pending_assistant.clone()),
                        snapshot_id: None,
                        message_id: None,
                        user_content: None,
                        rollback_checkpoint_id: None,
                        rollback_mode: None,
                    });
                    pending_assistant = AgentTurnSnapshot::default();
                }

                let rollback_mode = if message
                    .rollback_info
                    .as_ref()
                    .map(|info| info.has_file_edits)
                    .unwrap_or(false)
                {
                    Some("both".to_string())
                } else {
                    Some("session".to_string())
                };
                let rollback_checkpoint_id = message
                    .rollback_info
                    .as_ref()
                    .map(|info| info.checkpoint_id.clone());

                result.push(SessionMessage {
                    role: "user".to_string(),
                    id: format!(
                        "user_{}",
                        message
                            .seq
                            .map(|s| s.to_string())
                            .unwrap_or_else(|| format!("m{}", result.len()))
                    ),
                    snapshot: None,
                    snapshot_id: None,
                    message_id: message.message_id.clone(),
                    user_content: Some(display),
                    rollback_checkpoint_id,
                    rollback_mode,
                });

                visible_user_index += 1;
                loop_idx = 1;
            }
            Message::Assistant { content } => {
                if !is_renderable_source(*source) {
                    continue;
                }
                // `interrupted` 消息承载两种东西，按 kind 是否存在区分 —— 这里曾经搞反过：
                // 渲染侧按"它不是模型正文"整条丢弃，而落库侧（`pipeline.rs` 中断收尾的
                // 分支二）写的恰恰是"正文即半截内容；正文块恒存在"，于是用户取消后
                // 只剩一句状态说明，已经生成的部分凭空消失。
                // - kind 有值（新数据）：正文块里是**半截内容**，必须一并渲染；
                // - kind 缺失（旧库数据）：正文是 `**[回复被中断]** …` 这类标记文本，
                //   已由上面的 notice 承载，再 append 会把它当模型正文重复显示一遍。
                // notice 本身仍渲染在气泡**下方**的小字里，不挤进回复气泡内部。
                if is_interrupted_source(*source) {
                    // 阶段二：notice 优先按**结构化 kind** 生成（不再从正文猜）；
                    // kind 缺失（旧库数据 / 恢复重建未打标）才回退文本清洗路径。
                    if let Some(key) = interrupt_notice_key(kind.as_deref()) {
                        pending_assistant.notice_i18n_key = Some(key.to_string());
                    }
                    // 只有旧库的标记文本拦在这里；新数据的半截正文继续往下走去 append
                    if !interrupted_body_is_content(kind.as_deref()) {
                        continue;
                    }
                }
                append_assistant_content(&mut pending_assistant, content, loop_idx, current_ts);
                loop_idx += 1;
            }
        }
    }

    // finalize 最后一条 assistant 消息
    if !pending_assistant.is_empty() {
        pending_assistant.status = "FINISH".to_string();
        pending_assistant.version = 1;
        if pending_assistant.created_at == 0 {
            pending_assistant.created_at = current_ts;
        }
        result.push(SessionMessage {
            role: "agent".to_string(),
            id: format!(
                "agent_{}",
                last_seen_seq
                    .map(|s| s.to_string())
                    .unwrap_or_else(|| format!("m{}", result.len()))
            ),
            snapshot: Some(pending_assistant),
            snapshot_id: None,
            message_id: None,
            user_content: None,
            rollback_checkpoint_id: None,
            rollback_mode: None,
        });
    }

    Ok((result, has_more.unwrap_or(false), oldest_seq))
}

#[tauri::command]
pub async fn get_session_messages(
    session_id: String,
    session_manager: tauri::State<'_, SessionManager>,
    registry: tauri::State<'_, SnapshotRegistry>,
) -> Result<Vec<SessionMessage>, String> {
    // 重建时机前置：把 interrupted+resumable 的半截内容从 agent_run_events
    // 重放并落进 session_messages，使崩溃/中断后**仅查看会话**（无需发消息）
    // 即可看到完整对话。此前恢复只在发消息时触发（chat.ts 发送流程开头），
    // 重启后界面会一直缺半截内容，直到用户下一次发消息。
    //
    // 幂等性：走唯一闸门 `ensure_session_recovered`（原子抢占 + 先落库后盖章），
    // 无遗留 run 时零开销快速路径，高频调用无额外负担。
    let recovered = {
        let ctx = session_manager.get_or_create(&session_id).await;
        let active_run_id = ctx.active_run_id.lock().await.clone();
        let mut memory = ctx.memory.lock().await;
        crate::command::session::ensure_session_recovered(
            &session_id,
            &mut memory,
            active_run_id.as_deref(),
        )
    };
    if recovered {
        // 恢复出的增量已由闸门内统一落库；此处仅提示日志口径与恢复命令一致
        println!(
            "[JARVIS] get_session_messages：中断恢复已补回消息并落库（session {}）",
            session_id
        );
    }
    extract_session_messages(&session_id, &session_manager, &registry).await
}

/// 懒加载分页命令：**按轮凑页**——一轮 = 一条可渲染 user 消息 + 其后所有消息
/// （assistant / tool_result 等，直到下一条轮起点）。长任务一轮几十条消息时，
/// 页大小随轮密度自适应，永远不会出现半轮。
///
/// - 首屏（`beforeSeq = None`）：最新的 `maxTurns` 个完整轮；
/// - 上滑翻页（`beforeSeq = 游标`）：更早的 `maxTurns` 个完整轮；
/// - 游标 = 本页最老消息的 seq（`oldestSeq`）。
#[tauri::command]
pub async fn get_session_messages_paged(
    session_id: String,
    before_seq: Option<i64>,
    limit: Option<usize>,
    session_manager: tauri::State<'_, SessionManager>,
    registry: tauri::State<'_, SnapshotRegistry>,
) -> Result<PagedSessionMessages, String> {
    // 中断恢复前置（唯一闸门 `ensure_session_recovered`，原子抢占 + 先落库后盖章）：
    // 恢复落库的是尾部消息，天然包含在首屏窗口内。
    // 下方 extract_session_messages_window 内部还有一次同闸门调用——
    // 首次已抢占成功并盖章，此处第二次只会走"无遗留 run"快速路径，幂等无害。
    let recovered = {
        let ctx = session_manager.get_or_create(&session_id).await;
        let active_run_id = ctx.active_run_id.lock().await.clone();
        let mut memory = ctx.memory.lock().await;
        crate::command::session::ensure_session_recovered(
            &session_id,
            &mut memory,
            active_run_id.as_deref(),
        )
    };
    if recovered {
        println!(
            "[JARVIS] get_session_messages_paged：中断恢复已补回消息并落库（session {}）",
            session_id
        );
    }
    // limit 在此命令中语义为 **每页轮数**（默认 5 轮）
    let (messages, has_more, oldest_seq) = extract_session_messages_window(
        &session_id,
        &session_manager,
        &registry,
        before_seq,
        limit,
    )
    .await?;
    Ok(PagedSessionMessages {
        messages,
        has_more,
        oldest_seq,
    })
}

#[cfg(test)]
mod renderable_source_tests {
    use super::is_renderable_source;
    use crate::infra::types::models::MessageSource;

    /// 常规对话必须渲染
    #[test]
    fn chat_is_renderable() {
        assert!(is_renderable_source(MessageSource::Chat));
    }

    /// 回归防护：中断收尾消息必须渲染。
    /// 否则用户取消/上游失联后，中断轮次会从界面整体消失，
    /// 用户会误以为「根本没执行」——这正是本次要修掉的问题。
    #[test]
    fn interrupted_is_renderable() {
        assert!(is_renderable_source(MessageSource::Interrupted));
    }

    /// 其余来源不得泄漏到界面。
    /// 这里逐个列出（而不是遍历枚举）是为了与上一个版本保持同样的断言语义：
    /// 新增取值时本测试不会自动通过 —— 必须先想清楚它该不该渲染。
    #[test]
    fn other_sources_stay_hidden() {
        for src in [
            MessageSource::Compact,
            MessageSource::Inject,
            MessageSource::Placeholder,
        ] {
            assert!(!is_renderable_source(src), "{:?} 不应参与界面渲染", src);
        }
    }
}

#[cfg(test)]
mod notice_not_empty_tests {
    //! **实测 bug 的防护**：`is_empty()` 只统计正文块时，
    //! 「仅有一条中断说明」的收尾轮次会被判为空而整轮丢弃。
    //!
    //! 触发场景很常见：中断发生后**没有后续消息**（会话最后一条即 interrupted），
    //! 于是界面什么都不显示、连提示都没有。
    //!
    //! 同一处判定在前端也有一份（`ChatArea.vue hasCurrentTurnContent`），
    //! 两边都必须把 notice 计入 —— 只改一边仍会漏。
    use super::AgentTurnSnapshot;

    fn snapshot_with_notice(key: Option<&str>) -> AgentTurnSnapshot {
        AgentTurnSnapshot {
            notice_i18n_key: key.map(|s| s.to_string()),
            ..Default::default()
        }
    }

    /// 只有 notice 时不算空：否则整轮被丢弃，用户看不到任何中断/等待提示
    #[test]
    fn notice_only_is_not_empty() {
        assert!(!snapshot_with_notice(Some("notice.streamTimeout")).is_empty());
    }

    /// 空白 notice 仍算空，避免渲染出一个空壳气泡
    #[test]
    fn blank_notice_is_still_empty() {
        assert!(snapshot_with_notice(Some("   ")).is_empty());
        assert!(snapshot_with_notice(None).is_empty());
    }
}

#[cfg(test)]
mod interrupted_body_tests {
    //! **实测 bug 的防护**：`source = "interrupted"` 的消息曾被渲染层整条丢弃
    //! （理由写的是"它不是模型正文"），而落库侧 `pipeline.rs` 写的恰恰是
    //! "正文即半截内容；正文块恒存在"。
    //!
    //! 后果：用户点「停止生成」后，界面只剩一句"用户已取消执行"，
    //! 已经生成的那部分正文凭空消失。
    //!
    //! 两边的契约以 **kind** 为准：别再按 source 一刀切地丢正文。
    use super::interrupted_body_is_content;

    /// 新数据（kind 有值）：正文是半截内容，必须渲染
    #[test]
    fn interrupt_with_kind_renders_body() {
        assert!(interrupted_body_is_content(Some("user_cancel")));
        assert!(interrupted_body_is_content(Some("stream_timeout")));
        assert!(interrupted_body_is_content(Some("app_closed")));
    }

    /// 旧库数据（kind 为 NULL）：正文是中断标记文本，不得渲染
    /// （notice 已由文本清洗路径生成并承载，再渲染会重复一遍）
    #[test]
    fn legacy_marker_body_stays_hidden() {
        assert!(!interrupted_body_is_content(None));
    }
}
