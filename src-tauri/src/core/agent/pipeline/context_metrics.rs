//! # pipeline/context_metrics.rs — 上下文压缩、历史快照、用量更新与 LLM 请求

use super::*;

impl PipelineState {
    /// 上下文压缩检查（项目内唯一的 LLM 摘要压缩，属单级）：超阈值时调 LLM 把旧历史
    /// 压缩成一段摘要；压缩前后保证用户最新消息仍在历史末尾，且后续轮次能正确计算
    /// initial_msg_index
    ///
    /// 判据算法统一在 `infra::llm::context_budget`（与子代理共用，见该模块文档）：
    /// - 阈值 =（模型窗口 − 输出预算）× `COMPACT_TRIGGER_PERCENT`（当前 85%）
    /// - 占用 = 本地估算 ×（上一轮实测 ÷ 上一轮估算）
    pub(super) async fn compact_if_needed(&mut self) {
        // 1. 估算当前上下文 token（消息 + 工具 schema）
        let (messages_for_estimate, sources_for_estimate, kinds_for_estimate) = {
            let session = self.ctx.memory.lock().await;
            (
                session.messages.clone(),
                session.sources.clone(),
                session.interrupt_kinds.clone(),
            )
        };
        let history_snapshot = self.prepare_history_snapshot_from_messages(
            messages_for_estimate,
            &sources_for_estimate,
            &kinds_for_estimate,
        );
        let tools = self.current_tools();
        let estimate = self.build_context_estimate(&history_snapshot, &tools);
        let est_now = estimate.estimated_tokens;

        // 2. 判据：分母用模型的真实窗口并预留输出预算；分子用上一轮的厂商实测值
        //    标定本地估算（估算系统性偏低，直接比会一路不触发）。
        let window = crate::infra::llm::context_budget::resolve_context_window(&self.model_id);
        let output_budget = crate::infra::llm::context_budget::resolve_output_budget(
            &self.model_id,
            self.cfg.max_tokens,
        );
        let trigger =
            crate::infra::llm::context_budget::compact_trigger_tokens(window, output_budget);
        // 上一轮快照里的「实测 input / 本地估算」是同一时刻写入的一对，可直接作标定基准。
        // 首轮（或快照缺失）为 None，此时退回纯估算。
        let (prev_measured, est_prev) = crate::core::session::get_context_snapshot(&self.sid)
            .ok()
            .flatten()
            .map(|snapshot| {
                (
                    snapshot.provider_input_tokens,
                    Some(snapshot.estimated_tokens),
                )
            })
            .unwrap_or((None, None));
        let tokens = crate::infra::llm::context_budget::calibrated_context_tokens(
            prev_measured,
            est_prev,
            est_now,
        );

        // >85% 可用窗口：LLM 摘要压缩
        if crate::infra::llm::context_budget::should_compact(tokens, trigger) {
            println!(
                "[贾维斯] 上下文占用 {} > {}% 可用窗口（窗口 {} − 输出预算 {} = {}, 本地估算 {}, 上轮实测 {:?}）, 触发 LLM 摘要压缩",
                tokens,
                crate::infra::llm::context_budget::COMPACT_TRIGGER_PERCENT,
                window,
                output_budget,
                window.saturating_sub(output_budget),
                est_now,
                prev_measured
            );

            let mut session = self.ctx.memory.lock().await;
            // 2. 压缩前先临时取出最后一条用户消息，压缩完成后再放回。
            //
            // ⚠️ 注意「最后一条是用户消息」并不等于「最后一条是本轮提问」：
            // 工具结果同样是 `Message::User`（Anthropic 约定）。所以这段 pop/restore
            // 单独用**保不住本轮提问** —— 真正保证本轮任务不被压掉的是下面的
            // 「本轮区间白名单」（`initial_msg_index` 之后整段不参与压缩）。
            // 它在这里的职责只剩一个：避免压缩后历史以 User 结尾（Anthropic 要求
            // 首条须为 User、且相邻同角色需合并，这里补 Assistant 垫片兜住）。
            let mut last_user_msg = None;
            if let Some(Message::User { .. }) = session.messages.last() {
                last_user_msg = pop_message(&mut session);
            }

            // 2b. 本轮区间白名单：`initial_msg_index` = 本轮用户消息在 messages 中的下标
            //     （见 pre_loop 的 inject_user_message）。它之后的所有消息都属于
            //     「本轮任务」——本轮提问、本轮 assistant 块、本轮工具结果、
            //     中途切换工作模式追加的 context 快照 —— 一律不参与压缩。
            //
            //     取**连续区间**而非挑选消息：Assistant(ToolUse) 与其后的
            //     User(ToolResult) 必须成对出现在请求里，切开会让 provider 直接 400。
            let turn_start = self.initial_msg_index.min(session.messages.len());
            let turn_messages: Vec<Message> = session.messages.drain(turn_start..).collect();
            let turn_sources: Vec<MessageSource> = if session.sources.len() >= turn_messages.len() {
                let keep = session.sources.len().saturating_sub(turn_messages.len());
                session.sources.drain(keep..).collect()
            } else {
                Vec::new()
            };
            // 中断类型与本轮区间同尺 drain —— 三个平行数组必须一起摘出来、一起拼回，
            // 否则压缩后 kind 会错配到别的消息上（拼回时按索引取，越界得 None）。
            let turn_kinds: Vec<Option<String>> =
                if session.interrupt_kinds.len() >= turn_messages.len() {
                    let keep = session
                        .interrupt_kinds
                        .len()
                        .saturating_sub(turn_messages.len());
                    session.interrupt_kinds.drain(keep..).collect()
                } else {
                    Vec::new()
                };
            let turn_len = turn_messages.len();

            // 2c. 本轮就是全部历史（`initial_msg_index` 指向第 0 条）⇒ 没有可压的前缀。
            //     此时**不切开本轮**，直接放弃本次压缩，等下一次用户提问重新划界。
            //     切开本轮正是要消除的问题：模型会重做已做过的动作。
            let prefix_len = session.messages.len();
            if prefix_len == 0 {
                // 放回本轮区间后直接返回。
                //（用 append_message_with_kind 而非直接 push：它会先 normalize_message_ids
                // 把 message_ids / sources / interrupt_kinds 与 messages 重新对齐 ——
                // drain 之后它们长度已不同步）
                for (i, (msg, src)) in turn_messages.into_iter().zip(turn_sources).enumerate() {
                    let kind = turn_kinds.get(i).cloned().flatten();
                    append_message_with_kind(&mut session, msg, src, kind.as_deref());
                }
                // 本轮起点 = 放回后「区间起点」的下标
                self.initial_msg_index = session.messages.len().saturating_sub(turn_len);
                if let Some((msg, message_id)) = last_user_msg {
                    restore_message(&mut session, msg, message_id, MessageSource::Chat);
                    self.initial_msg_index = session.messages.len().saturating_sub(1);
                }
                println!(
                    "[JARVIS] 本轮即为全部历史（{} 条），无可压缩前缀，跳过本次压缩 —— 本轮区间保持完整",
                    session.messages.len()
                );
                return;
            }

            // 压缩调用也要能被打断：auto_compact 是**非流式** LLM 调用（整体超时
            // 约 60 秒），旧实现里用户此时点停止，要等它返回才轮到取消检查。
            // 取消时复用下方 Err 分支的收尾路径：把本轮区间原样拼回（它已被
            // drain 出来，不能丢），由下一次循环开头的取消检查走正常中断收口。
            let compact_result = tokio::select! {
                biased;
                _ = self.cancel_token.cancelled() => Err(
                    crate::infra::types::error::MemoryError::CompactionFailed(
                        "用户取消".to_string(),
                    ),
                ),
                result = auto_compact(
                    &self.sid,
                    &mut session,
                    &self.client,
                    &self.api_key,
                    &self.base_url,
                    &self.cfg.utility_model,
                    self.api_format,
                ) => result,
            };

            // 无论压缩成功与否，本轮区间都必须原样拼回
            match compact_result {
                Err(e) => {
                    println!("[JARVIS] 自动压缩失败: {}，继续使用原始上下文", e);
                    for (i, (msg, src)) in turn_messages.into_iter().zip(turn_sources).enumerate() {
                        let kind = turn_kinds.get(i).cloned().flatten();
                        append_message_with_kind(&mut session, msg, src, kind.as_deref());
                    }
                    // 前缀未被压，本轮起点仍落在原前缀之后
                    self.initial_msg_index = prefix_len;
                }
                Ok(usage) => {
                    // 压缩成功：前缀已被压成「压缩请求 + 摘要」两条，
                    // 本轮区间原样拼回，本轮起点 = 压缩后的前缀长度。
                    //
                    // ⚠️ 本轮提问「原本就在区间里」（turn_start 就是它的下标），
                    // 所以这里**不能**再用 last_user_msg 恢复一次，否则会插入两条提问。
                    // last_user_msg 只用于「最后一条是工具结果」的情形（见下），
                    // 那种情况下它属于本轮区间，已被上面拼回。
                    let compacted_len = session.messages.len();
                    for (i, (msg, src)) in turn_messages.into_iter().zip(turn_sources).enumerate() {
                        let kind = turn_kinds.get(i).cloned().flatten();
                        append_message_with_kind(&mut session, msg, src, kind.as_deref());
                    }
                    self.initial_msg_index = compacted_len;
                    println!(
                        "[JARVIS] 压缩完成：前缀 {} 条 → {} 条，本轮区间 {} 条原样保留（本轮起点 = {}）",
                        prefix_len,
                        compacted_len,
                        session.messages.len() - compacted_len,
                        self.initial_msg_index
                    );

                    // 压缩请求本身也真实消耗 token，按 Compact 来源单独入账；
                    // 记账失败只记日志，不得影响压缩主流程。
                    let record = crate::core::session::RequestUsageRecord::new(
                        self.sid.clone(),
                        crate::core::session::UsageSource::Compact,
                    )
                    .run(self.run_id.clone(), Some((self.total_loop_count + 1) as u32))
                    .model_id(self.cfg.utility_model.clone());
                    if let Err(err) = crate::core::session::usage::record_llm_usage(
                        &self.app,
                        record,
                        usage.as_ref(),
                    ) {
                        eprintln!("[JARVIS] 写入压缩用量明细失败: {}", err);
                    }
                }
            }

            // 3. 兜底：历史不能以 User 结尾（补一条 Assistant 垫片）。
            //    正常路径下本轮区间末尾是 assistant 块或工具结果，这里只是防御。
            if let Some((msg, message_id)) = last_user_msg {
                // ⚠️ 去重：最后一条用户消息若已随本轮区间拼回（message_id 已存在），
                //    就不能再恢复一次 —— 否则历史里会出现两条同样的提问。
                let already_present = session
                    .message_ids
                    .iter()
                    .any(|existing| existing == &message_id);
                if already_present {
                    let needs_assistant_pad = matches!(
                        session.messages.last(),
                        Some(Message::User { .. }) | None
                    );
                    if needs_assistant_pad {
                        // `Placeholder`：这条是**对齐填充**（保持 user/assistant 交替），
                        // 既不该给模型看也不该给用户看 —— 它原先标 `internal` 是"巧合的正确"，
                        // 因为 `internal` 恰好两边都不可见；但它与上面三条"给模型看的指令"
                        // 共用同一个值，正是来源语义被压扁的证据。
                        append_message(
                            &mut session,
                            Message::Assistant {
                                content: Content::Single("Context compressed.".to_string()),
                            },
                            MessageSource::Placeholder,
                        );
                    }
                } else {
                    restore_message(&mut session, msg, message_id, MessageSource::Chat);
                    self.initial_msg_index = session.messages.len().saturating_sub(1);
                }
            }
        }
    }

