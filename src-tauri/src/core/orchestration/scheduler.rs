//! 任务调度器模块 - 基于依赖图的并行任务调度
//!
//! 核心调度算法：循环检测就绪任务 → 并行派发子Agent → 等待完成 → 级联解锁 → 继续
//! 支持取消、循环依赖检测、批量并行执行。

// --- 任务调度器模块 ---
// 基于依赖图自动调度可并行的任务

use tauri::Emitter;

use crate::core::models::TaskStatus;
use crate::core::orchestration::tasks::{TaskManager, TaskUpdateParams};
use crate::core::tools::{run_subagent, IMPLEMENTATION_AGENT_TYPE};

/// 任务调度器：基于依赖图自动调度可并行的任务
pub struct TaskScheduler;

impl TaskScheduler {
    /// 执行调度循环：持续运行直到所有任务完成或取消
    ///
    /// 调度算法：
    /// 1. 获取所有就绪任务（Pending + blocked_by 为空）
    /// 2. 将就绪任务标记为 InProgress，并行 spawn 子Agent
    /// 3. join_all 等待本批次全部完成
    /// 4. 子Agent完成后自动标记 Completed，触发级联解锁
    /// 5. 回到步骤1，发现新解锁的任务，继续调度
    /// 6. 直到所有任务完成或检测到循环依赖
    pub async fn run_schedule(
        app: &tauri::AppHandle,
        session_id: &str,
        _run_id: &str,
        cancel_token: &tokio_util::sync::CancellationToken,
    ) -> (String, u64, u64) {
        let tm = TaskManager::for_session(session_id);
        let mut total_in: u64 = 0;
        let mut total_out: u64 = 0;
        let mut completed_count: usize = 0;
        let mut failed_count: usize = 0;
        let mut round: usize = 0;

        loop {
            if cancel_token.is_cancelled() {
                println!("[SCHEDULER] 调度被取消");
                break;
            }

            // 1. 获取所有就绪任务（Pending + blocked_by 为空）
            let ready_tasks = tm.get_ready_tasks();
            if ready_tasks.is_empty() {
                // 没有就绪任务，检查是否还有未完成的
                let remaining = tm.count_incomplete();
                if remaining == 0 {
                    println!("[SCHEDULER] 所有任务已完成");
                    break;
                }
                // 有未完成但无就绪 → 循环依赖
                let msg = format!(
                    "[SCHEDULER] 检测到循环依赖或无法调度的任务，{} 个任务未完成",
                    remaining
                );
                println!("{}", msg);
                return (msg, total_in, total_out);
            }

            round += 1;
            let batch_count = ready_tasks.len();
            println!(
                "[SCHEDULER] 轮次 {}：发现 {} 个就绪任务，开始并行执行",
                round, batch_count
            );

            // 2. 将所有就绪任务标记为 InProgress
            for task in &ready_tasks {
                let _ = tm.update(
                    task.id,
                    TaskUpdateParams {
                        status: Some(TaskStatus::InProgress),
                        subject: None,
                        description: None,
                        active_form: None,
                        owner: None,
                        add_blocked_by: None,
                        add_blocks: None,
                        metadata: None,
                    },
                );
                let _ = app.emit(
                    "agent-step",
                    serde_json::json!({
                        "type": "task_scheduled",
                        "taskId": task.id,
                        "subject": task.subject,
                        "sessionId": session_id,
                    }),
                );
            }

            // 3. 流式调度：JoinSet 替代 join_all，谁先完成就立刻解锁下游
            let mut set = tokio::task::JoinSet::new();
            for task in ready_tasks {
                let app_clone = app.clone();
                let sid = session_id.to_string();
                let prompt = if task.description.is_empty() {
                    task.subject.clone()
                } else {
                    format!("{}\n\n{}", task.subject, task.description)
                };
                let task_id = task.id;
                let task_subject = task.subject.clone();
                set.spawn(async move {
                    println!(
                        "[SCHEDULER] 启动子Agent: Task #{} - {}",
                        task_id, task_subject
                    );
                    let label = format!("Task #{}: {}", task_id, task_subject);
                    let result = tokio::time::timeout(
                        std::time::Duration::from_secs(300), // 5 分钟超时
                        run_subagent(
                            app_clone,
                            prompt,
                            false,
                            sid,
                            Some(task_id),
                            Some(label),
                            Some(IMPLEMENTATION_AGENT_TYPE.to_string()),
                            None,
                        ),
                    )
                    .await;
                    let (answer, si, so) = match result {
                        Ok(r) => r,
                        Err(_) => (format!("任务超时（超过 5 分钟）"), 0, 0),
                    };
                    (task_id, answer, si, so)
                });
            }

            // 4. 流式调度：完成一个立即解锁下游并检查新就绪任务
            while let Some(result) = set.join_next().await {
                if let Ok((task_id, answer, si, so)) = result {
                    total_in += si;
                    total_out += so;

                    let status_msg = if answer.contains("任务超时") {
                        failed_count += 1;
                        "超时"
                    } else if answer.contains("子代理已取消") {
                        failed_count += 1;
                        "取消"
                    } else if answer.contains("子代理执行达到") {
                        failed_count += 1;
                        "超限"
                    } else {
                        completed_count += 1;
                        "完成"
                    };

                    let _ = tm.update(
                        task_id,
                        TaskUpdateParams {
                            status: Some(TaskStatus::Completed),
                            subject: None,
                            description: None,
                            active_form: None,
                            owner: None,
                            add_blocked_by: None,
                            add_blocks: None,
                            metadata: None,
                        },
                    );

                    println!(
                        "[SCHEDULER] Task #{} {} (input: {}, output: {} tokens)",
                        task_id, status_msg, si, so
                    );

                    let _ = app.emit(
                        "agent-step",
                        serde_json::json!({
                            "type": "task_completed",
                            "taskId": task_id,
                            "status": status_msg,
                            "sessionId": session_id,
                        }),
                    );

                    // 立即检查是否有新解锁的任务，加入同一 JoinSet
                    let new_ready = tm.get_ready_tasks();
                    for task in new_ready {
                        if cancel_token.is_cancelled() { break; }
                        let app_clone = app.clone();
                        let sid = session_id.to_string();
                        let prompt = if task.description.is_empty() {
                            task.subject.clone()
                        } else {
                            format!("{}\n\n{}", task.subject, task.description)
                        };
                        let tid = task.id;
                        let tsubject = task.subject.clone();
                        let _ = tm.update(tid, TaskUpdateParams {
                            status: Some(TaskStatus::InProgress),
                            subject: None, description: None, active_form: None,
                            owner: None, add_blocked_by: None, add_blocks: None, metadata: None,
                        });
                        let _ = app.emit("agent-step", serde_json::json!({
                            "type": "task_scheduled", "taskId": tid,
                            "subject": tsubject, "sessionId": session_id,
                        }));
                        set.spawn(async move {
                            let label = format!("Task #{}: {}", tid, tsubject);
                            println!("[SCHEDULER] 流式解锁: {}", label);
                            let result = tokio::time::timeout(
                                std::time::Duration::from_secs(300),
                                run_subagent(app_clone, prompt, false, sid,
                                    Some(tid), Some(label),
                                    Some(IMPLEMENTATION_AGENT_TYPE.to_string()), None),
                            ).await;
                            let (a, si, so) = match result {
                                Ok(r) => r,
                                Err(_) => (format!("任务超时（超过 5 分钟）"), 0, 0),
                            };
                            (tid, a, si, so)
                        });
                    }
                }
            }
            // 回到循环顶部检查是否还有残留未完成任务
        }

        // 生成汇总报告
        let summary = tm
            .summary()
            .unwrap_or_else(|e| format!("获取任务摘要失败: {}", e));
        let report = format!(
            "任务调度完成：{} 成功，{} 失败，共 {} 轮调度\n\n{}",
            completed_count, failed_count, round, summary
        );

        println!(
            "[SCHEDULER] 调度结束: {} 成功, {} 失败, {} 轮",
            completed_count, failed_count, round
        );

        (report, total_in, total_out)
    }
}
