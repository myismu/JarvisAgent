//! # guards.rs — Shell 命令安全校验函数
//!
//! 包含针对 Windows/Unix 的细粒度命令校验逻辑，如反向 Shell 检测、提权检测等。
//!
//! ## Key Exports
//! - `check_command_safety_windows()`: 检查 Windows 命令安全性
//! - `check_command_safety_unix()`: 检查 Unix 命令安全性
//!
//! ## Dependencies
//! - Internal: `super::types::SafetyResult`, `super::regexes`
//! - External: `regex`

use super::regexes::*;
use super::types::SafetyResult;

pub fn check_control_chars(cmd: &str) -> Option<SafetyResult> {
    if control_char_re().is_match(cmd) {
        Some(SafetyResult::Block(
            "命令包含不可见控制字符，可能是注入攻击。".to_string(),
        ))
    } else {
        None
    }
}

pub fn check_reverse_shell(cmd: &str) -> Option<SafetyResult> {
    if reverse_shell_re().is_match(cmd) {
        Some(SafetyResult::Block(
            "检测到反向 Shell 模式（/dev/tcp, mkfifo, nc -e 等），禁止执行。".to_string(),
        ))
    } else {
        None
    }
}

pub fn check_encoding(cmd: &str) -> Option<SafetyResult> {
    if base64_decode_re().is_match(cmd) {
        Some(SafetyResult::Block(
            "检测到 base64 解码或编码混淆操作，可能隐藏恶意命令。如需解码，请告知用户手动操作。"
                .to_string(),
        ))
    } else {
        None
    }
}

pub fn check_dangerous_cmdlet(cmd: &str) -> Option<SafetyResult> {
    if dangerous_ps_cmdlet_re().is_match(cmd) {
        Some(SafetyResult::Block("检测到危险 PowerShell 命令（Invoke-Expression/Invoke-WebRequest/Start-Process/Net.WebClient 等），禁止执行。".to_string()))
    } else {
        None
    }
}

pub fn check_dangerous_variables(cmd: &str) -> Option<SafetyResult> {
    if dangerous_variable_re().is_match(cmd) {
        Some(SafetyResult::Block(
            "检测到危险变量注入（$RANDOM/$PPID/$IFS 等），禁止执行。".to_string(),
        ))
    } else {
        None
    }
}

pub fn check_obfuscated_flags(cmd: &str) -> Option<SafetyResult> {
    if obfuscated_flag_re().is_match(cmd) {
        Some(SafetyResult::Block(
            "检测到混淆标志（ANSI-C 引用或空引号拼接），可能是绕过安全检查的尝试。".to_string(),
        ))
    } else {
        None
    }
}

/// 检查 PowerShell 编码命令（绕过所有安全检查的最危险方式之一）
pub fn check_encoded_command(cmd: &str) -> Option<SafetyResult> {
    if encoded_command_re().is_match(cmd) {
        Some(SafetyResult::Block(
            "检测到 PowerShell -EncodedCommand 参数，这是绕过安全检查的常见手段，禁止执行。"
                .to_string(),
        ))
    } else {
        None
    }
}

/// 检查 Windows 内置下载工具
pub fn check_download_utilities(cmd: &str) -> Option<SafetyResult> {
    if download_utility_re().is_match(cmd) {
        Some(SafetyResult::Block("检测到 Windows 内置下载工具（certutil/bitsadmin/Start-BitsTransfer），可能用于下载恶意载荷，禁止执行。".to_string()))
    } else {
        None
    }
}

/// 检查 COM 对象创建
pub fn check_com_object(cmd: &str) -> Option<SafetyResult> {
    if com_object_re().is_match(cmd) {
        Some(SafetyResult::Block(
            "检测到 New-Object -ComObject，COM 对象可执行任意系统操作，禁止执行。".to_string(),
        ))
    } else {
        None
    }
}

/// 检查计划任务创建
pub fn check_scheduled_task(cmd: &str) -> Option<SafetyResult> {
    if scheduled_task_re().is_match(cmd) {
        Some(SafetyResult::Block(
            "检测到计划任务创建（Register-ScheduledTask/schtasks），可能用于持久化攻击，禁止执行。"
                .to_string(),
        ))
    } else {
        None
    }
}

/// 检查提权操作
pub fn check_runas(cmd: &str) -> Option<SafetyResult> {
    if runas_re().is_match(cmd) {
        Some(SafetyResult::Block(
            "检测到 Start-Process -Verb RunAs 提权操作，禁止执行。".to_string(),
        ))
    } else {
        None
    }
}

