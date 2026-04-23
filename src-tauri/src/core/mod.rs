use eventsource_stream::Eventsource;
use futures_util::StreamExt;
use serde_json::json;
use std::collections::HashMap;
use tauri::Emitter;
use tokio::sync::Mutex;

pub mod background;
pub mod cancellation;
pub mod config;
pub mod constants;
pub mod memory;
pub mod models;
pub mod prompts;
pub mod sessions;
pub mod tasks;
pub mod tools;
pub mod adapters;

use crate::get_agent_home;
use memory::*;
use models::*;
use prompts::*;
use tools::*;

// --- 2. 状态管理系统 (State Management) ---

pub struct SessionState(pub Mutex<SessionMemory>);

/// 当前活跃会话的 ID，用于持久化
pub struct ActiveSession(pub Mutex<Option<String>>);

pub struct SecurityState {
    pub session_allowed: Mutex<bool>,
}

pub struct PendingPermissions(pub Mutex<HashMap<String, tokio::sync::oneshot::Sender<String>>>);

// --- API 调用重试机制 ---

/// 带指数退避重试的 API 调用
/// 仅对网络错误和 5xx 服务端错误重试，4xx 客户端错误直接返回
async fn api_call_with_retry(
    client: &reqwest::Client,
    url: &str,
    body: &serde_json::Value,
    api_key: &str,
    api_format: &str,
    max_retries: u32,
    app: &tauri::AppHandle,
) -> Result<reqwest::Response, String> {
    let mut last_error = String::new();
    for attempt in 0..=max_retries {
        if attempt > 0 {
            let wait_secs = 1u64 << (attempt - 1); // 指数退避: 1s, 2s, 4s
            let _ = app.emit(
                "chat-stream",
                json!({
                    "content": format!("\n> ⚠️ API 调用失败，{}秒后进行第 {}/{} 次重试...\n", wait_secs, attempt, max_retries)
                }),
            );
            println!("[JARVIS] API 重试 {}/{}，等待 {}s...", attempt, max_retries, wait_secs);
            tokio::time::sleep(std::time::Duration::from_secs(wait_secs)).await;
        }

        let mut req = client
            .post(url)
            .header(reqwest::header::CONTENT_TYPE, "application/json");

        if api_format == "openai" {
            req = req.header("Authorization", format!("Bearer {}", api_key));
        } else {
            req = req
                .header("x-api-key", api_key)
                .header("anthropic-version", "2023-06-01");
        }

        match req.json(body).send().await {
            Ok(response) => {
                let status = response.status();
                if status.is_success() || status.as_u16() == 200 {
                    return Ok(response);
                }
                // 4xx 客户端错误不重试（如 API Key 无效、请求格式错误）
                if status.is_client_error() {
                    let err_body = response.text().await.unwrap_or_default();
                    return Err(format!("API 客户端错误 ({}): {}", status.as_u16(), err_body));
                }
                // 5xx 服务端错误 → 重试
                last_error = format!("API 服务端错误: {}", status.as_u16());
            }
            Err(e) => {
                last_error = format!("网络错误: {}", e);
            }
        }
    }
    Err(format!("API 调用在 {} 次重试后仍然失败: {}", max_retries, last_error))
}

