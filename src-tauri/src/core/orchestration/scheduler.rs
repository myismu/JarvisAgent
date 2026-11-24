//! 任务调度器模块 - 基于依赖图的流式任务调度
//!
//! 调度算法：
//! 1. 查找所有就绪任务（无依赖或依赖已满足）
//! 2. 并行 spawn 到 JoinSet
//! 3. 任一任务完成 → 立即标记 Completed → 检查级联解锁的新任务 → 加入同一 JoinSet
//! 4. JoinSet 自然耗尽 = 全部完成

use tauri::Emitter;

use crate::core::orchestration::tasks::{TaskManager, TaskUpdateParams};
use crate::core::tools::{run_subagent, IMPLEMENTATION_AGENT_ROLE};
use crate::infra::types::constants::SUBAGENT_HARD_TIMEOUT_SECS;
use crate::infra::types::models::{TaskStatus, TodoStatus};

/// 子代理超时结果的**结构化前缀标记**。
///
/// 由 `subagent.rs` 内部的预算判据（无进展 / 硬上限）产出，调度器只负责识别它，
/// 不做任何超时判断，也不拼装超时文案——文案属于判定层，判定层才知道
/// 「是静默超时还是总时长超限、静默了多久」。
const SUBAGENT_TIMEOUT_MARKER: &str = "【子代理超时】";

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
            let msg = format!("[SCHEDULER] 检测到循环依赖，{} 个任务无法调度", remaining);
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
                    subagent_type: None,
                },
            );
            let _ = app.emit(
                "agent-step",
                serde_json::json!({
                    "type": "task_scheduled", "taskId": task.id,
                    "subject": task.subject, "sessionId": session_id,
                }),
            );
            // 同步用户可见的 TODO 清单（任务图与 TODO 是同一批任务的两个视图）
            crate::core::orchestration::todo_sync::update_task_status(
                app,
                session_id,
                task.id,
                TodoStatus::InProgress,
            )
            .await;
        }

        // 2. 全部 spawn 到 JoinSet
        let mut set = tokio::task::JoinSet::new();
        for task in ready_tasks {
            spawn_into_set(
                &mut set,
                app,
                session_id,
                &task,
                &mut task_subjects,
                &completed_results,
            );
        }

        // 3. 流式级联：完成一个 → 标记 → 立即查新解锁 → spawn → 继续
        //
        // 2026-11 改造：用 select 同时等待"下一个任务完成"和"用户取消"。
        // 旧实现要等 join_next() 返回后才检查取消——子任务正卡在长命令里时，
        // 用户点停止后调度器要等下一个任务结束才醒来。
        let mut cancelled = false;
        loop {
            let joined = tokio::select! {
                biased;
                _ = cancel_token.cancelled() => {
                    cancelled = true;
                    // 中止仍在等待的子代理任务。子代理自己的取消令牌已由
                    // cancel_jarvis 级联取消，监控面板会各自收口；
                    // 这里不再等待它们逐个返回。
                    set.abort_all();
                    break;
                }
                joined = set.join_next() => joined,
            };

            let Some(result) = joined else { break };

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

                // 前缀匹配而非 contains：这是子代理内部产出的结构化标记，
                // 约定出现在回答开头；用前缀可避免正文里恰好提到该词时被误判。
                let status_msg = if answer.starts_with(SUBAGENT_TIMEOUT_MARKER) {
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
                        subagent_type: None,
                    },
                );

                println!(
                    "[SCHEDULER] Task #{} {} (input: {}, output: {} tokens)",
                    task_id, status_msg, si, so
                );

                let _ = app.emit(
                    "agent-step",
                    serde_json::json!({
                        "type": "task_completed", "taskId": task_id,
                        "status": status_msg, "sessionId": session_id,
                    }),
                );
                // 同步 TODO 清单。注意 TaskStatus 没有 Failed，失败任务也会落到 completed；
                // 失败详情以调度报告/时间线为准。
                crate::core::orchestration::todo_sync::update_task_status(
                    app,
                    session_id,
                    task_id,
                    TodoStatus::Completed,
                )
                .await;

                // 保存已完成任务的结果摘要（供下游任务引用）
                {
                    let subject = task_subjects
                        .get(&task_id)
                        .cloned()
                        .unwrap_or_else(|| format!("Task #{}", task_id));
                    if status_msg == "完成" {
                        let summary: String = answer.chars().take(500).collect();
                        completed_results.insert(task_id, (subject.clone(), summary));
                    }
                    let icon = if status_msg == "完成" {
                        "[OK]"
                    } else {
                        "[FAIL]"
                    };
                    let _ = app.emit(
                        "chat-stream",
                        serde_json::json!({
                            "content": format!(
                                "\n> {} Task #{}: {} ({}, {} tokens)\n",
                                icon, task_id, subject, status_msg, si + so
                            ),
                            "sessionId": session_id,
                        }),
                    );
                }

                // 级联解锁：立即查找新就绪任务，加入同一个 JoinSet
                let new_ready = tm.get_ready_tasks();
                for task in &new_ready {
                    if cancel_token.is_cancelled() {
                        break;
                    }
                    task_subjects.insert(task.id, task.subject.clone());
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
                            subagent_type: None,
                        },
                    );
                    let _ = app.emit(
                        "agent-step",
                        serde_json::json!({
                            "type": "task_scheduled", "taskId": task.id,
                            "subject": task.subject, "sessionId": session_id,
                        }),
                    );
                    crate::core::orchestration::todo_sync::update_task_status(
                        app,
                        session_id,
                        task.id,
                        TodoStatus::InProgress,
                    )
                    .await;
                    spawn_into_set(
                        &mut set,
                        app,
                        session_id,
                        task,
                        &mut task_subjects,
                        &completed_results,
                    );
                }
            }
        }

        // 4. 最终检查
        if cancelled {
            let remaining = tm.count_incomplete();
            let summary = tm.summary().unwrap_or_default();
            println!("[SCHEDULER] 调度被取消，{} 个任务未完成", remaining);
            return (
                format!("调度已取消：{} 个任务未能完成\n\n{}", remaining, summary),
                _total_in,
                total_out,
            );
        }
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

        let summary = tm
            .summary()
            .unwrap_or_else(|e| format!("获取任务摘要失败: {}", e));
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
        let mut ctx =
            String::from("【前置任务已完成，以下是它们的执行结果，请基于这些结果继续工作】\n");
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
    // 子代理类型取自任务本身（缺省回落到 implementation，保持历史行为）
    let agent_role = task
        .subagent_type
        .clone()
        .filter(|role| !role.trim().is_empty())
        .unwrap_or_else(|| IMPLEMENTATION_AGENT_ROLE.to_string());
    set.spawn(async move {
        println!("[SCHEDULER] 启动子Agent: Task #{} - {}", tid, tsubject);
        let label = format!("Task #{}: {}", tid, tsubject);

        // 外层兜底超时：**只防未预期挂死，不承担判定职责**
        //
        // 超时值 = 子代理内部硬上限 + 60 秒余量。超时的判定权已经交给 subagent.rs
        // 内部的预算判据：`SUBAGENT_IDLE_TIMEOUT_SECS`（无进展 180s，只管等模型的静默）、
        // `SUBAGENT_HARD_TIMEOUT_SECS`（硬上限 3600s，防真死循环），且工具执行阶段
        // 不参与 idle 判定。内部判据命中时会带着真实用量返回，并在回答开头打上
        // `【子代理超时】` 前缀。
        //
        // 因此本层在正常情况下永远不会触发，它兜的是一种极端情况：run_subagent
        // 既不返回、内部判据也没能收口（例如卡在某个不走预算判据的 await 上）。
        // 本层**不产出任何面向用户的文案**——判定与文案全部由 subagent.rs 内部负责，
        // 这里命中只意味着"这个 future 挂了"，仅此而已。
        // +60 秒是留给内部判据收尾（失败落库、监控面板收口）的余量，不是第二个判据。
        let fallback_secs = SUBAGENT_HARD_TIMEOUT_SECS + 60;
        let result = tokio::time::timeout(
            std::time::Duration::from_secs(fallback_secs),
            run_subagent(
                app_clone,
                prompt,
                false,
                sid,
                Some(tid),
                Some(label),
                Some(agent_role),
                None,
                None,
            ),
        )
        .await;

        match result {
            // 子代理自己收口：回答可能是正常结果，也可能是它内部判定的超时标记。
            // 无论哪种，tokens 一律原样透传——它们是子代理真实累计的用量，
            // 调度器不得改写，更不得为了方便展示而置 0。
            Ok((answer, si, so)) => (tid, answer, si, so),
            // 兜底命中：future 被丢弃，子代理一个轮次都没能交回来，
            // 因此**没有**任何真实用量可透传（这里的 0 表示"未知"，不是"用了 0"）。
            // 不再构造"超过 N 分钟"这类面向用户的超时文案，只回一个结构化失败标记，
            // 避免这次挂死被下游当成"完成"计入成功数。
            Err(_) => {
                println!(
                    "[SCHEDULER] Task #{} 命中外层兜底超时（{} 秒）：子代理未返回，内部判据未收口",
                    tid, fallback_secs
                );
                (
                    tid,
                    format!(
                        "{}调度器外层兜底命中：子代理未在 {} 秒内返回（未预期挂死）",
                        SUBAGENT_TIMEOUT_MARKER, fallback_secs
                    ),
                    0,
                    0,
                )
            }
        }
    });
}
