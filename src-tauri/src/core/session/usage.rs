//! # usage.rs — 请求级 token 用量的**唯一写入口**
//!
//! ## 为什么要收口到一个函数
//! 一次请求的用量的落点有两处，而且**必须同时改**：
//! - `request_usage`（明细，原始凭据，可按来源/模型/run 拆）
//! - `sessions.total_*`（累计，前端概览栏直接读）
//!
//! 如果让各个调用点各自写一遍，"明细加起来 ≠ 累计值"这种账就会在没有任何报错的情况下
//! 慢慢歪掉，而且只能靠人肉对数字才发现。所以两处写入**共用一个事务**，
//! 由 [`record_request_usage`] 独占。
//!
//! ## Key Exports
//! - `record_request_usage()`: 写一行明细 + 累加会话累计值 + 推送 `session-usage-updated`
//! - `record_llm_usage()`: 辅助调用（反思 / 压缩 / 命名 / 记忆整理）的记账入口，
//!   把「厂商未上报就不写零行」这条口径收在一处
//!
//! ## Dependencies
//! - Internal: `crate::core::session::repository`（SQL 原语）、`crate::infra::db`（事务）
//! - External: `tauri`（事件推送）
//!
//! ## Constraints
//! - 不得在事务闭包里再调 `with_connection` / `with_transaction`（会自锁）；本模块只用传进来的 tx。

use tauri::Emitter;

use super::{repository, RequestUsageRecord, SessionTokenTotals, SessionUsageView, UsageBreakdown};
use crate::infra::llm::usage::LlmUsage;

