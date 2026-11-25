//! # pipeline/tests.rs — Agent 管线单元测试

use super::*;

#[cfg(test)]
mod plan_watchdog_feeding_tests {
    use super::loop_submitted_plan;

    fn call(name: &str, input: &str) -> (String, String) {
        (name.to_string(), input.to_string())
    }

    /// 裸 ProposePlan 喂狗（既有行为，回归防护）
    #[test]
    fn bare_propose_plan_feeds_the_watchdog() {
        let calls = vec![call("ProposePlan", "{}")];
        assert!(loop_submitted_plan(&calls));
    }

    /// ExecuteTool 包装的 ProposePlan 必须喂狗——模型提交方案的固定形态，
    /// 旧实现只匹配裸名，导致提交轮不喂狗、看门狗误触发截胡收尾（B 事故路径）
    #[test]
    fn execute_tool_wrapped_propose_plan_feeds_the_watchdog() {
        let calls = vec![call(
            "ExecuteTool",
            r#"{"name":"ProposePlan","args":{"title":"方案","content":"..."}}"#,
        )];
        assert!(loop_submitted_plan(&calls));
    }

    /// ExecuteTool 包装的其他按需工具不算提交方案，照常计空转
    #[test]
    fn execute_tool_wrapped_other_tools_do_not_feed() {
        let calls = vec![call(
            "ExecuteTool",
            r#"{"name":"ReadFile","args":{"path":"src/main.rs"}}"#,
        )];
        assert!(!loop_submitted_plan(&calls));
    }

    /// 普通核心工具不算提交方案
    #[test]
    fn core_tools_do_not_feed() {
        let calls = vec![call("ListTasks", "{}"), call("ReadFile", "{}")];
        assert!(!loop_submitted_plan(&calls));
    }

    /// 参数 JSON 坏了不能 panic，按"未提交"处理
    #[test]
    fn malformed_execute_tool_input_does_not_panic() {
        let calls = vec![call("ExecuteTool", "{not-json")];
        assert!(!loop_submitted_plan(&calls));
    }

