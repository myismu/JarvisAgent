//! # pipeline.rs — Agent 主循环流水线
//!
//! 实现 Agent 的 5 阶段执行流水线：初始化 → 意图验证 → 上下文构建 → 主循环 → 收尾。
//! 主循环阶段包含压缩检查、API 调用、流式处理、工具执行等完整 Agent Loop 逻辑。
//!
//! ## 关键导出
//! - `run_pipeline()`: 主流程入口，依次执行 5 个阶段并返回 `JarvisResult`
//!
//! ## 依赖
//! - Internal: `crate::core::orchestration::agent_runs`, `crate::core::llm::api_client`, `crate::core::config::AgentConfig`, `crate::core::intent`, `crate::core::session::memory`, `crate::core::tools`
//! - External: `eventsource_stream`, `serde_json`, `tauri`, `tokio_util`, `reqwest`
//!
//! ## 约束
//! - 循环次数受 `MAX_AGENT_LOOP_BEFORE_CONFIRM` 和 `MAX_AGENT_LOOP_ABSOLUTE` 常量限制
//! - 取消令牌（`CancellationToken`）贯穿全流程，支持用户随时中断

use std::collections::HashMap;
use std::time::{SystemTime, UNIX_EPOCH};
use std::sync::Arc;

use eventsource_stream::Eventsource;
use serde_json::json;
use tauri::Emitter;

use crate::core::config::AgentConfig;
use crate::core::error::AgentError;
use crate::core::infra::debug_logger;
use crate::core::infra::prompts::*;
use crate::core::intent;
use crate::core::llm::api_client;
use crate::core::models::*;
use crate::core::orchestration::agent_runs;
use crate::core::session::memory::*;
use crate::core::tools::*;

use super::context::*;
use super::stream::{process_stream, StreamConfig};
use super::tools_runner::execute_tool_calls;

/// Pipeline 各阶段共享的状态
struct PipelineState {
    app: tauri::AppHandle,
    sid: String,
    ctx: Arc<crate::core::state::SessionContext>,
    cancel_token: tokio_util::sync::CancellationToken,
    request_workspace: Option<std::path::PathBuf>,
    cfg: AgentConfig,
    api_key: String,
    base_url: String,
    model_id: String,
    api_format: crate::core::llm::api_format::ApiFormat,
    client: reqwest::Client,
    system_prompt: String,
    msg: String,
    image_base64_list: Option<Vec<String>>,
    thinking_override: Option<bool>,
    detected_intent: String,
    dynamic_context_str: String,
    user_msg_for_memory: String,
    user_msg_preview: String,
    initial_msg_index: usize,
    should_think: bool,
    run_id: String,
    /// 循环状态
    loop_count: usize,
    total_loop_count: usize,
    req_input_tokens: u64,
    req_output_tokens: u64,
    final_answer: String,
    /// 渐进式工具披露：已通过 search_tools 激活的工具名
    activated_tools: Vec<String>,
}

const TEXTUAL_PROPOSE_PLAN_MARKER: &str = "<function=propose_plan>";
const DIRECT_DEVELOPER_INTENT: &str = "PROJECT_ACTION";

fn normalize_agent_display_mode(mode: Option<&str>) -> &'static str {
    match mode {
        Some("developer") => "developer",
        _ => "user",
    }
}

fn parse_textual_tool_parameters(raw: &str) -> HashMap<String, String> {
    let mut params = HashMap::new();
    let mut current_key: Option<String> = None;
    let mut current_value = String::new();

    for line in raw.lines() {
        let trimmed = line.trim_start();
        if let Some(rest) = trimmed.strip_prefix("<parameter=") {
            if let Some(key) = current_key.take() {
                params.insert(key, current_value.trim().to_string());
                current_value.clear();
            }

            if let Some(end) = rest.find('>') {
                current_key = Some(rest[..end].trim().to_string());
                current_value.push_str(rest[end + 1..].trim_start());
            }
        } else if trimmed.starts_with("<function=") {
            break;
        } else if current_key.is_some() {
            if !current_value.is_empty() {
                current_value.push('\n');
            }
            current_value.push_str(line);
        }
    }

    if let Some(key) = current_key.take() {
        params.insert(key, current_value.trim().to_string());
    }

    params
}

fn build_plan_content_from_textual_call(
    clean_text: &str,
    params: &HashMap<String, String>,
) -> String {
    if !clean_text.trim().is_empty() {
        return clean_text.trim().to_string();
    }

    let mut content = String::new();
    if let Some(description) = params
        .get("content")
        .or_else(|| params.get("plan_content"))
        .or_else(|| params.get("plan_description"))
    {
        content.push_str(description);
        content.push_str("\n\n");
    }
    if let Some(tasks) = params.get("task_breakdown") {
        content.push_str("## 任务分解\n\n```json\n");
        content.push_str(tasks);
        content.push_str("\n```\n\n");
    }
    if let Some(estimated_time) = params.get("estimated_time") {
        content.push_str("## 预估时间\n\n");
        content.push_str(estimated_time);
        content.push('\n');
    }

    if content.trim().is_empty() {
        "模型以文本形式输出了方案工具调用，但未提供方案正文。".to_string()
    } else {
        content.trim().to_string()
    }
}

