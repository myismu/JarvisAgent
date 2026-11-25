//! 任务管理器 - 任务生命周期与依赖关系管理
//!
//! 提供任务的创建、查询、更新、删除等 CRUD 操作。
//! 支持任务间依赖关系（blocked_by/blocks）和级联解锁机制。
//! 任务以 JSON 文件形式持久化存储。

use crate::core::session::resource_repository;
use crate::infra::types::models::{PlanDocument, Task, TaskStatus};

/// 一条待创建任务的规格。
///
/// `CreateTask` 工具与“方案审批自动建任务”共用同一套建图逻辑，避免两处各写一份
/// 1-based 依赖换算。
pub struct TaskSpec {
    pub subject: String,
    pub description: String,
    pub active_form: Option<String>,
    pub metadata: Option<serde_json::Value>,
    pub owner: Option<String>,
    pub subagent_type: Option<String>,
    /// 依赖的序号（1-based，指向同一批 specs）
    pub depends_on: Vec<usize>,
}

/// 批量建图结果。`errors` 非空表示有任务创建或依赖设置失败。
pub struct TaskGraphResult {
    pub created: Vec<Task>,
    pub errors: Vec<String>,
}

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
    /// 指定该任务由哪种子代理执行（`None` = 不修改）。2026-09-20 新增。
    pub subagent_type: Option<String>,
}

impl TaskManager {
    /// 创建任务管理器实例
    ///
    /// 使用 Agent 启动时的数据目录，而非 current_dir()，
    /// 防止进程 CWD 变化后 tasks 跑到用户项目中
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

