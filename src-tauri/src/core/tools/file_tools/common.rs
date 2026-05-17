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

/// 从工具调用参数中提取 file path，兼容 path / file_path / filePath 三种命名
pub(super) fn resolve_path(input: &serde_json::Value) -> &str {
    input["path"].as_str()
        .or_else(|| input["file_path"].as_str())
        .or_else(|| input["filePath"].as_str())
        .unwrap_or("")
}

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

    // UTF-16 BOM 不是二进制
    if sample.starts_with(&[0xFF, 0xFE]) || sample.starts_with(&[0xFE, 0xFF]) {
        return false;
    }

    // 任何 null 字节 = 二进制
    if nul_count > 0 {
        return true;
    }

    // 检测已知二进制/压缩文件魔数
    if has_binary_magic_bytes(sample) {
        return true;
    }

    // 非可打印字符比例超过 10% 视为二进制（排除常见的空白字符）
    let non_printable_ratio = non_printable_ratio(sample);
    non_printable_ratio > 0.10
}

/// 已知二进制/压缩文件格式的魔数
fn has_binary_magic_bytes(sample: &[u8]) -> bool {
    if sample.len() < 4 {
        return false;
    }
    // gzip (.gz)
    if sample.starts_with(&[0x1F, 0x8B]) {
        return true;
    }
    // zip / jar / docx / xlsx / pptx / apk
    if sample.starts_with(&[0x50, 0x4B, 0x03, 0x04])
        || sample.starts_with(&[0x50, 0x4B, 0x05, 0x06])
        || sample.starts_with(&[0x50, 0x4B, 0x07, 0x08])
    {
        return true;
    }
    // 7-zip (.7z)
    if sample.starts_with(b"7z\xBC\xAF\x27\x1C") {
        return true;
    }
    // xz (.xz / .tar.xz)
    if sample.starts_with(&[0xFD, 0x37, 0x7A, 0x58, 0x5A, 0x00]) {
        return true;
    }
    // bzip2 (.bz2)
    if sample.starts_with(b"BZh") {
        return true;
    }
    // zstd (.zst)
    if sample.starts_with(&[0x28, 0xB5, 0x2F, 0xFD]) {
        return true;
    }
    // lz4
    if sample.starts_with(&[0x04, 0x22, 0x4D, 0x18]) {
        return true;
    }
    // Windows PE (.exe, .dll, .sys, .pdb)
    if sample.starts_with(b"MZ") {
        return true;
    }
    // ELF (Linux binary)
    if sample.starts_with(&[0x7F, b'E', b'L', b'F']) {
        return true;
    }
    // Mach-O (macOS binary)
    if sample.starts_with(&[0xCF, 0xFA, 0xED, 0xFE])
        || sample.starts_with(&[0xCE, 0xFA, 0xED, 0xFE])
        || sample.starts_with(&[0xFE, 0xED, 0xFA, 0xCF])
        || sample.starts_with(&[0xFE, 0xED, 0xFA, 0xCE])
    {
        return true;
    }
    // RAR
    if sample.starts_with(b"Rar!\x1A\x07")
        || sample.starts_with(b"Rar!\x1A\x07\x01\x00")
    {
        return true;
    }
    // tar (ustar)
    if sample.len() >= 262 && &sample[257..262] == b"ustar" {
        return true;
    }
    // MSI / OLE2 (Office 旧格式)
    if sample.starts_with(&[0xD0, 0xCF, 0x11, 0xE0, 0xA1, 0xB1, 0x1A, 0xE1]) {
        return true;
    }
    // PNG
    if sample.starts_with(&[0x89, b'P', b'N', b'G']) {
        return true;
    }
    // JPEG
    if sample.starts_with(&[0xFF, 0xD8, 0xFF]) {
        return true;
    }
    // WebP
    if sample.len() >= 12 && &sample[..4] == b"RIFF" && &sample[8..12] == b"WEBP" {
        return true;
    }
    // GIF
    if sample.starts_with(b"GIF8") {
        return true;
    }
    // PDF
    if sample.starts_with(b"%PDF") {
        return true;
    }
    // ISO / DMG / raw disk images
    if sample.starts_with(&[0x43, 0x44, 0x30, 0x30, 0x31]) {
        return true; // CD001
    }
    // WebAssembly (.wasm)
    if sample.starts_with(&[0x00, 0x61, 0x73, 0x6D]) {
        return true;
    }
    false
}

