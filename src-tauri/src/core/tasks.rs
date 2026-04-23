use std::fs;
use std::path::PathBuf;
use crate::core::models::{Task, TaskStatus};
use crate::get_agent_home;

pub struct TaskManager {
    pub dir: PathBuf,
}

impl TaskManager {
    pub fn new() -> Self {
        // 使用 Agent 启动时的家目录，而非 current_dir()，防止 set_workspace 后 .tasks 跑到用户项目中
        let dir = get_agent_home().join(crate::core::constants::DIR_TASKS);
        if !dir.exists() {
            let _ = fs::create_dir_all(&dir);
        }
        Self { dir }
    }

    fn _max_id(&self) -> i32 {
        let mut max_id = 0;
        if let Ok(entries) = fs::read_dir(&self.dir) {
            for entry in entries.flatten() {
                let stem = entry.path().file_stem().unwrap_or_default().to_string_lossy().into_owned();
                if stem.starts_with("task_") {
                    if let Ok(id) = stem[5..].parse::<i32>() {
                        if id > max_id { max_id = id; }
                    }
                }
            }
        }
        max_id
    }

    fn _load(&self, id: i32) -> Result<Task, String> {
        let path = self.dir.join(format!("task_{}.json", id));
        if !path.exists() {
            return Err(format!("Task {} not found", id));
        }
        let content = fs::read_to_string(path).map_err(|e| e.to_string())?;
        serde_json::from_str(&content).map_err(|e| e.to_string())
    }

    fn _save(&self, task: &Task) -> Result<(), String> {
        let path = self.dir.join(format!("task_{}.json", task.id));
        let content = serde_json::to_string_pretty(task).map_err(|e| e.to_string())?;
        fs::write(path, content).map_err(|e| e.to_string())
    }

    pub fn create(&self, subject: String, description: String) -> Result<String, String> {
        let next_id = self._max_id() + 1;
        let task = Task {
            id: next_id,
            subject,
            description,
            status: TaskStatus::Pending,
            blocked_by: Vec::new(),
            blocks: Vec::new(),
            owner: String::new(),
        };
        self._save(&task)?;
        Ok(serde_json::to_string_pretty(&task).unwrap_or_default())
    }

    pub fn get(&self, id: i32) -> Result<String, String> {
        let task = self._load(id)?;
        Ok(serde_json::to_string_pretty(&task).unwrap_or_default())
    }

    pub fn update(
        &self, 
        id: i32, 
        status: Option<TaskStatus>, 
        add_blocked_by: Option<Vec<i32>>, 
        add_blocks: Option<Vec<i32>>
    ) -> Result<String, String> {
        let mut task = self._load(id)?;
        let mut cascade_msg = String::new();

        if let Some(s) = status {
            task.status = s.clone();
            if s == TaskStatus::Completed {
                let unblocked_ids = self._clear_dependency(id)?;
                if !unblocked_ids.is_empty() {
                    cascade_msg = format!("\n\n[Cascade Impact] The following tasks are now UNBLOCKED and ready to start: {:?}", unblocked_ids);
                }
            }
        }

        if let Some(mut abb) = add_blocked_by {
            task.blocked_by.append(&mut abb);
            task.blocked_by.sort();
            task.blocked_by.dedup();
        }

        if let Some(ab) = add_blocks {
            for blocked_id in &ab {
                if let Ok(mut blocked_task) = self._load(*blocked_id) {
                    if !blocked_task.blocked_by.contains(&id) {
                        blocked_task.blocked_by.push(id);
                        blocked_task.blocked_by.sort();
                        blocked_task.blocked_by.dedup();
                        self._save(&blocked_task)?;
                    }
                }
            }
            let mut ab_clone = ab.clone();
            task.blocks.append(&mut ab_clone);
            task.blocks.sort();
            task.blocks.dedup();
        }

        self._save(&task)?;
        let mut result = serde_json::to_string_pretty(&task).unwrap_or_default();
        if !cascade_msg.is_empty() {
            result.push_str(&cascade_msg);
        }
        Ok(result)
    }

