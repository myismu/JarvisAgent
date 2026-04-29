//! # tools_runner.rs — 工具调用并行执行引擎
//!
//! 实现工具调用的三阶段流水线：预处理（串行解析参数）→ 并行执行（tokio::spawn）→ 排序汇总。
//! 支持参数自动修复、取消检查、`run_tasks` 调度器特殊处理等。
//!
//! ## 关键导出
//! - `execute_tool_calls()`: 执行所有工具调用，返回结果块、手动压缩标记和子 agent token 统计
//!
//! ## 依赖
//! - Internal: `crate::core::llm::adapters::parse_streamed_tool_input`, `crate::core::orchestration::agent_runs`, `crate::core::orchestration::scheduler::TaskScheduler`, `crate::core::tools`
//! - External: `serde_json`, `tauri`, `tokio`, `futures_util`
//!
//! ## 约束
//! - 工具结果按原始 index 排序，确保与 `tool_use_id` 一一对应
//! - `run_tasks` 工具走独立调度路径，不进入通用并行执行

use serde_json::json;
use tauri::Emitter;

use crate::core::llm::adapters::parse_streamed_tool_input;
use crate::core::models::*;
use crate::core::orchestration::agent_runs;
use crate::core::orchestration::scheduler::TaskScheduler;
use crate::core::tools::*;

/// 阶段1产出：待并行执行的工具任务数据
struct ToolTaskData {
    index: usize,
    tool_use_id: String,
    name: String,
    input: serde_json::Value,
}

/// 阶段2产出：并行执行完成后的结果
struct ToolTaskResult {
    index: usize,
    tool_use_id: String,
    name: String,
    output: String,
    input_tokens: u64,
    output_tokens: u64,
}

