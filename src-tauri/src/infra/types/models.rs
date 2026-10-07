//! # models.rs — 数据模型定义模块
//!
//! 定义应用中所有核心数据结构，包括消息格式、API 请求/响应格式、会话记忆等。
//! 支持 Anthropic 和 OpenAI 两种 API 格式。
//!
//! ## 关键导出
//! - 消息格式: `Message`, `Content`, `ContentBlock`
//! - Anthropic 格式: `AnthropicRequest`, `ThinkingConfig`, `ImageSource`
//! - OpenAI 格式: `OpenAIRequest`, `OpenAIMessage`, `OpenAITool`, `OpenAIToolCall`
//! - 会话数据: `SessionMemory`, `SessionContextSnapshot`, `AgentStep`, `PlanDocument`
//! - 任务管理: `Task`, `TaskStatus`
//! - 工具定义: `Skill`
//!
//! ## 依赖
//! - Internal: 无
//! - External: `serde`, `serde_json`
//!
//! ## 约束
//! - 所有结构体必须实现 `Serialize` 和 `Deserialize`
//! - OpenAI 格式使用 `#[serde(tag = "role")]` 进行多态序列化
//! - 字段命名使用 camelCase 以匹配 JSON 格式

use serde::{Deserialize, Serialize};

#[derive(Serialize, Deserialize, Debug)]
pub struct JarvisResult {
    pub status: String,
    pub content: String,
    pub input_tokens: u64,
    pub output_tokens: u64,
    pub session_input_tokens: u64,
    pub session_output_tokens: u64,
    /// 会话累计缓存命中 / 未命中 token。
    ///
    /// 两者都为 0 表示该会话从未有请求上报缓存字段（前端据此显示 `--`，而不是 `0%`）。
    pub session_cache_hit_tokens: u64,
    pub session_cache_miss_tokens: u64,
    /// 后端为用户消息分配的 UUID，前端用于关联撤回按钮
    #[serde(skip_serializing_if = "Option::is_none")]
    pub user_message_id: Option<String>,
    /// 本轮创建的文件检查点 id（有文件编辑的轮次由 finalize 收尾时创建）。
    ///
    /// 与 `user_message_id` 同一设计动机：发消息那一刻检查点尚不存在，
    /// 靠返回值让前端实时回填撤回信息（「会话和代码撤回」立即出现），
    /// 不必等刷新走 `get_session_messages` 重查库。
    #[serde(skip_serializing_if = "Option::is_none")]
    pub checkpoint_id: Option<String>,
    /// 本轮是否有文件编辑补丁（与刷新路径 `rollback_info.has_file_edits` 同口径；
    /// 检查点创建失败时它仍为 true，前端据此展示 both 菜单）
    #[serde(skip_serializing_if = "Option::is_none")]
    pub checkpoint_has_patches: Option<bool>,
    /// break_loop 时的工具执行结果摘要（前端用于 toolBuffer，避免丢失工具执行日志）
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tool_execution_summary: Option<String>,
    /// 本轮状态标注的 **i18n key**（前端 `t(key)` 后渲染在气泡下方的小字里）。
    ///
    /// ## 为什么是 key 而不是文案
    ///
    /// 这里曾经是 `notice: Option<String>`，后端**直接写中文句子**
    /// （"用户已取消执行…" / "上游长时间未返回数据…"）。后果：界面语言切成英文后，
    /// 用户一取消生成、一遇到超时就冒出中文小字 —— 后端产出的用户可见文案
    /// **绕开了前端那套语言包**。现在后端只说"发生了什么"（key），
    /// 文案由前端出；与 `thinking_notice_i18n_key` 同一口径。
    ///
    /// ⚠️ 用词口径（写文案时仍受此约束）：只描述**可观测事实**（等待时长/用户动作），
    /// 不断言服务端状态——"上游服务已停止响应/已失联"这类措辞被明确否决
    /// （静默原因在链路上不可区分）。另注意区分两套机制：流空闲超时（stream.rs）
    /// 与 plan 模式「规划看门狗」（pipeline.rs::update_plan_watchdog）不是一回事。
    ///
    /// 与 `content` 的分工：content 是模型正文（渲染在回复气泡内），
    /// notice 是运行状态说明（渲染在气泡**下方**的小字里）。
    /// 中断类信息走 notice，避免混进正文看起来像模型自己说的话。
    #[serde(skip_serializing_if = "Option::is_none")]
    pub notice_i18n_key: Option<String>,
    /// 状态标注的补充数据（可选，**原样展示、不翻译**）。
    ///
    /// 只有一种用法：句子里需要塞入无法翻译的内容时（如具体的错误文本）。
    /// 前端以 `notice.detail` 这个 key 包一层再拼到主句后面。
    #[serde(skip_serializing_if = "Option::is_none")]
    pub notice_detail: Option<String>,
    /// 本轮最终采用的深度思考状态（由 `core::session::thinking::decide` 裁决）。
    ///
    /// 前端用它渲染开关状态与"被模型强制夹紧"的提示，**不再自行判断**。
    #[serde(skip_serializing_if = "Option::is_none")]
    pub thinking_enabled: Option<bool>,
    /// 裁决原因（`ThinkingReason` 的 Debug 名，如 `ClampedByForced`）
    #[serde(skip_serializing_if = "Option::is_none")]
    pub thinking_reason: Option<String>,
    /// 需要提示用户时的 i18n key（前端 `t(key)` 后展示）
    #[serde(skip_serializing_if = "Option::is_none")]
    pub thinking_notice_i18n_key: Option<String>,
}

