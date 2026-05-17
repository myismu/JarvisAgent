//! # context.rs — 上下文构建与消息注入
//!
//! 负责构建动态上下文（意图标签、全局记忆、项目结构、技能列表等），
//! 并将用户消息、图片数据、上下文信息注入到会话历史中。
//!
//! ## 关键导出
//! - `build_dynamic_context()`: 根据意图类型组装动态上下文字符串
//! - `inject_user_message()`: 将用户消息（含图片）注入会话历史，返回消息索引
//! - `inject_context_into_history()`: 将动态上下文注入到指定消息中
//! - `restore_image_data()`: 恢复历史消息中的图片数据（近期保留，远期折叠）
//!
//! ## 依赖
//! - Internal: `crate::core::session::memory`, `crate::infra::types::models`, `crate::core::tools`
//! - External: 无
//!
//! ## 约束
//! - 图片数据仅保留最近 2 条消息中的，远期图片会被折叠为文本摘要
//! - 闲聊模式（CHAT）下会截断工具返回内容以节省 Token

use crate::infra::types::models::*;
use crate::core::session::{append_message, memory::*};
use crate::core::tools::*;

pub fn build_dynamic_context(
    intent: &str,
    workspace: &Option<std::path::PathBuf>,
) -> String {
    match intent {
        "CHAT" => String::new(),
        "QUESTION" => {
            let global_content = read_memory_file(&get_global_memory_path(), "Global Memory");
            format!(
                "<intent>\nQUESTION\n</intent>\n\n<global_context>\n{}\n</global_context>\n",
                global_content
            )
        }
        _ => {
            let mut ctx = String::new();

            // ── 1. 行为约束（必定阅读，控制在 3 条）──
            ctx.push_str("── 行为约束\n");
            ctx.push_str("1. 代码探索优先用 FindFiles 获取结构，再 ReadFile 精准读取，禁止逐个文件遍历\n");
            ctx.push_str("2. 复杂任务（3+文件改动/架构变更）必须先 ProposePlan 提交方案，审批后才能执行\n");
            ctx.push_str("3. 当前模式：编辑（全工具可用，复杂任务切 Plan）\n");

            // ── 2. 环境信息 ──
            ctx.push_str("\n── 环境\n");
            if let Some(ref ws_path) = workspace {
                ctx.push_str(&format!("工作目录: {}\n", ws_path.display()));
            } else {
                ctx.push_str("工作目录: 当前目录（无沙箱限制）\n");
            }

            // ── 3. 参考索引（按需查阅，不重要可跳过）──
            ctx.push_str("\n── 参考索引\n");

            // 项目结构（仅在项目会话时生成）
            if let Some(ref ws_path) = workspace {
                let repo_map = generate_repo_map(ws_path, "", 0, 2);
                ctx.push_str("项目结构:\n");
                ctx.push_str(&repo_map);
            }

            // 工具索引（紧凑格式）
            ctx.push_str("可用工具:\n");
            ctx.push_str(&get_deferred_tools_context_compact(intent));

            // 技能（仅名称）
            let skills = load_all_skills();
            if !skills.is_empty() {
                let names: Vec<String> = skills.iter().map(|s| s.name.to_string()).collect();
                ctx.push_str(&format!("可用技能: {}\n", names.join(", ")));
            }

            // 全局记忆（压缩在末尾）
            let global_content = read_memory_file(&get_global_memory_path(), "Global Memory");
            let trimmed = global_content.trim();
            if !trimmed.is_empty() {
                ctx.push_str(&format!("\n── 全局记忆\n{}\n", trimmed));
            }

            ctx
        }
    }
}

pub fn inject_user_message(
    session: &mut SessionMemory,
    msg: &str,
    image_base64_list: &Option<Vec<String>>,
    active_session_id: &mut Option<String>,
) -> usize {
    let initial_msg_index = session.messages.len();

    let message = if let Some(images) = image_base64_list {
        if !images.is_empty() {
            let mut blocks: Vec<ContentBlock> = Vec::new();
            for img_base64 in images {
                let media_type = img_base64
                    .split(':')
                    .nth(1)
                    .and_then(|s| s.split(';').next())
                    .unwrap_or("image/png")
                    .to_string();
                let data = img_base64.split(',').nth(1).unwrap_or("").to_string();
                let session_id_str = active_session_id.clone().unwrap_or_default();
                let file_path = if !data.is_empty() {
                    let fp = crate::core::session::save_image_to_file(
                        &session_id_str,
                        &media_type,
                        &data,
                    );
                    Some(fp)
                } else {
                    None
                };
                blocks.push(ContentBlock::Image {
                    source: ImageSource {
                        r#type: "base64".to_string(),
                        media_type,
                        data: String::new(),
                        file_path,
                    },
                });
            }
            if !msg.is_empty() {
                blocks.insert(
                    0,
                    ContentBlock::Text {
                        text: msg.to_string(),
                    },
                );
            }
            Message::User {
                content: Content::Multiple(blocks),
            }
        } else {
            Message::User {
                content: Content::Single(msg.to_string()),
            }
        }
    } else {
        Message::User {
            content: Content::Single(msg.to_string()),
        }
    };
    append_message(session, message);

    initial_msg_index
}

