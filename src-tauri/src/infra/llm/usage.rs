//! # usage.rs — LLM 响应 `usage` 字段的归一化
//!
//! 各家（含 AI 中转站）报告缓存命中的字段名不一样，本项目实测到 4 种写法：
//!
//! | 写法 | 谁在用（实测） |
//! |---|---|
//! | `prompt_cache_hit_tokens` / `prompt_cache_miss_tokens` | DeepSeek 自有 |
//! | `prompt_tokens_details.cached_tokens`（嵌套） | OpenAI 官方 / 智谱 GLM / 小米 MiMo |
//! | `cached_tokens`（顶层平铺） | Kimi / Moonshot 及部分兼容实现 |
//! | `cache_read_input_tokens`（+ `input_tokens` 表未命中） | Anthropic 协议家族 |
//!
//! 本模块只做"翻译"：**纯函数、无 IO、不猜**。三条硬约束：
//! 1. **未知 ≠ 0**：没见过的写法返回 `hit: None`（`source == "unknown"`），由调用方显示"未知"；
//! 2. **不做模糊匹配**：不会因为某个键名里带 `cache` 就拿来做命中数
//!    （`cache_creation_input_tokens` 是"写入量"而不是"命中量"，语义完全不同）；
//! 3. **能推导才推导**：只给命中数的厂商用 `total - hit` 求未命中，并置 `derived = true` 标记来源。
//!
//! 依据与实测样例：`doc/缓存命中量化方案.md`、`doc/各厂商 usage 字段实测样例.md`。

use serde_json::Value;

/// 未识别到任何已知缓存字段时的 source 标记
pub const SOURCE_UNKNOWN: &str = "unknown";

/// 归一化后的缓存用量
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CacheUsage {
    /// 命中的输入 token；`None` = 该 provider 从未报告过（**不是 0**）
    pub hit: Option<u64>,
    /// 未命中的输入 token；`None` = 既没直接给也无法推导
    pub miss: Option<u64>,
    /// prompt 总输入 token（OpenAI 家族取 `prompt_tokens`；Anthropic 家族 = 命中 + 未命中）
    pub total_input: Option<u64>,
    /// `miss` 是否由 `total - hit` 推导而来
    pub derived: bool,
    /// 命中的字段名（排查"这家为什么显示未知"用）
    pub source: &'static str,
}

impl Default for CacheUsage {
    fn default() -> Self {
        Self {
            hit: None,
            miss: None,
            total_input: None,
            derived: false,
            source: SOURCE_UNKNOWN,
        }
    }
}

impl CacheUsage {
    /// 是否识别到了缓存字段（注意：识别到但命中为 0 也是"已知"）
    pub fn is_known(&self) -> bool {
        self.source != SOURCE_UNKNOWN && self.hit.is_some()
    }

    /// 输入总量：优先用厂商直接给的值，其次用 命中 + 未命中
    pub fn total(&self) -> Option<u64> {
        self.total_input.or(match (self.hit, self.miss) {
            (Some(h), Some(m)) => Some(h.saturating_add(m)),
            _ => None,
        })
    }

    /// 命中率；总量为 0 或未知时返回 None（不返回 0.0，避免把"未知"画成"0%"）
    pub fn hit_rate(&self) -> Option<f32> {
        let total = self.total()?;
        if total == 0 {
            return None;
        }
        Some(self.hit.unwrap_or(0) as f32 / total as f32)
    }
}

/// 按路径取 u64（缺失或类型不符返回 None）
fn u64_at(value: &Value, path: &[&str]) -> Option<u64> {
    let mut current = value;
    for key in path {
        current = current.get(*key)?;
    }
    current.as_u64()
}

/// 按候选表顺序探测缓存用量（顺序即优先级）
pub fn extract_cache_usage(usage: &Value) -> CacheUsage {
    try_deepseek(usage)
        .or_else(|| try_nested(usage))
        .or_else(|| try_flat(usage))
        .or_else(|| try_anthropic(usage))
        .unwrap_or_default()
}

