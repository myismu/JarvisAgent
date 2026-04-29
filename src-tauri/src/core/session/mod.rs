//! # 会话持久化模块 (Session Persistence)
//!
//! 将对话历史持久化到磁盘，支持多会话管理。
//! 每个会话存储为独立 JSON 文件，包含元信息和消息体。
//!
//! 主要功能：
//! - 会话 CRUD：创建、加载、保存、删除、重命名
//! - 图片管理：base64 图片存入文件，保存时清理内联数据
//! - 计划文档：会话级 PlanDocument 的增删改查
//! - 自动标题：从首条用户消息截取标题
//! - Token 统计：累计 input/output token 用量
//!
//! 存储结构：`<agent_home>/.sessions/<id>.json`

pub mod checkpoint;
pub mod memory;

use crate::core::models::{SessionMemory, Message, Content, ContentBlock, ImageSource, PlanDocument};
use crate::get_agent_home;
use base64::Engine;
use serde::{Deserialize, Serialize};
use std::fs;
use std::path::PathBuf;

/// 标题来源：默认（截取首条消息）、自动（LLM 生成）、手动（用户修改）
const DEFAULT_TITLE_SOURCE: &str = "default";
const AUTO_TITLE_SOURCE: &str = "auto";
const MANUAL_TITLE_SOURCE: &str = "manual";

/// 图片存储目录（agent_home/images/）
fn images_dir() -> PathBuf {
    let dir = get_agent_home().join(crate::core::constants::DIR_IMAGES);
    if !dir.exists() {
        let _ = fs::create_dir_all(&dir);
    }
    dir
}

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
    let path = images_dir().join(&filename);
    if let Ok(decoded) = base64::engine::general_purpose::STANDARD.decode(data) {
        let _ = fs::write(&path, decoded);
    }
    filename
}

/// 从文件加载图片并返回 base64 编码
pub fn load_image_data(filename: &str) -> Option<String> {
    let path = images_dir().join(filename);
    let bytes = fs::read(&path).ok()?;
    Some(base64::engine::general_purpose::STANDARD.encode(&bytes))
}

pub fn delete_image_file(filename: &str) {
    let path = images_dir().join(filename);
    if path.exists() {
        let _ = fs::remove_file(&path);
    }
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

/// 完整会话数据（含消息体，用于存储和加载）
#[derive(Serialize, Deserialize, Debug)]
struct SessionFile {
    meta: SessionMeta,
    memory: SessionMemory,
}

/// 获取会话存储目录
fn sessions_dir() -> PathBuf {
    let dir = get_agent_home().join(crate::core::constants::DIR_SESSIONS);
    if !dir.exists() {
        let _ = fs::create_dir_all(&dir);
    }
    dir
}

/// 获取最后活跃会话 ID 的记录文件路径
fn last_active_path() -> PathBuf {
    sessions_dir().join(crate::core::constants::FILE_LAST_ACTIVE_SESSION)
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
                    blocks.iter().find_map(|b| {
                        if let crate::core::models::ContentBlock::Text { text } = b {
                            Some(text.clone())
                        } else {
                            None
                        }
                    }).unwrap_or_default()
                }
            };
            // 清理动态上下文注入的前缀，提取真正的用户输入
            let user_input = if let Some(pos) = text.find("[User Input]:") {
                text[pos + 13..].trim().to_string()
            } else {
                text.trim().to_string()
            };
            if !user_input.is_empty() {
                let title: String = user_input.chars().take(crate::core::constants::MAX_SESSION_TITLE_LEN).collect();
                return if user_input.chars().count() > crate::core::constants::MAX_SESSION_TITLE_LEN {
                    format!("{}...", title)
                } else {
                    title
                };
            }
        }
    }
    "新会话".to_string()
}

/// 列出所有会话（按更新时间倒序），自动清理空会话
pub fn list_sessions() -> Vec<SessionMeta> {
    let dir = sessions_dir();
    let mut sessions = Vec::new();
    let last_active = get_last_active_session_id();
    
    if let Ok(entries) = fs::read_dir(&dir) {
        for entry in entries.flatten() {
            let path = entry.path();
            if path.extension().map_or(false, |ext| ext == "json") {
                if let Ok(content) = fs::read_to_string(&path) {
                    if let Ok(file) = serde_json::from_str::<SessionFile>(&content) {
                        // 不在左侧生成历史记录，如果是空会话，直接跳过并尝试删除（如果不是当前活跃会话，删除物理文件。就算当前活跃也不返回给前端列表）
                        if file.meta.message_count == 0 {
                            if Some(&file.meta.id) != last_active.as_ref() {
                                let _ = fs::remove_file(&path);
                            }
                            continue;
                        }
                        sessions.push(file.meta);
                    }
                }
            }
        }
    }
    // 按更新时间倒序排列
    sessions.sort_by(|a, b| b.updated_at.cmp(&a.updated_at));
    sessions
}

