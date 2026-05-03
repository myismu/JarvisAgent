//! # security.rs — Shell 安全门面模块
//!
//! 将细粒度的安全检查封装为统一的跨平台接口，并提取危险命令警告。
//!
//! ## Key Exports
//! - check_command_safety(): 主入口，根据平台自动分发安全性检查
//! - get_destructive_warning(): 获取破坏性操作的具体告警文本
//!
//! ## Dependencies
//! - Internal: super::types, super::guards, super::readonly, super::regexes

use super::guards::*;
use super::regexes::*;
pub use super::types::SafetyResult;

/// 检测破坏性命令，返回警告信息（仅用于权限确认显示，不拦截）
pub fn get_destructive_warning(cmd: &str) -> Option<String> {
    let mut warnings = Vec::new();

    if destructive_remove_re().is_match(cmd) {
        warnings.push("⚠ 检测到递归删除操作（Remove-Item -Recurse / rm -rf）");
    }
    if destructive_git_re().is_match(cmd) {
        warnings
            .push("⚠ 检测到 Git 破坏性操作（reset --hard / push --force / clean -f / stash drop）");
    }
    if destructive_sql_re().is_match(cmd) {
        warnings.push("⚠ 检测到 SQL 破坏性操作（DROP TABLE / TRUNCATE）");
    }
    if destructive_system_re().is_match(cmd) {
        warnings
            .push("⚠ 检测到系统级破坏性操作（Stop-Computer / Format-Volume / Clear-RecycleBin）");
    }
    if destructive_clear_content_re().is_match(cmd) {
        warnings.push("⚠ 检测到 Clear-Content 配合通配符（可能清空多个文件内容）");
    }

    if warnings.is_empty() {
        None
    } else {
        Some(warnings.join("\n"))
    }
}

// --- Unix/bash 特定安全检查（参考 bashSecurity.ts） ---
/// 返回第一个匹配的 Block 或所有 Warn 的合并。
pub fn check_command_safety(cmd: &str) -> SafetyResult {
    if cfg!(target_os = "windows") {
        check_command_safety_windows(cmd)
    } else {
        check_command_safety_unix(cmd)
    }
}