fn recover_textual_propose_plan_call(
    current_blocks: &mut Vec<ContentBlock>,
    tool_input_buffers: &mut HashMap<usize, String>,
    current_text_this_turn: &mut String,
    loop_count: usize,
) -> bool {
    let Some(start) = current_text_this_turn.find(TEXTUAL_PROPOSE_PLAN_MARKER) else {
        return false;
    };

    let clean_text = current_text_this_turn[..start].trim_end().to_string();
    let raw_call = &current_text_this_turn[start + TEXTUAL_PROPOSE_PLAN_MARKER.len()..];
    let params = parse_textual_tool_parameters(raw_call);
    let title = params
        .get("title")
        .or_else(|| params.get("plan_title"))
        .cloned()
        .unwrap_or_else(|| "实施方案".to_string());
    let content = build_plan_content_from_textual_call(&clean_text, &params);
    let input = json!({
        "title": title,
        "content": content,
    });

    let visible_notice = format!(
        "我已整理实施方案「{}」，请在右侧方案审批面板中审阅。",
        title
    );
    *current_text_this_turn = visible_notice.clone();
    let mut wrote_text = false;
    current_blocks.retain_mut(|block| match block {
        ContentBlock::Text { text } => {
            if wrote_text {
                return false;
            }
            *text = visible_notice.clone();
            wrote_text = true;
            !text.trim().is_empty()
        }
        _ => true,
    });
    if !wrote_text {
        current_blocks.insert(
            0,
            ContentBlock::Text {
                text: visible_notice,
            },
        );
    }

    let tool_index = current_blocks.len();
    current_blocks.push(ContentBlock::ToolUse {
        id: format!("textual_propose_plan_{}", loop_count),
        name: "propose_plan".to_string(),
        input: json!({}),
    });
    let input_text = serde_json::to_string(&input).unwrap_or_else(|_| "{}".to_string());
    tool_input_buffers.insert(tool_index, input_text);
    println!("[JARVIS] Recovered textual propose_plan call as a real tool call");
    true
}

impl PipelineState {
    /// 阶段 1: 会话初始化、配置加载、意图分类
    async fn setup(
        session_id: String,
        msg: String,
        thinking_override: Option<bool>,
        image_base64_list: Option<Vec<String>>,
        agent_display_mode: Option<String>,
        app: tauri::AppHandle,
        session_manager: tauri::State<'_, crate::core::state::SessionManager>,
        config_state: tauri::State<'_, crate::core::config::ConfigState>,
    ) -> Result<Self, AgentError> {
        println!("\n{}", "=".repeat(60));
        println!(
            "[贾维斯] 收到用户消息: {} (图片数量: {})",
            msg,
            image_base64_list.as_ref().map(|l| l.len()).unwrap_or(0)
        );
        println!("{}", "=".repeat(60));

        let sid = session_id.clone();
        let ctx = session_manager.get_or_create(&session_id).await;
        let has_active_run = ctx
            .cancel_token
            .lock()
            .await
            .as_ref()
            .map(|token| !token.is_cancelled())
            .unwrap_or(false);
        if has_active_run {
            return Err(AgentError::Session(
                "当前会话已有任务正在执行，请等待完成或先停止当前任务。".to_string(),
            ));
        }
        *ctx.session_allowed.lock().await = false;

        let cancel_token = tokio_util::sync::CancellationToken::new();
        *ctx.cancel_token.lock().await = Some(cancel_token.clone());

        let request_workspace = ctx.workspace.lock().await.clone();
        println!(
            "[DEBUG] Current Workspace for session {}: {:?}",
            sid, request_workspace
        );

        let app_cfg = config_state.0.lock().await.clone();
        let cfg = app_cfg.active_config();

        if cfg.api_key.is_empty() {
            *ctx.cancel_token.lock().await = None;
            return Err(AgentError::Config(
                "未配置 API Key，请在设置中填写".to_string(),
            ));
        }
        let api_key = cfg.api_key.clone();
        let base_url = cfg.base_url.clone();
        let model_id = cfg.main_model.clone();
        let utility_model_id = cfg.utility_model.clone();
        let api_format = cfg.api_format_enum();
        println!(
            "[JARVIS] Using model: {} (utility: {})",
            model_id, utility_model_id
        );

        let client = reqwest::Client::new();
        let system_prompt = MAIN_SYSTEM_PROMPT.to_string();

        let has_images = image_base64_list
            .as_ref()
            .map(|l| !l.is_empty())
            .unwrap_or(false);
        let agent_display_mode = normalize_agent_display_mode(agent_display_mode.as_deref());
        let msg_for_intent = if has_images {
            format!(
                "{}\n\n[用户同时附带了图片/截图；截图可能是报错、UI 异常、终端输出、运行结果或代码问题反馈，请结合文本判断是否属于项目操作，不要仅因有图判为 CHAT。]",
                msg
            )
        } else {
            msg.clone()
        };
        let detected_intent = if agent_display_mode == "developer" {
            println!("[JARVIS] 开发者模式：跳过意图分类，直接进入软件开发/项目操作流程");
            DIRECT_DEVELOPER_INTENT.to_string()
        } else {
            let history_for_classification = ctx.memory.lock().await.messages.clone();
            intent::classify_intent(
                &client,
                &api_key,
                &base_url,
                &utility_model_id,
                api_format,
                &msg_for_intent,
                &history_for_classification,
            )
            .await
        };
        println!("[JARVIS] Detected intent: {}", detected_intent);

        let activated_tools = ctx.memory.lock().await.activated_tools.clone();

        Ok(Self {
            app,
            sid,
            ctx,
            cancel_token,
            request_workspace,
            cfg,
            api_key,
            base_url,
            model_id,
            api_format,
            client,
            system_prompt,
            msg,
            image_base64_list,
            thinking_override,
            detected_intent,
            // 以下字段在后续阶段填充
            dynamic_context_str: String::new(),
            user_msg_for_memory: String::new(),
            user_msg_preview: String::new(),
            initial_msg_index: 0,
            should_think: false,
            run_id: String::new(),
            loop_count: 0,
            total_loop_count: 0,
            req_input_tokens: 0,
            req_output_tokens: 0,
            final_answer: String::new(),
            activated_tools,
        })
    }

