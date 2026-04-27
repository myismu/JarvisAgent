use std::fs::{self, OpenOptions};
use std::io::Write;
use std::path::PathBuf;

use crate::core::constants::{DIR_LOGS, FILE_AGENT_LOOP_DEBUG, FILE_THOUGHTS_LOG};
use crate::get_agent_home;

pub struct DebugLogger {
    thoughts_path: PathBuf,
    debug_path: PathBuf,
}

impl DebugLogger {
    pub fn new() -> Self {
        let log_dir = get_agent_home().join(DIR_LOGS);
        if !log_dir.exists() {
            let _ = fs::create_dir_all(&log_dir);
        }
        Self {
            thoughts_path: log_dir.join(FILE_THOUGHTS_LOG),
            debug_path: log_dir.join(FILE_AGENT_LOOP_DEBUG),
        }
    }

    pub fn log_request_to_terminal(&self, agent_type: &str, _loop_count: usize, request_json: &str) {
        println!("[{}] request logged ({} bytes)", agent_type, request_json.len());
    }

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
                request_json,
                response_summary
            );
        }
    }

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
