//! # 会话持久化模块 (Session Persistence)
//!
//! 将对话历史持久化到 SQLite，支持多会话管理。
//! 会话元数据、消息、步骤和计划文档由 `crate::core::session::repository` 统一读写。
//!
//! 主要功能：
//! - 会话 CRUD：创建、加载、保存、删除、重命名
//! - 图片管理：base64 图片存入附件目录，保存时清理内联数据
//! - 计划文档：会话级 PlanDocument 的增删改查
//! - 自动标题：从首条用户消息截取标题
//! - Token 统计：累计 input/output token 用量
//!
//! 存储结构：`<agent_home>/jarvis.sqlite3`

pub mod memory;
pub mod repository;
pub mod resource_repository;

use crate::infra::types::models::{
    Content, ContentBlock, ImageSource, Message, PlanDocument, SessionMemory,
};
use base64::Engine;
use serde::{Deserialize, Serialize};

/// 标题来源：默认（截取首条消息）、自动（LLM 生成）、手动（用户修改）
const DEFAULT_TITLE_SOURCE: &str = "default";
const AUTO_TITLE_SOURCE: &str = "auto";
const MANUAL_TITLE_SOURCE: &str = "manual";

/// 图片存储目录：`<agent_home>/sessions/<id>/attachments/images/`
/// 将 base64 图片数据解码并保存到文件，返回文件名
pub fn save_image_to_file(session_id: &str, media_type: &str, data: &str) -> String {
    let ext = if media_type.contains("jpeg") || media_type.contains("jpg") {
        "jpg"
    } else if media_type.contains("gif") {
        "gif"
    } else if media_type.contains("webp") {
        "webp"
    } else {
        "png"
    };
    let id = uuid::Uuid::new_v4().to_string()[..8].to_string();
    let filename = format!("{}_{}.{}", session_id, id, ext);
    if let Ok(decoded) = base64::engine::general_purpose::STANDARD.decode(data) {
        let _ = resource_repository::save_attachment(session_id, &filename, media_type, &decoded);
    }
    filename
}

/// 从文件加载图片并返回 base64 编码
pub fn load_image_data(filename: &str) -> Option<String> {
    let (_, bytes) = resource_repository::load_attachment(filename)
        .ok()
        .flatten()?;
    Some(base64::engine::general_purpose::STANDARD.encode(&bytes))
}

pub fn delete_image_file(filename: &str) {
    let _ = resource_repository::delete_attachment(filename);
}

/// 会话元信息（用于列表展示，不含完整消息体）
#[derive(Serialize, Deserialize, Debug, Clone)]
#[serde(rename_all = "camelCase")]
pub struct SessionMeta {
    pub id: String,
    pub title: String,
    pub created_at: u64,
    pub updated_at: u64,
    pub message_count: usize,
    #[serde(default)]
    pub is_smart_named: bool,
    #[serde(default)]
    pub profile_id: Option<String>,
    #[serde(default)]
    pub total_input_tokens: u64,
    #[serde(default)]
    pub total_output_tokens: u64,
    #[serde(default = "default_title_source")]
    pub title_source: String,
    /// 会话级工作目录沙箱，None 表示无限制
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub working_directory: Option<String>,
}

/// 获取当前时间戳（秒）
fn now_ts() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
}

fn default_title_source() -> String {
    DEFAULT_TITLE_SOURCE.to_string()
}

fn is_default_title_source(title_source: &str) -> bool {
    title_source == DEFAULT_TITLE_SOURCE
}

fn set_auto_title_source(meta: &mut SessionMeta) {
    meta.is_smart_named = true;
    meta.title_source = AUTO_TITLE_SOURCE.to_string();
}

fn set_manual_title_source(meta: &mut SessionMeta) {
    meta.is_smart_named = false;
    meta.title_source = MANUAL_TITLE_SOURCE.to_string();
}

