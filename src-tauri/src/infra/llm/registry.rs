// --- 模型能力注册表 ---
// 从 model_registry.json 编译时内嵌，提供模型能力查询接口。
// 使用 include_str!() 宏确保数据随二进制一起打包，无需运行时外部文件。

use serde::{Deserialize, Serialize};

/// 单个模型的能力描述
#[derive(Serialize, Deserialize, Debug, Clone)]
#[serde(rename_all = "camelCase")]
pub struct ModelCapabilities {
    /// 是否支持流式输出
    pub streaming: bool,
    /// 是否支持深度思考模式（可通过参数控制）
    pub thinking: bool,
    /// 控制思考的参数名。取值与 `apply_thinking_for_model()` 的分支一一对应：
    ///
    /// - `"reasoning_effort"` —— 顶层 `reasoning_effort`（OpenAI / Kimi / 小米旧写法）
    /// - `"thinking"`         —— `{type, budget_tokens}`（DeepSeek、Anthropic 老形态）
    /// - `"thinking_type"`    —— **只发** `{type}`，不带 `budget_tokens`
    ///   （智谱 GLM-5.x、小米 MiMo-V2.5：官方只定义 type）
    /// - `"thinkingBudget"`   —— Gemini 2.x 的 `thinking_budget` 整数
    /// - `"enable_thinking"`  —— 顶层布尔
    /// - `"extra_thinking"` / `"extra_enable_thinking"` —— `extra_body` 里的布尔
    /// - `"extra_chain_of_thought"` —— `extra_body:{chain_of_thought}`
    /// - `"thinking_enable"`  —— `{enable}`（腾讯混元）
    /// - `"enable_thought"`   —— `parameters:{enable_thought}`（MiniMax）
    #[serde(skip_serializing_if = "Option::is_none")]
    pub thinking_param: Option<String>,
    /// 是否支持温度参数（部分推理模型开启思考后不可调）
    pub temperature: bool,
    /// 是否支持视觉/多模态
    #[serde(default)]
    pub vision: bool,
    /// 最大输出 token 数
    #[serde(default)]
    pub max_tokens: u32,
    /// 最大上下文窗口 token 数
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_context_tokens: Option<u32>,
    /// 备注说明
    #[serde(default)]
    pub notes: String,
    /// 是否强制开启思考（用户不可关闭，适用于 thinking + tool_calls 必须共存的模型如 DeepSeek）
    #[serde(default)]
    pub thinking_forced: bool,
    /// 缓存命中字段的写法（**可选覆盖**，省略 = 运行时按候选表自动探测）：
    /// `"deepseek" | "nested" | "flat" | "anthropic" | "none"`
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cache_usage_style: Option<String>,
    /// 该模型支持的「思考深度档位」（可选，空 = 不声明）。
    ///
    /// 两条出口共用同一份档位声明，各自映射到自己的参数名：
    /// - OpenAI 出口 → 顶层 `reasoning_effort`
    /// - Anthropic 出口 → `output_config.effort`
    ///
    /// 有了它，"关思考"才能在真正支持 `none` 的模型上写成 `none`；没有声明的
    /// 模型维持原行为（省略参数），不会引入新风险。
    #[serde(default)]
    pub thinking_effort_values: Vec<String>,
    /// **仅 Anthropic 出口**：该模型接受哪种思考形态（省略 = `"budget"`）。
    ///
    /// - `"budget"`            —— 老形态 `{type: enabled|disabled, budget_tokens}`
    /// - `"adaptive_optional"` —— 省略即不思考；开启只能传 `{type:"adaptive"}`，关闭传 `disabled`
    /// - `"adaptive_default"`  —— **省略即思考**；关闭需显式 `{type:"disabled"}`
    /// - `"adaptive_only"`     —— 恒开、**不可关闭**（传 `disabled` 会 400），只能省略或传 `adaptive`
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub thinking_mode: Option<String>,
    /// 模型生命周期状态（**可选，省略 = `"active"`**）。
    ///
    /// - `"active"`       —— 官方在售，正常可用（省略字段即此值）
    /// - `"deprecated"`   —— 官方已公告即将下线，**当前仍可调用**（只告警，不拦）
    /// - `"retired"`      —— 官方已宣布退役/关停，请求必然失败（预设层禁止保存）
    /// - `"unverifiable"` —— 官方当前目录查不到该 ID，但**无退役公告**
    /// - `"alias"`        —— 旧名，请求被路由到新模型，仍可用
    ///
    /// **`unverifiable` 不等于 `retired`。** 前者只是"我们查不到"，不能据此
    /// 禁止用户使用——这与前端 `utils/thinking.ts` 立的「未知 ≠ 不支持」是同一条
    /// 原则：把检索失败当成事实结论，会把"我们没查到"变成"用户不能用"。
    #[serde(default = "default_model_status")]
    pub status: String,
    /// 状态补充说明（**只放事实**：何时退役、依据哪份官方公告）。
    ///
    /// 刻意**不记录"建议换成哪个型号"**：建议会被时间淘汰——被建议的型号自己
    /// 以后也可能退役，届时这条提示就从"有用"变成"误导"，还得持续维护。
    /// 界面统一引导用户去厂商官方文档查最新型号。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub status_note: Option<String>,
}

