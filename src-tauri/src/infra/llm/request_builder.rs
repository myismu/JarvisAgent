//! # request_builder.rs — LLM 请求体构建（模型接入层）
//!
//! 职责单一：把「已经备好的请求输入」组装成发往厂商的 wire format。
//!
//! ## 为什么在这里，而不是在 agent pipeline 里
//!
//! 构建请求体做的是**协议适配**——把内部数据结构翻译成 Anthropic / OpenAI 的
//! 报文形状。这是 transport 关注点，属于基建层；放在 core 的 pipeline 里，等于
//! 让编排层知道协议细节。
//!
//! ## 设计约定（与 `core/tools/framework/policy.rs::JudgementInput` 同源）
//!
//! `LlmRequestInput` 里**全部是「已经查明的事实」**，本模块不做 IO、不改状态。
//! 需要副作用的动作（例如更新上下文监控快照）由调用方在组装输入**之前**完成，
//! 不得混进这里——构建函数不该有副作用。
//!
//! ## 约束
//!
//! - 内部统一按 Anthropic 结构建模（`AnthropicRequest`）；OpenAI 出口再翻译
//! - 模型相关的决策（是否支持采样参数、思考参数怎么写、思考是否真的开着）
//!   一律查注册表，不在本模块里硬编码厂商判断
//! - 输出始终是始终流式（`stream: true`）的请求体

use serde_json::Value;

use crate::infra::llm::api_format::ApiFormat;
use crate::infra::llm::{adapters, registry};
use crate::infra::types::models::{AnthropicRequest, Message, OpenAIRequest, StreamOptions};

/// 构建请求体所需的全部输入。
///
/// 刻意**不接整个 pipeline 状态**：把依赖从「隐含」变成「声明」，这样
/// ① 看结构体就知道这个函数依赖什么；② 不搭 pipeline 也能测；③ 想搬走随时能搬。
#[derive(Debug, Clone)]
pub struct LlmRequestInput {
    /// 模型 ID。用于查注册表：思考参数写法、是否支持采样参数、输出预算
    pub model_id: String,
    /// 端点地址。用于判断是否为需要在请求侧回填思考链的端点（DeepSeek 这类）
    pub base_url: String,
    /// 本会话固化的出口协议（Anthropic / OpenAI），决定走哪条翻译分支
    pub api_format: ApiFormat,
    /// 系统提示词。会话内保持字节恒定，是 prompt cache 命中的前提
    pub system_prompt: String,
    /// 已备好的历史快照：内部消息已过滤、图片已恢复、残缺工具配对已修复
    pub messages: Vec<Message>,
    /// 本轮的工具 schema
    pub tools: Vec<Value>,
    /// 输出预算。已由调用方经 `context_budget::resolve_output_budget` 解析完毕
    /// （该解析含「用户配置 > 注册表 > 常量兜底」三级，属调用方的职责）
    pub max_tokens: i32,
    /// 本轮的深度思考开关
    pub should_think: bool,
    /// 采样参数。**是否真正下发由注册表决定**，此处只提供配置值
    pub temperature: Option<f32>,
    pub top_p: Option<f32>,
    pub top_k: Option<u32>,
}