async fn classify_intent(
    client: &reqwest::Client,
    api_key: &str,
    base_url: &str,
    model_id: &str,
    api_format: &str,
    msg: &str,
    history: &[Message],
) -> String {
    // 优化后的意图分类提示词：添加 Few-Shot 示例、上下文延续规则、DANGEROUS_ACTION 分类
    let system_prompt = "You are an intent classifier. Based on the conversation history and the new user input, classify the intent into EXACTLY ONE category.

Categories:
1. GENERAL_CHAT: Casual conversation, greetings, reactions, jokes, simple Q&A not requiring file/code operations.
   IMPORTANT: If the previous conversation was casual (e.g., telling a joke, chatting), short user replies like '哈哈', '不错', '好的', '好好好', '继续', '再来一个' are continuations of the chat, NOT project actions.
2. PROJECT_ACTION: Explicit requests to read/write files, execute commands, analyze code, manage tasks, or build/modify projects.
3. MEMORY_QUERY: Questions about past conversations, previous decisions, or historical context.
4. DANGEROUS_ACTION: Requests that involve potentially irreversible or destructive operations (e.g., 'delete all files', 'format disk', 'drop database', 'clear everything', 'remove the entire project', '把文件都删了', '清空数据库').

Key rules:
- When in doubt between GENERAL_CHAT and PROJECT_ACTION, prefer GENERAL_CHAT unless the user explicitly mentions files, code, commands, or project-specific actions.
- Short affirmative replies ('好', '嗯', '不错', '可以', '哈哈', '好好好') following a casual conversation are GENERAL_CHAT.
- Only classify as DANGEROUS_ACTION if the user's request EXPLICITLY mentions destructive operations.
- Pay attention to pronouns like 'it', 'that', 'this', 'earlier' which may imply PROJECT_ACTION or MEMORY_QUERY, but ONLY when the previous context was about project work.

Examples:
- User: '讲个笑话' → GENERAL_CHAT
- Previous: joke telling, User: '哈哈好好好程序员笑话' → GENERAL_CHAT
- Previous: joke telling, User: '再来一个' → GENERAL_CHAT
- User: '帮我读一下 main.rs' → PROJECT_ACTION
- User: '帮我把项目重构一下' → PROJECT_ACTION
- User: '删除所有日志文件' → DANGEROUS_ACTION
- User: '把整个项目目录清空' → DANGEROUS_ACTION
- User: '之前我们讨论的架构方案是什么' → MEMORY_QUERY

Output ONLY the category name.";

    let mut context_str = String::new();
    let recent: Vec<_> = history.iter().rev().take(4).rev().collect();
    for m in recent {
        match m {
            Message::User { content } => {
                let text = match content {
                    Content::Single(s) => s.clone(),
                    Content::Multiple(_) => "[Complex User Input]".to_string(),
                };
                context_str.push_str(&format!(
                    "User: {}\n",
                    text.chars().take(200).collect::<String>()
                ));
            }
            Message::Assistant { content } => {
                let text = match content {
                    Content::Single(s) => s.clone(),
                    Content::Multiple(_) => "[Complex Assistant Action]".to_string(),
                };
                context_str.push_str(&format!(
                    "Assistant: {}\n",
                    text.chars().take(200).collect::<String>()
                ));
            }
        }
    }

    let prompt_msg = format!(
        "Recent conversation:\n{}\n\nNew user input to classify: {}",
        context_str, msg
    );

    let request_body = AnthropicRequest {
        model: model_id.to_string(),
        max_tokens: 20,
        system: system_prompt.to_string(),
        messages: vec![Message::User {
            content: Content::Single(prompt_msg.clone()),
        }],
        tools: vec![],
        stream: false,
    };

    let (req_json, is_openai) = if api_format == "openai" {
        use crate::core::adapters::translate_messages_to_openai;
        use crate::core::models::OpenAIRequest;
        let openai_msgs = translate_messages_to_openai(&system_prompt, &request_body.messages);
        let openai_req = OpenAIRequest {
            model: model_id.to_string(),
            max_tokens: Some(20),
            messages: openai_msgs,
            tools: None,
            stream: false,
            stream_options: None,
        };
        (serde_json::to_value(openai_req).unwrap(), true)
    } else {
        (serde_json::to_value(request_body).unwrap(), false)
    };

    let mut req = client
        .post(base_url)
        .header(reqwest::header::CONTENT_TYPE, "application/json");

    if is_openai {
        req = req.header("Authorization", format!("Bearer {}", api_key));
    } else {
        req = req
            .header("x-api-key", api_key)
            .header("anthropic-version", "2023-06-01");
    }

    if let Ok(response) = req.json(&req_json).send().await {
        if let Ok(json) = response.json::<serde_json::Value>().await {
            let mut text_resp = String::new();
            if is_openai {
                if let Some(choices) = json["choices"].as_array() {
                    if let Some(first) = choices.first() {
                        if let Some(content) = first["message"]["content"].as_str() {
                            text_resp = content.to_string();
                        }
                    }
                }
            } else {
                if let Some(content) = json["content"].as_array() {
                    if let Some(text_block) = content.first() {
                        if let Some(text) = text_block["text"].as_str() {
                            text_resp = text.to_string();
                        }
                    }
                }
            }
            
            let t = text_resp.trim().to_uppercase();
            if t.contains("GENERAL_CHAT") {
                return "GENERAL_CHAT".to_string();
            }
            if t.contains("MEMORY_QUERY") {
                return "MEMORY_QUERY".to_string();
            }
            // 新增：识别危险操作意图
            if t.contains("DANGEROUS_ACTION") {
                return "DANGEROUS_ACTION".to_string();
            }
        }
    }
    "PROJECT_ACTION".to_string() // 默认回退到项目操作
}

#[tauri::command]
pub async fn resolve_permission(
    id: String,
    decision: String,
    pending: tauri::State<'_, PendingPermissions>,
) -> Result<(), String> {
    if let Some(tx) = pending.0.lock().await.remove(&id) {
        let _ = tx.send(decision);
    }
    Ok(())
}

#[tauri::command]
pub async fn cancel_jarvis(
    cancel_state: tauri::State<'_, cancellation::CancellationState>,
) -> Result<(), String> {
    println!("[JARVIS] 收到取消请求");
    cancel_state.cancel().await;
    Ok(())
}

