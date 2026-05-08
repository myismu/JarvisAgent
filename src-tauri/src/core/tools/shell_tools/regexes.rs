//! # regexes.rs — 预编译正则与常量字典
//!
//! 存储用于安全检查的正则表达式匹配器以及命令白/黑名单常量。
//!
//! ## Key Exports
//! - 各种正则表达式生成函数: 如 `control_char_re()`, `reverse_shell_re()`
//! - 各种常量字典: 如 `READONLY_UNIX_COMMANDS`, `READONLY_GH_ARGS`
//!
//! ## Dependencies
//! - External: `regex`, `std::sync::OnceLock`
//!
//! ## Constraints
//! - 使用 OnceLock 确保正则只编译一次

use regex::Regex;
use std::sync::OnceLock;

pub fn control_char_re() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| Regex::new(r"[\x00-\x08\x0B\x0C\x0E-\x1F\x7F]").unwrap())
}

pub fn reverse_shell_re() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| {
        Regex::new(
            r"(?i)(/dev/tcp|mkfifo|nc\s+-e|ncat\s+-e|socat\s+.*exec|bash\s+-i\s+>&|/dev/udp)",
        )
        .unwrap()
    })
}

pub fn base64_decode_re() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| {
        Regex::new(r"(?i)(base64\s+(-d|--decode)|xxd\s+-r|\[Convert\]::FromBase64String|FromBase64String|\[System\.Convert\]::FromBase64)").unwrap()
    })
}

pub fn dangerous_ps_cmdlet_re() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| {
        Regex::new(r"(?i)(Invoke-Expression|iex\s|Invoke-WebRequest|iwr\s|wget\s|curl\s|Start-Process|New-Object\s+Net\.WebClient|DownloadString|DownloadFile|DownloadData)").unwrap()
    })
}

pub fn long_running_re() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| {
        Regex::new(r"(?i)(npm\s+run\s+dev|npm\s+start|yarn\s+dev|yarn\s+start|pnpm\s+dev|pnpm\s+start|vite\b|vue-cli-service\s+serve|python\s+manage\.py\s+runserver|flask\s+run|uvicorn\s|npx\s+serve|http-server)").unwrap()
    })
}

pub fn sleep_re() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| Regex::new(r"(?i)(^|\s|;)(sleep\s+\d|Start-Sleep\s|timeout\s+/t\s)").unwrap())
}

pub fn dangerous_variable_re() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| {
        Regex::new(r"(?i)(\$RANDOM|\$PPID|\$LINENO|\$HOSTNAME|\$BASH_ENV|\$CDPATH|\$IFS)").unwrap()
    })
}

pub fn node_modules_re() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| {
        Regex::new(r"(?i)(dir\s+node_modules|ls\s+node_modules|Get-ChildItem\s+.*node_modules)")
            .unwrap()
    })
}

/// 递归列目录命令（容易无差别扫入 node_modules/.git 等数万文件）
pub fn recursive_listing_re() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| {
        Regex::new(
            r"(?i)(dir\s+/s\b|Get-ChildItem\s+.*-Recurse|tree\b|ls\s+.*-R\b|find\s+\.\s+-type|dir\s+/b\s+/s)",
        )
        .unwrap()
    })
}

/// 检查递归列目录命令是否排除了依赖目录
pub fn has_dependency_exclusion(cmd: &str) -> bool {
    let exclusions = [
        "node_modules", ".git", "target", "dist", "build",
        "__pycache__", ".next", ".nuxt", "vendor", "bower_components",
    ];
    let lower = cmd.to_lowercase();
    exclusions.iter().any(|d| lower.contains(d))
}

pub fn obfuscated_flag_re() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| {
        // ANSI-C quoting: $'...'
        // Empty quotes before dash: ''-cmd, ""-cmd
        // 3+ consecutive quotes at word start
        Regex::new(r#"(?i)(\$'[^']*'|''\s*-|""\s*-|'{3,}\w|"{3,}\w)"#).unwrap()
    })
}

pub fn command_substitution_re() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| Regex::new(r"\$\(").unwrap())
}

// --- 新增：PowerShell 深度安全检查正则（参考 powershellSecurity.ts） ---

pub fn encoded_command_re() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| {
        // -EncodedCommand / -enc / -e 作为 PowerShell/pwsh 的参数
        Regex::new(r"(?i)(powershell|pwsh)\s+.*-(EncodedCommand|enc|e)\s").unwrap()
    })
}

pub fn download_utility_re() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| {
        // certutil -urlcache, bitsadmin /transfer, Start-BitsTransfer
        Regex::new(r"(?i)(certutil\s+.*-urlcache|bitsadmin\s+/transfer|Start-BitsTransfer)")
            .unwrap()
    })
}

pub fn com_object_re() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| Regex::new(r"(?i)New-Object\s+.*-ComObject").unwrap())
}

pub fn scheduled_task_re() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| Regex::new(r"(?i)(Register-ScheduledTask|schtasks\s+/create)").unwrap())
}

pub fn runas_re() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| {
        // Start-Process -Verb RunAs (privilege escalation)
        Regex::new(r"(?i)Start-Process\s+.*-Verb\s+RunAs").unwrap()
    })
}

pub fn wmi_invoke_re() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| Regex::new(r"(?i)(Invoke-WmiMethod|Invoke-CimMethod)").unwrap())
}

pub fn unc_path_re() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| {
        // UNC path: \\server\share
        Regex::new(r#"\\\\[a-zA-Z0-9._-]+\\[a-zA-Z0-9._$-]+"#).unwrap()
    })
}