/// 记录一次模型请求的用量：明细 + 累计 + 事件，三件事一步做完。
///
/// ## 返回
/// - `Ok(Some(breakdown))`：写入成功，附上**写入之后**的按来源账本（调用方一般不用，
///   但测试与调试需要它）。注意事件载荷**不是**这份账本原形，而是它的视图投影
///   （[`SessionUsageView`]），理由见下面「事件」一节。
/// - `Ok(None)`：会话不存在或已归档 —— 本次请求**不入账**（连明细也不写），
///   调用方据此跳过事件推送。口径与旧的 `accumulate_session_token_usage` 一致。
///
/// ## 事务边界
/// 插入明细与累加累计在**同一个事务**里：任何一步失败都整体回滚。
/// 否则会出现"累计加了、明细没落"（账没法拆）或"明细落了、累计没加"（总数偏小）两种残账。
///
/// ## 缓存未上报
/// 明细里保留 `NULL`（厂商未上报），但累计值按 0 加：
/// 累计列是 `NOT NULL DEFAULT 0` 的历史形态，无法表达"未上报"，
/// 而展示层判断"有没有上报"靠的是明细表的 `IS NULL`（见 `UsageBreakdown`）。
/// 因此这里 **不会** 因为缓存未上报而丢掉整条明细的输入/输出 token。
///
/// ## 事件
/// 事件在事务**提交之后**才发：订阅方收到事件后可能立刻回读数据库，
/// 提交前发等于让前端读到旧值。
///
/// 载荷是**视图投影**（[`SessionUsageView`]），与前端 `SessionUsageUpdatedPayload`
/// 严格对齐，共两组字段：
/// - 扁平四值 + 会话 id：`sessionId / inputTokens / outputTokens / cacheHitTokens /
///   cacheMissTokens`。概览栏直接读它们，语义仍是**累计绝对值**（前端整值覆盖）；
/// - 分层明细：`breakdown`，内含 `main / subagent / system` 三桶与
///   `unreportedRequestCount / unreportedInput / hasDetail`。
///
/// ### 为什么事件载荷用投影，而不是账本原形
/// 账本（[`UsageBreakdown`]）是**存储口径**：六个 `UsageSource` 平铺成 `sources`
/// 数组，字段名（`input_tokens` / `unreported_requests` …）跟着明细表走，也没有
/// 前端要画的「主对话 / 子 Agent / 系统开销」三层。若把账本原形直接推给前端，
/// 前端就得自己再做一遍改名 + 分桶 + 派生 `cacheReported`，于是同一套归一化逻辑
/// 存在两份（后端一份给命令回包、前端一份给事件监听）。两处一旦漂移不会报错：
/// 界面上的三层之和与总计对不上，只能靠人肉对数字发现。
/// 所以折叠只在后端做一份（[`SessionUsageView::from_breakdown`]），事件只传结果；
/// 前端拿到什么就画什么，不做二次聚合。
/// 账本原形仍通过命令 `get_session_usage_breakdown` 对外提供——那是**审计口径**
/// （要能追溯到具体来源行），与展示投影是两条互不干扰的出口。
pub fn record_request_usage(
    app: &tauri::AppHandle,
    record: RequestUsageRecord,
) -> Result<Option<UsageBreakdown>, String> {
    let session_id = record.session_id.clone();

    // 累计值的四个分量：缓存 `None` 按 0 累加（理由见函数文档）
    let delta = SessionTokenTotals {
        input: record.input_tokens,
        output: record.output_tokens,
        cache_hit: record.cache_hit_tokens.unwrap_or(0),
        cache_miss: record.cache_miss_tokens.unwrap_or(0),
    };

    let outcome = crate::infra::db::with_transaction(|tx| {
        // 归档会话不再记账：归档可撤销，但归档期间发生的请求不应该改归档区里的旧账。
        // 会话根本不存在时同样直接返回 None（外键也会拦，但错误信息不如这里明确）。
        if !repository::session_usage_writable_in(tx, &session_id)? {
            return Ok(None);
        }

        repository::insert_request_usage_in(tx, &record)?;

        // `add_session_token_totals_in` 自带 `deleted_at IS NULL` 条件，返回 None 表示
        // 会话在这一瞬间被归档/删除。全库只有一把写连接，实际走不到；真走到了就整体回滚，
        // 宁可丢这一条账，也不留下"明细有、累计没有"的半截状态。
        if repository::add_session_token_totals_in(tx, &session_id, delta)?.is_none() {
            return Ok(None);
        }

        repository::session_usage_breakdown_in(tx, &session_id).map(Some)
    })?;

    let Some(breakdown) = outcome else {
        return Ok(None);
    };

    // 提交之后再推送。序列化失败只是没有事件（前端下次拉取/下一轮事件会补上），
    // 不能让一个展示层问题把已经落库的账回滚掉。
    //
    // 推送的是**视图投影**而不是账本原形：前端消费形状（三层 + 扁平四值）已定，
    // 折叠与改名在后端只做一份，避免前后端各归一化一遍而悄悄漂移。
    // `from_breakdown` 是纯折叠，不会失败，故不必为它单独兜错。
    let view = SessionUsageView::from_breakdown(&breakdown);
    if let Ok(payload) = serde_json::to_value(&view) {
        let _ = app.emit("session-usage-updated", payload);
    }

    Ok(Some(breakdown))
}

/// 这次辅助调用的用量**够不够格**写一行明细（纯函数，便于单测）。
///
/// 两种情况不入账，理由都不是"省事"而是"写进去会误导"：
/// - `None`：厂商没上报 `usage`；
/// - 输入与输出都是 0：同上（空 `usage` 对象也会走到这里），
///   写一行 0/0 只会把请求次数抬高，对账本没有信息量。
fn should_record_llm_usage(usage: Option<&LlmUsage>) -> bool {
    match usage {
        None => false,
        Some(usage) => usage.input_tokens != 0 || usage.output_tokens != 0,
    }
}

