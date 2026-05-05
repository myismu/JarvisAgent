//! # common.rs — file_tools 子模块共享的文件处理常量与辅助判断
//!
//! 集中维护文件读取限制、文本归一化、忽略目录/静态资源扩展名判断，以及 Windows/跨平台文件锁错误识别，供读写、搜索和目录工具复用。
//!
//! ## Key Exports
//! - `MAX_FILE_SIZE_BYTES`: 单次读取允许的最大文件大小
//! - `MAX_LINES_DEFAULT`: 单次输出允许的默认最大行数
//! - `normalize_quotes()`: 归一化弯引号和全角引号以辅助精确匹配
//! - `normalize_line_endings()`: 将文本行尾统一为 LF
//! - `is_ignored_entry_name()`: 判断目录遍历时应跳过的条目名
//! - `is_static_asset_extension()`: 判断静态资源扩展名
//! - `is_search_skipped_extension()`: 判断搜索时应跳过的扩展名
//! - `is_locked_file_error()`: 识别文件锁或访问拒绝错误

use std::io;
use std::path::Path;

use encoding_rs::{GBK, UTF_16BE, UTF_16LE};

/// 文件大小限制：超过此大小拒绝读取（256KB）
pub(super) const MAX_FILE_SIZE_BYTES: u64 = 256 * 1024;

/// 输出行数限制：超过此行数自动截断
pub(super) const MAX_LINES_DEFAULT: usize = 2000;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum TextEncoding {
    Utf8,
    Utf8Bom,
    Utf16Le,
    Utf16Be,
    Gbk,
}

pub(super) struct DecodedText {
    pub content: String,
    pub encoding: TextEncoding,
}

pub(super) fn read_text_preserve_encoding(path: impl AsRef<Path>) -> io::Result<DecodedText> {
    let bytes = std::fs::read(path)?;
    decode_text_preserve_encoding(&bytes)
}

pub(super) fn decode_text_preserve_encoding(bytes: &[u8]) -> io::Result<DecodedText> {
    if looks_like_binary(bytes) {
        return Err(invalid_data_error("文件看起来是二进制内容，拒绝按文本处理"));
    }

    if bytes.starts_with(&[0xEF, 0xBB, 0xBF]) {
        let content = std::str::from_utf8(&bytes[3..])
            .map_err(|_| invalid_data_error("UTF-8 BOM 文件内容不是合法 UTF-8"))?
            .to_string();
        return Ok(DecodedText {
            content,
            encoding: TextEncoding::Utf8Bom,
        });
    }

    if bytes.starts_with(&[0xFF, 0xFE]) {
        let (content, had_errors) = decode_with_encoding(UTF_16LE, &bytes[2..]);
        if had_errors {
            return Err(invalid_data_error("UTF-16LE 文件解码失败"));
        }
        return Ok(DecodedText {
            content,
            encoding: TextEncoding::Utf16Le,
        });
    }

    if bytes.starts_with(&[0xFE, 0xFF]) {
        let (content, had_errors) = decode_with_encoding(UTF_16BE, &bytes[2..]);
        if had_errors {
            return Err(invalid_data_error("UTF-16BE 文件解码失败"));
        }
        return Ok(DecodedText {
            content,
            encoding: TextEncoding::Utf16Be,
        });
    }

    if let Ok(content) = std::str::from_utf8(bytes) {
        return Ok(DecodedText {
            content: content.to_string(),
            encoding: TextEncoding::Utf8,
        });
    }

    let (content, had_errors) = decode_with_encoding(GBK, bytes);
    if had_errors {
        return Err(invalid_data_error(
            "文件不是合法 UTF-8，也无法按 GBK/GB18030 解码",
        ));
    }
    Ok(DecodedText {
        content,
        encoding: TextEncoding::Gbk,
    })
}

pub(super) fn encode_text_preserve_encoding(
    content: &str,
    encoding: TextEncoding,
) -> io::Result<Vec<u8>> {
    match encoding {
        TextEncoding::Utf8 => Ok(content.as_bytes().to_vec()),
        TextEncoding::Utf8Bom => {
            let mut bytes = vec![0xEF, 0xBB, 0xBF];
            bytes.extend_from_slice(content.as_bytes());
            Ok(bytes)
        }
        TextEncoding::Utf16Le => Ok(encode_utf16_bytes(content, true, &[0xFF, 0xFE])),
        TextEncoding::Utf16Be => Ok(encode_utf16_bytes(content, false, &[0xFE, 0xFF])),
        TextEncoding::Gbk => encode_with_encoding(GBK, content, &[]),
    }
}

fn decode_with_encoding(encoding: &'static encoding_rs::Encoding, bytes: &[u8]) -> (String, bool) {
    let (decoded, _, had_errors) = encoding.decode(bytes);
    (decoded.into_owned(), had_errors)
}

fn encode_with_encoding(
    encoding: &'static encoding_rs::Encoding,
    content: &str,
    bom: &[u8],
) -> io::Result<Vec<u8>> {
    let (encoded, _, had_errors) = encoding.encode(content);
    if had_errors {
        return Err(invalid_data_error("新内容包含原文件编码无法表示的字符"));
    }
    let mut bytes = Vec::with_capacity(bom.len() + encoded.len());
    bytes.extend_from_slice(bom);
    bytes.extend_from_slice(&encoded);
    Ok(bytes)
}