#[derive(Serialize, Clone, Debug)]
pub struct ThinkingConfig {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub r#type: Option<String>, // "enabled" / "disabled" / "adaptive"
    #[serde(skip_serializing_if = "Option::is_none")]
    pub budget_tokens: Option<i32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub enable: Option<bool>, // Hunyuan 格式
    /// 思考文本的呈现方式：`"summarized"` 才会返回可读文本。
    ///
    /// Anthropic 自 Opus 4.7 起默认 `"omitted"` —— 思考块只有一个空字符串加
    /// 加密签名，`delta["thinking"]` 拿不到任何内容，前端思考区会是空的。
    /// 其它厂商不认识该字段，故只在 Anthropic 出口、且注册表声明支持 adaptive
    /// 思考时下发（见 `registry::plan_anthropic_thinking`）。
    #[serde(skip_serializing_if = "Option::is_none")]
    pub display: Option<String>,
}

#[derive(Serialize, Clone, Debug)]
pub struct AnthropicRequest {
    pub model: String,
    pub max_tokens: i32,
    pub system: String,
    pub messages: Vec<Message>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub tools: Vec<serde_json::Value>,
    pub stream: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub thinking: Option<ThinkingConfig>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub temperature: Option<f32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub top_p: Option<f32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub top_k: Option<u32>,
    /// Anthropic 新代的输出配置 —— 目前只用于思考深度：
    /// `{"effort": "low" | "medium" | "high" | "xhigh" | "max"}`。
    ///
    /// 自 Opus 4.7 起思考深度不再靠 `budget_tokens`，改用这个字段。老模型不
    /// 认识它，故只在注册表声明了 `thinkingEffortValues` 时下发
    /// （见 `registry::plan_anthropic_thinking`）。
    #[serde(skip_serializing_if = "Option::is_none")]
    pub output_config: Option<serde_json::Value>,
}

// --- OpenAI Format Structs ---

#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct StreamOptions {
    pub include_usage: bool,
}

#[derive(Serialize, Clone, Debug)]
pub struct OpenAIRequest {
    pub model: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub max_tokens: Option<i32>,
    pub messages: Vec<OpenAIMessage>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tools: Option<Vec<OpenAITool>>,
    pub stream: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub stream_options: Option<StreamOptions>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reasoning_effort: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub thinking: Option<ThinkingConfig>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub thinking_budget: Option<i32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub enable_thinking: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub extra_body: Option<serde_json::Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub parameters: Option<serde_json::Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub temperature: Option<f32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub top_p: Option<f32>,
}

