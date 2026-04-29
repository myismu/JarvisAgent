//! 调试日志模块
//!
//! 提供代理运行时的调试日志记录功能，包括：
//! - 请求/响应日志（用于调试 API 交互）
//! - 思考过程日志（记录 LLM 的推理和工具调用）
//! - 意图分类日志（记录用户意图识别结果）
//! - SSE 流式事件日志（调试流式响应）

use std::fs::OpenOptions;
use std::io::Write;
use std::path::PathBuf;

use crate::core::constants::{FILE_AGENT_LOOP_DEBUG, FILE_THOUGHTS_LOG};

/// 调试日志记录器
///
/// 管理两个日志文件：
/// - `agent_loop_debug.log`: 记录原始请求/响应 JSON
/// - `thoughts.log`: 记录结构化的思考过程和决策
pub struct DebugLogger {
    thoughts_path: PathBuf,
    debug_path: PathBuf,
}

impl DebugLogger {
    /// 创建新的日志记录器实例
    ///
    /// 自动创建日志目录（如果不存在）
    pub fn new() -> Self {
        let log_dir = crate::core::data_paths::logs_dir();
        Self {
            thoughts_path: log_dir.join(FILE_THOUGHTS_LOG),
            debug_path: log_dir.join(FILE_AGENT_LOOP_DEBUG),
        }
    }

    /// 将请求摘要输出到终端（用于实时监控）
    pub fn log_request_to_terminal(
        &self,
        agent_type: &str,
        _loop_count: usize,
        request_json: &str,
    ) {
        println!(
            "[{}] request logged ({} bytes)",
            agent_type,
            request_json.len()
        );
    }

    /// 将完整请求 JSON 写入调试日志文件
    pub fn log_request_to_file(&self, agent_type: &str, loop_count: usize, request_json: &str) {
        if let Ok(mut file) = OpenOptions::new()
            .create(true)
            .append(true)
            .open(&self.debug_path)
        {
            let _ = writeln!(
                file,
                "\n{} [{}] LOOP {} - REQUEST {}\n{}\n",
                "=".repeat(30),
                agent_type,
                loop_count,
                "=".repeat(30),
                request_json
            );
        }
    }

    /// 将完整响应 JSON 写入调试日志文件
    pub fn log_response_to_file(&self, agent_type: &str, loop_count: usize, response_json: &str) {
        if let Ok(mut file) = OpenOptions::new()
            .create(true)
            .append(true)
            .open(&self.debug_path)
        {
            let _ = writeln!(
                file,
                "\n{} [{}] LOOP {} - RESPONSE {}\n{}\n",
                "=".repeat(30),
                agent_type,
                loop_count,
                "=".repeat(30),
                response_json
            );
        }
    }

    /// 记录原始 SSE 事件到调试日志（流式调试用）
    pub fn log_raw_sse_event(&self, loop_count: usize, raw_data: &str) {
        if let Ok(mut file) = OpenOptions::new()
            .create(true)
            .append(true)
            .open(&self.debug_path)
        {
            let truncated: String = if raw_data.len() > 2000 {
                format!(
                    "{}...(truncated, {} chars)",
                    raw_data.chars().take(2000).collect::<String>(),
                    raw_data.len()
                )
            } else {
                raw_data.to_string()
            };
            let _ = writeln!(file, "[LOOP {} SSE] {}", loop_count, truncated);
        }
    }

    /// 记录代理思考过程（包括 thinking、工具调用、token 使用）
    pub fn log_thoughts(
        &self,
        agent_type: &str,
        loop_count: usize,
        thinking: &str,
        response_text: &str,
        tool_calls: &[(String, String)],
        input_tokens: u64,
        output_tokens: u64,
    ) {
        if let Ok(mut file) = OpenOptions::new()
            .create(true)
            .append(true)
            .open(&self.thoughts_path)
        {
            let mut content = format!("\n## [{}] Loop {}\n", agent_type, loop_count);

            if !thinking.trim().is_empty() {
                content.push_str(&format!("### Thinking\n{}\n\n", thinking.trim()));
            }

            if tool_calls.is_empty() {
                content.push_str("### Decision\nReady to answer the user.\n");
                if !response_text.trim().is_empty() {
                    content.push_str(&format!("\n### Response\n{}\n", response_text.trim()));
                }
            } else {
                content.push_str("### Tool calls\n");
                for (name, args) in tool_calls {
                    content.push_str(&format!("- Tool: `{}`\n  Args: `{}`\n", name, args));
                }
            }

            content.push_str(&format!(
                "\n### Token usage\n- Input: {} | Output: {}\n",
                input_tokens, output_tokens
            ));

            let _ = writeln!(file, "{}\n---\n", content);
        }
    }

    /// 记录意图分类结果（包括分类方法和检测到的意图）
    pub fn log_intent_classifier(
        &self,
        user_input: &str,
        method: &str,
        request_json: &str,
        llm_response: &str,
        detected_intent: &str,
    ) {
        println!(
            "[INTENT] {} => {} ({})",
            method,
            detected_intent,
            user_input.chars().take(80).collect::<String>()
        );

        if let Ok(mut file) = OpenOptions::new()
            .create(true)
            .append(true)
            .open(&self.thoughts_path)
        {
            let mut content = format!(
                "\n## [INTENT CLASSIFIER]\n**User input**: {}\n**Method**: {}\n**Detected intent**: {}\n",
                user_input.chars().take(200).collect::<String>(),
                method,
                detected_intent
            );
            if method == "LLM" {
                content.push_str(&format!(
                    "**LLM raw response**: {}\n\n**Request**:\n```json\n{}\n```\n",
                    llm_response, request_json
                ));
            }
            let _ = writeln!(file, "{}\n---\n", content);
        }
    }

    /// 记录记忆代理的请求和响应
    pub fn log_memory_agent(&self, request_json: &str, response_summary: &str) {
        println!(
            "[MEMORY AGENT] {}",
            response_summary.chars().take(160).collect::<String>()
        );

        if let Ok(mut file) = OpenOptions::new()
            .create(true)
            .append(true)
            .open(&self.thoughts_path)
        {
            let _ = writeln!(
                file,
                "\n## [MEMORY AGENT]\n**Request**:\n```json\n{}\n```\n\n**Response**: {}\n\n---\n",
                request_json, response_summary
            );
        }
    }

    /// 记录会话结束时的 token 使用汇总
    pub fn log_session_summary(&self, input_tokens: u64, output_tokens: u64, status: &str) {
        if let Ok(mut file) = OpenOptions::new()
            .create(true)
            .append(true)
            .open(&self.thoughts_path)
        {
            let _ = writeln!(
                file,
                "\n## [SESSION SUMMARY]\n**Status**: {}\n**Total token usage**: input {} | output {} | total {}\n\n---\n",
                status,
                input_tokens,
                output_tokens,
                input_tokens + output_tokens
            );
        }
    }
}

impl Default for DebugLogger {
    fn default() -> Self {
        Self::new()
    }
}
