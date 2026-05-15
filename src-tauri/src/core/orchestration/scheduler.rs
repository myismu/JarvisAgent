//! 任务调度器模块 - 基于依赖图的流式任务调度
//!
//! 调度算法：
//! 1. 查找所有就绪任务（无依赖或依赖已满足）
//! 2. 并行 spawn 到 JoinSet
//! 3. 任一任务完成 → 立即标记 Completed → 检查级联解锁的新任务 → 加入同一 JoinSet
//! 4. JoinSet 自然耗尽 = 全部完成

use tauri::Emitter;
use tokio::sync::mpsc;

use crate::infra::types::models::TaskStatus;
use crate::core::orchestration::tasks::{TaskManager, TaskUpdateParams};
use crate::core::tools::{run_subagent, IMPLEMENTATION_AGENT_ROLE};

/// 调度器 → 主 Agent 的实时事件
#[derive(Debug, Clone)]
pub enum SchedulerEvent {
    /// 子任务完成（含 token 统计）
    TaskCompleted { task_id: i32, subject: String, tokens: (u64, u64) },
    /// 子任务失败/超时/取消
    TaskFailed { task_id: i32, subject: String, reason: String, error_detail: String },
    /// 全部任务结束（含汇总报告）
    AllDone { completed: usize, failed: usize, report: String },
}

pub struct TaskScheduler;