    /// 阶段 2: 意图验证 — DANGEROUS 权限确认 / UNCLEAR 澄清
    /// 返回 Some(JarvisResult) 表示需要提前返回
    async fn validate(&mut self) -> Result<Option<JarvisResult>, AgentError> {
        if self.detected_intent == "DANGEROUS" {
            let decision = request_permission(
                &self.app,
                &self.sid,
                &format!(
                    "△ 检测到可能的危险操作意图：「{}」\n确认要继续执行吗？",
                    self.msg
                ),
            )
            .await;
            if decision == "reject" {
                println!("[JARVIS] 用户拒绝了危险操作");
                *self.ctx.cancel_token.lock().await = None;
                return Ok(Some(JarvisResult {
                    status: "CANCELLED".to_string(),
                    content: "操作已取消。如果这是一个误判，请重新更具体地描述您的需求。"
                        .to_string(),
                    input_tokens: 0,
                    output_tokens: 0,
                    session_input_tokens: 0,
                    session_output_tokens: 0,
                }));
            }
            println!("[JARVIS] 用户确认了危险操作，继续执行");
        }

        if self.detected_intent == "UNCLEAR" {
            println!("[JARVIS] 意图不明确，询问用户澄清");
            let clarification = "先生，我不太确定您的意思。请问您具体想要做什么呢？\n\n例如：\n- **闲聊** — 随便聊聊天\n- **读写代码** — 查看、修改、审查代码\n- **运行命令** — 执行脚本、编译、部署\n- **咨询问题** — 技术概念、用法疑问\n- **记忆查询** — 查看之前的对话记录\n- **设置** — 配置修改\n- **危险操作** — 删除文件等不可逆操作\n\n请描述您的需求，我来为您处理。";
            *self.ctx.cancel_token.lock().await = None;
            return Ok(Some(JarvisResult {
                status: "CLARIFICATION_NEEDED".to_string(),
                content: clarification.to_string(),
                input_tokens: 0,
                output_tokens: 0,
                session_input_tokens: 0,
                session_output_tokens: 0,
            }));
        }

        Ok(None)
    }

    /// 阶段 3: 上下文构建 + 消息注入 + Agent Run 启动
    async fn pre_loop(&mut self) {
        self.dynamic_context_str = build_dynamic_context(
            &self.detected_intent,
            &self.request_workspace,
            &self.ctx.memory.lock().await.context,
        );

        self.user_msg_for_memory = self.msg.clone();
        self.user_msg_preview = if self.msg.chars().count() > 50 {
            self.msg.chars().take(50).collect::<String>()
        } else {
            self.msg.clone()
        };

        let memory_after_user_message = {
            let mut session = self.ctx.memory.lock().await;
            if crate::core::commands::session::recover_interrupted_into_memory(
                &self.sid,
                &mut session.messages,
            ) {
                let recovered_memory = session.clone();
                crate::core::session::save_session(&self.sid, &recovered_memory, None);
                if let Some(interrupted_run) = agent_runs::find_interrupted_run(&self.sid) {
                    let _ = agent_runs::mark_run_recovered(&interrupted_run.run_id);
                }
                let _ = self.app.emit("session-updated", ());
            }
            let mut active_sid = Some(self.sid.clone());
            self.initial_msg_index = inject_user_message(
                &mut session,
                &self.msg,
                &self.image_base64_list,
                &mut active_sid,
            );
            session.clone()
        };
        crate::core::session::save_session(&self.sid, &memory_after_user_message, None);
        let _ = self.app.emit("session-updated", ());

        self.should_think = self
            .thinking_override
            .unwrap_or_else(|| self.cfg.enable_thinking.unwrap_or(false));

        self.run_id = agent_runs::start_run(&self.app, &self.sid, &self.msg, None);
        *self.ctx.active_run_id.lock().await = Some(self.run_id.clone());
        {
            let session = self.ctx.memory.lock().await;
            agent_runs::save_checkpoint(
                &self.app,
                &self.run_id,
                &self.sid,
                self.total_loop_count,
                session.messages.clone(),
                self.req_input_tokens,
                self.req_output_tokens,
                "用户消息已写入",
            );
        }
    }

