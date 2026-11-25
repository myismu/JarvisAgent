//! # pipeline/recovery.rs — 异常收尾、取消处理、流空闲超时与循环确认

use super::*;

impl PipelineState {
    /// 异常收尾：把中断原因记录到审计层与诊断层，**保留已产生的内容与历史**，
    /// 并清理运行状态（清空 active_run_id、释放本轮执行槽，保证下次可重新执行）。
    ///
    /// 历史演进说明：旧实现只做「记错误 → fail_run → 返回 Err」，现场全部丢弃。
    /// 由于 `run_pipeline_inner` 在此之后直接 `return Err`，`finalize()` 不会执行，
    /// 于是会话历史里连一句中断说明都没有，用户误以为"根本没执行"。
    /// 现改为：把**本轮的结构化响应块**（`current_blocks`）连同中断提示
    /// **合并成一条**助手消息落库（见 `store_interrupted_turn` 的丙方案说明），保持一问一答。
    ///
    /// 遵守的约束：**错误文本不作为助手消息内容**（避免被当成模型发言污染上下文），
    /// 只写入 agent_runs.error 与 agent_run_events。
    pub(super) async fn abort_after_error(&mut self, error: &AgentError) {
        if self.run_id.is_empty() {
            self.execution_lease.finish();
            return;
        }

        // 面向用户的小字标注：错误原文可能很长且含技术细节，
        // 这里给一句可读的结论，完整错误走事件层与 agent_runs.error（界面"执行详情"可查）。
        // 状态标注只出 key；具体错误走 detail 原样展示（错误文本本身无法翻译）
        let notice_detail = error.to_string();
        println!("[JARVIS] 异常收尾（保留现场）: {}", error);

        // 1. 把已流式输出但尚未入库的内容补进历史。
        //
        // v15 起这里改读**内存中的结构化块**（`PipelineState.current_blocks`），
        // 不再是 `agent_runs.live_content / live_thinking` 那对字符串。理由：
        //
        // - 旧实现是"每帧把正文追加进一段大字符串"，思考与正文挤在两条独立的
        //   累积流里，中断时按字符串顺序拼回去 → 思考必然落在正文之前/之后某一侧，
        //   **与块的真实交错顺序脱节**（这正是方案 §3.1 记的"正文糅合"）。
        // - 结构化块从流层出来就是 `Vec<ContentBlock>`（Text / Thinking / ToolUse
        //   各占一块，顺序即产生顺序），既不用猜顺序，也不会把工具调用当成正文。
        //
        // 兜底：`current_blocks` 为空（如尚未收到任何帧就被取消）而 turn 级字符串
        // 有内容时，用字符串重建，避免"什么都没有"时把内容丢掉。
        let current_blocks = self.current_blocks.clone();
        let (thinking, text) = if current_blocks.is_empty() {
            (
                self.turn_thinking_this_turn.trim().to_string(),
                self.turn_text_this_turn.trim().to_string(),
            )
        } else {
            let (t, k) = blocks_to_text_and_thinking(&current_blocks);
            (k, t)
        };

        // 落库口径（丙方案）：半截内容 + 中断提示合并成**一条**助手消息。
        // 错误文本本身**不作为**助手消息内容（避免被当成模型发言污染上下文），
        // 只由统一的 INTERRUPT_MARKER_RESUMABLE 提示"该接着做"。
        {
            let mut session = self.ctx.memory.lock().await;
            store_interrupted_turn(&mut session, &text, &thinking, InterruptKind::PipelineError)
                .await;
        }

        self.final_answer = text;
        self.notice_i18n_key = Some("notice.pipelineError");
        self.notice_detail = Some(notice_detail);
        self.interrupted_reason = Some(error.to_string());

        // 3. 落库：本轮标记为 interrupted（**保留**帧级通道已写的半截内容）
        self.mark_loop_event_interrupted(error.to_string()).await;
        // run 级中断类型与消息级一致（上面 store_interrupted_turn 用的就是 PipelineError）
        agent_runs::interrupt_run(
            &self.app,
            &self.run_id,
            InterruptKind::PipelineError,
            self.req_input_tokens,
            self.req_output_tokens,
            // 等待用户审批的净耗时（区间并集；取走即清空）
            self.ctx.take_run_wait_ms().await,
        );

        // 4. 中断后立即持久化，保证用户刷新后仍能看到保留的内容与中断标记
        {
            let memory = self.ctx.memory.lock().await.clone();
            crate::core::session::save_session(&self.sid, &memory, None);
            let _ = self.app.emit("session-updated", ());
            // 中断保留的半截内容已落库：同一条通知让其它视图标脏
            crate::core::session::notify_messages_changed(&self.app, &self.sid, "interrupted");
        }

        // 5. 通知前端（与用户取消区分：type = interrupted）
        //    状态标注走 notice 结构化下发，不再推进正文
        let _ = self.app.emit(
            "agent-step",
            crate::infra::types::events::AgentStepEvent::Interrupted {
                reason: "pipeline_error".to_string(),
                session_id: self.sid.clone(),
                loop_count: self.total_loop_count + 1,
            },
        );

        {
            let mut active_run_id = self.ctx.active_run_id.lock().await;
            if active_run_id.as_deref() == Some(&self.run_id) {
                *active_run_id = None;
            }
        }
        self.execution_lease.finish();
    }

