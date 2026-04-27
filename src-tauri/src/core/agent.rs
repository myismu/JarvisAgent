use eventsource_stream::Eventsource;
use futures_util::StreamExt;
use serde_json::json;
use std::collections::HashMap;
use tauri::Emitter;

use crate::core::models::*;

use crate::core::api_client;
use crate::core::agent_runs;
use crate::core::intent;
use crate::core::memory::*;
use crate::core::prompts::*;
use crate::core::tools::*;
use crate::core::debug_logger;

fn build_dynamic_context(
    intent: &str,
    workspace: &Option<std::path::PathBuf>,
    session_context: &[String],
) -> String {
    match intent {
        "CHAT" => {
            "<intent>\nCHAT\n</intent>\n".to_string()
        }
        "MEMORY_QUERY" => {
            let global_content = read_memory_file(&get_global_memory_path(), "Global Memory");
            format!(
                "<intent>\nMEMORY_QUERY\n</intent>\n\n<global_context>\n{}\n</global_context>\n",
                global_content
            )
        }
        "QUESTION" => {
            let global_content = read_memory_file(&get_global_memory_path(), "Global Memory");
            format!(
                "<intent>\nQUESTION\n</intent>\n\n<global_context>\n{}\n</global_context>\n",
                global_content
            )
        }
        _ => {
            let global_content = read_memory_file(&get_global_memory_path(), "Global Memory");
            let repo_dir = {
                workspace.clone().unwrap_or_else(|| std::env::current_dir().unwrap_or_default())
            };
            let repo_map = generate_repo_map(&repo_dir, "", 0, 3);
            let mut ctx = format!(
                "<intent>\nPROJECT_ACTION\n</intent>\n\n<global_context>\n{}\n</global_context>\n\n<project_context>\n# Dynamic Repo Map\n{}\n</project_context>\n",
                global_content, repo_map
            );

            if !session_context.is_empty() {
                ctx.push_str(&format!(
                    "\n【当前任务状态】\n- {}\n",
                    session_context.join("\n- ")
                ));
            }

            let skills = load_all_skills();
            if !skills.is_empty() {
                println!(
                    "[JARVIS] Loaded {} skills: {:?}",
                    skills.len(),
                    skills.iter().map(|s| &s.name).collect::<Vec<_>>()
                );
                ctx.push_str("\n\n【可用技能】 (使用 load_skill 工具获取完整内容)：\n");
                for skill in &skills {
                    ctx.push_str(&format!("  - {}: {}\n", skill.name, skill.description));
                }
            }

            ctx.push_str("\n\n【重要提醒】对于复杂任务（涉及多步骤修改、架构变更等），必须使用 propose_plan 工具提交实施方案，等待用户在预览面板中审批通过后，才能使用 task_create 创建持久化任务。严禁跳过 propose_plan 直接创建任务！\n");

            if let Some(ref ws_path) = workspace {
                ctx.push_str(&format!(
                    "\n\n【会话沙箱】当前会话配置了工作目录沙箱，路径为 '{}'。所有文件操作、命令执行都被限制在此目录内。尝试访问沙箱外的路径会被系统拦截。\n",
                    ws_path.display()
                ));
            } else {
                ctx.push_str("\n\n【无沙箱限制】当前会话没有沙箱限制，您可以自由访问系统上的任何路径和执行任何命令。工作目录仅作为默认起始位置，不构成访问限制。\n");
            }

            ctx
        }
    }
}