/// `status` 字段缺省值：注册表未写该字段的条目一律视为在售。
///
/// 用默认值而非必填，是为了让 63 条里 37 条在售模型不必逐个补字段——
/// 注册表是编译期内嵌的，字段越少越不容易在下次维护时漏改。
fn default_model_status() -> String {
    "active".to_string()
}

/// 注册表中的单条模型记录
#[derive(Serialize, Deserialize, Debug, Clone)]
#[serde(rename_all = "camelCase")]
pub struct ModelRegistryEntry {
    /// 模型 ID（用于精确或模糊匹配）
    pub id: String,
    /// 服务商名称
    pub provider: String,
    /// 用户友好的显示名称
    pub display_name: String,
    /// 推荐的 API 格式
    pub api_format: String,
    /// 能力描述
    pub capabilities: ModelCapabilities,
}

/// 注册表根结构
#[derive(Deserialize, Debug)]
struct RegistryRoot {
    models: Vec<ModelRegistryEntry>,
}

/// 编译时内嵌注册表 JSON，确保打包后无需外部文件
const REGISTRY_JSON: &str = include_str!("../../../model_registry.json");

/// 加载全量注册表列表
pub fn load_registry() -> Vec<ModelRegistryEntry> {
    match serde_json::from_str::<RegistryRoot>(REGISTRY_JSON) {
        Ok(root) => root.models,
        Err(e) => {
            // 编译时内嵌，理论上不会失败；若失败则说明 JSON 格式错误
            eprintln!("[Registry] 解析 model_registry.json 失败: {}", e);
            vec![]
        }
    }
}

/// 按模型 ID 查询能力（精确匹配优先，后降级为前缀模糊匹配）
///
/// 匹配策略：
/// 1. 精确匹配 id
/// 2. 注册表中的 id 是用户输入的前缀（如 "deepseek-v4" 匹配 "deepseek-v4-pro"）
/// 3. 用户输入是注册表 id 的前缀
pub fn query_capabilities(model_id: &str) -> Option<ModelCapabilities> {
    let models = load_registry();
    let lower = model_id.to_lowercase();

    // 1. 精确匹配
    if let Some(entry) = models.iter().find(|m| m.id.to_lowercase() == lower) {
        return Some(entry.capabilities.clone());
    }

    // 2. 模糊匹配：注册表 id 包含用户输入，或用户输入包含注册表 id
    let fuzzy = models.iter().find(|m| {
        let mid = m.id.to_lowercase();
        lower.contains(&mid) || mid.contains(&lower)
    });
    fuzzy.map(|e| e.capabilities.clone())
}

/// Tauri 命令：前端查询指定模型的能力
/// 返回 Option<ModelCapabilities>，None 表示注册表中无此模型
#[tauri::command]
pub fn get_model_capabilities(model_id: String) -> Option<ModelCapabilities> {
    query_capabilities(&model_id)
}

/// 缓存命中字段写法的注册表覆盖（`cacheUsageStyle`）；None = 交给运行时自动探测
pub fn cache_usage_style_for(model_id: &str) -> Option<String> {
    query_capabilities(model_id).and_then(|caps| caps.cache_usage_style)
}

/// 模型生命周期状态；**注册表未收录时返回 `None`**。
///
/// 返回 `None` 的语义是"我们不知道"，调用方**不要**把它当成 `"retired"`——
/// 见 `ModelCapabilities::status` 的文档。
pub fn model_status(model_id: &str) -> Option<String> {
    query_capabilities(model_id).map(|caps| caps.status)
}

/// 该模型是否已被官方宣布退役（请求必然失败）。
///
/// 只认 `"retired"`：`"deprecated"`（即将下线但仍可调用）、`"unverifiable"`
/// （查不到但无退役公告）、`"alias"`（旧名路由，仍可用）都不算。
pub fn is_retired(model_id: &str) -> bool {
    model_status(model_id).as_deref() == Some("retired")
}