/// 从消息列表中提取标题（取第一条用户消息的前 30 个字符）
fn extract_title(messages: &[Message]) -> String {
    for msg in messages {
        if let Message::User { content } = msg {
            let text = match content {
                Content::Single(s) => s.clone(),
                Content::Multiple(blocks) => {
                    // 从多块内容中提取第一个文本块
                    blocks
                        .iter()
                        .find_map(|b| {
                            if let crate::infra::types::models::ContentBlock::Text { text } = b {
                                Some(text.clone())
                            } else {
                                None
                            }
                        })
                        .unwrap_or_default()
                }
            };
            // 清理动态上下文注入的前缀，提取真正的用户输入
            let user_input = if let Some(pos) = text.find("[User Input]:") {
                text[pos + 13..].trim().to_string()
            } else {
                text.trim().to_string()
            };
            if !user_input.is_empty() {
                let title: String = user_input
                    .chars()
                    .take(crate::infra::types::constants::MAX_SESSION_TITLE_LEN)
                    .collect();
                return if user_input.chars().count() > crate::infra::types::constants::MAX_SESSION_TITLE_LEN
                {
                    format!("{}...", title)
                } else {
                    title
                };
            }
        }
    }
    "新会话".to_string()
}

/// 列出所有会话（按更新时间倒序）
pub fn list_sessions() -> Vec<SessionMeta> {
    repository::list_sessions(None).unwrap_or_default()
}

/// 创建新会话，返回元信息
pub fn create_session(working_directory: Option<String>) -> SessionMeta {
    let now = now_ts();
    let meta = SessionMeta {
        id: uuid::Uuid::new_v4().to_string()[..8].to_string(),
        title: "新会话".to_string(),
        created_at: now,
        updated_at: now,
        message_count: 0,
        is_smart_named: false,
        profile_id: None,
        total_input_tokens: 0,
        total_output_tokens: 0,
        title_source: default_title_source(),
        working_directory,
    };
    let memory = SessionMemory::default();
    let _ = repository::upsert_session(&meta, &memory);
    let _ = repository::set_last_active_session_id(&meta.id);
    meta
}

/// 保证会话消息 ID 与消息数组长度一致。
pub fn normalize_message_ids(memory: &mut SessionMemory) {
    let msg_count = memory.messages.len();
    let id_count = memory.message_ids.len();

    // 多余的 message_ids（通常不会发生）直接截断
    if id_count > msg_count {
        memory.message_ids.truncate(msg_count);
    }

    // 填充空的 message_id
    for message_id in &mut memory.message_ids {
        if message_id.trim().is_empty() {
            *message_id = uuid::Uuid::new_v4().to_string();
        }
    }

    // 补齐缺失的 message_id（数组长度不一致时）
    // 使用索引生成固定前缀 id，避免随机 UUID 覆盖已有 id 导致消息对应关系错乱
    let prefix = uuid::Uuid::new_v4().simple().to_string()[..6].to_string();
    let missing = msg_count.saturating_sub(memory.message_ids.len());
    for _ in 0..missing {
        let index = memory.message_ids.len();
        memory.message_ids.push(format!(
            "auto:{}{}:{}",
            prefix,
            index,
            uuid::Uuid::new_v4().simple().to_string()[..4].to_string()
        ));
    }

    // 最终对齐：如果仍有差异（不应发生），告警并强制对齐
    if memory.message_ids.len() != msg_count {
        eprintln!(
            "[MEMORY] message_ids 与 messages 长度不一致 ({} vs {})，强制对齐",
            memory.message_ids.len(),
            msg_count
        );
        memory.message_ids.truncate(msg_count);
        while memory.message_ids.len() < msg_count {
            memory
                .message_ids
                .push(uuid::Uuid::new_v4().to_string());
        }
    }
}

pub fn append_message(memory: &mut SessionMemory, message: Message) -> String {
    normalize_message_ids(memory);
    let message_id = uuid::Uuid::new_v4().to_string();
    memory.messages.push(message);
    memory.message_ids.push(message_id.clone());
    message_id
}

