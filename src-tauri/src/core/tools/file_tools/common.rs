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
//! - `passes_file_filters()`: 文件过滤唯一入口（类型 + include + exclude）
//! - `input_usize()` / `input_string_list()` / `input_patterns()`: 工具入参解析

use std::io;
use std::path::{Path, PathBuf};

use encoding_rs::{GBK, UTF_16BE, UTF_16LE};
use sha2::{Digest, Sha256};
use tauri::Manager;

/// 文件大小限制：超过此大小拒绝读取（256KB）
pub(super) const MAX_FILE_SIZE_BYTES: u64 = 256 * 1024;

/// 输出行数限制：超过此行数自动截断
pub(super) const MAX_LINES_DEFAULT: usize = 2000;

/// 从工具调用参数中提取 file path，兼容 path / file_path / filePath 三种命名
/// 自动剥离 \\?\ 前缀（Windows 扩展长度路径前缀，文件系统不识别）
pub(super) fn resolve_path(input: &serde_json::Value) -> String {
    let raw = input["path"]
        .as_str()
        .or_else(|| input["file_path"].as_str())
        .or_else(|| input["filePath"].as_str())
        .unwrap_or("");
    strip_extended_path_prefix(raw)
}

/// 剥离 Windows 扩展长度路径前缀 \\?\
fn strip_extended_path_prefix(path: &str) -> String {
    if path.starts_with("\\\\?\\") {
        path[4..].to_string()
    } else {
        path.to_string()
    }
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
    if sample.starts_with(b"Rar!\x1A\x07") || sample.starts_with(b"Rar!\x1A\x07\x01\x00") {
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

// ===================== 工具入参解析 =====================

/// 从工具入参读取 usize 计数，兼容数字与字符串两种写法。
pub(super) fn input_usize(input: &serde_json::Value, key: &str) -> Option<usize> {
    let value = input.get(key)?;
    if let Some(value) = value.as_u64() {
        return Some(value as usize);
    }
    value.as_str().and_then(|value| value.trim().parse().ok())
}

/// 从工具入参读取字符串列表，兼容 `"a,b"` 字符串与 `["a","b"]` 数组两种写法。
pub(super) fn input_string_list(input: &serde_json::Value, key: &str) -> Vec<String> {
    if let Some(raw) = input[key].as_str() {
        return raw
            .split(|ch| ch == ',' || ch == ' ')
            .map(str::trim)
            .filter(|part| !part.is_empty())
            .map(str::to_string)
            .collect();
    }

    input[key]
        .as_array()
        .map(|items| {
            items
                .iter()
                .filter_map(|item| item.as_str())
                .map(str::trim)
                .filter(|part| !part.is_empty())
                .map(str::to_string)
                .collect()
        })
        .unwrap_or_default()
}

/// 按逗号/空格把一个 glob 串切成多条规则（`"**/*.rs, **/*.ts"`）。
pub(super) fn split_glob_patterns(glob: &str) -> Vec<String> {
    glob.split(|ch| ch == ',' || ch == ' ')
        .map(str::trim)
        .filter(|part| !part.is_empty())
        .map(str::to_string)
        .collect()
}

/// 从工具入参读取 glob 规则列表（仅字符串写法，多条以逗号/空格分隔）。
pub(super) fn input_patterns(input: &serde_json::Value, key: &str) -> Vec<String> {
    input[key]
        .as_str()
        .map(split_glob_patterns)
        .unwrap_or_default()
}

// ===================== 路径显示与排序 =====================

/// 转成相对进程当前目录、正斜杠分隔的展示路径；不在当前目录下则原样输出。
pub(super) fn display_path(path: &Path) -> String {
    let display = std::env::current_dir()
        .ok()
        .and_then(|cwd| path.strip_prefix(cwd).ok().map(PathBuf::from))
        .unwrap_or_else(|| path.to_path_buf());

    display.to_string_lossy().replace('\\', "/")
}

/// 路径中是否含有某个目录分量（如 `src`）。
pub(super) fn path_contains_component(path: &Path, component: &str) -> bool {
    path.components().any(|part| part.as_os_str() == component)
}

/// 是否常见代码文件（结果排序用：代码文件优先）。
pub(super) fn is_code_file(path: &Path) -> bool {
    path.extension()
        .and_then(|ext| ext.to_str())
        .map(|ext| {
            matches!(
                ext.to_lowercase().as_str(),
                "rs" | "ts"
                    | "tsx"
                    | "js"
                    | "jsx"
                    | "vue"
                    | "py"
                    | "go"
                    | "java"
                    | "c"
                    | "h"
                    | "cpp"
                    | "hpp"
                    | "cs"
                    | "php"
                    | "rb"
                    | "html"
                    | "css"
                    | "scss"
            )
        })
        .unwrap_or(false)
}

/// 搜索结果排序键：`src/` 下的代码文件优先，其余按展示路径字典序。
pub(super) fn search_path_rank(path: &Path) -> (usize, usize, String) {
    (
        if path_contains_component(path, "src") {
            0
        } else {
            1
        },
        if is_code_file(path) { 0 } else { 1 },
        display_path(path),
    )
}

// ===================== glob 过滤（include / exclude 语义分离）=====================
//
// 这里是四个搜索类工具曾集体失效的地方，改动前请先读完这段说明。
//
// 旧实现只有一个 `matches_any_glob(path, base, patterns)`，语义是"规则为空时
// 返回 true"（无规则 = 全通过）。用在 include 上是对的，但调用方写成
// `!matches_any_glob(..., exclude_patterns)` —— 于是"没有排除规则"被算成
// `!true = false`，**每个文件都被判定为被排除**，文件列表被清空，
// FindSymbol / FindReferences / CodeSearch / SearchRepo 一律返回"未找到"。
//
// 该 bug 自 2026-05-05 起存在，且因为 symbol.rs 与 search.rs 各抄了一份，
// 只修一份也治不好。现在只此一处实现，并把两个方向拆成名字自明的函数：
//   - `passes_include_globs`：空 = 不限制
//   - `hits_exclude_globs`  ：空 = 无命中
// 取反写在 `passes_file_filters` 里，且取反的对象是"是否命中排除规则"，
// 不再是"是否全通过"。

/// glob 通配符转正则：`*` 任意多字符、`?` 单字符，其余正则元字符转义。
fn glob_to_regex(pattern: &str) -> Option<regex::Regex> {
    let mut out = String::from("^");
    for ch in pattern.replace('\\', "/").chars() {
        match ch {
            '*' => out.push_str(".*"),
            '?' => out.push('.'),
            '/' => out.push('/'),
            ch if ".+()^$|[]{}\\".contains(ch) => {
                out.push('\\');
                out.push(ch);
            }
            ch => out.push(ch),
        }
    }
    out.push('$');
    regex::Regex::new(&out).ok()
}

/// 单条 glob 是否匹配路径。不含 `/` 的规则额外按文件名匹配（`*.rs` 命中任意层级）。
fn glob_matches(pattern: &str, path: &Path) -> bool {
    let normalized = path.to_string_lossy().replace('\\', "/");
    let file_name = path
        .file_name()
        .map(|name| name.to_string_lossy().replace('\\', "/"))
        .unwrap_or_default();
    glob_to_regex(pattern)
        .map(|re| re.is_match(&normalized) || (!pattern.contains('/') && re.is_match(&file_name)))
        .unwrap_or(false)
}

/// 相对基准目录的路径；不在基准下则用原路径。
fn relative_to<'a>(path: &'a Path, base: &Path) -> &'a Path {
    path.strip_prefix(base).unwrap_or(path)
}