pub fn inject_context_into_history(
    history_snapshot: &mut Vec<Message>,
    initial_msg_index: usize,
    dynamic_context_str: &str,
) {
    if let Some(initial_msg) = history_snapshot.get_mut(initial_msg_index) {
        if let Message::User {
            content: Content::Single(ref mut text),
        } = initial_msg
        {
            *text = format!("[User Input]:\n{}\n\n{}", text, dynamic_context_str);
        } else if let Message::User {
            content: Content::Multiple(ref mut blocks),
        } = initial_msg
        {
            if blocks.iter().any(|block| matches!(block, ContentBlock::ToolResult { .. })) {
                return;
            }
            blocks.push(
                ContentBlock::Text {
                    text: format!("\n\n{}", dynamic_context_str),
                },
            );
        }
    }
}

pub fn restore_image_data(history_snapshot: &mut Vec<Message>) {
    let keep_recent_image_msgs = 2;
    let total_msgs = history_snapshot.len();
    for (i, msg) in history_snapshot.iter_mut().enumerate() {
        if let Message::User { content } = msg {
            if let Content::Multiple(blocks) = content {
                let is_recent = i + keep_recent_image_msgs >= total_msgs;
                let mut new_blocks = Vec::new();
                for block in blocks.drain(..) {
                    match block {
                        ContentBlock::Image { ref source } => {
                            if is_recent {
                                let mut img_block = block.clone();
                                if let ContentBlock::Image { ref mut source } = img_block {
                                    if source.data.is_empty() {
                                        if let Some(ref fp) = source.file_path {
                                            if let Some(data) =
                                                crate::core::session::load_image_data(fp)
                                            {
                                                source.data = data;
                                            }
                                        }
                                    }
                                }
                                new_blocks.push(img_block);
                            } else {
                                let summary = format!("[图片: {}]", source.media_type);
                                new_blocks.push(ContentBlock::Text { text: summary });
                            }
                        }
                        _ => {
                            new_blocks.push(block);
                        }
                    }
                }
                *blocks = new_blocks;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_session() -> SessionMemory {
        SessionMemory::default()
    }

    #[test]
    fn inject_user_message_plain_text() {
        let mut session = make_session();
        let msg = "Hello, world!";
        let images: Option<Vec<String>> = None;
        let mut sid = Some("test-session".to_string());

        let idx = inject_user_message(&mut session, msg, &images, &mut sid);
        assert_eq!(idx, 0);
        assert_eq!(session.messages.len(), 1);
        match &session.messages[0] {
            Message::User { content } => match content {
                Content::Single(text) => assert_eq!(text, msg),
                _ => panic!("Expected single text content"),
            },
            _ => panic!("Expected user message"),
        }
    }

    #[test]
    fn inject_user_message_with_empty_images() {
        let mut session = make_session();
        let msg = "Test message";
        let images = Some(vec![]);
        let mut sid = Some("test-session".to_string());

        inject_user_message(&mut session, msg, &images, &mut sid);
        assert_eq!(session.messages.len(), 1);
        match &session.messages[0] {
            Message::User { content } => match content {
                Content::Single(text) => assert_eq!(text, msg),
                _ => panic!("Expected single text content"),
            },
            _ => panic!("Expected user message"),
        }
    }

    #[test]
    fn inject_context_into_history_single_text() {
        let mut history = vec![Message::User {
            content: Content::Single("原始消息".to_string()),
        }];
        let ctx = "<intent>\nCHAT\n</intent>\n";

        inject_context_into_history(&mut history, 0, ctx);
        match &history[0] {
            Message::User { content } => match content {
                Content::Single(text) => {
                    assert!(text.starts_with("[User Input]:"));
                    assert!(text.contains("原始消息"));
                }
                _ => panic!("Expected single text"),
            },
            _ => panic!("Expected user message"),
        }
    }

    #[test]
    fn inject_context_skips_tool_result_messages() {
        let mut history = vec![Message::User {
            content: Content::Multiple(vec![ContentBlock::ToolResult {
                tool_use_id: "call_1".to_string(),
                content: "工具结果".to_string(),
            }]),
        }];
        let ctx = "<intent>\nPROJECT_ACTION\n</intent>\n";

        inject_context_into_history(&mut history, 0, ctx);
        match &history[0] {
            Message::User { content: Content::Multiple(blocks) } => {
                assert_eq!(blocks.len(), 1);
                assert!(matches!(blocks[0], ContentBlock::ToolResult { .. }));
            }
            _ => panic!("Expected tool result user message"),
        }
    }
}