#[derive(Serialize, Deserialize, Clone, Debug)]
#[serde(untagged)]
pub enum OpenAIUserContent {
    Text(String),
    Parts(Vec<OpenAIContentPart>),
}

#[derive(Serialize, Deserialize, Clone, Debug)]
#[serde(tag = "role")]
pub enum OpenAIMessage {
    #[serde(rename = "system")]
    System { content: String },
    #[serde(rename = "user")]
    User { content: OpenAIUserContent },
    #[serde(rename = "assistant")]
    Assistant {
        #[serde(skip_serializing_if = "Option::is_none")]
        content: Option<String>,
        #[serde(skip_serializing_if = "Option::is_none")]
        tool_calls: Option<Vec<OpenAIToolCall>>,
        #[serde(skip_serializing_if = "Option::is_none")]
        reasoning_content: Option<serde_json::Value>,
    },
    #[serde(rename = "tool")]
    Tool {
        content: String,
        tool_call_id: String,
    },
}

#[derive(Serialize, Deserialize, Clone, Debug)]
#[serde(tag = "type")]
pub enum OpenAIContentPart {
    #[serde(rename = "text")]
    Text { text: String },
    #[serde(rename = "image_url")]
    ImageUrl { image_url: OpenAIImageUrl },
}

#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct OpenAIImageUrl {
    pub url: String,
}

#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct OpenAIToolCall {
    pub id: String,
    pub r#type: String, // always "function"
    pub function: OpenAIFunctionCall,
}

#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct OpenAIFunctionCall {
    pub name: String,
    pub arguments: String, // stringified JSON
}

#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct OpenAITool {
    pub r#type: String, // always "function"
    pub function: OpenAIFunctionDefinition,
}

#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct OpenAIFunctionDefinition {
    pub name: String,
    pub description: String,
    pub parameters: serde_json::Value,
}

#[derive(Serialize, Deserialize, Clone, Debug)]
#[serde(tag = "role")]
pub enum Message {
    #[serde(rename = "user")]
    User { content: Content },
    #[serde(rename = "assistant")]
    Assistant { content: Content },
}

#[derive(Serialize, Deserialize, Clone, Debug)]
#[serde(untagged)]
pub enum Content {
    Single(String),
    Multiple(Vec<ContentBlock>),
}

#[derive(Serialize, Deserialize, Clone, Debug)]
#[serde(tag = "type")]
pub enum ContentBlock {
    #[serde(rename = "text")]
    Text { text: String },
    #[serde(rename = "thinking")]
    Thinking { thinking: String, signature: String },
    #[serde(rename = "tool_use")]
    ToolUse {
        id: String,
        name: String,
        input: serde_json::Value,
    },
    #[serde(rename = "tool_result")]
    ToolResult {
        tool_use_id: String,
        content: String,
    },
    #[serde(rename = "image")]
    Image { source: ImageSource },
    /// 运行时上下文（意图标签 / 工作目录 / 项目结构 / 全局记忆）。
    /// 只在存储和内部表示中存在，出网前由 provider 翻译成普通 text 块。
    #[serde(rename = "context")]
    Context { text: String },
}