impl TaskScheduler {
    /// 流式调度：初始就绪任务 → JoinSet 并行 → 完成即级联解锁 → JoinSet 耗尽即结束
    pub async fn run_schedule(
        app: &tauri::AppHandle,
        session_id: &str,
        _run_id: &str,
        cancel_token: &tokio_util::sync::CancellationToken,
    ) -> (String, u64, u64) {
        let tm = TaskManager::for_session(session_id);
        let mut _total_in: u64 = 0;
        let mut total_out: u64 = 0;
        let mut completed_count: usize = 0;
        let mut failed_count: usize = 0;
        let mut task_subjects: std::collections::HashMap<i32, String> =
            std::collections::HashMap::new();
        // 已完成任务的结果摘要：task_id → (subject, result_summary)
        let mut completed_results: std::collections::HashMap<i32, (String, String)> =
            std::collections::HashMap::new();

        if cancel_token.is_cancelled() {
            return ("调度已取消".to_string(), 0, 0);
        }

        // 1. 初始就绪任务
        let ready_tasks = tm.get_ready_tasks();
        if ready_tasks.is_empty() {
            let remaining = tm.count_incomplete();
            if remaining == 0 {
                return ("无待执行任务".to_string(), 0, 0);
            }
            let msg = format!(
                "[SCHEDULER] 检测到循环依赖，{} 个任务无法调度",
                remaining
            );
            println!("{}", msg);
            return (msg, _total_in, total_out);
        }

        println!(
            "[SCHEDULER] 流式调度启动：{} 个初始就绪任务",
            ready_tasks.len()
        );

        // 初始任务列表播报
        {
            let mut lines: Vec<String> = Vec::new();
            for task in &ready_tasks {
                task_subjects.insert(task.id, task.subject.clone());
                lines.push(format!("  • Task #{}: {}", task.id, task.subject));
            }
            let _ = app.emit(
                "chat-stream",
                serde_json::json!({
                    "content": format!(
                        "\n> [调度启动] {} 个就绪任务\n{}\n",
                        ready_tasks.len(),
                        lines.join("\n")
                    ),
                    "sessionId": session_id,
                }),
            );
        }

        // 标记初始任务 InProgress
        for task in &ready_tasks {
            let _ = tm.update(task.id, TaskUpdateParams {
                status: Some(TaskStatus::InProgress),
                subject: None, description: None, active_form: None,
                owner: None, add_blocked_by: None, add_blocks: None, metadata: None,
            });
            let _ = app.emit("agent-step", serde_json::json!({
                "type": "task_scheduled", "taskId": task.id,
                "subject": task.subject, "sessionId": session_id,
            }));
        }

        // 2. 全部 spawn 到 JoinSet
        let mut set = tokio::task::JoinSet::new();
        for task in ready_tasks {
            spawn_into_set(&mut set, app, session_id, &task, &mut task_subjects, &completed_results);
        }

        // 3. 流式级联：完成一个 → 标记 → 立即查新解锁 → spawn → 继续
        while let Some(result) = set.join_next().await {
            if cancel_token.is_cancelled() {
                println!("[SCHEDULER] 调度被取消");
                break;
            }

            let (task_id, answer, si, so) = match result {
                Ok(r) => r,
                Err(e) => {
                    println!("[SCHEDULER] 子 Agent panic/cancel: {}", e);
                    failed_count += 1;
                    continue;
                }
            };
            {
                _total_in += si;
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

                let _ = tm.update(task_id, TaskUpdateParams {
                    status: Some(TaskStatus::Completed),
                    subject: None, description: None, active_form: None,
                    owner: None, add_blocked_by: None, add_blocks: None, metadata: None,
                });

                println!(
                    "[SCHEDULER] Task #{} {} (input: {}, output: {} tokens)",
                    task_id, status_msg, si, so
                );

                let _ = app.emit("agent-step", serde_json::json!({
                    "type": "task_completed", "taskId": task_id,
                    "status": status_msg, "sessionId": session_id,
                }));

                // 保存已完成任务的结果摘要（供下游任务引用）
                {
                    let subject = task_subjects.get(&task_id).cloned()
                        .unwrap_or_else(|| format!("Task #{}", task_id));
                    if status_msg == "完成" {
                        let summary: String = answer.chars().take(500).collect();
                        completed_results.insert(task_id, (subject.clone(), summary));
                    }
                    let icon = if status_msg == "完成" { "[OK]" } else { "[FAIL]" };
                    let _ = app.emit("chat-stream", serde_json::json!({
                        "content": format!(
                            "\n> {} Task #{}: {} ({}, {} tokens)\n",
                            icon, task_id, subject, status_msg, si + so
                        ),
                        "sessionId": session_id,
                    }));
                }

                // 级联解锁：立即查找新就绪任务，加入同一个 JoinSet
                let new_ready = tm.get_ready_tasks();
                for task in &new_ready {
                    if cancel_token.is_cancelled() { break; }
                    task_subjects.insert(task.id, task.subject.clone());
                    let _ = tm.update(task.id, TaskUpdateParams {
                        status: Some(TaskStatus::InProgress),
                        subject: None, description: None, active_form: None,
                        owner: None, add_blocked_by: None, add_blocks: None, metadata: None,
                    });
                    let _ = app.emit("agent-step", serde_json::json!({
                        "type": "task_scheduled", "taskId": task.id,
                        "subject": task.subject, "sessionId": session_id,
                    }));
                    spawn_into_set(&mut set, app, session_id, task, &mut task_subjects, &completed_results);
                }
            }
        }

        // 4. 最终检查
        let remaining = tm.count_incomplete();
        if remaining > 0 {
            let msg = format!(
                "[SCHEDULER] 调度结束，{} 个任务无法完成（循环依赖或前置失败）",
                remaining
            );
            println!("{}", msg);
            let summary = tm.summary().unwrap_or_default();
            return (format!("{}\n\n{}", msg, summary), _total_in, total_out);
        }

        let summary = tm.summary().unwrap_or_else(|e| format!("获取任务摘要失败: {}", e));
        let report = format!(
            "任务调度完成：{} 成功，{} 失败\n\n{}",
            completed_count, failed_count, summary
        );

        println!(
            "[SCHEDULER] 调度结束: {} 成功, {} 失败",
            completed_count, failed_count
        );

        (report, _total_in, total_out)
    }

