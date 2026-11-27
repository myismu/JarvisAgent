//! # pipeline/finalize.rs — 管线阶段 4（收尾：持久化、快照与结果组装）

use super::*;

impl PipelineState {
    /// 阶段 4：收尾 — 持久化 + 快照 + 记忆 + 结果组装
    ///
    /// 1. 崩溃兜底：把内存中未落库的编辑补丁先写入 agent_run_patches 表
    /// 2. 文件快照：本轮有文件改动才创建 Git 检查点（纯聊天轮次跳过），供 UI 回滚
    /// 3. 保存会话到 SQLite（含累计 token），必要时异步调 LLM 自动起名
    /// 4. 记忆超预算时后台整理一次（不阻塞返回；新增事实由主 Agent 的 UpdateMemory 负责）
    /// 5. 按取消/循环超时/正常三种情况组装 JarvisResult（FINISH / CANCELLED / PAUSED_LOOP_LIMIT）
    pub(super) async fn finalize(mut self) -> JarvisResult {
        let was_cancelled = self.cancel_token.is_cancelled();
        let was_loop_timeout = *self.ctx.loop_continuation_pending.lock().await;

        // 崩溃兜底：提交前先把补丁持久化到 agent_run_patches 表
        {
            let manager = self.app.state::<crate::core::runtime::SessionManager>();
            let ctx = manager.get_or_create(&self.sid).await;
            let patches: Vec<_> = ctx.pending_patches.lock().await.clone();
            for p in &patches {
                let _ = crate::core::rollback::patch_store::insert_agent_run_patch(
                    &self.sid,
                    &p.run_id,
                    p.seq,
                    &p.patch,
                    p.message.as_deref(),
                    p.trigger_user_message_id.as_deref(),
                );
            }
        }

        // 创建检查点快照（仅在有文件编辑时创建实快照，纯聊天轮次不创建）。
        // 改为块表达式把 (checkpoint_id, has_patches) 带出块外：发消息那一刻检查点
        // 尚不存在（收尾才创建），随 JarvisResult 下发后前端可实时回填
        // 「会话和代码撤回」菜单，不必等刷新走 get_session_messages 重查库。
        let (checkpoint_id, checkpoint_has_patches) = {
            let has_patches =
                crate::core::tools::file_tools::has_pending_patches(&self.app, &self.sid).await;

            let checkpoint_id = if has_patches {
                crate::core::tools::file_tools::commit_pending_snapshot(
                    &self.app,
                    &self.sid,
                    self.user_msg_preview.clone(),
                    self.user_message_id.clone(),
                )
                .await
            } else {
                // 纯聊天轮次 → 不创建快照，仅记录日志
                println!("[JARVIS] 纯聊天轮次，跳过快照创建");
                None
            };

            if let Some(id) = &checkpoint_id {
                println!("[JARVIS] 已创建本轮文件快照: {} (来自快照引擎)", id);
            }
            let _ = self.app.emit(
                "checkpoint-created",
                crate::infra::types::events::CheckpointCreatedEvent {
                    session_id: self.sid.clone(),
                    checkpoint_id: checkpoint_id.clone(),
                    has_patches,
                    can_rollback: true,
                    message: self.user_msg_preview.clone(),
                },
            );
            (checkpoint_id, has_patches)
        };

        // 3. 保存会话到 SQLite；纯聊天且被取消时不落空会话
        //
        // 收尾这里**不交任何 token 增量**（`None`）：
        // - 主循环的每个 loop 已在 `update_provider_usage_snapshot` 里逐请求直写；
        // - 子代理也在它自己的每一轮里经 `record_request_usage` 直写。
        // 两边都已入账，收尾再补一次就是**同一笔账记两遍**。
        //
        // 曾经的写法：收尾只补子代理那一份（因为主循环当时已经逐请求累加）。
        // 子代理改成逐请求直写后，这层手工补偿失去了存在理由——它只能靠
        // "主循环不算、子代理算"这类不对称约定维持正确，一旦任一侧改变写入时机
        // 就会静默双计（明细行数与累计值对不上，而不会有任何报错）。
        //
        // `session_meta` 仍然照常读回：`save_session` 返回的 `total_*` 是从库里
        // 读出的**当前累计值**（已含本轮所有请求），随 `JarvisResult` 下发不受影响。
        let memory = self.ctx.memory.lock().await.clone();

        let session_meta = if memory.messages.is_empty() && was_cancelled {
            None
        } else {
            let meta = crate::core::session::save_session(&self.sid, &memory, None);
            println!("[JARVIS] 会话 {} 已自动保存", self.sid);
            Some(meta)
        };
        if let Some(ref meta) = session_meta {
            let _ = self.app.emit("session-updated", ());
            // 回合收尾：末条消息与计数可能已变，统一在这里补一次变更通知
            crate::core::session::notify_messages_changed(&self.app, &self.sid, "turn-finalize");

            // 自动命名：消息数足够且未命名过时，后台调 LLM 生成标题
            if !was_cancelled && meta.message_count >= 2 && meta.title_source == "default" {
                let app_clone = self.app.clone();
                let sid_clone = self.sid.clone();
                let memory_clone = memory.clone();
                tokio::spawn(async move {
                    if let Err(e) = crate::core::session::naming::auto_name_session(
                        app_clone,
                        sid_clone,
                        memory_clone,
                    )
                    .await
                    {
                        println!("[JARVIS] Auto-naming failed: {}", e);
                    }
                });
            }
        }

        // 4. 记忆维护：只有全局记忆超过阈值时才后台整理一次
        // 原先是每轮结束都跑一次 LLM 重写，既贵，又会让记忆越滚越乱。
        // 新增事实由主 Agent 的 UpdateMemory 负责，这里只做合并压缩。
        if global_memory_char_count()
            > crate::core::tools::agent_tools::MEMORY_CONSOLIDATE_THRESHOLD_CHARS
        {
            let cfg_clone = self.cfg.clone();
            let sid_for_memory = self.sid.clone();
            let app_for_memory = self.app.clone();
            tokio::spawn(async move {
                run_memory_curator(app_for_memory, cfg_clone, sid_for_memory).await;
            });
        }

        // 汇总状态：正常结束 / 用户取消 / 循环超时暂停 / 运行被打断
        // interrupted_reason 由 handle_cancellation / handle_stream_idle_timeout 设置，
        // 且必须先于 was_cancelled 判断：取消也会置起 cancel_token，但语义是"中断"。
        let interrupted_reason = self.interrupted_reason.clone();
        let status = if interrupted_reason.is_some() {
            "INTERRUPTED"
        } else if was_cancelled {
            "CANCELLED"
        } else if was_loop_timeout {
            "PAUSED_LOOP_LIMIT"
        } else {
            "FINISH"
        };
        if was_loop_timeout {
            self.final_answer = format!(
                "代理执行已达到 {} 回合上限，等待用户确认是否继续。",
                crate::infra::types::constants::MAX_AGENT_LOOP_BEFORE_CONFIRM
            );
        }

        // 会话日志
        {
            debug_logger::debug_logger().log_session_summary(
                &self.sid,
                self.req_input_tokens,
                self.req_output_tokens,
                status,
            );
        }

        if let Some(reason) = &interrupted_reason {
            println!(
                "[JARVIS] 本轮为中断收尾（{}）：历史已保留，等待用户决定是否继续",
                reason
            );
        }

        // Agent Run 完成（中断与超时续跑都不标记完成，保持可续跑语义）
        //
        // 这里传的 `req_*_tokens` 是**含子代理的总量**，只用于 `agent_runs` 的
        // 回合汇总展示（面板上“这一轮花了多少”）。它**不是账本**：子代理的用量
        // 已由它自己逐请求写入 `request_usage` 与 `sessions.total_*`，
        // 若再从这里累加一次，明细与累计就会对不平。
        if !was_cancelled && !was_loop_timeout && interrupted_reason.is_none() {
            agent_runs::complete_run(
                &self.app,
                &self.run_id,
                self.req_input_tokens,
                self.req_output_tokens,
                Some(self.final_answer.chars().take(180).collect()),
                // 本轮等待用户审批的净耗时（区间并集；取走即清空）
                self.ctx.take_run_wait_ms().await,
            );
        } else if interrupted_reason.is_some() {
            // 方案 6：主动收尾（取消/错误中止/看门狗）已在此前把 run 置为
            // Interrupted/Cancelled，且上方 save_session 已把半截内容与中断
            // 标记落库——收尾至此**声明完成**，盖 closed 图章把 run 移出
            // 恢复候选（门卫 find_interrupted_run 不捞 closed）。此前这类
            // run 停留 Interrupted，收尾后的正常刷新就会被门卫重放比对，
            // 因标记只在消息层而误判缺失、重复补同一段（2302e81c）。
            // 若 save 与盖章之间崩溃（毫秒级窗口），run 停留 Interrupted
            // 被门卫捞出，由 messages_equivalent 的剥标记比对兜底。
            //
            // `mark_run_closed` 内部对 wait_ms 做**累加**：上面两条收尾函数
            // （cancel_run / interrupt_run）已经把区间取走过一次（已落库），
            // 这里再 take 拿到的是空集，累加即无副作用；但若本路径**没有**
            // 先前的收尾函数（例如"达到回合上限且未获续跑授权"是直接 break 出循环），
            // 这里就是唯一一次取走区间的机会。
            let wait_ms = self.ctx.take_run_wait_ms().await;
            agent_runs::mark_run_closed(&self.app, &self.run_id, wait_ms);
        }

        let session_input_tokens = session_meta
            .as_ref()
            .map(|meta| meta.total_input_tokens)
            .unwrap_or(0);
        let session_output_tokens = session_meta
            .as_ref()
            .map(|meta| meta.total_output_tokens)
            .unwrap_or(0);
        // 会话级缓存累计：0 表示该会话从未有请求上报过缓存字段（展示层显示 --）
        let session_cache_hit_tokens = session_meta
            .as_ref()
            .map(|meta| meta.total_cache_hit_tokens)
            .unwrap_or(0);
        let session_cache_miss_tokens = session_meta
            .as_ref()
            .map(|meta| meta.total_cache_miss_tokens)
            .unwrap_or(0);

        {
            let mut active_run_id = self.ctx.active_run_id.lock().await;
            if active_run_id.as_deref() == Some(&self.run_id) {
                *active_run_id = None;
            }
        }
        self.execution_lease.finish();

        println!(
            "[JARVIS] finalize：status={}，notice={:?}，content 长度={}，子代理用量 in={} out={}（已逐请求入账，仅展示）",
            status,
            self.notice_i18n_key.unwrap_or("<无>"),
            self.final_answer.chars().count(),
            self.req_sub_input_tokens,
            self.req_sub_output_tokens
        );

        JarvisResult {
            status: status.to_string(),
            // content 只放模型正文/部分结果；状态标注走 notice 单独下发，
            // 避免标记混进正文被渲染到回复气泡内部（实测反馈的 UI 问题）。
            content: self.final_answer,
            input_tokens: self.req_input_tokens,
            output_tokens: self.req_output_tokens,
            session_input_tokens,
            session_output_tokens,
            session_cache_hit_tokens,
            session_cache_miss_tokens,
            user_message_id: self.user_message_id.clone(),
            // 本轮检查点信息随结果下发（字段缺省时 serde 不序列化，旧前端不受影响）：
            // - checkpoint_id：本轮快照 id，快照创建失败则为 None
            // - checkpoint_has_patches：本轮是否有文件补丁，决定前端回填 both 模式
            checkpoint_id,
            checkpoint_has_patches: Some(checkpoint_has_patches),
            tool_execution_summary: self.tool_execution_summary,
            notice_i18n_key: self.notice_i18n_key.map(|k| k.to_string()),
            notice_detail: self.notice_detail,
            thinking_enabled: Some(self.turn_thinking_decision.enabled),
            thinking_reason: Some(format!("{:?}", self.turn_thinking_decision.reason)),
            thinking_notice_i18n_key: self
                .turn_thinking_decision
                .notice_i18n_key
                .map(|s| s.to_string()),
        }
    }
}
