//! 上下文压缩判据（**唯一口径**）
//!
//! 主 Agent（`core::agent::pipeline::compact_if_needed`）与子代理
//! （`core::tools::agent_tools::subagent::run_subagent`）共用本模块。
//！
//! 早先两边各写一份「常量 100k × 70%」：分母与模型真实窗口无关，1M 窗口的模型
//! 也用 100k 判据，几十轮就被压一次；分子只看本地分词器估算，而估算把
//! `tool_result` 截断后才分词，系统性偏低。这里统一成按模型真实能力 + 厂商实测标定。
//!
//! 判据三件事：
//!
//! 1. **分母（可用窗口）** `= W − R`
//!    - `W`：模型窗口，取自 `model_registry.json::maxContextTokens`；缺失时退回
//!      [`FALLBACK_CONTEXT_WINDOW`]（注册表里补全之前，宁可保守）
//!    - `R`：输出预算，与请求体里的 `max_tokens` **同源**（用户覆盖 > 注册表 > 常量兜底）。
//!      不预留的话，上下文可以一路涨到窗口边沿，回复反而没地方写
//! 2. **阈值** `= (W − R) × 70%`
//! 3. **分子（当前占用）** `= est_now × k`
//!
//! 标定系数 `k` 的意义：本地估算系统性低估（实测常见 2~3 倍），单看估算值会以为
//! 才用了一半、其实已经贴顶。所以拿**上一轮请求的厂商实测值**来标定：
//!
//! ```text
//! k    = P_prev / est_prev      // 上一轮实测 ÷ 上一轮估算
//! 分子 = est_now × k
//! ```
//!
//! 这与「`P_prev + (est_now − est_prev) × k`」完全等价——展开即为
//! `est_now × k`（`est_prev × k` 恰好等于 `P_prev`，两项相消）。取后者少一次减法、
//! 少一个溢出点，而且 `est_now < est_prev`（压缩刚发生、往轮图片被折叠）时也能
//! 自动把分子按比例缩回去，不会拿着压缩前的旧实测值反复触发压缩。
//!
//! 没有实测参考时（会话首轮）退回纯估算 `est_now`：宁可能晚压，不无依据地编数。

/// 触发压缩的可用窗口占用百分比
pub const COMPACT_TRIGGER_PERCENT: usize = 70;

/// 模型窗口未知时的兜底值（注册表补齐前的保守取值）
pub const FALLBACK_CONTEXT_WINDOW: usize = 100_000;

/// 解析模型上下文窗口：注册表 → 兜底常量
pub fn resolve_context_window(model_id: &str) -> usize {
    crate::infra::llm::registry::query_capabilities(model_id)
        .and_then(|capabilities| capabilities.max_context_tokens)
        .map(|value| value as usize)
        .filter(|value| *value > 0)
        .unwrap_or(FALLBACK_CONTEXT_WINDOW)
}

/// 解析输出预算：用户覆盖 > 注册表 `max_tokens` > 常量兜底
///
/// **必须与发给厂商的 `max_tokens` 同源**，否则预留量与真实输出能力对不上。
pub fn resolve_output_budget(model_id: &str, user_override: Option<i32>) -> usize {
    let resolved = user_override
        .or_else(|| {
            crate::infra::llm::registry::query_capabilities(model_id)
                .map(|capabilities| capabilities.max_tokens as i32)
        })
        .unwrap_or(crate::infra::types::constants::MAX_TOKENS_CONTEXT);
    resolved.max(0) as usize
}

/// 触发阈值 =（窗口 − 输出预算）× 70%
///
/// 输出预算 ≥ 窗口（配置异常）时不减，直接按整窗口算，避免可用窗口被削成 0
/// 导致每轮都触发压缩。
pub fn compact_trigger_tokens(window: usize, output_budget: usize) -> usize {
    let usable = if output_budget >= window {
        window
    } else {
        window - output_budget
    };
    usable * COMPACT_TRIGGER_PERCENT / 100
}

/// 用上一轮厂商实测值标定本地估算，得到"当前上下文占用"的估计
///
/// - `prev_measured`：上一轮请求的实测输入 token（`provider_input_tokens`）
/// - `est_prev`：与那次请求**同源**的本地估算（快照里的 `estimated_tokens`）
/// - `est_now`：本次判断时刻的本地估算
///
/// 三者缺任一有效值即退回纯估算 `est_now`。
pub fn calibrated_context_tokens(
    prev_measured: Option<u64>,
    est_prev: Option<usize>,
    est_now: usize,
) -> usize {
    match (prev_measured, est_prev) {
        (Some(measured), Some(prev)) if measured > 0 && prev > 0 && est_now > 0 => {
            // u128 中间量：est_now 与 measured 都可能到十万级，乘积会溢出 u64 以外的假设场景
            let scaled = (est_now as u128) * (measured as u128) / (prev as u128);
            scaled.min(usize::MAX as u128) as usize
        }
        _ => est_now,
    }
}

/// 是否触发压缩（严格大于阈值）
pub fn should_compact(context_tokens: usize, trigger: usize) -> bool {
    trigger > 0 && context_tokens > trigger
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn unknown_model_falls_back_to_default_window() {
        assert_eq!(
            resolve_context_window("__no-such-model-in-registry__"),
            FALLBACK_CONTEXT_WINDOW
        );
    }

    #[test]
    fn trigger_reserves_output_budget() {
        // 200k 窗口、32k 输出预算 → 可用 168k → 阈值 117.6k
        assert_eq!(compact_trigger_tokens(200_000, 32_000), 117_600);
    }

    #[test]
    fn output_budget_larger_than_window_does_not_underflow() {
        assert_eq!(compact_trigger_tokens(1_000, 5_000), 700);
    }

    #[test]
    fn calibration_scales_local_estimate_by_measured_ratio() {
        // 上一轮：实测 52k / 估算 17.3k → k ≈ 3.006；本轮估算 20k → 60.1k
        assert_eq!(
            calibrated_context_tokens(Some(52_000), Some(17_300), 20_000),
            60_115
        );
    }

    #[test]
    fn calibration_without_previous_measurement_returns_raw_estimate() {
        assert_eq!(calibrated_context_tokens(None, None, 20_000), 20_000);
        assert_eq!(calibrated_context_tokens(None, Some(17_300), 20_000), 20_000);
        assert_eq!(calibrated_context_tokens(Some(52_000), None, 20_000), 20_000);
        assert_eq!(calibrated_context_tokens(Some(0), Some(17_300), 20_000), 20_000);
        assert_eq!(calibrated_context_tokens(Some(52_000), Some(0), 20_000), 20_000);
    }

    #[test]
    fn calibration_shrinks_when_estimate_drops() {
        // 压缩刚发生：估算从 17.3k 掉到 5k → 标定后同步缩到 15.0k，不再沿用压缩前的 52k
        assert_eq!(
            calibrated_context_tokens(Some(52_000), Some(17_300), 5_000),
            15_028
        );
    }

    #[test]
    fn should_compact_is_strictly_greater_than_trigger() {
        assert!(!should_compact(70, 70));
        assert!(should_compact(71, 70));
        assert!(!should_compact(100, 0));
    }
}
