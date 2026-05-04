//! # token_count.rs — 模型上下文 token 本地计数
//!
//! 为上下文监控提供统一 token 计数入口，优先使用本地 tokenizer，失败时回退字符估算。
//!
//! ## Key Exports
//! - `TokenCount`: 单段内容的 token 数与计数来源
//! - `TokenCountMethod`: token 计数来源标识
//! - `count_text()`: 按模型 ID 统计文本 token
//! - `drift_percent()`: 计算本地估算与 provider usage 的偏差
//!
//! ## Dependencies
//! - External: `tiktoken_rs`
//!
//! ## Constraints
//! - tokenizer 不匹配未知模型时自动回退到通用 OpenAI BPE；仍失败则使用字符估算

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TokenCountMethod {
    Tokenizer,
    Estimate,
}

impl TokenCountMethod {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Tokenizer => "tokenizer",
            Self::Estimate => "estimate",
        }
    }
}

#[derive(Debug, Clone, Copy)]
pub struct TokenCount {
    pub tokens: usize,
    pub method: TokenCountMethod,
}

pub fn count_text(model_id: &str, text: &str) -> TokenCount {
    if text.is_empty() {
        return TokenCount {
            tokens: 0,
            method: TokenCountMethod::Tokenizer,
        };
    }

    if let Ok(bpe) =
        tiktoken_rs::get_bpe_from_model(model_id).or_else(|_| tiktoken_rs::cl100k_base())
    {
        return TokenCount {
            tokens: bpe.encode_with_special_tokens(text).len(),
            method: TokenCountMethod::Tokenizer,
        };
    }

    TokenCount {
        tokens: estimate_text_tokens(text),
        method: TokenCountMethod::Estimate,
    }
}

pub fn drift_percent(estimated_tokens: usize, actual_tokens: u64) -> Option<f32> {
    if estimated_tokens == 0 || actual_tokens == 0 {
        return None;
    }
    Some(
        ((actual_tokens as f64 - estimated_tokens as f64) / estimated_tokens as f64 * 100.0) as f32,
    )
}

fn estimate_text_tokens(text: &str) -> usize {
    let chars = text.chars().count();
    if chars == 0 {
        0
    } else {
        (chars + 3) / 4
    }
}
