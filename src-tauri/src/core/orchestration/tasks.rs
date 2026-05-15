//! 任务管理器 - 任务生命周期与依赖关系管理
//!
//! 提供任务的创建、查询、更新、删除等 CRUD 操作。
//! 支持任务间依赖关系（blocked_by/blocks）和级联解锁机制。
//! 任务以 JSON 文件形式持久化存储。

use crate::infra::types::models::{Task, TaskStatus};
use crate::core::session::resource_repository;

/// 任务管理器 - 基于 SQLite 的任务持久化
pub struct TaskManager {
    session_id: String,
}

/// update() 的可选参数集合
pub struct TaskUpdateParams {
    pub status: Option<TaskStatus>,
    pub subject: Option<String>,
    pub description: Option<String>,
    pub active_form: Option<String>,
    pub owner: Option<String>,
    pub add_blocked_by: Option<Vec<i32>>,
    pub add_blocks: Option<Vec<i32>>,
    pub metadata: Option<serde_json::Value>,
}

impl TaskManager {
    /// 创建任务管理器实例
    ///
    /// 使用 Agent 启动时的数据目录，而非 current_dir()，
    /// 防止 set_workspace 后 tasks 跑到用户项目中
    pub fn for_session(session_id: &str) -> Self {
        Self {
            session_id: session_id.to_string(),
        }
    }

    fn _load(&self, id: i32) -> Result<Task, String> {
        resource_repository::load_task(&self.session_id, id)?
            .ok_or_else(|| format!("Task {} not found", id))
    }

    fn _save(&self, task: &Task) -> Result<(), String> {
        resource_repository::save_task(&self.session_id, task)
    }

    fn _delete_file(&self, id: i32) -> Result<(), String> {
        resource_repository::delete_task(&self.session_id, id)
    }

    /// 创建任务，支持 activeForm、metadata、owner
    pub fn create(
        &self,
        subject: String,
        description: String,
        active_form: Option<String>,
        metadata: Option<serde_json::Value>,
        owner: Option<String>,
    ) -> Result<Task, String> {
        // 原子分配 ID + 保存：利用全局 DB Mutex 保证 max_id 查询和 insert 之间无竞态
        let task = Task {
            id: 0, // 占位，由 save_task_with_auto_id 分配真实 ID
            subject,
            description,
            status: TaskStatus::Pending,
            blocked_by: Vec::new(),
            blocks: Vec::new(),
            owner: owner.unwrap_or_default(),
            active_form,
            metadata,
        };
        self._save_atomic(&task)
    }

    /// 原子保存：在一个 with_connection 内完成 max_id + 1 + insert
    fn _save_atomic(&self, task: &Task) -> Result<Task, String> {
        resource_repository::save_task_with_auto_id(&self.session_id, task)
    }

    /// 获取单个任务详情
    pub fn get(&self, id: i32) -> Result<Task, String> {
        self._load(id)
    }