/// 记录一次**辅助调用**的用量：反思审查 / 上下文压缩 / 会话自动命名 / 记忆整理。
///
/// ## 为什么单独开一个入口
/// 这四类请求不是「主循环的一次模型请求」，但**确实在烧 token**：
/// 不落明细，它们在账面上就是零消耗，`request_usage` 按来源拆分时
/// 永远看不到反思/压缩/命名/记忆整理这几笔开销。
///
/// 它们与主循环的差别只在读数来源：主链路是流式（`StreamResult` 已归一），
/// 这里是非流式响应的 `usage` 快照 [`LlmUsage`]。差异被收在本函数里：
/// 调用方只负责给出「哪次请求、什么来源、哪个模型」，token 与缓存读数从这里灌进去。
///
/// ## 为什么不写零行
/// `LlmUsage` 的 `None` 语义是"厂家这次没上报"，与 `Some(0)`（报了，就是 0）是两件事。
/// 如果每个调用点各自 `if let Some(usage)` 再自己拼 `tokens()`，迟早会冒出一个
/// 把 `None` 当 0 写进明细的调用点，而那一行会让"这次请求零消耗"变成账面事实。
/// 因此"哪些情况不入账"钉死在 [`should_record_llm_usage`] 一处。
///
/// ## 返回与失败处理
/// - `Ok(true)`：已写入（明细 + 累计同事务）；
/// - `Ok(false)`：按上面的规则跳过，或会话已归档/不存在；
/// - `Err`：由调用方记日志。**辅助调用记账失败不得打断主流程**，
///   账本问题不应该把正在跑的任务打吹。
///
/// 事务边界与 [`record_request_usage`] 完全一致：一次短事务，写完即释放写连接，
/// 不会因为一次后台辅助调用把库锁长时间占住。
pub fn record_llm_usage(
    app: &tauri::AppHandle,
    mut record: RequestUsageRecord,
    usage: Option<&LlmUsage>,
) -> Result<bool, String> {
    if !should_record_llm_usage(usage) {
        return Ok(false);
    }
    // `should_record_llm_usage` 已排除 None，这里的 expect 不会触发；
    // 仍写成解构形式而不是 unwrap，是为了让"None 提前返回"只有一处出口。
    let Some(usage) = usage else {
        return Ok(false);
    };

    record = record.tokens(usage.input_tokens, usage.output_tokens).cache(
        usage.cache_hit_tokens,
        usage.cache_miss_tokens,
        usage.cache_source.as_deref(),
    );

    // `Ok(None)` = 会话已归档：本次请求不入账（与主链路口径一致）
    record_request_usage(app, record).map(|written| written.is_some())
}

#[cfg(test)]
mod usage_tests {
    //! 这里只测**纯函数层的契约**（枚举映射、builder 语义）。
    //!
    //! `record_request_usage` 本身依赖全局 DB 单例与 `AppHandle`，无法在单元测试里跑；
    //! 它调用的三个 SQL 原语由 `repository::usage_tests` 用内存库覆盖。

    use super::super::{RequestUsageRecord, UsageBreakdown, UsageSource, UsageSourceBreakdown};
    use super::super::{SessionUsageBreakdownView, SessionUsageView};
    use super::should_record_llm_usage;
    use crate::infra::llm::usage::LlmUsage;

    /// 枚举 ↔ 字符串的两个口径（`as_str` 与 serde）必须完全一致。
    ///
    /// 前者落库、后者进事件载荷与命令回包。两套映射分叉时不会报错：
    /// 前端拿到 `"sessionName"`、库里存 `"session_name"`，分组永远匹配不上。
    #[test]
    fn usage_source_serde_matches_as_str() {
        for source in UsageSource::all() {
            let serialized = serde_json::to_value(source).expect("序列化 UsageSource");
            assert_eq!(
                serialized,
                serde_json::Value::String(source.as_str().to_string()),
                "serde 与 as_str 必须给出同一个字符串（{:?}）",
                source
            );
            assert_eq!(
                UsageSource::from_str(source.as_str()),
                Ok(source),
                "as_str / from_str 必须一一对应"
            );
        }
    }

    /// 未知取值必须报错，不得静默降级成 `main`。
    #[test]
    fn unknown_source_is_rejected() {
        let err = UsageSource::from_str("Main").expect_err("大小写不同也是未知取值");
        assert!(err.contains("Main"), "错误信息要带上原始取值便于排查");
    }