/// 消息来源 —— 决定这条消息属于哪个**可见域**。
///
/// ## 它不是"消息的 loop 类型"
///
/// 一条 assistant 消息天然可**同时**含 `thinking` + `text` + `tool_use` 多类块，
/// 一条 user 消息可同时含 `context` + `image` + `text` + `tool_result`
/// （见 [`ContentBlock`]）。那些是**结构类型**，由 `content_json` 的块数组承担；
/// 用一维字符串去表达"这条消息算思考还是算工具调用"是一个没有答案的问题。
/// 本枚举只回答一件事：**这条消息该被谁看见**。
///
/// ## 不变式（见 [`MessageSource::in_llm_context`]）
///
/// 进不了模型上下文的消息，也没有资格进摘要输入 —— 摘要的职责是压缩
/// "模型看到过的历史"，模型没看到的东西没有理由进摘要。这条不变式把
/// "压缩要不要保留它"从一份独立维护的名单变成了本枚举的派生结论。
///
/// ## 与落库形态的关系
///
/// 数据库里仍存字符串（可读、可 SQL 查、便于迁移），转换只发生在
/// [`MessageSource::as_db`] / [`MessageSource::from_db`] 两个函数里 ——
/// **不额外提供 serde 映射**，避免出现第二套"字符串 ↔ 枚举"口径而漂移。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum MessageSource {
    /// 常规对话：用户真实输入 / 模型真实输出
    #[default]
    Chat,
    /// 压缩摘要：模型可见，界面不可见
    Compact,
    /// 运行期系统注入：**写给模型看的指令**（崩溃恢复指令 / 方案重定向通知 / 反思修正建议）。
    /// 它是"伪装成一轮发言喂给模型的系统指令" —— 模型可见、界面不可见。
    Inject,
    /// 中断收尾：两侧都可见。正文是**半截内容**，中断原因由 `interrupt_kind` 列表达
    /// （见 `SessionMemory::interrupt_kinds`）
    Interrupted,
    /// 对齐填充：只为保持 user/assistant 交替（如 `"Context compressed."`），两侧都不可见
    Placeholder,
}

impl MessageSource {
    /// 落库形态（`session_messages.source` 列的值）
    pub fn as_db(self) -> &'static str {
        match self {
            Self::Chat => "chat",
            Self::Compact => "compact",
            Self::Inject => "inject",
            Self::Interrupted => "interrupted",
            Self::Placeholder => "placeholder",
        }
    }

    /// 从落库形态还原。
    ///
    /// **未知值一律报错，不做静默降级。** 曾经的兜底是 `unwrap_or("chat")`，
    /// 而 `chat` 是"模型可见 + 界面可见"这个**最宽**的可见域 —— 任何拼错的值、
    /// 任何新增但忘了同步迁移的值，都会被悄悄升格成"两边都可见"：
    /// 内部噪声泄漏进界面，或不该给模型的上下文混进 prompt。
    /// 默认值必须往**更窄**的方向兜，不能往更宽的方向兜；
    /// 而这里选择直接失败，是因为迁移已经覆盖了全部历史取值 ——
    /// 走到这个分支即代表数据损坏或漏了迁移，两者都应当**响**，而不是被咽下去。
    pub fn from_db(raw: &str) -> Result<Self, String> {
        match raw {
            "chat" => Ok(Self::Chat),
            "compact" => Ok(Self::Compact),
            "inject" => Ok(Self::Inject),
            "interrupted" => Ok(Self::Interrupted),
            "placeholder" => Ok(Self::Placeholder),
            other => Err(format!(
                "未知的 session_messages.source 取值「{}」——可能漏了 schema 迁移，或数据被外部改写",
                other
            )),
        }
    }

    /// 是否进 LLM 上下文（即是否出现在发给模型的请求里）
    ///
    /// ⚠️ 写成**穷尽 `match`** 而不是 `!matches!(self, Self::Placeholder)`：
    /// 后者在新增变体时会静默落进"可见"分支 —— 而"多给了模型一段不该给的上下文"
    /// 是不报错、不崩溃、只是行为悄悄变坏的那类问题。穷尽 match 让它变成编译错误。
    pub fn in_llm_context(self) -> bool {
        match self {
            Self::Chat | Self::Compact | Self::Inject | Self::Interrupted => true,
            Self::Placeholder => false,
        }
    }

    /// 是否参与界面渲染（会话历史气泡）
    ///
    /// 同 [`MessageSource::in_llm_context`]，刻意写穷尽 `match`：
    /// `matches!` 会让新增变体静默变成"不渲染"，而消息从界面消失是**静默**的。
    pub fn rendered_in_ui(self) -> bool {
        match self {
            Self::Chat | Self::Interrupted => true,
            Self::Compact | Self::Inject | Self::Placeholder => false,
        }
    }
}

#[cfg(test)]
mod message_source_tests {
    //! `MessageSource` 是"这条消息该被谁看见"的**唯一事实来源** ——
    //! 模型白名单、界面渲染门、压缩清理名单全部由下面这几个方法派生。
    //! 这些测试逐个取值钉死两个可见域，并钉死"未知值必须报错"。
    //!
    //! 之所以逐值列举而不是遍历：新增取值时本测试**不会**自动通过 ——
    //! 必须先回来想清楚它在两个可见域里各占哪一格。

