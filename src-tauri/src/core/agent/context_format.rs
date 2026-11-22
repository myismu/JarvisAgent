//! # context_format.rs — 对话上下文「可读文本」的唯一实现
//!
//! ## 职责
//! 把 `Vec<Message>` 渲染成给人看的纯文本，用于两处展示：
//! 1. `pipeline::build_context_estimate` 组装 `messages` 分区的 `content`（上下文浮层）；
//! 2. `command::session::get_session_context_snapshot` 从内存态重建 `Session Messages` 段。
//!
//! ## 为什么必须只有一份实现
//! 这两处展示的是**同一份历史**。曾经它们各自持有一份内联副本，口径不同：
//! - `pipeline` 版：User 分支只认 Text / ToolResult / Image / Context，
//!   Assistant 分支只认 Text / ToolUse / Thinking，两侧互补但都不完整；
//! - `command` 版：不分角色统一渲染 ToolUse / Thinking，但同样漏掉另一侧的组合，
//!   且截断阈值不同（ToolUse 120 字符、ToolResult 2 行、Thinking 80 字符）。
//!
//! 结果是一次压缩或一次刷新之后，浮层里看到的文本与实时视图不一致 ——
//! 同一份数据出两种排版，排查问题时无法互相对照。所以合并为唯一实现：
//! 不区分角色，两种消息都渲染全部六类块（Text / ToolUse / ToolResult / Thinking /
//! Image / Context），保证任何块都不会被静默丢掉。
//!
//! ## 为什么不再截断
//! 旧实现为浮层做了预览式截断：`tool_result` 只留前 3 行（另一份是 2 行）、
//! `tool_use` 的 input JSON 截到 200 字符（另一份 120）、`thinking` 截到 80 字符
//! 并把换行压成空格。那套阈值是为「详情收起状态下的 80 字摘要」设计的；
//! 现在前端改为展开才渲染（`v-if` + 懒加载），截断只会让人看不到真实内容，
//! 还会让「浮层文本」与「实际发出去的 payload」差得更远。
//! 因此这里输出**完整内容**：tool_result 全量、input JSON 全量、thinking 全量且保留换行。
//!
//! 注意：本函数的输出不是 token 计数口径。`build_context_estimate` 的
//! `chars` / `estimated_tokens` 走完整 JSON（`section_with_raw_counting_full`），
//! 本函数只负责「给人看」。修改此处不会、也不应该改变任何 token 估算结果。
//!
//! ## 前端依赖的标记（改名会打断前端定位）
//! - 角色行：`[User] (msg N)` / `[Assistant] (msg N)`
//! - 上下文块：`  ← [Context]`
//! - 图片块：`  ← [Image]`
//! - 工具调用：`  → ToolCall: {name}({input_json})`
//! - 工具结果：`  ← ToolResult: {tool_use_id 尾 12 位}`
//! - 思考块：`  … Thinking: {thinking}`

use crate::infra::types::models::{Content, ContentBlock, Message};

/// 把消息列表渲染为人类可读的对话文本（完整内容，不截断）。
///
/// 每条消息先输出角色行，再按块在其原始顺序下渲染，最后补一个空行分隔。
/// 空白（trim 后为空）的 Text / Context 块不产生噪音行。
pub fn format_messages_readable(messages: &[Message]) -> String {
    let mut out = String::new();

    for (i, msg) in messages.iter().enumerate() {
        let idx = i + 1;
        // 角色前缀带序号：前端靠 "(msg N)" 把浮层文本与消息列表对齐
        match msg {
            Message::User { .. } => out.push_str(&format!("[User] (msg {})\n", idx)),
            Message::Assistant { .. } => out.push_str(&format!("[Assistant] (msg {})\n", idx)),
        }

        // 取出内容后统一处理：**不按角色区分块类型**。
        // 用户消息里可以带 ToolResult / Image / Context，助手消息里也可以带 ToolResult
        // （工具异常回灌等路径），任何「按角色白名单」都会丢块。
        let content = match msg {
            Message::User { content } | Message::Assistant { content } => content,
        };

        match content {
            Content::Single(text) => {
                let trimmed = text.trim();
                if !trimmed.is_empty() {
                    out.push_str(trimmed);
                    out.push('\n');
                }
            }
            Content::Multiple(blocks) => {
                for block in blocks {
                    render_block(&mut out, block);
                }
            }
        }

        out.push('\n');
    }

    out
}

/// 渲染单个内容块。穷尽 match：新增 `ContentBlock` 变体时编译期就会报错，
/// 避免再次出现「新块类型落进 `_ => {}` 被静默丢弃」的估算偏低问题。
fn render_block(out: &mut String, block: &ContentBlock) {
    match block {
        ContentBlock::Text { text } => {
            let trimmed = text.trim();
            if !trimmed.is_empty() {
                out.push_str(trimmed);
                out.push('\n');
            }
        }
        ContentBlock::ToolUse { name, input, .. } => {
            // 完整 input JSON：不截断，长参数（如整段文件内容）也要看得见
            let input_str = serde_json::to_string(input).unwrap_or_default();
            out.push_str(&format!("  → ToolCall: {}({})\n", name, input_str));
        }
        ContentBlock::ToolResult {
            tool_use_id,
            content,
        } => {
            // 只对 id 取尾部 12 位做短标识，正文全量输出
            let short_id = &tool_use_id[tool_use_id.len().saturating_sub(12)..];
            out.push_str(&format!("  ← ToolResult: {}\n", short_id));
            out.push_str(content);
            // 补换行，保证下一条输出不粘在工具输出末尾
            if !content.ends_with('\n') {
                out.push('\n');
            }
        }
        ContentBlock::Thinking { thinking, .. } => {
            // 保留换行：旧实现为 80 字预览把 \n 压成空格，全文展示必须保留原结构
            out.push_str(&format!("  … Thinking: {}\n", thinking));
        }
        ContentBlock::Image { .. } => {
            // 图片本体是 base64，不展开，只留占位标记
            out.push_str("  ← [Image]\n");
        }
        ContentBlock::Context { text } => {
            // 动态上下文块（意图标签 / 能力边界 / 项目结构 / 用户画像）。
            // 出网前由 `materialize_context_blocks_for_wire` 翻译成普通 Text 一并发出，
            // 所以它**真实占用** prompt token，这里必须原样渲染，不能丢。
            let trimmed = text.trim();
            if !trimmed.is_empty() {
                out.push_str("  ← [Context]\n");
                out.push_str(trimmed);
                out.push('\n');
            }
        }
    }
}
