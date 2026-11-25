//! # pipeline/main_loop.rs — 管线阶段 3（Agent 主循环）

use super::*;

impl PipelineState {
    /// 阶段 3：主循环 — Agent Loop 心脏（调 LLM → 流式解析 → 工具执行 → 循环）
    ///
    /// 每轮循环的执行顺序：
    /// 1. 取消检查 / 循环次数确认（满 30 轮弹窗询问）/ 上下文压缩检查
    /// 2. 准备历史快照（过滤内部消息、修复残缺工具配对、恢复图片、注入动态上下文）
    /// 3. build_llm_request 按模型格式构建请求（OpenAI 出口时翻译协议）
    /// 4. 调 API：直接等待（120s 超时 + 重试）
    /// 5. process_stream 流式解析：边收边推前端，累积工具参数分片
    /// 6. execute_tool_calls 并行执行工具，结果以 ToolResult 写回历史
    /// 7. 判断是否继续：有工具结果 → 下一轮；无工具结果 = 最终答案，结束循环
    ///
    /// 循环终止条件：无工具结果 / 工具请求 break_loop（如 ProposePlan 等待审批）/
    /// 用户取消 / 达到 200 轮绝对上限 / API 连续失败
    pub(super) async fn run_main_loop(&mut self) -> Result<(), AgentError> {
        loop {
            println!(
                "[JARVIS] 主循环开始: loop_count={}, total_loop_count={}",
                self.loop_count, self.total_loop_count
            );

            // ════ 主循环每轮开始 ════
            // 步骤 1：取消检查 —— 用户点停止则退出循环
            // 取消检查
            if self.cancel_token.is_cancelled() {
                println!("[JARVIS] 主循环: cancel_token 已取消，退出循环");
                self.handle_cancellation().await;
                break;
            }

            // 循环次数确认
            if self.loop_count >= crate::infra::types::constants::MAX_AGENT_LOOP_BEFORE_CONFIRM {
                let decision = self.request_loop_continuation().await;
                if !decision {
                    // 用户拒绝续跑 / 确认未完成：给一个明确的收尾说明，避免留下空气泡。
                    // 该说明必须落库——旧实现只写在内存 final_answer 里，
                    // 刷新界面后用户看不到任何"为什么停了"的交代。
                    //
                    // 注意这里**不再把文案写进 final_answer**：那是模型正文的位置，
                    // 会以"模型说的话"呈现，且文案是后端硬编码的中文。
                    // 改走 notice（i18n key），与刷新后历史路径按 kind 生成的小字**同一条** ——
                    // 旧实现两处措辞不同（此处"已停止执行…"vs 历史侧"已达回合上限…"），
                    // 刷新前后会变成两种说法。
                    self.notice_i18n_key = Some("notice.loopLimit");
                    // 写入历史的标记面向 LLM（会进上下文），不含"告诉我"这类
                    // 对用户说的措辞——模型会把它们当成自己的话。
                    self.append_interrupted_marker(InterruptKind::LoopLimit)
                        .await;
                    self.interrupted_reason = Some("达到回合上限且未获续跑授权".to_string());
                    break;
                }
            }

            // 后台任务结果不再注入会话上下文：失败/完成信息走前端界面提醒
            // （bg-task-done / background-failed 事件 → 聊天流小字），用户看到后
            // 主动发消息让 Agent 排查（排查用 CheckBackgroundCommand）。
            // 这样也消除了 background 注入对（User+Assistant 两条）破坏
            // 消息级 user/assistant 交替的问题。

            // 步骤 3：上下文压缩检查（超可用窗口 COMPACT_TRIGGER_PERCENT（85%）时自动摘要旧历史）
            // Token 压缩
            self.compact_if_needed().await;

            // 后续轮次：沿用本轮固定的 thinking 状态（不再回落 audience 默认值）。
            // 中途从 disabled 翻成 enabled 会让历史里那条没有 thinking 的 assistant
            // 无法满足 Anthropic 的"thinking 必须回传"要求 → 400。
            if self.loop_count > 0 {
                self.should_think = should_think_for_loop(self.turn_think, self.loop_count);
            }

            // 步骤 4：准备发给 LLM 的历史快照（过滤内部消息、修复残缺配对等）
            // 历史快照准备
            let history_snapshot = self.prepare_history_snapshot().await;

            // 步骤 5：构建 LLM 请求（OpenAI 出口时按模型翻译协议）+ 更新上下文快照
            // 构建请求并更新上下文快照
            let (req_json, req_api_format) = self.build_llm_request(history_snapshot);

            // 调试日志：logger 内部按 request_base + messages 增量落盘，
            // 这里只打印一个体量（compact 序列化，不再为打印付出 pretty 的开销）
            println!(
                "[MAIN AGENT] loop {} request ({} bytes)",
                self.total_loop_count + 1,
                serde_json::to_string(&req_json)
                    .map(|s| s.len())
                    .unwrap_or(0)
            );
            debug_logger::debug_logger().log_request(
                &self.sid,
                "MAIN",
                self.total_loop_count + 1,
                &req_json,
            );

            if self.cancel_token.is_cancelled() {
                continue;
            }

            // 步骤 6：调用 LLM API
            // 等待提示看门狗：从请求发出前一直存活到本轮流读取结束。
            // 由 SSE 帧到达续期，因此只在**真的没有数据**时才提示
            // （上游深度思考、网络迟滞、收到响应头后正文静默）。
            let progress_watchdog = self.spawn_waiting_hint();

            // 看门狗已在上面启动，这里只负责发请求并把它传给流阶段。
            let api_outcome = self.call_api_with_retry(&req_json).await;
            // 不在此 abort：看门狗要跨到流读取阶段才有效。
            // 响应头很快返回，真正的静默几乎都发生在正文阶段。
            let response = match api_outcome {
                Ok(Some(r)) => {
                    progress_watchdog.touch(&self.app, &self.sid, self.total_loop_count + 1);
                    Some(r)
                }
                // 提前退出（取消）：统一交给下面的 `let Some(...) else` 分支停看门狗并 continue
                Ok(None) => None,
                Err(e) => {
                    progress_watchdog.finish(&self.app, &self.sid, self.total_loop_count + 1);
                    // API 调用失败 → 记录诊断日志后返回 Err
                    println!("[JARVIS] API 调用失败，终止主循环: {}", e);
                    let messages_json = {
                        let session = self.ctx.memory.lock().await;
                        serde_json::to_string_pretty(&session.messages).unwrap_or_default()
                    };
                    crate::infra::debug_logger::debug_logger().log_api_error(
                        &self.sid,
                        "MAIN",
                        self.total_loop_count + 1,
                        &e.to_string(),
                        &messages_json,
                    );
                    return Err(e);
                }
            };

            // 流读取期间到达的帧会通过 on_frame 续期，真正空闲才提示。
            // 本轮结束（无论哪条路径）都必须 finish，否则会残留任务。
            //
            // ⚠️ 2026-11-22 修：这里原先只 store 计时、**不复位 `hint_sent`**，
            // 于是一轮流里第一段静默发过提示后，后面再静默永远不会再提示
            // （与 `touch()` 的注释口径不一致，属漏改）。现统一走 `touch_and_clear`：
            // 既复位标记，又在确实提示过时通知前端清除。
            let frame_tick = Some({
                let tick = progress_watchdog.last_tick_ms.clone();
                let sent = progress_watchdog.hint_sent.clone();
                let tick_app = self.app.clone();
                let tick_sid = self.sid.clone();
                let tick_loop = self.total_loop_count + 1;
                std::sync::Arc::new(move || {
                    touch_and_clear(&tick, &sent, &tick_app, &tick_sid, tick_loop);
                }) as std::sync::Arc<dyn Fn() + Send + Sync>
            });

            let Some(response) = response else {
                progress_watchdog.finish(&self.app, &self.sid, self.total_loop_count + 1);
                continue;
            };
            // 步骤 7：SSE 流式解析 —— 边收边推前端、累积工具参数分片
            // 流式处理（含一次断流重试）
            let stream_result = {
                // 注册表可选的缓存字段写法覆盖（一般不用；非标准命名时才在 model_registry.json 里写）
                let cache_usage_style =
                    crate::infra::llm::registry::cache_usage_style_for(&self.model_id);
                let mut stream = response.bytes_stream().eventsource();
                let mut result = process_stream(
                    &mut stream,
                    req_api_format,
                    &self.app,
                    &self.sid,
                    &self.run_id,
                    self.total_loop_count + 1,
                    &self.cancel_token,
                    StreamConfig {
                        is_subagent: false,
                        cache_usage_style: cache_usage_style.clone(),
                        on_frame: frame_tick.clone(),
                        model_id: Some(self.model_id.clone()),
                        crash_protection: self.crash_protection,
                    },
                )
                .await;

                // 重试条件：只由 stream 层给出的 `should_retry` 决定（零产出才为 true）。
                //
                // 为什么不在这里用 `idle_timed_out` 判断：该标记表示"空闲计时器触发了"，
                // 包含"吐了半截后静默"这种**不该重试**的情况。二者必须分开，
                // 否则会出现"既没重试、又没结束本轮"的卡死（见 StreamResult 注释）。
                if result.should_retry && !self.cancel_token.is_cancelled() {
                    if result.idle_timed_out {
                        println!("[JARVIS] 上游静默超时且零产出，尝试重试一次...");
                    } else {
                        println!("[JARVIS] 流式响应零产出即终止，尝试重试一次...");
                    }
                    // 重试给一个更短的、**覆盖全流程**的预算（请求头 + 读流）。
                    // 首次已等满一个空闲周期，若重试再等满，用户感知的等待会翻倍。
                    let retry_budget = Duration::from_secs(
                        crate::infra::types::constants::API_STREAM_RETRY_BUDGET_SECS,
                    );
                    let retry_fut = async {
                        if let Ok(Some(resp)) = self.call_api_with_retry(&req_json).await {
                            let mut stream2 = resp.bytes_stream().eventsource();
                            return Some(
                                process_stream(
                                    &mut stream2,
                                    req_api_format,
                                    &self.app,
                                    &self.sid,
                                    &self.run_id,
                                    self.total_loop_count + 1,
                                    &self.cancel_token,
                                    StreamConfig {
                                        is_subagent: false,
                                        cache_usage_style,
                                        on_frame: frame_tick.clone(),
                                        model_id: Some(self.model_id.clone()),
                                        crash_protection: self.crash_protection,
                                    },
                                )
                                .await,
                            );
                        }
                        None
                    };
                    match tokio::time::timeout(retry_budget, retry_fut).await {
                        Ok(Some(r)) => {
                            result = r;
                            if result.idle_timed_out {
                                println!("[JARVIS] 重试后仍为上游失联（零产出），不再重试");
                            }
                        }
                        Ok(None) => {}
                        Err(_) => {
                            println!(
                                "[JARVIS] 流式重试超过 {}s 预算（含读流），放弃重试",
                                crate::infra::types::constants::API_STREAM_RETRY_BUDGET_SECS
                            );
                            // 重试也没拿到结果 → 与空闲超时同样按中断收尾
                            result.idle_timed_out = true;
                        }
                    }
                }

                result
            };

            // 本轮流读取已结束：停掉等待提示看门狗（含重试流）。
            // 后面还有工具执行/下一轮循环，此处收尾最贴合"不再等上游数据"的语义。
            progress_watchdog.finish(&self.app, &self.sid, self.total_loop_count + 1);

            // 上游失联 → 保留现场并结束本轮，把决策权交还用户
            // （服务恢复后一句"继续"即可接上）。
            //
            // 注意：这里的判据是 `idle_timed_out`（计时器确实触发了），
            // **不是** `should_retry`。二者语义不同：
            // - "吐了半截后静默"时 should_retry=false（不该重试），但仍必须收尾并给出中断提示，
            //   否则界面只剩半截正文，用户会误以为已经正常完成（实测反馈的 bug）。
            if must_finish_as_interrupted(stream_result.idle_timed_out, stream_result.should_retry)
            {
                self.handle_stream_idle_timeout(stream_result).await;
                break;
            }

            let (
                mut current_blocks,
                tool_input_buffers,
                current_text_this_turn,
                mut current_thinking_this_turn,
                turn_has_tool,
                turn_in_tokens,
                turn_out_tokens,
            ) = (
                stream_result.blocks,
                stream_result.tool_input_buffers,
                stream_result.text,
                stream_result.thinking,
                stream_result.has_tool,
                stream_result.input_tokens,
                stream_result.output_tokens,
            );

            // 中断收尾要读的结构化现场（取消 / 报错可能发生在本 loop 的任意后续位置）
            self.current_blocks = current_blocks.clone();
            self.turn_text_this_turn = current_text_this_turn.clone();
            self.turn_thinking_this_turn = current_thinking_this_turn.clone();

            // ── 缓存命中：用"端点能力记忆"解释本次观测 ──
            // 有的厂商（实测 Kimi、小米 anthropic）在 0 命中时【不返回缓存字段】，
            // 因此单看一次响应无法区分"这家不报告"与"这次没命中（预热期）"。
            // 规则：该端点此前确认会上报 ⇒ 字段缺失按 0 命中解释；从未见过 ⇒ 保持未知。
            let cache_endpoint = crate::infra::llm::usage_memory::endpoint_key(
                &self.base_url,
                self.api_format.as_str(),
            );
            let cache_obs = crate::infra::llm::usage_memory::observe(
                &cache_endpoint,
                stream_result.cache_hit_tokens.is_some(),
                stream_result.cache_source,
            );
            if cache_obs.first_seen {
                println!(
                    "[JARVIS] 已记住端点 {} 会上报缓存字段（source={:?}）：此后字段缺失按 0 命中解释",
                    cache_endpoint, stream_result.cache_source
                );
            }
            let cache_cap = crate::infra::llm::usage_memory::capability(&cache_endpoint);
            let cache_outcome = crate::infra::llm::usage_memory::resolve_cache_outcome(
                stream_result.cache_hit_tokens,
                stream_result.cache_miss_tokens,
                stream_result.cache_source,
                cache_obs.reports_cache,
                cache_cap.cache_source.as_deref(),
                turn_in_tokens,
            );
            let cache_hit_tokens = cache_outcome.hit;
            let cache_miss_tokens = cache_outcome.miss;
            let cache_source = cache_outcome.source.as_deref();
            let cache_point = cache_hit_tokens.map(|hit| CacheHitPoint {
                loop_count: self.total_loop_count + 1,
                hit_tokens: hit,
                miss_tokens: cache_miss_tokens.unwrap_or(0),
                source: cache_outcome.source.clone(),
            });

            // 检测输出截断状态。
            //
            // 这里**只留日志，不再向 ToolResult 注入提示**：截断的处置建议已由
            // `infra::llm::adapters::ToolInputErrorKind::Truncated::advice()` 承担
            // （见 tools_runner.rs 生成 failure 处）。两处都写会在同一个工具结果里
            // 出现两条内容重叠的建议；且本处原文案写死了"每次只创建 1-2 个文件"，
            // 源自写文件场景，对 ProposePlan 这类调用并不贴切。
            // 分类 advice 的依据是 serde 对参数 JSON 的真实报错，比 `stop_reason` 更准。
            let is_truncated = matches!(
                stream_result.stop_reason.as_deref(),
                Some("max_tokens") | Some("length")
            );
            if is_truncated {
                println!(
                    "[JARVIS] 检测到输出截断 (stop_reason={:?})；截断提示由工具参数错误分类统一给出",
                    stream_result.stop_reason
                );
            }

            let tool_input_buffers_count = tool_input_buffers.len();
            self.req_input_tokens += turn_in_tokens;
            self.req_output_tokens += turn_out_tokens;
            if turn_in_tokens > 0 || turn_out_tokens > 0 {
                self.update_provider_usage_snapshot(
                    turn_in_tokens,
                    turn_out_tokens,
                    cache_hit_tokens,
                    cache_miss_tokens,
                    cache_source,
                    cache_point.as_ref(),
                );
            }

            // 提取工具调用信息
            let tool_calls: Vec<(String, String)> = tool_input_buffers
                .iter()
                .filter_map(|(idx, buf)| {
                    if let Some(ContentBlock::ToolUse { name, .. }) = current_blocks.get(*idx) {
                        Some((name.clone(), buf.clone()))
                    } else {
                        None
                    }
                })
                .collect();

            // 提取工具名称供反思审查使用
            let tool_names_for_reflection: Vec<String> =
                tool_calls.iter().map(|(name, _)| name.clone()).collect();

            // 记录响应摘要（含缓存命中：provider 未报告时是 None，不是 0）
            debug_logger::debug_logger().log_response(
                &self.sid,
                "MAIN",
                self.total_loop_count + 1,
                current_text_this_turn.len(),
                current_thinking_this_turn.len(),
                tool_calls.len(),
                turn_in_tokens,
                turn_out_tokens,
                cache_hit_tokens,
                cache_miss_tokens,
                cache_source,
                // provider 本次没报字段、数值来自端点记忆推断 ⇒ 日志里标明
                stream_result.cache_hit_tokens.is_none() && cache_hit_tokens.is_some(),
                stream_result.usage_raw.as_deref(),
            );

            // 记录思考过程
            debug_logger::debug_logger().log_thoughts(
                &self.sid,
                "MAIN",
                self.total_loop_count + 1,
                &current_thinking_this_turn,
                &current_text_this_turn,
                &tool_calls,
                self.req_input_tokens,
                self.req_output_tokens,
            );

            // 步骤 8：并行执行工具调用（tools_runner 三阶段流水线）
            // 工具执行
            let work_mode = self.ctx.agent_work_mode.lock().await.clone();
            let (mut tool_results, manual_compact, sub_in, sub_out) = execute_tool_calls(
                &mut current_blocks,
                tool_input_buffers,
                &self.app,
                &self.sid,
                &self.run_id,
                self.total_loop_count + 1,
                &self.cancel_token,
                &self.detected_intent,
                &work_mode,
            )
            .await;
            self.req_input_tokens += sub_in;
            self.req_output_tokens += sub_out;
            // 子代理用量另记一份，**仅供展示/诊断**：`JarvisResult`/`agent_runs`
            // 汇总里的输入输出已是含子代理的总量，这两个数用来说清"其中多少是子代理烧的"。
            // 它们**不参与** `sessions.total_*`——子代理每轮自己直写明细与累计。
            self.req_sub_input_tokens += sub_in;
            self.req_sub_output_tokens += sub_out;

            // （原「截断感知注入」已移除：截断的处置建议改由工具参数错误分类统一给出，
            //   见上方 `is_truncated` 处注释与 `ToolInputErrorKind::Truncated::advice()`。
            //   保留 `is_truncated` 仅用于日志诊断。）

            let _ = self.app.emit(
                "chat-turn-end",
                crate::infra::types::events::ChatTurnEndEvent {
                    has_tool: turn_has_tool,
                    session_id: self.sid.clone(),
                    loop_count: self.total_loop_count + 1,
                },
            );

            // 取消检查移到这里之前只做「下一轮是否继续」的判断。
            let was_cancelled_this_loop = self.cancel_token.is_cancelled();
            if was_cancelled_this_loop {
                println!(
                    "[JARVIS] 检测到取消：仍先落库本轮的助手回复与工具结果，避免「工具已执行但无记录」"
                );
            }

            // 步骤 9：把本轮助手响应（文本/思考/工具调用）写入会话历史。
            // 必须在取消判定之后仍执行——工具已在步骤 8 实际运行（可能已改文件），
            // 其结果必须留档；否则历史会出现 Assistant(ToolUse) 缺失 ToolResult 的残缺配对。
            //
            // 取消且无工具结果时跳过（2026-09-19 双写修复）：此时本轮半截内容
            // （可能只有 thinking，正文还没开始）由 handle_cancellation 的
            // store_interrupted_turn 统一收口成一条合并消息。若这里照常写入，
            // 半截内容会先落一条 src=chat 的消息；随后 store_interrupted_turn 的
            // 分支一去重只匹配 Text 块、匹配不到 thinking-only 消息，会再建一条
            // src=interrupted —— 同一段内容落库两遍（实测 seq 23/24 重复）。
            // 工具轮不受影响：tool_results 非空时仍照常写入保持配对完整。
            if !(was_cancelled_this_loop && tool_results.is_empty()) {
                self.store_assistant_response(&current_blocks).await;
            }

            // 工具结果为 User 消息，取消时同样必须落库以保持配对完整
            if was_cancelled_this_loop {
                if !tool_results.is_empty() {
                    let mut session = self.ctx.memory.lock().await;
                    append_message(
                        &mut session,
                        Message::User {
                            content: Content::Multiple(tool_results.clone()),
                        },
                        MessageSource::Chat,
                    );
                }
                continue;
            }

            // 检查工具是否请求结束本轮循环（如 ProposePlan 提交方案后等待用户审批）
            {
                let mut flags = self.ctx.tool_result_flags.lock().await;
                let should_break = flags.values().any(|(brk, _)| *brk);
                if should_break || tool_results.is_empty() {
                    println!(
                        "[JARVIS] break诊断: should_break={}, flags={:?}, tool_results.len()={}",
                        should_break,
                        flags
                            .iter()
                            .map(|(k, (brk, err))| format!("{}→break={},err={}", k, brk, err))
                            .collect::<Vec<_>>(),
                        tool_results.len()
                    );
                }
                flags.clear();
                if should_break {
                    // break 前必须存储 tool_results，否则下次 pipeline 会因
                    // Assistant(tool_calls) 后缺少 ToolResult 导致 API 400 错误
                    if !tool_results.is_empty() {
                        let mut session = self.ctx.memory.lock().await;
                        append_message(
                            &mut session,
                            Message::User {
                                content: Content::Multiple(tool_results.clone()),
                            },
                            MessageSource::Chat,
                        );
                    }
                    let tool_summary: String = tool_results
                        .iter()
                        .filter_map(|block| {
                            if let ContentBlock::ToolResult { content, .. } = block {
                                Some(content.as_str())
                            } else {
                                None
                            }
                        })
                        .collect::<Vec<_>>()
                        .join("\n\n");
                    // 保存工具结果摘要，传递给前端 toolBuffer
                    self.tool_execution_summary = if tool_summary.is_empty() {
                        None
                    } else {
                        Some(tool_summary.clone())
                    };
                    self.final_answer = if current_text_this_turn.trim().is_empty() {
                        tool_summary
                    } else if tool_summary.is_empty() {
                        current_text_this_turn
                    } else {
                        format!("{}\n\n{}", current_text_this_turn, tool_summary)
                    };
                    println!("[JARVIS] 主循环: 工具请求 break_loop，结束本轮");
                    break;
                }
            }

            // 步骤 10：判断是否继续 —— 无工具结果即为最终答案，结束循环
            // 判断是否继续循环
            println!(
                "[JARVIS] 主循环判断: tool_results.is_empty()={}, tool_results.len()={}, has_tool={}, tool_input_buffers.len()={}, text_len={}",
                tool_results.is_empty(), tool_results.len(), turn_has_tool,
                tool_input_buffers_count, current_text_this_turn.len()
            );
            if tool_results.is_empty() {
                self.final_answer = current_text_this_turn;
                // 模型可能只返回 thinking 而没有 text（DeepSeek 等模型常见）
                // 此时将 thinking 提升为回复，避免前端空响应
                if self.final_answer.trim().is_empty()
                    && !current_thinking_this_turn.trim().is_empty()
                {
                    self.final_answer = std::mem::take(&mut current_thinking_this_turn);
                }

                // 第3层防御：响应后置拦截 — 仅在 Edit/Plan 模式下检测 LLM 是否绕过 ProposePlan
                // tool_results 为空说明 LLM 没用任何工具，纯文本输出了方案
                let work_mode = self.ctx.agent_work_mode.lock().await.clone();
                if (work_mode == "edit" || work_mode == "plan")
                    && crate::core::complex_task::detect_plan_in_text(&self.final_answer)
                {
                    println!("[JARVIS] 响应后置拦截：检测到正文中的计划内容，重定向到 ProposePlan");
                    let _ = self.app.emit(
                        "chat-stream",
                        crate::infra::types::events::ChatStreamEvent {
                            content: "\n> **[方案重定向]** 检测到计划性内容，正在重定向到方案审批流程。\n"
                                .to_string(),
                            session_id: self.sid.clone(),
                            loop_count: Some(self.total_loop_count + 1),
                            is_sub_agent: None,
                        },
                    );

                    let redirect_msg = if work_mode == "plan" {
                        format!(
                            "【系统拦截通知】\n\
                            你当前处于规划模式，必须通过 ProposePlan 工具提交方案，不能在正文中直接输出计划内容。\n\
                            \n\
                            请调用 ProposePlan 工具，将你刚才的计划内容作为 content 参数提交。\n\
                            \n\
                            你刚才输出的内容摘要：\n{}",
                            self.final_answer.chars().take(500).collect::<String>()
                        )
                    } else {
                        format!(
                            "【系统拦截通知】\n\
                            你刚才在回复正文中输出了计划/步骤/方案内容，这违反了规则。\n\
                            计划必须通过 ProposePlan 工具提交到审批面板，不能写在正文里。\n\
                            \n\
                            请立即执行以下操作：\n\
                            1. 如果当前不在 Plan 模式，先调用 SwitchWorkMode(mode=\"plan\") 切换\n\
                            2. 调用 ProposePlan 工具，将你刚才的计划内容作为 content 参数提交\n\
                            3. 等待用户审批\n\
                            \n\
                            你刚才输出的内容摘要：\n{}",
                            self.final_answer.chars().take(500).collect::<String>()
                        )
                    };

                    {
                        let mut session = self.ctx.memory.lock().await;
                        // `Inject`：正文是"请调用 ProposePlan 工具…"的动作要求。
                        // 标成 `internal` 时它送不到模型，于是模型在同一上下文下反复
                        // 输出同样的计划正文 —— 看门狗只是把死循环变成"有限次后失败"。
                        append_message(
                            &mut session,
                            Message::User {
                                content: Content::Single(redirect_msg),
                            },
                            MessageSource::Inject,
                        );
                    }

                    // ⚠️ 关键修复（2026-09-19）：看门狗必须在这条「无工具 + 计划正文」
                    // 路径上也被评估。旧实现只挂在工具执行分支，而拦截死循环走的正是
                    // 无工具分支 —— 计数器永远停在 0，看门狗结构性失效，只能靠用户手点
                    // 取消脱困。
                    if self.update_plan_watchdog(&work_mode, &[]) {
                        println!(
                            "[JARVIS] Plan 看门狗触发（拦截路径）：loops_without_plan={}",
                            self.plan_total_loops_without_plan
                        );
                        self.handle_plan_watchdog_summary().await;
                        break;
                    }

                    self.loop_count += 1;
                    self.total_loop_count += 1;
                    continue;
                }

                // 本轮（无工具 = 最终回复）整轮 fallback 落库。
                //
                // ⚠️ 正常路径不靠这里：步骤 9 的 `store_assistant_response` 之后，
                // 紧跟的 `record_loop_event` 会把 resp（含 thinking/tool_use）与
                // tool_results 一起结构化写入 `agent_run_events`。这条 **continue 路径**
                // 绕过了那两处（它不落 session，也不走下面的工具分支），若不补写，
                // "拦截重定向"这类轮次就会在崩溃重建时凭空消失。
                //
                // thinking 从 `current_thinking_this_turn` 补回：它不是 `current_blocks`
                // 的可靠来源（流层对 thinking 的落块与文本累积是两条独立路径），
                // 而这里需要的是"这一轮模型说过什么"的完整快照。
                let resp_for_event = take_resp_blocks_with_thinking(
                    &current_blocks,
                    &mut current_thinking_this_turn,
                );
                self.record_loop_event(&resp_for_event, &[]).await;

                break;
            } else {
                println!("[JARVIS] 主循环: tool_results 不为空，添加到历史消息并继续循环");
                // 先检查是否需要反思审查（在获取 session 锁之前）
                let should_reflect = crate::core::agent::reflection::strategy::should_reflect(
                    &self.reflection_mode,
                    self.loop_count,
                    self.total_reflections,
                    self.consecutive_reflection_nos,
                    &tool_names_for_reflection,
                    &current_thinking_this_turn,
                );

                // —— 模式切换快照：本 loop 的 SwitchWorkMode 真实变更了 work_mode ——
                // 不修改已发出的 <context_snapshot>，而是生成一条 seq 更大的完整新快照，
                // 并入本条 tool_result 消息的块数组尾部（不独立成条）：
                // - 历史前缀仍逐字节命中缓存（该消息本轮新产生、从未入缓存前缀）
                // - system 的“最新快照优先”规则随即切到新模式现场
                // - session_messages 保持消息级 user/assistant 严格交替（数据结构对称）
                // 出网时 Context 块由 adapters 逐块 materialize 成 Text，与块位置无关；
                // 前端 user_display_content 对 Context 块本就跳过，不影响界面渲染。
                let mode_after_execution = { self.ctx.agent_work_mode.lock().await.clone() };
                let mode_switched = mode_after_execution != work_mode;
                if mode_switched {
                    println!(
                        "[JARVIS] 工作模式在本 loop 内切换：{} -> {}，新快照并入工具结果消息",
                        work_mode, mode_after_execution
                    );
                    self.merge_mode_snapshot_into_blocks(&mut tool_results, &mode_after_execution)
                        .await;
                }

                // 添加工具结果（模式切换时末块为新快照）到 session
                {
                    let mut session = self.ctx.memory.lock().await;
                    append_message(
                        &mut session,
                        Message::User {
                            content: Content::Multiple(tool_results.clone()),
                        },
                        MessageSource::Chat,
                    );
                    // 模式切换当轮立即落库：保证崩溃恢复时 snapshot_seq 与新快照同时存在，避免 seq 回退/重复
                    if mode_switched {
                        crate::core::session::save_session(&self.sid, &session, None);
                    }
                } // session 锁在这里释放

                // —— 反思审查：工具结果已写入 session，审查 Agent 携带完整上下文判断 ——
                //
                // 判定结果先收在这里，等本轮 `agent_run_events` 的行建好之后
                // （见下方 `upsert_loop_event`）再单独落库：默认配置下该行是
                // loop 收尾才建的，反思发生时它还不存在，写不进去。
                let mut reflection_record: Option<(String, Option<String>, Option<String>)> = None;
                if should_reflect {
                    println!(
                        "[审查 Agent] 触发反思 (mode={}, model={})",
                        self.reflection_mode, self.cfg.utility_model
                    );
                    // 获取 session 消息用于审查
                    let session_messages = {
                        let session = self.ctx.memory.lock().await;
                        session.messages.clone()
                    };

                    match crate::core::agent::reflection::strategy::execute_review(
                        &self.client,
                        &self.api_key,
                        &self.base_url,
                        &self.cfg.utility_model,
                        self.api_format,
                        &session_messages,
                        &self.msg,
                        &current_blocks,
                        &[],
                    )
                    .await
                    {
                        Ok((judgment, usage)) => {
                            // 审查请求本身也真实消耗 token，按 Reflection 来源单独入账；
                            // 无论判断是 OK 还是 NO 都有这次调用，所以记账放在分支判定之前。
                            // 记账失败只记日志，不得影响反思主流程。
                            let record = crate::core::session::RequestUsageRecord::new(
                                self.sid.clone(),
                                crate::core::session::UsageSource::Reflection,
                            )
                            .run(
                                self.run_id.clone(),
                                Some((self.total_loop_count + 1) as u32),
                            )
                            .model_id(self.cfg.utility_model.clone());
                            if let Err(err) = crate::core::session::usage::record_llm_usage(
                                &self.app,
                                record,
                                usage.as_ref(),
                            ) {
                                eprintln!("[JARVIS] 写入反思用量明细失败: {}", err);
                            }

                            match judgment {
                                // 判断 OK：工具结果符合预期，继续正常流程
                                crate::core::agent::reflection::ReflectionJudgment::Ok => {
                                    println!("[审查 Agent] 判断: OK");
                                    self.total_reflections += 1;
                                    self.consecutive_reflection_nos = 0;
                                    reflection_record = Some(("ok".to_string(), None, None));
                                    let _ = self.app.emit(
                                        "agent-step",
                                        crate::infra::types::events::AgentStepEvent::Reflection {
                                            session_id: self.sid.clone(),
                                            loop_count: self.total_loop_count + 1,
                                            judgment: "ok".to_string(),
                                            reason: None,
                                            suggestion: None,
                                        },
                                    );
                                }
                                // 判断 NO：递增连续 NO 计数，并把修正建议注入 session
                                crate::core::agent::reflection::ReflectionJudgment::NotOk {
                                    reason,
                                    suggestion,
                                } => {
                                    println!(
                                        "[审查 Agent] 判断: NO — {}\n建议: {}",
                                        reason, suggestion
                                    );
                                    self.total_reflections += 1;
                                    self.consecutive_reflection_nos += 1;
                                    reflection_record = Some((
                                        "not_ok".to_string(),
                                        Some(reason.clone()),
                                        Some(suggestion.clone()),
                                    ));
                                    let _ = self.app.emit(
                                        "agent-step",
                                        crate::infra::types::events::AgentStepEvent::Reflection {
                                            session_id: self.sid.clone(),
                                            loop_count: self.total_loop_count + 1,
                                            judgment: "not_ok".to_string(),
                                            reason: Some(reason.clone()),
                                            suggestion: Some(suggestion.clone()),
                                        },
                                    );

                                    // 注入修正建议到 session
                                    let mut session = self.ctx.memory.lock().await;
                                    // `Inject`：`doc/reflection-mechanism-design.md` §3.2 写明
                                    // "继续循环，LLM 在下一轮看到修正提示"。标成 `internal` 时
                                    // 这条提示进不了模型上下文，反思判定对主 Agent 零影响。
                                    append_message(
                                        &mut session,
                                        Message::User {
                                            content: Content::Single(format!(
                                                "审查发现以下问题：{}\n建议修正：{}\n请根据建议修正后继续。",
                                                reason, suggestion
                                            )),
                                        },
                                        MessageSource::Inject,
                                    );
                                }
                            }
                        }
                        Err(e) => {
                            // 审查调用失败不影响主流程，仅记录日志；失败没有用量可记
                            println!("[审查 Agent] 调用失败: {}", e);
                        }
                    }
                }

                // 执行后续操作
                let mut session = self.ctx.memory.lock().await;
                // LLM 请求了 CompactConversation：工具结果写回后立即执行压缩
                if manual_compact {
                    let _ = auto_compact(
                        &self.sid,
                        &mut session,
                        &self.client,
                        &self.api_key,
                        &self.base_url,
                        &self.model_id,
                        self.api_format,
                    )
                    .await;
                }
                agent_runs::upsert_loop_event(
                    &self.run_id,
                    &self.sid,
                    self.total_loop_count + 1,
                    current_blocks.clone(),
                    tool_results.clone(),
                    "complete",
                    None,
                    turn_in_tokens,
                    turn_out_tokens,
                    Some(self.model_id.clone()),
                );
                drop(session);

                // 反思判定落库（独立 UPDATE，且刻意不进 upsert_loop_event 的覆盖集合——
                // 否则帧级通道/收尾的任何一次重写都会把它抹掉）
                if let Some((judgment, reason, suggestion)) = reflection_record {
                    agent_runs::mark_loop_reflection(
                        &self.run_id,
                        self.total_loop_count + 1,
                        &judgment,
                        reason.as_deref(),
                        suggestion.as_deref(),
                    );
                }

                // B3 Plan 看门狗：仅 plan 模式；连续 20 轮未提交 ProposePlan 时，
                // 先做一次缓存友好的 LLM 进度小结，再强制停下交还决策权。
                // 传 tool_calls（含参数 JSON）而非纯名字列表：喂狗判定要解包 ExecuteTool 包装。
                let current_mode_after = self.ctx.agent_work_mode.lock().await.clone();
                if self.update_plan_watchdog(&current_mode_after, &tool_calls) {
                    println!(
                        "[JARVIS] Plan 看门狗触发：loops_without_plan={}",
                        self.plan_total_loops_without_plan
                    );
                    self.handle_plan_watchdog_summary().await;
                    break;
                }
            }
            self.loop_count += 1;
            self.total_loop_count += 1;
            // 步骤 11：进入下一轮循环（累计轮数，超 200 轮绝对上限强制停止）
            println!(
                "[JARVIS] 主循环: 进入下一轮 loop_count={}, total_loop_count={}",
                self.loop_count, self.total_loop_count
            );

            if self.total_loop_count >= crate::infra::types::constants::MAX_AGENT_LOOP_ABSOLUTE {
                self.final_answer = format!(
                    "代理执行超过绝对上限 {} 轮，为防止死循环已强制停止。",
                    crate::infra::types::constants::MAX_AGENT_LOOP_ABSOLUTE
                );
                break;
            }
        }

        Ok(())
    }
}