    /// 缓存读数成对：只给一个，按“未上报”处理（两列都 NULL）。
    ///
    /// 半截数据不拿来算命中率——错误的命中率比“未报告”更误导人。
    #[test]
    fn cache_builder_requires_both_values() {
        let half = RequestUsageRecord::new("s1", UsageSource::Main).cache(Some(10), None, None);
        assert_eq!(half.cache_hit_tokens, None);
        assert_eq!(half.cache_miss_tokens, None);
        assert_eq!(half.cache_source, None);

        // 上报了 0 也必须原样保留（Some(0) 与 None 是两件事）
        let zero = RequestUsageRecord::new("s1", UsageSource::Main)
            .cache(Some(0), Some(100), Some("deepseek"));
        assert_eq!(zero.cache_hit_tokens, Some(0));
        assert_eq!(zero.cache_miss_tokens, Some(100));
        assert_eq!(zero.cache_source.as_deref(), Some("deepseek"));
    }

    /// builder 只写调用方明确设置的字段，其余保持“未知”。
    #[test]
    fn builder_leaves_optional_fields_empty() {
        let record = RequestUsageRecord::new("s1", UsageSource::Subagent)
            .run("run-1", Some(3))
            .parent_tool_use_id("toolu_1")
            .model_id("deepseek-chat")
            .tokens(120, 8);
        assert_eq!(record.session_id, "s1");
        assert_eq!(record.source, UsageSource::Subagent);
        assert_eq!(record.run_id.as_deref(), Some("run-1"));
        assert_eq!(record.loop_index, Some(3));
        assert_eq!(record.agent_role, None);
        assert_eq!(record.task_id, None);
        assert_eq!(record.created_at, None);
        assert_eq!((record.input_tokens, record.output_tokens), (120, 8));
    }

    /// 「未上报」与「上报了 0」必须分开：前者不写行，后者（输入或输出有值）照写。
    ///
    /// 这条是辅助调用记账的核心口径：反思/压缩的 usage 缺失很常见（中转站不报），
    /// 一旦把它写成 0/0 明细，按来源拆账时会出现一堆"花了 0 token"的请求。
    #[test]
    fn auxiliary_usage_is_bookable_only_when_reported() {
        assert!(!should_record_llm_usage(None), "厂商未上报不得写零行");
        assert!(
            !should_record_llm_usage(Some(&LlmUsage::default())),
            "空 usage（0/0）与未上报同义，同样不写"
        );
        assert!(should_record_llm_usage(Some(&LlmUsage {
            input_tokens: 1726,
            output_tokens: 0,
            ..Default::default()
        })));
        assert!(should_record_llm_usage(Some(&LlmUsage {
            input_tokens: 0,
            output_tokens: 24,
            ..Default::default()
        })));
    }

    // ========================================================================
    // 账本 → 视图的投影（`SessionUsageView::from_breakdown`）
    //
    // 这些用例都在纯函数层跑：折叠是**加法与改名**，不需要数据库。
    // 要盯住的只有一件事——投影不能丢账，也不能凭空造账。
    // ========================================================================

    /// 构造一段来源账（测试用，字段顺序固定，避免各处手写漏字段）。
    #[allow(clippy::too_many_arguments)]
    fn source_row(
        source: UsageSource,
        request_count: u64,
        input: u64,
        output: u64,
        cache_hit: u64,
        cache_miss: u64,
        unreported_requests: u64,
        unreported_input: u64,
    ) -> UsageSourceBreakdown {
        UsageSourceBreakdown {
            source,
            request_count,
            input_tokens: input,
            output_tokens: output,
            cache_hit_tokens: cache_hit,
            cache_miss_tokens: cache_miss,
            unreported_requests,
            unreported_input_tokens: unreported_input,
        }
    }