/// 该模型是否接受 `temperature` / `top_p` / `top_k` 三个采样参数。
///
/// 注册表里 `temperature: false` 就是"不接受" —— Anthropic 自 Opus 4.7 起、
/// 以及 Opus 5 / Sonnet 5 / Fable 5 全系已废弃这三个参数，**传了直接 400**。
/// 改造前这个标志全仓没有任何读取点，用户只要在预设里调过温度就会踩到。
/// 无注册表条目时保守放行（自定义端点不该被我们擅自剥参数）。
pub fn supports_sampling_params(model_id: &str) -> bool {
    query_capabilities(model_id)
        .map(|caps| caps.temperature)
        .unwrap_or(true)
}

/// effort 档位的相对高低；未知档位返回 None。
fn effort_rank(level: &str) -> Option<u8> {
    match level {
        "none" => Some(0),
        "low" => Some(1),
        "medium" => Some(2),
        "high" => Some(3),
        "xhigh" => Some(4),
        "max" => Some(5),
        _ => None,
    }
}

/// 在模型声明的档位集合里挑一个可用档位：期望值优先，否则兜底到「不高于 high 的最高档」，
/// 再兜底取集合首项。
///
/// 集合为空表示模型没声明档位 → 返回 `None`，调用方**不要下发该参数**
/// （对不认识该字段的模型下发 effort 会直接 400）。
fn resolve_effort(want: Option<&str>, levels: &[String]) -> Option<String> {
    if levels.is_empty() {
        return None;
    }
    if let Some(w) = want {
        if levels.iter().any(|level| level == w) {
            return Some(w.to_string());
        }
    }
    let high_rank = effort_rank("high").unwrap_or(3);
    let mut best: Option<(u8, &str)> = None;
    for level in levels {
        if let Some(rank) = effort_rank(level) {
            if rank <= high_rank && best.map_or(true, |(current, _)| rank > current) {
                best = Some((rank, level.as_str()));
            }
        }
    }
    best.or_else(|| levels.first().map(|level| (0, level.as_str())))
        .map(|(_, level)| level.to_string())
}

/// Anthropic 出口的思考参数计划。
#[derive(Debug, Clone, Default)]
pub struct AnthropicThinkingPlan {
    /// 要写进请求体的 `thinking` 字段（`None` = 完全不发该字段）
    pub thinking: Option<crate::infra::types::models::ThinkingConfig>,
    /// 要写进请求体的 `output_config`（`None` = 不发）
    pub output_config: Option<serde_json::Value>,
}

impl AnthropicThinkingPlan {
    /// 本次请求会不会真的产生思考内容。
    ///
    /// 不能只看 `should_think`：`adaptive_only` 这类模型无法关闭，调用方传 false
    /// 我们照样得把思考开着。
    pub fn thinking_active(&self) -> bool {
        match self.thinking.as_ref().and_then(|t| t.r#type.as_deref()) {
            Some("disabled") | None => false,
            Some(_) => true,
        }
    }
}

