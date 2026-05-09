//! 后台任务管理模块
//!
//! 提供异步进程执行能力，支持：
//! - 非阻塞启动 shell 命令（避免阻塞主对话）
//! - 自动检测服务端口和任务类型
//! - stdout/stderr 实时捕获与缓冲
//! - 任务状态追踪与通知队列

use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::process::Stdio;
use std::sync::Arc;
use std::time::Duration;
use tauri::{Emitter, Manager};
use tokio::io::{AsyncBufReadExt, BufReader};
use tokio::sync::Mutex;

/// 后台任务信息
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct BackgroundTask {
    pub id: String,
    pub session_id: Option<String>,
    pub command: String,
    pub status: String,
    pub result: Option<String>,
    pub port: Option<u16>,
    pub task_type: Option<String>,
}

/// 任务完成通知（Tauri 事件推送 + 轮询兼容）
#[derive(Clone, Debug, Serialize)]
pub struct Notification {
    pub task_id: String,
    pub session_id: Option<String>,
    pub status: String,
    pub command: String,
    pub result: String,
    pub port: Option<u16>,
    pub task_type: Option<String>,
}

/// 后台任务管理器
///
/// 维护所有运行中任务的状态和通知队列，以及子进程句柄用于安全终止
pub struct BackgroundManager {
    pub tasks: HashMap<String, BackgroundTask>,
    pub notification_queue: Vec<Notification>,
    pub child_processes: HashMap<String, Arc<tokio::sync::Mutex<Option<tokio::process::Child>>>>,
}

impl BackgroundManager {
    pub fn new() -> Self {
        Self {
            tasks: HashMap::new(),
            notification_queue: Vec::new(),
            child_processes: HashMap::new(),
        }
    }

    /// 终止单个后台任务进程
    pub async fn kill_task(&mut self, task_id: &str) {
        if let Some(child_arc) = self.child_processes.remove(task_id) {
            let mut child_guard = child_arc.lock().await;
            if let Some(ref mut child) = *child_guard {
                let _ = child.kill().await;
                let _ = child.wait().await;
                println!("[BACKGROUND] Killed task {}", task_id);
            }
        }
        if let Some(task) = self.tasks.get_mut(task_id) {
            if task.status == "running" {
                task.status = "killed".to_string();
            }
        }
    }

    /// 终止所有运行中的后台任务进程（撤回前调用，释放文件锁）
    pub async fn kill_all(&mut self) {
        let ids: Vec<String> = self
            .tasks
            .iter()
            .filter(|(_, t)| t.status == "running")
            .map(|(id, _)| id.clone())
            .collect();
        for id in &ids {
            self.kill_task(id).await;
        }
        println!("[BACKGROUND] Killed {} running tasks", ids.len());
    }