    /// 中断收尾共用逻辑：**保留全部已持久化历史**，并把中断提示追加为一条
    /// `source = "interrupted"` 的助手消息。
    ///
    /// 与「用户主动撤销」（撤回消息 / 回滚检查点）的关键区别：
    /// 撤销是用户希望内容消失，截断正确；中断只是运行被打断，
    /// **任何一方都不希望数据消失**，因此这里绝不 truncate。
    ///
    /// `source = "interrupted"` 的语义定位：
    /// - LLM 上下文：**可见**（`prepare_history_snapshot_from_messages` 白名单），
    ///   使"继续"时模型能看到自己中断于何处；
    /// - UI 渲染：**可见**（`command/history.rs` 的渲染门），否则中断轮次会从界面消失。
    ///
    /// 阶段二起正文**不再放标记文本**（留空块作渲染锚点），中断类型走
    /// `interrupt_kinds`；标记在发送给模型前由 `interrupt_marker_for` 按 kind 拼回。
    ///
    /// 返回该消息在会话历史中的下标。
    pub(super) async fn append_interrupted_marker(&mut self, kind: InterruptKind) -> usize {
        let mut session = self.ctx.memory.lock().await;
        append_message_with_kind(
            &mut session,
            Message::Assistant {
                content: Content::Multiple(vec![ContentBlock::Text {
                    text: String::new(),
                }]),
            },
            MessageSource::Interrupted,
            Some(kind.as_str()),
        );
        session.messages.len() - 1
    }

    /// 等待上游响应期间的"进度提示"看门狗。
    ///
    /// 上游深度思考、网络迟滞、或者**收到响应头后正文长时间静默**时，界面上
    /// 可能长时间没有任何事件，用户只看到转圈，无法区分"模型在思考"与
    /// "服务已经死了"。本看门狗让等待**可见**。
    ///
    /// ## 为什么必须跨越两个阶段
    ///
    /// 首个版本的看门狗只覆盖"等待响应头"阶段，一旦 `call_api_with_retry`
    /// 返回就 `abort()`。但真实故障几乎都发生在**响应头已到、正文静默**阶段
    /// （响应头通常很快返回），于是看门狗永远没机会触发。
    ///
    /// 因此改为：从请求发出前启动，一直存活到本轮流结束，
    /// 启动进度提示看门狗。返回后应立即 `touch()` 一次以开始计时。
    pub(super) fn spawn_waiting_hint(&self) -> WaitingHint {
        use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};

        let hint_secs = crate::core::agent::stream::waiting_hint_secs();
        let last_tick_ms = Arc::new(AtomicU64::new(now_millis()));
        let hint_sent = Arc::new(AtomicBool::new(false));
        let done = Arc::new(AtomicBool::new(false));
        let app = self.app.clone();
        let sid = self.sid.clone();
        let loop_count = self.total_loop_count + 1;
        let tick = last_tick_ms.clone();
        let sent = hint_sent.clone();
        let stop = done.clone();