    /// 阶段 4: 主循环 — 压缩 → 请求构建 → API 调用 → 流处理 → 工具执行
    async fn run_main_loop(&mut self) -> Result<(), AgentError> {
        loop {
            // 取消检查
            if self.cancel_token.is_cancelled() {
                self.handle_cancellation().await;
                break;
            }

            // 循环次数确认
            if self.loop_count >= crate::core::constants::MAX_AGENT_LOOP_BEFORE_CONFIRM {
                let decision = self.request_loop_continuation().await;
                if !decision {
                    break;
                }
            }

            // 后台通知注入
            self.drain_background_notifications().await;

            // Token 压缩
            self.compact_if_needed().await;

            // 历史快照准备
            let history_snapshot = self.prepare_history_snapshot().await;

            // 构建请求并更新上下文快照
            let (req_json, is_openai) = self.build_llm_request(history_snapshot);

            // 调试日志
            let request_json = serde_json::to_string_pretty(&req_json).unwrap_or_default();
            let logger = debug_logger::DebugLogger::new();
            logger.log_request_to_terminal("MAIN AGENT", self.total_loop_count + 1, &request_json);
            logger.log_request_to_file("MAIN AGENT", self.total_loop_count + 1, &request_json);

            if self.cancel_token.is_cancelled() {
                continue;
            }

            // API 调用（含重试）
            let response = match self.call_api_with_retry(&req_json).await? {
                Some(resp) => resp,
                None => continue,
            };

            // 流式处理
            let mut stream = response.bytes_stream().eventsource();
            let stream_result = process_stream(
                &mut stream,
                is_openai,
                &self.app,
                &self.sid,
                &self.run_id,
                self.total_loop_count + 1,
                &self.cancel_token,
                StreamConfig::default(),
            )
            .await;

            let (
                mut current_blocks,
                mut tool_input_buffers,
                mut current_text_this_turn,
                mut current_thinking_this_turn,
                mut turn_has_tool,
                turn_in_tokens,
                turn_out_tokens,
            ) = (
                stream_result.blocks,
                stream_result.tool_input_buffers,
                stream_result.text,
                stream_result.thinking,
                stream_result.has_tool,
                stream_result.input_tokens,
                stream_result.output_tokens,
            );

            self.req_input_tokens += turn_in_tokens;
            self.req_output_tokens += turn_out_tokens;
            if turn_in_tokens > 0 || turn_out_tokens > 0 {
                self.update_provider_usage_snapshot(turn_in_tokens, turn_out_tokens);
            }

            if recover_textual_propose_plan_call(
                &mut current_blocks,
                &mut tool_input_buffers,
                &mut current_text_this_turn,
                self.total_loop_count + 1,
            ) {
                turn_has_tool = true;
            }

            // 提取工具调用信息
            let tool_calls: Vec<(String, String)> = tool_input_buffers
                .iter()
                .filter_map(|(idx, buf)| {
                    if let Some(ContentBlock::ToolUse { name, .. }) = current_blocks.get(*idx) {
                        Some((name.clone(), buf.clone()))
                    } else {
                        None
                    }
                })
                .collect();

            // 原始 SSE 事件已在 stream.rs 中实时记录，这里只标记流结束
            logger.log_response_to_file(
                "MAIN AGENT",
                self.total_loop_count + 1,
                &format!(
                    "[流结束] text_len={} thinking_len={} tool_blocks={} input_tokens={} output_tokens={}",
                    current_text_this_turn.len(),
                    current_thinking_this_turn.len(),
                    tool_calls.len(),
                    turn_in_tokens,
                    turn_out_tokens
                ),
            );

            // 记录思考过程到 thoughts 日志
            logger.log_thoughts(
                "MAIN AGENT",
                self.total_loop_count + 1,
                &current_thinking_this_turn,
                &current_text_this_turn,
                &tool_calls,
                self.req_input_tokens,
                self.req_output_tokens,
            );

            // 工具执行
            let (tool_results, manual_compact, sub_in, sub_out) = execute_tool_calls(
                &mut current_blocks,
                tool_input_buffers,
                &self.app,
                &self.sid,
                &self.run_id,
                self.total_loop_count + 1,
                &self.cancel_token,
                &self.detected_intent,
            )
            .await;
            self.req_input_tokens += sub_in;
            self.req_output_tokens += sub_out;

            // 检测 search_tools 调用，激活匹配的延迟工具到下一轮请求中
            for block in current_blocks.iter() {
                if let ContentBlock::ToolUse { name, input, .. } = block {
                    if name == "search_tools" {
                        if let Some(query) = input.get("query").and_then(|v| v.as_str()) {
                            let max_results = input
                                .get("max_results")
                                .and_then(|v| v.as_u64())
                                .unwrap_or(5)
                                as usize;
                            let deferred = get_deferred_tool_search_entries(&self.detected_intent);
                            let matches = search_deferred_tools(query, &deferred, max_results);
                            let mut activated_this_turn = Vec::new();
                            for tool_name in matches {
                                if !self.activated_tools.contains(&tool_name) {
                                    println!("[JARVIS] 激活延迟工具: {}", tool_name);
                                    self.activated_tools.push(tool_name.clone());
                                    activated_this_turn.push(tool_name);
                                }
                            }
                            if !activated_this_turn.is_empty() {
                                let mut session = self.ctx.memory.lock().await;
                                for tool_name in activated_this_turn {
                                    if !session.activated_tools.contains(&tool_name) {
                                        session.activated_tools.push(tool_name);
                                    }
                                }
                                crate::core::session::save_session(&self.sid, &session, None);
                            }
                        }
                    }
                }
            }

            let _ = self.app.emit(
                "chat-turn-end",
                json!({
                    "has_tool": turn_has_tool,
                    "sessionId": self.sid,
                    "loopCount": self.total_loop_count + 1
                }),
            );

            if self.cancel_token.is_cancelled() {
                continue;
            }

            // 存储助手回复
            self.store_assistant_response(current_blocks).await;

            // 判断是否继续循环
            if tool_results.is_empty() {
                self.final_answer = current_text_this_turn;
                // 模型可能只返回 thinking 而没有 text（DeepSeek 等模型常见）
                // 此时将 thinking 提升为回复，避免前端空响应
                if self.final_answer.trim().is_empty()
                    && !current_thinking_this_turn.trim().is_empty()
                {
                    self.final_answer = std::mem::take(&mut current_thinking_this_turn);
                }
                {
                    let session = self.ctx.memory.lock().await;
                    agent_runs::save_checkpoint(
                        &self.app,
                        &self.run_id,
                        &self.sid,
                        self.total_loop_count + 1,
                        session.messages.clone(),
                        self.req_input_tokens,
                        self.req_output_tokens,
                        "模型已给出最终回复",
                    );
                }
                break;
            } else {
                let mut session = self.ctx.memory.lock().await;
                session.messages.push(Message::User {
                    content: Content::Multiple(tool_results),
                });
                if manual_compact {
                    let _ = auto_compact(
                        &self.sid,
                        &mut session.messages,
                        &self.client,
                        &self.api_key,
                        &self.base_url,
                        &self.model_id,
                        self.api_format,
                    )
                    .await;
                }
                agent_runs::save_checkpoint(
                    &self.app,
                    &self.run_id,
                    &self.sid,
                    self.total_loop_count + 1,
                    session.messages.clone(),
                    self.req_input_tokens,
                    self.req_output_tokens,
                    "工具结果已写回上下文",
                );
            }
            self.loop_count += 1;
            self.total_loop_count += 1;

            if self.total_loop_count >= crate::core::constants::MAX_AGENT_LOOP_ABSOLUTE {
                self.final_answer = format!(
                    "代理执行超过绝对上限 {} 轮，为防止死循环已强制停止。",
                    crate::core::constants::MAX_AGENT_LOOP_ABSOLUTE
                );
                break;
            }
        }

        Ok(())
    }