    /// 从命令字符串中检测服务端口和任务类型
    ///
    /// 支持显式端口参数（`--port`/`-p`）和框架默认端口推断
    fn detect_port_and_type(command: &str, dir: Option<&str>) -> (Option<u16>, Option<String>) {
        let lower = command.to_lowercase();
        let dir_lower = dir.map(|d| d.to_lowercase()).unwrap_or_default();

        let task_type: Option<String> = {
            // 优先根据目录名判断
            if dir_lower.contains("frontend")
                || dir_lower.contains("client")
                || dir_lower.contains("web")
                || dir_lower.ends_with("/fe")
            {
                Some("frontend".to_string())
            } else if dir_lower.contains("backend")
                || dir_lower.contains("server")
                || dir_lower.contains("api")
                || dir_lower.ends_with("/be")
            {
                Some("backend".to_string())
            } else if lower.contains("vite")
                || lower.contains("vue-cli-service serve")
                || lower.contains("next dev")
                || lower.contains("nuxt dev")
            {
                Some("frontend".to_string())
            } else if lower.contains("python")
                || lower.contains("flask")
                || lower.contains("uvicorn")
                || lower.contains("cargo run")
            {
                Some("backend".to_string())
            } else {
                // npm run dev/node 命令无法仅从命令字符串判断类型，留空
                None
            }
        };

        let port = if lower.contains("--port") || lower.contains("-p ") {
            let parts: Vec<&str> = command.split_whitespace().collect();
            for i in 0..parts.len() {
                if parts[i] == "--port" && i + 1 < parts.len() {
                    if let Ok(p) = parts[i + 1].parse::<u16>() {
                        return (Some(p), task_type);
                    }
                }
                if parts[i].starts_with("-p") {
                    let port_str = if parts[i] == "-p" && i + 1 < parts.len() {
                        parts[i + 1]
                    } else {
                        &parts[i][2..]
                    };
                    if let Ok(p) = port_str.parse::<u16>() {
                        return (Some(p), task_type);
                    }
                }
            }
            None
        } else if lower.contains("vite") {
            Some(5173)
        } else if lower.contains("next dev") || lower.contains("nuxt dev") {
            Some(3000)
        } else if lower.contains("npm run dev") || lower.contains("npm start") {
            // 根据目录推断端口：backend 通常是 3000/8000，frontend 通常是 5173
            if dir_lower.contains("backend") || dir_lower.contains("server") || dir_lower.contains("api") {
                Some(3000)
            } else {
                Some(5173)
            }
        } else if lower.contains("flask run") {
            Some(5000)
        } else if lower.contains("uvicorn") {
            Some(8000)
        } else if lower.contains("cargo run") && lower.contains("tauri") {
            Some(1420)
        } else {
            None
        };

        (port, task_type)
    }

