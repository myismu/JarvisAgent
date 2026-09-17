//! # thinking.rs — 深度思考档位的会话级裁决
//!
//! 把"深度思考要不要开"从**输入框组件的局部 UI 状态**提升为**会话级一等配置**，
//! 并与"模型预设"完全对称：切会话即切档位。
//!
//! ## 三层语义（裁决方向唯一，反向永不成立）
//!
//! | 层 | 名称 | 存储 | 说明 |
//! |---|---|---|---|
//! | L3 | 单轮覆盖 `thinking_override` | 不持久化（`ask_jarvis` 入参） | 仅供程序化调用/测试 |
//! | L2 | 会话档位 `sessions.thinking_mode` | DB | `NULL`=auto、`always`、`never` |
//! | L1 | 预设默认 `profiles[].thinkingDefault` | `config.json` | `auto` / `on` / `off` |
//!
//! L1 的 `auto` 再回落到全局 `agent_audience == "developer"`（保持既有语义，见
//! `pipeline` 的 `loop_think_default`）。
//!
//! ## 核心不变量
//! - **I1**：决策只依赖 `sessionId` + 当前配置，不依赖任何前端内存状态。
//! - **I3**：模型能力**只能夹紧**（clamp），**绝不回写** `sessions.thinking_mode`。
//! - **P-DS**：DeepSeek 系模型强制开启思考（`thinkingForced=true`，且 `thinking` 与
//!   `tool_calls` 必须共存）。**该约束优先于 L2/L3 的用户意愿**，任何实现若在这类模型上
//!   出现 `enabled=false` 即为回归。
//!
//! ## 依赖
//! - Internal: `crate::infra::llm::registry::ModelCapabilities`
//!
//! ## 约束
//! - `decide` 必须是**纯函数**（无 IO、无锁、无时间），以便穷举单测
//! - 前端 `src/utils/thinking.ts` 是同一份逻辑的 TS 镜像，两侧须共用同一组测试向量

use crate::infra::llm::registry::ModelCapabilities;

/// 深度思考档位的**会话级表态**（L2）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ThinkingMode {
    /// 跟随预设默认（存储为 `NULL`）
    Auto,
    /// 本会话强制开启
    Always,
    /// 本会话强制关闭
    Never,
}

impl ThinkingMode {
    /// 解析存储态/前端值；无法识别一律回落 [`ThinkingMode::Auto`]（宽松向前兼容）。
    pub fn parse(raw: &str) -> Self {
        match raw.trim().to_ascii_lowercase().as_str() {
            "always" | "on" | "true" => ThinkingMode::Always,
            "never" | "off" | "false" => ThinkingMode::Never,
            _ => ThinkingMode::Auto,
        }
    }

    /// 归一化为**存储态**：`Auto` → `None`（DB 写 `NULL`）。
    pub fn as_storage(self) -> Option<&'static str> {
        match self {
            ThinkingMode::Auto => None,
            ThinkingMode::Always => Some("always"),
            ThinkingMode::Never => Some("never"),
        }
    }

    /// 归一化为**前端态**：`None`/`NULL` → `"auto"`（永远给前端一个确定字符串）。
    pub fn as_api(self) -> &'static str {
        match self {
            ThinkingMode::Auto => "auto",
            ThinkingMode::Always => "always",
            ThinkingMode::Never => "never",
        }
    }
}

/// 把任意来源的会话档位归一化为**存储态**（`None` = `NULL` = auto）。
///
/// 非法输入回落 `None` 而非报错：DB 里可能存在历史脏值，
/// 读路径必须宽容，写路径由命令层负责校验。
pub fn normalize_session_mode(raw: Option<&str>) -> Option<&'static str> {
    match raw {
        None => None,
        Some(value) => ThinkingMode::parse(value).as_storage(),
    }
}

/// 预设默认档位（L1）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ThinkingDefault {
    /// 交给全局回退（`agent_audience == "developer"`）
    Auto,
    On,
    Off,
}

impl ThinkingDefault {
    /// 严格解析（写路径用）。返回 `None` 表示非法值，由调用方决定报错。
    pub fn parse_strict(raw: &str) -> Option<Self> {
        match raw.trim().to_ascii_lowercase().as_str() {
            "auto" => Some(ThinkingDefault::Auto),
            "on" => Some(ThinkingDefault::On),
            "off" => Some(ThinkingDefault::Off),
            _ => None,
        }
    }

    /// 宽容解析（读路径用）：非法值回落 `Auto`。
    pub fn parse(raw: &str) -> Self {
        Self::parse_strict(raw).unwrap_or(ThinkingDefault::Auto)
    }

    /// 与全局回退合并为一个确定的布尔值。
    pub fn resolve(self, audience_default: bool) -> bool {
        match self {
            ThinkingDefault::Auto => audience_default,
            ThinkingDefault::On => true,
            ThinkingDefault::Off => false,
        }
    }