    /// 获取当前会话可用的工具定义（核心工具集，再按本会话的工具开关过滤）。
    ///
    /// 用 `self.tool_filter`（setup 时取好的会话快照）而非现读配置：
    /// 这样整个会话内 `tools` 参数保持字节恒定，prompt cache 照常命中。
    pub(super) fn current_tools(&self) -> Vec<serde_json::Value> {
        get_tools_definition(&self.tool_filter)
    }

    /// 从完整消息列表生成“发给 LLM 的历史快照”（只读处理，不改原历史）：
    /// 1. 过滤 internal/background 内部消息
    /// 2. 折叠往轮图片、恢复本轮图片的 base64（动态上下文已随消息落库，不再另行注入）
    /// 3. 修复残缺的 Assistant(tool_calls) → ToolResult 配对（防 API 400）
    ///
    /// 这里不改写工具结果内容：任何对「已经发出去过的前缀」的改写都会让 provider 的
    /// prompt cache 整体失效，比省下的 token 贵得多。
    pub(super) fn prepare_history_snapshot_from_messages(
        &self,
        messages: Vec<Message>,
        sources: &[MessageSource],
        interrupt_kinds: &[Option<String>],
    ) -> Vec<Message> {
        // 步骤 1：过滤不进模型上下文的消息，并把「本轮用户消息」的下标换算到
        // 过滤后的快照坐标系——initial_msg_index 是过滤前的下标，而图片折叠要的是过滤后的下标。
        //
        // 判据取 `MessageSource::in_llm_context()`，不再维护字符串白名单：
        // 曾经这里是 `chat | compact | context | interrupted` 四个字面量，而
        // `Inject`（当时的 `internal`）不在其中 —— 崩溃恢复指令、方案重定向通知、
        // 反思修正建议三条**写给模型的指令**因此从未送达模型（见
        // `doc/消息来源与轮次元数据重构方案.md` §2.2）。现在"哪些来源进上下文"
        // 由枚举一处说了算，新增取值时编译器强制表态。
        //
        // `Interrupted` 在其中是有意的：运行被打断（取消/失联/报错）后，模型需要
        // 看到自己中断于何处，用户回一句"继续"才能顺着接上。半截正文出现在
        // 上下文里是预期行为，因为它真实反映了中断时的状态。
        let session_turn_start = self.initial_msg_index;
        let mut filtered: Vec<Message> = Vec::with_capacity(messages.len());
        let mut snapshot_turn_start: Option<usize> = None;
        for (idx, (mut msg, src)) in messages.into_iter().zip(sources.iter()).enumerate() {
            if !src.in_llm_context() {
                continue;
            }
            // 中断类型 → 把标记拼回正文末尾（**模型必须看到"上一句被截断"**，
            // 否则续跑会重复或断片）。存储侧正文是干净的，标记只在发送前出现。
            if let Some(kind) = interrupt_kinds
                .get(idx)
                .and_then(|k| k.as_deref())
                .and_then(InterruptKind::from_db)
            {
                append_interrupt_marker_to_message(&mut msg, interrupt_marker_for(kind));
            }
            if idx == session_turn_start {
                snapshot_turn_start = Some(filtered.len());
            }
            filtered.push(msg);
        }
        // 兜底：没命中就把所有图片当往轮处理（不会 panic，也不会把 base64 全量发出去）
        let current_turn_start = snapshot_turn_start.unwrap_or(filtered.len());

        let mut history_snapshot = filtered;

        // 防御性检查：确保每个 Assistant(tool_calls) 后跟 ToolResult
        // 修复流式中断、并发修改等边缘 case 导致的消息序列断裂
        fix_broken_tool_call_pairs(&mut history_snapshot);

        // 步骤 2：折叠往轮图片、恢复本轮图片。动态上下文已随消息落库，这里不再注入
        restore_image_data(&mut history_snapshot, current_turn_start);

        history_snapshot
    }