    /// 启动后台任务
    ///
    /// 通过 PowerShell 执行命令，异步捕获输出，任务完成后推送通知
    pub async fn run(app: tauri::AppHandle, command: String, dir: Option<String>, session_id: Option<String>) -> String {
        let task_id = uuid::Uuid::new_v4().to_string()[..8].to_string();

        let mut short_cmd = command.clone();
        if short_cmd.len() > 80 {
            short_cmd.truncate(80);
            short_cmd.push_str("...");
        }

        let (detected_port, task_type) = Self::detect_port_and_type(&command, dir.as_deref());

        if let Some(state) = app.try_state::<BackgroundState>() {
            let state_clone = state.0.clone();
            let task_id_clone = task_id.clone();
            let cmd_clone = command.clone();

            let session_id_clone = session_id.clone();
            {
                let mut bg = state_clone.lock().await;
                bg.tasks.insert(
                    task_id_clone.clone(),
                    BackgroundTask {
                        id: task_id_clone.clone(),
                        session_id: session_id.clone(),
                        command: cmd_clone.clone(),
                        status: "running".to_string(),
                        result: None,
                        port: detected_port,
                        task_type: task_type.clone(),
                    },
                );
            }

            let app_handle = app.clone();
            let task_id_async = task_id.clone();
            let cmd_async = command.clone();
            let dir_async = dir.clone();
            let port_async = detected_port;
            let type_async = task_type.clone();

            tokio::spawn(async move {
                let target_dir = dir_async.unwrap_or_else(|| {
                    std::env::current_dir()
                        .unwrap_or_default()
                        .to_string_lossy()
                        .to_string()
                });

                let (shell, shell_args): (String, Vec<String>) = if cfg!(target_os = "windows") {
                    let ps_cmd = format!(
                        "[Console]::OutputEncoding = [System.Text.Encoding]::UTF8; {}",
                        cmd_async
                    );
                    ("powershell".to_string(), vec!["-NoProfile".to_string(), "-Command".to_string(), ps_cmd])
                } else {
                    ("bash".to_string(), vec!["-c".to_string(), cmd_async.clone()])
                };

                let mut cmd = tokio::process::Command::new(&shell);
                cmd.current_dir(&target_dir).args(&shell_args);

                let child = match cmd.stdout(Stdio::piped()).stderr(Stdio::piped()).spawn()
                {
                    Ok(c) => c,
                    Err(e) => {
                        if let Some(st) = app_handle.try_state::<BackgroundState>() {
                            let mut bg = st.0.lock().await;
                            if let Some(task) = bg.tasks.get_mut(&task_id_async) {
                                task.status = "error".to_string();
                                task.result = Some(format!("Failed to spawn: {}", e));
                            }
                            bg.notification_queue.push(Notification {
                                task_id: task_id_async,
                                session_id: session_id_clone.clone(),
                                status: "error".to_string(),
                                command: cmd_async,
                                result: format!("Failed to spawn: {}", e),
                                port: port_async,
                                task_type: type_async,
                            });
                        }
                        return;
                    }
                };

                // 保存子进程句柄，用于撤回前安全终止
                let child_arc = Arc::new(tokio::sync::Mutex::new(Some(child)));
                {
                    let mut bg = state_clone.lock().await;
                    bg.child_processes.insert(task_id_async.clone(), child_arc.clone());
                }

                // 取出 child 用于后续流式读取
                let mut child_owned = {
                    let mut guard = child_arc.lock().await;
                    guard.take().expect("child just inserted")
                };

                let output_buffer = Arc::new(tokio::sync::Mutex::new(String::new()));
                let max_output = crate::core::constants::MAX_BACKGROUND_OUTPUT_LEN;

                let stdout = child_owned.stdout.take();
                let stderr = child_owned.stderr.take();

                if let Some(stdout) = stdout {
                    let reader = BufReader::new(stdout);
                    let mut lines = reader.lines();
                    let task_id_for_stdout = task_id_async.clone();
                    let buf = output_buffer.clone();

                    tokio::spawn(async move {
                        while let Ok(Some(line)) = lines.next_line().await {
                            println!("[bg:{}] {}", task_id_for_stdout, line);
                            let mut b = buf.lock().await;
                            if b.len() < max_output {
                                b.push_str(&line);
                                b.push('\n');
                            }
                        }
                    });
                }

                if let Some(stderr) = stderr {
                    let reader = BufReader::new(stderr);
                    let mut lines = reader.lines();
                    let task_id_for_stderr = task_id_async.clone();
                    let buf = output_buffer.clone();

                    tokio::spawn(async move {
                        while let Ok(Some(line)) = lines.next_line().await {
                            println!("[bg:{} ERR] {}", task_id_for_stderr, line);
                            let mut b = buf.lock().await;
                            if b.len() < max_output {
                                b.push_str(&line);
                                b.push('\n');
                            }
                        }
                    });
                }

                let status = match child_owned.wait().await {
                    Ok(s) => {
                        if s.success() {
                            "completed"
                        } else {
                            "error"
                        }
                    }
                    Err(_) => "error",
                };

                // 等待一小段时间让 stdout/stderr 任务完成写入
                tokio::time::sleep(Duration::from_millis(50)).await;

                let final_output = {
                    let buf = output_buffer.lock().await;
                    if buf.is_empty() {
                        "(process finished)".to_string()
                    } else {
                        buf.clone()
                    }
                };

                let notif = Notification {
                    task_id: task_id_async.clone(),
                    session_id: session_id_clone.clone(),
                    status: status.to_string(),
                    command: cmd_async.clone(),
                    result: final_output,
                    port: port_async,
                    task_type: type_async,
                };

                // Tauri 事件推送（实时通知前端，替代轮询）
                let _ = app_handle.emit("bg-task-done", &notif);

                if let Some(st) = app_handle.try_state::<BackgroundState>() {
                    let mut bg = st.0.lock().await;
                    if let Some(task) = bg.tasks.get_mut(&task_id_async) {
                        task.status = status.to_string();
                        task.result = Some(notif.result.clone());
                    }
                    bg.child_processes.remove(&task_id_async);
                    bg.notification_queue.push(notif);
                }
            });
        }

        let type_info = task_type
            .as_ref()
            .map(|t| format!(" [{}]", t))
            .unwrap_or_default();
        let port_info = detected_port
            .map(|p| format!(" :{}", p))
            .unwrap_or_default();
        format!(
            "Background task {} started{}{}: {}",
            task_id, type_info, port_info, short_cmd
        )
    }

    /// 移除单个后台任务（无论状态）
    fn remove_task(&mut self, task_id: &str) {
        self.tasks.remove(task_id);
        self.child_processes.remove(task_id);
        println!("[BACKGROUND] Dismissed task {}", task_id);
    }

