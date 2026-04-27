use std::collections::HashMap;
use std::sync::Arc;
use std::process::Stdio;
use tauri::Manager;
use tokio::sync::Mutex;
use tokio::io::{AsyncBufReadExt, BufReader};
use serde::{Serialize, Deserialize};

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct BackgroundTask {
    pub id: String,
    pub command: String,
    pub status: String,
    pub result: Option<String>,
    pub port: Option<u16>,
    pub task_type: Option<String>,
}

#[derive(Clone, Debug)]
pub struct Notification {
    pub task_id: String,
    pub status: String,
    pub command: String,
    pub result: String,
    pub port: Option<u16>,
    pub task_type: Option<String>,
}

pub struct BackgroundManager {
    pub tasks: HashMap<String, BackgroundTask>,
    pub notification_queue: Vec<Notification>,
}

impl BackgroundManager {
    pub fn new() -> Self {
        Self {
            tasks: HashMap::new(),
            notification_queue: Vec::new(),
        }
    }

    fn detect_port_and_type(command: &str) -> (Option<u16>, Option<String>) {
        let lower = command.to_lowercase();
        
        let task_type = if lower.contains("npm run dev") || lower.contains("npm start") 
            || lower.contains("vite") || lower.contains("vue-cli-service serve")
            || lower.contains("next dev") || lower.contains("nuxt dev") {
            Some("frontend".to_string())
        } else if lower.contains("python") || lower.contains("flask") || lower.contains("uvicorn")
            || lower.contains("node ") || lower.contains("cargo run") {
            Some("backend".to_string())
        } else {
            None
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
        } else if lower.contains("vite") || lower.contains("npm run dev") {
            Some(5173)
        } else if lower.contains("next dev") {
            Some(3000)
        } else if lower.contains("nuxt dev") {
            Some(3000)
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

    pub async fn run(app: tauri::AppHandle, command: String, dir: Option<String>) -> String {
        let task_id = uuid::Uuid::new_v4().to_string()[..8].to_string();

        let mut short_cmd = command.clone();
        if short_cmd.len() > 80 {
            short_cmd.truncate(80);
            short_cmd.push_str("...");
        }

        let (detected_port, task_type) = Self::detect_port_and_type(&command);

        if let Some(state) = app.try_state::<BackgroundState>() {
            let state_clone = state.0.clone();
            let task_id_clone = task_id.clone();
            let cmd_clone = command.clone();

            {
                let mut bg = state_clone.lock().await;
                bg.tasks.insert(
                    task_id_clone.clone(),
                    BackgroundTask {
                        id: task_id_clone.clone(),
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

                let ps_cmd = format!(
                    "[Console]::OutputEncoding = [System.Text.Encoding]::UTF8; {}",
                    cmd_async
                );

                let mut child = match tokio::process::Command::new("powershell")
                    .current_dir(&target_dir)
                    .args(&["-NoProfile", "-Command", &ps_cmd])
                    .stdout(Stdio::piped())
                    .stderr(Stdio::piped())
                    .spawn()
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

                let output_buffer = String::new();
                
                let stdout = child.stdout.take();
                let stderr = child.stderr.take();
                
                if let Some(stdout) = stdout {
                    let reader = BufReader::new(stdout);
                    let mut lines = reader.lines();
                    let task_id_for_stdout = task_id_async.clone();
                    
                    tokio::spawn(async move {
                        while let Ok(Some(line)) = lines.next_line().await {
                            println!("[bg:{}] {}", task_id_for_stdout, line);
                        }
                    });
                }
                
                if let Some(stderr) = stderr {
                    let reader = BufReader::new(stderr);
                    let mut lines = reader.lines();
                    let task_id_for_stderr = task_id_async.clone();
                    
                    tokio::spawn(async move {
                        while let Ok(Some(line)) = lines.next_line().await {
                            println!("[bg:{} ERR] {}", task_id_for_stderr, line);
                        }
                    });
                }

                let status = match child.wait().await {
                    Ok(s) => {
                        if s.success() { "completed" } else { "error" }
                    }
                    Err(_) => "error",
                };

                if let Some(st) = app_handle.try_state::<BackgroundState>() {
                    let mut bg = st.0.lock().await;
                    if let Some(task) = bg.tasks.get_mut(&task_id_async) {
                        task.status = status.to_string();
                        task.result = if output_buffer.is_empty() {
                            Some("(process finished)".to_string())
                        } else {
                            Some(output_buffer)
                        };
                    }

                    bg.notification_queue.push(Notification {
                        task_id: task_id_async,
                        status: status.to_string(),
                        command: cmd_async,
                        result: "(process finished)".to_string(),
                        port: port_async,
                        task_type: type_async,
                    });
                }
            });
        }

        let type_info = task_type.as_ref().map(|t| format!(" [{}]", t)).unwrap_or_default();
        let port_info = detected_port.map(|p| format!(" :{}", p)).unwrap_or_default();
        format!("Background task {} started{}{}: {}", task_id, type_info, port_info, short_cmd)
    }

    pub async fn check(app: &tauri::AppHandle, task_id: Option<String>) -> String {
        if let Some(state) = app.try_state::<BackgroundState>() {
            let bg = state.0.lock().await;
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
}

pub struct BackgroundState(pub Arc<Mutex<BackgroundManager>>);

impl Default for BackgroundState {
    fn default() -> Self {
        Self(Arc::new(Mutex::new(BackgroundManager::new())))
    }
}