/// 检查 WMI 远程执行
pub fn check_wmi_invoke(cmd: &str) -> Option<SafetyResult> {
    if wmi_invoke_re().is_match(cmd) {
        Some(SafetyResult::Block(
            "检测到 Invoke-WmiMethod/Invoke-CimMethod，WMI 可用于远程执行，禁止执行。".to_string(),
        ))
    } else {
        None
    }
}

/// 检查 UNC 路径（网络路径访问）
pub fn check_unc_path(cmd: &str) -> Option<SafetyResult> {
    if unc_path_re().is_match(cmd) {
        Some(SafetyResult::Block(
            "检测到 UNC 网络路径（\\\\server\\share），禁止访问网络资源。".to_string(),
        ))
    } else {
        None
    }
}

// --- Warn 类检查 ---

pub fn check_long_running(cmd: &str) -> Option<SafetyResult> {
    if long_running_re().is_match(cmd) {
        Some(SafetyResult::Warn(
            "检测到长周期命令（开发服务器等）。建议使用 `run_in_background: true` 以避免阻塞对话。"
                .to_string(),
        ))
    } else {
        None
    }
}

pub fn check_sleep(cmd: &str) -> Option<SafetyResult> {
    if sleep_re().is_match(cmd) {
        Some(SafetyResult::Warn(
            "检测到 sleep 命令。请确认是否必要——大多数场景应避免 sleep，改用事件驱动或轮询。"
                .to_string(),
        ))
    } else {
        None
    }
}

pub fn check_node_modules(cmd: &str) -> Option<SafetyResult> {
    if node_modules_re().is_match(cmd) {
        Some(SafetyResult::Warn("禁止使用 shell 浏览 node_modules 目录（会产生数千行无用输出）。请使用 list_directory 工具或直接假设依赖已安装。".to_string()))
    } else {
        None
    }
}

pub fn check_command_substitution(cmd: &str) -> Option<SafetyResult> {
    if command_substitution_re().is_match(cmd) {
        Some(SafetyResult::Warn(
            "命令包含 $() 子表达式。请确认子命令安全。".to_string(),
        ))
    } else {
        None
    }
}

/// 检查模块加载/安装
pub fn check_module_loading(cmd: &str) -> Option<SafetyResult> {
    if module_loading_re().is_match(cmd) {
        Some(SafetyResult::Warn(
            "检测到 PowerShell 模块操作（Import-Module/Install-Module）。请确认模块来源可信。"
                .to_string(),
        ))
    } else {
        None
    }
}

/// 检查 .NET 静态方法调用
pub fn check_dotnet_method(cmd: &str) -> Option<SafetyResult> {
    if dotnet_method_re().is_match(cmd) {
        Some(SafetyResult::Warn(
            "命令包含 .NET 静态方法调用 [Type]::Method()。请确认调用安全。".to_string(),
        ))
    } else {
        None
    }
}

/// 检查别名操作
pub fn check_alias_manipulation(cmd: &str) -> Option<SafetyResult> {
    if alias_manipulation_re().is_match(cmd) {
        Some(SafetyResult::Warn(
            "检测到别名操作（Set-Alias/New-Alias），修改别名可能劫持后续命令。".to_string(),
        ))
    } else {
        None
    }
}

/// 检查 New-Object -TypeName（非 COM）
pub fn check_new_object_typename(cmd: &str) -> Option<SafetyResult> {
    if new_object_typename_re().is_match(cmd) && !com_object_re().is_match(cmd) {
        Some(SafetyResult::Warn(
            "检测到 New-Object -TypeName，创建 .NET 对象。请确认类型安全。".to_string(),
        ))
    } else {
        None
    }
}

// --- 只读命令检测 ---

/// 提取命令的第一个 token（cmdlet/命令名），处理管道和分号
pub fn extract_command_name(cmd: &str) -> String {
    let trimmed = cmd.trim();
    // 取第一个非空 token
    trimmed
        .split_whitespace()
        .next()
        .unwrap_or("")
        .trim_matches('"')
        .trim_matches('\'')
        .to_lowercase()
}

/// 检查命令是否为只读操作（可跳过危险命令权限确认）
///
/// 安全约束：
/// - 包含 $()、@、赋值 =、重定向 > 的命令不算只读
/// 检查 eval/source（Unix 特定高危）
pub fn check_eval(cmd: &str) -> Option<SafetyResult> {
    if eval_re().is_match(cmd) {
        Some(SafetyResult::Block(
            "检测到 eval/source 命令，可能执行任意代码，禁止执行。".to_string(),
        ))
    } else {
        None
    }
}

/// 检查 sudo（Unix 特定）
pub fn check_sudo(cmd: &str) -> Option<SafetyResult> {
    if sudo_re().is_match(cmd) {
        Some(SafetyResult::Warn(
            "检测到 sudo 命令。请确认是否需要管理员权限。".to_string(),
        ))
    } else {
        None
    }
}

