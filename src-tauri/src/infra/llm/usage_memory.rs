//! # usage_memory.rs — 端点级 usage 能力"自动记忆"
//!
//! 解决的问题：各家（尤其是 AI 中转站）报告缓存命中的字段名不同，且有的厂商在
//! **0 命中时干脆不返回该字段**（实测：Kimi / 小米 anthropic）。于是单看一次响应无法区分
//! "这家不报告" 与 "这次没命中（预热期）"。
//!
//! 做法：按 **`host|api_format`**（不是模型名——决定字段透不透传的是实际走的链路）记一本小账，
//! 写进 `data/global/model_caps.json`：
//!
//! ```jsonc
//! { "version": 1,
//!   "endpoints": {
//!     "api.deepseek.com|openai": { "cacheSource": "prompt_cache_hit_tokens", "reports": 12, "observes": 12 },
//!     "api.moonshot.cn|openai":  { "cacheSource": "cached_tokens", "omitsAtZero": true }
//!   } }
//! ```
//!
//! 由此得到两条判定：
//! - `reports > 0` ⇒ 该端点**会**报告缓存字段（后续字段缺失 ⇒ 本次活动按 **0 命中**解释，即预热）；
//! - `reports == 0` ⇒ 至今没见过 ⇒ 显示"未知"，绝不冒充 0。
//!
//! ## 约束
//! - 纯本地、不联网、不花钱；文件可删（= 重新学习）；
//! - 读写失败一律降级为"内存态可用"，绝不影响 LLM 调用主流程；
//! - 只在能力**发生变化**时落盘，避免每轮写文件。

use std::collections::BTreeMap;
use std::path::PathBuf;
use std::sync::{Mutex, OnceLock};

use serde::{Deserialize, Serialize};

const MEMORY_FILE: &str = "model_caps.json";
const MEMORY_VERSION: u32 = 1;

/// 单个端点（host + 协议格式）的观测记录
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct EndpointCap {
    /// 首次观测到的缓存字段名；`None` = 至今未见过
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cache_source: Option<String>,
    /// 见过缓存字段的次数
    #[serde(default)]
    pub reports: u32,
    /// 观测总次数（含字段缺失的响应）
    #[serde(default)]
    pub observes: u32,
    /// 是否曾出现"先缺失、后又出现"的情况 ⇒ 缺失应解释为 0（该端点用省略表达 0）
    #[serde(default)]
    pub omits_at_zero: bool,
    /// 最近一次观测时间（RFC3339）
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_seen: Option<String>,
}

impl EndpointCap {
    /// 该端点是否确认会报告缓存字段
    pub fn reports_cache(&self) -> bool {
        self.reports > 0
    }
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct MemoryFile {
    version: u32,
    #[serde(default)]
    endpoints: BTreeMap<String, EndpointCap>,
}

impl Default for MemoryFile {
    fn default() -> Self {
        Self {
            version: MEMORY_VERSION,
            endpoints: BTreeMap::new(),
        }
    }
}

/// 观测结果（供调用方决定"字段缺失"该怎么解释）
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Observation {
    /// 该端点是否确认会报告缓存字段（true 时：字段缺失 ⇒ 视为 0 命中）
    pub reports_cache: bool,
    /// 本次观测是否首次发现该端点会上报（可用于打日志）
    pub first_seen: bool,
}

/// 记忆库（内存态 + 落盘路径）
pub struct UsageMemory {
    path: PathBuf,
    data: MemoryFile,
}

impl UsageMemory {
    fn new(path: PathBuf) -> Self {
        let data = std::fs::read_to_string(&path)
            .ok()
            .and_then(|text| serde_json::from_str::<MemoryFile>(&text).ok())
            .filter(|file| file.version == MEMORY_VERSION)
            .unwrap_or_default();
        Self { path, data }
    }

    /// 记录一次观测，返回该端点的当前能力判定
    pub fn observe(
        &mut self,
        endpoint_key: &str,
        saw_cache_field: bool,
        source: Option<&str>,
    ) -> Observation {
        let entry = self.data.endpoints.entry(endpoint_key.to_string()).or_default();
        let before_reports = entry.reports;
        entry.observes = entry.observes.saturating_add(1);
        entry.last_seen = Some(chrono::Utc::now().to_rfc3339());

        let mut changed = false;
        if saw_cache_field {
            entry.reports = entry.reports.saturating_add(1);
            if let Some(source) = source {
                if entry.cache_source.as_deref() != Some(source) {
                    entry.cache_source = Some(source.to_string());
                    changed = true;
                }
            }
            // 之前出现过"字段缺失"，现在又出现了 ⇒ 说明该端点用省略表达 0
            if entry.observes > entry.reports && !entry.omits_at_zero {
                entry.omits_at_zero = true;
                changed = true;
            }
            if before_reports == 0 {
                changed = true;
            }
        }

        let observation = Observation {
            reports_cache: entry.reports > 0,
            first_seen: before_reports == 0 && entry.reports > 0,
        };
        if changed {
            self.save();
        }
        observation
    }