    /// 同轮混合调用：一个 ExecuteTool(ProposePlan) 就算提交（喂狗是 any 语义）
    #[test]
    fn mixed_calls_with_one_submit_feed() {
        let calls = vec![
            call("ExecuteTool", r#"{"name":"SearchWorkspace","args":{}}"#),
            call("ExecuteTool", r#"{"name":"ProposePlan","args":{}}"#),
        ];
        assert!(loop_submitted_plan(&calls));
    }
}

#[cfg(test)]
mod plan_watchdog_scoring_tests {
    use super::advance_plan_watchdog;
    use crate::infra::types::constants::PLAN_WATCHDOG_MAX_LOOPS_WITHOUT_PLAN;

    /// 计分按轮：第 20 轮才触发，前 19 轮都不打扰。
    ///
    /// 旧口径按工具调用次数累加，一轮里并行读 6 个文件就会触发；
    /// 这里锁死新口径，防止改回去。
    #[test]
    fn triggers_on_the_last_allowed_round() {
        let mut loops = 0;
        for _ in 0..PLAN_WATCHDOG_MAX_LOOPS_WITHOUT_PLAN - 1 {
            assert!(!advance_plan_watchdog("plan", false, &mut loops));
        }
        assert_eq!(loops, PLAN_WATCHDOG_MAX_LOOPS_WITHOUT_PLAN - 1);
        assert!(advance_plan_watchdog("plan", false, &mut loops));
        assert_eq!(loops, PLAN_WATCHDOG_MAX_LOOPS_WITHOUT_PLAN);
    }

    #[test]
    fn submitting_plan_resets_counter() {
        let mut loops = 10;
        assert!(!advance_plan_watchdog("plan", true, &mut loops));
        assert_eq!(loops, 0);
    }

    #[test]
    fn leaving_plan_mode_resets_counter() {
        let mut loops = 10;
        assert!(!advance_plan_watchdog("edit", false, &mut loops));
        assert_eq!(loops, 0);
    }
}

#[cfg(test)]
mod thinking_freeze_tests {
    use super::should_think_for_loop;

    /// 回归：整轮每个 loop 的 thinking 状态必须恒定不变。
    ///
    /// 旧实现在 `loop_count > 0` 时回落到 audience 默认，于是第二轮变 enabled，
    /// 而历史里那条 assistant 是在关闭 thinking 时产生的、没有 thinking 块可回传，
    /// Anthropic 协议（含 DeepSeek 的 anthropic 兼容端点）会直接 400：
    /// `content[].thinking in the thinking mode must be passed back to the API`。
    ///
    /// v14 起"整轮的初始值"由 `thinking::decide` 一次裁决给出，本函数只负责守住"不翻转"。
    #[test]
    fn thinking_state_is_frozen_within_a_turn() {
        let cases = [
            crate::core::session::thinking::ThinkingMode::ON,
            crate::core::session::thinking::ThinkingMode::OFF,
        ];
        for mode in cases {
            let turn_think = mode.0;
            for loop_count in 0..=5 {
                assert_eq!(
                    should_think_for_loop(turn_think, loop_count),
                    turn_think,
                    "loop {} 不得把 thinking 翻转",
                    loop_count
                );
            }
        }
    }
}

#[cfg(test)]
mod interrupted_tail_dedup_tests {
    use super::assistant_text_exists_at_tail;
    use crate::infra::types::models::*;

    fn assistant_single(text: &str) -> Message {
        Message::Assistant {
            content: Content::Single(text.to_string()),
        }
    }

    fn assistant_blocks(blocks: Vec<ContentBlock>) -> Message {
        Message::Assistant {
            content: Content::Multiple(blocks),
        }
    }

    /// 中断收尾时若半截内容已由 store_assistant_response 落库，不得重复写入
    #[test]
    fn detects_existing_tail_text() {
        let messages = vec![assistant_single("沐先生，很")];
        assert!(assistant_text_exists_at_tail(&messages, "沐先生，很"));
    }

    /// 前后空白不应影响判定，否则同一段话会被写两遍
    #[test]
    fn ignores_surrounding_whitespace() {
        let messages = vec![assistant_single("  沐先生，很  ")];
        assert!(assistant_text_exists_at_tail(&messages, "沐先生，很"));
    }

    /// 多块消息里的 Text 块同样要能被识别（流式回复常为多块）
    #[test]
    fn detects_text_inside_multiple_blocks() {
        let messages = vec![assistant_blocks(vec![
            ContentBlock::Thinking {
                thinking: "先看看目录".to_string(),
                signature: String::new(),
            },
            ContentBlock::Text {
                text: "正在读取文件".to_string(),
            },
        ])];
        assert!(assistant_text_exists_at_tail(&messages, "正在读取文件"));
    }

    /// 不同内容不得误判为已存在，否则中断内容会被漏写
    #[test]
    fn does_not_match_different_text() {
        let messages = vec![assistant_single("上一轮已经说完的完整回复")];
        assert!(!assistant_text_exists_at_tail(
            &messages,
            "本轮被打断的半截话"
        ));
    }

    /// 空目标视为不存在，避免把空串当命中而跳过写入
    #[test]
    fn empty_target_is_never_a_match() {
        let messages = vec![assistant_single("")];
        assert!(!assistant_text_exists_at_tail(&messages, "   "));
    }

    /// 只检查尾部若干条：很早以前的相同文本不应影响本次判定
    #[test]
    fn only_inspects_recent_tail() {
        let mut messages = vec![assistant_single("重复的一句话")];
        for _ in 0..6 {
            messages.push(assistant_single("中间过程的其它回复"));
        }
        assert!(
            !assistant_text_exists_at_tail(&messages, "重复的一句话"),
            "超出尾部窗口的历史不应被当作本次已写入"
        );
    }

    /// 用户消息里的相同文本不算命中（只认助手消息）
    #[test]
    fn user_message_does_not_count() {
        let messages = vec![Message::User {
            content: Content::Single("沐先生，很".to_string()),
        }];
        assert!(!assistant_text_exists_at_tail(&messages, "沐先生，很"));
    }
}

#[cfg(test)]
mod interrupt_marker_tests {
    //! **措辞收敛的防回归**：中断标记按「模型能否接着做」二分。
    //!
    //! 背景：曾按中断原因写了 4 种措辞（超时/报错/取消/轮次上限）。
    //! 该标记进入对话历史后**每轮都会重发**，多种措辞 = 多个 prompt 前缀变体，
    //! 会让 provider 的 prompt cache 更易失效；而原因并不改变模型的动作。
    //! 因此"能接着做"的三种统一成一句。
    use super::{INTERRUPT_MARKER_RESUMABLE, INTERRUPT_MARKER_STOPPED};

    /// 能接着做的三种中断（超时/报错/取消）必须完全一致 —— 措辞漂移会让
    /// 缓存命中率下降，且难以察觉。
    #[test]
    fn resumable_markers_are_identical() {
        assert_eq!(
            INTERRUPT_MARKER_RESUMABLE,
            "**[回复被中断]** 上次回复在此处中断，请基于上下文继续完成。"
        );
    }

    /// 轮次上限刻意不同：此时模型**不该**接着写，否则立刻再次触达上限
    #[test]
    fn stopped_marker_is_distinct() {
        assert_ne!(INTERRUPT_MARKER_RESUMABLE, INTERRUPT_MARKER_STOPPED);
        assert!(
            INTERRUPT_MARKER_STOPPED.contains("回合上限"),
            "轮次上限标记应说明停下原因"
        );
    }

    /// 两种标记共享统一前缀；`strip_interrupt_markers`（agent_runs.rs）的
    /// "重建消息 vs 现存消息"等价比较依赖它
    #[test]
    fn both_share_detectable_prefix() {
        for marker in [INTERRUPT_MARKER_RESUMABLE, INTERRUPT_MARKER_STOPPED] {
            assert!(
                marker.contains("[回复被中断]"),
                "等价比较逻辑依赖该标签，不得修改：{marker}"
            );
            assert!(
                marker.starts_with("**["),
                "应以 `**[标签]` 开头（2026-09-20 风格统一：去掉 `>` 引用符与 emoji），\
                 便于统一剥离：{marker}"
            );
            assert!(!marker.contains('⚠'), "不应再含 emoji 哨兵：{marker}");
        }
    }
}

#[cfg(test)]
mod stream_retry_gate_tests {
    //! 重试判据测试。
    //!
    //! 判据已收敛到 `stream::is_zero_output`（见该函数注释），本模块只断言
    //! "零产出才重试"这一条策略本身。**关键回归**：曾把 `idle_timed_out` 与
    //! "是否允许重试"合用一个字段，导致"吐了半截后静默"时超时判定被置 false，
    //! run 永久卡死并锁住会话。因此这里必须覆盖"收到内容后不重试"的每一种形态。
    use crate::core::agent::stream::is_zero_output;

    /// 上游静默超时且零产出 → 允许重试（最需要重试的场景）
    #[test]
    fn zero_output_retries() {
        assert!(is_zero_output(true, true, false));
    }

    /// 已收到正文 → 不重试（避免界面文本拼接重复）
    #[test]
    fn partial_text_does_not_retry() {
        assert!(!is_zero_output(false, true, false));
    }

    /// 只收到思考 → 不重试（丢弃思考块会破坏思考链回传要求）
    #[test]
    fn thinking_only_does_not_retry() {
        assert!(!is_zero_output(true, false, false));
    }

    /// 已产生工具调用 → 不重试（工具可能已执行，重试会重复副作用）
    #[test]
    fn tool_call_does_not_retry() {
        assert!(!is_zero_output(true, true, true));
    }

    /// 回归防护（本次线上 bug）：正文与思考**都收到部分**后静默 → 不重试。
    /// 这正是 interrupted 模式的行为。此前该场景会因字段含义混用而既不重试
    /// 也不结束本轮，run 卡死。
    #[test]
    fn partial_text_and_thinking_does_not_retry() {
        assert!(!is_zero_output(false, false, false));
    }
}

#[cfg(test)]
mod waiting_hint_tests {
    use super::should_emit_waiting_hint;
    use crate::core::agent::stream::{waiting_hint_secs, STREAM_IDLE_TIMEOUT_SECS};

    /// 静默未达阈值：不提示（避免正常生成被打扰）
    #[test]
    fn no_hint_before_threshold() {
        assert!(!should_emit_waiting_hint(waiting_hint_secs() - 1, false));
        assert!(!should_emit_waiting_hint(0, false));
    }

    /// 刚好达到阈值且本段静默期未提示过 → 提示
    #[test]
    fn hints_once_at_threshold() {
        assert!(should_emit_waiting_hint(waiting_hint_secs(), false));
    }

    /// **回归防护（用户实测 bug）**：同一段静默期继续延长时**不得重复提示**。
    /// 首版每 30s 无条件发一条，页面被「已 30 秒未收到数据」刷屏。
    #[test]
    fn does_not_repeat_within_same_silence() {
        for extra in [0, 30, 60, 300] {
            assert!(
                !should_emit_waiting_hint(waiting_hint_secs() + extra, true),
                "静默 {} 秒时不应重复提示",
                waiting_hint_secs() + extra
            );
        }
    }

    /// **提示阈值必须由超时阈值派生**：以后者为唯一事实源，改一处即可。
    /// 硬编码独立的 30 曾在超时放宽到 120 后把用户晾在中间 90 秒。
    #[test]
    fn hint_threshold_is_derived_from_idle_timeout() {
        assert_eq!(waiting_hint_secs(), (STREAM_IDLE_TIMEOUT_SECS / 3).max(15));
        assert!(
            waiting_hint_secs() < STREAM_IDLE_TIMEOUT_SECS,
            "提示必须早于终止：hint={} timeout={}",
            waiting_hint_secs(),
            STREAM_IDLE_TIMEOUT_SECS
        );
    }
}

#[cfg(test)]
mod idle_terminal_tests {
    //! 「空闲超时必须以中断收尾」的语义防护。
    //!
    //! **实测 bug**：首版只在"零产出"时才收尾，于是 `interrupted` 形态
    //! （吐了半截后静默）虽然触发了计时器，却走到了正常完成路径 ——
    //! 界面只剩半截正文，用户以为已经正常完成。
    use super::must_finish_as_interrupted;

    /// 计时器触发就必须收尾，与是否重试、产出多少无关
    #[test]
    fn idle_timeout_always_terminates() {
        // (idle_timed_out, should_retry) → 必须以中断收尾
        assert!(must_finish_as_interrupted(true, true)); // hang：零帧静默
        assert!(
            must_finish_as_interrupted(true, false),
            "吐了半截后静默也必须收尾并给出中断提示——这正是实测漏掉的场景"
        );
    }

    /// 未触发空闲超时时不得收尾（否则正常完成会被误判为中断）
    #[test]
    fn normal_completion_does_not_terminate() {
        assert!(!must_finish_as_interrupted(false, false));
        assert!(!must_finish_as_interrupted(false, true));
    }

    /// 防回归：收尾判据**不得**依赖 should_retry（用错字段即本次 bug）
    #[test]
    fn termination_does_not_depend_on_retry_flag() {
        for idle in [true, false] {
            assert_eq!(
                must_finish_as_interrupted(idle, true),
                must_finish_as_interrupted(idle, false),
                "should_retry 不应影响收尾判定（idle_timed_out={idle}）"
            );
        }
    }
}

/// 上下文估算口径的回归测试。
///
/// `build_context_estimate` 的 `messages` 分区是按 `format_messages_readable` 的
/// 输出做 token 计数的，所以那段摘要**必须覆盖真正发出去的内容**。
///
/// 动态上下文（`<context>` 块）出网前会被
/// `adapters::materialize_context_blocks_for_wire` 翻译成普通 Text 一并发出，
/// 但早先的渲染器把它落到 `_ => {}` 静默丢弃，于是估算系统性偏低
/// （实测长会话里估算 17.3k、实际 52.0k），连带把压缩触发阈值拖到 3 倍之后。
#[cfg(test)]
mod context_estimate_tests {
    use super::*;

    /// 动态上下文块必须出现在摘要里，且排在用户原文之前（与落库顺序一致）
    #[test]
    fn context_blocks_are_rendered_in_readable_digest() {
        let ctx = "<context_snapshot>\n项目结构索引占位\n</context_snapshot>";
        let messages = vec![Message::User {
            content: Content::Multiple(vec![
                ContentBlock::Context {
                    text: ctx.to_string(),
                },
                ContentBlock::Text {
                    text: "帮我看看".to_string(),
                },
            ]),
        }];

        let out = crate::core::agent::context_format::format_messages_readable(&messages);

        assert!(
            out.contains("项目结构索引占位"),
            "动态上下文块被丢弃了，估算会偏低：\n{out}"
        );
        assert!(
            out.contains("[Context]"),
            "应带 [Context] 标记，便于在浮层里定位：\n{out}"
        );
        assert!(out.contains("帮我看看"), "用户原文不能丢：\n{out}");
        assert!(
            out.find("[Context]") < out.find("帮我看看"),
            "块顺序应与消息内顺序一致（上下文在前）：\n{out}"
        );
    }

    /// 空白的上下文块不得产生噪音行
    #[test]
    fn empty_context_block_renders_nothing() {
        let messages = vec![Message::User {
            content: Content::Multiple(vec![
                ContentBlock::Context {
                    text: "   ".to_string(),
                },
                ContentBlock::Text {
                    text: "hi".to_string(),
                },
            ]),
        }];

        let out = crate::core::agent::context_format::format_messages_readable(&messages);
        assert!(!out.contains("[Context]"), "空白上下文块不应渲染：\n{out}");
    }
}

#[cfg(test)]
mod interrupted_turn_store_tests {
    //! 「中断落库保持一问一答」的语义防护（2026-09-19 丙方案定案）。
    //!
    //! 背景事故：取消/断流/报错三条收尾路径各自乱拼——
    //! 把多段正文拼成一条、又额外补一条 assistant 占位提示，
    //! 结果一次中断产出好几条助手消息，且正文为空时前端**向上一条**助手
    //! 消息借文渲染，用户看到的内容顺序割裂。
    //!
    //! 现口径：半截内容（思考 + 正文）+ 中断提示**合并成一条**消息；
    //! 正文为空（纯思考期中止）时把提示本身作为正文块，保证渲染锚点存在。
    use super::store_interrupted_turn;
    use crate::infra::types::models::{
        Content, ContentBlock, InterruptKind, Message, MessageSource, SessionMemory,
    };

    /// 测试用中断类型（阶段二起 `store_interrupted_turn` 收 kind，不再收标记文本）。
    /// 取 `PipelineError` 代表"能接着做"一类——它的标记文本仍是
    /// `INTERRUPT_MARKER_RESUMABLE`，供"发送前拼回"的断言使用。
    const KIND: InterruptKind = InterruptKind::PipelineError;

    fn empty_memory() -> SessionMemory {
        SessionMemory::default()
    }

    /// 模拟正常路径已落库的助手消息（`store_assistant_response` 的产物：裸正文，无提示）
    fn seed_plain_assistant(session: &mut SessionMemory, text: &str) {
        super::append_message(
            session,
            Message::Assistant {
                content: Content::Multiple(vec![ContentBlock::Text {
                    text: text.to_string(),
                }]),
            },
            MessageSource::Chat,
        );
    }

    fn only_blocks(session: &SessionMemory) -> Vec<ContentBlock> {
        blocks_at(session, 0)
    }

    fn blocks_at(session: &SessionMemory, idx: usize) -> Vec<ContentBlock> {
        match &session.messages[idx] {
            Message::Assistant {
                content: Content::Multiple(blocks),
            } => blocks.clone(),
            other => panic!("期望 Multiple 助手消息，实际: {other:?}"),
        }
    }

    fn body_text(blocks: &[ContentBlock]) -> String {
        blocks
            .iter()
            .rev()
            .find_map(|b| match b {
                ContentBlock::Text { text } => Some(text.clone()),
                _ => None,
            })
            .expect("正文块必须存在（渲染锚点）")
    }

    // ── 分支二：尾部没有当轮内容 → 新建一条合并消息 ──

    /// 有正文 + 有思考：一条消息，思考在前正文在后，正文末尾附提示
    #[tokio::test]
    async fn text_and_thinking_merge_into_single_message() {
        let mut session = empty_memory();
        let wrote = store_interrupted_turn(&mut session, "半截正文", "半截思考", KIND).await;

        assert!(wrote, "有内容时必须落库");
        assert_eq!(
            session.messages.len(),
            1,
            "必须只有一条助手消息（一问一答）"
        );
        let blocks = only_blocks(&session);
        assert!(
            matches!(&blocks[0], ContentBlock::Thinking { .. }),
            "思考块在前"
        );
        let body = body_text(&blocks);
        assert!(body.starts_with("半截正文"), "正文必须保留原文：{body}");
        assert!(body.contains("半截正文"));
        assert!(
            !body.contains("回复被中断"),
            "阶段二起标记不进正文（只写 interrupt_kinds）：{body}"
        );
        assert_eq!(
            session.interrupt_kinds.last().cloned().flatten().as_deref(),
            Some(KIND.as_str()),
            "中断类型必须打在该消息上"
        );
        assert_eq!(session.sources[0], MessageSource::Interrupted);
    }

    /// 纯思考期中止（正文为空）：提示本身作为正文块 —— 前端渲染锚点不能缺
    #[tokio::test]
    async fn thinking_only_promotes_marker_to_body() {
        let mut session = empty_memory();
        let wrote = store_interrupted_turn(&mut session, "", "只有思考", KIND).await;

        assert!(wrote);
        assert_eq!(session.messages.len(), 1);
        let blocks = only_blocks(&session);
        assert_eq!(
            body_text(&blocks),
            "",
            "无产出时正文留空块（渲染锚点），小字由 kind 提供"
        );
    }

    /// 正文期中止（无思考）：只有正文块，末尾附提示
    #[tokio::test]
    async fn text_only_appends_marker() {
        let mut session = empty_memory();
        store_interrupted_turn(&mut session, "半截正文", "", KIND).await;

        let blocks = only_blocks(&session);
        assert_eq!(blocks.len(), 1, "无思考时不应凭空造 Thinking 块");
        let body = body_text(&blocks);
        assert!(!body.contains("回复被中断"), "标记不得进正文：{body}");
        assert!(body.contains("半截正文"));
    }

    /// 完全无产出：仍需一条只有提示的助手消息（否则界面无锚点，会向上借文）
    #[tokio::test]
    async fn nothing_produced_still_anchors_marker() {
        let mut session = empty_memory();
        let wrote = store_interrupted_turn(&mut session, "   ", "\n", KIND).await;

        assert!(wrote, "无产出也必须给出提示锚点");
        assert_eq!(session.messages.len(), 1);
        assert_eq!(
            body_text(&only_blocks(&session)),
            "",
            "无产出时正文留空块（渲染锚点）"
        );
    }

    // ── 分支一：尾部已有当轮内容 → 原地追加提示 ──

    /// **核心场景**：主循环 `store_assistant_response` 已落当轮裸正文，
    /// 收尾时必须在**同一条消息**的正文末尾追加提示，既不新增消息、也不丢提示。
    ///
    /// 旧口径（整块精确比对即视为"已落库"直接返回）会让提示**永远写不进去**——
    /// 这正是本次要修的行为。
    #[tokio::test]
    async fn appends_marker_to_existing_tail_message() {
        let mut session = empty_memory();
        seed_plain_assistant(&mut session, "半截正文");

        let wrote = store_interrupted_turn(&mut session, "半截正文", "", KIND).await;

        assert!(wrote, "提示必须被写入（不能因正文已存在就跳过）");
        assert_eq!(session.messages.len(), 1, "必须仍是一条消息（一问一答）");
        let body = body_text(&only_blocks(&session));
        assert!(body.starts_with("半截正文"), "原文不能丢：{body}");
        assert!(!body.contains("回复被中断"), "标记不得进正文：{body}");
    }

    /// 已有消息带思考+正文时，提示只追加到正文块，绝不污染思考块
    #[tokio::test]
    async fn marker_never_pollutes_thinking_block() {
        let mut session = empty_memory();
        super::append_message(
            &mut session,
            Message::Assistant {
                content: Content::Multiple(vec![
                    ContentBlock::Thinking {
                        thinking: "内心独白".to_string(),
                        signature: String::new(),
                    },
                    ContentBlock::Text {
                        text: "半截正文".to_string(),
                    },
                ]),
            },
            MessageSource::Chat,
        );

        store_interrupted_turn(&mut session, "半截正文", "内心独白", KIND).await;

        let blocks = only_blocks(&session);
        match &blocks[0] {
            ContentBlock::Thinking { thinking, .. } => {
                assert!(
                    !thinking.contains("回复被中断"),
                    "思考块不得混入系统提示：{thinking}"
                );
            }
            other => panic!("第一个块应为思考，实际: {other:?}"),
        }
        assert!(!body_text(&blocks).contains("回复被中断"));
    }

    /// 幂等：同一收尾重复调用，提示只追加一次
    #[tokio::test]
    async fn repeated_store_is_idempotent() {
        let mut session = empty_memory();
        seed_plain_assistant(&mut session, "半截正文");

        assert!(store_interrupted_turn(&mut session, "半截正文", "", KIND).await);
        let after_first = session.messages.len();
        let first_body = body_text(&only_blocks(&session));

        let wrote = store_interrupted_turn(&mut session, "半截正文", "", KIND).await;
        assert!(!wrote, "重复收尾不应再次改动");
        assert_eq!(session.messages.len(), after_first, "不得新增消息");
        assert_eq!(
            body_text(&only_blocks(&session)),
            first_body,
            "正文不得被追加两次"
        );
    }
}
