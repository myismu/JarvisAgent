//! # context.rs — 上下文构建与消息注入
//!
//! 负责构建随用户消息一起落库的动态上下文（意图标签、项目结构、用户画像），
//! 并将用户消息、图片数据、上下文信息注入到会话历史中。
//!
//! 静态规范（探索纪律、审批纪律）与沙箱语义不在这里：它们在 system prompt 中，
//! 因为 system 每轮必发且是缓存前缀，放那里既权威，又不会在每条用户消息里重复一遍。
//!
//! ## 关键导出
//! - `build_dynamic_context()`: 根据意图类型组装动态上下文字符串
//! - `inject_user_message()`: 将用户消息（含动态上下文块、图片）写入会话历史，返回消息索引
//! - `restore_image_data()`: 恢复历史消息中的图片数据（本轮保留 base64，往轮折叠为摘要）
//!
//! ## 依赖
//! - Internal: `crate::core::session::memory`, `crate::infra::types::models`, `crate::core::tools`
//! - External: 无
//!
//! ## 约束
//! - 图片以「本轮用户消息」为界：本轮始终携带 base64，往轮折叠为 `[图片: type]`。
//!   不用「最后一条 Assistant」当界——工具循环中途会插入 Assistant，那样会让本轮图片
//!   在循环第二轮就被折叠，请求前缀中途改变（缓存全失效），模型也再看不到本轮图片
//! - 动态上下文只放「每轮才有效且会变」的内容，格式统一为 XML 标签

use crate::infra::types::models::*;
use crate::core::session::{append_message, memory::*};
use crate::core::tools::*;
use crate::core::agent::prompts::get_mode_prompt;

/// 构建随用户消息一起落库的动态上下文。
///
/// 只放「每轮才有效、且会随会话变化」的东西：意图标签、项目结构、用户画像。
///
/// 静态规范（探索纪律、审批纪律）和沙箱语义都在 system prompt 里，这里不再重复——
/// 之前把「当前模式」写死在这一段里，模式切换后 system prompt 说"规划"、
/// 而历史里冻结着一堆"编辑"，模型会被就近的错误信息带偏。
///
/// 格式统一为 XML 标签：边界明确（模型不会把分隔符当正文），
/// 以后要做程序化裁剪或检索式注入也能直接解析。
pub fn build_dynamic_context(
    intent: &str,
    workspace: &Option<std::path::PathBuf>,
    capabilities: &crate::core::tools::framework::capabilities::Capabilities,
    work_mode: &str,
    snapshot_seq: u64,
) -> String {
    // 闲聊：不需要任何运行时上下文
    if intent == "CHAT" {
        return String::new();
    }

    let mode = if work_mode == "plan" { "plan" } else { "edit" };
    let mut ctx = format!(
        "<context_snapshot seq=\"{}\" mode=\"{}\" />\n本回合工作模式：{}\n",
        snapshot_seq, mode, mode
    );
    ctx.push_str("<mode_rules>\n");
    ctx.push_str(get_mode_prompt(mode));
    ctx.push_str("\n</mode_rules>\n");
    ctx.push_str(&format!("<intent>{}</intent>\n", intent));
    // 能力边界：放在最前面，让模型第一轮就知道"哪些事在本会话根本做不到"，
    // 不必（也不该）靠 GetToolCatalog / DiscoverTools 去试探
    ctx.push_str(&capabilities.context_block());
    ctx.push('\n');

    // 项目结构：只有真正绑定了工作区才生成；QUESTION 保持轻量，不生成
    if intent != "QUESTION" {
        if let Some(ref ws_path) = workspace {
            let repo_map = generate_repo_map(ws_path, "", 0, 2);
            if !repo_map.trim().is_empty() {
                ctx.push_str("<project_index>\n");
                ctx.push_str(&repo_map);
                ctx.push_str("\n</project_index>\n");
            }
        }
    }

    // 用户画像：只注入「身份 + 交互偏好」精简画像，完整记忆按需用 ReadMemory 取。
    // 全量注入会让记忆随轮次在历史里累积 N 份，且记忆膨胀后无法收敛。
    // 持锁读，避免读到写入中途的半截内容（并发的 UpdateMemory 会整份覆盖写）
    let global_content = {
        let _guard = lock_memory_file();
        read_memory_file(&get_global_memory_path(), "Global Memory")
    };
    let profile = crate::core::tools::agent_tools::extract_profile(&global_content);
    if !profile.is_empty() {
        ctx.push_str("<user_profile>\n");
        ctx.push_str(&profile);
        ctx.push_str("\n</user_profile>\n");
    }

    ctx
}

