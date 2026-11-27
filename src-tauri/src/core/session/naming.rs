//! # naming.rs — 会话自动命名领域服务
//!
//! 从会话消息提取核心主题与最终助手产出，调用轻量 utility 模型生成描述性标题，
//! 并将本次请求的 token 用量记入 `UsageSource::SessionName` 账本后更新会话标题。
//!
//! ## Key Exports
//! - `auto_name_session()`: 异步生成并落库会话标题，同时广播 `session-renamed` 事件
//!
//! ## Dependencies
//! - Internal: `crate::core::session`, `crate::infra::config::config`, `crate::infra::llm::api_client`
//! - External: `tauri`

use crate::core::session;
use crate::infra::llm::api_client;
use crate::infra::types::models::{Content, ContentBlock, Message, SessionMemory};
use tauri::Emitter;

/// 使用 utility 模型自动生成会话名称。
///
/// 提取前几条用户消息 + 助手最后一条文本，过滤掉系统注入的上下文消息。
/// 对 LLM 返回结果做长度与原文复述校验。
///
/// 命名请求的 token 会以 `UsageSource::SessionName` 落一行用量明细（`run_id` 留空）；
/// 记账失败只打日志，不影响取名与 rename。
pub async fn auto_name_session(
    app: tauri::AppHandle,
    session_id: String,
    memory: SessionMemory,
) -> Result<(), String> {
    if memory.messages.is_empty() {
        return Ok(());
    }

    // 提取用于命名的摘要文本：取用户消息 + 最后一条助手消息（反映实际产出），跳过系统注入。
    // 不再提前 break —— 遍历全部消息以获取最后一条助手回复，首条助手回复往往是「我来做X」
    // 这种流程性表述，而最后一条更可能包含实际成果。
    let mut user_texts: Vec<String> = Vec::new();
    let mut assistant_text: Option<String> = None;
    for msg in &memory.messages {
        match msg {
            Message::User { content } => {
                // 动态上下文是独立的 Context 块，extract_plain_text 只取 Text 块，
                // 所以这里拿到的就是用户原文，无需再做字符串手术
                let text = extract_plain_text(content);
                if !text.trim().is_empty() {
                    user_texts.push(text);
                }
            }
            Message::Assistant { content } => {
                // 始终覆盖，取最后一条助手消息（更可能描述实际产出而非流程承诺）
                if let Content::Multiple(blocks) = content {
                    for b in blocks {
                        if let ContentBlock::Text { text } = b {
                            let t = text.trim().to_string();
                            if !t.is_empty() {
                                assistant_text = Some(t);
                                break;
                            }
                        }
                    }
                }
            }
        }
    }

    if user_texts.is_empty() {
        return Ok(());
    }

    // 拼成简洁摘要：用户说的话 + 助手简要
    let mut context = String::new();
    for (i, t) in user_texts.iter().enumerate() {
        let short: String = t.chars().take(300).collect();
        context.push_str(&format!("用户{}: {}\n", i + 1, short));
    }
    if let Some(ref at) = assistant_text {
        let short: String = at.chars().take(120).collect();
        context.push_str(&format!("助手: {}\n", short));
    }

    let summary_prompt = format!(
        "为以上对话生成一个描述性标题，让用户以后能一眼回忆起对话的核心主题和关键成果。\n\
         \n\
         规则：\n\
         - 标题应概括「核心主题 + 关键产出」，而非描述「当前处于哪个流程阶段」\n\
         - 避免使用「审批」「讨论中」「进行中」「审查」「确认」「评估」等流程节点词\n\
         - 长度 3~20 字，不含引号、标点和解释\n\
         - 只输出标题本身，不要输出任何其他内容\n\
         \n\
         正确示例（描述内容与成果）：\n\
         - 任务与知识库管理系统架构设计\n\
         - 支付模块空指针异常修复\n\
         - 用户认证 JWT 方案设计\n\
         - 搜索结果分页改用游标方案\n\
         \n\
         错误示例（描述流程阶段）：\n\
         - 实施方案审批 → 看不出具体项目\n\
         - 代码审查 → 不知道审查什么\n\
         - 需求讨论 → 不知道讨论什么\n\
         \n\
         对话内容：\n{}",
        context
    );

    let cfg = crate::infra::config::config::load_config();
    let agent_cfg = cfg.active_config();

    // 辅助调用统一走带超时的客户端
    let client = api_client::build_utility_client();
    let completion = api_client::call_llm_simple(
        &client,
        &agent_cfg.api_key,
        &agent_cfg.base_url,
        &agent_cfg.utility_model,
        agent_cfg.api_format_enum(),
        "你是一个会话标题生成器。为对话生成描述性标题，概括核心主题与关键成果，避免流程节点词。只输出标题文本。",
        &summary_prompt,
        30,
    )
    .await;

    // 记账口径：命名请求是一次真实的 utility 模型调用，token 确实花掉了，
    // 所以必须落一行 `session_name` 明细（`run_id = NULL` 表示不属于任何一轮主 loop）。
    if let Ok(completion) = completion.as_ref() {
        let record =
            session::RequestUsageRecord::new(session_id.clone(), session::UsageSource::SessionName)
                .model_id(agent_cfg.utility_model.clone());
        if let Err(err) = session::usage::record_llm_usage(&app, record, completion.usage.as_ref())
        {
            eprintln!("[JARVIS] 写入会话命名用量明细失败: {}", err);
        }
    }

    // 记账之后才取正文：厂商哪怕回了空正文，这次 token 也已经是事实。
    let title = completion
        .map(|completion| completion.text)
        .unwrap_or_default();

    let title = title
        .trim()
        .trim_matches('"')
        .trim_matches('\'')
        .trim_matches('「')
        .trim_matches('」')
        .to_string();

    // 校验：标题必须在 2~30 字之间，不是原文复述
    let char_count = title.chars().count();
    if (2..=30).contains(&char_count) {
        let is_verbatim = user_texts
            .iter()
            .any(|t| t.contains(&title) && t.len() < title.len() * 3)
            || assistant_text
                .as_ref()
                .map(|t| t.contains(&title) && t.len() < title.len() * 3)
                .unwrap_or(false);
        if !is_verbatim {
            let _ = session::rename_session(&session_id, &title, true);
            let _ = app.emit("session-renamed", ());
            return Ok(());
        }
    }

    Ok(())
}

/// 从 Content 中提取纯文本
fn extract_plain_text(content: &Content) -> String {
    match content {
        Content::Single(s) => s.clone(),
        Content::Multiple(blocks) => blocks
            .iter()
            .filter_map(|b| {
                if let ContentBlock::Text { text } = b {
                    Some(text.clone())
                } else {
                    None
                }
            })
            .collect::<Vec<_>>()
            .join(" "),
    }
}
