//! # core/mcp/naming.rs — MCP 服务名与工具名规范化、命名空间映射
//!
//! 为外部 MCP 工具生成统一的命名空间名称 `mcp__{server}__{tool}`，同时满足
//! OpenAI / Anthropic 等大模型接口对工具名的硬性限制：
//! - 仅包含 ASCII 字母、数字、`_`、`-`
//! - 长度不超过 64 字符
//!
//! ## 关键导出
//! - `MCP_TOOL_PREFIX`: 统一前缀 `"mcp__"`
//! - `is_mcp_tool_name()`: 快速判定工具名是否属于 MCP 工具
//! - `validate_server_name()` / `sanitize_server_name()`: 服务名校验与导入清洗
//! - `McpToolNameMap`: 清洗后工具名 ↔ `(server_name, original_tool_name)` 双向映射表
//!
//! ## 约束
//! - 所有函数均为纯函数，不依赖 IO 或运行时状态，便于完整单元测试。
//! - 服务名不允许包含连续双下划线 `__`，避免与命名空间分隔符产生歧义。

use sha2::{Digest, Sha256};
use std::collections::HashMap;

/// MCP 工具统一命名空间前缀
pub const MCP_TOOL_PREFIX: &str = "mcp__";

/// 服务名最大长度
pub const MAX_SERVER_NAME_LEN: usize = 24;

/// 大模型接口（OpenAI 兼容）对工具名的最大长度限制
pub const MAX_LLM_TOOL_NAME_LEN: usize = 64;

/// 判定给定工具名是否为 MCP 工具（按统一前缀判定）
pub fn is_mcp_tool_name(name: &str) -> bool {
    name.starts_with(MCP_TOOL_PREFIX)
}

/// 校验服务名是否合法：`[a-z0-9_-]`，长度 1..=24，且不含连续双下划线 `__`
pub fn validate_server_name(name: &str) -> Result<(), String> {
    if name.is_empty() {
        return Err("服务名不能为空".to_string());
    }
    if name.len() > MAX_SERVER_NAME_LEN {
        return Err(format!(
            "服务名 '{}' 长度超过 {} 字符限制",
            name, MAX_SERVER_NAME_LEN
        ));
    }
    if name.contains("__") {
        return Err(format!(
            "服务名 '{}' 不能包含连续双下划线 '__'（保留作命名空间分隔符）",
            name
        ));
    }
    if !name
        .chars()
        .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '_' || c == '-')
    {
        return Err(format!(
            "服务名 '{}' 只能包含小写字母、数字、'-' 或 '_'",
            name
        ));
    }
    Ok(())
}

/// 将任意导入的服务名清洗为合法服务名（用于粘贴 Cursor / Claude Desktop JSON 时的自动修正）
///
/// 返回 `(清洗后的名称, 是否发生过修改)`。
pub fn sanitize_server_name(raw: &str) -> (String, bool) {
    let trimmed = raw.trim();
    if validate_server_name(trimmed).is_ok() {
        return (trimmed.to_string(), trimmed != raw);
    }

    let mut out = String::with_capacity(trimmed.len().min(MAX_SERVER_NAME_LEN));
    let mut prev_underscore = false;
    for ch in trimmed.chars() {
        let mapped = if ch.is_ascii_alphanumeric() {
            Some(ch.to_ascii_lowercase())
        } else if ch == '-' {
            Some('-')
        } else {
            Some('_')
        };
        if let Some(c) = mapped {
            if c == '_' {
                if prev_underscore || out.is_empty() {
                    continue;
                }
                prev_underscore = true;
            } else {
                prev_underscore = false;
            }
            out.push(c);
        }
    }

    while out.ends_with('_') {
        out.pop();
    }

    if out.is_empty() {
        // 全是非 ASCII 字符（如纯中文名）：用短哈希生成一个稳定的合法服务名
        let hash = short_hash(trimmed, 8);
        out = format!("srv_{}", hash);
    } else if out.len() > MAX_SERVER_NAME_LEN {
        let hash = short_hash(trimmed, 6);
        let prefix_len = MAX_SERVER_NAME_LEN - 7; // 预留 `_` + 6 位哈希
        let mut prefix: String = out.chars().take(prefix_len).collect();
        while prefix.ends_with('_') {
            prefix.pop();
        }
        out = format!("{}_{}", prefix, hash);
    }

    let changed = out != raw;
    (out, changed)
}