    use super::MessageSource;

    /// 落库形态与还原必须一一对应（`as_db` / `from_db` 是唯一的转换口径）
    #[test]
    fn db_form_round_trips() {
        for src in ALL_SOURCES {
            assert_eq!(
                MessageSource::from_db(src.as_db()),
                Ok(src),
                "{:?} 的落库形态无法还原",
                src
            );
        }
    }

    /// 未知值必须报错，**不得静默降级**。
    ///
    /// 曾经的兜底是 `unwrap_or("chat")`，而 `chat` 是"模型可见 + 界面可见"
    /// 这个最宽的可见域 —— 任何拼错的值都会被悄悄升格成"两边都可见"。
    /// 特别钉死：旧值 `internal` 不能再被当成合法值（v21 已把它拆成
    /// `inject` / `placeholder`，走这条分支说明迁移没跑）。
    #[test]
    fn unknown_db_value_is_an_error() {
        for raw in ["", "chat ", "CHAT", "internal", "background", "context", "ui_only"] {
            assert!(
                MessageSource::from_db(raw).is_err(),
                "「{raw}」不是合法取值，必须报错而不是降级"
            );
        }
    }

    /// 模型上下文可见域：只有 `Placeholder`（对齐填充）被挡在外面。
    ///
    /// `Inject` 为真这一条是 A 类缺陷的回归防护 —— 崩溃恢复指令 / 方案重定向通知 /
    /// 反思修正建议都是**写给模型看的指令**，它们此前被标成 `internal` 而不在白名单里，
    /// 从未送达模型。
    #[test]
    fn llm_visibility_per_source() {
        assert!(MessageSource::Chat.in_llm_context());
        assert!(MessageSource::Compact.in_llm_context());
        assert!(
            MessageSource::Inject.in_llm_context(),
            "系统注入是写给模型看的，必须进模型上下文"
        );
        assert!(
            MessageSource::Interrupted.in_llm_context(),
            "中断锚点：模型要看到自己中断于何处才能续跑"
        );
        assert!(
            !MessageSource::Placeholder.in_llm_context(),
            "对齐填充不该占用模型上下文"
        );
    }

    /// 界面渲染可见域：只有 `Chat` 与 `Interrupted`。
    #[test]
    fn ui_visibility_per_source() {
        assert!(MessageSource::Chat.rendered_in_ui());
        assert!(MessageSource::Interrupted.rendered_in_ui());
        assert!(!MessageSource::Compact.rendered_in_ui());
        assert!(
            !MessageSource::Inject.rendered_in_ui(),
            "系统注入若渲染，用户会看到一条冒充自己提问的气泡"
        );
        assert!(!MessageSource::Placeholder.rendered_in_ui());
    }

    /// 不变式：**进不了模型上下文的消息，也没有资格进摘要输入**。
    ///
    /// 压缩的清理名单过去是独立维护的 `chat | compact | context`，
    /// 把 `interrupted` 漏在外面 —— 于是"模型必须看到中断锚点"的设计
    /// 在压缩后失效。现在压缩侧直接用 `in_llm_context()`，
    /// 本测试锁住"两者同源"这个前提。
    #[test]
    fn summarizer_input_excludes_exactly_placeholder() {
        let excluded: Vec<MessageSource> = ALL_SOURCES
            .into_iter()
            .filter(|s| !s.in_llm_context())
            .collect();
        assert_eq!(
            excluded,
            vec![MessageSource::Placeholder],
            "压缩要剔除的集合必须恰好是 {{Placeholder}}；多一个就是历史被吞，少一个就是噪声进摘要"
        );
    }