/// 构建 LLM API 请求体（内部统一按 Anthropic 结构建模）
///
/// - 总是流式请求（`stream: true`），写入系统提示词、工具 schema、思考配置、温度等
/// - OpenAI 格式模型：经 adapters 翻译消息/工具，并按模型注册表注入各家「思考参数」
/// - 返回值第二项 `ApiFormat` 交给 stream.rs 与 UsageObservation，
///   协议差异在模型接入层内部消化
///
/// **纯函数**：不改入参、不读全局状态、不写日志以外的副作用。唯一的输出是返回值
/// 与一条 Anthropic 出口的调试打印。
pub fn build_request_body(input: LlmRequestInput) -> (Value, ApiFormat) {
    // 采样参数：注册表声明不接受的模型一律剥离。Anthropic 自 Opus 4.7 起、
    // 以及 Opus 5 / Sonnet 5 / Fable 5 全系已废弃 temperature/top_p/top_k，
    // 传了直接 400（改造前这个能力标志全仓没人读）。
    let sampling_ok = registry::supports_sampling_params(&input.model_id);

    let mut request_body = AnthropicRequest {
        model: input.model_id.clone(),
        max_tokens: input.max_tokens,
        system: input.system_prompt.clone(),
        messages: input.messages,
        tools: input.tools,
        stream: true,
        thinking: None,
        temperature: if sampling_ok { input.temperature } else { None },
        top_p: if sampling_ok { input.top_p } else { None },
        top_k: if sampling_ok { input.top_k } else { None },
        output_config: None,
    };

    // 思考参数走注册表统一决策（`registry::plan_anthropic_thinking`）。原先这里
    // 写死 `{type: enabled|disabled, budget_tokens: 1024}`，而该形态在 Opus 4.7
    // 及之后已被移除（Fable 5 系连 disabled 都拒），直连必然 400。
    let thinking_plan = registry::plan_anthropic_thinking(&input.model_id, input.should_think, None);
    let thinking_active = thinking_plan.thinking_active();
    request_body.thinking = thinking_plan.thinking;
    request_body.output_config = thinking_plan.output_config;

    // 用计划里的实际状态判断，而不是 `input.should_think`：`adaptive_only` 类模型
    // 无法关闭，调用方传 false 时思考依然是开着的。
    //
    // 注意：这里改的是 `request_body` 的字段，**不影响 `input.max_tokens`**。
    // OpenAI 出口下面用的是 `input.max_tokens`（未被抬过的原值），与 Anthropic
    // 出口的结果不一致。此差异为原有行为，本次重构原样保留，另案处理。
    if thinking_active && request_body.max_tokens <= 1024 {
        request_body.max_tokens = 4096;
    }

    // 出网前把内部 Context 块降级为普通 Text（协议不认 "context" 类型）
    adapters::materialize_context_blocks_for_wire(&mut request_body.messages);

    if input.api_format.is_openai() {
        let backfill_reasoning = adapters::should_backfill_deepseek_reasoning_content(
            &input.model_id,
            &input.base_url,
            input.should_think,
        );
        let openai_msgs = adapters::translate_messages_to_openai_with_reasoning_backfill(
            &request_body.system,
            &request_body.messages,
            backfill_reasoning,
        );
        let openai_tools = adapters::translate_tools_to_openai(&request_body.tools);

        // OpenAI 出口：翻译消息/工具，并按模型注册表注入该模型的思考参数
        let mut openai_req = OpenAIRequest {
            model: input.model_id.clone(),
            max_tokens: Some(input.max_tokens),
            messages: openai_msgs,
            tools: if openai_tools.is_empty() {
                None
            } else {
                Some(openai_tools)
            },
            stream: true,
            stream_options: Some(StreamOptions {
                include_usage: true,
            }),
            reasoning_effort: None,
            thinking: None,
            thinking_budget: None,
            enable_thinking: None,
            extra_body: None,
            parameters: None,
            temperature: request_body.temperature,
            top_p: request_body.top_p,
        };

        registry::apply_thinking_for_model(&mut openai_req, &input.model_id, input.should_think);
        (
            serde_json::to_value(openai_req).unwrap(),
            ApiFormat::OpenAI,
        )
    } else {
        // Anthropic 出口的 thinking 块策略按服务商分两种：
        // - 真 Anthropic：无 signature 的 thinking 回传会被判 400 → 必须剥掉；
        // - DeepSeek 这类端点：思考模式下**要求**把 thinking 原样带回，剥掉会报
        //   `content[].thinking in the thinking mode must be passed back to the API`
        //   （与 OpenAI 出口的 reasoning_content 回填是同一件事的两面）。
        if adapters::should_strip_unsigned_thinking(
            &input.model_id,
            &input.base_url,
            input.should_think,
        ) {
            request_body.messages =
                adapters::strip_unsigned_thinking_for_anthropic(&request_body.messages);
        } else {
            println!(
                "[JARVIS] Anthropic 出口：保留无签名 thinking 块（{} 要求回传思考链）",
                input.model_id
            );
        }
        (
            serde_json::to_value(request_body).unwrap(),
            ApiFormat::Anthropic,
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::infra::types::models::Content;

    /// 造一份最小输入。各用例只覆盖自己关心的字段，其余走这里的默认值。
    fn input_with(model_id: &str, api_format: ApiFormat) -> LlmRequestInput {
        LlmRequestInput {
            model_id: model_id.to_string(),
            base_url: "https://example.invalid".to_string(),
            api_format,
            system_prompt: "系统提示词".to_string(),
            messages: vec![Message::User {
                content: Content::Single("你好".to_string()),
            }],
            tools: Vec::new(),
            max_tokens: 4096,
            should_think: false,
            temperature: Some(0.7),
            top_p: Some(0.9),
            top_k: Some(40),
        }
    }

    /// Anthropic 出口：返回 Anthropic 格式，且是流式、带 system 字段。
    #[test]
    fn anthropic_export_is_anthropic_format_and_streams() {
        let (body, format) = build_request_body(input_with("claude-opus-5", ApiFormat::Anthropic));

        assert_eq!(format, ApiFormat::Anthropic);
        assert_eq!(body["stream"], serde_json::json!(true));
        assert_eq!(body["system"], serde_json::json!("系统提示词"));
        assert_eq!(body["model"], serde_json::json!("claude-opus-5"));
    }

    /// OpenAI 出口：返回 OpenAI 格式，且开启 usage 上报。
    ///
    /// 流式请求下 token 用量只能靠 `stream_options.include_usage` 回来，缺了它
    /// 上下文监控就没有实测值可用（`context_budget` 的校准依赖它）。
    #[test]
    fn openai_export_is_openai_format_with_usage_enabled() {
        let (body, format) = build_request_body(input_with("deepseek-v4-pro", ApiFormat::OpenAI));

        assert_eq!(format, ApiFormat::OpenAI);
        assert_eq!(body["stream"], serde_json::json!(true));
        assert_eq!(
            body["stream_options"]["include_usage"],
            serde_json::json!(true)
        );
        // OpenAI 协议没有顶层 system，系统提示词被并进 messages
        assert!(body.get("system").is_none());
    }

    /// 注册表声明「不接受采样参数」的模型，即便输入带了也必须剥掉——传了会 400。
    #[test]
    fn sampling_params_are_stripped_for_models_that_reject_them() {
        let input = input_with("deepseek-v4-pro", ApiFormat::Anthropic);
        assert!(input.temperature.is_some(), "前提：输入确实带了采样参数");

        let (body, _) = build_request_body(input);

        assert!(body.get("temperature").is_none(), "temperature 应被剥离");
        assert!(body.get("top_p").is_none(), "top_p 应被剥离");
        assert!(body.get("top_k").is_none(), "top_k 应被剥离");
    }

    /// 注册表里查不到的模型（自定义端点）保守放行——不擅自替用户剥参数。
    ///
    /// 只断言「参数还在」，不断言具体数值：`temperature` / `top_p` 是 f32，经
    /// serde_json 序列化后呈 `0.699999988079071` 这类长尾小数（浮点精度所致，
    /// 属既有行为，非本模块引入）。
    #[test]
    fn sampling_params_survive_for_unknown_models() {
        let input = input_with("my-self-hosted-model", ApiFormat::Anthropic);
        assert!(input.temperature.is_some(), "前提：输入确实带了采样参数");

        let (body, _) = build_request_body(input);

        assert!(body.get("temperature").is_some(), "temperature 应保留");
        assert!(body.get("top_p").is_some(), "top_p 应保留");
        assert!(body.get("top_k").is_some(), "top_k 应保留");
    }
}