    /// 准备历史消息快照（含图片恢复 + 上下文注入）
    pub(super) async fn prepare_history_snapshot(&self) -> Vec<Message> {
        let session = self.ctx.memory.lock().await;
        let messages = session.messages.clone();
        let sources = session.sources.clone();
        let interrupt_kinds = session.interrupt_kinds.clone();
        drop(session); // 释放锁
        self.prepare_history_snapshot_from_messages(messages, &sources, &interrupt_kinds)
    }

    /// 估算一次请求的上下文用量（消息 + 工具 schema，按分区统计），
    /// 结果用于前端上下文监控展示与压缩触发判断；
    /// 每个 section 记录独立字符数/token 数，方便 UI 定位占比。
    ///
    /// **口径：按实际发出去的 payload 计数。** 这里刻意**不**调用
    /// `adapters::strip_context_blocks`——出网路径
    /// （`adapters::materialize_context_blocks_for_wire`）会把每条用户消息里的
    /// `<context>` 动态上下文块翻译成普通 Text 一并发给模型，它真实占用 prompt token。
    /// 早先版本先 strip 再计数，把这部分整块漏算，实测长会话里估算 17.3k vs 实际 52.0k，
    /// 连带把压缩触发阈值拖到 3 倍之后。`Session Messages` 的 raw JSON 里因此会出现
    /// `"type": "context"` 块——那是内部表示，出网时会被翻译，不是协议字段。
    pub(super) fn build_context_estimate(
        &self,
        history_snapshot: &[Message],
        tools: &[serde_json::Value],
    ) -> ContextEstimate {
        fn section(
            model_id: &str,
            key: &str,
            label: &str,
            content: String,
            item_count: usize,
        ) -> ContextSectionSnapshot {
            let chars = content.chars().count();
            let token_count = crate::infra::llm::token_count::count_text(model_id, &content);
            ContextSectionSnapshot {
                key: key.to_string(),
                label: label.to_string(),
                chars,
                estimated_tokens: token_count.tokens,
                token_count_method: token_count.method.as_str().to_string(),
                item_count,
                content,
                truncated: false,
                raw_content: None,
            }
        }

        /// 计数口径与展示内容分离的版本：`content` 仍用可读文本（面板展示用），
        /// 但 `chars` / `estimated_tokens` 按 `count_text` 计算（真实 payload 口径）。
        ///
        /// 2026-09-17 起 `messages` 分区改用这个：`context_format::format_messages_readable`
        /// 产出的是可读文本（现已不截断，但仍是“给人看”的排版），而真实发出去的是完整 JSON
        /// —— 两者字段、转义形式不同，拿可读文本计数会系统性低估
        /// （长会话实测 17.3k vs 实际 52.0k，约 3 倍），压缩该触发时不触发。
        fn section_with_raw_counting_full(
            model_id: &str,
            key: &str,
            label: &str,
            display_content: String,
            counting_text: &str,
            item_count: usize,
            raw: String,
        ) -> ContextSectionSnapshot {
            let chars = counting_text.chars().count();
            let token_count = crate::infra::llm::token_count::count_text(model_id, counting_text);
            ContextSectionSnapshot {
                key: key.to_string(),
                label: label.to_string(),
                chars,
                estimated_tokens: token_count.tokens,
                token_count_method: token_count.method.as_str().to_string(),
                item_count,
                // 面板展开看的是可读文本；计数走 counting_text
                content: display_content,
                truncated: false,
                raw_content: Some(raw),
            }
        }

        /// 统计各类内容块的出现次数，返回 `(tool_use, tool_result, image)`。
        ///
        /// 只统计这三类：它们各自是前端要单独展示的计数。thinking 块不再计数——
        /// 它原来唯一的消费方是被删除的 `runtime` 分区（那个分区不占独立体积，
        /// thinking 内容本身已经计入 messages），留着就是一个恒被丢弃的值。
        fn count_blocks(messages: &[Message]) -> (usize, usize, usize) {
            let mut tool_calls = 0;
            let mut tool_results = 0;
            let mut images = 0;
            for message in messages {
                let Content::Multiple(blocks) = (match message {
                    Message::User { content } | Message::Assistant { content } => content,
                }) else {
                    continue;
                };
                for block in blocks {
                    match block {
                        ContentBlock::ToolUse { .. } => tool_calls += 1,
                        ContentBlock::ToolResult { .. } => tool_results += 1,
                        ContentBlock::Image { .. } => images += 1,
                        // Text / Context / Thinking 都不计数，但必须穷尽列出：
                        // 用 `_ => {}` 会让将来新增的块类型被静默漏计。
                        ContentBlock::Text { .. } => {}
                        ContentBlock::Context { .. } => {}
                        ContentBlock::Thinking { .. } => {}
                    }
                }
            }
            (tool_calls, tool_results, images)
        }

        // 直接数**未 strip** 的 history：Context 块会被翻译成 Text 发出去，必须计入。
        let (tool_call_count, tool_result_count, image_count) = count_blocks(history_snapshot);
        let messages_text = crate::core::agent::context_format::format_messages_readable(history_snapshot);
        let messages_json = serde_json::to_string_pretty(history_snapshot).unwrap_or_default();
        let tools_json = serde_json::to_string_pretty(tools).unwrap_or_default();
        // 分区固定为 system → messages → tools 三项，顺序不乱。
        // 不再有 push，故无需 mut（曾因 attachments / runtime 追加分区而需要）。
        let sections = vec![
            section(
                &self.model_id,
                "system",
                "System Prompt",
                self.system_prompt.clone(),
                1,
            ),
            // 这里曾有一个独立的 `dynamic` 分区（`self.dynamic_context_str`）。
            // 它已经**不再需要**：动态上下文由 `pre_loop` 经 `inject_user_message`
            // 写进本轮用户消息的 `<context>` 块，随 history 一起落库；
            // 上面的 `messages` 分区现在按未 strip 的 history 计数，已经包含它。
            // 保留独立分区等于把本轮动态上下文重复计一次，总量会虚高。
            // 需要看它有多大时，展开 `messages` 分区找 `[Context]` 段即可。
            // ⚠️ 计数口径（2026-09-17 修）：`context_format::format_messages_readable`
            // 产出的是**给人看**的可读排版（现已取消截断，但仍不是协议字段的严格 JSON：
            // 枚举变体名、转义形式、字段集合都不一样）。真实发给厂商的 payload 是完整 JSON，
            // 用可读文本计数会系统性低估（长会话实测 17.3k vs 实际 52.0k，约 3 倍），
            // 导致压缩该触发时不触发。
            // 故：`content`（面板展示）继续用可读文本，`chars` / `estimated_tokens`
            // （判据消费）改用完整 JSON。
            section_with_raw_counting_full(
                &self.model_id,
                "messages",
                "Session Messages",
                messages_text,
                &messages_json,
                history_snapshot.len(),
                messages_json.clone(),
            ),
            // tools 分区用 `section` 而不是 `section_with_raw`：它的 `content`
            // 与 `raw` 本来就是同一份 pretty JSON，存两遍没有意义（只会让快照
            // JSON 体积翻倍）。前端 copy 已兼容 raw 缺失，回落到 content。
            section(
                &self.model_id,
                "tools",
                "Tools Schema",
                tools_json,
                tools.len(),
            ),
        ];
        // 这里曾有 `attachments`（图片）与 `runtime`（tool_result / thinking）两个分区，
        // 已删除。理由是它们**不是体积来源**，而是对 `messages` 内容的重复统计：
        // 图片块、tool_result、thinking 本来就在 messages 的 JSON 里，
        // 再各开一段只会把同一份内容计入两次，让分区之和大于真实 payload。
        // 于是「分区之和 = 总量」这条不变式被破坏，压缩判据（读 total）与
        // 面板逐项对照（读 sections）互相矛盾。
        // 图片数改由顶层 `image_count` 字段承担，tool_result 数同理
        // （`tool_result_count`），二者都是计数而非尺寸，不参与合计。

        let total_chars = sections.iter().map(|item| item.chars).sum();
        let estimated_tokens = sections.iter().map(|item| item.estimated_tokens).sum();
        ContextEstimate {
            total_chars,
            estimated_tokens,
            message_count: history_snapshot.len(),
            tool_schema_count: tools.len(),
            tool_call_count,
            tool_result_count,
            image_count,
            sections,
        }
    }