    /// 阶段 5: 检查点创建 + 会话保存 + 记忆代理 + 结果组装
    async fn finalize(self) -> JarvisResult {
        let was_cancelled = self.cancel_token.is_cancelled();

        // 创建检查点快照（仅在有文件编辑时创建实快照，纯聊天轮次不创建）
        {
            let has_operations = self.ctx.memory.lock().await.agent_steps.len() > 0;
            // 检查自上次 checkpoint 以来是否有新的文件补丁
            let has_patches = crate::core::tools::file_tools::has_patches_since_last_checkpoint(
                &self.app,
                &self.sid,
            )
            .await;

            let checkpoint_id = if has_patches {
                // 有文件编辑 → 创建实 checkpoint 快照
                crate::core::tools::file_tools::commit_checkpoint_snapshot(
                    &self.app,
                    &self.sid,
                    self.user_msg_preview.clone(),
                    Some(self.initial_msg_index),
                )
                .await
            } else {
                // 纯聊天轮次 → 不创建快照，仅记录日志
                println!("[JARVIS] 纯聊天轮次，跳过检查点快照创建");
                None
            };

            if let Some(id) = &checkpoint_id {
                println!(
                    "[JARVIS] 已创建检查点快照: {} (来自快照引擎)",
                    id
                );
            }
            let _ = self.app.emit(
                "checkpoint-created",
                serde_json::json!({
                    "sessionId": self.sid,
                    "checkpointId": checkpoint_id,
                    "hasOperations": has_operations,
                    "hasPatches": has_patches,
                    "canRollback": true,
                    "message": self.user_msg_preview
                }),
            );
        }

        // 保存会话
        let session_meta = {
            let memory = self.ctx.memory.lock().await.clone();
            let meta = if was_cancelled {
                crate::core::session::save_session(&self.sid, &memory, None)
            } else {
                crate::core::session::save_session(
                    &self.sid,
                    &memory,
                    Some((self.req_input_tokens, self.req_output_tokens)),
                )
            };
            println!("[JARVIS] 会话 {} 已自动保存", self.sid);
            let _ = self.app.emit("session-updated", ());

            if !was_cancelled && meta.message_count >= 2 && meta.title_source == "default" {
                let app_clone = self.app.clone();
                let sid_clone = self.sid.clone();
                let memory_clone = memory.clone();
                tokio::spawn(async move {
                    if let Err(e) = crate::core::commands::session::auto_name_session(
                        app_clone,
                        sid_clone,
                        memory_clone,
                    )
                    .await
                    {
                        println!("[JARVIS] Auto-naming failed: {}", e);
                    }
                });
            }

            Some(meta)
        };

        // 记忆代理
        let reply_for_memory = self.final_answer.clone();
        let cfg_clone = self.cfg.clone();
        tokio::spawn(async move {
            run_memory_agent(self.user_msg_for_memory, reply_for_memory, cfg_clone).await;
        });

        let status = if was_cancelled { "CANCELLED" } else { "FINISH" };

        // 会话日志
        {
            let logger = debug_logger::DebugLogger::new();
            logger.log_session_summary(self.req_input_tokens, self.req_output_tokens, status);
        }

        // Agent Run 完成
        if !was_cancelled {
            agent_runs::complete_run(
                &self.app,
                &self.run_id,
                self.req_input_tokens,
                self.req_output_tokens,
                Some(self.final_answer.chars().take(180).collect()),
            );
        }

        let session_input_tokens = session_meta
            .as_ref()
            .map(|meta| meta.total_input_tokens)
            .unwrap_or(0);
        let session_output_tokens = session_meta
            .as_ref()
            .map(|meta| meta.total_output_tokens)
            .unwrap_or(0);

        {
            let mut active_run_id = self.ctx.active_run_id.lock().await;
            if active_run_id.as_deref() == Some(&self.run_id) {
                *active_run_id = None;
            }
        }
        *self.ctx.cancel_token.lock().await = None;

        JarvisResult {
            status: status.to_string(),
            content: self.final_answer,
            input_tokens: self.req_input_tokens,
            output_tokens: self.req_output_tokens,
            session_input_tokens,
            session_output_tokens,
        }
    }

    // ─── 主循环辅助方法 ───

    /// 处理用户取消：保留本轮用户消息，清理本轮未完成的 agent 内部消息，并发送取消事件
    async fn abort_after_error(&self, error: &AgentError) {
        if !self.run_id.is_empty() {
            agent_runs::fail_run(&self.app, &self.run_id, error.to_string());
            let mut active_run_id = self.ctx.active_run_id.lock().await;
            if active_run_id.as_deref() == Some(&self.run_id) {
                *active_run_id = None;
            }
        }
        *self.ctx.cancel_token.lock().await = None;
    }

    async fn handle_cancellation(&mut self) {
        println!(
            "[JARVIS] 用户已取消执行，保留用户消息并清理 agent 输出，user index {}",
            self.initial_msg_index
        );

        {
            let mut session = self.ctx.memory.lock().await;
            let keep_len = (self.initial_msg_index + 1).min(session.messages.len());
            session.messages.truncate(keep_len);
            self.final_answer = "用户已取消执行。".to_string();
            session.messages.push(Message::Assistant {
                content: Content::Single(self.final_answer.clone()),
            });
        }
        let _ = self.app.emit(
            "chat-stream",
            json!({
                "content": "\n> ✕ **用户已取消执行**\n",
                "sessionId": self.sid,
                "loopCount": self.total_loop_count + 1
            }),
        );
        let _ = self.app.emit(
            "agent-step",
            json!({
                "type": "cancelled",
                "sessionId": self.sid,
                "loopCount": self.total_loop_count + 1
            }),
        );
        agent_runs::cancel_run(
            &self.app,
            &self.run_id,
            self.req_input_tokens,
            self.req_output_tokens,
            Some(self.final_answer.clone()),
        );
    }