    /// 只读查询（不改状态、不落盘）
    pub fn capability(&self, endpoint_key: &str) -> EndpointCap {
        self.data
            .endpoints
            .get(endpoint_key)
            .cloned()
            .unwrap_or_default()
    }

    fn save(&self) {
        if let Some(parent) = self.path.parent() {
            let _ = std::fs::create_dir_all(parent);
        }
        if let Ok(text) = serde_json::to_string_pretty(&self.data) {
            // 先写临时文件再改名：避免写一半留下坏 JSON
            let tmp = self.path.with_extension("json.tmp");
            if std::fs::write(&tmp, text).is_ok() {
                let _ = std::fs::rename(&tmp, &self.path);
            }
        }
    }
}

/// 一次请求最终的缓存口径（"厂商本次上报"与"端点能力记忆"综合后的结果）
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CacheOutcome {
    /// 命中 token；`None` = 无法判定（该端点至今没表现出会报告缓存字段）
    pub hit: Option<u64>,
    pub miss: Option<u64>,
    /// 命中的字段名
    pub source: Option<String>,
}

impl CacheOutcome {
    /// 是否给出了明确口径（false 时 UI 应显示 `?` 而不是 0%）
    pub fn is_known(&self) -> bool {
        self.hit.is_some()
    }
}

/// 综合判定本次缓存口径。
///
/// | 本次响应 | 端点记忆 | 结论 |
/// |---|---|---|
/// | 报告了字段（含明确的 0） | —— | 原样采信 |
/// | 字段缺失 | 确认会报告 | 视为 **0 命中**（预热期），miss = 本次输入总量 |
/// | 字段缺失 | 从未见过 | **未知**，绝不猜成 0 |
///
/// 字段缺失时用 `input_tokens` 兜底 miss：该端点既然会报告，说明它支持缓存，
/// 那么"没命中"的输入就是这次全部输入（Anthropic 口径下 `input_tokens` 本身就是 miss）。
pub fn resolve_cache_outcome(
    reported_hit: Option<u64>,
    reported_miss: Option<u64>,
    reported_source: Option<&str>,
    known_reporter: bool,
    remembered_source: Option<&str>,
    input_tokens: u64,
) -> CacheOutcome {
    match reported_hit {
        Some(hit) => CacheOutcome {
            hit: Some(hit),
            miss: reported_miss,
            source: reported_source.map(|s| s.to_string()),
        },
        None if known_reporter => CacheOutcome {
            hit: Some(0),
            miss: Some(input_tokens),
            source: remembered_source.or(reported_source).map(|s| s.to_string()),
        },
        None => CacheOutcome {
            hit: None,
            miss: None,
            source: None,
        },
    }
}

static MEMORY: OnceLock<Mutex<UsageMemory>> = OnceLock::new();

fn memory() -> &'static Mutex<UsageMemory> {
    MEMORY.get_or_init(|| {
        let path = crate::infra::config::data_paths::global_dir().join(MEMORY_FILE);
        Mutex::new(UsageMemory::new(path))
    })
}

/// 由 base_url + 协议格式推导端点键（`host|format`）
///
/// 用 host 而不是完整 URL：同一家的 `/v1` 与 `/anthropic` 是两条链路（格式已区分），
/// 而路径里的额外前缀（有的中转带 `/api/paas/v4`）不影响"字段透不透传"这件事。
pub fn endpoint_key(base_url: &str, api_format: &str) -> String {
    let host = base_url
        .split("://")
        .nth(1)
        .unwrap_or(base_url)
        .split('/')
        .next()
        .unwrap_or(base_url)
        .to_ascii_lowercase();
    format!("{host}|{}", api_format.to_ascii_lowercase())
}

/// 记录一次观测（全局单例）
pub fn observe(endpoint_key: &str, saw_cache_field: bool, source: Option<&str>) -> Observation {
    match memory().lock() {
        Ok(mut guard) => guard.observe(endpoint_key, saw_cache_field, source),
        Err(poisoned) => poisoned
            .into_inner()
            .observe(endpoint_key, saw_cache_field, source),
    }
}

/// 查询端点能力（全局单例，只读）
pub fn capability(endpoint_key: &str) -> EndpointCap {
    match memory().lock() {
        Ok(guard) => guard.capability(endpoint_key),
        Err(poisoned) => poisoned.into_inner().capability(endpoint_key),
    }
}