// --- 配置管理 Tauri Commands ---

#[tauri::command]
pub async fn get_config(
    config_state: tauri::State<'_, config::ConfigState>,
) -> Result<config::AppConfig, String> {
    Ok(config_state.0.lock().await.clone())
}

#[tauri::command]
pub async fn save_config_cmd(
    new_config: config::AppConfig,
    config_state: tauri::State<'_, config::ConfigState>,
) -> Result<(), String> {
    let mut current = config_state.0.lock().await;
    *current = new_config.clone();
    config::save_config(&new_config);
    let active = new_config.active_config();
    println!("[配置] 已保存应用配置，当前激活: {} (main_model={})", new_config.active_profile_id, active.main_model);
    Ok(())
}

// --- 会话管理 Tauri Commands ---

#[tauri::command]
pub async fn get_active_session_id(
    active_session: tauri::State<'_, ActiveSession>,
) -> Result<Option<String>, String> {
    Ok(active_session.0.lock().await.clone())
}

#[tauri::command]
pub async fn list_sessions() -> Result<Vec<sessions::SessionMeta>, String> {
    Ok(sessions::list_sessions())
}

#[tauri::command]
pub async fn create_session(
    session_state: tauri::State<'_, SessionState>,
    active_session: tauri::State<'_, ActiveSession>,
) -> Result<sessions::SessionMeta, String> {
    let meta = sessions::create_session();
    // 切换到新会话：清空内存中的对话
    *session_state.0.lock().await = SessionMemory::default();
    *active_session.0.lock().await = Some(meta.id.clone());
    Ok(meta)
}

#[tauri::command]
pub async fn switch_session(
    app: tauri::AppHandle,
    id: String,
    session_state: tauri::State<'_, SessionState>,
    active_session: tauri::State<'_, ActiveSession>,
) -> Result<sessions::SessionMeta, String> {
    // 先保存当前会话
    let current_id = active_session.0.lock().await.clone();
    if let Some(cid) = &current_id {
        let memory = session_state.0.lock().await.clone();
        sessions::save_session(cid, &memory);
        
        // 检查是否需要智能命名
        let all = sessions::list_sessions();
        if let Some(meta) = all.into_iter().find(|m| m.id == *cid) {
            if !meta.is_smart_named && meta.message_count > 0 {
                let app_clone = app.clone();
                let cid_clone = cid.clone();
                tokio::spawn(async move {
                    if let Err(e) = auto_name_session(app_clone, cid_clone, memory).await {
                        println!("[JARVIS] Auto-naming failed: {}", e);
                    }
                });
            }
        }
    }
    // 加载目标会话
    let memory = sessions::load_session(&id)?;
    *session_state.0.lock().await = memory;
    *active_session.0.lock().await = Some(id.clone());
    // 返回元信息
    let all = sessions::list_sessions();
    all.into_iter().find(|m| m.id == id).ok_or_else(|| "会话不存在".to_string())
}

async fn auto_name_session(
    app: tauri::AppHandle,
    session_id: String,
    memory: SessionMemory,
) -> Result<(), String> {
    if memory.messages.is_empty() {
        return Ok(());
    }

    let cfg = crate::core::config::load_config();
    let agent_cfg = cfg.active_config();
    let model_id = &agent_cfg.utility_model;
    let api_key = &agent_cfg.api_key;
    let base_url = &agent_cfg.base_url;
    let api_format = &agent_cfg.api_format;

    // 提取前几轮对话内容供总结
    let mut text_to_summarize = String::new();
    for msg in memory.messages.iter().take(4) {
        if let Ok(m) = serde_json::to_string(msg) {
            text_to_summarize.push_str(&m);
            text_to_summarize.push('\n');
        }
    }

    let summary_prompt = format!("请根据以下对话内容，给出一个极简的会话名称（不超过10个字，不要有任何解释，不要包含标点符号和引号）：\n\n{}", text_to_summarize);

    let request_body = AnthropicRequest {
        model: model_id.to_string(),
        max_tokens: 50,
        system: "你是一个专门用于提取会话名称的助手。只输出名称本身。".to_string(),
        messages: vec![Message::User { content: Content::Single(summary_prompt) }],
        tools: vec![],
        stream: false,
    };

    let client = reqwest::Client::new();
    
    let (req_json, is_openai) = if api_format == "openai" {
        use crate::core::adapters::translate_messages_to_openai;
        use crate::core::models::OpenAIRequest;
        let openai_msgs = translate_messages_to_openai(&request_body.system, &request_body.messages);
        let openai_req = OpenAIRequest {
            model: model_id.to_string(),
            messages: openai_msgs,
            max_tokens: Some(request_body.max_tokens),
            tools: None,
            stream: false,
            stream_options: None,
        };
        (serde_json::to_value(openai_req).unwrap(), true)
    } else {
        (serde_json::to_value(request_body).unwrap(), false)
    };

    let res = client
        .post(base_url)
        .header(reqwest::header::CONTENT_TYPE, "application/json")
        .header(
            if is_openai { "Authorization" } else { "x-api-key" },
            if is_openai { format!("Bearer {}", api_key) } else { api_key.to_string() },
        )
        .header("anthropic-version", "2023-06-01")
        .json(&req_json)
        .send()
        .await
        .map_err(|e| e.to_string())?;

    if !res.status().is_success() {
        return Err(format!("LLM Request failed: {}", res.status()));
    }

    let response_text = res.text().await.map_err(|e| e.to_string())?;
    let parsed: serde_json::Value = serde_json::from_str(&response_text).map_err(|e| e.to_string())?;
    
    let title = if is_openai {
        parsed["choices"][0]["message"]["content"].as_str().unwrap_or("").to_string()
    } else {
        parsed["content"][0]["text"].as_str().unwrap_or("").to_string()
    };
    
    let title = title.trim().trim_matches('"').trim_matches('\'').to_string();
    if !title.is_empty() {
        // 更新会话名称，rename_session 会将 is_smart_named 置为 true
        let _ = sessions::rename_session(&session_id, &title);
        use tauri::Emitter;
        let _ = app.emit("session-renamed", ());
    }

    Ok(())
}