    /// 创建任务，支持 activeForm、metadata、owner、subagent_type
    pub fn create(
        &self,
        subject: String,
        description: String,
        active_form: Option<String>,
        metadata: Option<serde_json::Value>,
        owner: Option<String>,
        subagent_type: Option<String>,
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
            subagent_type,
        };
        self._save_atomic(&task)
    }

    /// 原子保存：在一个 with_connection 内完成 max_id + 1 + insert
    fn _save_atomic(&self, task: &Task) -> Result<Task, String> {
        resource_repository::save_task_with_auto_id(&self.session_id, task)
    }

    /// 批量创建任务并建立依赖关系。
    ///
    /// 依赖用**批次内 1-based 序号**表达：必须等全部任务创建完、拿到真实 id 后
    /// 才能换算，所以建任务与挂依赖分成两趟。
    ///
    /// 空 subject 的条目按历史行为跳过且不报错，但它**占住自己的序号**：
    /// 引用它的 depends_on 会被丢弃，而不是错挂到下一个任务上。
    pub fn create_graph(&self, specs: &[TaskSpec]) -> TaskGraphResult {
        let mut ids: Vec<Option<i32>> = vec![None; specs.len()];
        let mut created = Vec::new();
        let mut errors = Vec::new();

        for (index, spec) in specs.iter().enumerate() {
            let subject = spec.subject.trim();
            if subject.is_empty() {
                continue;
            }
            match self.create(
                subject.to_string(),
                spec.description.clone(),
                spec.active_form.clone(),
                spec.metadata.clone(),
                spec.owner.clone(),
                spec.subagent_type.clone(),
            ) {
                Ok(task) => {
                    ids[index] = Some(task.id);
                    created.push(task);
                }
                Err(error) => errors.push(format!("「{}」: {}", subject, error)),
            }
        }

        for (index, spec) in specs.iter().enumerate() {
            let Some(task_id) = ids[index] else {
                continue;
            };
            let blocked_by: Vec<i32> = spec
                .depends_on
                .iter()
                .filter_map(|dep| if *dep == 0 { None } else { Some(*dep - 1) })
                .filter_map(|dep_index| ids.get(dep_index).copied().flatten())
                .collect();
            if blocked_by.is_empty() {
                continue;
            }
            if let Err(error) = self.update(
                task_id,
                TaskUpdateParams {
                    status: None,
                    subject: None,
                    description: None,
                    active_form: None,
                    owner: None,
                    metadata: None,
                    add_blocked_by: Some(blocked_by),
                    add_blocks: None,
                    subagent_type: None,
                },
            ) {
                errors.push(format!("任务 #{} 设置依赖失败: {}", task_id, error));
            }
        }

        TaskGraphResult { created, errors }
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

        // 更新 subagent_type（指定该任务由哪种子代理执行；空串视为不修改）
        if let Some(role) = params.subagent_type {
            let role = role.trim().to_string();
            if !role.is_empty() && task.subagent_type.as_deref() != Some(role.as_str()) {
                task.subagent_type = Some(role);
                updated_fields.push("subagentType".to_string());
            }
        }

        // 依赖引用校验：引用的 task ID 必须存在，且不能自引用
        let mut invalid_ids: Vec<i32> = Vec::new();
        if let Some(ref abb) = params.add_blocked_by {
            for &dep_id in abb {
                if dep_id == id {
                    return Err(format!(
                        "任务不能依赖自身：blocked_by 中包含自己的 ID ({})",
                        id
                    ));
                }
                if self._load(dep_id).is_err() {
                    invalid_ids.push(dep_id);
                }
            }
        }
        if let Some(ref ab) = params.add_blocks {
            for &blocked_id in ab {
                if blocked_id == id {
                    return Err(format!(
                        "任务不能阻塞自身：add_blocks 中包含自己的 ID ({})",
                        id
                    ));
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

        let mut report = "### Task Summary\n".to_string();
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
            report.push('\n');
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

/// 方案审批通过后，把方案里的结构化任务分解同步成任务图。
///
/// 返回值语义：
/// - `Some(tasks)`：本方案的任务图已就绪，续跑应直接 `RunSubagentsSequentially`；
///   调用方还会拿这份任务列表投影成 TODO 清单；
/// - `None`：方案没有结构化任务（存量方案），或建任务失败。续跑走兼容分支，
///   由模型按正文里的任务分解自行 `CreateTask`。
///
/// 幂等：同一 `plan_id` 重复审批不会重复建任务。
/// 清理：旧方案遗留的 **pending** 任务会被删除（新方案已取代它，不该再被调度）；
/// in_progress / completed 保留，作为历史记录。
pub fn sync_plan_tasks(session_id: &str, plan: &PlanDocument) -> Option<Vec<Task>> {
    if plan.tasks.is_empty() {
        return None;
    }

    let manager = TaskManager::for_session(session_id);
    let existing = manager.get_all_tasks();

    // 幂等：同一方案重复审批（双击、退回后再点）直接复用已有任务。
    let mut existing_for_plan: Vec<Task> = existing
        .iter()
        .filter(|task| plan_id_of(task) == Some(plan.id.as_str()))
        .cloned()
        .collect();
    if !existing_for_plan.is_empty() {
        // 按方案原始序号恢复顺序，保证 TODO 清单与方案里的任务顺序一致。
        existing_for_plan.sort_by_key(|task| {
            task.metadata
                .as_ref()
                .and_then(|meta| meta.get("planIndex"))
                .and_then(|value| value.as_u64())
                .unwrap_or(task.id as u64)
        });
        return Some(existing_for_plan);
    }

    // 清理旧方案遗留的 pending 任务。只删 pending：已执行过的任务要留在数据库里。
    for task_id in stale_pending_plan_task_ids(&existing, &plan.id) {
        if let Err(error) = manager.delete(task_id) {
            eprintln!("[JARVIS] 清理旧方案任务 #{} 失败：{}", task_id, error);
        }
    }

    let specs = plan_tasks_to_specs(plan);
    let result = manager.create_graph(&specs);
    if !result.errors.is_empty() {
        eprintln!(
            "[JARVIS] 方案「{}」自动建任务失败：{:?}",
            plan.title, result.errors
        );
        // 全有或全无：回滚已建的部分，避免只建了一半、续跑文案却宣称“任务已创建”。
        // 失败后返回 None，续跑走兼容分支让模型重新 CreateTask。
        for task in &result.created {
            let _ = manager.delete(task.id);
        }
        return None;
    }

    Some(result.created)
}

/// 任务来自哪个方案（无方案来源的任务返回 None）。
fn plan_id_of(task: &Task) -> Option<&str> {
    task.metadata.as_ref()?.get("planId")?.as_str()
}

/// 需要清理的旧方案 pending 任务 id。
///
/// 只处理**带 planId 且不属于当前方案**的任务：
/// - 手工创建的任务（没有 planId）不动；
/// - in_progress / completed 不动。
fn stale_pending_plan_task_ids(tasks: &[Task], active_plan_id: &str) -> Vec<i32> {
    tasks
        .iter()
        .filter(|task| task.status == TaskStatus::Pending)
        .filter(|task| {
            plan_id_of(task)
                .map(|plan_id| plan_id != active_plan_id)
                .unwrap_or(false)
        })
        .map(|task| task.id)
        .collect()
}

/// 把方案里的任务分解转换成建图规格。
///
/// metadata 里的 `planId` 是幂等与旧任务清理的识别依据；`planIndex` 保留任务在方案里的
/// 原始序号，便于排障时反查。
fn plan_tasks_to_specs(plan: &PlanDocument) -> Vec<TaskSpec> {
    plan.tasks
        .iter()
        .enumerate()
        .map(|(index, task)| TaskSpec {
            subject: task.subject.clone(),
            description: task.description.clone(),
            active_form: None,
            metadata: Some(serde_json::json!({
                "source": "plan",
                "planId": plan.id,
                "planIndex": index + 1,
            })),
            owner: None,
            subagent_type: None,
            depends_on: task.depends_on.clone(),
        })
        .collect()
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

#[cfg(test)]
mod plan_task_sync_tests {
    use super::*;
    use crate::infra::types::models::PlanTask;

    fn task(id: i32, plan_id: Option<&str>, status: TaskStatus) -> Task {
        Task {
            id,
            subject: format!("任务 {}", id),
            description: String::new(),
            status,
            blocked_by: Vec::new(),
            blocks: Vec::new(),
            owner: String::new(),
            active_form: None,
            metadata: plan_id.map(|plan_id| serde_json::json!({ "planId": plan_id })),
            subagent_type: None,
        }
    }

    fn plan(id: &str, tasks: Vec<PlanTask>) -> PlanDocument {
        PlanDocument {
            id: id.to_string(),
            session_id: "session-1".to_string(),
            title: "测试方案".to_string(),
            content: String::new(),
            status: "approved".to_string(),
            path: None,
            created_at: 0,
            updated_at: 0,
            decided_at: None,
            rejection_feedback: None,
            tasks,
        }
    }

    /// 旧方案清理只针对“其他方案的 pending 任务”：
    /// 手工任务（无 planId）、已执行任务、当前方案任务都必须保留。
    #[test]
    fn stale_pending_only_targets_other_plan_pending_tasks() {
        let tasks = vec![
            task(1, Some("plan_old"), TaskStatus::Pending),
            task(2, Some("plan_old"), TaskStatus::Completed),
            task(3, Some("plan_new"), TaskStatus::Pending),
            task(4, None, TaskStatus::Pending),
        ];

        assert_eq!(stale_pending_plan_task_ids(&tasks, "plan_new"), vec![1]);
    }

    /// 方案任务转建图规格时，必须带 planId（幂等/清理依据）与 1-based depends_on。
    #[test]
    fn plan_specs_carry_plan_metadata_and_dependencies() {
        let plan = plan(
            "plan_1",
            vec![
                PlanTask {
                    subject: "#1 建库".to_string(),
                    description: "建表".to_string(),
                    depends_on: Vec::new(),
                    can_parallel_with: Vec::new(),
                },
                PlanTask {
                    subject: "#2 写接口".to_string(),
                    description: "路由".to_string(),
                    depends_on: vec![1],
                    can_parallel_with: Vec::new(),
                },
            ],
        );

        let specs = plan_tasks_to_specs(&plan);
        assert_eq!(specs.len(), 2);
        assert_eq!(specs[1].depends_on, vec![1]);

        let metadata = specs[0].metadata.as_ref().expect("方案任务必须带 metadata");
        assert_eq!(metadata["planId"].as_str(), Some("plan_1"));
        assert_eq!(metadata["planIndex"].as_u64(), Some(1));
        assert_eq!(metadata["source"].as_str(), Some("plan"));
    }
}
