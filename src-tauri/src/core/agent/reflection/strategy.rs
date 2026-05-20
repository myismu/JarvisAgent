use crate::infra::llm::api_client;
use crate::infra::llm::api_format::ApiFormat;
use crate::infra::types::error::ApiError;
use crate::infra::types::models::{Content, ContentBlock, Message};

use super::prompt;
use super::ReflectionJudgment;

/// 最大会话累计反思次数
const MAX_TOTAL_REFLECTIONS: usize = 10;
/// 连续 NO 上限，超过则停止反思
const MAX_CONSECUTIVE_NOS: usize = 3;
/// 反思触发的最小循环次数（前 N 轮探索阶段不触发）
const MIN_LOOP_FOR_REFLECTION: usize = 2;

/// 不确定性关键词（smart 模式检测 thinking 文本）
const UNCERTAINTY_KEYWORDS: &[&str] = &[
    "不确定",
    "不太确定",
    "试一下",
    "也许",
    "可能",
    "应该",
    "似乎",
    "不太确定",
    "不确定是否",
    "maybe",
    "perhaps",
    "try",
    "might",
    "could",
    "not sure",
    "probably",
    "let me try",
    "i think",
];

/// 对话类/系统类工具名称（这些工具的结果不适合做语义审查）
const CONVERSATIONAL_TOOLS: &[&str] = &[
    "ReadMemory",
    "WriteMemory",
    "SearchMemory",
    "CompactConversation",
];

/// 子 Agent 调度工具（完成代表一个独立工作单元结束）
const SUBAGENT_TOOLS: &[&str] = &[
    "RunSubagent",
    "RunSubagentsSequentially",
];

/// 变更类工具（产生副作用的操作，完成后值得审查）
const MUTATION_TOOLS: &[&str] = &[
    "WriteFile",
    "EditFile",
    "ApplyPatch",
    "DeleteFile",
    "RenameFile",
    "RunCommand",
    "EditNotebook",
    "ProposePlan",
];

/// 判断是否需要触发反思审查
///
/// 按优先级检查：模式 → 硬跳过条件 → 模式特定条件
pub fn should_reflect(
    reflection_mode: &str,
    loop_count: usize,
    total_reflections: usize,
    consecutive_nos: usize,
    tool_names: &[String],
    thinking_text: &str,
) -> bool {
    // off 模式直接跳过
    if reflection_mode == "off" {
        return false;
    }

    // 硬跳过条件（所有模式）
    if loop_count < MIN_LOOP_FOR_REFLECTION {
        return false;
    }
    if total_reflections >= MAX_TOTAL_REFLECTIONS {
        return false;
    }
    if consecutive_nos >= MAX_CONSECUTIVE_NOS {
        return false;
    }

    // 对话类工具跳过
    if tool_names
        .iter()
        .any(|name| CONVERSATIONAL_TOOLS.iter().any(|ct| name.contains(ct)))
    {
        return false;
    }

    match reflection_mode {
        "always" => should_reflect_always(tool_names),
        "smart" => should_reflect_smart(thinking_text),
        _ => false,
    }
}

/// always 模式：基于语义边界触发
///
/// 触发条件（满足任一）：
/// 1. 子 Agent 完成 — 独立工作单元结束，审查结果质量
/// 2. 变更类工具执行 — 产生副作用的操作，及时审查
/// 3. 上下文压缩后 — context 被压缩，审查当前进展
///
/// 读操作不触发：无副作用、可逆、由 smart 模式的不确定性检测兜底
fn should_reflect_always(tool_names: &[String]) -> bool {
    // 子 Agent 完成
    if tool_names
        .iter()
        .any(|name| SUBAGENT_TOOLS.iter().any(|st| name.contains(st)))
    {
        return true;
    }

    // 变更类工具执行
    if tool_names
        .iter()
        .any(|name| MUTATION_TOOLS.iter().any(|mt| name.contains(mt)))
    {
        return true;
    }

    false
}

/// smart 模式：thinking 文本不确定性检测
fn should_reflect_smart(thinking_text: &str) -> bool {
    let lower_thinking = thinking_text.to_lowercase();
    UNCERTAINTY_KEYWORDS
        .iter()
        .any(|kw| lower_thinking.contains(kw))
}