/// 带"注册表覆盖"的探测。
///
/// `style` 来自 `model_registry.json` 的可选能力位 `cacheUsageStyle`：
/// - `None` / 未知取值 ⇒ 走候选表自动探测（默认，也是绝大多数情况）；
/// - `"deepseek" | "nested" | "flat" | "anthropic"` ⇒ 只按指定写法取（应对非标准命名被误判）；
/// - `"none" | "off" | "disabled"` ⇒ 明确知道这家不报告，直接判未知（省掉无谓探测）。
pub fn extract_cache_usage_with_style(usage: &Value, style: Option<&str>) -> CacheUsage {
    let normalized = style.map(|s| s.trim().to_ascii_lowercase());
    match normalized.as_deref() {
        Some("none") | Some("off") | Some("disabled") => CacheUsage::default(),
        Some("deepseek") => try_deepseek(usage).unwrap_or_default(),
        Some("nested") => try_nested(usage).unwrap_or_default(),
        Some("flat") => try_flat(usage).unwrap_or_default(),
        Some("anthropic") => try_anthropic(usage).unwrap_or_default(),
        _ => extract_cache_usage(usage),
    }
}

/// 1) DeepSeek 自有：命中与未命中都给，最完整
fn try_deepseek(usage: &Value) -> Option<CacheUsage> {
    let hit = u64_at(usage, &["prompt_cache_hit_tokens"])?;
    let miss = u64_at(usage, &["prompt_cache_miss_tokens"]);
    let total_input =
        u64_at(usage, &["prompt_tokens"]).or_else(|| miss.map(|m| hit.saturating_add(m)));
    Some(CacheUsage {
        hit: Some(hit),
        miss,
        total_input,
        derived: false,
        source: "prompt_cache_hit_tokens",
    })
}

/// 2) OpenAI 官方 / 智谱 GLM / 小米：嵌套 cached_tokens
fn try_nested(usage: &Value) -> Option<CacheUsage> {
    let hit = u64_at(usage, &["prompt_tokens_details", "cached_tokens"])?;
    let total_input = u64_at(usage, &["prompt_tokens"]);
    Some(CacheUsage {
        hit: Some(hit),
        miss: total_input.map(|t| t.saturating_sub(hit)),
        total_input,
        derived: true,
        source: "prompt_tokens_details.cached_tokens",
    })
}

/// 3) 顶层平铺 cached_tokens（Kimi 等）
fn try_flat(usage: &Value) -> Option<CacheUsage> {
    let hit = u64_at(usage, &["cached_tokens"])?;
    let total_input = u64_at(usage, &["prompt_tokens"]);
    Some(CacheUsage {
        hit: Some(hit),
        miss: total_input.map(|t| t.saturating_sub(hit)),
        total_input,
        derived: true,
        source: "cached_tokens",
    })
}

/// 4) Anthropic 协议：cache_read = 命中，input_tokens = 未命中
///    （cache_creation_input_tokens 是"写入缓存量"，实测 DeepSeek/GLM/Kimi 恒 0，不能当未命中）
fn try_anthropic(usage: &Value) -> Option<CacheUsage> {
    let hit = u64_at(usage, &["cache_read_input_tokens"])?;
    let miss = u64_at(usage, &["input_tokens"]);
    let total_input = miss.map(|m| hit.saturating_add(m));
    Some(CacheUsage {
        hit: Some(hit),
        miss,
        total_input,
        derived: false,
        source: "cache_read_input_tokens",
    })
}

/// 合并两次观测（Anthropic 的 `message_start` / `message_delta` 会先后带 usage）。
///
/// 规则：**只在 incoming 已知时覆盖**，且逐字段取 `Some` —— 避免后到的、字段不全的事件
/// 把已经拿到的命中数冲成 `None`。
pub fn merge_cache_usage(current: CacheUsage, incoming: CacheUsage) -> CacheUsage {
    if !incoming.is_known() {
        return current;
    }
    CacheUsage {
        hit: incoming.hit.or(current.hit),
        miss: incoming.miss.or(current.miss),
        total_input: incoming.total_input.or(current.total_input),
        derived: incoming.derived,
        source: incoming.source,
    }
}

/// 基础 token：两大家族（OpenAI: `prompt_tokens`/`completion_tokens`；Anthropic: `input_tokens`/`output_tokens`）
///
/// 返回 `Option`：`None` = 该 provider 未报告，调用方应显示"未知"而不是 0。
pub fn extract_base_tokens(usage: &Value) -> (Option<u64>, Option<u64>) {
    let input = u64_at(usage, &["prompt_tokens"]).or_else(|| u64_at(usage, &["input_tokens"]));
    let output =
        u64_at(usage, &["completion_tokens"]).or_else(|| u64_at(usage, &["output_tokens"]));
    (input, output)
}

