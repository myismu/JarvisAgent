//! # thinking.rs — 深度思考档位的会话级裁决
//!
//! 把"深度思考要不要开"从**输入框组件的局部 UI 状态**提升为**会话级一等配置**，
//! 与"权限档位 / 工作模式"完全对称：**设置默认喂新会话、界面态管渲染、session 库态随会话走**。
//!
//! ## 三层语义（2026-09-19 重构，取代旧的三态方案）
//!
//! | 层 | 名称 | 存储 | 说明 |
//! |---|---|---|---|
//! | L3 | 单轮覆盖 `thinking_override` | 不持久化（`ask_jarvis` 入参） | 仅供程序化调用/测试 |
//! | L2 | 会话档位 `sessions.thinking_mode` | DB，**布尔** | 只有 `true`/`false`，**不再有 NULL** |
//! | L1 | 设置默认 `ui_preferences.thinking_default` | `app-config.json` | `follow_global` / `on` / `off` |
//!
//! ## 为什么 L2 是布尔、L1 是三态（关键设计，勿轻易推翻）
//!
//! 旧设计让 L2 保存三态（`NULL`=auto），由裁决层**每轮现读** L1 解析。
//! 结果是：改一个预设的默认档位，**所有 NULL 会话下次发消息时全部跟着变**——
//! 因为 `NULL` 不是一个值，而是"每次都去外面问"。
//!
//! 现设计把"跟随"的解析**提前到会话创建的那一刻**：
//! - 用户没拨过开关 → 建会话时按 L1 解析出确定布尔写入 L2；
//! - 用户拨过 → 直接写用户的表态。
//!
//! 于是 L2 永远是确定值，**任何设置改动都无法再倒灌已有会话**。
//! 代价是"跟随"不可逆——但这是刻意的（见下方 I4）。
//!
//! ## 核心不变量
//! - **I1**：决策只依赖 `sessionId` + 该会话的模型能力，不依赖任何前端内存状态。
//! - **I3**：模型能力**只能夹紧**（clamp），**绝不回写** `sessions.thinking_mode`。
//! - **I4**：设置默认值（L1）**只作用于新建会话**，永不回溯修改已有会话的 L2。
//! - **P-DS**：DeepSeek 系模型强制开启思考（`thinkingForced=true`，且 `thinking` 与
//!   `tool_calls` 必须共存）。**该约束优先于 L2/L3 的用户意愿**，任何实现若在这类模型上
//!   出现 `enabled=false` 即为回归。
//!
//! ## 依赖
//! - Internal: `crate::infra::llm::registry::ModelCapabilities`
//!
//! ## 约束
//! - `decide` 必须是**纯函数**（无 IO、无锁、无时间），以便穷举单测

use crate::infra::llm::registry::ModelCapabilities;

/// **设置页**的深度思考默认档位（L1）。三态，只存在于 `app-config.json`。
///
/// 注意它**不是**会话档位——会话档位是布尔（见 [`ThinkingMode`]）。
/// 这个三态只在**新建会话那一刻**被解析成布尔，此后不再参与裁决。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ThinkingDefault {
    /// 跟随全局：**强制思考模型 → 开，非强制思考模型 → 关**。
    ///
    /// 判据是模型能力（`caps.thinking_forced`），不是用户类型（`agent_audience`）——
    /// 旧设计用 audience 作为全局回退，导致"输出风格受众"与"推理档"共用一个开关，
    /// 已于本次重构解耦。
    FollowGlobal,
    /// 无条件开启
    On,
    /// 无条件关闭
    Off,
}

impl ThinkingDefault {
    /// 严格解析（写路径用）。返回 `None` 表示非法值，由调用方决定报错。
    pub fn parse_strict(raw: &str) -> Option<Self> {
        match raw.trim().to_ascii_lowercase().as_str() {
            "follow_global" | "auto" => Some(ThinkingDefault::FollowGlobal),
            "on" | "always" | "true" => Some(ThinkingDefault::On),
            "off" | "never" | "false" => Some(ThinkingDefault::Off),
            _ => None,
        }
    }

