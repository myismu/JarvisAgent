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

/// 文件大小限制：超过此大小拒绝读取（256KB）
pub(super) const MAX_FILE_SIZE_BYTES: u64 = 256 * 1024;

/// 输出行数限制：超过此行数自动截断
pub(super) const MAX_LINES_DEFAULT: usize = 2000;

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
