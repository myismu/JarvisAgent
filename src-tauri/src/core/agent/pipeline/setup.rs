//! # pipeline/setup.rs — 管线阶段 1（初始化）与阶段 2（循环前准备）

use super::*;

impl PipelineState {
    /// **首次固化**会话的深度思考档位。
    ///
    /// ## 为什么需要它
    ///
    /// 新建会话时（`create_session`）还不知道本次要用哪个模型，因此拿不到
    /// `thinking_forced`，无法解析"跟随全局"的默认档位，只能先写占位 `false`。
    /// 真正可解析的时点是**第一条消息**——此时 `model_id` 已确定。
    ///
    /// ## 判定"是否需要固化"
    ///
    /// 用 `ctx` 里是否已有"用户表态"来区分。但 `ctx.thinking_mode` 只是布尔，
    /// 无法区分"占位 false"与"用户主动选的 false"。因此这里改用**另一条事实**：
    /// 会话行是否已被固化过。
    ///
    /// 为免再加一列，采用更简单的口径：**只有"本会话从未跑过主 Agent"时才固化**，
    /// 判据是 `ctx.turn_think` 尚未被写过（`None`）。跑过一轮之后，用户若拨了开关，
    /// 值会由 `set_session_thinking_enabled` 命令直接写入，不会再进这里。
    ///
    /// 这样"用户拨过 → 用用户的值"与"用户没拨 → 用设置默认"两条路径不会互相覆盖。
    pub(super) async fn ensure_session_thinking_initialized(
        app: &tauri::AppHandle,
        session_id: &str,
        ctx: &std::sync::Arc<crate::infra::state::state::SessionContext>,
        thinking_forced: bool,
    ) {
        // 已经跑过主 Agent：说明值已固化（或用户已表态），不再改动
        if ctx.turn_think.lock().await.is_some() {
            return;
        }

        // 读设置里的默认档位（L1），解析为确定布尔
        let default_raw = crate::command::app_config::read_file()
            .ui_preferences
            .thinking_default;
        let resolved = crate::core::session::thinking::ThinkingDefault::parse(&default_raw)
            .resolve(thinking_forced);

        // 写内存态 + 落库（**只在这里写一次**，此后设置改动不再影响本会话）
        *ctx.thinking_mode.lock().await = resolved;
        if let Err(e) = crate::core::session::update_session_thinking_mode(session_id, resolved) {
            // 落库失败不阻断本轮：内存值仍然生效，下轮会重试固化
            eprintln!("[JARVIS] 固化会话思考档位失败（不阻断本轮）: {}", e);
            return;
        }
        // 通知前端刷新（监控窗口 / 主窗口的开关都需要跟着显示）
        let _ = app.emit(
            "session-thinking-mode-changed",
            crate::infra::types::events::SessionThinkingModeChangedEvent {
                session_id: session_id.to_string(),
                thinking_enabled: resolved,
            },
        );
    }