    fn _clear_dependency(&self, completed_id: i32) -> Result<Vec<i32>, String> {
        let mut unblocked_tasks = Vec::new();
        if let Ok(entries) = fs::read_dir(&self.dir) {
            for entry in entries.flatten() {
                let stem = entry.path().file_stem().unwrap_or_default().to_string_lossy().into_owned();
                if stem.starts_with("task_") {
                    if let Ok(id) = stem[5..].parse::<i32>() {
                        if let Ok(mut task) = self._load(id) {
                            if task.blocked_by.contains(&completed_id) {
                                task.blocked_by.retain(|&x| x != completed_id);
                                self._save(&task)?;
                                if task.blocked_by.is_empty() {
                                    unblocked_tasks.push(id);
                                }
                            }
                        }
                    }
                }
            }
        }
        Ok(unblocked_tasks)
    }

    pub fn summary(&self) -> Result<String, String> {
        let mut tasks = Vec::new();
        if let Ok(entries) = fs::read_dir(&self.dir) {
            for entry in entries.flatten() {
                let stem = entry.path().file_stem().unwrap_or_default().to_string_lossy().into_owned();
                if stem.starts_with("task_") {
                    if let Ok(id) = stem[5..].parse::<i32>() {
                        if let Ok(task) = self._load(id) {
                            tasks.push(task);
                        }
                    }
                }
            }
        }
        if tasks.is_empty() {
            return Ok("No tasks found.".to_string());
        }

        let total = tasks.len();
        let completed = tasks.iter().filter(|t| t.status == TaskStatus::Completed).count();
        let in_progress = tasks.iter().filter(|t| t.status == TaskStatus::InProgress).count();
        let pending = tasks.iter().filter(|t| t.status == TaskStatus::Pending).count();
        let percentage = if total > 0 { (completed as f32 / total as f32) * 100.0 } else { 0.0 };

        let mut ready_tasks: Vec<&Task> = tasks.iter()
            .filter(|t| t.status == TaskStatus::Pending && t.blocked_by.is_empty())
            .collect();
        ready_tasks.sort_by_key(|t| t.id);
        
        let mut active_tasks: Vec<&Task> = tasks.iter()
            .filter(|t| t.status != TaskStatus::Completed && !t.blocks.is_empty())
            .collect();
        active_tasks.sort_by_key(|t| std::cmp::Reverse(t.blocks.len()));

        let mut report = format!("### Task Summary\n");
        report.push_str(&format!("Progress: {:.1}% ({}/{})\n", percentage, completed, total));
        report.push_str(&format!("Status: {} Completed, {} In Progress, {} Pending\n\n", completed, in_progress, pending));
        
        if !active_tasks.is_empty() {
            report.push_str("🔥 Bottlenecks (Blocking others):\n");
            for t in active_tasks.iter().take(3) {
                report.push_str(&format!("  - Task #{} (Blocks {} tasks): {}\n", t.id, t.blocks.len(), t.subject));
            }
            report.push_str("\n");
        }

        if !ready_tasks.is_empty() {
            report.push_str("✅ Ready to Start (No blocked_by):\n");
            for t in ready_tasks.iter().take(3) {
                report.push_str(&format!("  - Task #{}: {}\n", t.id, t.subject));
            }
        } else if pending > 0 || in_progress > 0 {
            report.push_str("⏳ No completely unblocked pending tasks. Check bottlenecks!\n");
        } else {
            report.push_str("🎉 All tasks completed!\n");
        }

        Ok(report)
    }

    pub fn list_all(&self) -> Result<String, String> {
        let mut tasks = Vec::new();
        if let Ok(entries) = fs::read_dir(&self.dir) {
            for entry in entries.flatten() {
                let stem = entry.path().file_stem().unwrap_or_default().to_string_lossy().into_owned();
                if stem.starts_with("task_") {
                    if let Ok(id) = stem[5..].parse::<i32>() {
                        if let Ok(task) = self._load(id) {
                            tasks.push(task);
                        }
                    }
                }
            }
        }
        tasks.sort_by_key(|t| t.id);
        
        if tasks.is_empty() {
            return Ok("No tasks found.".to_string());
        }

        let mut lines = Vec::new();
        for t in tasks {
            let marker = match t.status {
                TaskStatus::Pending => "[ ]",
                TaskStatus::InProgress => "[>]",
                TaskStatus::Completed => "[x]",
            };
            let blocked = if t.blocked_by.is_empty() {
                String::new()
            } else {
                format!(" (blocked by: {:?})", t.blocked_by)
            };
            lines.push(format!("{} #{} {}{}", marker, t.id, t.subject, blocked));
        }
        Ok(lines.join("\n"))
    }
}