    pub fn as_str(self) -> &'static str {
        match self {
            ThinkingDefault::Auto => "auto",
            ThinkingDefault::On => "on",
            ThinkingDefault::Off => "off",
        }
    }
}

/// 决策原因。**前端据此渲染 UI 状态，不做二次判断**（否则等于把决策逻辑搬回前端）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ThinkingReason {
    /// 模型不支持思考 → 强制关闭
    Unsupported,
    /// 模型强制思考 → 开启（用户意愿已满足，无需提示）
    ForcedByModel,
    /// 模型强制思考，但用户选的是"关闭" → 夹紧为开启，需提示
    ClampedByForced,
    /// 本会话明确要求开启
    SessionAlways,
    /// 本会话明确要求关闭
    SessionNever,
    /// 跟随预设默认
    ProfileDefault,
    /// 单轮覆盖（程序化调用）
    Override,
}

/// 决策结果。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ThinkingDecision {
    /// 本轮实际是否开启思考
    pub enabled: bool,
    pub reason: ThinkingReason,
    /// 需要告知用户时的 **i18n key**（不是本地化文案：后端不掌握前端 locale）
    pub notice_i18n_key: Option<&'static str>,
}

impl Default for ThinkingDecision {
    /// 占位值：尚未裁决时保持"关闭 + 跟随预设"，真正的值由 `decide` 覆盖。
    fn default() -> Self {
        Self {
            enabled: false,
            reason: ThinkingReason::ProfileDefault,
            notice_i18n_key: None,
        }
    }
}

impl ThinkingDecision {
    fn new(enabled: bool, reason: ThinkingReason) -> Self {
        Self {
            enabled,
            reason,
            notice_i18n_key: None,
        }
    }

    fn with_notice(mut self, key: &'static str) -> Self {
        self.notice_i18n_key = Some(key);
        self
    }

    /// 前端 `input.thinkingClampedByModel` 的键名
    pub const NOTICE_CLAMPED: &'static str = "input.thinkingClampedByModel";
}

/// 唯一决策逻辑（纯函数）。
///
/// 优先序：**L3 覆盖 ▸ 能力夹紧 ▸ L2 会话 ▸ L1 预设**。
///
/// 注意能力**夹紧**（而非"覆盖"）：`Unsupported` 与 `ForcedByModel` 会**无视** L2/L3 的
/// 相反意愿，因为它们是模型的硬约束；但**不修改**存储，用户表态原样保留（I3）。
pub fn decide(
    override_val: Option<bool>,
    session_mode: ThinkingMode,
    profile_resolved_default: bool,
    caps: Option<&ModelCapabilities>,
) -> ThinkingDecision {
    // 1. 模型不支持思考：无视一切意愿
    if let Some(caps) = caps {
        if !caps.thinking {
            return ThinkingDecision::new(false, ThinkingReason::Unsupported);
        }
    }

    // 2. 单轮覆盖优先（L3）
    if let Some(value) = override_val {
        return clamp_to_model(value, ThinkingReason::Override, caps, true);
    }

    // 3. 会话档位（L2）
    match session_mode {
        ThinkingMode::Always => clamp_to_model(true, ThinkingReason::SessionAlways, caps, true),
        ThinkingMode::Never => clamp_to_model(false, ThinkingReason::SessionNever, caps, true),
        // 会话未表态 → 按预设推导。此时没有任何"用户意愿"被违反，故不提示。
        ThinkingMode::Auto => clamp_to_model(
            profile_resolved_default,
            ThinkingReason::ProfileDefault,
            caps,
            false,
        ),
    }
}