    /// 阶段 1：初始化 — 会话与配置准备 + 复杂任务判定
    ///
    /// 相当于“启动前检查 + 路由决策”，产出可执行的 PipelineState。核心逻辑：
    /// 1. 校验：会话是否正忙（已有任务在跑则拒绝）、是否配置了 API Key
    /// 2. 创建本次执行的取消令牌（用户随时可停止），并记录请求工作区；校验沙箱目录仍存在
    /// 3. 读取会话级双轴配置：受众（developer/user）× 工作模式（edit/plan），
    ///    据此组装系统提示词并**按会话缓存**（会话内字节恒定，保证 prompt cache 命中）；
    ///    同时固化本会话的工具开关快照与能力清单
    /// 4. 复杂任务判定（已无意图分类，仅此一项布尔检测）：`is_complex_task` 正则命中
    ///    → 产出 TASK_PLAN，首轮强制切 Plan 并广播 agent-work-mode-changed；
    ///    未命中 → 直接进入项目操作流程
    /// 5. 输入框自然语言审批：短消息 + 上轮刚提交方案 → 直接更新方案状态（同意/驳回）
    #[allow(clippy::too_many_arguments)]
    pub(super) async fn setup(
        session_id: String,
        msg: String,
        thinking_override: Option<bool>,
        image_base64_list: Option<Vec<String>>,
        _agent_display_mode: Option<String>,
        reflection_mode_override: Option<String>,
        display_msg: Option<String>,
        _inject_user_message: bool,
        app: tauri::AppHandle,
        session_manager: tauri::State<'_, crate::infra::state::state::SessionManager>,
        config_state: tauri::State<'_, crate::infra::config::config::ConfigState>,
    ) -> Result<Self, AgentError> {
        println!("\n{}", "=".repeat(60));
        println!(
            "[贾维斯] 收到用户消息: {} (图片数量: {})",
            msg,
            image_base64_list.as_ref().map(|l| l.len()).unwrap_or(0)
        );
        println!("{}", "=".repeat(60));

        let sid = session_id.clone();
        let ctx = session_manager.get_or_create(&session_id).await;
        // 步骤 1：用同一把会话锁完成“检查并占用”，并给本轮分配独立编号。
        // 取消信号和占用状态一起登记，避免并发请求同时通过检查、互相覆盖令牌。
        let execution_id = format!("ex_{}", uuid::Uuid::new_v4());
        let cancel_token = tokio_util::sync::CancellationToken::new();
        let mut execution_lease = match ctx
            .try_start_execution(execution_id.clone(), cancel_token.clone())
            .await
        {
            Ok(lease) => lease,
            Err(is_stopping) => {
                return Err(if is_stopping {
                    AgentError::ExecutionStopping(
                        "上一轮正在停止中，请等待完成后再发送。".to_string(),
                    )
                } else {
                    AgentError::ExecutionBusy(
                        "当前会话已有任务正在执行，请等待完成或先停止当前任务。".to_string(),
                    )
                })
            }
        };

        // 步骤 3：记录本次请求的工作区（文件操作 / 沙箱边界根目录）
        let request_workspace = ctx.workspace.lock().await.clone();
        println!(
            "[DEBUG] Current Workspace for session {}: {:?}",
            sid, request_workspace
        );

        // 步骤 3.5：沙箱目录存在性校验。
        // 会话绑定的项目目录可能事后被删除/移动：目录没了还继续跑，
        // 校验层按沙箱 join 放行、执行层却找不到文件，模型会在错误目录里打转。
        if let Some(ws) = &request_workspace {
            if !ws.exists() {
                execution_lease.finish();
                return Err(AgentError::Session(format!(
                    "会话绑定的项目目录已不存在: {}（可能已被删除或移动）。请重建会话并重新挂载项目后再试。",
                    ws.display()
                )));
            }
        }

        // 步骤 4：读取用户配置，校验 API Key 是否已配置
        let app_cfg = config_state.0.lock().await.clone();
        let cfg = app_cfg.active_config();

        if cfg.api_key.is_empty() {
            execution_lease.finish();
            return Err(AgentError::Config(
                "未配置 API Key，请在设置中填写".to_string(),
            ));
        }
        let api_key = cfg.api_key.clone();
        let base_url = cfg.base_url.clone();
        let model_id = cfg.main_model.clone();
        let utility_model_id = cfg.utility_model.clone();
        let api_format = cfg.api_format_enum();
        println!(
            "[JARVIS] Using model: {} (utility: {})",
            model_id, utility_model_id
        );

        // 步骤 5：创建 HTTP 客户端（后续所有 LLM 调用共用）
        // 统一构造：带连接超时与 TCP keepalive，避免半开连接永久挂起。
        // 不在此设置整体 timeout —— 会误杀长时间流式生成，分层时限见
        // api_client::build_client 的说明。
        let client = api_client::build_streaming_client();

        // 步骤 6：工作模式 / 权限档位 / 用户类型以会话上下文为准。
        //
        // 三者已是会话级属性（sessions 表落库，get_or_create 恢复：落库值，否则设置默认），
        // pipeline 不再做任何"按偏好覆盖"——否则会把用户在本会话内的切换/落库值冲掉
        // （历史包袱：这里曾按 has_history 用偏好覆盖模式与档位、每轮覆盖受众；
        // 设置里的默认现在只决定"新会话的初始值"这一个用途）。
        let current_work_mode = ctx.agent_work_mode.lock().await.clone();
        let current_audience = ctx.agent_audience.lock().await.clone();
        // system 提示词：**会话级缓存，get_or_assemble**。
        // 提示词磁盘化后（data/prompts/ 用户可覆盖），如果不缓存，用户改了 md 文件
        // 后同一会话的下一个 turn 会重组 system → 缓存前缀整体失效。
        // 缓存住 = 会话内字节恒定从"约定"升级为"代码保证"，改动只对新会话生效。
        let system_prompt = {
            let mut cache = ctx.system_prompt_cache.lock().await;
            if cache.is_none() {
                *cache = Some(crate::core::agent::prompts::get_system_prompt(
                    &current_audience,
                    &current_work_mode,
                    request_workspace.as_deref(),
                ));
            }
            cache.clone().expect("just populated")
        };

        // 步骤 6.4：取本会话的工具开关快照
        //
        // 在这里取（而不是每轮现取）有两层用意：
        // 1. `ctx.tool_filter()` 会按需从配置固化一份快照，此后本会话复用 —— 于是
        //    `current_tools()` 每轮拿到的 tools 参数字节恒定，prompt cache 照常命中；
        // 2. 它是异步的，而 `current_tools()` 是同步的，只能在这里取好存进 PipelineState。
        let tool_filter = ctx.tool_filter().await;

        // 步骤 6.5：能力清单
        //
        // 只用于注入"能力边界声明"（动态上下文 + 工具目录结论），让模型第一轮就知道
        // 哪些能力在本模式不存在，不必用发现类工具去试探。
        // 受限模式（规划）下用户要求改文件时，不再提前截断——交给模型自己解释并走方案审批流程，
        // 这样用户少一次往返（不必先回一句"先给方案"）。
        let capabilities = crate::core::tools::framework::capabilities::Capabilities::for_work_mode(
            &current_work_mode,
            &tool_filter,
        );

        // 步骤 7：复杂任务判定，一律直接对用户原文做正则直判。
        //
        // 此处原有一段 `has_images` / `msg_for_intent`：带图片时给输入拼一句
        // "不要仅因有图判为 CHAT"的说明。那句是给**已取消的意图分类**用的，取消后
        // 拼接结果只喂给下面的正则与前缀判断，而追加文字命中不了 COMPLEX_TASK_KEYWORDS
        // 的任何一条，属惰性代码，已随死代码清理删除。
        //
        // "用户已同意方案/要求修改方案"是审批续跑，不算复杂任务
        let is_approval_continuation =
            msg.starts_with("用户已同意方案") || msg.starts_with("用户要求修改方案");
        // 复杂任务判定：正则直判（命中 → 走方案审批）；"用户已同意方案/要求修改"属于审批续跑
        let detected_intent = {
            let is_complex = crate::core::complex_task::is_complex_task(&msg);
            if is_complex && !is_approval_continuation {
                println!(
                    "[JARVIS] {} 模式：规则检测到复杂任务，首轮直接进入方案审批流程",
                    current_work_mode
                );
                "TASK_PLAN".to_string()
            } else {
                println!("[JARVIS] {} 模式：直接进入项目操作流程", current_work_mode);
                DIRECT_DEVELOPER_INTENT.to_string()
            }
        };
        println!("[JARVIS] Detected intent: {}", detected_intent);

        // 输入框自然语言审批：
        // 仅在三种条件同时满足时自动更新 plan status：
        // 1. 短消息（≤5 字），长消息不可能是纯粹审批回复
        // 2. 上一轮助手刚提交了方案（history 最后一条 assistant 含 ProposePlan 或方案提交文本）
        // 3. plan_documents 中存在 pending 方案
        if msg.trim().chars().count() <= 5 {
            // 上一轮 Agent 最后一次工具行动是否为提方案
            // 找最后一条含 tool_use 的 assistant 消息，检查是否为 ProposePlan
            let was_proposing_plan = {
                let memory = ctx.memory.lock().await;
                memory.messages.iter().rev()
                    .find_map(|m| {
                        if let Message::Assistant {
                            content: Content::Multiple(blocks),
                        } = m
                        {
                            let has_tool = blocks.iter().any(|b| matches!(b, ContentBlock::ToolUse { .. }));
                            if has_tool {
                                let is_plan = blocks.iter().any(|b| matches!(b, ContentBlock::ToolUse { name, .. } if name == "ProposePlan"));
                                return Some(is_plan);
                            }
                        }
                        None
                    })
                    .unwrap_or(false)
            };

            if was_proposing_plan {
                let mut approved_any = false;
                // 本轮批准通过的方案文档：锁外据此自动建任务图。
                // 必须在 memory 锁外做 DB 写入，避免与其它会话操作形成锁顺序问题。
                let mut approved_plans: Vec<PlanDocument> = Vec::new();
                {
                    let mut memory = ctx.memory.lock().await;
                    let pending_plans: Vec<_> = memory
                        .plan_documents
                        .iter()
                        .filter(|doc| doc.status == "pending")
                        .map(|doc| (doc.id.clone(), doc.title.clone()))
                        .collect();

                    if !pending_plans.is_empty() {
                        // 区分同意/拒绝：短回复没有稳定的结构特征，只能靠文本里的否定词判断
                        let msg_trim = msg.trim();
                        let is_reject = msg_trim.starts_with("不")
                            || msg_trim == "拒绝"
                            || msg_trim == "reject"
                            || msg_trim == "no";
                        let new_status = if is_reject {
                            "revision_requested"
                        } else {
                            "approved"
                        };
                        approved_any = new_status == "approved";

                        for (plan_id, plan_title) in &pending_plans {
                            if let Ok(Some(doc)) = crate::core::session::update_plan_document_status(
                                &session_id,
                                plan_id,
                                new_status,
                                None,
                            ) {
                                if let Some(existing) =
                                    memory.plan_documents.iter_mut().find(|d| d.id == doc.id)
                                {
                                    *existing = doc.clone();
                                }
                                let _ = app.emit("plan-document-updated", &doc);
                                if new_status == "approved" {
                                    approved_plans.push(doc);
                                }
                            }
                            println!(
                                "[JARVIS] 输入框审批：方案「{}」→ {}",
                                plan_title, new_status
                            );
                        }
                    }
                } // 释放 memory 锁，避免锁顺序死锁

                // B2：方案批准后由服务端强制切回 edit；模型侧的 SwitchWorkMode(edit) 仅作双保险
                if approved_any {
                    // 审批动作本身负责生成清单：按方案的结构化任务分解建任务图，
                    // 并投影成 TODO 清单（任务图留 SQLite 供调度器使用，TODO 是内存态）。
                    // 全部成功才算“任务已创建”；任一方案失败则整体走兼容分支。
                    let mut tasks_ready = !approved_plans.is_empty();
                    for plan in &approved_plans {
                        let synced =
                            crate::core::orchestration::tasks::sync_plan_tasks(&session_id, plan);
                        match synced {
                            Some(tasks) => {
                                crate::core::orchestration::todo_sync::mirror_plan_tasks(
                                    &app,
                                    &ctx,
                                    &session_id,
                                    &plan.id,
                                    &tasks,
                                )
                                .await;
                            }
                            None => tasks_ready = false,
                        }
                    }

                    // 输入框短回复审批不会经过前端那段“已同意方案”的续跑消息，
                    // 这里补一条内部指令，保证两条审批入口对模型的要求完全一致。
                    let titles = approved_plans
                        .iter()
                        .map(|plan| plan.title.as_str())
                        .collect::<Vec<_>>()
                        .join("、");
                    // 极端情况下（状态已改但文档读取失败）标题为空，别把「」送给模型。
                    let titles = if titles.is_empty() {
                        "方案".to_string()
                    } else {
                        titles
                    };
                    let instruction = if tasks_ready {
                        format!(
                            "用户已同意方案「{}」。任务清单已由系统生成到待办清单，**不要再调用 CreateTask**。\
                             请切回编辑模式（SwitchWorkMode mode=\"edit\"），然后直接调用 \
                             RunSubagentsSequentially 执行任务；调度完成后读取报告并向用户汇报结果。",
                            titles
                        )
                    } else {
                        format!(
                            "用户已同意方案「{}」。请切回编辑模式（SwitchWorkMode mode=\"edit\"），\
                             按方案的任务分解调用 CreateTask 创建任务图，再调用 \
                             RunSubagentsSequentially 执行任务；完成后向用户汇报结果。",
                            titles
                        )
                    };
                    {
                        let mut session = ctx.memory.lock().await;
                        append_message(
                            &mut session,
                            Message::User {
                                content: Content::Single(instruction),
                            },
                            MessageSource::Inject,
                        );
                    }

                    let mut mode = ctx.agent_work_mode.lock().await;
                    if mode.as_str() != "edit" {
                        let old_mode = mode.clone();
                        *mode = "edit".to_string();
                        drop(mode);
                        // 模式是会话级属性：同步落库（sessions.work_mode），随会话恢复
                        if let Err(e) =
                            crate::core::session::update_session_work_mode(&session_id, "edit")
                        {
                            eprintln!("[JARVIS] 工作模式落库失败（会话 {}）：{}", session_id, e);
                        }
                        let _ = app.emit(
                            "agent-work-mode-changed",
                            crate::infra::types::events::AgentWorkModeChangedEvent {
                                session_id: session_id.clone(),
                                from: old_mode,
                                to: "edit".to_string(),
                                reason: "方案已批准，自动切回编辑模式".to_string(),
                            },
                        );
                    }
                }
            }
        }

        // 步骤 8：确定反思审查模式（取配置默认值，可被本次请求覆盖）
        let resolved_reflection_mode = reflection_mode_override
            .filter(|m| !m.is_empty())
            .unwrap_or_else(|| cfg.reflection_mode.clone());

        // 步骤 8.5：解析深度思考档位（v14 起只有 L2 会话布尔，不再有 L1 现读）
        //
        // 关键变化：**设置默认值（L1）不在这里参与裁决**。
        // 它只在"会话还没有值"时被解析一次并落库（见下），此后这个会话就只认库里的布尔。
        // 这样设置页改默认档位**永远不会倒灌已有会话**——正是本次重构要修的问题。
        //
        // 落点说明：新建会话时 `create_session` 只能写占位 `false`（那时还不知道主模型），
        // 所以真正的初值必须在**第一条消息**、拿到 `model_id` 之后解析。
        {
            let caps = crate::infra::llm::registry::query_capabilities(&model_id);
            let thinking_forced = caps.as_ref().map(|c| c.thinking_forced).unwrap_or(false);
            Self::ensure_session_thinking_initialized(&app, &sid, &ctx, thinking_forced).await;
        }
        let session_think_mode =
            crate::core::session::thinking::ThinkingMode(*ctx.thinking_mode.lock().await);

        // 步骤 9：组装 PipelineState（意图、提示词、取消令牌等已就绪）
        let mut state = Self {
            app,
            sid,
            ctx,
            execution_lease,
            cancel_token,
            request_workspace,
            cfg,
            api_key,
            base_url,
            model_id,
            api_format,
            client,
            system_prompt,
            msg,
            image_base64_list,
            thinking_override,
            session_think_mode,
            detected_intent: detected_intent.clone(),
            capabilities,
            tool_filter,
            // 以下字段在后续阶段填充
            dynamic_context_str: String::new(),
            user_msg_preview: String::new(),
            initial_msg_index: 0,
            user_message_id: None,
            should_think: false,
            turn_think: false,
            turn_thinking_decision: crate::core::session::thinking::ThinkingDecision::default(),
            run_id: String::new(),
            loop_count: 0,
            total_loop_count: 0,
            req_input_tokens: 0,
            req_output_tokens: 0,
            req_sub_input_tokens: 0,
            req_sub_output_tokens: 0,
            final_answer: String::new(),
            reflection_mode: resolved_reflection_mode,
            total_reflections: 0,
            consecutive_reflection_nos: 0,
            plan_total_loops_without_plan: 0,
            interrupted_reason: None,
            notice_i18n_key: None,
            notice_detail: None,
            current_blocks: Vec::new(),
            turn_text_this_turn: String::new(),
            turn_thinking_this_turn: String::new(),
            // 崩溃保护是纯全局设置：读一次当轮快照，中途改设置不影响进行中的轮次
            crash_protection: crate::command::app_config::read_file()
                .ui_preferences
                .crash_protection,
            display_msg,
            tool_execution_summary: None,
        };

        // 步骤 10：TASK_PLAN 前置拦截 —— 复杂任务首轮强制切到 plan 模式并广播
        if detected_intent == "TASK_PLAN" && current_work_mode != "plan" {
            println!("[JARVIS] 意图前置拦截：TASK_PLAN 意图，首轮强制切换到 Plan 模式");
            *state.ctx.agent_work_mode.lock().await = "plan".to_string();
            // 模式是会话级属性：同步落库（sessions.work_mode），随会话恢复
            if let Err(e) = crate::core::session::update_session_work_mode(&state.sid, "plan") {
                eprintln!("[JARVIS] 工作模式落库失败（会话 {}）：{}", state.sid, e);
            }
            // system 必须全程字节恒定：这里只切换 work_mode，不重建 system。
            state.detected_intent = "TASK_PLAN".to_string();
            let _ = state.app.emit(
                "agent-work-mode-changed",
                crate::infra::types::events::AgentWorkModeChangedEvent {
                    session_id: state.sid.clone(),
                    from: current_work_mode,
                    to: "plan".to_string(),
                    reason: "检测到复杂任务，自动切换到计划模式".to_string(),
                },
            );
        }

        Ok(state)
    }