    /// 循环上限确认：请求用户授权继续
    /// 返回 true 表示继续，false 表示终止
    async fn request_loop_continuation(&mut self) -> bool {
        let _ = self.app.emit(
            "chat-stream",
            json!({
                "content": format!("\n> **代理执行已达到 {} 回合，正在等待用户确认是否继续...**\n", crate::core::constants::MAX_AGENT_LOOP_BEFORE_CONFIRM),
                "sessionId": self.sid,
                "loopCount": self.total_loop_count + 1
            }),
        );
        let decision = request_permission(
            &self.app,
            &self.sid,
            &format!(
                "代理执行已达到 {} 回合，可能任务较为复杂或陷入循环。是否继续执行？",
                crate::core::constants::MAX_AGENT_LOOP_BEFORE_CONFIRM
            ),
        )
        .await;
        if decision == "allow" || decision == "allow_session" {
            self.loop_count = 0;
            let _ = self.app.emit(
                "chat-stream",
                json!({
                    "content": "\n> **用户已授权继续执行。**\n",
                    "sessionId": self.sid,
                    "loopCount": self.total_loop_count + 1
                }),
            );
            true
        } else {
            self.final_answer = "用户已终止代理的继续执行。".to_string();
            false
        }
    }

    /// 注入后台任务完成通知到会话中
    async fn drain_background_notifications(&self) {
        let notifs =
            crate::core::infra::background::BackgroundManager::drain_notifications(&self.app).await;
        if !notifs.is_empty() {
            let mut notif_text = String::new();
            for n in notifs {
                notif_text.push_str(&format!("[bg:{}] {}: {}\n", n.task_id, n.status, n.result));
            }
            let mut session = self.ctx.memory.lock().await;
            session.messages.push(Message::User {
                content: Content::Single(format!(
                    "<background-results>\n{}\n</background-results>",
                    notif_text
                )),
            });
            session.messages.push(Message::Assistant {
                content: Content::Single("Noted background results.".to_string()),
            });
        }
    }

    /// 检查 token 用量并在需要时执行压缩
    async fn compact_if_needed(&mut self) {
        let mut session = self.ctx.memory.lock().await;
        micro_compact(&mut session.messages);
        let tokens = estimate_tokens(&session.messages);
        if tokens > crate::core::constants::MAX_TOKENS_COMPACT_TRIGGER {
            println!(
                "[贾维斯] Token 估算值 > {} ({})，触发自动压缩",
                crate::core::constants::MAX_TOKENS_COMPACT_TRIGGER,
                tokens
            );

            let mut last_user_msg = None;
            if let Some(Message::User { .. }) = session.messages.last() {
                last_user_msg = session.messages.pop();
            }

            let compact_result = auto_compact(
                &self.sid,
                &mut session.messages,
                &self.client,
                &self.api_key,
                &self.base_url,
                &self.model_id,
                self.api_format,
            )
            .await;

            if let Err(e) = compact_result {
                println!("[JARVIS] 自动压缩失败: {}，继续使用原始上下文", e);
            } else {
                self.initial_msg_index = session.messages.len();
            }

            if let Some(msg) = last_user_msg {
                let needs_assistant_pad = match session.messages.last() {
                    Some(Message::User { .. }) => true,
                    None => true,
                    _ => false,
                };
                if needs_assistant_pad {
                    session.messages.push(Message::Assistant {
                        content: Content::Single("Context compressed.".to_string()),
                    });
                }
                self.initial_msg_index = session.messages.len();
                session.messages.push(msg);
            }
        }
    }

    /// 准备历史消息快照（含图片恢复 + 上下文注入）
    async fn prepare_history_snapshot(&self) -> Vec<Message> {
        let session = self.ctx.memory.lock().await;
        let mut history_snapshot = if self.detected_intent == "CHAT" {
            let mut pruned = session.messages.clone();
            for msg in &mut pruned {
                if let Message::User { content } = msg {
                    if let Content::Multiple(blocks) = content {
                        for block in blocks {
                            if let ContentBlock::ToolResult {
                                ref mut content, ..
                            } = block
                            {
                                *content =
                                    "[系统截断：为闲聊模式节省Token，工具返回的冗长详情已被折叠。]"
                                        .to_string();
                            }
                        }
                    }
                }
            }
            pruned
        } else {
            session.messages.clone()
        };
        drop(session); // 释放锁

        restore_image_data(&mut history_snapshot);
        let snapshot_initial_msg_index = self.initial_msg_index;
        inject_context_into_history(
            &mut history_snapshot,
            snapshot_initial_msg_index,
            &self.dynamic_context_str,
        );

        history_snapshot
    }