    /// 宽容解析（读路径用）：非法值回落 [`ThinkingDefault::FollowGlobal`]。
    ///
    /// 为什么回落到"跟随全局"而不是"关"：老 `config.json` 里可能残留
    /// `"auto"` 这类旧值，落回"跟随全局"与旧行为最接近（旧 auto 正是走全局回退）。
    pub fn parse(raw: &str) -> Self {
        Self::parse_strict(raw).unwrap_or(ThinkingDefault::FollowGlobal)
    }

    /// 把设置档位解析为**确定布尔值**——这是"跟随全局"的实现。
    ///
    /// `thinking_forced`：目标模型是否强制开启思考（DeepSeek 系为 `true`）。
    ///
    /// 调用点只有一个语义位置：**新建会话时**（`pipeline::start_run` 首次解析 + 命令层
    /// `set_session_thinking_enabled` 的兜底）。裁决层（[`decide`]）不再调用它——
    /// 到了裁决时，会话档位早已是确定布尔值。
    pub fn resolve(self, thinking_forced: bool) -> bool {
        match self {
            ThinkingDefault::FollowGlobal => thinking_forced,
            ThinkingDefault::On => true,
            ThinkingDefault::Off => false,
        }
    }

    pub fn as_str(self) -> &'static str {
        match self {
            ThinkingDefault::FollowGlobal => "follow_global",
            ThinkingDefault::On => "on",
            ThinkingDefault::Off => "off",
        }
    }
}

/// **会话级**深度思考档位（L2）。**布尔**，落库为 `INTEGER`（1/0）。
///
/// 保留这个 newtype 而非裸 `bool`，是为了让"从 DB 读取"这件事有唯一入口
/// （[`ThinkingMode::from_storage`]），避免各处自行 `!= 0` 产生口径分歧。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ThinkingMode(pub bool);

impl ThinkingMode {
    pub const ON: ThinkingMode = ThinkingMode(true);
    pub const OFF: ThinkingMode = ThinkingMode(false);

    /// 从 DB 的值构造。
    ///
    /// **`NULL` 按 `false` 处理**：迁移 v14 会把存量 `NULL` 统一刷成 `0`，
    /// 所以正常不会有 `NULL`；这里保留兼容口径只是防御性——宁可保守关闭，
    /// 也不要出现"键不存在"式的未定义行为。
    pub fn from_storage(value: Option<i64>) -> Self {
        ThinkingMode(value.unwrap_or(0) != 0)
    }