/// 创建新会话，返回元信息
pub fn create_session(working_directory: Option<String>) -> SessionMeta {
    let id = uuid::Uuid::new_v4().to_string()[..8].to_string();
    let meta = SessionMeta {
        id: id.clone(),
        title: "新会话".to_string(),
        created_at: now_ts(),
        updated_at: now_ts(),
        message_count: 0,
        is_smart_named: false,
        profile_id: None,
        total_input_tokens: 0,
        total_output_tokens: 0,
        title_source: default_title_source(),
        working_directory,
    };
    let file = SessionFile {
        meta: meta.clone(),
        memory: SessionMemory::default(),
    };
    let path = sessions_dir().join(format!("{}.json", id));
    let _ = fs::write(&path, serde_json::to_string_pretty(&file).unwrap_or_default());
    // 记录为最后活跃会话
    let _ = fs::write(last_active_path(), &id);
    meta
}

/// 保存会话数据到磁盘
/// 保存时会过滤掉工具调用和工具结果，仅保留用户输入和助手文本回复，
/// 大幅减少文件体积。
pub fn save_session(id: &str, memory: &SessionMemory, token_usage_delta: Option<(u64, u64)>) -> SessionMeta {
    let path = sessions_dir().join(format!("{}.json", id));
    // 尝试加载已有元信息，保留 created_at
    let mut meta = if let Ok(content) = fs::read_to_string(&path) {
        if let Ok(file) = serde_json::from_str::<SessionFile>(&content) {
            file.meta
        } else {
            SessionMeta {
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
            }
        }
    } else {
        SessionMeta {
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
        }
    };

    // --- 过滤工具消息，仅保留有意义的对话内容 ---
    let filtered_messages: Vec<Message> = memory.messages.iter().filter_map(|msg| {
        match msg {
            Message::User { content } => {
                match content {
                    Content::Single(_) => Some(msg.clone()),
                    Content::Multiple(blocks) => {
                        let filtered_blocks: Vec<ContentBlock> = blocks.iter()
                            .filter(|b| matches!(b, ContentBlock::Text { .. } | ContentBlock::Image { .. }))
                            .map(|b| {
                                if let ContentBlock::Image { source } = b {
                                    let file_path = if source.file_path.is_some() {
                                        source.file_path.clone()
                                    } else if !source.data.is_empty() {
                                        let fp = save_image_to_file(id, &source.media_type, &source.data);
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
                                Some(Message::User { content: Content::Single(text.clone()) })
                            } else {
                                Some(Message::User { content: Content::Multiple(filtered_blocks) })
                            }
                        } else {
                            Some(Message::User { content: Content::Multiple(filtered_blocks) })
                        }
                    }
                }
            }
            Message::Assistant { content } => {
                match content {
                    Content::Single(text) => {
                        if text.trim().is_empty() {
                            None
                        } else {
                            Some(Message::Assistant { content: Content::Single(text.clone()) })
                        }
                    }
                    Content::Multiple(blocks) => {
                        let text_blocks: Vec<ContentBlock> = blocks.iter().filter_map(|b| {
                            match b {
                                ContentBlock::Text { text } if !text.trim().is_empty() => Some(b.clone()),
                                ContentBlock::Thinking { thinking, .. } if !thinking.trim().is_empty() => Some(b.clone()),
                                _ => None,
                            }
                        }).collect();
                        if text_blocks.is_empty() {
                            None
                        } else {
                            Some(Message::Assistant {
                                content: Content::Multiple(text_blocks),
                            })
                        }
                    }
                }
            }
        }
    }).collect();

    if let Some((input_delta, output_delta)) = token_usage_delta {
        meta.total_input_tokens = meta.total_input_tokens.saturating_add(input_delta);
        meta.total_output_tokens = meta.total_output_tokens.saturating_add(output_delta);
    }

    // 更新元信息：仅在有新消息时才更新时间戳
    let new_count = filtered_messages.len();
    if new_count > meta.message_count {
        meta.updated_at = now_ts();
    }
    meta.message_count = new_count;
    if is_default_title_source(&meta.title_source) && !memory.messages.is_empty() {
        meta.title = extract_title(&memory.messages);
    }

    let filtered_memory = SessionMemory {
        messages: filtered_messages,
        context: memory.context.clone(),
        agent_steps: memory.agent_steps.clone(),
        plan_documents: memory.plan_documents.clone(),
    };

    let file = SessionFile {
        meta: meta.clone(),
        memory: filtered_memory,
    };
    let _ = fs::write(&path, serde_json::to_string_pretty(&file).unwrap_or_default());
    // 更新最后活跃会话
    let _ = fs::write(last_active_path(), id);
    meta
}

/// 加载指定会话的完整数据
pub fn load_session(id: &str) -> Result<SessionMemory, String> {
    let path = sessions_dir().join(format!("{}.json", id));
    if !path.exists() {
        return Err(format!("会话 {} 不存在", id));
    }
    let content = fs::read_to_string(&path).map_err(|e| e.to_string())?;
    let file: SessionFile = serde_json::from_str(&content).map_err(|e| e.to_string())?;
    // 更新最后活跃会话
    let _ = fs::write(last_active_path(), id);
    Ok(file.memory)
}

/// 列出会话关联的计划文档（按更新时间倒序）
pub fn list_plan_documents(session_id: &str) -> Result<Vec<PlanDocument>, String> {
    let mut plans = load_session(session_id)?.plan_documents;
    plans.sort_by(|a, b| b.updated_at.cmp(&a.updated_at));
    Ok(plans)
}

pub fn upsert_plan_document(session_id: &str, mut document: PlanDocument) -> Result<PlanDocument, String> {
    let mut memory = load_session(session_id).unwrap_or_default();
    if document.session_id.is_empty() {
        document.session_id = session_id.to_string();
    }
    if document.created_at == 0 {
        document.created_at = now_ts();
    }
    document.updated_at = now_ts();

    if let Some(existing) = memory.plan_documents.iter_mut().find(|item| item.id == document.id) {
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

    if let Some(document) = memory.plan_documents.iter_mut().find(|item| item.id == plan_id) {
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
    let path = sessions_dir().join(format!("{}.json", id));
    if !path.exists() {
        return Err(format!("会话 {} 不存在", id));
    }
    let content = fs::read_to_string(&path).map_err(|e| e.to_string())?;
    let file: SessionFile = serde_json::from_str(&content).map_err(|e| e.to_string())?;
    println!("[DEBUG] get_session_meta: id={}, working_directory={:?}", id, file.meta.working_directory);
    Ok(file.meta)
}

/// 删除会话（同时清理关联的图片文件）
pub fn delete_session(id: &str) -> Result<(), String> {
    let path = sessions_dir().join(format!("{}.json", id));
    if path.exists() {
        let _ = fs::remove_file(&path);
    }
    let img_dir = images_dir();
    if let Ok(entries) = fs::read_dir(&img_dir) {
        let prefix = format!("{}_", id);
        for entry in entries.flatten() {
            if let Some(name) = entry.file_name().to_str() {
                if name.starts_with(&prefix) {
                    let _ = fs::remove_file(entry.path());
                }
            }
        }
    }
    Ok(())
}

/// 重命名会话
pub fn rename_session(id: &str, new_title: &str, is_auto_generated: bool) -> Result<SessionMeta, String> {
    let path = sessions_dir().join(format!("{}.json", id));
    if !path.exists() {
        return Err(format!("会话 {} 不存在", id));
    }
    let content = fs::read_to_string(&path).map_err(|e| e.to_string())?;
    let mut file: SessionFile = serde_json::from_str(&content).map_err(|e| e.to_string())?;
    file.meta.title = new_title.to_string();
    if is_auto_generated {
        set_auto_title_source(&mut file.meta);
    } else {
        set_manual_title_source(&mut file.meta);
    }
    let _ = fs::write(&path, serde_json::to_string_pretty(&file).unwrap_or_default());
    Ok(file.meta)
}

/// 更新会话的模型预设
pub fn update_session_profile(id: &str, profile_id: &str) -> Result<(), String> {
    let path = sessions_dir().join(format!("{}.json", id));
    if !path.exists() {
        return Err(format!("会话 {} 不存在", id));
    }
    let content = fs::read_to_string(&path).map_err(|e| e.to_string())?;
    let mut file: SessionFile = serde_json::from_str(&content).map_err(|e| e.to_string())?;
    file.meta.profile_id = Some(profile_id.to_string());
    let _ = fs::write(&path, serde_json::to_string_pretty(&file).unwrap_or_default());
    Ok(())
}

/// 获取最后活跃的会话 ID
pub fn get_last_active_session_id() -> Option<String> {
    fs::read_to_string(last_active_path()).ok().map(|s| s.trim().to_string())
}