/// 将原始工具名片段清洗为仅含 `[a-zA-Z0-9_-]` 的安全字符串
fn sanitize_tool_segment(raw: &str) -> String {
    let mut out = String::with_capacity(raw.len());
    let mut prev_underscore = false;
    for ch in raw.chars() {
        if ch.is_ascii_alphanumeric() || ch == '-' {
            out.push(ch);
            prev_underscore = false;
        } else if !prev_underscore && !out.is_empty() {
            out.push('_');
            prev_underscore = true;
        }
    }
    while out.ends_with('_') {
        out.pop();
    }
    if out.is_empty() {
        format!("tool_{}", short_hash(raw, 8))
    } else {
        out
    }
}

/// 生成固定长度的十六进制短哈希（基于 SHA-256，用于截断或冲突去重）
fn short_hash(input: &str, hex_len: usize) -> String {
    let mut hasher = Sha256::new();
    hasher.update(input.as_bytes());
    let digest = hasher.finalize();
    let mut hex = String::with_capacity(hex_len);
    for byte in digest.iter() {
        if hex.len() >= hex_len {
            break;
        }
        let s = format!("{:02x}", byte);
        for c in s.chars() {
            if hex.len() < hex_len {
                hex.push(c);
            }
        }
    }
    hex
}

/// 组合完整的暴露工具名 `mcp__{server}__{tool}`，超长时保留前缀并追加短哈希
fn build_qualified_name(server_name: &str, original_tool_name: &str, force_hash: bool) -> String {
    let clean_tool = sanitize_tool_segment(original_tool_name);
    let candidate = format!("{}{}__{}", MCP_TOOL_PREFIX, server_name, clean_tool);
    if !force_hash && candidate.len() <= MAX_LLM_TOOL_NAME_LEN {
        return candidate;
    }

    // 超长或发生清洗冲突：用 `server::original_tool_name` 的 8 位哈希保证唯一性
    let hash = short_hash(&format!("{}::{}", server_name, original_tool_name), 8);
    let suffix = format!("_{}", hash);
    let max_prefix = MAX_LLM_TOOL_NAME_LEN.saturating_sub(suffix.len());
    let mut prefix: String = candidate.chars().take(max_prefix).collect();
    while prefix.ends_with('_') {
        prefix.pop();
    }
    format!("{}{}", prefix, suffix)
}

/// MCP 工具名双向映射条目
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct McpToolTarget {
    /// 规范化后的服务名
    pub server_name: String,
    /// MCP Server 声明的原始工具名（调用 `tools/call` 时必须原样回传）
    pub original_tool_name: String,
}

/// 暴露给大模型的工具名 ↔ `(server_name, original_tool_name)` 映射表
#[derive(Debug, Clone, Default)]
pub struct McpToolNameMap {
    /// qualified_name → target
    by_qualified: HashMap<String, McpToolTarget>,
    /// (server_name, original_tool_name) → qualified_name
    by_origin: HashMap<(String, String), String>,
}

impl McpToolNameMap {
    pub fn new() -> Self {
        Self::default()
    }