    /// 更新本轮请求的上下文 token 快照
    fn update_context_snapshot(&self, history_snapshot: &[Message], tools: &[serde_json::Value]) {
        fn now_ms() -> u64 {
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap_or_default()
                .as_millis() as u64
        }

        fn section(
            model_id: &str,
            key: &str,
            label: &str,
            content: String,
            item_count: usize,
        ) -> ContextSectionSnapshot {
            const MAX_PREVIEW_CHARS: usize = 6000;
            let chars = content.chars().count();
            let token_count = crate::core::llm::token_count::count_text(model_id, &content);
            let truncated = chars > MAX_PREVIEW_CHARS;
            let content = if truncated {
                let preview: String = content.chars().take(MAX_PREVIEW_CHARS).collect();
                format!("{}\n\n…已截断，仅展示前 {} 字符", preview, MAX_PREVIEW_CHARS)
            } else {
                content
            };
            ContextSectionSnapshot {
                key: key.to_string(),
                label: label.to_string(),
                chars,
                estimated_tokens: token_count.tokens,
                token_count_method: token_count.method.as_str().to_string(),
                item_count,
                content,
                truncated,
            }
        }

        fn strip_dynamic_context(messages: &[Message], initial_msg_index: usize, dynamic_context: &str) -> Vec<Message> {
            let mut cleaned = messages.to_vec();
            if dynamic_context.is_empty() {
                return cleaned;
            }
            if let Some(Message::User { content }) = cleaned.get_mut(initial_msg_index) {
                match content {
                    Content::Single(text) => {
                        let prefix = format!("{}\n\n[User Input]:\n", dynamic_context);
                        if let Some(rest) = text.strip_prefix(&prefix) {
                            *text = rest.to_string();
                        }
                    }
                    Content::Multiple(blocks) => {
                        if let Some(ContentBlock::Text { text }) = blocks.first() {
                            if text.trim() == dynamic_context.trim() {
                                blocks.remove(0);
                            }
                        }
                    }
                }
            }
            cleaned
        }

        fn count_blocks(messages: &[Message]) -> (usize, usize, usize, usize) {
            let mut tool_calls = 0;
            let mut tool_results = 0;
            let mut images = 0;
            let mut thinking = 0;
            for message in messages {
                let Content::Multiple(blocks) = (match message {
                    Message::User { content } | Message::Assistant { content } => content,
                }) else {
                    continue;
                };
                for block in blocks {
                    match block {
                        ContentBlock::ToolUse { .. } => tool_calls += 1,
                        ContentBlock::ToolResult { .. } => tool_results += 1,
                        ContentBlock::Image { .. } => images += 1,
                        ContentBlock::Thinking { .. } => thinking += 1,
                        ContentBlock::Text { .. } => {}
                    }
                }
            }
            (tool_calls, tool_results, images, thinking)
        }

        let cleaned_messages = strip_dynamic_context(
            history_snapshot,
            self.initial_msg_index,
            &self.dynamic_context_str,
        );
        let (tool_call_count, tool_result_count, image_count, thinking_count) = count_blocks(history_snapshot);
        let messages_json = serde_json::to_string_pretty(&cleaned_messages).unwrap_or_default();
        let tools_json = serde_json::to_string_pretty(tools).unwrap_or_default();
        let mut sections = vec![
            section(&self.model_id, "system", "System Prompt", self.system_prompt.clone(), 1),
            section(
                &self.model_id,
                "dynamic",
                "Dynamic Context",
                self.dynamic_context_str.clone(),
                1,
            ),
            section(
                &self.model_id,
                "messages",
                "Session Messages",
                messages_json,
                cleaned_messages.len(),
            ),
            section(&self.model_id, "tools", "Tools Schema", tools_json, tools.len()),
        ];
        if image_count > 0 {
            sections.push(section(
                &self.model_id,
                "attachments",
                "Attachments / Images",
                format!("当前请求中包含 {} 个图片块。近期图片会恢复为 base64，远期图片会折叠为文本摘要。", image_count),
                image_count,
            ));
        }
        if tool_result_count > 0 || thinking_count > 0 {
            sections.push(section(
                &self.model_id,
                "runtime",
                "Tool Results / Thinking",
                format!("tool_result: {}\nthinking: {}", tool_result_count, thinking_count),
                tool_result_count + thinking_count,
            ));
        }

        let total_chars = sections.iter().map(|item| item.chars).sum();
        let estimated_tokens = sections.iter().map(|item| item.estimated_tokens).sum();
        let snapshot = SessionContextSnapshot {
            session_id: self.sid.clone(),
            run_id: Some(self.run_id.clone()),
            loop_count: self.total_loop_count + 1,
            model: self.model_id.clone(),
            intent: self.detected_intent.clone(),
            api_format: self.api_format.as_str().to_string(),
            created_at: now_ms(),
            total_chars,
            estimated_tokens,
            provider_input_tokens: None,
            provider_output_tokens: None,
            provider_total_tokens: None,
            drift_percent: None,
            max_context_tokens: crate::core::llm::registry::query_capabilities(&self.model_id)
                .and_then(|capabilities| capabilities.max_context_tokens),
            max_output_tokens: crate::core::constants::MAX_TOKENS_CONTEXT,
            message_count: cleaned_messages.len(),
            tool_schema_count: tools.len(),
            tool_call_count,
            tool_result_count,
            sections,
        };

        if let Err(err) = crate::core::session::save_context_snapshot(&snapshot) {
            eprintln!("[JARVIS] 保存上下文快照失败: {}", err);
        }
        let _ = self.app.emit("context-snapshot-updated", &snapshot);
    }

    fn update_provider_usage_snapshot(&self, input_tokens: u64, output_tokens: u64) {
        let total_tokens = input_tokens.saturating_add(output_tokens);
        let drift = crate::core::session::get_context_snapshot(&self.sid)
            .ok()
            .flatten()
            .and_then(|snapshot| {
                crate::core::llm::token_count::drift_percent(snapshot.estimated_tokens, input_tokens)
            });

        match crate::core::session::update_context_snapshot_usage(
            &self.sid,
            input_tokens,
            output_tokens,
            total_tokens,
            drift,
        ) {
            Ok(Some(snapshot)) => {
                let _ = self.app.emit("context-snapshot-updated", &snapshot);
            }
            Ok(None) => {}
            Err(err) => eprintln!("[JARVIS] 更新上下文 usage 失败: {}", err),
        }
    }