fn inject_user_message(
    session: &mut SessionMemory,
    msg: &str,
    image_base64_list: &Option<Vec<String>>,
    active_session_id: &mut Option<String>,
) -> usize {
    let initial_msg_index = session.messages.len();

    if let Some(images) = image_base64_list {
        if !images.is_empty() {
            let mut blocks: Vec<ContentBlock> = Vec::new();
            for img_base64 in images {
                let media_type = img_base64.split(':').nth(1)
                    .and_then(|s| s.split(';').next())
                    .unwrap_or("image/png")
                    .to_string();
                let data = img_base64.split(',').nth(1).unwrap_or("").to_string();
                let session_id_str = active_session_id.clone().unwrap_or_default();
                let file_path = if !data.is_empty() {
                    let fp = crate::core::sessions::save_image_to_file(&session_id_str, &media_type, &data);
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
                blocks.insert(0, ContentBlock::Text { text: msg.to_string() });
            }
            session.messages.push(Message::User {
                content: Content::Multiple(blocks),
            });
        } else {
            session.messages.push(Message::User {
                content: Content::Single(msg.to_string()),
            });
        }
    } else {
        session.messages.push(Message::User {
            content: Content::Single(msg.to_string()),
        });
    }

    initial_msg_index
}

fn inject_context_into_history(
    history_snapshot: &mut Vec<Message>,
    initial_msg_index: usize,
    dynamic_context_str: &str,
) {
    if let Some(initial_msg) = history_snapshot.get_mut(initial_msg_index) {
        if let Message::User {
            content: Content::Single(ref mut text),
        } = initial_msg
        {
            *text = format!("{}\n\n[User Input]:\n{}", dynamic_context_str, text);
        } else if let Message::User {
            content: Content::Multiple(ref mut blocks),
        } = initial_msg
        {
            blocks.insert(
                0,
                ContentBlock::Text {
                    text: format!("{}\n\n", dynamic_context_str),
                },
            );
        }
    }
}

fn restore_image_data(history_snapshot: &mut Vec<Message>) {
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
                                            if let Some(data) = crate::core::sessions::load_image_data(fp) {
                                                source.data = data;
                                            }
                                        }
                                    }
                                }
                                new_blocks.push(img_block);
                            } else {
                                let summary = format!(
                                    "[图片: {}]",
                                    source.media_type
                                );
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

async fn process_stream(
    stream: &mut (impl StreamExt<Item = Result<eventsource_stream::Event, eventsource_stream::EventStreamError<reqwest::Error>>> + Unpin),
    is_openai: bool,
    app: &tauri::AppHandle,
    sid: &str,
    run_id: &str,
    loop_count: usize,
    cancel_token: &tokio_util::sync::CancellationToken,
) -> (Vec<ContentBlock>, HashMap<usize, String>, String, String, bool, u64, u64) {
    let mut current_blocks: Vec<ContentBlock> = Vec::new();
    let mut tool_input_buffers: HashMap<usize, String> = HashMap::new();
    let mut openai_tool_block_map: HashMap<usize, usize> = HashMap::new();
    let mut current_text_this_turn = String::new();
    let mut current_thinking_this_turn = String::new();
    let mut turn_has_tool = false;
    let mut req_input_tokens: u64 = 0;
    let mut req_output_tokens: u64 = 0;

    let _ = app.emit("chat-turn-start", json!({ "sessionId": sid }));

    loop {
        let event_result = tokio::select! {
            next = stream.next() => next,
            _ = cancel_token.cancelled() => {
                println!("[JARVIS] 流式接收中途被用户取消");
                break;
            }
        };
        let Some(event_result) = event_result else {
            break;
        };
        let event = match event_result {
            Ok(e) => e,
            Err(_) => continue,
        };
        let data = event.data;
        if data == "[DONE]" {
            break;
        }
        let json_val: serde_json::Value = serde_json::from_str(&data).unwrap_or(json!({}));

        if is_openai {
            if let Some(usage) = json_val.get("usage") {
                if let Some(in_toks) = usage.get("prompt_tokens").and_then(|v| v.as_u64()) {
                    req_input_tokens += in_toks;
                }
                if let Some(out_toks) = usage.get("completion_tokens").and_then(|v| v.as_u64()) {
                    req_output_tokens += out_toks;
                }
            }

            if let Some(choices) = json_val["choices"].as_array() {
                if let Some(first) = choices.first() {
                    if let Some(delta) = first.get("delta") {
                        if let Some(t) = delta["content"].as_str() {
                            if !t.is_empty() {
                                let is_text = matches!(current_blocks.last(), Some(ContentBlock::Text { .. }));
                                if !is_text {
                                    current_blocks.push(ContentBlock::Text {
                                        text: String::new(),
                                    });
                                }
                                if let Some(ContentBlock::Text { text }) = current_blocks.last_mut()
                                {
                                    text.push_str(t);
                                    current_text_this_turn.push_str(t);
                                    let _ = app.emit("chat-content", json!({ "content": t, "sessionId": sid }));
                                    agent_runs::append_content(app, run_id, t, loop_count);
                                }
                            }
                        }
                        if let Some(t) = delta["reasoning_content"].as_str() {
                            if !t.is_empty() {
                                let is_thinking = matches!(current_blocks.last(), Some(ContentBlock::Thinking { .. }));
                                if !is_thinking {
                                    current_blocks.push(ContentBlock::Thinking {
                                        thinking: String::new(),
                                        signature: String::new(),
                                    });
                                }
                                if let Some(ContentBlock::Thinking { thinking, .. }) =
                                    current_blocks.last_mut()
                                {
                                    thinking.push_str(t);
                                    current_thinking_this_turn.push_str(t);
                                    let _ = app.emit("chat-thinking", json!({ "content": t, "sessionId": sid }));
                                    agent_runs::append_thinking(app, run_id, t, loop_count);
                                }
                            }
                        }
                        if let Some(tool_calls) = delta["tool_calls"].as_array() {
                            for tc in tool_calls {
                                let tool_call_index = tc["index"].as_u64().unwrap_or(0) as usize;

                                if !openai_tool_block_map.contains_key(&tool_call_index) {
                                    let id = tc["id"].as_str().unwrap_or("").to_string();
                                    let name = tc["function"]["name"]
                                        .as_str()
                                        .unwrap_or("")
                                        .to_string();
                                    current_blocks.push(ContentBlock::ToolUse {
                                        id,
                                        name,
                                        input: json!({}),
                                    });
                                    let block_index = current_blocks.len() - 1;
                                    openai_tool_block_map.insert(tool_call_index, block_index);
                                    tool_input_buffers.insert(block_index, String::new());
                                    turn_has_tool = true;
                                    let _ = app.emit("chat-tool-start", json!({ "sessionId": sid }));
                                    agent_runs::append_tool_log(app, run_id, "\n> 工具参数接收中\n", loop_count);
                                }

                                if let Some(args) = tc["function"]["arguments"].as_str() {
                                    if let Some(block_index) = openai_tool_block_map.get(&tool_call_index) {
                                        if let Some(buf) = tool_input_buffers.get_mut(block_index) {
                                            buf.push_str(args);
                                        }
                                    }
                                }
                            }
                        }
                    }
                }
            }
        } else {
            match json_val["type"].as_str().unwrap_or("") {
                "message_start" => {
                    if let Some(usage) = json_val.get("message").and_then(|m| m.get("usage")) {
                        req_input_tokens += usage
                            .get("input_tokens")
                            .and_then(|v| v.as_u64())
                            .unwrap_or(0);
                    }
                }
                "message_delta" => {
                    if let Some(usage) = json_val.get("usage") {
                        if let Some(in_toks) =
                            usage.get("input_tokens").and_then(|v| v.as_u64())
                        {
                            req_input_tokens += in_toks;
                        }
                        if let Some(out_toks) =
                            usage.get("output_tokens").and_then(|v| v.as_u64())
                        {
                            req_output_tokens += out_toks;
                        }
                    }
                }
                "content_block_start" => {
                    let block = &json_val["content_block"];
                    match block["type"].as_str().unwrap_or("") {
                        "text" => current_blocks.push(ContentBlock::Text {
                            text: String::new(),
                        }),
                        "thinking" => current_blocks.push(ContentBlock::Thinking {
                            thinking: String::new(),
                            signature: block["signature"].as_str().unwrap_or("").to_string(),
                        }),
                        "tool_use" => {
                            let tool_name = block["name"].as_str().unwrap_or("").to_string();
                            current_blocks.push(ContentBlock::ToolUse {
                                id: block["id"].as_str().unwrap_or("").to_string(),
                                name: tool_name.clone(),
                                input: json!({}),
                            });
                            tool_input_buffers.insert(current_blocks.len() - 1, String::new());
                            turn_has_tool = true;
                            let _ = app.emit("chat-tool-start", json!({ "sessionId": sid }));
                            agent_runs::append_tool_log(app, run_id, "\n> 工具参数接收中\n", loop_count);
                        }
                        _ => {}
                    }
                }
                "content_block_delta" => {
                    let index = json_val["index"].as_u64().unwrap_or(0) as usize;
                    let delta = &json_val["delta"];
                    if let Some(block) = current_blocks.get_mut(index) {
                        match block {
                            ContentBlock::Text { text } => {
                                if let Some(t) = delta["text"].as_str() {
                                    text.push_str(t);
                                    current_text_this_turn.push_str(t);
                                    let _ = app.emit("chat-content", json!({ "content": t, "sessionId": sid }));
                                    agent_runs::append_content(app, run_id, t, loop_count);
                                }
                            }
                            ContentBlock::Thinking { thinking, .. } => {
                                if let Some(t) = delta["thinking"].as_str() {
                                    thinking.push_str(t);
                                    current_thinking_this_turn.push_str(t);
                                    let _ = app.emit("chat-thinking", json!({ "content": t, "sessionId": sid }));
                                    agent_runs::append_thinking(app, run_id, t, loop_count);
                                }
                            }
                            ContentBlock::ToolUse { .. } => {
                                if let Some(partial) = delta["partial_json"].as_str() {
                                    if let Some(buf) = tool_input_buffers.get_mut(&index) {
                                        buf.push_str(partial);
                                    }
                                }
                            }
                            _ => {}
                        }
                    }
                }
                _ => {}
            }
        }
    }

    (current_blocks, tool_input_buffers, current_text_this_turn, current_thinking_this_turn, turn_has_tool, req_input_tokens, req_output_tokens)
}

async fn execute_tool_calls(
    current_blocks: &mut Vec<ContentBlock>,
    tool_input_buffers: HashMap<usize, String>,
    app: &tauri::AppHandle,
    sid: &str,
    run_id: &str,
    loop_count: usize,
    cancel_token: &tokio_util::sync::CancellationToken,
) -> (Vec<ContentBlock>, bool, u64, u64) {
    let mut tool_results = Vec::new();
    let mut manual_compact = false;
    let mut sub_in: u64 = 0;
    let mut sub_out: u64 = 0;

    for (index, buf) in tool_input_buffers {
        if cancel_token.is_cancelled() {
            break;
        }
        if let Some(ContentBlock::ToolUse {
            name, input, id, ..
        }) = current_blocks.get_mut(index)
        {
            match crate::core::adapters::parse_streamed_tool_input(&buf) {
                Ok((parsed_input, recovered)) => {
                    *input = parsed_input;
                    if name == "compact" {
                        manual_compact = true;
                    }
                    if recovered {
                        let _ = app.emit(
                            "chat-tool-debug",
                            json!({
                                "content": format!("\n> ↻ 参数已自动修复: `{}`\n", name),
                                "sessionId": sid
                            }),
                        );
                    }

                    let input_summary: String = {
                        let s = input.to_string();
                        if s.len() > 120 { format!("{}...", s.chars().take(120).collect::<String>()) } else { s }
                    };
                    let _ = app.emit("chat-tool-debug", json!({
                        "kind": "tool_status",
                        "status": "running",
                        "tool": name.clone(),
                        "toolCallId": id.clone(),
                        "sessionId": sid
                    }));
                    agent_runs::append_tool_log(app, run_id, &format!("\n> ▸ 执行: `{}`\n", name), loop_count);
                    let _ = app.emit("agent-step", json!({
                        "type": "tool_call",
                        "tool": name,
                        "input_summary": input_summary,
                        "sessionId": sid
                    }));
                    agent_runs::record_tool_call(app, run_id, name, Some(input_summary), loop_count);

                    if cancel_token.is_cancelled() {
                        break;
                    }

                    let (output, si, so) = handle_tool_call(app, name, input, sid).await;
                    sub_in += si;
                    sub_out += so;

                    if cancel_token.is_cancelled() {
                        break;
                    }

                    let output_summary: String = {
                        if output.len() > 150 { format!("{}...", output.chars().take(150).collect::<String>()) } else { output.clone() }
                    };
                    let _ = app.emit(
                        "chat-tool-debug",
                        json!({
                            "kind": "tool_status",
                            "status": "completed",
                            "tool": name.clone(),
                            "toolCallId": id.clone(),
                            "sessionId": sid
                        }),
                    );
                    agent_runs::append_tool_log(app, run_id, &format!("> ◈ 完成: `{}`\n", name), loop_count);
                    let _ = app.emit("agent-step", json!({
                        "type": "tool_result",
                        "tool": name,
                        "output_summary": output_summary,
                        "sessionId": sid
                    }));
                    agent_runs::record_tool_result(app, run_id, name, Some(output_summary), None, loop_count);
                    tool_results.push(ContentBlock::ToolResult {
                        tool_use_id: id.clone(),
                        content: output,
                    });
                }
                Err(err) => {
                    let preview: String = buf.chars().take(500).collect();
                    let truncated = if buf.chars().count() > 500 {
                        format!("{}...(truncated)", preview)
                    } else {
                        preview
                    };
                    let failure = format!(
                        "工具 `{}` 参数解析失败：{}\n原始参数片段：{}",
                        name, err, truncated
                    );
                    println!("[JARVIS] {}", failure);
                    let _ = app.emit(
                        "chat-tool-debug",
                        json!({
                            "kind": "tool_status",
                            "status": "error",
                            "tool": name.clone(),
                            "toolCallId": id.clone(),
                            "content": format!("\n> ✕ 参数解析失败: `{}` - {}\n", name, err),
                            "sessionId": sid
                        }),
                    );
                    agent_runs::append_tool_log(app, run_id, &format!("\n> ✕ 参数解析失败: `{}` - {}\n", name, err), loop_count);
                    let _ = app.emit("agent-step", json!({
                        "type": "tool_error",
                        "tool": name,
                        "error": format!("{}", err),
                        "sessionId": sid
                    }));
                    agent_runs::record_tool_result(app, run_id, name, None, Some(format!("{}", err)), loop_count);
                    tool_results.push(ContentBlock::ToolResult {
                        tool_use_id: id.clone(),
                        content: failure,
                    });
                }
            }
        }
    }

    (tool_results, manual_compact, sub_in, sub_out)
}

#[tauri::command]
pub async fn ask_jarvis(
    session_id: String,
    msg: String,
    thinking_override: Option<bool>,
    image_base64_list: Option<Vec<String>>,
    app: tauri::AppHandle,
    session_manager: tauri::State<'_, crate::core::state::SessionManager>,
    config_state: tauri::State<'_, crate::core::config::ConfigState>,
) -> Result<JarvisResult, String> {
    println!("\n{}", "=".repeat(60));
    println!("[贾维斯] 收到用户消息: {} (图片数量: {})", msg, image_base64_list.as_ref().map(|l| l.len()).unwrap_or(0));
    println!("{}", "=".repeat(60));

    let sid = session_id.clone();
    let ctx = session_manager.get_or_create(&session_id).await;
    ctx.pending_checkpoint.lock().await.clear();
    *ctx.session_allowed.lock().await = false;

    let cancel_token = tokio_util::sync::CancellationToken::new();
    *ctx.cancel_token.lock().await = Some(cancel_token.clone());

    let request_workspace = ctx.workspace.lock().await.clone();
    println!("[DEBUG] Current Workspace for session {}: {:?}", sid, request_workspace);

    let app_cfg = config_state.0.lock().await.clone();
    let cfg = app_cfg.active_config();

    if cfg.api_key.is_empty() {
        *ctx.cancel_token.lock().await = None;
        return Err("未配置 API Key，请在设置中填写".to_string());
    }
    let api_key = cfg.api_key.clone();
    let base_url = cfg.base_url.clone();
    let model_id = cfg.main_model.clone();
    let utility_model_id = cfg.utility_model.clone();
    println!("[JARVIS] Using model: {} (utility: {})", model_id, utility_model_id);

    let client = reqwest::Client::new();
    let system_prompt = MAIN_SYSTEM_PROMPT.to_string();

    let has_images = image_base64_list.as_ref().map(|l| !l.is_empty()).unwrap_or(false);

    let detected_intent = if has_images {
        println!("[JARVIS] 检测到图片输入，跳过意图分类，直接进入对话流程");
        "CHAT".to_string()
    } else {
        let history_for_classification = ctx.memory.lock().await.messages.clone();
        intent::classify_intent(
            &client,
            &api_key,
            &base_url,
            &utility_model_id,
            &cfg.api_format,
            &msg,
            &history_for_classification,
        )
        .await
    };
    println!("[JARVIS] Detected intent: {}", detected_intent);

    if detected_intent == "DANGEROUS" {
        let decision = request_permission(
            &app,
            &sid,
            &format!(
                "△ 检测到可能的危险操作意图：「{}」\n确认要继续执行吗？",
                msg
            ),
        )
        .await;
        if decision == "reject" {
            println!("[JARVIS] 用户拒绝了危险操作");
            *ctx.cancel_token.lock().await = None;
            return Ok(JarvisResult {
                status: "CANCELLED".to_string(),
                content: "操作已取消。如果这是一个误判，请重新更具体地描述您的需求。".to_string(),
                input_tokens: 0,
                output_tokens: 0,
                session_input_tokens: 0,
                session_output_tokens: 0,
            });
        }
        println!("[JARVIS] 用户确认了危险操作，继续执行");
    }

    if detected_intent == "UNCLEAR" {
        println!("[JARVIS] 意图不明确，询问用户澄清");
        let clarification = "先生，我不太确定您的意思。请问您具体想要做什么呢？\n\n例如：\n- **闲聊** — 随便聊聊天\n- **读写代码** — 查看、修改、审查代码\n- **运行命令** — 执行脚本、编译、部署\n- **咨询问题** — 技术概念、用法疑问\n- **记忆查询** — 查看之前的对话记录\n- **设置** — 配置修改\n- **危险操作** — 删除文件等不可逆操作\n\n请描述您的需求，我来为您处理。";
        *ctx.cancel_token.lock().await = None;
        return Ok(JarvisResult {
            status: "CLARIFICATION_NEEDED".to_string(),
            content: clarification.to_string(),
            input_tokens: 0,
            output_tokens: 0,
            session_input_tokens: 0,
            session_output_tokens: 0,
        });
    }

    let dynamic_context_str = build_dynamic_context(&detected_intent, &request_workspace, &ctx.memory.lock().await.context);

    let user_msg_for_memory = msg.clone();
    let user_msg_preview = if msg.chars().count() > 50 {
        msg.chars().take(50).collect::<String>()
    } else {
        msg.clone()
    };

    let mut initial_msg_index;
    {
        let mut session = ctx.memory.lock().await;
        let mut active_sid = Some(sid.clone());
        initial_msg_index = inject_user_message(&mut session, &msg, &image_base64_list, &mut active_sid);
    }

    let should_think =
        thinking_override.unwrap_or_else(|| cfg.enable_thinking.unwrap_or(false));

    let mut loop_count = 0;
    let mut total_loop_count = 0;
    let final_answer;
    let mut req_input_tokens: u64 = 0;
    let mut req_output_tokens: u64 = 0;
    let run_id = agent_runs::start_run(&app, &sid, &msg, None);
    {
        let session = ctx.memory.lock().await;
        agent_runs::save_checkpoint(
            &app,
            &run_id,
            &sid,
            total_loop_count,
            session.messages.clone(),
            req_input_tokens,
            req_output_tokens,
            "用户消息已写入",
        );
    }

    loop {
        if cancel_token.is_cancelled() {
            println!("[JARVIS] 用户已取消执行，回滚消息到 index {}", initial_msg_index);
            {
                let mut session = ctx.memory.lock().await;
                session.messages.truncate(initial_msg_index);
            }
            let current_msg_count = {
                let session = ctx.memory.lock().await;
                session.messages.len()
            };
            let has_new_messages = current_msg_count > initial_msg_index;

            if has_new_messages {
                let messages_snapshot = {
                    let session = ctx.memory.lock().await;
                    session.messages[initial_msg_index..].to_vec()
                };
                let mut summary_context = String::new();
                for m in &messages_snapshot {
                    match m {
                        Message::User { content } => {
                            let text = match content {
                                Content::Single(s) => s.clone(),
                                Content::Multiple(blocks) => blocks.iter()
                                    .filter_map(|b| if let ContentBlock::Text { text } = b { Some(text.as_str()) } else { None })
                                    .collect::<Vec<_>>()
                                    .join(" "),
                            };
                            summary_context.push_str(&format!("用户: {}\n", text));
                        }
                        Message::Assistant { content } => {
                            let text = match content {
                                Content::Single(s) => s.clone(),
                                Content::Multiple(blocks) => blocks.iter()
                                    .filter_map(|b| if let ContentBlock::Text { text } = b { Some(text.as_str()) } else { None })
                                    .collect::<Vec<_>>()
                                    .join(" "),
                            };
                            if !text.is_empty() {
                                let preview = if text.len() > 2000 { &text[..2000] } else { &text };
                                summary_context.push_str(&format!("助手: {}\n", preview));
                            }
                        }
                    }
                }

                if !summary_context.is_empty() {
                    let summary_prompt = format!(
                        "用户取消了正在执行的AI助手任务。请根据以下已有的对话和执行内容，用简洁的中文总结目前已完成的工作和当前状态。不要添加未完成的内容，只总结已有结果。\n\n{}", 
                        if summary_context.len() > 8000 { &summary_context[summary_context.len() - 8000..] } else { &summary_context }
                    );

                    let summary_result = crate::core::memory::auto_compact_summary(
                        &client, &api_key, &base_url, &utility_model_id, &cfg.api_format, &summary_prompt,
                    ).await;

                    match summary_result {
                        Ok(summary) => {
                            final_answer = format!("⚠️ 用户已取消执行\n\n**已完成工作摘要：**\n{}", summary);
                        }
                        Err(e) => {
                            println!("[JARVIS] 取消时生成摘要失败: {}", e);
                            final_answer = "用户已取消执行。".to_string();
                        }
                    }
                } else {
                    final_answer = "用户已取消执行。".to_string();
                }
            } else {
                final_answer = "用户已取消执行。".to_string();
            }

            {
                let mut session = ctx.memory.lock().await;
                session.messages.truncate(initial_msg_index);
            }
            let _ = app.emit(
                "chat-stream",
                json!({ "content": "\n> ✕ **用户已取消执行**\n", "sessionId": sid }),
            );
            let _ = app.emit("agent-step", json!({
                "type": "cancelled",
                "sessionId": sid
            }));
            agent_runs::cancel_run(
                &app,
                &run_id,
                req_input_tokens,
                req_output_tokens,
                Some(final_answer.clone()),
            );
            break;
        }

        if loop_count >= crate::core::constants::MAX_AGENT_LOOP_BEFORE_CONFIRM {
            let _ = app.emit(
                "chat-stream",
                json!({
                    "content": format!("\n> **代理执行已达到 {} 回合，正在等待用户确认是否继续...**\n", crate::core::constants::MAX_AGENT_LOOP_BEFORE_CONFIRM),
                    "sessionId": sid
                }),
            );
            let decision = request_permission(
                &app,
                &sid,
                &format!(
                    "代理执行已达到 {} 回合，可能任务较为复杂或陷入循环。是否继续执行？",
                    crate::core::constants::MAX_AGENT_LOOP_BEFORE_CONFIRM
                ),
            )
            .await;
            if decision == "allow" || decision == "allow_session" {
                loop_count = 0;
                let _ = app.emit(
                    "chat-stream",
                    json!({
                        "content": "\n> **用户已授权继续执行。**\n",
                        "sessionId": sid
                    }),
                );
            } else {
                final_answer = "用户已终止代理的继续执行。".to_string();
                break;
            }
        }

        let notifs = crate::core::background::BackgroundManager::drain_notifications(&app).await;
        if !notifs.is_empty() {
            let mut notif_text = String::new();
            for n in notifs {
                notif_text.push_str(&format!("[bg:{}] {}: {}\n", n.task_id, n.status, n.result));
            }
            let mut session = ctx.memory.lock().await;
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

        {
            let mut session = ctx.memory.lock().await;
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
                    &mut session.messages,
                    &client,
                    &api_key,
                    &base_url,
                    &model_id,
                    &cfg.api_format,
                )
                .await;

                if let Err(e) = compact_result {
                    println!("[JARVIS] 自动压缩失败: {}，继续使用原始上下文", e);
                } else {
                    initial_msg_index = session.messages.len();
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
                    initial_msg_index = session.messages.len();
                    session.messages.push(msg);
                }
            }
        }

        let mut history_snapshot = {
            let session = ctx.memory.lock().await;
            if detected_intent == "CHAT" {
                let mut pruned = session.messages.clone();
                for msg in &mut pruned {
                    if let Message::User { content } = msg {
                        if let Content::Multiple(blocks) = content {
                            for block in blocks {
                                if let ContentBlock::ToolResult {
                                    ref mut content, ..
                                } = block
                                {
                                    *content = "[系统截断：为闲聊模式节省Token，工具返回的冗长详情已被折叠。]".to_string();
                                }
                            }
                        }
                    }
                }
                pruned
            } else {
                session.messages.clone()
            }
        };

        restore_image_data(&mut history_snapshot);

        let snapshot_initial_msg_index = initial_msg_index;

        inject_context_into_history(&mut history_snapshot, snapshot_initial_msg_index, &dynamic_context_str);

        let mut request_body = AnthropicRequest {
            model: model_id.clone(),
            max_tokens: crate::core::constants::MAX_TOKENS_CONTEXT,
            system: system_prompt.clone(),
            messages: history_snapshot,
            tools: get_tools_definition(&detected_intent),
            stream: true,
            thinking: None,
            temperature: cfg.temperature,
            top_p: cfg.top_p,
            top_k: cfg.top_k,
        };

        if should_think {
            request_body.thinking = Some(ThinkingConfig {
                r#type: "enabled".to_string(),
                budget_tokens: Some(1024),
            });
            if request_body.max_tokens <= 1024 {
                request_body.max_tokens = 4096;
            }
        }

        let (req_json, is_openai) = if cfg.api_format == "openai" {
            use crate::core::adapters::{
                should_backfill_deepseek_reasoning_content,
                translate_messages_to_openai_with_reasoning_backfill,
                translate_tools_to_openai,
            };
            let backfill_reasoning_content = should_backfill_deepseek_reasoning_content(
                &model_id,
                &cfg.base_url,
                should_think,
            );
            let openai_msgs = translate_messages_to_openai_with_reasoning_backfill(
                &request_body.system,
                &request_body.messages,
                backfill_reasoning_content,
            );
            let openai_tools = translate_tools_to_openai(&request_body.tools);
            let mut openai_req = OpenAIRequest {
                model: model_id.clone(),
                max_tokens: Some(crate::core::constants::MAX_TOKENS_CONTEXT),
                messages: openai_msgs,
                tools: if openai_tools.is_empty() {
                    None
                } else {
                    Some(openai_tools)
                },
                stream: true,
                stream_options: Some(crate::core::models::StreamOptions {
                    include_usage: true,
                }),
                reasoning_effort: None,
                thinking: None,
                thinking_budget: None,
                enable_thinking: None,
                temperature: request_body.temperature,
                top_p: request_body.top_p,
            };

            let thinking_param = crate::core::registry::query_capabilities(&model_id)
                .and_then(|c| c.thinking_param);

            match thinking_param.as_deref() {
                Some("reasoning_effort") => {
                    if should_think {
                        openai_req.reasoning_effort = Some("high".to_string());
                    }
                }
                Some("thinking") => {
                    openai_req.thinking = Some(ThinkingConfig {
                        r#type: if should_think { "enabled".to_string() } else { "disabled".to_string() },
                        budget_tokens: None,
                    });
                }
                Some("thinkingBudget") => {
                    openai_req.thinking_budget = Some(if should_think { 8192 } else { 0 });
                }
                Some("enable_thinking") => {
                    openai_req.enable_thinking = Some(should_think);
                }
                _ => {
                    if should_think {
                        openai_req.reasoning_effort = Some("high".to_string());
                    }
                }
            }
            (serde_json::to_value(openai_req).unwrap(), true)
        } else {
            (serde_json::to_value(request_body).unwrap(), false)
        };

        let request_json = serde_json::to_string_pretty(&req_json).unwrap_or_default();

        let logger = debug_logger::DebugLogger::new();
        logger.log_request_to_terminal("MAIN AGENT", total_loop_count + 1, &request_json);
        logger.log_request_to_file("MAIN AGENT", total_loop_count + 1, &request_json);

        if cancel_token.is_cancelled() {
            continue;
        }

        let api_request = api_client::api_call_with_retry(
            &client,
            &base_url,
            &req_json,
            &api_key,
            &cfg.api_format,
            3,
            &app,
            &sid,
        );

        let response = match tokio::select! {
            result = api_request => result,
            _ = cancel_token.cancelled() => {
                continue;
            }
        } {
            Ok(resp) => resp,
            Err(e) => {
                agent_runs::fail_run(&app, &run_id, e.to_string());
                *ctx.cancel_token.lock().await = None;
                return Err(e.to_string());
            }
        };

        if cancel_token.is_cancelled() {
            continue;
        }

        let mut stream = response.bytes_stream().eventsource();

        let (mut current_blocks, tool_input_buffers, current_text_this_turn, current_thinking_this_turn, turn_has_tool, turn_in_tokens, turn_out_tokens) =
            process_stream(&mut stream, is_openai, &app, &sid, &run_id, total_loop_count + 1, &cancel_token).await;

        req_input_tokens += turn_in_tokens;
        req_output_tokens += turn_out_tokens;

        logger.log_response_to_file("MAIN AGENT", total_loop_count + 1, &format!("[stream processed]"));

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
        logger.log_thoughts("MAIN AGENT", total_loop_count + 1, &current_thinking_this_turn, &current_text_this_turn, &tool_calls, req_input_tokens, req_output_tokens);

        let (tool_results, manual_compact, sub_in, sub_out) =
            execute_tool_calls(&mut current_blocks, tool_input_buffers, &app, &sid, &run_id, total_loop_count + 1, &cancel_token).await;
        req_input_tokens += sub_in;
        req_output_tokens += sub_out;

        let _ = app.emit("chat-turn-end", json!({ "has_tool": turn_has_tool, "sessionId": sid }));

        if cancel_token.is_cancelled() {
            continue;
        }

        {
            let mut session = ctx.memory.lock().await;
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

        if tool_results.is_empty() {
            final_answer = current_text_this_turn;
            {
                let session = ctx.memory.lock().await;
                agent_runs::save_checkpoint(
                    &app,
                    &run_id,
                    &sid,
                    total_loop_count + 1,
                    session.messages.clone(),
                    req_input_tokens,
                    req_output_tokens,
                    "模型已给出最终回复",
                );
            }
            break;
        } else {
            let mut session = ctx.memory.lock().await;
            session.messages.push(Message::User {
                content: Content::Multiple(tool_results),
            });
            if manual_compact {
                let _ = auto_compact(
                    &mut session.messages,
                    &client,
                    &api_key,
                    &base_url,
                    &model_id,
                    &cfg.api_format,
                )
                .await;
            }
            agent_runs::save_checkpoint(
                &app,
                &run_id,
                &sid,
                total_loop_count + 1,
                session.messages.clone(),
                req_input_tokens,
                req_output_tokens,
                "工具结果已写回上下文",
            );
        }
        loop_count += 1;
        total_loop_count += 1;

        if total_loop_count >= crate::core::constants::MAX_AGENT_LOOP_ABSOLUTE {
            final_answer = format!(
                "代理执行超过绝对上限 {} 轮，为防止死循环已强制停止。",
                crate::core::constants::MAX_AGENT_LOOP_ABSOLUTE
            );
            break;
        }
    }

    let was_cancelled = cancel_token.is_cancelled();

    {
        let operations: Vec<crate::core::checkpoint::FileOperation> = ctx.pending_checkpoint.lock().await.drain(..).collect();
        let has_operations = !operations.is_empty();
        let parent_id = crate::core::checkpoint::get_head_checkpoint_id(&sid);
        let cp = crate::core::checkpoint::create_checkpoint(
            &sid,
            parent_id.as_deref(),
            &user_msg_preview,
            None,
            None,
            operations,
        );
        println!("[JARVIS] 已创建检查点: {} (操作数: {})", cp.id, cp.operations.len());
        let _ = app.emit("checkpoint-created", serde_json::json!({
            "sessionId": sid,
            "checkpointId": cp.id,
            "hasOperations": has_operations,
            "message": user_msg_preview
        }));
    }

    let session_meta = {
        let memory = ctx.memory.lock().await.clone();
        let meta = if was_cancelled {
            crate::core::sessions::save_session(&sid, &memory, None)
        } else {
            crate::core::sessions::save_session(&sid, &memory, Some((req_input_tokens, req_output_tokens)))
        };
        println!("[JARVIS] 会话 {} 已自动保存", sid);
        let _ = app.emit("session-updated", ());

        if !was_cancelled
            && meta.message_count >= 2
            && meta.title.trim() == "新会话"
            && meta.title_source == "default"
        {
            let app_clone = app.clone();
            let sid_clone = sid.clone();
            let memory_clone = memory.clone();
            tokio::spawn(async move {
                if let Err(e) = crate::core::commands::session::auto_name_session(app_clone, sid_clone, memory_clone).await {
                    println!("[JARVIS] Auto-naming failed: {}", e);
                }
            });
        }

        Some(meta)
    };

    let reply_for_memory = final_answer.clone();
    let cfg_clone = cfg.clone();
    tokio::spawn(async move {
        run_memory_agent(user_msg_for_memory, reply_for_memory, cfg_clone).await;
    });

    let status = if was_cancelled {
        "CANCELLED"
    } else {
        "FINISH"
    };

    {
        let logger = debug_logger::DebugLogger::new();
        logger.log_session_summary(req_input_tokens, req_output_tokens, status);
    }

    if !was_cancelled {
        agent_runs::complete_run(
            &app,
            &run_id,
            req_input_tokens,
            req_output_tokens,
            Some(final_answer.chars().take(180).collect()),
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

    *ctx.cancel_token.lock().await = None;

    Ok(JarvisResult {
        status: status.to_string(),
        content: final_answer,
        input_tokens: req_input_tokens,
        output_tokens: req_output_tokens,
        session_input_tokens,
        session_output_tokens,
    })
}