/// 按模型硬约束夹紧用户意愿。
///
/// **P-DS**：`thinking_forced` 为真时一律开启。
///
/// `notice_on_conflict` 决定"是否打扰用户"：
/// - `false`（会话 `auto` / 预设推导）：**没有任何用户意愿被违反**，无需解释，返回
///   [`ThinkingReason::ForcedByModel`]；
/// - `true`（用户明确表态过，即 `always` / `never` / 单轮覆盖）：若用户想要关闭却被
///   强制开启，返回 [`ThinkingReason::ClampedByForced`] 并带上提示 key——让用户知道
///   "点了但没生效"，而不是像改造前那样按钮灰着、点了没反应、也没有任何解释。
fn clamp_to_model(
    user_wants: bool,
    reason: ThinkingReason,
    caps: Option<&ModelCapabilities>,
    notice_on_conflict: bool,
) -> ThinkingDecision {
    let forced = caps.map(|c| c.thinking_forced).unwrap_or(false);
    if forced {
        return if !notice_on_conflict || user_wants {
            ThinkingDecision::new(true, ThinkingReason::ForcedByModel)
        } else {
            ThinkingDecision::new(true, ThinkingReason::ClampedByForced)
                .with_notice(ThinkingDecision::NOTICE_CLAMPED)
        };
    }
    ThinkingDecision::new(user_wants, reason)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn caps(thinking: bool, forced: bool) -> ModelCapabilities {
        ModelCapabilities {
            streaming: true,
            thinking,
            thinking_param: Some("thinking".to_string()),
            temperature: true,
            vision: false,
            max_tokens: 8192,
            max_context_tokens: None,
            notes: String::new(),
            thinking_forced: forced,
            cache_usage_style: None,
            thinking_effort_values: Vec::new(),
            thinking_mode: None,
        }
    }

    /// 无注册表条目（自定义模型）时的保守口径：支持思考、不强制
    fn caps_unknown() -> Option<&'static ModelCapabilities> {
        None
    }

    // ── 判定表逐行 ──

    #[test]
    fn row1_unsupported_model_always_off() {
        let c = caps(false, false);
        for mode in [ThinkingMode::Auto, ThinkingMode::Always, ThinkingMode::Never] {
            for default in [true, false] {
                for ov in [None, Some(true), Some(false)] {
                    let d = decide(ov, mode, default, Some(&c));
                    assert!(!d.enabled, "不支持思考的模型必须关闭");
                    assert_eq!(d.reason, ThinkingReason::Unsupported);
                    assert!(d.notice_i18n_key.is_none());
                }
            }
        }
    }

    #[test]
    fn row2_always_plus_forced_is_on_without_notice() {
        let c = caps(true, true);
        let d = decide(None, ThinkingMode::Always, false, Some(&c));
        assert!(d.enabled);
        assert_eq!(d.reason, ThinkingReason::ForcedByModel);
        assert!(d.notice_i18n_key.is_none(), "用户本就想开，不该打扰");
    }

    #[test]
    fn row3_auto_plus_forced_is_on_without_notice() {
        let c = caps(true, true);
        // 会话未表态（auto）：无论预设推导出什么，都没有"用户意愿"被违反 → 不打扰
        for default in [true, false] {
            let d = decide(None, ThinkingMode::Auto, default, Some(&c));
            assert!(d.enabled);
            assert_eq!(
                d.reason,
                ThinkingReason::ForcedByModel,
                "auto 下不该报成 ClampedByForced（没有用户意愿被违反）"
            );
            assert!(d.notice_i18n_key.is_none());
        }
    }

    /// 只有"用户明确表态过（always/never/override）且被强制模型否决"才提示
    #[test]
    fn notice_only_when_explicit_intent_is_overridden() {
        let c = caps(true, true);
        // 明确想关 → 提示
        assert_eq!(
            decide(None, ThinkingMode::Never, true, Some(&c)).reason,
            ThinkingReason::ClampedByForced
        );
        // 明确想开 → 满足，不提示
        assert_eq!(
            decide(None, ThinkingMode::Always, false, Some(&c)).reason,
            ThinkingReason::ForcedByModel
        );
        // 覆盖想关 → 提示
        assert_eq!(
            decide(Some(false), ThinkingMode::Auto, false, Some(&c)).reason,
            ThinkingReason::ClampedByForced
        );
        // 覆盖想开 → 满足，不提示
        assert_eq!(
            decide(Some(true), ThinkingMode::Auto, false, Some(&c)).reason,
            ThinkingReason::ForcedByModel
        );
    }

    #[test]
    fn row4_never_plus_forced_is_clamped_on_with_notice() {
        let c = caps(true, true);
        let d = decide(None, ThinkingMode::Never, true, Some(&c));
        assert!(d.enabled, "P-DS：强制思考模型上不得关闭");
        assert_eq!(d.reason, ThinkingReason::ClampedByForced);
        assert_eq!(d.notice_i18n_key, Some(ThinkingDecision::NOTICE_CLAMPED));
    }

    #[test]
    fn row5_always_plus_optional_is_on() {
        let c = caps(true, false);
        let d = decide(None, ThinkingMode::Always, false, Some(&c));
        assert!(d.enabled);
        assert_eq!(d.reason, ThinkingReason::SessionAlways);
    }

    #[test]
    fn row6_never_plus_optional_is_off() {
        let c = caps(true, false);
        let d = decide(None, ThinkingMode::Never, true, Some(&c));
        assert!(!d.enabled);
        assert_eq!(d.reason, ThinkingReason::SessionNever);
    }

    #[test]
    fn row7_auto_follows_profile_default() {
        let c = caps(true, false);
        assert!(decide(None, ThinkingMode::Auto, true, Some(&c)).enabled);
        assert!(!decide(None, ThinkingMode::Auto, false, Some(&c)).enabled);
        assert_eq!(
            decide(None, ThinkingMode::Auto, true, Some(&c)).reason,
            ThinkingReason::ProfileDefault
        );
    }

    // ── L3 单轮覆盖优先 ──

    #[test]
    fn override_beats_session_and_profile() {
        let c = caps(true, false);
        assert!(decide(Some(true), ThinkingMode::Never, false, Some(&c)).enabled);
        assert!(!decide(Some(false), ThinkingMode::Always, true, Some(&c)).enabled);
        assert_eq!(
            decide(Some(false), ThinkingMode::Always, true, Some(&c)).reason,
            ThinkingReason::Override
        );
    }

    #[test]
    fn override_is_also_clamped_by_forced() {
        let c = caps(true, true);
        let d = decide(Some(false), ThinkingMode::Auto, false, Some(&c));
        assert!(d.enabled, "P-DS：override 也不得绕过硬约束");
        assert_eq!(d.reason, ThinkingReason::ClampedByForced);
        assert!(d.notice_i18n_key.is_some());
    }

    #[test]
    fn override_cannot_enable_unsupported_model() {
        let c = caps(false, false);
        let d = decide(Some(true), ThinkingMode::Always, true, Some(&c));
        assert!(!d.enabled);
        assert_eq!(d.reason, ThinkingReason::Unsupported);
    }

    // ── 未知模型（自定义/未注册）保守口径 ──

    #[test]
    fn unknown_model_does_not_clamp() {
        let d = decide(None, ThinkingMode::Never, true, caps_unknown());
        assert!(!d.enabled, "未注册模型不应被夹紧");
        assert_eq!(d.reason, ThinkingReason::SessionNever);
        assert!(d.notice_i18n_key.is_none());

        let d2 = decide(None, ThinkingMode::Always, false, caps_unknown());
        assert!(d2.enabled);
    }

    // ── 全量穷举：3(override) × 3(session) × 2(default) × 3(caps) = 54 组合 ──

    #[test]
    fn exhaustive_matrix_is_self_consistent() {
        let cap_variants = [caps(false, false), caps(true, false), caps(true, true)];
        for c in &cap_variants {
            for mode in [ThinkingMode::Auto, ThinkingMode::Always, ThinkingMode::Never] {
                for default in [true, false] {
                    for ov in [None, Some(true), Some(false)] {
                        let d = decide(ov, mode, default, Some(c));
                        // 不变量：不支持思考 ⇒ 一定关闭
                        if !c.thinking {
                            assert!(!d.enabled);
                        }
                        // 不变量：强制思考 ⇒ 一定开启（P-DS）
                        if c.thinking && c.thinking_forced {
                            assert!(d.enabled, "P-DS 被破坏: mode={:?} ov={:?}", mode, ov);
                        }
                        // 不变量：只有"夹紧"才带提示
                        assert_eq!(
                            d.notice_i18n_key.is_some(),
                            d.reason == ThinkingReason::ClampedByForced,
                            "notice 只应出现在 ClampedByForced"
                        );
                    }
                }
            }
        }
    }

    // ── 解析与归一化 ──

    #[test]
    fn parse_tolerates_variants_and_falls_back_to_auto() {
        assert_eq!(ThinkingMode::parse("always"), ThinkingMode::Always);
        assert_eq!(ThinkingMode::parse(" ALWAYS "), ThinkingMode::Always);
        assert_eq!(ThinkingMode::parse("true"), ThinkingMode::Always);
        assert_eq!(ThinkingMode::parse("never"), ThinkingMode::Never);
        assert_eq!(ThinkingMode::parse("off"), ThinkingMode::Never);
        assert_eq!(ThinkingMode::parse("garbage"), ThinkingMode::Auto);
        assert_eq!(ThinkingMode::parse(""), ThinkingMode::Auto);
    }

    #[test]
    fn storage_normalization_maps_auto_to_null() {
        assert_eq!(normalize_session_mode(None), None);
        assert_eq!(normalize_session_mode(Some("auto")), None);
        assert_eq!(normalize_session_mode(Some("")), None);
        assert_eq!(normalize_session_mode(Some("always")), Some("always"));
        assert_eq!(normalize_session_mode(Some("never")), Some("never"));
        assert_eq!(normalize_session_mode(Some("bogus")), None);
    }

    #[test]
    fn thinking_default_resolution() {
        assert!(ThinkingDefault::Auto.resolve(true));
        assert!(!ThinkingDefault::Auto.resolve(false));
        assert!(ThinkingDefault::On.resolve(false));
        assert!(!ThinkingDefault::Off.resolve(true));
        assert_eq!(ThinkingDefault::parse("garbage"), ThinkingDefault::Auto);
        assert_eq!(ThinkingDefault::parse_strict("garbage"), None);
        assert_eq!(ThinkingDefault::parse_strict("on"), Some(ThinkingDefault::On));
    }
}