    /// 构建 LLM API 请求体
    fn build_llm_request(&self, history_snapshot: Vec<Message>) -> (serde_json::Value, bool) {
        let tools = get_tools_definition(&self.detected_intent, &self.activated_tools);
        self.update_context_snapshot(&history_snapshot, &tools);

        let mut request_body = AnthropicRequest {
            model: self.model_id.clone(),
            max_tokens: crate::core::constants::MAX_TOKENS_CONTEXT,
            system: self.system_prompt.clone(),
            messages: history_snapshot,
            tools,
            stream: true,
            thinking: None,
            temperature: self.cfg.temperature,
            top_p: self.cfg.top_p,
            top_k: self.cfg.top_k,
        };

        if self.should_think {
            request_body.thinking = Some(ThinkingConfig {
                r#type: "enabled".to_string(),
                budget_tokens: Some(1024),
            });
            if request_body.max_tokens <= 1024 {
                request_body.max_tokens = 4096;
            }
        }

        if self.api_format.is_openai() {
            use crate::core::llm::adapters::{
                should_backfill_deepseek_reasoning_content,
                translate_messages_to_openai_with_reasoning_backfill, translate_tools_to_openai,
            };
            let backfill_reasoning = should_backfill_deepseek_reasoning_content(
                &self.model_id,
                &self.cfg.base_url,
                self.should_think,
            );
            let openai_msgs = translate_messages_to_openai_with_reasoning_backfill(
                &request_body.system,
                &request_body.messages,
                backfill_reasoning,
            );
            let openai_tools = translate_tools_to_openai(&request_body.tools);
            let mut openai_req = OpenAIRequest {
                model: self.model_id.clone(),
                max_tokens: Some(crate::core::constants::MAX_TOKENS_CONTEXT),
                messages: openai_msgs,
                tools: if openai_tools.is_empty() {
                    None
                } else {
                    Some(openai_tools)
                },
                stream: true,
                stream_options: Some(StreamOptions {
                    include_usage: true,
                }),
                reasoning_effort: None,
                thinking: None,
                thinking_budget: None,
                enable_thinking: None,
                temperature: request_body.temperature,
                top_p: request_body.top_p,
            };

            let thinking_param = crate::core::llm::registry::query_capabilities(&self.model_id)
                .and_then(|c| c.thinking_param);
            match thinking_param.as_deref() {
                Some("reasoning_effort") => {
                    if self.should_think {
                        openai_req.reasoning_effort = Some("high".to_string());
                    }
                }
                Some("thinking") => {
                    openai_req.thinking = Some(ThinkingConfig {
                        r#type: if self.should_think {
                            "enabled".to_string()
                        } else {
                            "disabled".to_string()
                        },
                        budget_tokens: None,
                    });
                }
                Some("thinkingBudget") => {
                    openai_req.thinking_budget = Some(if self.should_think { 8192 } else { 0 });
                }
                Some("enable_thinking") => {
                    openai_req.enable_thinking = Some(self.should_think);
                }
                _ => {
                    if self.should_think {
                        openai_req.reasoning_effort = Some("high".to_string());
                    }
                }
            }
            (serde_json::to_value(openai_req).unwrap(), true)
        } else {
            (serde_json::to_value(request_body).unwrap(), false)
        }
    }

    /// 调用 LLM API（含取消检查）
    /// 返回 None 表示请求被取消，调用方应 continue 到下一轮循环
    async fn call_api_with_retry(
        &self,
        req_json: &serde_json::Value,
    ) -> Result<Option<reqwest::Response>, AgentError> {
        let api_request = api_client::api_call_with_retry(
            &self.client,
            &self.base_url,
            req_json,
            &self.api_key,
            self.api_format,
            3,
            &self.app,
            &self.sid,
        );

        match tokio::select! {
            result = api_request => result,
            _ = self.cancel_token.cancelled() => {
                return Ok(None);
            }
        } {
            Ok(resp) => Ok(Some(resp)),
            Err(e) => {
                agent_runs::fail_run(&self.app, &self.run_id, e.to_string());
                *self.ctx.cancel_token.lock().await = None;
                Err(e.into())
            }
        }
    }

    /// 存储助手回复到会话历史中
    async fn store_assistant_response(&self, current_blocks: Vec<ContentBlock>) {
        let mut session = self.ctx.memory.lock().await;
        let filtered_blocks: Vec<ContentBlock> = current_blocks
            .into_iter()
            .filter(|block| match block {
                ContentBlock::Text { text } => !text.trim().is_empty(),
                ContentBlock::Thinking { thinking, .. } => !thinking.trim().is_empty(),
                ContentBlock::ToolUse { .. }
                | ContentBlock::ToolResult { .. }
                | ContentBlock::Image { .. } => true,
            })
            .collect();
        if !filtered_blocks.is_empty() {
            session.messages.push(Message::Assistant {
                content: Content::Multiple(filtered_blocks),
            });
        }
    }
}

/// 主流程入口：依次执行 5 个阶段
pub async fn run_pipeline(
    session_id: String,
    msg: String,
    thinking_override: Option<bool>,
    image_base64_list: Option<Vec<String>>,
    agent_display_mode: Option<String>,
    app: tauri::AppHandle,
    session_manager: tauri::State<'_, crate::core::state::SessionManager>,
    config_state: tauri::State<'_, crate::core::config::ConfigState>,
) -> Result<JarvisResult, AgentError> {
    // 阶段 1: 初始化
    let mut state = PipelineState::setup(
        session_id,
        msg,
        thinking_override,
        image_base64_list,
        agent_display_mode,
        app,
        session_manager,
        config_state,
    )
    .await?;

    // 阶段 2: 意图验证 (可能提前返回)
    if let Some(early_result) = state.validate().await? {
        return Ok(early_result);
    }

    // 阶段 3: 循环前准备
    state.pre_loop().await;

    // 阶段 4: 主循环
    if let Err(err) = state.run_main_loop().await {
        state.abort_after_error(&err).await;
        return Err(err);
    }

    // 阶段 5: 收尾
    Ok(state.finalize().await)
}