/// **Anthropic 出口思考参数的唯一决策口。**
///
/// 原先这段逻辑在主 Agent、子代理、Anthropic provider 三处各硬编码了一份
/// `{type: enabled|disabled, budget_tokens: 1024}`，而那套形态在 Opus 4.7 及之后的
/// 模型上已被移除（传 `enabled` 直接 400），Fable 5 系更是连 `disabled` 都拒 —— 
/// 结果是"注册表里写着支持思考，直连却必然报错"。
///
/// 形态由注册表的 `thinkingMode` 决定，档位由 `thinkingEffortValues` 决定；
/// 两个字段都没声明的模型回落到老形态，行为与改造前完全一致。
///
/// `effort_hint` 是调用方期望的档位（当前生产路径传 `None` = 用模型默认）；
/// 未声明档位的模型不会下发 `output_config`。
pub fn plan_anthropic_thinking(
    model_id: &str,
    should_think: bool,
    effort_hint: Option<&str>,
) -> AnthropicThinkingPlan {
    let Some(caps) = query_capabilities(model_id) else {
        return AnthropicThinkingPlan::default();
    };
    if !caps.thinking {
        return AnthropicThinkingPlan::default();
    }
    // 与 OpenAI 出口同一道兜底：强制思考的模型无视调用方传进来的 false。
    let should_think = should_think || caps.thinking_forced;

    // `output_config` 是 Anthropic 新版专有字段，**只在 adaptive 形态下才发**。
    //
    // 不能只看 `thinkingEffortValues` 是否非空 —— DeepSeek 这类 OpenAI 家族模型也
    // 声明了档位，那是给顶层 `reasoning_effort` 用的；它们走 /anthropic 兼容端点时
    // 收到 `output_config` 大概率直接 400。老 `budget` 形态同理：那一代模型不认识它。
    let effort = match caps.thinking_mode.as_deref() {
        Some("adaptive_only") | Some("adaptive_default") | Some("adaptive_optional") => {
            resolve_effort(effort_hint, &caps.thinking_effort_values)
        }
        _ => None,
    };
    let output_config = effort.map(|effort| serde_json::json!({ "effort": effort }));

    // adaptive 形态必须显式要可读文本：自 Opus 4.7 起默认 omitted，
    // 不发 display 的话思考块只有空串加签名，`delta["thinking"]` 恒空。
    let adaptive = || crate::infra::types::models::ThinkingConfig {
        r#type: Some("adaptive".to_string()),
        budget_tokens: None,
        enable: None,
        display: Some("summarized".to_string()),
    };
    let disabled = || crate::infra::types::models::ThinkingConfig {
        r#type: Some("disabled".to_string()),
        budget_tokens: None,
        enable: None,
        display: None,
    };

    let thinking = match caps.thinking_mode.as_deref() {
        // 恒开、不可关闭 —— 传 disabled 会 400，故 should_think=false 也照常传 adaptive。
        Some("adaptive_only") => Some(adaptive()),
        // `adaptive_optional`：省略即不思考；`adaptive_default`：省略即思考。
        // 两者「开启」的写法相同，关闭统一用 disabled。
        Some("adaptive_default") | Some("adaptive_optional") => {
            Some(if should_think { adaptive() } else { disabled() })
        }
        // 未声明 thinkingMode → 老形态，保持改造前行为。
        _ => Some(crate::infra::types::models::ThinkingConfig {
            r#type: Some(if should_think { "enabled" } else { "disabled" }.to_string()),
            budget_tokens: if should_think { Some(1024) } else { None },
            enable: None,
            display: None,
        }),
    };

    AnthropicThinkingPlan {
        thinking,
        output_config,
    }
}

/// 统一入口：根据模型注册表的 thinkingParam，向 OpenAIRequest 写入思考参数
/// should_think=true 开启，false 显式关闭。不匹配或无能力时不做任何操作。
pub fn apply_thinking_for_model(
    req: &mut crate::infra::types::models::OpenAIRequest,
    model_id: &str,
    should_think: bool,
) {
    let Some(caps) = query_capabilities(model_id) else { return };
    if !caps.thinking { return; }
    // P-DS 兜底：强制思考的模型（DeepSeek 系）无视调用方的 false。
    //
    // 这是"最后一道防线"：裁决层（core::session::thinking::decide）已经会夹紧，
    // 但这里再兜一次，保证任何未来新增的调用路径即使传错，也不会向这类模型下发
    // `thinking:{type:disabled}`（DeepSeek 要求 thinking 与 tool_calls 共存）。
    let should_think = should_think || caps.thinking_forced;
    match caps.thinking_param.as_deref() {
        Some("reasoning_effort") => {
            if should_think {
                // 档位取自注册表声明；没声明就沿用改造前的固定 "high"。
                req.reasoning_effort = resolve_effort(None, &caps.thinking_effort_values)
                    .or_else(|| Some("high".to_string()));
            } else if caps.thinking_effort_values.iter().any(|l| l == "none") {
                // **省略 ≠ 关闭**：多数推理模型省略 effort 会走自己的默认档
                // （GPT-5.6 文档口径是 medium），所以"点了关思考"其实没关。
                // 只有模型明确声明支持 `none` 时才真写 none；没声明的维持原样
                // （写出去可能直接 400）。
                req.reasoning_effort = Some("none".to_string());
            }
        }
        Some("thinking") => {
            // 保留调用方已设置的 budget_tokens（build_llm_request 会预设）
            let existing_budget = req.thinking.as_ref().and_then(|t| t.budget_tokens);
            req.thinking = Some(crate::infra::types::models::ThinkingConfig {
                r#type: Some(if should_think { "enabled" } else { "disabled" }.to_string()),
                budget_tokens: if should_think { existing_budget.or(Some(1024)) } else { None },
                enable: None,
                display: None,
            });
        }
        // 只下发 `{type: enabled|disabled}`、**不带 budget_tokens** 的形态。
        //
        // 智谱 GLM-5.x 与小米 MiMo-V2.5 的官方文档只定义 `thinking.type`，
        // 从未定义 `budget_tokens`。而上面 `Some("thinking")` 那条通道总会补一个
        // 1024 —— 那是 DeepSeek / Anthropic 的字段。多带一个上游未定义的字段有
        // 400 风险，所以给这类"只认 type"的模型单开一条通道。
        Some("thinking_type") => {
            req.thinking = Some(crate::infra::types::models::ThinkingConfig {
                r#type: Some(if should_think { "enabled" } else { "disabled" }.to_string()),
                budget_tokens: None,
                enable: None,
                display: None,
            });
        }
        Some("thinkingBudget") => {
            req.thinking_budget = Some(if should_think { 8192 } else { 0 });
        }
        Some("enable_thinking") => {
            req.enable_thinking = Some(should_think);
        }
        Some("extra_thinking") | Some("extra_enable_thinking") => {
            let key = if caps.thinking_param.as_deref() == Some("extra_enable_thinking") {
                "enable_thinking"
            } else {
                "thinking"
            };
            req.extra_body = Some(serde_json::json!({ key: should_think }));
        }
        Some("extra_chain_of_thought") => {
            req.extra_body = Some(serde_json::json!({ "chain_of_thought": should_think }));
        }
        Some("thinking_enable") => {
            let existing_budget = req.thinking.as_ref().and_then(|t| t.budget_tokens);
            req.thinking = Some(crate::infra::types::models::ThinkingConfig {
                r#type: None,
                budget_tokens: if should_think { existing_budget.or(Some(1024)) } else { None },
                enable: Some(should_think),
                display: None,
            });
        }
        Some("enable_thought") => {
            req.parameters = Some(serde_json::json!({ "enable_thought": should_think }));
        }
        _ => {
            if should_think {
                req.reasoning_effort = Some("high".to_string());
            }
        }
    }
}