    /// 全部取值的测试清单。
    ///
    /// ⚠️ 新增变体时这份清单**不会**自动变长 —— 但这不构成漏网：
    /// `as_db` / `from_db` / `in_llm_context` / `rendered_in_ui` 四个函数都写成了
    /// 穷尽 `match`，少处理一个变体就是**编译错误**，比任何运行时护栏都硬。
    /// 本清单只负责把预期行为写成可读的事实。
    const ALL_SOURCES: [MessageSource; 5] = [
        MessageSource::Chat,
        MessageSource::Compact,
        MessageSource::Inject,
        MessageSource::Interrupted,
        MessageSource::Placeholder,
    ];

    /// 默认值是 `Chat`，且 `Chat` 是"两侧都可见"的最宽可见域 ——
    /// 因此**任何兜底路径都不得使用 `Default::default()` 来填一个未知来源**，
    /// 这里只是把这件事显式记下来。
    #[test]
    fn default_is_chat() {
        assert_eq!(MessageSource::default(), MessageSource::Chat);
    }
}

#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct ImageSource {
    pub r#type: String,
    pub media_type: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub data: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub file_path: Option<String>,
}

#[derive(Serialize, Clone, Debug)]
#[serde(rename_all = "camelCase")]
pub struct Skill {
    pub name: String,
    pub description: String,
    pub body: String,
    pub path: String,
}

/// Skill 列表元数据（不含 body，用于列表展示）
#[derive(Serialize, Clone, Debug)]
#[serde(rename_all = "camelCase")]
pub struct SkillMeta {
    pub name: String,
    pub description: String,
    pub path: String,
    pub body_tokens: usize,
    pub active: bool,
}

/// Skill 完整详情（含 body，用于详情展示）
#[derive(Serialize, Clone, Debug)]
#[serde(rename_all = "camelCase")]
pub struct SkillDetail {
    pub name: String,
    pub description: String,
    pub path: String,
    pub body: String,
}

#[derive(Serialize, Deserialize, Debug, Clone)]
#[serde(rename_all = "camelCase")]
pub struct ContextSectionSnapshot {
    pub key: String,
    pub label: String,
    pub chars: usize,
    pub estimated_tokens: usize,
    pub token_count_method: String,
    pub item_count: usize,
    pub content: String,
    pub truncated: bool,
    /// 原始 JSON 数据（仅 messages 和 tools section 有值）
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub raw_content: Option<String>,
}

/// 单个 loop 的缓存命中记录（用于渲染回合内趋势）
#[derive(Serialize, Deserialize, Debug, Clone, Default)]
#[serde(rename_all = "camelCase")]
pub struct CacheHitPoint {
    pub loop_count: usize,
    pub hit_tokens: u64,
    pub miss_tokens: u64,
    /// 命中的字段名（服务商未报告时为 None）
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source: Option<String>,
}

impl CacheHitPoint {
    pub fn total(&self) -> u64 {
        self.hit_tokens.saturating_add(self.miss_tokens)
    }

    pub fn hit_rate(&self) -> Option<f32> {
        let total = self.total();
        if total == 0 {
            None
        } else {
            Some(self.hit_tokens as f32 / total as f32)
        }
    }
}

#[derive(Serialize, Deserialize, Debug, Clone)]
#[serde(rename_all = "camelCase")]
pub struct SessionContextSnapshot {
    pub session_id: String,
    pub run_id: Option<String>,
    pub loop_count: usize,
    pub model: String,
    pub intent: String,
    pub api_format: String,
    pub created_at: u64,
    pub total_chars: usize,
    pub estimated_tokens: usize,
    pub provider_input_tokens: Option<u64>,
    pub provider_output_tokens: Option<u64>,
    pub provider_total_tokens: Option<u64>,
    /// 缓存命中 / 未命中的输入 token。
    /// `None` = 该 provider（或中转链路）未报告该字段，**与 0 命中是两回事**。
    #[serde(default)]
    pub cache_hit_tokens: Option<u64>,
    #[serde(default)]
    pub cache_miss_tokens: Option<u64>,
    /// 命中的字段名（如 `prompt_cache_hit_tokens` / `cached_tokens` / `cache_read_input_tokens`）
    #[serde(default)]
    pub cache_source: Option<String>,
    /// 本会话逐 loop 的缓存命中记录（保留最近 N 条，用于渲染趋势）
    #[serde(default)]
    pub cache_history: Vec<CacheHitPoint>,
    pub drift_percent: Option<f32>,
    pub max_context_tokens: Option<u32>,
    pub max_output_tokens: i32,
    pub message_count: usize,
    pub tool_schema_count: usize,
    pub tool_call_count: usize,
    pub tool_result_count: usize,
    pub sections: Vec<ContextSectionSnapshot>,
}