    /// 异步调度：将调度逻辑 spawn 为后台 task，通过 channel 实时推送事件给主 Agent
    pub fn run_schedule_async(
        app: tauri::AppHandle,
        session_id: String,
        cancel_token: tokio_util::sync::CancellationToken,
        event_tx: mpsc::UnboundedSender<SchedulerEvent>,
    ) {
        tokio::spawn(async move {
            let tm = TaskManager::for_session(&session_id);
            let _ = 0u64; // total_in placeholder (async模式下token统计暂不追踪)
            let mut completed_count: usize = 0;
            let mut failed_count: usize = 0;
            let mut task_subjects: std::collections::HashMap<i32, String> =
                std::collections::HashMap::new();
            if cancel_token.is_cancelled() {
                let _ = event_tx.send(SchedulerEvent::AllDone {
                    completed: 0, failed: 0,
                    report: "调度已取消".to_string(),
                });
                return;
            }

            let ready_tasks = tm.get_ready_tasks();
            if ready_tasks.is_empty() {
                let remaining = tm.count_incomplete();
                if remaining == 0 {
                    let _ = event_tx.send(SchedulerEvent::AllDone {
                        completed: 0, failed: 0,
                        report: "无待执行任务".to_string(),
                    });
                    return;
                }
                let _ = event_tx.send(SchedulerEvent::AllDone {
                    completed: 0, failed: remaining,
                    report: format!("{} 个任务因循环依赖无法调度", remaining),
                });
                return;
            }

            for task in &ready_tasks {
                task_subjects.insert(task.id, task.subject.clone());
                let _ = tm.update(task.id, TaskUpdateParams {
                    status: Some(TaskStatus::InProgress),
                    subject: None, description: None, active_form: None,
                    owner: None, add_blocked_by: None, add_blocks: None, metadata: None,
                });
            }

            // spawn 所有就绪任务到 JoinSet
            let mut set = tokio::task::JoinSet::new();
            for task in ready_tasks {
                let tid = task.id;
                let prompt = if task.description.is_empty() {
                    task.subject.clone()
                } else {
                    format!("{}\n\n{}", task.subject, task.description)
                };
                let app_c = app.clone();
                let sid = session_id.clone();
                set.spawn(async move {
                    let fut = run_subagent(
                        app_c, prompt, false, sid,
                        Some(tid), None,
                        Some(IMPLEMENTATION_AGENT_ROLE.to_string()), None,
                    );
                    let result = tokio::time::timeout(
                        std::time::Duration::from_secs(300),
                        fut,
                    ).await;
                    match result {
                        Ok(r) => (tid, r.0, r.1, r.2),
                        Err(_) => (tid, "任务超时（超过 5 分钟）".to_string(), 0, 0),
                    }
                });
            }

            // 流式级联
            while let Some(result) = set.join_next().await {
                if cancel_token.is_cancelled() { break; }

                let (task_id, answer, si, so) = match result {
                    Ok(r) => r,
                    Err(e) => {
                        println!("[SCHEDULER] 子 Agent panic/cancel: {}", e);
                        let _ = event_tx.send(SchedulerEvent::TaskFailed {
                            task_id: 0,
                            subject: "子 Agent 进程崩溃".to_string(),
                            reason: "panic".to_string(),
                            error_detail: format!("{}", e),
                        });
                        continue;
                    }
                };

                let subject = task_subjects.get(&task_id).cloned()
                        .unwrap_or_else(|| format!("Task #{}", task_id));

                    let (is_fail, reason) = if answer.contains("任务超时") {
                        (true, "超时")
                    } else if answer.contains("子代理已取消") {
                        (true, "取消")
                    } else if answer.contains("子代理执行达到") {
                        (true, "达到循环上限")
                    } else if answer.contains("失败") || answer.contains("error") {
                        (true, "执行错误")
                    } else {
                        (false, "")
                    };

                    let status = if is_fail { TaskStatus::Pending } else { TaskStatus::Completed };
                    let _ = tm.update(task_id, TaskUpdateParams {
                        status: Some(status),
                        subject: None, description: None, active_form: None,
                        owner: None, add_blocked_by: None, add_blocks: None, metadata: None,
                    });

                    if is_fail {
                        failed_count += 1;
                        let error_detail: String = answer.chars().take(500).collect();
                        let _ = event_tx.send(SchedulerEvent::TaskFailed {
                            task_id, subject: subject.clone(),
                            reason: reason.to_string(),
                            error_detail,
                        });
                    } else {
                        completed_count += 1;
                        let _summary: String = answer.chars().take(500).collect();
                        let _ = event_tx.send(SchedulerEvent::TaskCompleted {
                            task_id, subject: subject.clone(),
                            tokens: (si, so),
                        });
                    }

                    // 只有成功完成才级联解锁新任务
                    if !is_fail {
                        let new_ready = tm.get_ready_tasks();
                        for task in &new_ready {
                            if cancel_token.is_cancelled() { break; }
                            task_subjects.insert(task.id, task.subject.clone());
                            let _ = tm.update(task.id, TaskUpdateParams {
                                status: Some(TaskStatus::InProgress),
                                subject: None, description: None, active_form: None,
                                owner: None, add_blocked_by: None, add_blocks: None, metadata: None,
                            });
                            let tid = task.id;
                            let prompt = if task.description.is_empty() {
                                task.subject.clone()
                            } else {
                                format!("{}\n\n{}", task.subject, task.description)
                            };
                            let app_c = app.clone();
                            let sid = session_id.clone();
                            set.spawn(async move {
                                let fut = run_subagent(
                                    app_c, prompt, false, sid,
                                    Some(tid), None,
                                    Some(IMPLEMENTATION_AGENT_ROLE.to_string()), None,
                                );
                                let result = tokio::time::timeout(
                                    std::time::Duration::from_secs(300),
                                    fut,
                                ).await;
                                match result {
                                    Ok(r) => (tid, r.0, r.1, r.2),
                                    Err(_) => (tid, "任务超时（超过 5 分钟）".to_string(), 0, 0),
                                }
                            });
                        }
                    }
                }

            let remaining = tm.count_incomplete();
            let report = if remaining > 0 {
                format!("调度结束：{} 成功，{} 失败，{} 个未完成", completed_count, failed_count, remaining)
            } else {
                format!("调度结束：{} 成功，{} 失败", completed_count, failed_count)
            };

            let _ = event_tx.send(SchedulerEvent::AllDone {
                completed: completed_count,
                failed: failed_count,
                report,
            });
        });
    }
}