    /// 注册一个 `(server_name, original_tool_name)`，返回分配给大模型的唯一安全工具名。
    /// 若同一 `(server_name, original_tool_name)` 已注册，直接返回原有名字。
    pub fn register(&mut self, server_name: &str, original_tool_name: &str) -> String {
        let origin_key = (server_name.to_string(), original_tool_name.to_string());
        if let Some(existing) = self.by_origin.get(&origin_key) {
            return existing.clone();
        }

        let mut qualified = build_qualified_name(server_name, original_tool_name, false);
        if self.by_qualified.contains_key(&qualified) {
            // 清洗后与别的原始工具名撞名，强制附加短哈希消歧
            qualified = build_qualified_name(server_name, original_tool_name, true);
        }
        // 极端兜底：如果哈希仍撞名，追加数字后缀
        let mut counter = 2usize;
        while self.by_qualified.contains_key(&qualified) {
            let suffix = format!("_{}", counter);
            let max_prefix = MAX_LLM_TOOL_NAME_LEN.saturating_sub(suffix.len());
            let base: String = qualified.chars().take(max_prefix).collect();
            qualified = format!("{}{}", base, suffix);
            counter += 1;
        }

        self.by_qualified.insert(
            qualified.clone(),
            McpToolTarget {
                server_name: server_name.to_string(),
                original_tool_name: original_tool_name.to_string(),
            },
        );
        self.by_origin.insert(origin_key, qualified.clone());
        qualified
    }

    /// 根据大模型传入的工具名反查真实 `(server_name, original_tool_name)`
    pub fn resolve(&self, qualified_name: &str) -> Option<&McpToolTarget> {
        self.by_qualified.get(qualified_name)
    }

    /// 按原始名查询暴露名
    pub fn qualified_for(&self, server_name: &str, original_tool_name: &str) -> Option<&str> {
        self.by_origin
            .get(&(server_name.to_string(), original_tool_name.to_string()))
            .map(String::as_str)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn validate_server_name_rules() {
        assert!(validate_server_name("filesystem").is_ok());
        assert!(validate_server_name("my-db_1").is_ok());
        assert!(validate_server_name("").is_err());
        assert!(validate_server_name("MyServer").is_err());
        assert!(validate_server_name("a__b").is_err());
        assert!(validate_server_name("a".repeat(25).as_str()).is_err());
    }

    #[test]
    fn sanitize_server_name_handles_spaces_upper_chinese_and_long() {
        let (clean, changed) = sanitize_server_name("My GitHub__Server");
        assert!(changed);
        assert_eq!(clean, "my_github_server");
        assert!(validate_server_name(&clean).is_ok());

        let (cn, changed_cn) = sanitize_server_name("数据库服务");
        assert!(changed_cn);
        assert!(cn.starts_with("srv_"));
        assert!(validate_server_name(&cn).is_ok());

        let (long, changed_long) = sanitize_server_name("a_very_long_mcp_server_name_exceeding_limit");
        assert!(changed_long);
        assert!(long.len() <= MAX_SERVER_NAME_LEN);
        assert!(validate_server_name(&long).is_ok());
    }

    #[test]
    fn tool_name_map_handles_special_chars_collisions_and_length() {
        let mut map = McpToolNameMap::new();

        // 1. 普通工具名
        let n1 = map.register("fs", "read_file");
        assert_eq!(n1, "mcp__fs__read_file");
        assert_eq!(
            map.resolve(&n1),
            Some(&McpToolTarget {
                server_name: "fs".into(),
                original_tool_name: "read_file".into()
            })
        );

        // 2. 含点号和空格的冲突工具名（清洗后都会变成 a_b）
        let n2 = map.register("srv", "a.b");
        let n3 = map.register("srv", "a b");
        assert_ne!(n2, n3);
        assert_eq!(map.resolve(&n2).unwrap().original_tool_name, "a.b");
        assert_eq!(map.resolve(&n3).unwrap().original_tool_name, "a b");

        // 3. 中文工具名
        let n4 = map.register("srv", "查询用户");
        assert!(n4.starts_with("mcp__srv__tool_"));
        assert!(n4.len() <= MAX_LLM_TOOL_NAME_LEN);
        assert_eq!(map.resolve(&n4).unwrap().original_tool_name, "查询用户");

        // 4. 超长工具名截断 <= 64 字符
        let long_tool = "very_long_tool_name_that_definitely_exceeds_sixty_four_characters_total";
        let n5 = map.register("filesystem", long_tool);
        assert!(n5.len() <= MAX_LLM_TOOL_NAME_LEN);
        assert!(n5
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-'));
        assert_eq!(map.resolve(&n5).unwrap().original_tool_name, long_tool);
    }
}