/// 中断类型（**结构化替代"从正文猜标记"**）。
///
/// 中断收尾会把 `**[回复被中断]** …` 追到 assistant 正文尾部——**模型必须看到**
/// （否则续跑时不知道上一句被截断）。但界面不该在气泡里显示它，此前只能靠
/// 前端正则从文本里"猜着剥"。有了本枚举，界面侧直接读字段生成 notice 小字，
/// 正文保持干净。
///
/// 落库位置（TEXT 列，取值见 `as_str`）：
/// - `session_messages.interrupt_kind` —— 渲染层与发送层读取（**唯一在用的落点**）。
///
/// ⚠️ 曾有第二个落点 `agent_runs.interrupt_kind`（设想用于"崩溃重建时给重建消息
/// 打标"），但那一列**从未被写入、也从未被读取**，已于 v18 迁移删除。
/// "哪一轮被打断"由 `agent_run_events.status` 承担。
///
/// 详见 doc/状态标注符号统一与结构化改造方案.md（阶段二）。
#[derive(Serialize, Deserialize, Debug, Clone, Copy, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum InterruptKind {
    /// 流空闲超时（SSE 阈值内零帧）
    StreamTimeout,
    /// 用户取消
    UserCancel,
    /// 执行报错（原始错误文本另存 `interrupted_reason` / `agent_runs.error`）
    PipelineError,
    /// 应用关闭（崩溃恢复占位）
    AppClosed,
    /// 已达回合上限且未获续跑授权（模型**不该**续写，否则立刻再次触顶）
    LoopLimit,
    /// 规划探索到上限（规划看门狗）
    PlanLimit,
}

impl InterruptKind {
    /// 落库字符串（`snake_case`，与 serde 序列化一致；TEXT 列直接存取）
    pub fn as_str(self) -> &'static str {
        match self {
            InterruptKind::StreamTimeout => "stream_timeout",
            InterruptKind::UserCancel => "user_cancel",
            InterruptKind::PipelineError => "pipeline_error",
            InterruptKind::AppClosed => "app_closed",
            InterruptKind::LoopLimit => "loop_limit",
            InterruptKind::PlanLimit => "plan_limit",
        }
    }

    /// 从落库字符串还原；未知值返回 `None`（调用方按"非中断消息"处理）
    pub fn from_db(value: &str) -> Option<Self> {
        match value {
            "stream_timeout" => Some(InterruptKind::StreamTimeout),
            "user_cancel" => Some(InterruptKind::UserCancel),
            "pipeline_error" => Some(InterruptKind::PipelineError),
            "app_closed" => Some(InterruptKind::AppClosed),
            "loop_limit" => Some(InterruptKind::LoopLimit),
            "plan_limit" => Some(InterruptKind::PlanLimit),
            _ => None,
        }
    }
}