/// include 规则：**没有规则 = 不限制**，任何文件都通过。
pub(super) fn passes_include_globs(path: &Path, base: &Path, patterns: &[String]) -> bool {
    if patterns.is_empty() {
        return true;
    }
    let relative = relative_to(path, base);
    patterns
        .iter()
        .any(|pattern| glob_matches(pattern, relative))
}

/// exclude 规则：**没有规则 = 不排除**，无人命中。
pub(super) fn hits_exclude_globs(path: &Path, base: &Path, patterns: &[String]) -> bool {
    let relative = relative_to(path, base);
    patterns
        .iter()
        .any(|pattern| glob_matches(pattern, relative))
}

// ===================== 文件类型过滤 =====================

/// 类型别名到扩展名集合；未收录的类型按"扩展名等于类型名"处理。
pub(super) fn type_extensions(file_type: &str) -> Vec<&'static str> {
    match file_type.to_lowercase().as_str() {
        "ts" | "typescript" => vec!["ts", "tsx"],
        "js" | "javascript" => vec!["js", "jsx", "mjs", "cjs"],
        "rs" | "rust" => vec!["rs"],
        "vue" => vec!["vue"],
        "py" | "python" => vec!["py"],
        "md" | "markdown" => vec!["md", "mdx"],
        "json" => vec!["json"],
        _ => Vec::new(),
    }
}