fn encode_utf16_bytes(content: &str, little_endian: bool, bom: &[u8]) -> Vec<u8> {
    let mut bytes = Vec::with_capacity(bom.len() + content.len() * 2);
    bytes.extend_from_slice(bom);
    for unit in content.encode_utf16() {
        let encoded = if little_endian {
            unit.to_le_bytes()
        } else {
            unit.to_be_bytes()
        };
        bytes.extend_from_slice(&encoded);
    }
    bytes
}

fn looks_like_binary(bytes: &[u8]) -> bool {
    if bytes.is_empty() {
        return false;
    }

    let sample_len = bytes.len().min(4096);
    let sample = &bytes[..sample_len];
    let nul_count = sample.iter().filter(|&&b| b == 0).count();

    if sample.starts_with(&[0xFF, 0xFE]) || sample.starts_with(&[0xFE, 0xFF]) {
        return false;
    }

    nul_count > 0
}

fn invalid_data_error(message: &str) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, message)
}

/// 归一化弯引号为直引号（LLM 可能输出直引号而文件使用弯引号，用于匹配比较）
pub(super) fn normalize_quotes(s: &str) -> String {
    s.replace('\u{201C}', "\"")
        .replace('\u{201D}', "\"") // 中文双弯引号 ""
        .replace('\u{2018}', "'")
        .replace('\u{2019}', "'") // 中文单弯引号 ''
        .replace('\u{FF02}', "\"") // 全角双引号
        .replace('\u{FF07}', "'") // 全角单引号
}

/// 统一换行符为 LF（写入文件前调用）
pub(super) fn normalize_line_endings(content: &str) -> String {
    content.replace("\r\n", "\n").replace('\r', "\n")
}

pub(super) fn is_ignored_entry_name(file_name: &str) -> bool {
    file_name == "node_modules"
        || file_name == "target"
        || file_name == "dist"
        || file_name.starts_with('.')
}

pub(super) fn is_static_asset_extension(ext: &str) -> bool {
    matches!(
        ext.to_lowercase().as_str(),
        "png"
            | "ico"
            | "icns"
            | "jpg"
            | "jpeg"
            | "gif"
            | "svg"
            | "webp"
            | "mp3"
            | "mp4"
            | "wav"
            | "woff"
            | "woff2"
            | "ttf"
            | "eot"
    )
}

pub(super) fn is_search_skipped_extension(ext: &str) -> bool {
    is_static_asset_extension(ext) || matches!(ext.to_lowercase().as_str(), "pdf" | "zip")
}

pub(super) fn is_locked_file_error(err_msg: &str) -> bool {
    err_msg.contains("Access is denied")
        || err_msg.contains("os error 32")
        || err_msg.contains("os error 5")
        || err_msg.contains("being used by another process")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn decodes_and_encodes_utf8() {
        let decoded = decode_text_preserve_encoding("中文 hello".as_bytes()).unwrap();

        assert_eq!(decoded.encoding, TextEncoding::Utf8);
        assert_eq!(decoded.content, "中文 hello");
        assert_eq!(
            encode_text_preserve_encoding(&decoded.content, decoded.encoding).unwrap(),
            "中文 hello".as_bytes()
        );
    }

    #[test]
    fn preserves_utf8_bom() {
        let bytes = [vec![0xEF, 0xBB, 0xBF], "中文".as_bytes().to_vec()].concat();
        let decoded = decode_text_preserve_encoding(&bytes).unwrap();

        assert_eq!(decoded.encoding, TextEncoding::Utf8Bom);
        assert_eq!(decoded.content, "中文");
        assert_eq!(
            encode_text_preserve_encoding(&decoded.content, decoded.encoding).unwrap(),
            bytes
        );
    }

    #[test]
    fn preserves_utf16le_bom() {
        let bytes = vec![0xFF, 0xFE, 0x2D, 0x4E, 0x87, 0x65];
        let decoded = decode_text_preserve_encoding(&bytes).unwrap();

        assert_eq!(decoded.encoding, TextEncoding::Utf16Le);
        assert_eq!(decoded.content, "中文");
        assert_eq!(
            encode_text_preserve_encoding(&decoded.content, decoded.encoding).unwrap(),
            bytes
        );
    }

    #[test]
    fn preserves_utf16be_bom() {
        let bytes = vec![0xFE, 0xFF, 0x4E, 0x2D, 0x65, 0x87];
        let decoded = decode_text_preserve_encoding(&bytes).unwrap();

        assert_eq!(decoded.encoding, TextEncoding::Utf16Be);
        assert_eq!(decoded.content, "中文");
        assert_eq!(
            encode_text_preserve_encoding(&decoded.content, decoded.encoding).unwrap(),
            bytes
        );
    }

    #[test]
    fn falls_back_to_gbk() {
        let bytes = vec![0xD6, 0xD0, 0xCE, 0xC4];
        let decoded = decode_text_preserve_encoding(&bytes).unwrap();

        assert_eq!(decoded.encoding, TextEncoding::Gbk);
        assert_eq!(decoded.content, "中文");
        assert_eq!(
            encode_text_preserve_encoding(&decoded.content, decoded.encoding).unwrap(),
            bytes
        );
    }

    #[test]
    fn rejects_unencodable_gbk_content() {
        let result = encode_text_preserve_encoding("中文😀", TextEncoding::Gbk);

        assert!(result.is_err());
    }

    #[test]
    fn rejects_binary_content() {
        let result = decode_text_preserve_encoding(&[0x00, 0x01, 0x02, 0x03]);

        assert!(result.is_err());
    }
}