#[derive(Serialize, Deserialize, Debug, Clone, Default)]
pub struct SessionMemory {
    /// 运行时消息（从 session_messages 表按 active_message_ids 重建，不序列化存储）
    ///
    /// 用 `skip` 而不是 `skip_serializing`：三个投影字段（messages / sources /
    /// interrupt_kinds）的**唯一事实来源是 session_messages 表**，
    /// `skip` 连反序列化也跳过，等于声明"就算 memory_json 里出现了这个字段也不认"。
    /// 只用 `skip_serializing` 的话，万一某份历史 JSON 里带着 messages 而没带
    /// sources（或反之），重建时就会拼出长度失衡的三数组 ——
    /// 正是 `save_session` 里那段"三 Vec 失衡"对账要抓的状态。
    #[serde(default, skip)]
    pub messages: Vec<Message>,
    /// LLM 活动视图索引 —— 指向 session_messages 表中 LLM 当前应看到的消息 ID
    #[serde(default)]
    pub message_ids: Vec<String>,
    /// 每条消息的来源分类（与 messages 平行），从 session_messages 表重建，不序列化存储
    /// （`skip` 的理由见 `messages` 字段）。
    ///
    /// 取值语义与两个可见域见 [`MessageSource`]。**它是"这条消息该被谁看见"的唯一事实来源**：
    /// 模型上下文白名单、界面渲染门、压缩清理名单全部由它的能力方法派生，
    /// 不允许任何消费方再各抄一份字符串名单（历史教训见
    /// `doc/消息来源与轮次元数据重构方案.md` §2.1）。
    #[serde(default, skip)]
    pub sources: Vec<MessageSource>,
    /// 每条消息的**中断类型**（与 messages/sources 严格平行），从 session_messages 表重建。
    ///
    /// `None` = 非中断消息。中断收尾会往 assistant 正文尾部追一句
    /// `**[回复被中断]** …`（**模型需要看到**，否则续跑时不知道上一句被截断），
    /// 但界面不该在气泡里显示它。有了 kind，界面侧直接读字段生成 notice、
    /// 正文保持干净，不必再用正则从文本里"猜着剥"。
    /// 详见 doc/状态标注符号统一与结构化改造方案.md（阶段二）。
    #[serde(default, skip)]
    pub interrupt_kinds: Vec<Option<String>>,
    /// 上下文快照单调序号：每次注入快照前自增，压缩/重启后仍保持单调递增
    #[serde(default)]
    pub snapshot_seq: u64,
    #[serde(default)]
    pub plan_documents: Vec<PlanDocument>,
}

#[derive(Serialize, Deserialize, Debug, Clone)]
#[serde(rename_all = "camelCase")]
pub struct AgentStep {
    #[serde(rename = "type")]
    pub step_type: String,
    pub tool: Option<String>,
    pub input_summary: Option<String>,
    pub output_summary: Option<String>,
    pub error: Option<String>,
    pub task: Option<String>,
    pub attempt: Option<i32>,
    pub max: Option<i32>,
    pub content: Option<String>,
    pub timestamp: u64,
}

#[derive(Serialize, Deserialize, Debug, Clone)]
#[serde(rename_all = "camelCase")]
pub struct PlanDocument {
    pub id: String,
    #[serde(default)]
    pub session_id: String,
    pub title: String,
    pub content: String,
    pub status: String,
    pub path: Option<String>,
    pub created_at: u64,
    pub updated_at: u64,
    pub decided_at: Option<u64>,
    /// 用户拒绝方案时填写的修改意见（与 content 分离，不覆盖原始方案）
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub rejection_feedback: Option<String>,
}

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq)]
#[serde(rename_all = "snake_case")]
pub enum TodoStatus {
    Pending,
    InProgress,
    Completed,
}

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct TodoItem {
    pub id: String,
    /// Imperative form describing what needs to be done.
    pub content: String,
    /// Present continuous form shown while the item is in progress.
    pub active_form: String,
    pub status: TodoStatus,
}

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq)]
#[serde(rename_all = "snake_case")]
pub enum TaskStatus {
    Pending,
    InProgress,
    Completed,
}

#[derive(Serialize, Deserialize, Debug, Clone)]
#[serde(rename_all = "camelCase")]
pub struct Task {
    pub id: i32,
    pub subject: String,
    pub description: String,
    pub status: TaskStatus,
    pub blocked_by: Vec<i32>,
    pub blocks: Vec<i32>,
    pub owner: String,
    /// 进行中时显示的动态文本（如 "Fixing authentication bug"）
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub active_form: Option<String>,
    /// 任意附加元数据
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub metadata: Option<serde_json::Value>,
    /// 该任务由哪种子代理执行（`RunSubagentsSequentially` 调度时读取）。
    ///
    /// `None` = 沿用历史行为（implementation 型）。任务整体以 JSON 存进
    /// `session_tasks.task_json`，所以新增字段**不需要 DB 迁移**。2026-09-20 新增。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub subagent_type: Option<String>,
}