    /// 更新任务：支持所有字段的增量更新
    ///
    /// - status='deleted' 时执行硬删除
    /// - status='in_progress' 且无 owner 时自动设置 owner
    /// - status='completed' 时触发级联解锁
    pub fn update(&self, id: i32, params: TaskUpdateParams) -> Result<TaskUpdateResult, String> {
        let mut task = self._load(id)?;
        let mut updated_fields: Vec<String> = Vec::new();
        let mut cascade_msg = String::new();
        let old_status = task.status.clone();

        // 更新 status
        if let Some(s) = params.status {
            if s != task.status {
                task.status = s.clone();
                updated_fields.push("status".to_string());
                if s == TaskStatus::Completed {
                    let unblocked_ids = self._clear_dependency(id)?;
                    if !unblocked_ids.is_empty() {
                        cascade_msg = format!(
                            "\n\n[Cascade Impact] The following tasks are now UNBLOCKED and ready to start: {:?}",
                            unblocked_ids
                        );
                    }
                }
            }
        }

        // 更新 subject
        if let Some(subject) = params.subject {
            if subject != task.subject {
                task.subject = subject;
                updated_fields.push("subject".to_string());
            }
        }

        // 更新 description
        if let Some(description) = params.description {
            if description != task.description {
                task.description = description;
                updated_fields.push("description".to_string());
            }
        }

        // 更新 active_form
        if let Some(active_form) = params.active_form {
            task.active_form = Some(active_form);
            updated_fields.push("activeForm".to_string());
        }

        // 更新 owner
        if let Some(owner) = params.owner {
            if owner != task.owner {
                task.owner = owner;
                updated_fields.push("owner".to_string());
            }
        }

        // 自动设置 owner：标记 in_progress 时若无 owner 则自动填充
        if task.status == TaskStatus::InProgress && task.owner.is_empty() {
            task.owner = "main-agent".to_string();
            updated_fields.push("owner".to_string());
        }

        // 合并 metadata
        if let Some(new_meta) = params.metadata {
            let merged = match task.metadata {
                Some(existing) => merge_metadata(existing, new_meta),
                None => new_meta,
            };
            task.metadata = Some(merged);
            updated_fields.push("metadata".to_string());
        }

        // 依赖引用校验：引用的 task ID 必须存在，且不能自引用
        let mut invalid_ids: Vec<i32> = Vec::new();
        if let Some(ref abb) = params.add_blocked_by {
            for &dep_id in abb {
                if dep_id == id {
                    return Err(format!("任务不能依赖自身：blocked_by 中包含自己的 ID ({})", id));
                }
                if self._load(dep_id).is_err() {
                    invalid_ids.push(dep_id);
                }
            }
        }
        if let Some(ref ab) = params.add_blocks {
            for &blocked_id in ab {
                if blocked_id == id {
                    return Err(format!("任务不能阻塞自身：add_blocks 中包含自己的 ID ({})", id));
                }
                if self._load(blocked_id).is_err() {
                    invalid_ids.push(blocked_id);
                }
            }
        }
        if !invalid_ids.is_empty() {
            invalid_ids.sort();
            invalid_ids.dedup();
            return Err(format!(
                "以下任务 ID 不存在：{:?}。请用 ListTasks 确认已有任务的 ID，或先 CreateTask 创建这些前置任务。",
                invalid_ids
            ));
        }

        // 添加 blocked_by
        if let Some(mut abb) = params.add_blocked_by {
            task.blocked_by.append(&mut abb);
            task.blocked_by.sort();
            task.blocked_by.dedup();
            updated_fields.push("blockedBy".to_string());
        }

        // 添加 blocks（同时更新被阻塞任务的 blocked_by）
        if let Some(ab) = params.add_blocks {
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
            updated_fields.push("blocks".to_string());
        }

        self._save(&task)?;

        let status_change = if old_status != task.status {
            Some(StatusChange {
                from: old_status,
                to: format!("{:?}", task.status).to_lowercase(),
            })
        } else {
            None
        };

        Ok(TaskUpdateResult {
            task_id: id,
            updated_fields,
            success: true,
            error: None,
            status_change,
            cascade_message: if cascade_msg.is_empty() {
                None
            } else {
                Some(cascade_msg)
            },
        })
    }

    /// 硬删除任务
    pub fn delete(&self, id: i32) -> Result<bool, String> {
        let _ = self._load(id)?; // 确认存在
        self._remove_all_references(id)?;
        self._delete_file(id)?;
        Ok(true)
    }

    /// 清理所有对指定任务 ID 的引用（blocked_by 和 blocks）
    fn _remove_all_references(&self, target_id: i32) -> Result<(), String> {
        for mut task in self._load_all_tasks() {
            if task.id == target_id {
                continue;
            }
            let mut changed = false;
            if task.blocked_by.contains(&target_id) {
                task.blocked_by.retain(|&x| x != target_id);
                changed = true;
            }
            if task.blocks.contains(&target_id) {
                task.blocks.retain(|&x| x != target_id);
                changed = true;
            }
            if changed {
                self._save(&task)?;
            }
        }
        Ok(())
    }

    fn _clear_dependency(&self, completed_id: i32) -> Result<Vec<i32>, String> {
        let mut unblocked_tasks = Vec::new();
        for mut task in self._load_all_tasks() {
            if task.blocked_by.contains(&completed_id) {
                task.blocked_by.retain(|&x| x != completed_id);
                self._save(&task)?;
                if task.blocked_by.is_empty() {
                    unblocked_tasks.push(task.id);
                }
            }
        }
        Ok(unblocked_tasks)
    }