fn spawn_into_set(
    set: &mut tokio::task::JoinSet<(i32, String, u64, u64)>,
    app: &tauri::AppHandle,
    session_id: &str,
    task: &crate::infra::types::models::Task,
    _task_subjects: &std::collections::HashMap<i32, String>,
    completed_results: &std::collections::HashMap<i32, (String, String)>,
) {
    let app_clone = app.clone();
    let sid = session_id.to_string();

    // 构建基础 prompt
    let base_prompt = if task.description.is_empty() {
        task.subject.clone()
    } else {
        format!("{}\n\n{}", task.subject, task.description)
    };

    // 注入上游已完成任务的结果
    let prompt = if !task.blocked_by.is_empty() {
        let mut ctx = String::from("【前置任务已完成，以下是它们的执行结果，请基于这些结果继续工作】\n");
        for upstream_id in &task.blocked_by {
            if let Some((subject, summary)) = completed_results.get(upstream_id) {
                let short: String = summary.chars().take(300).collect();
                ctx.push_str(&format!(
                    "\n- Task #{}: {} — {}",
                    upstream_id, subject, short
                ));
            }
        }
        ctx.push_str(&format!("\n\n【你的任务】\n{}", base_prompt));
        ctx
    } else {
        base_prompt
    };
    let tid = task.id;
    let tsubject = task.subject.clone();
    set.spawn(async move {
        println!("[SCHEDULER] 启动子Agent: Task #{} - {}", tid, tsubject);
        let label = format!("Task #{}: {}", tid, tsubject);
        let result = tokio::time::timeout(
            std::time::Duration::from_secs(300),
            run_subagent(
                app_clone, prompt, false, sid,
                Some(tid), Some(label),
                Some(IMPLEMENTATION_AGENT_ROLE.to_string()), None,
            ),
        ).await;
        let (answer, si, so) = match result {
            Ok(r) => r,
            Err(_) => (format!("任务超时（超过 5 分钟）"), 0, 0),
        };
        (tid, answer, si, so)
    });
}