pub fn pop_message(memory: &mut SessionMemory) -> Option<(Message, String)> {
    normalize_message_ids(memory);
    let message = memory.messages.pop()?;
    let message_id = memory.message_ids.pop().unwrap_or_else(|| uuid::Uuid::new_v4().to_string());
    Some((message, message_id))
}

pub fn restore_message(memory: &mut SessionMemory, message: Message, message_id: String) {
    normalize_message_ids(memory);
    memory.messages.push(message);
    memory.message_ids.push(message_id);
}

pub fn reset_message_ids(memory: &mut SessionMemory) {
    // 只清理超出 messages 范围的多余 id，保留已有配对
    memory.message_ids.truncate(memory.messages.len());
    normalize_message_ids(memory);
}

/// 保存会话数据到 SQLite。
/// 保存时会过滤掉工具调用和工具结果，仅保留用户输入和助手文本回复，
/// 大幅减少存储体积。
pub fn save_session(
    id: &str,
    memory: &SessionMemory,
    token_usage_delta: Option<(u64, u64)>,
) -> SessionMeta {
    let mut meta = repository::get_session_meta(id).unwrap_or_else(|_| SessionMeta {
        id: id.to_string(),
        title: "新会话".to_string(),
        created_at: now_ts(),
        updated_at: now_ts(),
        message_count: memory.messages.len(),
        is_smart_named: false,
        profile_id: None,
        total_input_tokens: 0,
        total_output_tokens: 0,
        title_source: default_title_source(),
        working_directory: None,
    });

    let mut normalized_memory = memory.clone();
    normalize_message_ids(&mut normalized_memory);

    let filtered_pairs: Vec<(String, Message)> = normalized_memory
        .messages
        .iter()
        .enumerate()
        .filter_map(|(idx, msg)| {
            let message_id = normalized_memory.message_ids[idx].clone();
            let filtered_message = match msg {
            Message::User { content } => match content {
                Content::Single(_) => Some(msg.clone()),
                Content::Multiple(blocks) => {
                    let filtered_blocks: Vec<ContentBlock> = blocks
                        .iter()
                        .filter(|b| {
                            matches!(
                                b,
                                ContentBlock::Text { .. }
                                    | ContentBlock::Image { .. }
                                    | ContentBlock::ToolResult { .. }
                            )
                        })
                        .map(|b| {
                            if let ContentBlock::Image { source } = b {
                                let file_path = if source.file_path.is_some() {
                                    source.file_path.clone()
                                } else if !source.data.is_empty() {
                                    let fp =
                                        save_image_to_file(id, &source.media_type, &source.data);
                                    Some(fp)
                                } else {
                                    None
                                };
                                ContentBlock::Image {
                                    source: ImageSource {
                                        r#type: source.r#type.clone(),
                                        media_type: source.media_type.clone(),
                                        data: String::new(),
                                        file_path,
                                    },
                                }
                            } else {
                                b.clone()
                            }
                        })
                        .collect();
                    if filtered_blocks.is_empty() {
                        None
                    } else if filtered_blocks.len() == 1 {
                        if let ContentBlock::Text { text } = &filtered_blocks[0] {
                            Some(Message::User {
                                content: Content::Single(text.clone()),
                            })
                        } else {
                            Some(Message::User {
                                content: Content::Multiple(filtered_blocks),
                            })
                        }
                    } else {
                        Some(Message::User {
                            content: Content::Multiple(filtered_blocks),
                        })
                    }
                }
            },
            Message::Assistant { content } => match content {
                Content::Single(text) => {
                    if text.trim().is_empty() {
                        None
                    } else {
                        Some(Message::Assistant {
                            content: Content::Single(text.clone()),
                        })
                    }
                }
                Content::Multiple(blocks) => {
                    let text_blocks: Vec<ContentBlock> = blocks
                        .iter()
                        .filter_map(|b| match b {
                            ContentBlock::Text { text } if !text.trim().is_empty() => {
                                Some(b.clone())
                            }
                            ContentBlock::Thinking { thinking, .. }
                                if !thinking.trim().is_empty() =>
                            {
                                Some(b.clone())
                            }
                            ContentBlock::ToolUse { .. } => Some(b.clone()),
                            _ => None,
                        })
                        .collect();
                    if text_blocks.is_empty() {
                        None
                    } else {
                        Some(Message::Assistant {
                            content: Content::Multiple(text_blocks),
                        })
                    }
                }
            },
        };
            filtered_message.map(|message| (message_id, message))
        })
        .collect();
    let (filtered_message_ids, filtered_messages): (Vec<String>, Vec<Message>) =
        filtered_pairs.into_iter().unzip();

    if let Some((input_delta, output_delta)) = token_usage_delta {
        meta.total_input_tokens = meta.total_input_tokens.saturating_add(input_delta);
        meta.total_output_tokens = meta.total_output_tokens.saturating_add(output_delta);
    }

    let new_count = filtered_messages.len();
    if new_count > meta.message_count {
        meta.updated_at = now_ts();
    }
    meta.message_count = new_count;
    if is_default_title_source(&meta.title_source) && !normalized_memory.messages.is_empty() {
        meta.title = extract_title(&normalized_memory.messages);
    }

    let filtered_memory = SessionMemory {
        messages: filtered_messages,
        message_ids: filtered_message_ids,
        activated_tools: normalized_memory.activated_tools.clone(),
        plan_documents: normalized_memory.plan_documents.clone(),
    };

    repository::upsert_session(&meta, &filtered_memory)
        .unwrap_or_else(|err| panic!("保存 SQLite 会话 {} 失败: {}", id, err));
    let visible_pairs: Vec<(String, Message)> = filtered_memory
        .message_ids
        .iter()
        .cloned()
        .zip(filtered_memory.messages.iter().cloned())
        .filter(|(_, message)| !is_compact_summary_message(message))
        .collect();
    let (visible_message_ids, visible_messages): (Vec<String>, Vec<Message>) =
        visible_pairs.into_iter().unzip();
    // 同步前清理 session_messages 中已被压缩覆盖的孤儿行
    // compressed User 消息不写入 session_messages，但需保留在 alive 集合中避免被误清
    let _ = repository::hide_orphan_session_messages(id, &filtered_memory.message_ids);
    repository::append_or_upsert_session_messages(
        id,
        &visible_messages,
        &visible_message_ids,
        "chat",
        meta.updated_at,
    )
    .unwrap_or_else(|err| panic!("保存 SQLite 会话历史 {} 失败: {}", id, err));
    let _ = repository::set_last_active_session_id(id);
    meta
}