    /// 更新本轮请求的上下文 token 快照
    pub(super) fn update_context_snapshot(&self, history_snapshot: &[Message], tools: &[serde_json::Value]) {
        fn now_ms() -> u64 {
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap_or_default()
                .as_millis() as u64
        }

        let estimate = self.build_context_estimate(history_snapshot, tools);
        // 沿用上一次快照里的缓存口径与逐 loop 趋势：
        // - 趋势不能每次重算快照就清零；
        // - 缓存数值也一并沿用（它描述的是"这条前缀最近一次观测到的命中情况"），
        //   否则每轮请求构建期间 UI 都会闪回「?」——厂商在请求进行中并不会重新表态。
        let previous_cache = crate::core::session::get_context_snapshot(&self.sid)
            .ok()
            .flatten()
            .map(|previous| {
                (
                    previous.cache_hit_tokens,
                    previous.cache_miss_tokens,
                    previous.cache_source,
                    previous.cache_history,
                )
            })
            .unwrap_or((None, None, None, Vec::new()));
        let (prev_cache_hit, prev_cache_miss, prev_cache_source, cache_history) = previous_cache;
        let snapshot = SessionContextSnapshot {
            session_id: self.sid.clone(),
            run_id: Some(self.run_id.clone()),
            loop_count: self.total_loop_count + 1,
            model: self.model_id.clone(),
            intent: self.detected_intent.clone(),
            api_format: self.api_format.as_str().to_string(),
            created_at: now_ms(),
            total_chars: estimate.total_chars,
            estimated_tokens: estimate.estimated_tokens,
            provider_input_tokens: None,
            provider_output_tokens: None,
            provider_total_tokens: None,
            cache_hit_tokens: prev_cache_hit,
            cache_miss_tokens: prev_cache_miss,
            cache_source: prev_cache_source,
            cache_history,
            drift_percent: None,
            max_context_tokens: crate::infra::llm::registry::query_capabilities(&self.model_id)
                .and_then(|capabilities| capabilities.max_context_tokens),
            // 必须与请求体里的 `max_tokens` **同源**（`resolve_max_tokens()`）。原先写死
            // `MAX_TOKENS_CONTEXT`(=8192)，导致快照里给用户看的输出上限与真正发出去的不是一个数；
            // 自动压缩判据（`context_budget`）的输出预算也是同一个函数，三处从此对齐。
            max_output_tokens: self.resolve_max_tokens(),
            message_count: estimate.message_count,
            tool_schema_count: estimate.tool_schema_count,
            tool_call_count: estimate.tool_call_count,
            tool_result_count: estimate.tool_result_count,
            image_count: estimate.image_count,
            sections: estimate.sections,
        };

        if let Err(err) = crate::core::session::save_context_snapshot(&snapshot) {
            eprintln!("[JARVIS] 保存上下文快照失败: {}", err);
        }
        let _ = self.app.emit("context-snapshot-updated", &snapshot);
    }