// ───────────────────────── 测试（样本取自真机实测原文）─────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn deepseek_openai_uses_own_field_names() {
        let usage = json!({
            "prompt_tokens": 1726, "completion_tokens": 24, "total_tokens": 1750,
            "prompt_tokens_details": { "cached_tokens": 1536 },
            "completion_tokens_details": { "reasoning_tokens": 24 },
            "prompt_cache_hit_tokens": 1536, "prompt_cache_miss_tokens": 190
        });
        let cache = extract_cache_usage(&usage);
        assert_eq!(cache.source, "prompt_cache_hit_tokens");
        assert_eq!(cache.hit, Some(1536));
        assert_eq!(cache.miss, Some(190));
        assert_eq!(cache.total_input, Some(1726));
        assert!(!cache.derived, "DeepSeek 直接给了未命中数，不需要推导");
        assert_eq!(cache.hit_rate(), Some(1536.0 / 1726.0));
    }

    #[test]
    fn deepseek_anthropic_uses_cache_read_and_input_tokens() {
        // Anthropic 口径：input_tokens 是"未命中部分"
        let usage = json!({
            "input_tokens": 190, "cache_creation_input_tokens": 0,
            "cache_read_input_tokens": 1536, "output_tokens": 19, "service_tier": "standard"
        });
        let cache = extract_cache_usage(&usage);
        assert_eq!(cache.source, "cache_read_input_tokens");
        assert_eq!(cache.hit, Some(1536));
        assert_eq!(cache.miss, Some(190));
        assert_eq!(cache.total(), Some(1726));
    }

    #[test]
    fn glm_and_xiaomi_use_nested_cached_tokens() {
        // 不同厂商的键顺序不同，解析不受影响；未命中由 total - hit 推导
        let glm = json!({
            "completion_tokens": 24, "completion_tokens_details": { "reasoning_tokens": 24 },
            "prompt_tokens": 1833, "prompt_tokens_details": { "cached_tokens": 1792 }, "total_tokens": 1857
        });
        let cache = extract_cache_usage(&glm);
        assert_eq!(cache.source, "prompt_tokens_details.cached_tokens");
        assert_eq!(cache.hit, Some(1792));
        assert_eq!(cache.miss, Some(41));
        assert!(cache.derived);

        let xiaomi = json!({
            "completion_tokens": 20, "prompt_tokens": 1831, "total_tokens": 1851,
            "completion_tokens_details": { "reasoning_tokens": 17 },
            "prompt_tokens_details": { "cached_tokens": 1792 }
        });
        assert_eq!(extract_cache_usage(&xiaomi).miss, Some(39));
    }

    #[test]
    fn kimi_flat_cached_tokens_is_recognized() {
        let usage = json!({
            "prompt_tokens": 1730, "completion_tokens": 24, "total_tokens": 1754,
            "cached_tokens": 1536,
            "completion_tokens_details": { "reasoning_tokens": 23 },
            "prompt_tokens_details": { "cached_tokens": 1536 }
        });
        // 嵌套优先（同一个值，但记录更明确的来源）
        let cache = extract_cache_usage(&usage);
        assert_eq!(cache.source, "prompt_tokens_details.cached_tokens");
        assert_eq!(cache.hit, Some(1536));

        // 只有平铺时也能识别（Kimi anthropic 出口的形态）
        let flat_only = json!({ "prompt_tokens": 1694, "cached_tokens": 1536, "completion_tokens": 24 });
        let cache = extract_cache_usage(&flat_only);
        assert_eq!(cache.source, "cached_tokens");
        assert_eq!(cache.hit, Some(1536));
        assert_eq!(cache.miss, Some(158));
    }

    #[test]
    fn missing_field_is_unknown_not_zero() {
        // Kimi / 小米 anthropic 在 0 命中时【整个字段都不出现】
        let kimi_cold = json!({
            "prompt_tokens": 1694, "completion_tokens": 24, "total_tokens": 1718,
            "completion_tokens_details": { "reasoning_tokens": 23 }
        });
        let cache = extract_cache_usage(&kimi_cold);
        assert!(!cache.is_known(), "字段缺失必须判为未知");
        assert_eq!(cache.hit, None, "绝不把未知当 0");
        assert_eq!(cache.hit_rate(), None);
        assert_eq!(cache.source, SOURCE_UNKNOWN);

        let xiaomi_cold = json!({ "input_tokens": 1813, "output_tokens": 24 });
        assert!(!extract_cache_usage(&xiaomi_cold).is_known());
    }

    #[test]
    fn explicit_zero_is_known_zero() {
        // DeepSeek / GLM 在 0 命中时会显式给 0 —— 这是"已知且为 0"
        let deepseek_cold = json!({
            "prompt_tokens": 1714, "prompt_cache_hit_tokens": 0, "prompt_cache_miss_tokens": 1714
        });
        let cache = extract_cache_usage(&deepseek_cold);
        assert!(cache.is_known());
        assert_eq!(cache.hit, Some(0));
        assert_eq!(cache.hit_rate(), Some(0.0));

        let glm_cold = json!({ "prompt_tokens": 1809, "prompt_tokens_details": { "cached_tokens": 0 } });
        let cache = extract_cache_usage(&glm_cold);
        assert!(cache.is_known());
        assert_eq!(cache.hit_rate(), Some(0.0));
    }

    #[test]
    fn cache_creation_is_not_treated_as_miss() {
        // cache_creation_input_tokens 是"写入缓存量"，不能当未命中（实测恒 0）
        let only_creation = json!({ "input_tokens": 100, "cache_creation_input_tokens": 900 });
        assert!(!extract_cache_usage(&only_creation).is_known());
    }

    #[test]
    fn merge_keeps_earlier_values_when_later_event_lacks_them() {
        let start = json!({ "input_tokens": 190, "cache_read_input_tokens": 1536, "output_tokens": 1 });
        let delta = json!({ "input_tokens": 190, "output_tokens": 42 }); // 没有缓存字段
        let merged = merge_cache_usage(
            extract_cache_usage(&start),
            extract_cache_usage(&delta),
        );
        assert_eq!(merged.hit, Some(1536), "后到但字段缺失的事件不能冲掉已有命中数");
        assert_eq!(merged.source, "cache_read_input_tokens");
    }

    #[test]
    fn merge_upgrades_unknown_to_known() {
        let merged = merge_cache_usage(
            CacheUsage::default(),
            extract_cache_usage(&json!({ "prompt_tokens": 100, "cached_tokens": 64 })),
        );
        assert!(merged.is_known());
        assert_eq!(merged.hit, Some(64));
    }

    #[test]
    fn base_tokens_cover_both_families() {
        assert_eq!(
            extract_base_tokens(&json!({ "prompt_tokens": 10, "completion_tokens": 2 })),
            (Some(10), Some(2))
        );
        assert_eq!(
            extract_base_tokens(&json!({ "input_tokens": 10, "output_tokens": 2 })),
            (Some(10), Some(2))
        );
        assert_eq!(extract_base_tokens(&json!({})), (None, None));
    }

    #[test]
    fn registry_style_override_is_honoured() {
        // 混合 payload：既有嵌套又有平铺（Kimi anthropic 出口的形态）
        let mixed = json!({
            "prompt_tokens": 1730, "cached_tokens": 1536,
            "prompt_tokens_details": { "cached_tokens": 1536 }
        });
        // 不指定 → 候选表顺序（嵌套优先）
        assert_eq!(
            extract_cache_usage_with_style(&mixed, None).source,
            "prompt_tokens_details.cached_tokens"
        );
        // 指定 flat → 只按平铺取
        assert_eq!(extract_cache_usage_with_style(&mixed, Some("flat")).source, "cached_tokens");
        // 指定 deepseek → 该写法不存在 ⇒ 未知
        assert!(!extract_cache_usage_with_style(&mixed, Some("deepseek")).is_known());
        // 明确知道不报告 → 直接未知，省掉探测
        assert!(!extract_cache_usage_with_style(&mixed, Some("none")).is_known());
        // 大小写/空白容错
        assert_eq!(
            extract_cache_usage_with_style(&mixed, Some(" NESTED ")).source,
            "prompt_tokens_details.cached_tokens"
        );
    }
}