fn is_compact_summary_message(message: &Message) -> bool {
    let Message::User { content } = message else {
        return false;
    };
    match content {
        Content::Single(text) => text.trim_start().starts_with("[Conversation compressed."),
        Content::Multiple(blocks) => blocks.iter().any(|block| {
            matches!(
                block,
                ContentBlock::Text { text }
                    if text.trim_start().starts_with("[Conversation compressed.")
            )
        }),
    }
}

/// 加载指定会话的完整数据
pub fn load_session(id: &str) -> Result<SessionMemory, String> {
    let memory = repository::load_session(id)?;
    repository::set_last_active_session_id(id)?;
    Ok(memory)
}

pub fn list_visible_session_messages(id: &str) -> Result<Vec<repository::StoredSessionMessage>, String> {
    repository::list_visible_session_messages(id)
}

pub fn find_session_message_by_id(
    session_id: &str,
    message_id: &str,
) -> Result<Option<repository::StoredSessionMessage>, String> {
    repository::find_session_message_by_id(session_id, message_id)
}

pub fn hide_session_messages_from_seq(
    session_id: &str,
    seq: usize,
    recalled: bool,
) -> Result<(), String> {
    repository::hide_session_messages_from_seq(session_id, seq, recalled)
}

pub fn delete_session_messages_from_seq(
    session_id: &str,
    seq: usize,
) -> Result<(), String> {
    repository::delete_session_messages_from_seq(session_id, seq)
}