/// 执行反思审查
///
/// 构建审查请求：session history + 本轮工具调用/结果 + 审查 prompt，
/// 调用 LLM 进行非流式判断。
pub async fn execute_review(
    client: &reqwest::Client,
    api_key: &str,
    base_url: &str,
    model_id: &str,
    api_format: ApiFormat,
    session_messages: &[Message],
    user_request: &str,
    current_assistant_blocks: &[ContentBlock],
    current_tool_results: &[ContentBlock],
) -> Result<ReflectionJudgment, ApiError> {
    // 构建审查消息序列：session history + 本轮 assistant 工具调用 + 本轮工具结果 + 审查 prompt
    let mut review_messages: Vec<Message> = session_messages.to_vec();

    // 追加本轮 assistant 的工具调用（如果不在 session 中）
    if !current_assistant_blocks.is_empty() {
        review_messages.push(Message::Assistant {
            content: Content::Multiple(current_assistant_blocks.to_vec()),
        });
    }

    // 追加本轮工具结果
    if !current_tool_results.is_empty() {
        review_messages.push(Message::User {
            content: Content::Multiple(current_tool_results.to_vec()),
        });
    }

    // 构建审查 prompt：提取工具名称
    let tool_names: Vec<String> = current_assistant_blocks
        .iter()
        .filter_map(|block| {
            if let ContentBlock::ToolUse { name, .. } = block {
                Some(name.clone())
            } else {
                None
            }
        })
        .collect();

    let tool_names_str = if tool_names.is_empty() {
        "未知".to_string()
    } else {
        tool_names.join("、")
    };

    let review_prompt = format!(
        "用户的需求是：{}\n\n\
         你刚才调用了 {} 工具。\n\n\
         请判断：这些工具的执行结果是否有助于满足用户需求？\n\
         - 如果结果正确且推进了任务，回复 OK\n\
         - 如果结果有误、偏离需求或遗漏关键信息，回复 NO: <原因> | 建议: <修正方案>",
        user_request, tool_names_str
    );

    review_messages.push(Message::User {
        content: Content::Single(review_prompt),
    });

    // 调用审查 Agent（非流式，不带 tools）
    let response = api_client::call_llm_with_messages(
        client,
        api_key,
        base_url,
        model_id,
        api_format,
        prompt::get_review_system_prompt(),
        review_messages,
        512, // 审查回复不需要太长
    )
    .await?;

    // 解析判断结果
    Ok(parse_reflection_response(&response))
}

