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

            // 3. 并行 spawn 所有就绪任务的子Agent
            let handles: Vec<_> = ready_tasks
                .into_iter()
                .map(|task| {
                    let app_clone = app.clone();
                    let sid = session_id.to_string();
                    let prompt = if task.description.is_empty() {
                        task.subject.clone()
                    } else {
                        format!("{}\n\n{}", task.subject, task.description)
                    };
                    let task_id = task.id;
                    let task_subject = task.subject.clone();
                    tokio::spawn(async move {
                        println!(
                            "[SCHEDULER] 启动子Agent: Task #{} - {}",
                            task_id, task_subject
                        );
                        let (answer, si, so) = run_subagent(
                            app_clone,
                            prompt,
                            false, // 非只读，允许写操作
                            sid,
                            Some(task_id),
                            Some(format!("Task #{}", task_id)),
                            Some(IMPLEMENTATION_AGENT_TYPE.to_string()),
                            None,
                        )
                        .await;
                        (task_id, answer, si, so)
                    })
                })
                .collect();

            // 4. join_all 等待本批次全部完成
            let results = futures_util::future::join_all(handles).await;

            // 5. 处理结果，更新任务状态
            for result in results {
                if let Ok((task_id, answer, si, so)) = result {
                    total_in += si;
                    total_out += so;

                    // 无论成功失败都标记为 Completed（避免阻塞后续任务）
                    let status_msg = if answer.contains("子代理已取消") {
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
                    // _clear_dependency 在 update(Completed) 中自动调用，级联解锁下游任务

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
                }
            }
            // 回到循环顶部，自动发现新解锁的任务
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