    /// 归一化为**存储态**：`1` / `0`，**永不返回 NULL**。
    pub fn as_storage(self) -> i64 {
        if self.0 {
            1
        } else {
            0
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
    /// 采用会话档位（用户在本会话表态过，或建会话时按设置默认固化的值）
    SessionMode,
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
    /// 占位值：尚未裁决时保持"关闭"。真正的值由 [`decide`] 覆盖。
    fn default() -> Self {
        Self {
            enabled: false,
            reason: ThinkingReason::SessionMode,
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
/// 优先序：**能力夹紧 ▸ L3 单轮覆盖 ▸ L2 会话档位**。
///
/// 与旧版的区别：**不再有 L1（设置默认）参与**。L1 已在会话创建时被解析并写入 L2，
/// 到这里只剩"这个会话想不想开"这个确定事实。这正是"设置改动不倒灌老会话"的实现方式。
///
/// 注意能力**夹紧**（而非"覆盖"）：`Unsupported` 与 `ForcedByModel` 会**无视** L2/L3 的
/// 相反意愿，因为它们是模型的硬约束；但**不修改**存储，用户表态原样保留（I3）。
pub fn decide(
    override_val: Option<bool>,
    session_mode: ThinkingMode,
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

    // 3. 会话档位（L2）——已是确定布尔，无需再做"跟随"解析
    clamp_to_model(
        session_mode.0,
        ThinkingReason::SessionMode,
        caps,
        true,
    )
}

/// 按模型硬约束夹紧用户意愿。
///
/// **P-DS**：`thinking_forced` 为真时一律开启。
///
/// `notice_on_conflict` 决定"是否打扰用户"：
/// - `false`：**没有任何用户意愿被违反**，无需解释，返回 [`ThinkingReason::ForcedByModel`]；
/// - `true`：若用户想要关闭却被强制开启，返回 [`ThinkingReason::ClampedByForced`] 并带上提示
///   key——让用户知道"点了但没生效"，而不是按钮灰着、点了没反应、也没有任何解释。
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
            // 生命周期字段与裁决逻辑无关：这里只测「思考开关怎么被能力夹紧」，
            // 一律按在售模型构造。
            status: "active".to_string(),
            replaced_by: None,
            status_note: None,
        }
    }

    /// 无注册表条目（自定义模型）时的保守口径：支持思考、不强制
    fn caps_unknown() -> Option<&'static ModelCapabilities> {
        None
    }

    // ── 基本裁决 ──

    #[test]
    fn unsupported_model_always_off() {
        let c = caps(false, false);
        for on in [true, false] {
            for ov in [None, Some(true), Some(false)] {
                let d = decide(ov, ThinkingMode(on), Some(&c));
                assert!(!d.enabled, "不支持思考的模型必须关闭");
                assert_eq!(d.reason, ThinkingReason::Unsupported);
                assert!(d.notice_i18n_key.is_none());
            }
        }
    }

    #[test]
    fn forced_model_is_always_on() {
        let c = caps(true, true);
        for ov in [None, Some(true), Some(false)] {
            for on in [true, false] {
                let d = decide(ov, ThinkingMode(on), Some(&c));
                assert!(d.enabled, "P-DS：强制思考模型上不得关闭");
            }
        }
    }

    #[test]
    fn forced_model_notices_only_when_user_wanted_off() {
        let c = caps(true, true);
        // 想关 → 被夹紧，提示
        let d = decide(None, ThinkingMode::OFF, Some(&c));
        assert_eq!(d.reason, ThinkingReason::ClampedByForced);
        assert_eq!(d.notice_i18n_key, Some(ThinkingDecision::NOTICE_CLAMPED));
        // 本想开 → 满足，不提示
        let d = decide(None, ThinkingMode::ON, Some(&c));
        assert_eq!(d.reason, ThinkingReason::ForcedByModel);
        assert!(d.notice_i18n_key.is_none());
        // override 想关 → 提示
        assert_eq!(
            decide(Some(false), ThinkingMode::ON, Some(&c)).reason,
            ThinkingReason::ClampedByForced
        );
    }

    #[test]
    fn optional_model_follows_session_mode() {
        let c = caps(true, false);
        let on = decide(None, ThinkingMode::ON, Some(&c));
        assert!(on.enabled);
        assert_eq!(on.reason, ThinkingReason::SessionMode);
        assert!(on.notice_i18n_key.is_none());

        let off = decide(None, ThinkingMode::OFF, Some(&c));
        assert!(!off.enabled);
        assert_eq!(off.reason, ThinkingReason::SessionMode);
    }

    // ── L3 单轮覆盖优先 ──

    #[test]
    fn override_beats_session_mode() {
        let c = caps(true, false);
        assert!(decide(Some(true), ThinkingMode::OFF, Some(&c)).enabled);
        assert!(!decide(Some(false), ThinkingMode::ON, Some(&c)).enabled);
        assert_eq!(
            decide(Some(false), ThinkingMode::ON, Some(&c)).reason,
            ThinkingReason::Override
        );
    }

    #[test]
    fn override_is_also_clamped_by_forced() {
        let c = caps(true, true);
        let d = decide(Some(false), ThinkingMode::ON, Some(&c));
        assert!(d.enabled, "P-DS：override 也不得绕过硬约束");
        assert_eq!(d.reason, ThinkingReason::ClampedByForced);
        assert!(d.notice_i18n_key.is_some());
    }

    #[test]
    fn override_cannot_enable_unsupported_model() {
        let c = caps(false, false);
        let d = decide(Some(true), ThinkingMode::ON, Some(&c));
        assert!(!d.enabled);
        assert_eq!(d.reason, ThinkingReason::Unsupported);
    }

    // ── 未知模型（自定义/未注册）保守口径 ──

    #[test]
    fn unknown_model_does_not_clamp() {
        let d = decide(None, ThinkingMode::OFF, caps_unknown());
        assert!(!d.enabled, "未注册模型不应被夹紧");
        assert_eq!(d.reason, ThinkingReason::SessionMode);
        assert!(d.notice_i18n_key.is_none());

        assert!(decide(None, ThinkingMode::ON, caps_unknown()).enabled);
    }

    // ── 全量穷举：3(override) × 2(session) × 3(caps) = 18 组合 ──

    #[test]
    fn exhaustive_matrix_is_self_consistent() {
        let cap_variants = [caps(false, false), caps(true, false), caps(true, true)];
        for c in &cap_variants {
            for on in [true, false] {
                for ov in [None, Some(true), Some(false)] {
                    let d = decide(ov, ThinkingMode(on), Some(c));
                    // 不变量：不支持思考 ⇒ 一定关闭
                    if !c.thinking {
                        assert!(!d.enabled);
                    }
                    // 不变量：强制思考 ⇒ 一定开启（P-DS）
                    if c.thinking && c.thinking_forced {
                        assert!(d.enabled, "P-DS 被破坏: on={} ov={:?}", on, ov);
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

    // ── 存储态（布尔）──

    #[test]
    fn storage_roundtrip_never_yields_null() {
        assert_eq!(ThinkingMode::ON.as_storage(), 1);
        assert_eq!(ThinkingMode::OFF.as_storage(), 0);
        assert!(ThinkingMode::from_storage(Some(0)).0 == false);
        assert!(ThinkingMode::from_storage(Some(1)).0 == true);
        // 兼容口径：NULL 按关处理（v14 已把存量刷成 0，这里只防御）
        assert_eq!(ThinkingMode::from_storage(None), ThinkingMode::OFF);
    }

    // ── 设置默认档位（L1）──

    #[test]
    fn thinking_default_resolution_uses_model_capability() {
        // 跟随全局：强制思考模型 → 开，非强制 → 关（已与 agent_audience 解耦）
        assert!(ThinkingDefault::FollowGlobal.resolve(true));
        assert!(!ThinkingDefault::FollowGlobal.resolve(false));
        // 显式档位无视模型能力（能力夹紧仍由裁决层负责）
        assert!(ThinkingDefault::On.resolve(false));
        assert!(!ThinkingDefault::Off.resolve(true));
    }

    #[test]
    fn thinking_default_parse_tolerates_legacy_values() {
        assert_eq!(
            ThinkingDefault::parse("follow_global"),
            ThinkingDefault::FollowGlobal
        );
        // 旧 config.json 里的 "auto" 落到 follow_global（与旧行为最接近）
        assert_eq!(ThinkingDefault::parse("auto"), ThinkingDefault::FollowGlobal);
        assert_eq!(ThinkingDefault::parse("garbage"), ThinkingDefault::FollowGlobal);
        assert_eq!(ThinkingDefault::parse_strict("garbage"), None);
        assert_eq!(ThinkingDefault::parse_strict("on"), Some(ThinkingDefault::On));
        assert_eq!(ThinkingDefault::parse_strict("off"), Some(ThinkingDefault::Off));
    }
}