/// 解析审查 Agent 的回复
///
/// 格式：
/// - `OK` → Judgment::Ok
/// - `NO: <reason> | 建议: <suggestion>` → Judgment::NotOk
fn parse_reflection_response(response: &str) -> ReflectionJudgment {
    let trimmed = response.trim();

    // 纯 OK
    if trimmed.eq_ignore_ascii_case("OK") || trimmed == "OK." {
        return ReflectionJudgment::Ok;
    }

    // NO: <reason> | 建议: <suggestion>
    if let Some(no_part) = trimmed.strip_prefix("NO:").or_else(|| trimmed.strip_prefix("NO :")) {
        let no_part = no_part.trim();
        // 兼容 "|建议:" 和 "| 建议:" 两种格式
        if let Some((reason, suggestion)) = no_part
            .split_once("|建议:")
            .or_else(|| no_part.split_once("| 建议:"))
        {
            let reason = reason
                .trim()
                .strip_prefix("建议:")
                .unwrap_or(reason)
                .trim();
            return ReflectionJudgment::NotOk {
                reason: reason.to_string(),
                suggestion: suggestion.trim().to_string(),
            };
        }
        // 只有 NO: <reason>，没有建议
        return ReflectionJudgment::NotOk {
            reason: no_part.to_string(),
            suggestion: "请重新检查工具调用参数和预期结果".to_string(),
        };
    }

    // 以 NO 开头（无冒号）
    if trimmed.to_uppercase().starts_with("NO") {
        let rest = trimmed[2..].trim().trim_start_matches(':').trim();
        if !rest.is_empty() {
            return ReflectionJudgment::NotOk {
                reason: rest.to_string(),
                suggestion: "请重新检查工具调用参数和预期结果".to_string(),
            };
        }
    }

    // 无法识别的回复，默认为 OK（避免误判导致循环）
    ReflectionJudgment::Ok
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_ok() {
        assert!(matches!(
            parse_reflection_response("OK"),
            ReflectionJudgment::Ok
        ));
        assert!(matches!(
            parse_reflection_response("OK."),
            ReflectionJudgment::Ok
        ));
        assert!(matches!(
            parse_reflection_response("  OK  "),
            ReflectionJudgment::Ok
        ));
    }

    #[test]
    fn test_parse_no_with_suggestion() {
        match parse_reflection_response("NO: 文件路径错误 | 建议: 使用绝对路径") {
            ReflectionJudgment::NotOk { reason, suggestion } => {
                assert_eq!(reason, "文件路径错误");
                assert_eq!(suggestion, "使用绝对路径");
            }
            _ => panic!("Expected NotOk"),
        }
    }

    #[test]
    fn test_parse_no_without_suggestion() {
        match parse_reflection_response("NO: 结果不完整") {
            ReflectionJudgment::NotOk { reason, .. } => {
                assert_eq!(reason, "结果不完整");
            }
            _ => panic!("Expected NotOk"),
        }
    }

    #[test]
    fn test_should_reflect_off() {
        assert!(!should_reflect("off", 5, 0, 0, &["EditFile".into()], "不确定"));
    }

    #[test]
    fn test_should_reflect_always_subagent() {
        assert!(should_reflect("always", 3, 0, 0, &["RunSubagent".into()], ""));
        assert!(should_reflect("always", 3, 0, 0, &["RunSubagentsSequentially".into()], ""));
    }

    #[test]
    fn test_should_reflect_always_mutation() {
        assert!(should_reflect("always", 3, 0, 0, &["WriteFile".into()], ""));
        assert!(should_reflect("always", 3, 0, 0, &["EditFile".into()], ""));
        assert!(should_reflect("always", 3, 0, 0, &["DeleteFile".into()], ""));
        assert!(should_reflect("always", 3, 0, 0, &["RunCommand".into()], ""));
        assert!(should_reflect("always", 3, 0, 0, &["ProposePlan".into()], ""));
    }

    #[test]
    fn test_should_reflect_always_read_only_no_trigger() {
        // 纯读取 → 不触发
        assert!(!should_reflect("always", 3, 0, 0, &["ReadFile".into()], ""));
        assert!(!should_reflect("always", 3, 0, 0, &["ListDirectory".into()], ""));
        assert!(!should_reflect("always", 3, 0, 0, &["SearchRepo".into()], ""));
        assert!(!should_reflect("always", 3, 0, 0, &["FindFiles".into()], ""));
    }

    #[test]
    fn test_should_reflect_smart_keyword() {
        assert!(should_reflect(
            "smart", 3, 0, 0,
            &["ReadFile".into()],
            "我不确定这个结果是否正确"
        ));
        assert!(!should_reflect(
            "smart", 3, 0, 0,
            &["ReadFile".into()],
            "文件内容已成功读取"
        ));
    }

    #[test]
    fn test_should_reflect_skip_conversational() {
        assert!(!should_reflect(
            "always", 3, 0, 0,
            &["ReadMemory".into()], ""
        ));
    }

    #[test]
    fn test_should_reflect_max_total() {
        assert!(!should_reflect(
            "always", 3, 10, 0,
            &["EditFile".into()], ""
        ));
    }

    #[test]
    fn test_should_reflect_consecutive_nos() {
        assert!(!should_reflect(
            "always", 3, 5, 3,
            &["EditFile".into()], ""
        ));
    }

    #[test]
    fn test_should_reflect_always_multiple_tools() {
        // 一轮中既有读又有写 → 变更工具触发
        assert!(should_reflect(
            "always", 3, 0, 0,
            &["ReadFile".into(), "EditFile".into()], ""
        ));
    }
}