pub fn inject_user_message(
    session: &mut SessionMemory,
    msg: &str,
    image_base64_list: &Option<Vec<String>>,
    dynamic_context: &str,
    active_session_id: &mut Option<String>,
) -> usize {
    let initial_msg_index = session.messages.len();

    let mut blocks: Vec<ContentBlock> = Vec::new();

    // 动态上下文作为独立块放在最前，用户原文紧随其后
    let context = dynamic_context.trim();
    if !context.is_empty() {
        blocks.push(ContentBlock::Context {
            text: context.to_string(),
        });
    }
    if !msg.is_empty() {
        blocks.push(ContentBlock::Text {
            text: msg.to_string(),
        });
    }
    if let Some(images) = image_base64_list {
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
                let fp =
                    crate::core::session::save_image_to_file(&session_id_str, &media_type, &data);
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
    }

    let content = match blocks.len() {
        0 => Content::Single(String::new()),
        1 => match blocks.pop().expect("length checked") {
            ContentBlock::Text { text } => Content::Single(text),
            other => Content::Multiple(vec![other]),
        },
        _ => Content::Multiple(blocks),
    };

    append_message(session, Message::User { content }, "chat");

    initial_msg_index
}

/// 折叠/恢复历史快照里的图片。
///
/// `current_turn_start` = 本轮用户消息在快照中的下标：
/// - 下标 `< current_turn_start`：往轮消息，图片折叠为 `[图片: media_type]` 省 token；
/// - 下标 `>= current_turn_start`：本轮消息，恢复 base64（模型必须真正看到图）。
///
/// 边界不能用「最后一条 Assistant 的位置」：一次工具循环会插入多条 Assistant，
/// 那样本轮图片在循环第二轮就被折叠，请求前缀中途改变导致缓存全失效，
/// 模型在后续轮次也再也看不到本轮图片。
pub fn restore_image_data(history_snapshot: &mut Vec<Message>, current_turn_start: usize) {
    // 往轮：折叠为文本摘要
    for msg in history_snapshot.iter_mut().take(current_turn_start) {
        if let Message::User { content } = msg {
            if let Content::Multiple(blocks) = content {
                let mut new_blocks = Vec::with_capacity(blocks.len());
                for block in blocks.drain(..) {
                    match block {
                        ContentBlock::Image { source } => {
                            new_blocks.push(ContentBlock::Text {
                                text: format!("[图片: {}]", source.media_type),
                            });
                        }
                        other => new_blocks.push(other),
                    }
                }
                *blocks = new_blocks;
            }
        }
    }

    // 本轮及之后：按需从磁盘恢复 base64
    for msg in history_snapshot.iter_mut().skip(current_turn_start) {
        if let Message::User { content } = msg {
            if let Content::Multiple(blocks) = content {
                for block in blocks.iter_mut() {
                    if let ContentBlock::Image { source } = block {
                        if source.data.is_empty() {
                            if let Some(ref fp) = source.file_path {
                                if let Some(data) = crate::core::session::load_image_data(fp) {
                                    source.data = data;
                                }
                            }
                        }
                    }
                }
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

        let idx = inject_user_message(&mut session, msg, &images, "", &mut sid);
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

        inject_user_message(&mut session, msg, &images, "", &mut sid);
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
    fn inject_user_message_puts_context_block_before_user_text() {
        let mut session = make_session();
        let images: Option<Vec<String>> = None;
        let mut sid = Some("test-session".to_string());

        inject_user_message(
            &mut session,
            "看看这个 bug",
            &images,
            "<intent>ACTION</intent>\n<global_memory>\n偏好简洁\n</global_memory>",
            &mut sid,
        );

        match &session.messages[0] {
            Message::User { content: Content::Multiple(blocks) } => {
                assert_eq!(blocks.len(), 2, "应该是 context + text 两个块");
                match &blocks[0] {
                    ContentBlock::Context { text } => assert!(text.contains("<intent>ACTION")),
                    other => panic!("第一个块应为 Context，实际: {:?}", other),
                }
                match &blocks[1] {
                    ContentBlock::Text { text } => assert_eq!(text, "看看这个 bug"),
                    other => panic!("第二个块应为 Text，实际: {:?}", other),
                }
            }
            other => panic!("Expected multiple content, got {:?}", other),
        }
    }

    #[test]
    fn inject_user_message_skips_empty_context() {
        let mut session = make_session();
        let images: Option<Vec<String>> = None;
        let mut sid = Some("test-session".to_string());

        // CHAT 意图下 ctx 为空串：不应产生 Context 块，保持单文本形态
        inject_user_message(&mut session, "你好", &images, "", &mut sid);
        match &session.messages[0] {
            Message::User { content } => match content {
                Content::Single(text) => assert_eq!(text, "你好"),
                other => panic!("空 ctx 时应保持 Single，实际: {:?}", other),
            },
            _ => panic!("Expected user message"),
        }
    }

    fn image_source(data: &str) -> ImageSource {
        ImageSource {
            r#type: "base64".to_string(),
            media_type: "image/png".to_string(),
            data: data.to_string(),
            file_path: None,
        }
    }

    fn user_with_image(data: &str) -> Message {
        Message::User {
            content: Content::Multiple(vec![ContentBlock::Image {
                source: image_source(data),
            }]),
        }
    }

    fn first_block(msg: &Message) -> &ContentBlock {
        match msg {
            Message::User {
                content: Content::Multiple(blocks),
            } => blocks.first().expect("empty blocks"),
            other => panic!("not a multi-block user message: {:?}", other),
        }
    }

    #[test]
    fn restore_image_data_collapses_previous_turns_only() {
        let mut history = vec![
            user_with_image("OLD"),
            Message::Assistant {
                content: Content::Single("ok".to_string()),
            },
            user_with_image("NEW"),
        ];

        // 本轮用户消息是下标 2
        restore_image_data(&mut history, 2);

        match first_block(&history[0]) {
            ContentBlock::Text { text } => assert_eq!(text, "[图片: image/png]"),
            other => panic!("往轮图片应折叠为文本，实际: {:?}", other),
        }
        match first_block(&history[2]) {
            ContentBlock::Image { source } => assert_eq!(source.data, "NEW"),
            other => panic!("本轮图片应保留 base64，实际: {:?}", other),
        }
    }

    #[test]
    fn restore_image_data_keeps_current_turn_image_during_tool_loop() {
        // 工具循环中途：User(图) → Assistant(tool_use) → User(tool_result)
        // 旧实现用「最后一条 Assistant 的位置」当界，会在这一轮就把图片折叠掉
        let mut history = vec![
            user_with_image("CURRENT"),
            Message::Assistant {
                content: Content::Single("calling tool".to_string()),
            },
            Message::User {
                content: Content::Single("tool result".to_string()),
            },
        ];

        restore_image_data(&mut history, 0);

        match first_block(&history[0]) {
            ContentBlock::Image { source } => assert_eq!(source.data, "CURRENT"),
            other => panic!("本轮图片不应在工具循环中被折叠，实际: {:?}", other),
        }
    }

    #[test]
    fn restore_image_data_fallback_collapses_everything() {
        // 兜底边界（本轮消息没找到）时把所有图片当往轮，不会 panic
        let mut history = vec![user_with_image("X")];
        restore_image_data(&mut history, 1);
        match first_block(&history[0]) {
            ContentBlock::Text { text } => assert_eq!(text, "[图片: image/png]"),
            other => panic!("应折叠为文本，实际: {:?}", other),
        }
    }
}