    pub(super) fn update_provider_usage_snapshot(
        &self,
        input_tokens: u64,
        output_tokens: u64,
        cache_hit_tokens: Option<u64>,
        cache_miss_tokens: Option<u64>,
        cache_source: Option<&str>,
        cache_point: Option<&CacheHitPoint>,
    ) {
        let total_tokens = input_tokens.saturating_add(output_tokens);
        let drift = crate::core::session::get_context_snapshot(&self.sid)
            .ok()
            .flatten()
            .and_then(|snapshot| {
                crate::infra::llm::token_count::drift_percent(
                    snapshot.estimated_tokens,
                    input_tokens,
                )
            });

        match crate::core::session::update_context_snapshot_usage(
            &self.sid,
            input_tokens,
            output_tokens,
            total_tokens,
            drift,
            cache_hit_tokens,
            cache_miss_tokens,
            cache_source,
            cache_point,
        ) {
            Ok(Some(snapshot)) => {
                let _ = self.app.emit("context-snapshot-updated", &snapshot);
            }
            Ok(None) => {}
            Err(err) => eprintln!("[JARVIS] 更新上下文 usage 失败: {}", err),
        }

        // 请求级入账：走**唯一写入口** `session::usage::record_request_usage`。
        //
        // 为什么不再手写增量：一次请求的用量有两个落点，而且必须同时改 ——
        // `request_usage`（明细，可按来源/模型/run 拆账）与 `sessions.total_*`
        // （累计，前端概览栏直接读）。让两侧各写一遍，"明细加起来 ≠ 累计值"
        // 这种账会在没有任何报错的情况下慢慢歪掉，只能靠人肉对数字才发现。
        // 现在两者共用一个事务、一个事件推送（`record_request_usage` 内部发），
        // 本函数只管把"本次请求事实上花了多少"如实交给它。
        //
        // 缓存字段的语义要守住：`None` 表示这家没报告（明细列落 NULL），按 0 累加
        // （不回退成 `input - hit` 之类的推导，各家 `input_tokens` 口径不同，
        // 见 `SessionMeta` 的注释）。这里的 `cache_*` 已由 `usage_memory` 的端点能力
        // 记忆解释过（字段缺失但该端点确认会上报 ⇒ 按 0 命中），推断逻辑本身不动。
        let record = crate::core::session::RequestUsageRecord::new(
            self.sid.clone(),
            crate::core::session::UsageSource::Main,
        )
        // loop_index 用 1 基序号，与日志/事件里的 `loopCount` 同一口径
        .run(self.run_id.clone(), Some((self.total_loop_count + 1) as u32))
        .model_id(self.model_id.clone())
        .tokens(input_tokens, output_tokens)
        .cache(cache_hit_tokens, cache_miss_tokens, cache_source);

        if let Err(err) =
            crate::core::session::usage::record_request_usage(&self.app, record)
        {
            eprintln!("[JARVIS] 写入请求用量明细失败: {}", err);
        }
    }