// ───────────────────────── 测试 ─────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_memory(tag: &str) -> UsageMemory {
        let path = std::env::temp_dir().join(format!(
            "jarvis-usage-memory-{}-{}.json",
            tag,
            uuid::Uuid::new_v4()
        ));
        UsageMemory::new(path)
    }

    #[test]
    fn endpoint_key_uses_host_and_format() {
        assert_eq!(
            endpoint_key("https://api.deepseek.com/anthropic", "anthropic"),
            "api.deepseek.com|anthropic"
        );
        assert_eq!(
            endpoint_key("https://open.bigmodel.cn/api/paas/v4/chat/completions", "openai"),
            "open.bigmodel.cn|openai"
        );
        // 同一家不同格式是两条链路
        assert_ne!(
            endpoint_key("https://api.deepseek.com", "openai"),
            endpoint_key("https://api.deepseek.com", "anthropic")
        );
    }

    #[test]
    fn unknown_endpoint_reports_nothing() {
        let mem = temp_memory("unknown");
        assert!(!mem.capability("x|openai").reports_cache());
    }

    #[test]
    fn first_sighting_marks_endpoint_as_reporting() {
        let mut mem = temp_memory("first");
        let obs = mem.observe("api.deepseek.com|openai", true, Some("prompt_cache_hit_tokens"));
        assert!(obs.reports_cache);
        assert!(obs.first_seen);
        let cap = mem.capability("api.deepseek.com|openai");
        assert_eq!(cap.cache_source.as_deref(), Some("prompt_cache_hit_tokens"));
        assert_eq!(cap.reports, 1);
        assert!(!cap.omits_at_zero, "首次就是命中，还不能断定它会省略 0");
    }

    #[test]
    fn later_missing_field_flips_omits_at_zero() {
        // 复刻 Kimi 的行为：先缺字段（0 命中），后出现字段
        let mut mem = temp_memory("omits");
        let cold = mem.observe("api.moonshot.cn|openai", false, None);
        assert!(!cold.reports_cache, "第一次没见过字段 ⇒ 仍是未知");
        let hit = mem.observe("api.moonshot.cn|openai", true, Some("cached_tokens"));
        assert!(hit.reports_cache);
        assert!(hit.first_seen);
        let cap = mem.capability("api.moonshot.cn|openai");
        assert!(cap.omits_at_zero, "先缺失后出现 ⇒ 缺失应解释为 0");
        assert_eq!(cap.observes, 2);
        assert_eq!(cap.reports, 1);
    }

    #[test]
    fn reported_field_is_taken_as_is() {
        let out = resolve_cache_outcome(
            Some(1536),
            Some(178),
            Some("prompt_cache_hit_tokens"),
            true,
            Some("prompt_cache_hit_tokens"),
            1714,
        );
        assert_eq!(out.hit, Some(1536));
        assert_eq!(out.miss, Some(178));
        assert_eq!(out.source.as_deref(), Some("prompt_cache_hit_tokens"));
        assert!(out.is_known());
    }

    #[test]
    fn explicit_zero_is_not_confused_with_unknown() {
        // 小米 anthropic 实测：命中时报告字段，0 命中时字段缺失；
        // 但 deepseek 会明确报 0 —— 两者都不许被当成"未知"。
        let out = resolve_cache_outcome(
            Some(0),
            Some(20000),
            Some("prompt_cache_hit_tokens"),
            true,
            None,
            20000,
        );
        assert_eq!(out.hit, Some(0));
        assert_eq!(out.miss, Some(20000));
        assert!(out.is_known());
    }

    #[test]
    fn missing_field_on_known_reporter_means_warmup() {
        // 复刻 Kimi openai 第 1 轮：字段缺失，但该端点此前确认会报告
        let out = resolve_cache_outcome(None, None, None, true, Some("cached_tokens"), 20000);
        assert_eq!(out.hit, Some(0), "已知会报告的端点：缺失 ⇒ 0 命中");
        assert_eq!(out.miss, Some(20000), "未命中量 = 本次输入总量");
        assert_eq!(out.source.as_deref(), Some("cached_tokens"), "字段名沿用记忆");
        assert!(out.is_known());
    }

    #[test]
    fn missing_field_on_unknown_endpoint_stays_unknown() {
        let out = resolve_cache_outcome(None, None, None, false, None, 20000);
        assert_eq!(out.hit, None, "从没见过的端点：绝不猜成 0");
        assert_eq!(out.miss, None);
        assert_eq!(out.source, None);
        assert!(!out.is_known());
    }

    #[test]
    fn warmup_without_remembered_source_still_reports_zero() {
        let out = resolve_cache_outcome(None, None, None, true, None, 1234);
        assert_eq!(out.hit, Some(0));
        assert_eq!(out.miss, Some(1234));
        assert_eq!(out.source, None, "字段名未知但口径已知");
        assert!(out.is_known());
    }

    #[test]
    fn memory_survives_reload() {
        let path = std::env::temp_dir().join(format!(
            "jarvis-usage-memory-reload-{}.json",
            uuid::Uuid::new_v4()
        ));
        {
            let mut mem = UsageMemory::new(path.clone());
            mem.observe("relay.example|openai", true, Some("cached_tokens"));
        }
        let reloaded = UsageMemory::new(path.clone());
        let cap = reloaded.capability("relay.example|openai");
        assert_eq!(cap.cache_source.as_deref(), Some("cached_tokens"));
        assert!(cap.reports_cache());
        let _ = std::fs::remove_file(&path);
    }
}
