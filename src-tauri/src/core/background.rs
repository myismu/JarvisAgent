use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::Mutex;
use tauri::Manager;

#[derive(Clone, Debug)]
pub struct BackgroundTask {
    pub id: String,
    pub command: String,
    pub status: String,
    pub result: Option<String>,
}

#[derive(Clone, Debug)]
pub struct Notification {
    pub task_id: String,
    pub status: String,
    pub command: String,
    pub result: String,
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

    pub async fn run(app: tauri::AppHandle, command: String, dir: Option<String>) -> String {
        let task_id = uuid::Uuid::new_v4().to_string()[..8].to_string();
        
        let mut short_cmd = command.clone();
        if short_cmd.len() > 80 {
            short_cmd.truncate(80);
            short_cmd.push_str("...");
        }
        
        if let Some(state) = app.try_state::<BackgroundState>() {
            let state_clone = state.0.clone();
            let task_id_clone = task_id.clone();
            let cmd_clone = command.clone();
            
            // Register the task
            {
                let mut bg = state_clone.lock().await;
                bg.tasks.insert(task_id_clone.clone(), BackgroundTask {
                    id: task_id_clone.clone(),
                    command: cmd_clone.clone(),
                    status: "running".to_string(),
                    result: None,
                });
            }

            let app_handle = app.clone();
            let task_id_async = task_id.clone();
            let cmd_async = command.clone();
            let dir_async = dir.clone();
            
            tokio::spawn(async move {
                let target_dir = dir_async.unwrap_or_else(|| {
                    std::env::current_dir().unwrap_or_default().to_string_lossy().to_string()
                });
                
                let ps_cmd = format!("[Console]::OutputEncoding = [System.Text.Encoding]::UTF8; {}", cmd_async);
                
                let output_res = tokio::process::Command::new("powershell")
                    .current_dir(&target_dir)
                    .args(&["-NoProfile", "-Command", &ps_cmd])
                    .output()
                    .await;

                let (status_str, result_str) = match output_res {
                    Ok(out) => {
                        let mut res = String::from_utf8_lossy(&out.stdout).into_owned();
                        let err = String::from_utf8_lossy(&out.stderr).into_owned();
                        if !err.is_empty() {
                            if !res.is_empty() { res.push('\n'); }
                            res.push_str(&err);
                        }
                        if res.len() > crate::core::constants::MAX_BACKGROUND_OUTPUT_LEN {
                            res.truncate(crate::core::constants::MAX_BACKGROUND_OUTPUT_LEN);
                            res.push_str("... (truncated)");
                        }
                        if res.is_empty() {
                            res = "(no output)".to_string();
                        }
                        let status = if out.status.success() { "completed" } else { "error" };
                        (status.to_string(), res)
                    }
                    Err(e) => {
                        ("error".to_string(), format!("Error: {}", e))
                    }
                };

                if let Some(st) = app_handle.try_state::<BackgroundState>() {
                    let mut bg = st.0.lock().await;
                    if let Some(task) = bg.tasks.get_mut(&task_id_async) {
                        task.status = status_str.clone();
                        task.result = Some(result_str.clone());
                    }
                    
                    let mut short_res = result_str;
                    if short_res.len() > crate::core::constants::MAX_BACKGROUND_NOTIFY_LEN {
                        short_res.truncate(crate::core::constants::MAX_BACKGROUND_NOTIFY_LEN);
                        short_res.push_str("...(truncated)");
                    }
                    
                    bg.notification_queue.push(Notification {
                        task_id: task_id_async,
                        status: status_str,
                        command: cmd_async,
                        result: short_res,
                    });
                }
            });
        }
        
        format!("Background task {} started: {}", task_id, short_cmd)
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
                    format!("[{}] {}\n{}", t.status, short_cmd, t.result.as_deref().unwrap_or("(running)"))
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