    /// 解析 max_tokens：用户覆盖 > 模型注册表 > 常量兜底
    ///
    /// 它与上下文压缩判据里的「输出预算」是**同一个数**（都走
    /// `infra::llm::context_budget::resolve_output_budget`）——给模型留出的输出
    /// 空间，必须和真正发出去的 `max_tokens` 对得上。
    pub(super) fn resolve_max_tokens(&self) -> i32 {
        crate::infra::llm::context_budget::resolve_output_budget(
            &self.model_id,
            self.cfg.max_tokens,
        ) as i32
    }

    /// 构建 LLM API 请求体（内部统一按 Anthropic 结构建模）
    ///
    /// 本函数只负责**把 pipeline 状态组装成 `LlmRequestInput`**；协议适配逻辑在
    /// `infra::llm::request_builder::build_request_body`——那属于基建层职责，
    /// 留在编排层会让 core 知道协议细节。
    ///
    /// - 返回值第二项 ApiFormat 交给 stream.rs 与 UsageObservation
    pub(super) fn build_llm_request(
        &self,
        history_snapshot: Vec<Message>,
    ) -> (serde_json::Value, crate::infra::llm::api_format::ApiFormat) {
        // 1. 取工具定义，并更新上下文监控快照。
        // 更新快照是有副作用的动作，刻意留在 pipeline、不进 `LlmRequestInput`：
        // 构建函数不该有副作用。
        let tools = self.current_tools();
        self.update_context_snapshot(&history_snapshot, &tools);

        // 2. 组装输入。这里只放「已经查明的事实」；模型相关的决策（采样参数是否
        // 真正下发、思考参数怎么写、思考是否真的开着）由 request_builder 查注册表
        // 自行完成，不在编排层预判。
        // system 在 setup 阶段只组装一次并保持字节恒定，整个会话内不再随 work_mode 变化。
        let input = crate::infra::llm::request_builder::LlmRequestInput {
            model_id: self.model_id.clone(),
            base_url: self.cfg.base_url.clone(),
            api_format: self.api_format,
            system_prompt: self.system_prompt.clone(),
            messages: history_snapshot,
            tools,
            max_tokens: self.resolve_max_tokens(),
            should_think: self.should_think,
            temperature: self.cfg.temperature,
            top_p: self.cfg.top_p,
            top_k: self.cfg.top_k,
        };

        // 3. 交给基建层构建（纯函数，无 IO、不改状态）
        crate::infra::llm::request_builder::build_request_body(input)
    }