#[tauri::command]
pub async fn delete_session(
    id: String,
    active_session: tauri::State<'_, ActiveSession>,
) -> Result<(), String> {
    let current_id = active_session.0.lock().await.clone();
    if current_id.as_deref() == Some(&id) {
        return Err("不能删除当前活跃的会话".to_string());
    }
    sessions::delete_session(&id)
}

#[tauri::command]
pub async fn rename_session(
    id: String,
    title: String,
) -> Result<sessions::SessionMeta, String> {
    sessions::rename_session(&id, &title)
}

/// 获取当前会话的可渲染历史文本
/// 将 SessionMemory 中的消息转换为前端可以直接显示的 Markdown 字符串
#[tauri::command]
pub async fn get_session_history(
    session_state: tauri::State<'_, SessionState>,
) -> Result<String, String> {
    let session = session_state.0.lock().await;
    if session.messages.is_empty() {
        return Ok(String::new());
    }

    let mut history = String::new();
    for msg in &session.messages {
        match msg {
            Message::User { content } => {
                let text = match content {
                    Content::Single(s) => {
                        if let Some(pos) = s.find("[User Input]:") {
                            s[pos + 13..].trim().to_string()
                        } else {
                            s.trim().to_string()
                        }
                    }
                    Content::Multiple(_) => continue,
                };
                if !text.is_empty() {
                    history.push_str(&format!(
                        "<div class=\"chat-message user-message\"><div class=\"message-content\">\n\n{}\n\n</div></div>\n\n",
                        text
                    ));
                }
            }
            Message::Assistant { content } => {
                history.push_str("<div class=\"chat-message agent-message\"><div class=\"message-content\">\n\n");
                match content {
                    Content::Single(s) => {
                        history.push_str(s);
                    }
                    Content::Multiple(blocks) => {
                        for block in blocks {
                            if let ContentBlock::Text { text } = block {
                                history.push_str(text);
                            }
                        }
                    }
                }
                history.push_str("\n\n</div></div>\n\n");
            }
        }
    }
    Ok(history)
}