pub fn hide_orphan_session_messages(
    session_id: &str,
    alive_message_ids: &[String],
) -> Result<usize, String> {
    repository::hide_orphan_session_messages(session_id, alive_message_ids)
}

pub fn session_messages_count(session_id: &str) -> Result<usize, String> {
    repository::session_messages_count(session_id)
}

pub fn save_context_snapshot(
    snapshot: &crate::infra::types::models::SessionContextSnapshot,
) -> Result<(), String> {
    repository::upsert_context_snapshot(snapshot)
}

pub fn update_context_snapshot_usage(
    session_id: &str,
    provider_input_tokens: u64,
    provider_output_tokens: u64,
    provider_total_tokens: u64,
    drift_percent: Option<f32>,
) -> Result<Option<crate::infra::types::models::SessionContextSnapshot>, String> {
    repository::update_context_snapshot_usage(
        session_id,
        provider_input_tokens,
        provider_output_tokens,
        provider_total_tokens,
        drift_percent,
    )
}

pub fn get_context_snapshot(
    session_id: &str,
) -> Result<Option<crate::infra::types::models::SessionContextSnapshot>, String> {
    repository::get_context_snapshot(session_id)
}

/// 列出会话关联的计划文档（按更新时间倒序）
pub fn list_plan_documents(session_id: &str) -> Result<Vec<PlanDocument>, String> {
    let mut plans = load_session(session_id)?.plan_documents;
    plans.sort_by(|a, b| b.updated_at.cmp(&a.updated_at));
    Ok(plans)
}

pub fn upsert_plan_document(
    session_id: &str,
    mut document: PlanDocument,
) -> Result<PlanDocument, String> {
    let mut memory = load_session(session_id).unwrap_or_default();
    if document.session_id.is_empty() {
        document.session_id = session_id.to_string();
    }
    if document.created_at == 0 {
        document.created_at = now_ts();
    }
    document.updated_at = now_ts();

    if let Some(existing) = memory
        .plan_documents
        .iter_mut()
        .find(|item| item.id == document.id)
    {
        *existing = document.clone();
    } else {
        memory.plan_documents.push(document.clone());
    }

    save_session(session_id, &memory, None);
    Ok(document)
}

pub fn update_plan_document_status(
    session_id: &str,
    plan_id: &str,
    status: &str,
    content: Option<String>,
) -> Result<Option<PlanDocument>, String> {
    let mut memory = load_session(session_id).unwrap_or_default();
    let mut updated = None;
    let now = now_ts();

    if let Some(document) = memory
        .plan_documents
        .iter_mut()
        .find(|item| item.id == plan_id)
    {
        document.status = status.to_string();
        if let Some(content) = content {
            document.content = content;
        }
        document.updated_at = now;
        document.decided_at = Some(now);
        updated = Some(document.clone());
    }

    if updated.is_some() {
        save_session(session_id, &memory, None);
    }

    Ok(updated)
}

/// 获取单个会话的元信息（不受 message_count 过滤影响）
pub fn get_session_meta(id: &str) -> Result<SessionMeta, String> {
    repository::get_session_meta(id)
}

/// 删除会话
pub fn delete_session(id: &str) -> Result<(), String> {
    repository::delete_session(id)
}

/// 重命名会话
pub fn rename_session(
    id: &str,
    new_title: &str,
    is_auto_generated: bool,
) -> Result<SessionMeta, String> {
    let mut meta = repository::get_session_meta(id)?;
    if is_auto_generated {
        set_auto_title_source(&mut meta);
    } else {
        set_manual_title_source(&mut meta);
    }
    repository::rename_session(id, new_title, meta.is_smart_named, &meta.title_source)
}

/// 更新会话的模型预设
pub fn update_session_profile(id: &str, profile_id: &str) -> Result<(), String> {
    repository::update_session_profile(id, profile_id)
}

/// 获取最后活跃的会话 ID
pub fn get_last_active_session_id() -> Option<String> {
    repository::get_last_active_session_id()
}