    /// 阶段 2：上下文构建 + 消息注入 + Agent Run 启动
    ///
    /// 主循环开始前的一次性准备：
    /// 1. 构建动态上下文（意图相关提示、工作区信息等）
    /// 2. 崩溃恢复：若上次 run 异常中断（含 InProgress 残留任务），注入“恢复指令”给 LLM
    /// 3. 把用户消息（含图片）注入会话历史，记录其在消息列表中的位置 initial_msg_index
    /// 4. 决定首轮是否深度思考（用户临时开关优先，否则按受众默认值）
    /// 5. 在 agent_runs 表登记本次 run，并保存第一个检查点
    pub(super) async fn pre_loop(&mut self) {
        // 步骤 1：构建动态上下文（意图相关提示 + 工作区信息）
        // 快照 seq 使用 SessionMemory 的持久化单调计数器，压缩/重启后仍严格递增，
        // 保证“以 seq 最大（最新）的快照为准”不会因 messages.len() 回退而失效。
        let current_mode = { self.ctx.agent_work_mode.lock().await.clone() };
        let snapshot_seq = {
            let mut session = self.ctx.memory.lock().await;
            session.snapshot_seq = session.snapshot_seq.saturating_add(1);
            session.snapshot_seq
        };
        // 能力清单位于快照内，必须与当前 work_mode 保持一致（审批通过/首轮强制切 plan 后亦然）；
        // 工具开关则用 setup 时取好的会话快照，保证与 current_tools() 的可见性口径一致
        self.capabilities =
            crate::core::tools::framework::capabilities::Capabilities::for_work_mode(
                &current_mode,
                &self.tool_filter,
            );
        self.dynamic_context_str = build_dynamic_context(
            &self.detected_intent,
            &self.request_workspace,
            &self.capabilities,
            &current_mode,
            snapshot_seq,
        );

        // 步骤 2：准备用户消息的短版预览（给 UI 展示）
        self.user_msg_preview = if self.msg.chars().count() > 50 {
            self.msg.chars().take(50).collect::<String>()
        } else {
            self.msg.clone()
        };

        // 步骤 3：恢复 + 注入 —— 处理崩溃残留并把本次用户消息写入历史
        // 注意：active_run_id 在拿 memory 锁**之前**读取（统一锁顺序，避免交叉持有）
        let active_run_id_before_recovery = self.ctx.active_run_id.lock().await.clone();
        let memory_after_user_message = {
            let mut session = self.ctx.memory.lock().await;
            // 3a. 崩溃恢复：上次 run 异常中断时，把残留消息恢复进内存。
            // 走唯一闸门 `ensure_session_recovered`（原子抢占锁 + 闸门内统一落库/盖章），
            // 本入口不再自行 save/mark。
            let recovered = crate::command::session::ensure_session_recovered(
                &self.sid,
                &mut session,
                active_run_id_before_recovery.as_deref(),
            );
            if recovered {
                // 检测程序崩溃时残留的 InProgress 任务，注入恢复指令给 LLM
                let tm = crate::core::orchestration::tasks::TaskManager::for_session(&self.sid);
                let in_progress: Vec<_> = tm
                    .get_all_tasks()
                    .into_iter()
                    .filter(|t| t.status == crate::infra::types::models::TaskStatus::InProgress)
                    .collect();
                if !in_progress.is_empty() {
                    let task_list: String = in_progress
                        .iter()
                        .map(|t| format!("  • Task #{}: {}", t.id, t.subject))
                        .collect::<Vec<_>>()
                        .join("\n");
                    let recovery_msg = format!(
                        "【系统恢复通知】\n\
                        程序上次非正常结束（崩溃/强退），以下 {} 个任务在执行中被中断：\n\
                        {}\n\n\
                        请按以下步骤处理：\n\
                        1. 检查工作目录中的实际文件状态，判断哪些任务已完成、部分完成、未开始\n\
                        2. 已完成的任务 → 用 UpdateTask 标为 completed\n\
                        3. 部分完成的任务 → 用 UpdateTask 标为 pending（或保持 InProgress 不处理），评估剩余工作\n\
                        4. 未开始的任务 → 保持 pending\n\
                        5. 完成状态整理后，重新调用 RunSubagentsSequentially 继续执行未完成的任务",
                        in_progress.len(), task_list
                    );
                    // 来源是 `Inject` 而**不是** `Placeholder`：这条通知的正文写着
                    // "检查文件状态 → UpdateTask → 重新调用 RunSubagentsSequentially"，
                    // 是**写给模型看的动作要求**，必须进模型上下文（`in_llm_context()` 为真）。
                    // 它曾经被标成 `internal`，而 `internal` 不在模型白名单里 ——
                    // 恢复指令从落地那天起就没送到过模型（见
                    // `doc/消息来源与轮次元数据重构方案.md` §2.2）。
                    //
                    // role 从 `Assistant` 改成 `User`：**这条消息以前两侧都不可见，
                    // role 是什么无所谓；一旦真的发给模型，role 就开始表达语义。**
                    // 正文是"请按以下步骤处理"——这是系统对模型下的指令，不是模型自己说过的话。
                    // 留在 `Assistant` 会让模型把它读成自己先前的表态，从而当成已做出的承诺。
                    // 与另外两处 `Inject`（方案重定向通知 / 反思修正建议）保持同为 `User`。
                    append_message(
                        &mut session,
                        Message::User {
                            content: Content::Single(recovery_msg),
                        },
                        MessageSource::Inject,
                    );
                    println!(
                        "[JARVIS] 恢复：检测到 {} 个 InProgress 任务，已注入恢复指令",
                        in_progress.len()
                    );
                }

                let _ = self.app.emit("session-updated", ());
            }
            // 3b. 把用户消息（含图片）注入历史，记录起始位置 initial_msg_index
            let mut active_sid = Some(self.sid.clone());
            let (initial_msg_index, user_message_id) = inject_user_message(
                &mut session,
                self.display_msg.as_deref().unwrap_or(&self.msg),
                &self.image_base64_list,
                &self.dynamic_context_str,
                &mut active_sid,
            );
            self.initial_msg_index = initial_msg_index;
            self.user_message_id = Some(user_message_id);
            session.clone()
        };
        // 步骤 4：注入后立即落库（防止崩溃丢消息）
        crate::core::session::save_session(&self.sid, &memory_after_user_message, None);
        let _ = self.app.emit("session-updated", ());
        // 可见消息集合变了：让非活跃的会话视图标脏（前端切回时只重拉一次）
        crate::core::session::notify_messages_changed(&self.app, &self.sid, "agent-commit");

        // 深度思考决策（整轮一次性决定，之后不再改变）
        //
        // 优先序（设计文档 §4）：L3 单轮覆盖 ▸ 能力夹紧 ▸ L2 会话档位 ▸ L1 预设默认。
        // 注意"夹紧"：模型硬约束（不支持思考 / 强制思考）会无视会话与覆盖的相反意愿，
        // 但**不回写** sessions.thinking_mode（不变量 I3），用户表态原样保留。
        //
        // 本轮所有 loop 都用同一个值：中途翻转会让 Anthropic 协议的 thinking 链要求不成立
        //（首轮 disabled 就没有 thinking 块可回传，第二轮突然 enabled 会 400）。
        let override_val = self.thinking_override.take();
        let model_caps = crate::infra::llm::registry::query_capabilities(&self.model_id);
        let decision = crate::core::session::thinking::decide(
            override_val,
            self.session_think_mode,
            model_caps.as_ref(),
        );
        self.turn_thinking_decision = decision.clone();
        let turn_think = decision.enabled;
        self.turn_think = turn_think;
        self.should_think = turn_think;
        // 供子 Agent 继承（设计文档 K3）：主 Agent 本轮用什么档位，本轮派生的子 Agent 就用什么
        *self.ctx.turn_think.lock().await = Some(turn_think);
        println!(
            "[JARVIS] 本轮 thinking 状态固定为 {}（override={:?}, 会话档位={}, 裁决={:?}）",
            if turn_think { "enabled" } else { "disabled" },
            override_val,
            self.session_think_mode.0,
            decision.reason
        );

        let user_message_id = {
            let session = self.ctx.memory.lock().await;
            session.message_ids.get(self.initial_msg_index).cloned()
        };
        println!(
            "[JARVIS] start_run: message_id={:?} initial_msg_index={}",
            user_message_id, self.initial_msg_index
        );
        // 步骤 6：在 agent_runs 表登记本次 run（实时进度 + 崩溃恢复用）
        // （不再传用户消息文本：它只曾用于 v19 删掉的 user_message_preview 列）
        //
        // 先清空等待区间：区间表挂在**会话**上，上一轮若没被收尾取走（例如异常退出
        // 导致收尾函数没跑到），残留区间会被算进本轮的 `wait_ms`，让"人类确认耗时"虚高。
        self.ctx.clear_run_wait().await;
        self.run_id = agent_runs::start_run(&self.app, &self.sid, None, user_message_id);
        // 步骤 7：标记当前 run 为活跃。
        //
        // v15 起这里**不再**落 checkpoint：崩溃重建的唯一数据源是「每轮一行」的
        // `agent_run_events`，而本轮还没有任何 loop 走完，落一条空 checkpoint
        // 只会在恢复时重建出空内容（→ NeedsClosure 收口），没有信息增量。
        *self.ctx.active_run_id.lock().await = Some(self.run_id.clone());
    }
}