#[tauri::command]
pub async fn ask_jarvis(
    msg: String,
    app: tauri::AppHandle,
    session_state: tauri::State<'_, SessionState>,
    cancel_state: tauri::State<'_, cancellation::CancellationState>,
    active_session: tauri::State<'_, ActiveSession>,
    config_state: tauri::State<'_, config::ConfigState>,
) -> Result<JarvisResult, String> {
    println!("\n{}", "=".repeat(60));
    println!("[贾维斯] 收到用户消息: {}", msg);
    println!("{}", "=".repeat(60));

    // 创建本次请求的取消令牌
    let cancel_token = cancel_state.create_token().await;

    // 从 ConfigState 读取配置
    let app_cfg = config_state.0.lock().await.clone();
    let cfg = app_cfg.active_config();
    
    if cfg.api_key.is_empty() {
        return Err("未配置 API Key，请在设置中填写".to_string());
    }
    let api_key = cfg.api_key.clone();
    let base_url = cfg.base_url.clone();
    let model_id = cfg.main_model.clone();
    println!("[JARVIS] Using model: {}", model_id);

    let client = reqwest::Client::new();
    let system_prompt = MAIN_SYSTEM_PROMPT.to_string(); // 静态，用于命中 Prompt Caching

    // 1. 意图识别分类
    let history_for_classification = session_state.0.lock().await.messages.clone();
    let intent = classify_intent(
        &client,
        &api_key,
        &base_url,
        &model_id,
        &cfg.api_format,
        &msg,
        &history_for_classification,
    )
    .await;
    println!("[JARVIS] Detected intent: {}", intent);

    let mut dynamic_context_str;

    // 2. 根据意图组装上下文
    // 新增：DANGEROUS_ACTION 意图前置拦截 —— 在组装上下文之前先弹窗确认
    if intent == "DANGEROUS_ACTION" {
        let decision = request_permission(
            &app,
            &format!("⚠️ 检测到可能的危险操作意图：「{}」\n确认要继续执行吗？", msg),
        ).await;
        if decision == "reject" {
            println!("[JARVIS] 用户拒绝了危险操作");
            return Ok(JarvisResult {
                status: "CANCELLED".to_string(),
                content: "操作已取消。如果这是一个误判，请重新更具体地描述您的需求。".to_string(),
                input_tokens: 0,
                output_tokens: 0,
            });
        }
        println!("[JARVIS] 用户确认了危险操作，继续执行");
    }

    match intent.as_str() {
        "GENERAL_CHAT" => {
            // 闲聊：不加载项目结构和记忆
            dynamic_context_str = "<intent>\nGENERAL_CHAT\n</intent>\n".to_string();
        }
        "MEMORY_QUERY" => {
            // 记忆查询：加载全局记忆，也可考虑加载局部记忆（如果有）
            let global_content = read_memory_file(&get_global_memory_path(), "Global Memory");
            dynamic_context_str = format!(
                "<intent>\nMEMORY_QUERY\n</intent>\n\n<global_context>\n{}\n</global_context>\n",
                global_content
            );
        }
        _ => {
            // 默认 PROJECT_ACTION / DANGEROUS_ACTION（确认后）：加载完整项目上下文
            let global_content = read_memory_file(&get_global_memory_path(), "Global Memory");
            let current_dir = std::env::current_dir().unwrap_or_default();
            let repo_map = generate_repo_map(&current_dir, "", 0, 3);
            dynamic_context_str = format!(
                "<intent>\nPROJECT_ACTION\n</intent>\n\n<global_context>\n{}\n</global_context>\n\n<project_context>\n# Dynamic Repo Map\n{}\n</project_context>\n",
                global_content, repo_map
            );

            let session = session_state.0.lock().await;
            if !session.context.is_empty() {
                dynamic_context_str.push_str(&format!(
                    "\n【当前任务状态】\n- {}\n",
                    session.context.join("\n- ")
                ));
            }

            let skills = load_all_skills();
            if !skills.is_empty() {
                println!(
                    "[JARVIS] Loaded {} skills: {:?}",
                    skills.len(),
                    skills.iter().map(|s| &s.name).collect::<Vec<_>>()
                );
                dynamic_context_str
                    .push_str("\n\n【可用技能】 (使用 load_skill 工具获取完整内容)：\n");
                for skill in &skills {
                    dynamic_context_str
                        .push_str(&format!("  - {}: {}\n", skill.name, skill.description));
                }
            }

            // 注入方案审批提醒：复杂任务需先用 propose_plan 工具提交方案
            dynamic_context_str.push_str("\n\n【重要提醒】对于复杂任务（涉及多步骤修改、架构变更等），必须使用 propose_plan 工具提交实施方案，等待用户在预览面板中审批通过后，才能使用 task_create 创建持久化任务。严禁跳过 propose_plan 直接创建任务！\n");
        }
    }

    let user_msg_for_memory = msg.clone();

    let initial_msg_index;
    {
        let mut session = session_state.0.lock().await;
        initial_msg_index = session.messages.len();
        session.messages.push(Message::User {
            content: Content::Single(msg),
        });
    }

    let mut loop_count = 0;
    let mut total_loop_count = 0;
    let final_answer;
    let mut req_input_tokens: u64 = 0;
    let mut req_output_tokens: u64 = 0;

    loop {
        // --- 取消检查 ---
        if cancel_token.is_cancelled() {
            println!("[JARVIS] 用户已取消执行");
            final_answer = "用户已取消执行。".to_string();
            let _ = app.emit(
                "chat-stream",
                json!({ "content": "\n> ⛔ **用户已取消执行**\n" }),
            );
            break;
        }

        if loop_count >= crate::core::constants::MAX_AGENT_LOOP_BEFORE_CONFIRM {
            let _ = app.emit(
                "chat-stream",
                json!({
                    "content": format!("\n> **代理执行已达到 {} 回合，正在等待用户确认是否继续...**\n", crate::core::constants::MAX_AGENT_LOOP_BEFORE_CONFIRM)
                }),
            );
            let decision = request_permission(
                &app,
                &format!("代理执行已达到 {} 回合，可能任务较为复杂或陷入循环。是否继续执行？", crate::core::constants::MAX_AGENT_LOOP_BEFORE_CONFIRM),
            )
            .await;
            if decision == "allow" || decision == "allow_session" {
                loop_count = 0;
                let _ = app.emit(
                    "chat-stream",
                    json!({
                        "content": "\n> **用户已授权继续执行。**\n"
                    }),
                );
            } else {
                final_answer = "用户已终止代理的继续执行。".to_string();
                break;
            }
        }

        println!("\n[贾维斯] --- 代理循环回合 #{} ---", total_loop_count + 1);

        // Drain background notifications and inject as system message before LLM call
        let notifs = background::BackgroundManager::drain_notifications(&app).await;
        if !notifs.is_empty() {
            let mut notif_text = String::new();
            for n in notifs {
                notif_text.push_str(&format!("[bg:{}] {}: {}\n", n.task_id, n.status, n.result));
            }
            let mut session = session_state.0.lock().await;
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
            let mut session = session_state.0.lock().await;
            micro_compact(&mut session.messages);
            let tokens = estimate_tokens(&session.messages);
            if tokens > crate::core::constants::MAX_TOKENS_COMPACT_TRIGGER {
                println!("[贾维斯] Token 估算值 > {} ({})，触发自动压缩", crate::core::constants::MAX_TOKENS_COMPACT_TRIGGER, tokens);

                // Safely pop the last user message to keep it pristine through compaction
                let mut last_user_msg = None;
                if let Some(Message::User { .. }) = session.messages.last() {
                    last_user_msg = session.messages.pop();
                }

                let _ = auto_compact(
                    &mut session.messages,
                    &client,
                    &api_key,
                    &base_url,
                    &model_id,
                    &cfg.api_format,
                )
                .await;

                // Ensure the list ends with Assistant before pushing User back
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
                    session.messages.push(msg);
                }
            }
        }

        let mut history_snapshot = {
            let session = session_state.0.lock().await;
            if intent == "GENERAL_CHAT" {
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

        let snapshot_initial_msg_index = initial_msg_index;

        // 将动态上下文注入到当前回合的第一条 User 消息（即原始提问），以保持 System Prompt 纯净且不会随工具结果产生位移
        if let Some(initial_msg) = history_snapshot.get_mut(snapshot_initial_msg_index) {
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

        let request_body = AnthropicRequest {
            model: model_id.clone(),
            max_tokens: crate::core::constants::MAX_TOKENS_CONTEXT,
            system: system_prompt.clone(),
            messages: history_snapshot,
            tools: get_tools_definition(&intent),
            stream: true,
        };

        let (req_json, is_openai) = if cfg.api_format == "openai" {
            use crate::core::adapters::{translate_messages_to_openai, translate_tools_to_openai};
            use crate::core::models::OpenAIRequest;
            let openai_msgs = translate_messages_to_openai(&request_body.system, &request_body.messages);
            let openai_tools = translate_tools_to_openai(&request_body.tools);
            let openai_req = OpenAIRequest {
                model: model_id.clone(),
                max_tokens: Some(crate::core::constants::MAX_TOKENS_CONTEXT),
                messages: openai_msgs,
                tools: if openai_tools.is_empty() { None } else { Some(openai_tools) },
                stream: true,
                stream_options: Some(crate::core::models::StreamOptions { include_usage: true }),
            };
            (serde_json::to_value(openai_req).unwrap(), true)
        } else {
            (serde_json::to_value(request_body).unwrap(), false)
        };

        let request_json = serde_json::to_string_pretty(&req_json).unwrap_or_default();
        let log_dir = get_agent_home().join(crate::core::constants::DIR_LOGS);
        if !log_dir.exists() {
            let _ = std::fs::create_dir_all(&log_dir);
        }
        let log_path = log_dir.join(crate::core::constants::FILE_AGENT_LOOP_DEBUG);

        if let Ok(mut file) = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(&log_path)
        {
            use std::io::Write;
            let _ = writeln!(
                file,
                "\n========== LOOP {} ==========\n[REQUEST]\n{}\n[RESPONSE]",
                total_loop_count + 1,
                request_json
            );
        }

        let response = api_call_with_retry(
            &client,
            &base_url,
            &req_json,
            &api_key,
            &cfg.api_format,
            3,    // 最多重试 3 次
            &app,
        )
        .await
        .map_err(|e| e.to_string())?;

        let mut stream = response.bytes_stream().eventsource();
        let mut current_blocks: Vec<ContentBlock> = Vec::new();
        let mut tool_input_buffers: HashMap<usize, String> = HashMap::new();
        let mut current_text_this_turn = String::new();
        let mut turn_has_tool = false;
        let mut debug_response_buffer = String::new();

        let _ = app.emit("chat-turn-start", ());

        while let Some(event_result) = stream.next().await {
            // 流式接收过程中也检查取消信号
            if cancel_token.is_cancelled() {
                println!("[JARVIS] 流式接收中途被用户取消");
                break;
            }
            let event = match event_result {
                Ok(e) => e,
                Err(_) => continue,
            };
            let data = event.data;
            debug_response_buffer.push_str(&data);
            debug_response_buffer.push('\n');
            if data == "[DONE]" {
                break;
            }
            let json: serde_json::Value = serde_json::from_str(&data).unwrap_or(json!({}));

            if is_openai {
                if let Some(usage) = json.get("usage") {
                    if let Some(in_toks) = usage.get("prompt_tokens").and_then(|v| v.as_u64()) {
                        req_input_tokens += in_toks;
                    }
                    if let Some(out_toks) = usage.get("completion_tokens").and_then(|v| v.as_u64()) {
                        req_output_tokens += out_toks;
                    }
                }

                if let Some(choices) = json["choices"].as_array() {
                    if let Some(first) = choices.first() {
                        if let Some(delta) = first.get("delta") {
                            // Handle text content
                            if let Some(t) = delta["content"].as_str() {
                                if current_blocks.is_empty() {
                                    current_blocks.push(ContentBlock::Text { text: String::new() });
                                }
                                if let Some(ContentBlock::Text { text }) = current_blocks.last_mut() {
                                    text.push_str(t);
                                    current_text_this_turn.push_str(t);
                                    let _ = app.emit("chat-content", json!({ "content": t }));
                                }
                            }
                            // Handle tool calls
                            if let Some(tool_calls) = delta["tool_calls"].as_array() {
                                for tc in tool_calls {
                                    let index = tc["index"].as_u64().unwrap_or(0) as usize;
                                    
                                    // If first time seeing this index
                                    if !tool_input_buffers.contains_key(&index) {
                                        let id = tc["id"].as_str().unwrap_or("").to_string();
                                        let name = tc["function"]["name"].as_str().unwrap_or("").to_string();
                                        current_blocks.push(ContentBlock::ToolUse {
                                            id,
                                            name,
                                            input: json!({}),
                                        });
                                        tool_input_buffers.insert(index, String::new());
                                        turn_has_tool = true;
                                        let _ = app.emit("chat-tool-start", ());
                                    }
                                    
                                    // Append arguments string
                                    if let Some(args) = tc["function"]["arguments"].as_str() {
                                        if let Some(buf) = tool_input_buffers.get_mut(&index) {
                                            buf.push_str(args);
                                        }
                                    }
                                }
                            }
                        }
                    }
                }
            } else {
                match json["type"].as_str().unwrap_or("") {
                    "message_start" => {
                        if let Some(usage) = json.get("message").and_then(|m| m.get("usage")) {
                            req_input_tokens += usage
                                .get("input_tokens")
                                .and_then(|v| v.as_u64())
                                .unwrap_or(0);
                        }
                    }
                    "message_delta" => {
                        if let Some(usage) = json.get("usage") {
                            if let Some(in_toks) = usage.get("input_tokens").and_then(|v| v.as_u64()) {
                                req_input_tokens += in_toks;
                            }
                            if let Some(out_toks) = usage.get("output_tokens").and_then(|v| v.as_u64())
                            {
                                req_output_tokens += out_toks;
                            }
                        }
                    }
                    "content_block_start" => {
                        let index = json["index"].as_u64().unwrap_or(0) as usize;
                        let block = &json["content_block"];
                        match block["type"].as_str().unwrap_or("") {
                            "text" => current_blocks.push(ContentBlock::Text {
                                text: String::new(),
                            }),
                            "tool_use" => {
                                let tool_name = block["name"].as_str().unwrap_or("").to_string();
                                current_blocks.push(ContentBlock::ToolUse {
                                    id: block["id"].as_str().unwrap_or("").to_string(),
                                    name: tool_name.clone(),
                                    input: json!({}),
                                });
                                tool_input_buffers.insert(index, String::new());
                                turn_has_tool = true;
                                let _ = app.emit("chat-tool-start", ());
                            }
                            _ => {}
                        }
                    }
                    "content_block_delta" => {
                        let index = json["index"].as_u64().unwrap_or(0) as usize;
                        let delta = &json["delta"];
                        if let Some(block) = current_blocks.get_mut(index) {
                            match block {
                                ContentBlock::Text { text } => {
                                    if let Some(t) = delta["text"].as_str() {
                                        text.push_str(t);
                                        current_text_this_turn.push_str(t);
                                        let _ = app.emit("chat-content", json!({ "content": t }));
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

        let log_dir = get_agent_home().join(crate::core::constants::DIR_LOGS);
        if !log_dir.exists() {
            let _ = std::fs::create_dir_all(&log_dir);
        }
        let log_path = log_dir.join(crate::core::constants::FILE_AGENT_LOOP_DEBUG);

        if let Ok(mut file) = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(&log_path)
        {
            use std::io::Write;
            let _ = writeln!(file, "{}\n", debug_response_buffer);
        }

        let thoughts_log_path = log_dir.join(crate::core::constants::FILE_THOUGHTS_LOG);
        if let Ok(mut file) = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(&thoughts_log_path)
        {
            use std::io::Write;
            let mut log_content =
                format!("\n## [MAIN AGENT] --- Loop {} ---\n", total_loop_count + 1);
            if !current_text_this_turn.trim().is_empty() {
                log_content.push_str(&format!(
                    "### 思考过程:\n{}\n\n",
                    current_text_this_turn.trim()
                ));
            }
            if tool_input_buffers.is_empty() {
                log_content.push_str("### 最终决断:\n准备输出最终回复给用户。\n");
            } else {
                log_content.push_str("### 决定执行操作:\n");
                for (idx, buf) in tool_input_buffers.iter() {
                    if let Some(ContentBlock::ToolUse { name, .. }) = current_blocks.get(*idx) {
                        log_content.push_str(&format!("- 工具: `{}`\n  参数: `{}`\n", name, buf));
                    }
                }
            }
            let _ = writeln!(file, "{}\n---\n", log_content);
        }

        let mut tool_results = Vec::new();
        let mut manual_compact = false;
        for (index, buf) in tool_input_buffers {
            if let Some(ContentBlock::ToolUse {
                name, input, id, ..
            }) = current_blocks.get_mut(index)
            {
                if let Ok(parsed_input) = serde_json::from_str::<serde_json::Value>(&buf) {
                    *input = parsed_input;
                    if name == "compact" {
                        manual_compact = true;
                    }
                    let _ = app.emit("chat-tool-debug", json!({ 
                        "content": format!("\n> **执行工具: `{}`**\n> 输入参数: `{}`\n", name, input) 
                    }));
                    let (output, sub_in, sub_out) = handle_tool_call(&app, name, input).await;
                    req_input_tokens += sub_in;
                    req_output_tokens += sub_out;
                    let _ = app.emit(
                        "chat-tool-debug",
                        json!({
                            "content": format!("> 执行结果: \n`````text\n{}\n`````\n", output)
                        }),
                    );
                    tool_results.push(ContentBlock::ToolResult {
                        tool_use_id: id.clone(),
                        content: output,
                    });
                }
            }
        }

        let _ = app.emit("chat-turn-end", json!({ "has_tool": turn_has_tool }));

        {
            let mut session = session_state.0.lock().await;
            if !current_blocks.is_empty() {
                session.messages.push(Message::Assistant {
                    content: Content::Multiple(current_blocks),
                });
            }
        }

        if tool_results.is_empty() {
            final_answer = current_text_this_turn;
            break;
        } else {
            let mut session = session_state.0.lock().await;
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
        }
        loop_count += 1;
        total_loop_count += 1;

        if total_loop_count >= crate::core::constants::MAX_AGENT_LOOP_ABSOLUTE {
            final_answer = format!("代理执行超过绝对上限 {} 轮，为防止死循环已强制停止。", crate::core::constants::MAX_AGENT_LOOP_ABSOLUTE);
            break;
        }
    }

    // --- 清除取消令牌 ---
    cancel_state.cancel().await; // 确保旧 token 被清理

    // --- 会话自动持久化 ---
    {
        let session_id = active_session.0.lock().await.clone();
        if let Some(sid) = &session_id {
            let memory = session_state.0.lock().await.clone();
            sessions::save_session(sid, &memory);
            println!("[JARVIS] 会话 {} 已自动保存", sid);
            let _ = app.emit("session-updated", ());
        }
    }

    let reply_for_memory = final_answer.clone();
    tokio::spawn(async move {
        run_memory_agent(user_msg_for_memory, reply_for_memory, cfg).await;
    });

    let status = if cancel_token.is_cancelled() {
        "CANCELLED"
    } else {
        "FINISH"
    };

    Ok(JarvisResult {
        status: status.to_string(),
        content: final_answer,
        input_tokens: req_input_tokens,
        output_tokens: req_output_tokens,
    })
}
