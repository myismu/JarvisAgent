use serde::{Deserialize, Serialize};

/// API 协议格式枚举，替换字符串比较
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ApiFormat {
    Anthropic,
    OpenAI,
}

impl ApiFormat {
    pub fn from_str(s: &str) -> Self {
        match s {
            "openai" => Self::OpenAI,
            _ => Self::Anthropic,
        }
    }

    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Anthropic => "anthropic",
            Self::OpenAI => "openai",
        }
    }

    /// 获取认证头 (header name, header value)
    pub fn auth_header(&self, api_key: &str) -> (&'static str, String) {
        match self {
            Self::Anthropic => ("x-api-key", api_key.to_string()),
            Self::OpenAI => ("authorization", format!("Bearer {}", api_key)),
        }
    }

    /// 是否需要 Anthropic 版本头
    pub fn requires_anthropic_version(&self) -> bool {
        matches!(self, Self::Anthropic)
    }

    /// 是否为 OpenAI 格式（用于需要 bool 的快速路径）
    pub fn is_openai(&self) -> bool {
        matches!(self, Self::OpenAI)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn from_str_openai() {
        assert_eq!(ApiFormat::from_str("openai"), ApiFormat::OpenAI);
    }

    #[test]
    fn from_str_anthropic_default() {
        assert_eq!(ApiFormat::from_str("anthropic"), ApiFormat::Anthropic);
        assert_eq!(ApiFormat::from_str(""), ApiFormat::Anthropic);
        assert_eq!(ApiFormat::from_str("unknown"), ApiFormat::Anthropic);
    }

    #[test]
    fn as_str_roundtrip() {
        assert_eq!(ApiFormat::OpenAI.as_str(), "openai");
        assert_eq!(ApiFormat::Anthropic.as_str(), "anthropic");
    }

    #[test]
    fn auth_header_openai() {
        let (name, value) = ApiFormat::OpenAI.auth_header("sk-test");
        assert_eq!(name, "authorization");
        assert_eq!(value, "Bearer sk-test");
    }

    #[test]
    fn auth_header_anthropic() {
        let (name, value) = ApiFormat::Anthropic.auth_header("key-123");
        assert_eq!(name, "x-api-key");
        assert_eq!(value, "key-123");
    }

    #[test]
    fn requires_anthropic_version() {
        assert!(ApiFormat::Anthropic.requires_anthropic_version());
        assert!(!ApiFormat::OpenAI.requires_anthropic_version());
    }

    #[test]
    fn is_openai() {
        assert!(ApiFormat::OpenAI.is_openai());
        assert!(!ApiFormat::Anthropic.is_openai());
    }
}