        let handle = tokio::spawn(async move {
            // 轮询粒度取提示阈值的一半，保证提示足够及时
            let step = Duration::from_secs((hint_secs / 2).max(1));
            loop {
                tokio::time::sleep(step).await;
                if stop.load(Ordering::Relaxed) {
                    return;
                }
                let silent_ms = now_millis().saturating_sub(tick.load(Ordering::Relaxed));
                // 同一段静默期只提示一次：收到新数据后 touch() 会复位 sent
                if should_emit_waiting_hint(silent_ms / 1000, sent.load(Ordering::Relaxed)) {
                    sent.store(true, Ordering::Relaxed);
                    // 走 agent-step 而非 chat-stream：等待提示属于**状态标注**，
                    // 应渲染在气泡下方的小字里；经 chat-stream 会混进模型正文，
                    // 在回复气泡内部显示（实测反馈的 UI 问题）。
                    //
                    // 事件**不带 content 文案**，只带两个秒数，由前端拼句子 ——
                    // 后端写死中文会让英文界面在等待时冒中文（本文件 notice 一节的同一问题）。
                    let silent_secs = silent_ms / 1000;
                    // 剩余秒数按当前生效的终止阈值算。
                    // ⚠️ 这里用 `STREAM_IDLE_TIMEOUT_SECS` 是**准确**的：等待响应头
                    // 阶段由 `API_RESPONSE_HEADER_TIMEOUT_SECS` 收尾，两者当前同为
                    // 120s，故两个阶段的剩余时间算法一致。**若将来把这两个阈值改成
                    // 不相等**，需要按阶段把各自的阈值传进来，不能再用同一个常量。
                    let remain_secs = crate::core::agent::stream::STREAM_IDLE_TIMEOUT_SECS
                        .saturating_sub(silent_secs);
                    let _ = app.emit(
                        "agent-step",
                        crate::infra::types::events::AgentStepEvent::WaitingHint {
                            // 两个秒数交给前端 `notice.waitingHint` 组句
                            silent_secs,
                            remain_secs,
                            session_id: sid.clone(),
                            loop_count,
                        },
                    );
                }
            }
        });