    /// 调用 LLM API（含取消检查）
    /// 返回 None 表示请求被取消，调用方应 continue 到下一轮循环
    pub(super) async fn call_api_with_retry(
        &self,
        req_json: &serde_json::Value,
    ) -> Result<Option<reqwest::Response>, AgentError> {
        let api_request = api_client::api_call_with_retry(
            &self.client,
            &self.base_url,
            req_json,
            &self.api_key,
            self.api_format,
            api_client::MAX_API_RETRIES,
            &self.app,
            &self.sid,
        );

        match tokio::select! {
            result = tokio::time::timeout(
                Duration::from_secs(
                    crate::infra::types::constants::API_RESPONSE_HEADER_TIMEOUT_SECS,
                ),
                api_request,
            ) => {
                match result {
                    Ok(result) => result,
                    Err(_) => {
                        let error = ApiError::Network(format!(
                            "API 请求超过 {} 秒未返回响应头，已自动终止。",
                            crate::infra::types::constants::API_RESPONSE_HEADER_TIMEOUT_SECS
                        ));
                        // 本轮标记为 interrupted（保留帧级通道已写的半截内容）
                        self.mark_loop_event_interrupted(error.to_string()).await;
                        agent_runs::fail_run(
                            &self.app,
                            &self.run_id,
                            self.req_input_tokens,
                            self.req_output_tokens,
                            // 等待用户审批的净耗时（区间并集；取走即清空）
                            self.ctx.take_run_wait_ms().await,
                        );
                        return Err(error.into());
                    }
                }
            }
            _ = self.cancel_token.cancelled() => {
                return Ok(None);
            }
        } {
            Ok(resp) => Ok(Some(resp)),
            Err(e) => {
                // 本轮标记为 interrupted（保留帧级通道已写的半截内容）
                self.mark_loop_event_interrupted(e.to_string()).await;
                agent_runs::fail_run(
                    &self.app,
                    &self.run_id,
                    self.req_input_tokens,
                    self.req_output_tokens,
                    // 等待用户审批的净耗时（区间并集；取走即清空）
                    self.ctx.take_run_wait_ms().await,
                );
                Err(e.into())
            }
        }
    }

    /// 存储助手回复到会话历史（过滤空文本/空思考块，工具块原样保留）
    pub(super) async fn store_assistant_response(&self, current_blocks: &[ContentBlock]) {
        let mut session = self.ctx.memory.lock().await;
        // 过滤空文本/空思考块，仅保留有效内容
        let filtered_blocks: Vec<ContentBlock> = current_blocks
            .iter()
            .filter(|block| match block {
                ContentBlock::Text { text } => !text.trim().is_empty(),
                ContentBlock::Thinking { thinking, .. } => !thinking.trim().is_empty(),
                ContentBlock::ToolUse { .. }
                | ContentBlock::ToolResult { .. }
                | ContentBlock::Image { .. }
                | ContentBlock::Context { .. } => true,
            })
            .cloned()
            .collect();
        // 写入会话历史（chat 来源，下一轮请求会发给 LLM）
        if !filtered_blocks.is_empty() {
            append_message(
                &mut session,
                Message::Assistant {
                    content: Content::Multiple(filtered_blocks),
                },
                MessageSource::Chat,
            );
        }
    }

    /// B3 Plan 看门狗：按轮推进计数并判断是否触发（仅 plan 模式）。
    ///
    /// 喂狗动作（计数清零）：
    /// - 本 loop 提交了规划方案（`loop_submitted_plan`：裸 ProposePlan 或
    ///   经 ExecuteTool 包装——只匹配裸名时，包装形态的提交轮不喂狗，看门狗误触发）；
    /// - 本 loop 已把 work_mode 切出 plan（SwitchWorkMode 到 edit）。
    ///
    /// **调用位置要求**：必须在"有工具"和"无工具"两条路径上都能到达。
    /// 旧实现只挂在工具执行分支，而"纯文本空转"走的是无工具分支，
    /// 计数器永远停在 0、看门狗结构性失效（2026-09-19 死循环事故根因）。
    pub(super) fn update_plan_watchdog(&mut self, work_mode: &str, tool_calls: &[(String, String)]) -> bool {
        advance_plan_watchdog(
            work_mode,
            loop_submitted_plan(tool_calls),
            &mut self.plan_total_loops_without_plan,
        )
    }