pub fn module_loading_re() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| Regex::new(r"(?i)(Import-Module|Install-Module|Update-Module)").unwrap())
}

pub fn dotnet_method_re() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| {
        // [TypeName]::Method() pattern - .NET static method calls
        Regex::new(r"\[[\w.]+\]::\w+\s*\(").unwrap()
    })
}

pub fn alias_manipulation_re() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| Regex::new(r"(?i)(Set-Alias|New-Alias)\s").unwrap())
}

pub fn new_object_typename_re() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| Regex::new(r"(?i)New-Object\s+.*-TypeName").unwrap())
}

// --- 破坏性命令警告正则 ---

pub fn destructive_remove_re() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| {
        // Remove-Item -Recurse, rm -rf, rd /s, rmdir /s
        Regex::new(r"(?i)(Remove-Item\s+.*-Recurse|rm\s+.*-rf|rd\s+/s|rmdir\s+/s)").unwrap()
    })
}

pub fn destructive_git_re() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| {
        Regex::new(r"(?i)(git\s+reset\s+--hard|git\s+push\s+.*--force|git\s+clean\s+-f|git\s+stash\s+(drop|clear))").unwrap()
    })
}

pub fn destructive_sql_re() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| Regex::new(r"(?i)(DROP\s+(TABLE|DATABASE|SCHEMA)|TRUNCATE\s+TABLE)").unwrap())
}

pub fn destructive_system_re() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| {
        Regex::new(
            r"(?i)(Stop-Computer|Restart-Computer|Clear-RecycleBin|Format-Volume|Clear-Disk)",
        )
        .unwrap()
    })
}

pub fn destructive_clear_content_re() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| Regex::new(r"(?i)Clear-Content\s+.*\*").unwrap())
}

// --- 只读命令白名单正则 ---

/// 只读 PowerShell cmdlet（不修改文件/系统状态）
pub const READONLY_CMDLETS: &[&str] = &[
    // 文件系统读取
    "get-childitem",
    "get-content",
    "get-item",
    "test-path",
    "resolve-path",
    "get-filehash",
    "get-acl",
    "get-authenticodesignature",
    // 文本搜索
    "select-string",
    // 对象检查
    "get-member",
    "compare-object",
    "measure-object",
    "join-string",
    "get-random",
    // 路径工具
    "convert-path",
    "join-path",
    "split-path",
    // 系统信息
    "get-process",
    "get-service",
    "get-computerinfo",
    "get-host",
    "get-date",
    "get-location",
    "get-psdrive",
    "get-module",
    "get-alias",
    "get-history",
    "get-culture",
    "get-timezone",
    "get-uptime",
    "get-clipboard",
    // 输出格式
    "write-output",
    "write-host",
    "format-table",
    "format-list",
    "format-wide",
    "format-custom",
    "select-object",
    "sort-object",
    "group-object",
    "where-object",
    "out-string",
    "out-host",
    "tee-object",
    // 网络信息
    "get-netadapter",
    "get-netipaddress",
    "get-netipconfiguration",
    "get-netroute",
    "get-dnsclientcache",
    "get-dnsclient",
    // 事件日志
    "get-eventlog",
    "get-winevent",
    // 数据转换（只读）
    "convertto-json",
    "convertfrom-json",
    "convertto-csv",
    "convertfrom-csv",
    "convertto-html",
    "convertto-xml",
    // 导航（不修改文件）
    "set-location",
    "push-location",
    "pop-location",
];

/// 只读外部命令的子命令
pub const READONLY_GIT_ARGS: &[&str] = &[
    "status",
    "diff",
    "log",
    "show",
    "branch",
    "tag",
    "remote",
    "describe",
    "rev-parse",
    "name-rev",
    "ls-files",
    "ls-tree",
    "cat-file",
    "count-objects",
    "shortlog",
    "blame",
    "whatchanged",
];

pub const READONLY_GH_ARGS: &[&str] = &[
    "auth",
    "browse",
    "codespace",
    "config",
    "gpg-key",
    "label",
    "release",
    "repo",
    "secret",
    "ssh-key",
    "status",
];

/// gh 的二级子命令中只读的 action
pub const READONLY_GH_PR_ISSUE_ACTIONS: &[&str] = &[
    "list", "view", "diff", "checks", "ready", "reopen", "status",
];

pub const READONLY_DOCKER_ARGS: &[&str] = &[
    "ps", "images", "logs", "inspect", "stats", "top", "port", "diff",
];

/// Windows 只读命令
pub const READONLY_WIN_COMMANDS: &[&str] = &[
    "ipconfig",
    "netstat",
    "systeminfo",
    "tasklist",
    "where.exe",
    "where",
    "hostname",
    "whoami",
    "ver",
    "arp",
    "route",
    "getmac",
    "file",
    "tree",
    "findstr",
    "find",
    "fc",
    "comp",
    "type",
    "more",
    "cls",
    "echo",
    "set",
    "dir",
    "cd",
    "vol",
    "label",
    "chkdsk",
    "driverquery",
    "schtasks",
    "tasklist",
    "reg query",
];

// --- 检查函数实现 ---

pub fn eval_re() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| Regex::new(r"(?i)(^|\s|;)(eval\s|source\s)").unwrap())
}

pub fn sudo_re() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| Regex::new(r"(?i)(^|\s)sudo\s").unwrap())
}

pub fn package_install_re() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| {
        Regex::new(r"(?i)(apt\s+install|apt-get\s+install|yum\s+install|dnf\s+install|brew\s+install|pacman\s+-S\s|pip\s+install|npm\s+install\s+-g)").unwrap()
    })
}