/// Tauri 命令：返回完整的注册表列表（用于前端下拉选择）
#[tauri::command]
pub fn list_model_registry() -> Vec<ModelRegistryEntry> {
    load_registry()
}

#[cfg(test)]
mod anthropic_thinking_tests {
    //! 锁住「注册表驱动的思考参数形态」。
    //!
    //! 这些断言全部依赖真实 `model_registry.json`，改表即会被测到 —— 这正是目的：
    //! 形态错了下游是 HTTP 400，本地没有任何其它手段能提前发现（没有 key 就更不能）。

    use super::*;
    use crate::infra::types::models::OpenAIRequest;

    fn thinking_type(plan: &AnthropicThinkingPlan) -> Option<String> {
        plan.thinking.as_ref().and_then(|t| t.r#type.clone())
    }

    fn thinking_display(plan: &AnthropicThinkingPlan) -> Option<String> {
        plan.thinking.as_ref().and_then(|t| t.display.clone())
    }

    fn effort_of(plan: &AnthropicThinkingPlan) -> Option<String> {
        plan.output_config
            .as_ref()
            .and_then(|v| v.get("effort"))
            .and_then(|v| v.as_str())
            .map(|s| s.to_string())
    }

    fn openai_req(model: &str) -> OpenAIRequest {
        OpenAIRequest {
            model: model.to_string(),
            max_tokens: Some(8192),
            messages: vec![],
            tools: None,
            stream: true,
            stream_options: None,
            reasoning_effort: None,
            thinking: None,
            thinking_budget: None,
            enable_thinking: None,
            extra_body: None,
            parameters: None,
            temperature: None,
            top_p: None,
        }
    }

    #[test]
    fn fable_5_1_is_always_adaptive_and_never_disabled() {
        for should_think in [true, false] {
            let plan = plan_anthropic_thinking("claude-fable-5-1", should_think, None);
            assert_eq!(
                thinking_type(&plan).as_deref(),
                Some("adaptive"),
                "Fable 5.1 传 enabled / disabled 都会 400，任何情况都只能是 adaptive"
            );
            assert!(plan.thinking_active(), "这类模型恒开，调用方传 false 也关不掉");
            assert_eq!(
                thinking_display(&plan).as_deref(),
                Some("summarized"),
                "4.7 起默认 omitted，不要 display 的话思考块是空的、前端思考区全白"
            );
            assert_eq!(effort_of(&plan).as_deref(), Some("high"));
        }
    }

    #[test]
    fn opus_5_omitted_means_on_and_disable_uses_disabled() {
        let on = plan_anthropic_thinking("claude-opus-5", true, None);
        assert_eq!(thinking_type(&on).as_deref(), Some("adaptive"));
        assert!(on.thinking_active());

        let off = plan_anthropic_thinking("claude-opus-5", false, None);
        assert_eq!(thinking_type(&off).as_deref(), Some("disabled"));
        assert!(!off.thinking_active());
    }

    #[test]
    fn opus_4_7_must_not_send_legacy_enabled() {
        let plan = plan_anthropic_thinking("claude-opus-4-7", true, None);
        assert_eq!(
            thinking_type(&plan).as_deref(),
            Some("adaptive"),
            "Opus 4.7 已移除 type=enabled，发了就是 400（改造前一直发）"
        );
        assert!(plan
            .thinking
            .as_ref()
            .and_then(|t| t.budget_tokens)
            .is_none());
    }

    #[test]
    fn legacy_claude_keeps_budget_shape() {
        let plan = plan_anthropic_thinking("claude-opus-4-5", true, None);
        assert_eq!(
            thinking_type(&plan).as_deref(),
            Some("enabled"),
            "没声明 thinkingMode 的模型必须保持改造前行为"
        );
        assert_eq!(
            plan.thinking.as_ref().and_then(|t| t.budget_tokens),
            Some(1024)
        );
        assert!(
            plan.output_config.is_none(),
            "未声明档位的模型不能收到 output_config"
        );
        assert!(thinking_display(&plan).is_none());
    }

    #[test]
    fn unsupported_effort_level_is_clamped() {
        // Sonnet 4.6 支持 low/medium/high/max，但**不在**官方 effort 页的 xhigh 可用清单里。
        let plan = plan_anthropic_thinking("claude-sonnet-4-6", true, Some("xhigh"));
        assert_eq!(
            effort_of(&plan).as_deref(),
            Some("high"),
            "Sonnet 4.6 不支持 xhigh，必须夹紧到不高于 high 的最高档"
        );

        let plan_max = plan_anthropic_thinking("claude-opus-5", true, Some("max"));
        assert_eq!(
            effort_of(&plan_max).as_deref(),
            Some("max"),
            "Opus 5 声明支持 max，应当原样保留"
        );

        // 反例：Sonnet 5 官方明确支持 xhigh（effort 页的 xhigh 可用清单含它），
        // 夹紧它就是错的——改造前注册表把它写成"不支持"，这条断言正是那处错误的守门人。
        let sonnet5 = plan_anthropic_thinking("claude-sonnet-5", true, Some("xhigh"));
        assert_eq!(
            effort_of(&sonnet5).as_deref(),
            Some("xhigh"),
            "官方 effort 页把 Sonnet 5 列进 xhigh 可用清单，不得夹紧"
        );
    }

    #[test]
    fn thinking_type_channel_omits_budget_tokens() {
        // 智谱 GLM-5.x 与小米 MiMo-V2.5 的官方文档只定义 `thinking.type`，
        // 从未定义 `budget_tokens`——多带一个上游不认识的字段有 400 风险。
        for model in ["glm-5.1", "mimo-v2.5-pro"] {
            let mut on = openai_req(model);
            apply_thinking_for_model(&mut on, model, true);
            let thinking = on.thinking.as_ref().unwrap_or_else(|| panic!("「{}」应写入 thinking", model));
            assert_eq!(thinking.r#type.as_deref(), Some("enabled"));
            assert_eq!(
                thinking.budget_tokens, None,
                "「{}」的官方文档未定义 budget_tokens，不能多带",
                model
            );

            let mut off = openai_req(model);
            apply_thinking_for_model(&mut off, model, false);
            let thinking = off.thinking.as_ref().unwrap_or_else(|| panic!("「{}」应写入 thinking", model));
            assert_eq!(thinking.r#type.as_deref(), Some("disabled"));
            assert_eq!(thinking.budget_tokens, None);
        }
    }

    #[test]
    fn deepseek_still_gets_budget_tokens() {
        // 反向守门：`thinking_type` 不能把老通道顺手改掉。
        // DeepSeek 的 `thinking` 通道一直带 budget_tokens:1024，行为必须不变。
        let mut req = openai_req("deepseek-flash");
        apply_thinking_for_model(&mut req, "deepseek-flash", true);
        assert_eq!(
            req.thinking.as_ref().and_then(|t| t.budget_tokens),
            Some(1024),
            "DeepSeek 走的仍是带 budget_tokens 的老通道"
        );
    }

    #[test]
    fn output_config_only_goes_to_adaptive_models() {
        // DeepSeek 也声明了档位，但那是给顶层 reasoning_effort 的；
        // 它走 /anthropic 兼容端点时不认识 output_config，发了就是 400。
        let plan = plan_anthropic_thinking("deepseek-v4-pro", true, None);
        assert!(plan.output_config.is_none());
        assert_eq!(
            thinking_type(&plan).as_deref(),
            Some("enabled"),
            "它没声明 thinkingMode，必须保持老的 budget 形态"
        );

        let adaptive = plan_anthropic_thinking("claude-opus-5", true, None);
        assert!(adaptive.output_config.is_some());
    }

    #[test]
    fn wire_shape_is_exactly_what_the_api_expects() {
        // 光断言结构体字段不够：字段名序列化错了（比如 outputConfig / budgetTokens）
        // 照样过编译、照样过结构体断言，到了线上才是 400。
        let adaptive = plan_anthropic_thinking("claude-fable-5-1", true, None);
        assert_eq!(
            serde_json::to_value(adaptive.thinking.as_ref().unwrap()).unwrap(),
            serde_json::json!({ "type": "adaptive", "display": "summarized" }),
            "adaptive 形态只能有 type + display，带上 budget_tokens 会被拒"
        );
        assert_eq!(
            serde_json::to_value(adaptive.output_config.as_ref().unwrap()).unwrap(),
            serde_json::json!({ "effort": "high" })
        );

        let legacy = plan_anthropic_thinking("claude-opus-4-5", true, None);
        assert_eq!(
            serde_json::to_value(legacy.thinking.as_ref().unwrap()).unwrap(),
            serde_json::json!({ "type": "enabled", "budget_tokens": 1024 }),
            "老形态必须原样保留（含 budget_tokens）且不带 display"
        );
    }

    #[test]
    fn unknown_model_yields_empty_plan() {
        let plan = plan_anthropic_thinking("no-such-model-xyz", true, None);
        assert!(plan.thinking.is_none());
        assert!(plan.output_config.is_none());
        assert!(!plan.thinking_active());
    }

    #[test]
    fn openai_exit_writes_none_only_when_model_declares_it() {
        let mut req = openai_req("gpt-5.6-sol");
        apply_thinking_for_model(&mut req, "gpt-5.6-sol", false);
        assert_eq!(
            req.reasoning_effort.as_deref(),
            Some("none"),
            "GPT-5.6 省略 effort 会走自己的默认档（medium），不显式写 none 就等于没关"
        );

        let mut req2 = openai_req("gpt-5.4-pro");
        apply_thinking_for_model(&mut req2, "gpt-5.4-pro", false);
        assert!(
            req2.reasoning_effort.is_none(),
            "未声明 none 的模型维持原行为：什么都不写"
        );
    }

    #[test]
    fn sampling_params_follow_registry_flag() {
        assert!(
            !supports_sampling_params("claude-opus-4-7"),
            "4.7 起 temperature/top_p/top_k 已废弃，传了 400"
        );
        // 用**在售**模型做正向断言：原先用的是 claude-3-5-sonnet-20241022，
        // 它已退役；一旦将来把它删掉，`query_capabilities` 返回 None →
        // `unwrap_or(true)` 会让这条断言静默通过，测试退化成空断言。
        assert!(supports_sampling_params("gpt-4o"));
        assert!(
            supports_sampling_params("no-such-model-xyz"),
            "未知模型保守放行，不擅自替用户剥参数"
        );
    }
}

#[cfg(test)]
mod status_tests {
    //! 锁住「模型生命周期状态」的分类语义。
    //!
    //! 这些断言直接依赖真实 `model_registry.json`。重点不是"某模型退役了"这件事
    //! 本身（那会随官方更新变化），而是**分类边界不能被写歪**：
    //! `unverifiable` 一旦被当成 `retired`，就会把"我们没查到"变成"用户不能用"。

    use super::*;

    /// 官方已明确宣布退役/关停的条目。
    const RETIRED: &[&str] = &[
        // OpenAI：退役时间表见 developers.openai.com/api/docs/deprecations
        "o3",         // 2026-12-11 → gpt-5.6-sol
        "o4-mini",    // 2026-10-23 → gpt-5.6-terra
        "o1",         // 2026-10-23 → gpt-5.6-sol
        "o1-pro",     // 2026-10-23 → gpt-5.6-sol
        "gpt-4.5",    // 2025-07-14（gpt-4.5-preview）→ gpt-4.1
        // Anthropic：model-deprecations 页标 Retired，请求会失败
        "claude-3-7-sonnet-20250219",
        "claude-3-5-sonnet-20241022",
        "claude-3-5-haiku-20241022",
        // Google：Gemini API changelog 的 shutdown 条目
        "gemini-2.0-flash", // 2026-06-01 → gemini-3.6-flash
        "gemini-1.5-pro",   // 2025-09-29
        "gemini-1.5-flash", // 2025-09-29
        // DeepSeek：2026-07-24 停用
        "deepseek-chat",
        "deepseek-reasoner",
        // 小米：2026-06-30 下线
        "mimo-v2-flash",
        "mimo-v2-pro",
        "mimo-v2-omni",
    ];

    /// **查不到 ≠ 退役**。这些条目在官方当前目录里找不到，但官方从未公告退役，
    /// 因此只能标 `unverifiable`，不得禁止用户使用。
    const UNVERIFIABLE: &[&str] = &[
        "o4-mini-high",     // 非官方 API 模型 ID，官方无独立页面
        "qwen2.5-max",      // 官方文档 404、模型列表未列，但未找到下线公告
        "hunyuan-pro-think", // 未在腾讯任何官方清单中出现
        "abab6.5s-think",   // 不在 MiniMax 官方现行模型列表
    ];

    #[test]
    fn retired_models_are_exactly_the_expected_set() {
        let actual: Vec<String> = load_registry()
            .into_iter()
            .filter(|m| m.capabilities.status == "retired")
            .map(|m| m.id)
            .collect();

        for id in RETIRED {
            assert!(
                actual.iter().any(|a| a == id),
                "「{}」官方已宣布退役，必须标 status=retired；否则用户选中后直接吃上游错误",
                id
            );
        }
        assert_eq!(
            actual.len(),
            RETIRED.len(),
            "retired 集合多出或少了条目，实际为: {:?}",
            actual
        );
    }

    #[test]
    fn unverifiable_must_never_be_classified_as_retired() {
        for id in UNVERIFIABLE {
            let status = model_status(id)
                .unwrap_or_else(|| panic!("「{}」应有注册表条目", id));
            assert_eq!(
                status, "unverifiable",
                "「{}」只是官方目录里查不到，并无退役公告——标成 retired 会把\
                 「我们没查到」变成「用户不能用」（同 thinking.ts 的「未知 ≠ 不支持」）",
                id
            );
            assert!(
                !is_retired(id),
                "「{}」不得被 is_retired 判为真，否则预设层会误禁用户保存",
                id
            );
        }
    }

    #[test]
    fn deprecated_models_are_still_usable() {
        // 阿里云公告 2026-07-06，2026-10-10 下线。公告已发但尚未生效，
        // 当前仍可正常调用，因此只能告警、不能拦。
        for id in [
            "qwen-vl-max",
            "qwen-turbo",
            "qwen3-235b-a22b",
            "qwen3-32b",
            "qwen3-14b",
        ] {
            assert_eq!(
                model_status(id).as_deref(),
                Some("deprecated"),
                "「{}」官方已公告下线但当前仍可调用，应为 deprecated",
                id
            );
            assert!(!is_retired(id), "尚未生效的下线公告不能按 retired 拦");
        }
    }

    #[test]
    fn alias_stays_usable() {
        // 模型已退役、名字仍受理，请求由官方指定的新模型承接——旧会话引用了它也不该被拦。
        assert_eq!(
            model_status("deepseek-v4-flash").as_deref(),
            Some("alias")
        );
        assert!(!is_retired("deepseek-v4-flash"));
    }

    #[test]
    fn absent_status_field_defaults_to_active() {
        // 在售条目刻意不写 status，靠 serde 默认值兜底——省掉 37 个字段的维护成本。
        let caps = query_capabilities("gpt-4o").expect("gpt-4o 应在注册表中");
        assert_eq!(
            caps.status, "active",
            "未写 status 的条目必须被解析为 active，而不是空串或报错"
        );
        assert!(caps.status_note.is_none());
    }

    #[test]
    fn unknown_model_reports_no_status_rather_than_retired() {
        assert!(
            model_status("no-such-model-xyz").is_none(),
            "注册表未收录应返回 None（我们不知道），不能回落成 retired"
        );
        assert!(!is_retired("no-such-model-xyz"));
    }

    #[test]
    fn every_non_active_entry_explains_why() {
        // 非在售条目必须带 statusNote（事实：日期 + 依据哪份官方公告），
        // 否则界面只能说"不可用"，说不出"凭什么说它不可用"。
        //
        // 注意这里**不再**断言"迁移目标"：那个字段已被刻意删除——
        // 建议换成的型号自己以后也会退役，写死就是持续维护负担，见 status_note 的文档。
        for m in load_registry() {
            let status = m.capabilities.status.as_str();
            if status == "active" {
                assert!(
                    m.capabilities.status_note.is_none(),
                    "在售条目不该带 statusNote（它是退役/降级说明）：「{}」",
                    m.id
                );
                continue;
            }
            assert!(
                m.capabilities.status_note.is_some(),
                "「{}」标了 {} 却没写依据，界面无法向用户解释",
                m.id,
                status
            );
        }
    }
}