    pub fn summary(&self) -> Result<String, String> {
        let tasks = self._load_all_tasks();
        if tasks.is_empty() {
            return Ok("No tasks found.".to_string());
        }

        let total = tasks.len();
        let completed = tasks
            .iter()
            .filter(|t| t.status == TaskStatus::Completed)
            .count();
        let in_progress = tasks
            .iter()
            .filter(|t| t.status == TaskStatus::InProgress)
            .count();
        let pending = tasks
            .iter()
            .filter(|t| t.status == TaskStatus::Pending)
            .count();
        let percentage = if total > 0 {
            (completed as f32 / total as f32) * 100.0
        } else {
            0.0
        };

        let mut ready_tasks: Vec<&Task> = tasks
            .iter()
            .filter(|t| t.status == TaskStatus::Pending && t.blocked_by.is_empty())
            .collect();
        ready_tasks.sort_by_key(|t| t.id);

        let mut active_tasks: Vec<&Task> = tasks
            .iter()
            .filter(|t| t.status != TaskStatus::Completed && !t.blocks.is_empty())
            .collect();
        active_tasks.sort_by_key(|t| std::cmp::Reverse(t.blocks.len()));

        let mut report = format!("### Task Summary\n");
        report.push_str(&format!(
            "Progress: {:.1}% ({}/{})\n",
            percentage, completed, total
        ));
        report.push_str(&format!(
            "Status: {} Completed, {} In Progress, {} Pending\n\n",
            completed, in_progress, pending
        ));

        if !active_tasks.is_empty() {
            report.push_str("[!] Bottlenecks (Blocking others):\n");
            for t in active_tasks.iter().take(3) {
                report.push_str(&format!(
                    "  - Task #{} (Blocks {} tasks): {}\n",
                    t.id,
                    t.blocks.len(),
                    t.subject
                ));
            }
            report.push_str("\n");
        }

        if !ready_tasks.is_empty() {
            report.push_str("◈ Ready to Start (No blocked_by):\n");
            for t in ready_tasks.iter().take(3) {
                report.push_str(&format!("  - Task #{}: {}\n", t.id, t.subject));
            }
        } else if pending > 0 || in_progress > 0 {
            report.push_str("⏳ No completely unblocked pending tasks. Check bottlenecks!\n");
        } else {
            report.push_str("[OK] All tasks completed!\n");
        }

        Ok(report)
    }

    /// 获取所有非删除任务（返回 Task 结构体列表，内部用）
    fn _load_all_tasks(&self) -> Vec<Task> {
        let mut tasks = resource_repository::list_tasks(&self.session_id).unwrap_or_default();
        tasks.sort_by_key(|t| t.id);
        tasks
    }

    /// 获取所有任务
    pub fn get_all_tasks(&self) -> Vec<Task> {
        self._load_all_tasks()
    }

    /// 获取所有就绪任务（Pending 且 blocked_by 为空）
    pub fn get_ready_tasks(&self) -> Vec<Task> {
        self._load_all_tasks()
            .into_iter()
            .filter(|t| t.status == TaskStatus::Pending && t.blocked_by.is_empty())
            .collect()
    }

    /// 统计未完成任务数（Pending + InProgress）
    pub fn count_incomplete(&self) -> usize {
        self._load_all_tasks()
            .into_iter()
            .filter(|t| t.status != TaskStatus::Completed)
            .count()
    }

    /// 列出所有任务，智能过滤 blockedBy 中已完成的 ID
    pub fn list_all(&self) -> Result<String, String> {
        let tasks = self._load_all_tasks();

        if tasks.is_empty() {
            return Ok("No tasks found.".to_string());
        }

        // 构建已完成任务 ID 集合，用于过滤 blockedBy 显示
        let completed_ids: std::collections::HashSet<i32> = tasks
            .iter()
            .filter(|t| t.status == TaskStatus::Completed)
            .map(|t| t.id)
            .collect();

        let mut lines = Vec::new();
        for t in &tasks {
            let marker = match t.status {
                TaskStatus::Pending => "[ ]",
                TaskStatus::InProgress => "[>]",
                TaskStatus::Completed => "[x]",
            };
            let owner_str = if t.owner.is_empty() {
                String::new()
            } else {
                format!(" ({})", t.owner)
            };
            let display_subject = if t.status == TaskStatus::InProgress {
                t.active_form.as_deref().unwrap_or(&t.subject)
            } else {
                &t.subject
            };
            // 智能过滤：只显示尚未完成的 blocker
            let active_blockers: Vec<i32> = t
                .blocked_by
                .iter()
                .filter(|id| !completed_ids.contains(id))
                .cloned()
                .collect();
            let blocked = if active_blockers.is_empty() {
                String::new()
            } else {
                format!(" [blocked by {:?}]", active_blockers)
            };
            lines.push(format!(
                "{} #{} {}{}{}",
                marker, t.id, display_subject, owner_str, blocked
            ));
        }
        Ok(lines.join("\n"))
    }
}

/// update() 的返回结构
pub struct TaskUpdateResult {
    pub task_id: i32,
    pub updated_fields: Vec<String>,
    pub success: bool,
    pub error: Option<String>,
    pub status_change: Option<StatusChange>,
    pub cascade_message: Option<String>,
}

pub struct StatusChange {
    pub from: TaskStatus,
    pub to: String,
}

/// 合并两个 JSON 对象（浅合并，null 值删除 key）
fn merge_metadata(existing: serde_json::Value, new: serde_json::Value) -> serde_json::Value {
    use serde_json::Value;
    match (existing, new) {
        (Value::Object(mut base), Value::Object(updates)) => {
            for (key, value) in updates {
                if value.is_null() {
                    base.remove(&key);
                } else {
                    base.insert(key, value);
                }
            }
            Value::Object(base)
        }
        // 如果不是对象类型，直接用新的覆盖
        (_, new_val) => new_val,
    }
}