    /// B3 触发后：先复用当前 system + 历史做一次缓存友好的 LLM 进度小结，
    /// 再把决策权交还用户并强制结束当前 loop。
    pub(super) async fn handle_plan_watchdog_summary(&mut self) {
        use crate::infra::types::constants::PLAN_WATCHDOG_MAX_LOOPS_WITHOUT_PLAN;

        // 触发指令带实际轮数：触发时计数尚未清零（触发分支直接 break），
        // 把真实值与阈值一起告诉模型，让它知道已经连续探索了多少轮。
        let instruction = format!(
            "【系统通知】规划探索已连续 {} 轮未提交方案（阈值 {} 轮），已到上限。请立即停止探索，不要调用任何工具。思考可以照常进行，但必须用你自己的话在可见正文中总结：当前已完成什么、卡在哪里、下一步建议（继续探索 / 缩小范围 / 直接执行）。只有思考而没有正文，用户将看不到本轮结论。",
            self.plan_total_loops_without_plan,
            PLAN_WATCHDOG_MAX_LOOPS_WITHOUT_PLAN,
        );
        let mut snapshot = self.prepare_history_snapshot().await;
        snapshot.push(Message::User {
            content: Content::Single(instruction),
        });

        let (req_json, req_api_format) = self.build_llm_request(snapshot);
        let mut summary = String::new();
        match self.call_api_with_retry(&req_json).await {
            Ok(Some(resp)) => {
                let mut stream = resp.bytes_stream().eventsource();
                let parsed = process_stream(
                    &mut stream,
                    req_api_format,
                    &self.app,
                    &self.sid,
                    &self.run_id,
                    self.total_loop_count + 1,
                    &self.cancel_token,
                    StreamConfig {
                        is_subagent: false,
                        cache_usage_style: crate::infra::llm::registry::cache_usage_style_for(
                            &self.model_id,
                        ),
                        // 看门狗小结是短请求，不需要等待提示
                        on_frame: None,
                        model_id: Some(self.model_id.clone()),
                        // 小结是内部一次性的短请求，没有"崩溃后要续跑"的语义
                        crash_protection: false,
                    },
                )
                .await;
                summary = parsed.text.trim().to_string();
            }
            Ok(None) => {}
            Err(e) => {
                println!("[JARVIS] Plan 看门狗小结调用失败: {}", e);
            }
        }

        if summary.is_empty() {
            summary = "本次规划探索尚未收敛，已自动停止。请选择：继续探索 / 缩小范围 / 直接执行。"
                .to_string();
        }

        self.final_answer = summary.clone();
        // 收尾小结**既不写** `tool_execution_summary`，**也不发** `chat-stream`。
        // 两者都是前端的「执行过程」通道，只是入口不同：前者进 fallback 日志行，
        // 后者直接 append 成 `turn.logs`（渲染为「输出 #N · [标签] 摘要」）。
        //
        // 代价（实测反馈，别再补回来）：只要正文之后多出这样一条过程行，
        // 前端 `AgentTurn.timelineSplit` 的「时间线末尾必须是正文」判据就不再成立，
        // 整段小结会连同工具调用一起被折回执行过程，最终回答区反而空着。
        //
        // 这条信息只有一个承载者：`summary` 本身就是正文，
        // 实时经 `chat-text` 推给前端，落库为 assistant 消息。
        //
        // 刻意**不**打 `InterruptKind::PlanLimit`：那会在气泡下方额外渲染一行
        // 「规划探索已达到阈值」，与模型自己的收尾重复，且把系统判定摆到模型判断前面。
        // 用户需要看到的是模型用自己的话说清现状，不是系统的判定标签。

        // 小结落库：此前看门狗收尾只进 final_answer 与事件流，session_messages
        // 零痕迹（尾部悬 tool_result，刷新后聊天流回退到上一条正文）。
        // 这里把小结补成 assistant 消息并立即整仓落库，尾部以 assistant 收口。
        //
        // 不带中断类型：看门狗收尾不按「中断」呈现，不给用户额外的系统提示。
        // source 保持 `chat`——这条消息**会发给 LLM**，作为下一轮可续写的上下文。
        {
            let mut session = self.ctx.memory.lock().await;
            if !assistant_text_exists_at_tail(&session.messages, &summary) {
                append_message(
                    &mut session,
                    Message::Assistant {
                        content: Content::Single(summary.clone()),
                    },
                    MessageSource::Chat,
                );
            }
            crate::core::session::save_session(&self.sid, &session, None);
        }
    }

    /// 中途模式切换：生成一条 seq 更大的完整上下文快照，并入本 loop 的
    /// tool_result 块数组（尾部追加，不回改旧前缀、不独立成条）。
    ///
    /// 旧形态（独立 user/context 消息）会在 session_messages 里产生
    /// user→user 相邻，破坏消息级 user/assistant 严格交替；并入后快照随
    /// 工具结果消息落库，序列保持 assistant(tool_use) → user(tool_result+快照)
    /// → assistant 的标准工具循环形态。缓存安全性不变：tool_result 消息
    /// 是本轮新产生、从未入缓存前缀，加块不影响已缓存前缀。
    /// 落库由调用侧在 append 后立即执行（防 seq 回退/重复的动机不变）。
    pub(super) async fn merge_mode_snapshot_into_blocks(
        &mut self,
        blocks: &mut Vec<ContentBlock>,
        mode: &str,
    ) {
        let seq = {
            let mut session = self.ctx.memory.lock().await;
            session.snapshot_seq = session.snapshot_seq.saturating_add(1);
            session.snapshot_seq
        };

        let capabilities = crate::core::tools::framework::capabilities::Capabilities::for_work_mode(
            mode,
            &self.tool_filter,
        );
        let snapshot = build_dynamic_context(
            &self.detected_intent,
            &self.request_workspace,
            &capabilities,
            mode,
            seq,
        );
        self.capabilities = capabilities;

        if snapshot.is_empty() {
            return;
        }

        self.dynamic_context_str = snapshot.clone();
        blocks.push(ContentBlock::Context { text: snapshot });
    }
}