/// 检查包管理器安装（Unix 特定）
pub fn check_package_install(cmd: &str) -> Option<SafetyResult> {
    if package_install_re().is_match(cmd) {
        Some(SafetyResult::Warn(
            "检测到包管理器安装命令。请确认安装来源可信。".to_string(),
        ))
    } else {
        None
    }
}

/// Unix/bash 安全检查（参考 bashSecurity.ts）
pub fn check_command_safety_unix(cmd: &str) -> SafetyResult {
    let trimmed = cmd.trim();

    if trimmed.is_empty() {
        return SafetyResult::Block("命令为空。".to_string());
    }

    // Block 类（Unix 特定 + 通用）
    let block_checks: &[fn(&str) -> Option<SafetyResult>] = &[
        check_control_chars,
        check_reverse_shell,
        check_encoding,
        check_eval,     // Unix 特定：eval/source
        check_unc_path, // 通用
        check_dangerous_variables,
        check_obfuscated_flags,
    ];

    for check in block_checks {
        if let Some(result) = check(trimmed) {
            return result;
        }
    }

    // Warn 类（Unix 特定 + 通用）
    let warn_checks: &[fn(&str) -> Option<SafetyResult>] = &[
        check_long_running,
        check_sleep,
        check_node_modules,
        check_command_substitution,
        check_sudo,            // Unix 特定
        check_package_install, // Unix 特定
    ];

    let mut warnings = Vec::new();
    for check in warn_checks {
        if let Some(SafetyResult::Warn(msg)) = check(trimmed) {
            warnings.push(msg);
        }
    }

    if !warnings.is_empty() {
        SafetyResult::Warn(warnings.join("\n"))
    } else {
        SafetyResult::Safe
    }
}

/// Windows/PowerShell 安全检查
pub fn check_command_safety_windows(cmd: &str) -> SafetyResult {
    let trimmed = cmd.trim();

    if trimmed.is_empty() {
        return SafetyResult::Block("命令为空。".to_string());
    }

    // Block 类（Windows 特定 + 通用）
    let block_checks: &[fn(&str) -> Option<SafetyResult>] = &[
        check_control_chars,
        check_reverse_shell,
        check_encoding,
        check_encoded_command,
        check_download_utilities,
        check_com_object,
        check_scheduled_task,
        check_runas,
        check_wmi_invoke,
        check_unc_path,
        check_dangerous_cmdlet,
        check_dangerous_variables,
        check_obfuscated_flags,
    ];

    for check in block_checks {
        if let Some(result) = check(trimmed) {
            return result;
        }
    }

    // Warn 类（Windows 特定 + 通用）
    let warn_checks: &[fn(&str) -> Option<SafetyResult>] = &[
        check_long_running,
        check_sleep,
        check_node_modules,
        check_command_substitution,
        check_module_loading,
        check_dotnet_method,
        check_alias_manipulation,
        check_new_object_typename,
    ];

    let mut warnings = Vec::new();
    for check in warn_checks {
        if let Some(SafetyResult::Warn(msg)) = check(trimmed) {
            warnings.push(msg);
        }
    }

    if !warnings.is_empty() {
        SafetyResult::Warn(warnings.join("\n"))
    } else {
        SafetyResult::Safe
    }
}

// --- Unix/bash 只读命令白名单 ---

pub const READONLY_UNIX_COMMANDS: &[&str] = &[
    // 文件系统读取
    "ls",
    "cat",
    "head",
    "tail",
    "less",
    "more",
    "file",
    "stat",
    "wc",
    "realpath",
    "readlink",
    "basename",
    "dirname",
    // 文本处理（只读）
    "grep",
    "egrep",
    "fgrep",
    "rg",
    "awk",
    "sed",
    "cut",
    "sort",
    "uniq",
    "tr",
    "tee",
    "xargs",
    // 系统信息
    "echo",
    "printf",
    "pwd",
    "env",
    "printenv",
    "which",
    "whereis",
    "whoami",
    "id",
    "uname",
    "date",
    "cal",
    "uptime",
    "df",
    "du",
    "free",
    "ps",
    "top",
    "htop",
    "lsof",
    "fuser",
    "hostname",
    "arch",
    "nproc",
    // 网络信息（只读）
    "ifconfig",
    "ip",
    "ss",
    "netstat",
    "ping",
    "traceroute",
    "dig",
    "nslookup",
    "host",
    "curl",
    "wget",
    // 版本控制（只读）
    "git",
    "gh",
    // 其他只读
    "tree",
    "find",
    "locate",
    "man",
    "info",
    "help",
    "diff",
    "cmp",
    "md5sum",
    "sha256sum",
    "tar",
    "gzip",
    "gunzip",
    "zip",
    "unzip",
];