    /// 六个来源齐全时，三层折叠之和必须**分毫不差**地等于账本总计。
    ///
    /// 这是投影层唯一不能破的契约：折叠一旦少算一个来源（例如忘了把
    /// `MemoryCurator` 归进 `system`），界面上三层加起来就会比总数少一截，
    /// 而这种缺失不会报错，只能靠对数字发现。
    #[test]
    fn view_folds_all_six_sources_into_three_layers() {
        let sources = vec![
            source_row(UsageSource::Main, 3, 1_000, 200, 800, 100, 0, 0),
            source_row(UsageSource::Subagent, 2, 400, 90, 300, 50, 1, 100),
            source_row(UsageSource::Reflection, 1, 50, 10, 0, 0, 1, 50),
            source_row(UsageSource::Compact, 1, 70, 20, 40, 10, 0, 0),
            source_row(UsageSource::SessionName, 1, 30, 5, 0, 0, 1, 30),
            source_row(UsageSource::MemoryCurator, 1, 20, 8, 10, 5, 0, 0),
        ];

        // 总计按来源逐列累加（等价于 SQL 的 SUM，但这里手工写清每个数来自哪一行）
        let breakdown = UsageBreakdown {
            session_id: "s-fold".to_string(),
            input_tokens: 1_570,
            output_tokens: 333,
            cache_hit_tokens: 1_150,
            cache_miss_tokens: 165,
            request_count: 9,
            unreported_requests: 3,
            unreported_input_tokens: 180,
            has_detail: true,
            sources,
        };

        let view = SessionUsageView::from_breakdown(&breakdown);

        // 总计原样透传
        assert_eq!(view.session_id, "s-fold");
        assert_eq!((view.input_tokens, view.output_tokens), (1_570, 333));
        assert_eq!((view.cache_hit_tokens, view.cache_miss_tokens), (1_150, 165));

        // 三层相加 == 总计（输入、输出两列都必须成立）
        let layers_input =
            view.breakdown.main.input + view.breakdown.subagent.input + view.breakdown.system.input;
        let layers_output = view.breakdown.main.output
            + view.breakdown.subagent.output
            + view.breakdown.system.output;
        assert_eq!(layers_input, breakdown.input_tokens, "三层输入之和必须等于账本总计");
        assert_eq!(layers_output, breakdown.output_tokens, "三层输出之和必须等于账本总计");

        // 分层归属：四个系统内部来源都进了 system（漏一个这里就会变小）
        assert_eq!(view.breakdown.main.input, 1_000);
        assert_eq!(view.breakdown.subagent.input, 400);
        assert_eq!(view.breakdown.system.input, 170);
        assert_eq!(view.breakdown.system.request_count, 4);
    }

    /// 空账本（无明细的历史会话）折叠后必须是「全零 + has_detail=false」。
    ///
    /// 关键是不能顺手把 `has_detail` 抬成 true：界面靠它决定"退回显示累计值"，
    /// 一旦变成 true，用户会看到三层 0，被读成"主对话一个 token 没花"。
    #[test]
    fn empty_breakdown_folds_to_zeroed_view_without_detail() {
        let ledger = UsageBreakdown::empty("s-empty");
        let view = SessionUsageView::from_breakdown(&ledger);

        assert!(!view.breakdown.has_detail, "无明细必须保持 has_detail=false");
        assert_eq!(view.session_id, "s-empty");
        assert_eq!(view.input_tokens, 0);
        assert_eq!(view.output_tokens, 0);

        fn assert_source_zero(src: &super::super::UsageSourceView) {
            assert_eq!(
                (
                    src.input,
                    src.output,
                    src.cache_hit,
                    src.cache_miss,
                    src.request_count,
                    src.cache_reported
                ),
                (0, 0, 0, 0, 0, 0),
                "空账本的三层必须全零"
            );
        }
        assert_source_zero(&view.breakdown.main);
        assert_source_zero(&view.breakdown.subagent);
        assert_source_zero(&view.breakdown.system);
        assert_eq!(view.breakdown.unreported_request_count, 0);
        assert_eq!(view.breakdown.unreported_input, 0);
    }