    /// 清理指定会话的所有非 running 任务
    fn remove_session_tasks(&mut self, session_id: &str) {
        let ids: Vec<String> = self
            .tasks
            .iter()
            .filter(|(_, t)| {
                t.session_id.as_deref() == Some(session_id) && t.status != "running"
            })
            .map(|(id, _)| id.clone())
            .collect();
        for id in &ids {
            self.tasks.remove(id);
            self.child_processes.remove(id);
        }
        println!(
            "[BACKGROUND] Cleared {} tasks for session {}",
            ids.len(),
            session_id
        );
    }

    /// 清理已完成且子进程句柄已释放的后台任务，防止内存无限增长
    fn cleanup_expired(&mut self) {
        // 清理那些已经不在 child_processes 中的非 running 任务
        // child_processes 在任务完成时会被移除 (task 完成逻辑中 child_processes.remove)
        self.tasks.retain(|id, task| {
            task.status == "running" || self.child_processes.contains_key(id)
        });
    }

    /// 查询任务状态
    ///
    /// 提供 task_id 时返回单个任务详情，否则返回所有任务摘要
    pub async fn check(app: &tauri::AppHandle, task_id: Option<String>) -> String {
        if let Some(state) = app.try_state::<BackgroundState>() {
            let mut bg = state.0.lock().await;
            bg.cleanup_expired();
            if let Some(tid) = task_id {
                if let Some(t) = bg.tasks.get(&tid) {
                    let mut short_cmd = t.command.clone();
                    if short_cmd.len() > 60 {
                        short_cmd.truncate(60);
                        short_cmd.push_str("...");
                    }
                    format!(
                        "[{}] {}\n{}",
                        t.status,
                        short_cmd,
                        t.result.as_deref().unwrap_or("(running)")
                    )
                } else {
                    format!("Error: Unknown task {}", tid)
                }
            } else {
                let mut lines = Vec::new();
                for (tid, t) in &bg.tasks {
                    let mut short_cmd = t.command.clone();
                    if short_cmd.len() > 60 {
                        short_cmd.truncate(60);
                        short_cmd.push_str("...");
                    }
                    lines.push(format!("{}: [{}] {}", tid, t.status, short_cmd));
                }
                if lines.is_empty() {
                    "No background tasks.".to_string()
                } else {
                    lines.join("\n")
                }
            }
        } else {
            "Error: Background state not initialized.".to_string()
        }
    }

    /// 取出并清空所有待处理通知（用于前端轮询）
    pub async fn drain_notifications(app: &tauri::AppHandle) -> Vec<Notification> {
        if let Some(state) = app.try_state::<BackgroundState>() {
            let mut bg = state.0.lock().await;
            let notifs = bg.notification_queue.clone();
            bg.notification_queue.clear();
            notifs
        } else {
            Vec::new()
        }
    }

    /// 终止所有运行中的后台任务（撤回文件前调用，释放文件锁）
    pub async fn kill_all_background(app: &tauri::AppHandle) {
        if let Some(state) = app.try_state::<BackgroundState>() {
            let mut bg = state.0.lock().await;
            bg.kill_all().await;
        }
    }

    /// 移除单个后台任务
    pub async fn dismiss_task(app: &tauri::AppHandle, task_id: &str) -> bool {
        if let Some(state) = app.try_state::<BackgroundState>() {
            let mut bg = state.0.lock().await;
            bg.remove_task(task_id);
            true
        } else {
            false
        }
    }

    /// 清理指定会话的非运行中任务
    pub async fn clear_session_tasks(app: &tauri::AppHandle, session_id: &str) -> usize {
        if let Some(state) = app.try_state::<BackgroundState>() {
            let mut bg = state.0.lock().await;
            let before = bg.tasks.len();
            bg.remove_session_tasks(session_id);
            before - bg.tasks.len()
        } else {
            0
        }
    }
}

/// Tauri 状态包装器，用于注入到应用状态管理
pub struct BackgroundState(pub Arc<Mutex<BackgroundManager>>);

impl Default for BackgroundState {
    fn default() -> Self {
        Self(Arc::new(Mutex::new(BackgroundManager::new())))
    }
}