/// 计算非可打印字符比例（排除 \t \n \r）
fn non_printable_ratio(sample: &[u8]) -> f64 {
    if sample.is_empty() {
        return 0.0;
    }
    let non_printable = sample
        .iter()
        .filter(|&&b| b != b'\t' && b != b'\n' && b != b'\r' && (b < 0x20 || b == 0x7F))
        .count();
    non_printable as f64 / sample.len() as f64
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

/// 检查扩展名是否属于二进制/压缩文件（不应作文本读取）
pub fn is_binary_extension(path: &std::path::Path) -> bool {
    path.extension()
        .and_then(|ext| ext.to_str())
        .map(|ext| is_binary_ext_str(ext))
        .unwrap_or(false)
}

fn is_binary_ext_str(ext: &str) -> bool {
    matches!(
        ext.to_lowercase().as_str(),
        // 压缩/归档
        "zip" | "gz" | "tar" | "bz2" | "xz" | "7z" | "zst" | "lz4" | "rar" | "tgz" | "tbz2" | "txz"
        // 可执行/库
        | "exe" | "dll" | "so" | "dylib" | "pdb" | "sys" | "msi" | "app" | "bin"
        // 图片（直接读取无意义）
        | "png" | "jpg" | "jpeg" | "gif" | "webp" | "bmp" | "ico" | "icns" | "tiff" | "tif"
        // 音视频
        | "mp3" | "mp4" | "wav" | "ogg" | "flac" | "avi" | "mov" | "mkv" | "webm" | "m4a" | "aac"
        // 字体
        | "ttf" | "otf" | "woff" | "woff2" | "eot"
        // 文档（按文本读取无意义）
        | "pdf" | "doc" | "docx" | "xls" | "xlsx" | "ppt" | "pptx" | "odt" | "ods" | "odp"
        // 其他二进制
        | "wasm" | "class" | "pyc" | "pyo" | "o" | "obj" | "lib" | "a" | "dex" | "apk"
        | "whl" | "egg" | "rlib" | "rmeta" | "ilk" | "exp" | "res" | "manifest"
        | "pak" | "dat" | "db" | "sqlite" | "sqlite3" | "mdb" | "accdb"
    )
}

/// 检查文件扩展名并给出替代工具建议
pub fn binary_file_read_error(path: &std::path::Path) -> Option<String> {
    if !is_binary_extension(path) {
        return None;
    }
    let ext = path
        .extension()
        .and_then(|ext| ext.to_str())
        .unwrap_or("")
        .to_lowercase();
    let suggestion = match ext.as_str() {
        "zip" | "gz" | "tar" | "bz2" | "xz" | "7z" | "zst" | "lz4" | "rar" | "tgz" | "tbz2" | "txz" => {
            "这是压缩文件，请使用 RunCommand 执行解压命令（如 Expand-Archive / tar -xzf）查看内容，不要用 ReadFile 直接读取。"
        }
        "png" | "jpg" | "jpeg" | "gif" | "webp" | "bmp" | "ico" | "icns" | "tiff" | "tif" => {
            "这是图片文件，ReadFile 会读取无意义的二进制数据。请直接使用 ReadFile 查看图片（系统支持图片渲染）。"
        }
        "pdf" => {
            "这是 PDF 文件，请使用 ReadFile 并指定 pages 参数读取（如 pages: \"1-5\"），不要全文读取。"
        }
        "docx" | "xlsx" | "pptx" | "doc" | "xls" | "ppt" | "odt" | "ods" | "odp" => {
            "这是 Office 文档格式，无法直接按文本读取。如需查看内容，请使用对应的办公软件打开。"
        }
        "exe" | "dll" | "so" | "dylib" | "pdb" | "sys" | "bin" | "wasm" | "class" | "pyc" | "pyo" | "o" | "obj" | "lib" | "a" | "rlib" | "rmeta" | "ilk" => {
            "这是编译产物/二进制文件，无法按文本读取。请读取对应的源代码文件。"
        }
        "mp3" | "mp4" | "wav" | "ogg" | "flac" | "avi" | "mov" | "mkv" | "webm" | "m4a" | "aac" => {
            "这是音视频文件，无法按文本读取。"
        }
        "db" | "sqlite" | "sqlite3" | "mdb" | "accdb" => {
            "这是数据库文件，请使用对应的数据库工具查看，不要用 ReadFile 直接读取。"
        }
        _ => "这是二进制文件格式，无法按文本读取。",
    };
    Some(format!(
        "读取错误: 文件 '{}' 的扩展名 .{} 表明这是二进制格式。\n{}",
        path.display(),
        ext,
        suggestion
    ))
}

pub(super) fn is_locked_file_error(err_msg: &str) -> bool {
    err_msg.contains("Access is denied")
        || err_msg.contains("os error 32")
        || err_msg.contains("os error 5")
        || err_msg.contains("being used by another process")
}

/// 检测 Windows UNC 路径 (\\server\share\...)
pub(super) fn is_unc_path(path: &str) -> bool {
    path.starts_with("\\\\") || path.starts_with("//")
}

/// UNC 路径拦截的通用错误消息
pub(super) fn unc_path_rejection(tool: &str, path: &str) -> String {
    format!(
        "{}失败: 不支持 UNC 路径 ({})。请使用本地映射驱动器或复制文件到本地工作区。",
        tool, path
    )
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

    #[test]
    fn detects_gzip_magic_bytes() {
        // gzip magic: 1F 8B
        let bytes = vec![0x1F, 0x8B, 0x08, 0x00];
        assert!(looks_like_binary(&bytes));
    }

    #[test]
    fn detects_zip_magic_bytes() {
        // zip magic: 50 4B 03 04
        let bytes = vec![0x50, 0x4B, 0x03, 0x04, 0x00, 0x00];
        assert!(looks_like_binary(&bytes));
    }

    #[test]
    fn detects_pe_magic_bytes() {
        // Windows PE magic: MZ
        let bytes = b"MZ\x00\x00PE\x00\x00".to_vec();
        assert!(looks_like_binary(&bytes));
    }

    #[test]
    fn detects_binary_by_non_printable_ratio() {
        // 50% non-printable bytes
        let mut bytes = vec![b'a'; 500];
        bytes.extend(vec![0x01; 500]); // 500 non-printable + 500 printable = 50%
        assert!(looks_like_binary(&bytes));
    }

    #[test]
    fn text_file_passes_all_checks() {
        // Normal text should NOT be detected as binary
        let bytes = b"fn main() {\n    println!(\"Hello\");\n}\n".to_vec();
        assert!(!looks_like_binary(&bytes));
    }

    #[test]
    fn utf8_with_unicode_passes() {
        let bytes = "中文 hello мир".as_bytes().to_vec();
        assert!(!looks_like_binary(&bytes));
    }

    #[test]
    fn binary_extensions_are_detected_for_tar_gz() {
        assert!(is_binary_extension(std::path::Path::new("archive.tar.gz")));
        assert!(is_binary_extension(std::path::Path::new("app.exe")));
        assert!(is_binary_extension(std::path::Path::new("lib.dll")));
        assert!(is_binary_extension(std::path::Path::new("debug.pdb")));
        assert!(is_binary_extension(std::path::Path::new("data.zip")));
        assert!(is_binary_extension(std::path::Path::new("image.png")));
    }

    #[test]
    fn text_extensions_are_not_binary() {
        assert!(!is_binary_extension(std::path::Path::new("main.rs")));
        assert!(!is_binary_extension(std::path::Path::new("app.tsx")));
        assert!(!is_binary_extension(std::path::Path::new("README.md")));
        assert!(!is_binary_extension(std::path::Path::new(".gitignore")));
        assert!(!is_binary_extension(std::path::Path::new("Makefile.toml")));
    }

    #[test]
    fn svg_is_not_binary() {
        assert!(!is_binary_extension(std::path::Path::new("icon.svg")));
    }

    #[test]
    fn unc_path_detection() {
        assert!(is_unc_path("\\\\server\\share\\file.txt"));
        assert!(is_unc_path("//server/share/file.txt"));
        assert!(!is_unc_path("C:\\Users\\test\\file.txt"));
        assert!(!is_unc_path("/home/user/file.txt"));
        assert!(!is_unc_path("./relative/path.txt"));
    }
}