    /// 未上报字段与派生字段：`unreported*` 从账本透传，`cacheReported` 按差值推导。
    ///
    /// `cache_reported = request_count - unreported_requests` 是**推导**而非独立读数，
    /// 这里把三个数一起断言，防止将来有人另加一列 SQL 却忘了改这个换算。
    #[test]
    fn unreported_fields_pass_through_and_cache_reported_is_derived() {
        let breakdown = UsageBreakdown {
            session_id: "s-unreported".to_string(),
            input_tokens: 300,
            output_tokens: 40,
            cache_hit_tokens: 60,
            cache_miss_tokens: 20,
            request_count: 5,
            unreported_requests: 2,
            unreported_input_tokens: 220,
            has_detail: true,
            sources: vec![source_row(
                UsageSource::Main,
                5,
                300,
                40,
                60,
                20,
                2,
                220,
            )],
        };

        let view: SessionUsageView = SessionUsageView::from_breakdown(&breakdown);
        let ledger_view: &SessionUsageBreakdownView = &view.breakdown;

        // 透传：未上报两项不做任何换算
        assert_eq!(ledger_view.unreported_request_count, 2);
        assert_eq!(ledger_view.unreported_input, 220);
        assert!(ledger_view.has_detail);

        // 派生：5 次请求里有 2 次没报缓存 → 3 次上报了
        assert_eq!(ledger_view.main.request_count, 5);
        assert_eq!(ledger_view.main.cache_reported, 3);
        // 缓存读数只统计已上报的部分（未上报的不按 0 计入分母，但也不另加一条）
        assert_eq!(ledger_view.main.cache_hit, 60);
        assert_eq!(ledger_view.main.cache_miss, 20);
        // 没进 main 的桶必须干净，避免"同一行被折两次"
        assert_eq!(ledger_view.subagent.request_count, 0);
        assert_eq!(ledger_view.system.request_count, 0);
    }

    /// 事件载荷的**键名契约**：序列化结果必须与前端 `SessionUsageUpdatedPayload`
    /// （`src/types/index.ts`）逐字对齐。
    ///
    /// 这条测试防的是**跨语言改名**：Rust 侧把 `cache_hit` 改成 `cacheHitTokens`
    /// 或给视图加一层嵌套，编译器都不会拦——前端拿到 undefined，概览栏静默显示 0，
    /// 只有用户发现"数字怎么不动"才知道出事了。字段增减必须同时改这份断言与前端的 TS 接口。
    #[test]
    fn usage_view_serializes_to_frontend_payload_shape() {
        let view = SessionUsageView::from_breakdown(&UsageBreakdown::empty("s-shape"));
        let json = serde_json::to_value(&view).expect("序列化用量视图");
        let obj = json.as_object().expect("载荷顶层必须是对象");

        // 顶层：会话 id + 四个扁平累计值 + 分层明细（与前端 interface 字段顺序无关，只比集合）
        let mut keys: Vec<&str> = obj.keys().map(String::as_str).collect();
        keys.sort_unstable();
        assert_eq!(
            keys,
            vec!["breakdown", "cacheHitTokens", "cacheMissTokens", "inputTokens", "outputTokens", "sessionId"],
            "顶层键名必须与 SessionUsageUpdatedPayload 一致（camelCase）"
        );

        // 明细层：三桶 + 未上报两项 + hasDetail
        let breakdown = obj["breakdown"].as_object().expect("breakdown 必须是对象");
        let mut breakdown_keys: Vec<&str> = breakdown.keys().map(String::as_str).collect();
        breakdown_keys.sort_unstable();
        assert_eq!(
            breakdown_keys,
            vec!["hasDetail", "main", "subagent", "system", "unreportedInput", "unreportedRequestCount"],
            "明细键名必须与 SessionUsageBreakdown 一致（camelCase）"
        );

        // 单桶层：`input/output` 是改过名的字段，`cacheReported` 是派生字段
        let main = breakdown["main"].as_object().expect("main 必须是对象");
        let mut main_keys: Vec<&str> = main.keys().map(String::as_str).collect();
        main_keys.sort_unstable();
        assert_eq!(
            main_keys,
            vec!["cacheHit", "cacheMiss", "cacheReported", "input", "output", "requestCount"],
            "单桶键名必须与 UsageSourceBreakdown 一致（camelCase）"
        );
    }
}