/// 文件类型是否匹配；未指定类型时一律通过。
pub(super) fn matches_file_type(path: &Path, file_type: Option<&str>) -> bool {
    let Some(file_type) = file_type else {
        return true;
    };
    let extensions = type_extensions(file_type);
    path.extension()
        .and_then(|ext| ext.to_str())
        .map(|ext| {
            let ext = ext.to_lowercase();
            if extensions.is_empty() {
                ext == file_type.to_lowercase()
            } else {
                extensions.iter().any(|candidate| *candidate == ext)
            }
        })
        .unwrap_or(false)
}

// ===================== 文件过滤唯一入口 =====================

/// 文件是否通过过滤：类型 + include + exclude。
///
/// 全 file_tools 只有这一处实现（symbol.rs 与 search.rs 曾各有一份拷贝，
/// 两处都写错了 exclude 的空集合语义，见上方说明）。
pub(super) fn passes_file_filters(
    path: &Path,
    base: &Path,
    include_patterns: &[String],
    exclude_patterns: &[String],
    file_type: Option<&str>,
) -> bool {
    matches_file_type(path, file_type)
        && passes_include_globs(path, base, include_patterns)
        && !hits_exclude_globs(path, base, exclude_patterns)
}

/// 目录遍历时是否跳过该目录名（内置忽略名单 + 调用方自带名单）。
pub(super) fn is_skippable_dir_name(name: &str, ignore_dirs: &[String]) -> bool {
    is_ignored_entry_name(name) || ignore_dirs.iter().any(|ignored| ignored == name)
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
        // 这两条建议原先都在描述**不存在的能力**，2026-09-21 修正：
        // - 图片：原文写"请直接使用 ReadFile 查看图片（系统支持图片渲染）"，但图片本就在
        //   黑名单里 —— 这条建议自己被触发就证明文件已被拦下，模型照做只会再被拦一次。
        //   读取图片的能力从未实现（read_file() 只走文本解码，无任何图片分支）。
        // - PDF：原文让模型"指定 pages 参数"，而 ReadFile 的 schema 里根本没有这个参数
        //   （只有 path/start_line/end_line），提示词那侧还写着同一句 ——
        //   两处互相印证一个假参数，模型试错后也无从纠正。
        "png" | "jpg" | "jpeg" | "gif" | "webp" | "bmp" | "ico" | "icns" | "tiff" | "tif" => {
            "这是图片文件，无法按文本读取。如需看图，请把图片直接发送到对话里。"
        }
        "pdf" => {
            "这是 PDF 文件，无法按文本读取。"
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
/// 排除 \\?\ 前缀（Windows 扩展长度路径前缀，不是 UNC 路径）
pub(super) fn is_unc_path(path: &str) -> bool {
    (path.starts_with("\\\\") && !path.starts_with("\\\\?\\")) || path.starts_with("//")
}

/// UNC 路径拦截的通用错误消息
pub(super) fn unc_path_rejection(tool: &str, path: &str) -> String {
    format!(
        "{}失败: 不支持 UNC 路径 ({})。请使用本地映射驱动器或复制文件到本地工作区。",
        tool, path
    )
}

// ---------------------------------------------------------------------------
// 「先读后改」闸门（read-before-write）
//
// 解决的问题：模型凭陈旧认知盲改文件。两种失败形态：
// 1. 没读过就改 —— old_text 靠猜，匹配失败烧轮次，碰巧匹配上则改在错误前提上；
// 2. 读过但文件随后被外部改了（其他 agent / 用户 / git）—— 编辑按旧认知落盘，
//    静默覆盖别人的修改。这是最坏的失败形态：无声、且发现时已隔着多轮。
//
// 机制：会话内存表「路径 → 内容指纹」（SessionContext::read_file_fingerprints）。
// 读类工具成功后记录"模型看到的版本"；写改类工具落盘前比对当前内容指纹，
// 不一致或无记录即拦截。mtime 不参与跨轮判定（时钟精度依赖文件系统，
// FAT32 秒级刻度会漏检同刻修改），指纹对内容取 SHA-256，改 1 字节必变。
//
// 唯一口径：校验逻辑只有本文件的 `ensure_fresh_read`，三个写改工具各插一行调用；
// 记录逻辑只有 `record_file_read`，五个接入点（两个读 + 三个写）共用。
// ---------------------------------------------------------------------------

/// 对解码后的文本内容取 SHA-256 指纹（hex 字符串）。
///
/// 对**解码后**的内容而非原始字节计算：同一段内容以 GBK 或 UTF-8 存放，
/// 模型看到的文本一致即视为"认知未过时"——编码层转存不算文件变更。
pub(super) fn content_fingerprint(content: &str) -> String {
    let mut hasher = Sha256::new();
    hasher.update(content.as_bytes());
    format!("{:x}", hasher.finalize())
}

/// 纯判定（供单测与 `_on_ctx` 复用）：
/// - 无记录 → 要求先读；
/// - 记录与当前指纹不一致 → 文件在读取后被外部修改，要求重读；
/// - 一致 → 放行。
fn freshness_error(recorded: Option<&str>, current_fingerprint: &str) -> Option<String> {
    match recorded {
        None => Some(
            "修改中止: 文件在本次会话中尚未被读取过。请先用 ReadFile 读取该文件了解当前内容，再执行修改。"
                .to_string(),
        ),
        Some(recorded) if recorded != current_fingerprint => Some(
            "修改中止: 文件自上次读取后被外部修改过（可能是其他程序、其他智能体或 git 操作）。请重新 ReadFile 获取当前内容后再修改。"
                .to_string(),
        ),
        Some(_) => None,
    }
}

/// 记录一次「模型看到了这个文件的这个版本」。
///
/// 写入点：ReadFile / ReadSymbol 成功返回前；三个写改工具成功落盘后
/// （此时传**写入后的新内容**，刷新认知，连续编辑不会误报）。
pub(super) async fn record_file_read(
    app: &tauri::AppHandle,
    session_id: &str,
    path: &str,
    content: &str,
) {
    if let Some(manager) = app.try_state::<crate::infra::state::state::SessionManager>() {
        let ctx = manager.get_or_create(session_id).await;
        record_file_read_on_ctx(&ctx, path, content).await;
    }
}

/// 「先读后改」校验 —— 写改类工具落盘前的统一闸门。
///
/// `path` 必须是 `resolve_exec_path` 之后的路径（与记录侧同一 key 形式）；
/// `current_content` 是本次工具执行中已经读到的**当前**文件内容
/// （EditFile / WriteFile / ApplyPatch 落盘前本来就持有，指纹计算零额外 I/O）。
///
/// 语义边界：目标文件不存在的分支由调用方处理（EditFile 读不到文件早在
/// 上游报错；WriteFile / ApplyPatch 的新建文件无需"读过"，直接放行）。
/// 没有 SessionManager（单测 / 启动极早期）时放行 —— 与 `tool_filter_for`
/// 同一先例：开关类守卫在拿不到会话时不锁死功能，生产路径必有 SessionManager。
pub(super) async fn ensure_fresh_read(
    app: &tauri::AppHandle,
    session_id: &str,
    path: &str,
    current_content: &str,
) -> Result<(), String> {
    let Some(manager) = app.try_state::<crate::infra::state::state::SessionManager>() else {
        return Ok(());
    };
    let ctx = manager.get_or_create(session_id).await;
    ensure_fresh_read_on_ctx(&ctx, path, current_content).await
}

/// `record_file_read` 的核心（不依赖 AppHandle，供单测构造全链路场景）。
async fn record_file_read_on_ctx(
    ctx: &crate::infra::state::state::SessionContext,
    path: &str,
    content: &str,
) {
    let fingerprint = content_fingerprint(content);
    ctx.read_file_fingerprints
        .lock()
        .await
        .insert(path.to_string(), fingerprint);
}

/// `ensure_fresh_read` 的核心（不依赖 AppHandle，供单测构造全链路场景）。
async fn ensure_fresh_read_on_ctx(
    ctx: &crate::infra::state::state::SessionContext,
    path: &str,
    current_content: &str,
) -> Result<(), String> {
    let recorded = ctx.read_file_fingerprints.lock().await.get(path).cloned();
    match freshness_error(recorded.as_deref(), &content_fingerprint(current_content)) {
        Some(err) => Err(format!("{} (文件: {})", err, path)),
        None => Ok(()),
    }
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

    #[test]
    fn fingerprint_is_stable_and_sensitive() {
        let a = content_fingerprint("hello\nworld\n");
        // 同内容同指纹
        assert_eq!(a, content_fingerprint("hello\nworld\n"));
        // 任何字节差异（空格、行尾）都算变更
        assert_ne!(a, content_fingerprint("hello\nworld \n"));
        assert_ne!(a, content_fingerprint("hello\nworld"));
    }

    #[test]
    fn freshness_blocks_never_read_and_stale_versions() {
        let fp = content_fingerprint("v1");
        // 从未读过 → 拦
        assert!(freshness_error(None, &fp).is_some());
        // 读过且当前内容未变 → 放行
        assert!(freshness_error(Some(&fp), &fp).is_none());
        // 读过但内容已变 → 拦
        assert!(freshness_error(Some(&fp), &content_fingerprint("v2")).is_some());
    }

    /// 全链路场景（不依赖 AppHandle）：没读拦 → 读后放 → 外部变更拦 →
    /// 重读放 → 写后刷新不误报。
    #[tokio::test]
    async fn read_then_edit_cycle_with_external_change() {
        let ctx = crate::infra::state::state::SessionContext::new("test-session".into());
        let path = "E:\\proj\\main.rs";

        // 没读过就改 → 拦
        assert!(ensure_fresh_read_on_ctx(&ctx, path, "v1").await.is_err());

        // ReadFile 记录 v1 → 同版本修改放行
        record_file_read_on_ctx(&ctx, path, "v1").await;
        assert!(ensure_fresh_read_on_ctx(&ctx, path, "v1").await.is_ok());

        // 外部把文件改成 v2 → 拦（防止按旧认知盲改、覆盖别人的修改）
        assert!(ensure_fresh_read_on_ctx(&ctx, path, "v2").await.is_err());

        // 模型重读 v2 → 再改放行
        record_file_read_on_ctx(&ctx, path, "v2").await;
        assert!(ensure_fresh_read_on_ctx(&ctx, path, "v2").await.is_ok());

        // 写改工具成功落盘后刷新为新版本 v3 → 紧接着的下一条编辑不误报
        record_file_read_on_ctx(&ctx, path, "v3").await;
        assert!(ensure_fresh_read_on_ctx(&ctx, path, "v3").await.is_ok());
    }

    // ============ 搜索过滤：空规则语义（回归护栏）============
    //
    // 背景：`!matches_any_glob(空)` 曾把"没有排除规则"算成"排除所有文件"，
    // 导致 FindSymbol / FindReferences / CodeSearch / SearchRepo 四个工具
    // 在不传 exclude 时一律返回"未找到"（自 2026-05-05 起，持续四个半月）。

    fn path_of(s: &str) -> PathBuf {
        PathBuf::from(s)
    }

    #[test]
    fn include_globs_empty_means_no_restriction() {
        assert!(passes_include_globs(
            &path_of("proj/src/a.rs"),
            &path_of("proj"),
            &[]
        ));
    }

    #[test]
    fn exclude_globs_empty_means_nothing_excluded() {
        // 空 exclude 必须返回"未命中"；否则取反后会把所有文件排除掉
        assert!(!hits_exclude_globs(
            &path_of("proj/src/a.rs"),
            &path_of("proj"),
            &[]
        ));
    }

    #[test]
    fn file_filters_pass_without_any_patterns() {
        // 三个搜索类工具的默认调用形态：只给目录，不给 include / exclude / type
        let base = path_of("proj");
        for file in [
            "proj/src/a.rs",
            "proj/src/a.ts",
            "proj/src/a.vue",
            "proj/README.md",
        ] {
            assert!(
                passes_file_filters(&path_of(file), &base, &[], &[], None),
                "{} 应在无任何规则时通过过滤",
                file
            );
        }
    }

    #[test]
    fn file_filters_still_apply_real_rules() {
        let base = path_of("proj");
        let rs_only = vec!["**/*.rs".to_string()];

        // include 有规则时照常生效
        assert!(passes_file_filters(
            &path_of("proj/src/a.rs"),
            &base,
            &rs_only,
            &[],
            None
        ));
        assert!(!passes_file_filters(
            &path_of("proj/src/a.ts"),
            &base,
            &rs_only,
            &[],
            None
        ));

        // exclude 有规则时照常生效
        assert!(!passes_file_filters(
            &path_of("proj/src/a.rs"),
            &base,
            &[],
            &rs_only,
            None
        ));
        assert!(passes_file_filters(
            &path_of("proj/src/a.ts"),
            &base,
            &[],
            &rs_only,
            None
        ));

        // type 过滤照常生效
        assert!(passes_file_filters(
            &path_of("proj/src/a.rs"),
            &base,
            &[],
            &[],
            Some("rust")
        ));
        assert!(!passes_file_filters(
            &path_of("proj/src/a.ts"),
            &base,
            &[],
            &[],
            Some("rust")
        ));
    }

    #[test]
    fn skippable_dir_covers_builtin_and_custom() {
        let custom = vec!["vendor".to_string()];
        assert!(is_skippable_dir_name("node_modules", &custom));
        assert!(is_skippable_dir_name("target", &custom));
        assert!(is_skippable_dir_name("vendor", &custom));
        assert!(!is_skippable_dir_name("src", &custom));
    }
}
