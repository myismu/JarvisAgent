// --- 会话持久化模块 (Sessions) ---
// 将对话历史持久化到磁盘，支持多会话管理（创建、切换、删除、重命名）。

use crate::core::models::{SessionMemory, Message, Content, ContentBlock};
use crate::get_agent_home;
use serde::{Deserialize, Serialize};
use std::fs;
use std::path::PathBuf;

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
pub fn create_session() -> SessionMeta {
    let id = uuid::Uuid::new_v4().to_string()[..8].to_string();
    let meta = SessionMeta {
        id: id.clone(),
        title: "新会话".to_string(),
        created_at: now_ts(),
        updated_at: now_ts(),
        message_count: 0,
        is_smart_named: false,
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
pub fn save_session(id: &str, memory: &SessionMemory) {
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
                message_count: 0,
                is_smart_named: false,
            }
        }
    } else {
        SessionMeta {
            id: id.to_string(),
            title: "新会话".to_string(),
            created_at: now_ts(),
            updated_at: now_ts(),
            message_count: 0,
            is_smart_named: false,
        }
    };

    // --- 过滤工具消息，仅保留有意义的对话内容 ---
    let filtered_messages: Vec<Message> = memory.messages.iter().filter_map(|msg| {
        match msg {
            Message::User { content } => {
                match content {
                    Content::Single(_) => Some(msg.clone()), // 保留用户文本输入
                    Content::Multiple(_) => None, // 跳过 tool_result 回传消息
                }
            }
            Message::Assistant { content } => {
                match content {
                    Content::Single(_) => Some(msg.clone()), // 保留助手文本回复
                    Content::Multiple(blocks) => {
                        // 仅保留文本块，过滤掉 tool_use 块
                        let text_blocks: Vec<ContentBlock> = blocks.iter().filter(|b| {
                            matches!(b, ContentBlock::Text { .. })
                        }).cloned().collect();
                        if text_blocks.is_empty() {
                            None // 如果助手消息只有工具调用没有文本，跳过
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

    // 更新元信息：仅在有新消息时才更新时间戳
    let new_count = filtered_messages.len();
    if new_count != meta.message_count {
        meta.updated_at = now_ts();
    }
    meta.message_count = new_count;
    // 如果标题还是默认的，尝试自动提取
    if meta.title == "新会话" && !memory.messages.is_empty() {
        meta.title = extract_title(&memory.messages);
    }

    let filtered_memory = SessionMemory {
        messages: filtered_messages,
        context: memory.context.clone(),
    };

    let file = SessionFile {
        meta,
        memory: filtered_memory,
    };
    let _ = fs::write(&path, serde_json::to_string_pretty(&file).unwrap_or_default());
    // 更新最后活跃会话
    let _ = fs::write(last_active_path(), id);
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

/// 删除会话
pub fn delete_session(id: &str) -> Result<(), String> {
    let path = sessions_dir().join(format!("{}.json", id));
    if path.exists() {
        fs::remove_file(&path).map_err(|e| e.to_string())?;
    }
    Ok(())
}

/// 重命名会话
pub fn rename_session(id: &str, new_title: &str) -> Result<SessionMeta, String> {
    let path = sessions_dir().join(format!("{}.json", id));
    if !path.exists() {
        return Err(format!("会话 {} 不存在", id));
    }
    let content = fs::read_to_string(&path).map_err(|e| e.to_string())?;
    let mut file: SessionFile = serde_json::from_str(&content).map_err(|e| e.to_string())?;
    file.meta.title = new_title.to_string();
    file.meta.is_smart_named = true;
    file.meta.updated_at = now_ts();
    let _ = fs::write(&path, serde_json::to_string_pretty(&file).unwrap_or_default());
    Ok(file.meta)
}

/// 获取最后活跃的会话 ID
pub fn get_last_active_session_id() -> Option<String> {
    fs::read_to_string(last_active_path()).ok().map(|s| s.trim().to_string())
}