        WaitingHint {
            last_tick_ms,
            hint_sent,
            done,
            handle,
        }
    }

    /// 把本 loop 的完整轮次打上 `interrupted` 标记。
    ///
    /// 与 [`Self::record_loop_event`] 共用同一把尺子（同一个 `loop_index`、同一套
    /// resp/tool 块），只差 `status` 与 `error`——中断轮同样要有 events 行，
    /// 否则崩溃重建时"进行到一半就被打断的那一轮"会整轮消失。
    ///
    /// 之所以要**标记而不是删除**：删除等于宣称"这轮没发生过"，而它的工具可能
    /// 已经改了文件；标记为 interrupted 才能让重建如实还原现场。
    ///
    /// 轮号与收尾写入同取 `total_loop_count + 1`（见 [`Self::record_loop_event`] 的
    /// 取号说明）。`total_loop_count == 0` 时**不再提前返回**：帧级通道可能已经
    /// 建了第 1 行（streaming），漏标会让崩溃重建把它当有效半截内容重放；
    /// UPDATE 0 行本就无害，交给 SQL 即可。
    pub(super) async fn mark_loop_event_interrupted(&self, error: String) {
        if self.run_id.is_empty() {
            return;
        }
        agent_runs::mark_loop_event_interrupted(
            &self.run_id,
            &self.sid,
            self.total_loop_count + 1,
            error,
        );
    }

    /// 把本 loop 的响应与工具结果结构化写入 `agent_run_events`（崩溃重建的唯一数据源）。
    ///
    /// ## 取号说明（⚠️ 曾因注释错误取错轮号）
    ///
    /// `total_loop_count` 在**每轮结束时**才自增（主循环各 continue/break 分支），
    /// `process_stream` 收到的帧级轮号是 `total_loop_count + 1`。收尾覆盖必须用
    /// **同一个号**，才能落回帧级通道建的那一行——此处曾误用未自增的
    /// `total_loop_count`，把本轮内容覆盖到**上一轮**的 events 行上
    /// （上一轮 tool_results 丢失、取消标记错标上一轮）。
    pub(super) async fn record_loop_event(&self, resp_blocks: &[ContentBlock], tool_results: &[ContentBlock]) {
        agent_runs::upsert_loop_event(
            &self.run_id,
            &self.sid,
            self.total_loop_count + 1,
            resp_blocks.to_vec(),
            tool_results.to_vec(),
            "complete",
            None,
            self.req_input_tokens,
            self.req_output_tokens,
            Some(self.model_id.clone()),
        );
    }

    /// 用户取消处理：保留全部历史与已流式输出的部分内容，把中断原因作为
    /// `interrupted` 消息追加，并标记 run 为 CANCELLED。
    ///
    /// 历史演进说明：旧实现在此处把历史 `truncate` 到本轮用户消息之后，
    /// 结果是「已跑完的 10 轮工具调用 + 思考」被整段删除，只剩一句残文；
    /// 且因取消检查位于 `store_assistant_response()` 之前，中断轮的回复与
    /// 工具结果也从未写入。现改为不截断、只追加标记。
    pub(super) async fn handle_cancellation(&mut self) {
        println!(
            "[JARVIS] 用户已取消执行：保留全部历史，user index {}",
            self.initial_msg_index
        );

        // 取回已流式输出的部分结果。
        //
        // v15 起改读**内存中的结构化块**（理由见 `abort_after_error`）：
        // 块顺序即真实产生顺序，思考/正文/工具调用各归其位，不再靠字符串拼接猜。
        // 兜底与 `abort_after_error` 一致：块为空时回落到 turn 级字符串。
        let (partial, partial_thinking) = {
            let blocks = self.current_blocks.clone();
            if blocks.is_empty() {
                let t = self.turn_text_this_turn.trim().to_string();
                let k = self.turn_thinking_this_turn.trim().to_string();
                (
                    if !t.is_empty() {
                        t
                    } else if !self.final_answer.is_empty()
                        && self.final_answer != "用户已取消执行。"
                    {
                        std::mem::take(&mut self.final_answer)
                    } else {
                        String::new()
                    },
                    k,
                )
            } else {
                let (t, k) = blocks_to_text_and_thinking(&blocks);
                (t, k)
            }
        };

        // 面向用户的状态标注只出 i18n key（文案在前端 `notice.userCancel`）；
        // 写入历史的标记面向 LLM（会进上下文），故用最小信息量的统一措辞，
        // 避免模型把系统视角描述当成自己的话 —— 见 `interrupt_marker_for`。

        // 落库口径（丙方案）：半截内容 + 中断提示合并成**一条**助手消息，
        // 保持一问一答；正文为空时把提示本身作为正文块，保证前端渲染锚点存在。
        {
            let mut session = self.ctx.memory.lock().await;
            store_interrupted_turn(
                &mut session,
                &partial,
                &partial_thinking,
                InterruptKind::UserCancel,
            )
            .await;
        }

        self.final_answer = partial.clone();
        self.notice_i18n_key = Some("notice.userCancel");
        self.interrupted_reason = Some("用户取消".to_string());

        // 状态标注走 notice 结构化下发，不再推进正文
        let _ = self.app.emit(
            "agent-step",
            crate::infra::types::events::AgentStepEvent::Cancelled {
                session_id: self.sid.clone(),
                loop_count: self.total_loop_count + 1,
            },
        );
        // 标记 run 为 CANCELLED，并向前端发送取消通知。
        // 轮次事件同样打上 interrupted（保留帧级通道已写的半截内容）。
        self.mark_loop_event_interrupted("用户取消".to_string())
            .await;
        agent_runs::cancel_run(
            &self.app,
            &self.run_id,
            self.req_input_tokens,
            self.req_output_tokens,
            Some(self.final_answer.clone()),
            // 等待用户审批的净耗时（区间并集；取走即清空）
            self.ctx.take_run_wait_ms().await,
        );
        println!("[JARVIS] 已把中断轮内容与提示合并落库（interrupted）");
    }

    /// 上游失联（流内空闲超时）收尾：保留现场，结束本轮，把决策权交还用户。
    ///
    /// 流层已采用「优雅终止」——`process_stream` 返回已累积的部分结果而非抛错，
    /// 因此这里不需要（也不应该）走 `fail_run` 的抹除路径。服务恢复后用户
    /// 直接说"继续"即可接上，因为历史完整且标记对模型可见。
    pub(super) async fn handle_stream_idle_timeout(
        &mut self,
        stream_result: crate::core::agent::StreamResult,
    ) {
        let partial = stream_result.text.trim().to_string();
        println!(
            "[JARVIS] 上游超时收尾：已累积文本 {} 字，工具={}，loop={}",
            partial.chars().count(),
            stream_result.has_tool,
            self.total_loop_count + 1
        );

        // 本轮的完整响应块（含 thinking / tool_use）——断流时流层已优雅返回，
        // 这里把它整轮留档，供崩溃重建原样还原（工具调用不能丢：它可能已经改了文件）。
        let mut thinking_for_event = stream_result.thinking.clone();
        let resp_blocks_for_event =
            take_resp_blocks_with_thinking(&stream_result.blocks, &mut thinking_for_event);

        // 面向用户的状态标注只出 i18n key，文案在前端 `notice.streamTimeout`。
        // 等待了多少秒作为 detail 原样带上（数字无需翻译）。
        //
        // ⚠️ 归因口径（前端写文案时仍受此约束，勿再混淆）：本函数是**流空闲超时**
        // （SSE 30s 零帧）的收尾，与 plan 模式的「规划看门狗」
        // （update_plan_watchdog / 阈值 20 轮）是两套完全不同的机制。
        // 文案只描述"等待"这个客观事实，**不得断言服务端状态**
        // （"已停止响应/已失联"都不行）——静默可能是服务繁忙、链路抖动或半开连接，
        // 在链路上不可区分，断言会误导用户。
        let waited_secs = crate::core::agent::stream::STREAM_IDLE_TIMEOUT_SECS.to_string();

        // 落库口径（丙方案）：半截内容落成**一条**助手消息，中断类型结构化写进
        // `interrupt_kinds`（标记文本在发送给模型前由 kind 拼回，见
        // `interrupt_marker_for`）。断流时流层已优雅返回，思考内容同样随正文落库。
        {
            let mut session = self.ctx.memory.lock().await;
            store_interrupted_turn(
                &mut session,
                &partial,
                &stream_result.thinking,
                InterruptKind::StreamTimeout,
            )
            .await;
        }

        self.final_answer = partial.clone();
        self.notice_i18n_key = Some("notice.streamTimeout");
        self.notice_detail = Some(waited_secs);
        // 状态字符串（进 agent_runs.error / 审计，非用户可见）：同样只描述等待事实，
        // 不断言服务端。见本函数上方 handle_stream_idle_timeout 的归因注释。
        self.interrupted_reason = Some("上游响应超时（流内空闲超时）".to_string());

        // 状态标注不再经 chat-stream 推进正文（会挤进回复气泡内部），
        // 改由 JarvisResult.notice_i18n_key 结构化下发，前端渲染为气泡下方小字。
        let _ = self.app.emit(
            "agent-step",
            crate::infra::types::events::AgentStepEvent::Interrupted {
                reason: "stream_idle_timeout".to_string(),
                session_id: self.sid.clone(),
                loop_count: self.total_loop_count + 1,
            },
        );
        println!(
            "[JARVIS] 中断收尾完成：notice={:?}，content 长度={}",
            self.notice_i18n_key.unwrap_or("<无>"),
            self.final_answer.chars().count()
        );

        // 记录到审计层（agent_run_events），供界面「执行详情」展示中断原因；
        // 同时保留本轮已收到的响应块，崩溃重建时可原样还原。
        // 轮号 +1 与帧级通道/收尾覆盖同尺（见 record_loop_event 取号说明）。
        agent_runs::upsert_loop_event(
            &self.run_id,
            &self.sid,
            self.total_loop_count + 1,
            resp_blocks_for_event,
            Vec::new(),
            "interrupted",
            // 审计层写**技术原因**（非用户文案）——它进「执行详情」与日志，
            // 与 notice 的 i18n key 是两回事
            Some("上游响应超时（流内空闲超时）".to_string()),
            0,
            0,
            Some(self.model_id.clone()),
        );

        // ⚠️ 把 run 状态落为 Interrupted（与错误中止/用户取消两条收尾路对齐）。
        //
        // 此前本路径只写 events 行、不更新 agent_runs.status —— run 永久停留
        // Running，重启后被 find_interrupted_run 以 STALE 判定当崩溃遗留捞出，
        // 触发恢复链把已收尾的内容再补一遍（session 10b68285 重复数据事故的
        // 直接根因）。统一收尾处对 interrupted_reason 路径刻意跳过 complete_run，
        // 所以这里必须自己落状态。
        //
        // 注意：本处 reason 属**流空闲超时**（stream.rs 的 STREAM_IDLE_TIMEOUT_SECS），
        // 不是 plan 模式的「规划看门狗」——后者走 handle_plan_watchdog_summary
        // 且以 chat source 正常收尾，不会进本路径。措辞只描述超时事实，不断言服务端。
        agent_runs::interrupt_run(
            &self.app,
            &self.run_id,
            InterruptKind::StreamTimeout,
            self.req_input_tokens,
            self.req_output_tokens,
            // 等待用户审批的净耗时（区间并集；取走即清空）
            self.ctx.take_run_wait_ms().await,
        );
    }

    /// 循环上限确认（满 30 轮触发）：弹窗请用户授权继续
    /// 返回 true 表示继续（重置 loop_count），false 表示终止；
    /// 超时或拒绝时置 loop_continuation_pending，用户稍后可通过 resume_pipeline 续跑
    pub(super) async fn request_loop_continuation(&mut self) -> bool {
        let _ = self.app.emit(
            "chat-stream",
            crate::infra::types::events::ChatStreamEvent {
                content: format!("\n> **[等待确认]** 代理执行已达到 {} 回合，等待用户确认是否继续。\n", crate::infra::types::constants::MAX_AGENT_LOOP_BEFORE_CONFIRM),
                session_id: self.sid.clone(),
                loop_count: Some(self.total_loop_count + 1),
                is_sub_agent: None,
            },
        );
        // 弹权限确认：允许 → 继续并重置本轮计数；超时/拒绝 → 置可续跑标记
        let decision = request_permission(
            &self.app,
            &self.sid,
            &format!(
                "代理执行已达到 {} 回合，可能任务较为复杂或陷入循环。是否继续执行？",
                crate::infra::types::constants::MAX_AGENT_LOOP_BEFORE_CONFIRM
            ),
            PermissionKind::LoopContinuation,
            // 循环续跑确认没有会话级允许语义：照抄"本次会话都允许"会顺带放行其它工具调用
            Vec::new(),
        )
        .await;
        if decision.is_allowed() {
            // 清除待续跑标记（用户及时响应了）
            *self.ctx.loop_continuation_pending.lock().await = false;
            self.loop_count = 0;
            let _ = self.app.emit(
                "chat-stream",
                crate::infra::types::events::ChatStreamEvent {
                    content: "\n> **[已授权]** 用户已授权继续执行。\n".to_string(),
                    session_id: self.sid.clone(),
                    loop_count: Some(self.total_loop_count + 1),
                    is_sub_agent: None,
                },
            );
            true
        } else {
            // 只有"没拿到结论"（取消、通道关闭）才保留续跑入口；
            // 用户明确点了拒绝，就不该再给一个"稍后点允许继续"的后门
            if decision.is_rejected() {
                *self.ctx.loop_continuation_pending.lock().await = false;
                println!("[JARVIS] 用户拒绝继续执行，循环终止");
            } else {
                *self.ctx.loop_continuation_pending.lock().await = true;
                println!(
                    "[JARVIS] 循环续跑确认未完成（{}），保留续跑入口",
                    decision.status_label()
                );
            }
            false
        }
    }
}