/// 执行所有工具调用（并行模式）
///
/// 三阶段流水线：
/// 1. 预处理（串行）：解析参数、emit "running" 事件、收集 ToolTaskData
/// 2. 并行执行：tokio::spawn 每个工具调用，join_all 收集结果
/// 3. 排序汇总：按原始 index 排序，emit "completed" 事件，组装返回
pub async fn execute_tool_calls(
    current_blocks: &mut Vec<ContentBlock>,
    tool_input_buffers: std::collections::HashMap<usize, String>,
    app: &tauri::AppHandle,
    sid: &str,
    run_id: &str,
    loop_count: usize,
    cancel_token: &tokio_util::sync::CancellationToken,
    intent: &str,
) -> (Vec<ContentBlock>, bool, u64, u64) {
    let mut manual_compact = false;

    // ========== 阶段 1：预处理（串行） ==========
    // 解析参数、emit "running" 事件、收集待执行任务
    let mut spawn_tasks: Vec<ToolTaskData> = Vec::new();
    let mut immediate_results: Vec<ToolTaskResult> = Vec::new();

    for (index, buf) in tool_input_buffers {
        if cancel_token.is_cancelled() {
            break;
        }
        if let Some(ContentBlock::ToolUse {
            name, input, id, ..
        }) = current_blocks.get_mut(index)
        {
            match parse_streamed_tool_input(&buf) {
                Ok((parsed_input, recovered)) => {
                    *input = parsed_input;
                    if name == "compact" {
                        manual_compact = true;
                    }
                    if recovered {
                        let _ = app.emit(
                            "chat-tool-debug",
                            json!({
                                "content": format!("\n> ↻ 参数已自动修复: `{}`\n", name),
                                "sessionId": sid,
                                "loopCount": loop_count
                            }),
                        );
                    }

                    let input_summary: String = {
                        let s = input.to_string();
                        if s.len() > 120 {
                            format!("{}...", s.chars().take(120).collect::<String>())
                        } else {
                            s
                        }
                    };

                    // emit "running" 状态事件
                    let _ = app.emit(
                        "chat-tool-debug",
                        json!({
                            "kind": "tool_status",
                            "status": "running",
                            "tool": name.clone(),
                            "toolCallId": id.clone(),
                            "sessionId": sid,
                            "loopCount": loop_count
                        }),
                    );
                    agent_runs::append_tool_log(
                        app,
                        run_id,
                        &format!("\n> ▸ 执行: `{}`\n", name),
                        loop_count,
                    );
                    let _ = app.emit(
                        "agent-step",
                        json!({
                            "type": "tool_call",
                            "tool": name,
                            "input_summary": input_summary,
                            "content": input.to_string(),
                            "sessionId": sid,
                            "loopCount": loop_count
                        }),
                    );
                    agent_runs::record_tool_call(
                        app,
                        run_id,
                        name,
                        Some(input_summary.clone()),
                        loop_count,
                    );

                    // run_tasks 工具特殊处理：直接调用 TaskScheduler（内部已有并行机制）
                    if name == "run_tasks" {
                        println!("[JARVIS] 检测到 run_tasks，启动任务调度器");
                        let (output, si, so) =
                            TaskScheduler::run_schedule(app, sid, run_id, cancel_token).await;
                        immediate_results.push(ToolTaskResult {
                            index,
                            tool_use_id: id.clone(),
                            name: name.clone(),
                            output,
                            input_tokens: si,
                            output_tokens: so,
                        });
                        continue;
                    }

                    // 收集到待执行列表
                    spawn_tasks.push(ToolTaskData {
                        index,
                        tool_use_id: id.clone(),
                        name: name.clone(),
                        input: input.clone(),
                    });
                }
                Err(err) => {
                    // 参数解析失败 → 直接进 immediate_results，不 spawn
                    let preview: String = buf.chars().take(500).collect();
                    let truncated = if buf.chars().count() > 500 {
                        format!("{}...(truncated)", preview)
                    } else {
                        preview
                    };
                    let failure = format!(
                        "工具 `{}` 参数解析失败：{}\n原始参数片段：{}",
                        name, err, truncated
                    );
                    println!("[JARVIS] {}", failure);
                    let _ = app.emit(
                        "chat-tool-debug",
                        json!({
                            "kind": "tool_status",
                            "status": "error",
                            "tool": name.clone(),
                            "toolCallId": id.clone(),
                            "content": format!("\n> ✕ 参数解析失败: `{}` - {}\n", name, err),
                            "sessionId": sid,
                            "loopCount": loop_count
                        }),
                    );
                    agent_runs::append_tool_log(
                        app,
                        run_id,
                        &format!("\n> ✕ 参数解析失败: `{}` - {}\n", name, err),
                        loop_count,
                    );
                    let _ = app.emit(
                        "agent-step",
                        json!({
                            "type": "tool_error",
                            "tool": name,
                            "error": format!("{}", err),
                            "content": failure,
                            "output_summary": format!("参数解析失败: {}", err),
                            "sessionId": sid,
                            "loopCount": loop_count
                        }),
                    );
                    agent_runs::record_tool_result(
                        app,
                        run_id,
                        name,
                        None,
                        Some(format!("{}", err)),
                        loop_count,
                    );
                    immediate_results.push(ToolTaskResult {
                        index,
                        tool_use_id: id.clone(),
                        name: name.clone(),
                        output: failure,
                        input_tokens: 0,
                        output_tokens: 0,
                    });
                }
            }
        }
    }

    // ========== 阶段 2：并行执行 ==========
    let mut all_results = immediate_results;

    if !spawn_tasks.is_empty() && !cancel_token.is_cancelled() {
        let handles: Vec<_> = spawn_tasks
            .into_iter()
            .map(|task| {
                let app_clone = app.clone();
                let sid_clone = sid.to_string();
                let intent_clone = intent.to_string();
                let cancel = cancel_token.clone();
                tokio::spawn(async move {
                    // spawn 后立即检查取消
                    if cancel.is_cancelled() {
                        return ToolTaskResult {
                            index: task.index,
                            tool_use_id: task.tool_use_id,
                            name: task.name,
                            output: "已取消".to_string(),
                            input_tokens: 0,
                            output_tokens: 0,
                        };
                    }
                    let (output, si, so) = handle_tool_call_owned(
                        app_clone,
                        task.name.clone(),
                        task.input.clone(),
                        sid_clone,
                        intent_clone,
                    )
                    .await;
                    ToolTaskResult {
                        index: task.index,
                        tool_use_id: task.tool_use_id,
                        name: task.name,
                        output,
                        input_tokens: si,
                        output_tokens: so,
                    }
                })
            })
            .collect();

        // join_all 收集所有结果
        let spawned_results = futures_util::future::join_all(handles).await;
        for result in spawned_results {
            if let Ok(r) = result {
                all_results.push(r);
            }
        }
    }

    // ========== 阶段 3：排序 + 汇总 ==========
    // 按原始 index 排序，保证 tool_results 顺序与 tool_use_id 对应
    all_results.sort_by_key(|r| r.index);

    let mut tool_results = Vec::new();
    let mut sub_in: u64 = 0;
    let mut sub_out: u64 = 0;

    for result in all_results {
        sub_in += result.input_tokens;
        sub_out += result.output_tokens;

        // emit "completed" 或 "error" 事件
        let status = if result.output.starts_with("工具 `")
            && result.output.contains("参数解析失败")
        {
            "error"
        } else {
            "completed"
        };
        let _ = app.emit(
            "chat-tool-debug",
            json!({
                "kind": "tool_status",
                "status": status,
                "tool": result.name.clone(),
                "toolCallId": result.tool_use_id.clone(),
                "sessionId": sid,
                "loopCount": loop_count
            }),
        );
        agent_runs::append_tool_log(
            app,
            run_id,
            &format!("> ◈ 完成: `{}`\n", result.name),
            loop_count,
        );

        let output_summary: String = {
            if result.output.len() > 150 {
                format!("{}...", result.output.chars().take(150).collect::<String>())
            } else {
                result.output.clone()
            }
        };
        let is_error = status == "error";
        let _ = app.emit(
            "agent-step",
            json!({
                "type": if is_error { "tool_error" } else { "tool_result" },
                "tool": result.name,
                "output_summary": output_summary,
                "content": result.output,
                "error": if is_error { Some(output_summary.clone()) } else { None },
                "sessionId": sid,
                "loopCount": loop_count
            }),
        );
        agent_runs::record_tool_result(
            app,
            run_id,
            &result.name,
            Some(output_summary),
            None,
            loop_count,
        );

        tool_results.push(ContentBlock::ToolResult {
            tool_use_id: result.tool_use_id,
            content: result.output,
        });
    }

    (tool_results, manual_compact, sub_in, sub_out)
}
