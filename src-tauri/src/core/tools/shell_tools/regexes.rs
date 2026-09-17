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
        // 对照 CC（destructiveCommandWarning）删除类补缺：Remove-Item 及别名（ri/del/erase）
        // + -Recurse/-Force 组合；rm -r / -rf / --recursive；cmd 的 rd /s、rmdir /s。
        // 标志两侧用 (\s|^)…(\s|$) 锚定，防 `--preserve-root` 这类内含 "-r" 的 token 误报；
        // 纯 `Remove-Item x` 单文件删除属正常操作，不警示（与 CC 一致）。
        // 警示纯提示不拦截，宁多勿漏。
        Regex::new(
            r"(?i)(\b(Remove-Item|ri|del|erase)\b\s+(.*\s)?(-Recurse|-Force)(\s|$)|\brm\s+(.*\s)?(-rf|-r|--recursive)(\s|$)|\b(rd|rmdir)\s+/s)",
        )
        .unwrap()
    })
}

pub fn destructive_git_re() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| {
        // 对照 CC 补缺：push 的短标志 -f 与 --force 同等警示（--force-with-lease 刻意
        // 不警示——带 lease 保护，不是裸强推）；clean 补 --force，且 -f 无右边界
        // 让 -fd / -fdx 家族一并命中。reset --hard / stash drop|clear 与 CC 一致。
        Regex::new(
            r"(?i)(\bgit\s+reset\s+--hard|\bgit\s+push\s+(.*\s)?(--force(\s|$)|-f(\s|$))|\bgit\s+clean\s+(.*\s)?(-f|--force)|\bgit\s+stash\s+(drop|clear))",
        )
        .unwrap()
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
    // 清控制台缓冲区（cls/clear 的归一目标）：不改文件系统。
    // 缺它的话别名归一（PS_ALIAS_TO_CANONICAL）会把 `cls` 从 WIN 名单的免问
    // 变成弹卡——回归，故随改造②一并补入。
    "clear-host",
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
    // 网络探活（纯查询/等待，无副作用；服务启动验证高频使用）
    "get-nettcpconnection",
    "get-netudpconnection",
    "test-netconnection",
    "resolve-dnsname",
    "start-sleep",
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

/// PowerShell 别名 → canonical cmdlet（**单跳映射**，全部小写）。
///
/// 照搬 Claude Code `COMMON_ALIASES`（parser.ts:1326），全量 75 条收录，
/// 由 [`super::readonly::normalize_ps_alias`] 消费（唯一口径）：
/// Windows 侧只读判定先归一再查 `READONLY_CMDLETS` / `READONLY_WIN_COMMANDS`，
/// `rm`/`del`/`erase`/`ri` 从此不需要在每张名单里重复列别名，漏列即穿透的问题消失。
///
/// 曾因"归一目标可执行脚本块"砍掉 `foreach` / `%` / `?` / `select` 四条；
/// 脚本块一票否决（readonly.rs：命令含 `{` 即非只读，对应 CC hasScriptBlocks）
/// 落地后已收回——`dir | ? { Remove-Item $_ }` 这类形态由否决兜底，归一只管口径统一。
///
/// 单跳映射：`md` 直接映射 `new-item`，不做 `md → mkdir → new-item` 链式解析。
/// 归一后目标不在免问名单的（`kill` → `stop-process`、`iwr` → `invoke-webrequest` …）
/// 仍弹卡——归一只统一口径，不改变"能不能免问"。
pub const PS_ALIAS_TO_CANONICAL: &[(&str, &str)] = &[
    // 文件系统
    ("ls", "get-childitem"),
    ("dir", "get-childitem"),
    ("gci", "get-childitem"),
    ("cat", "get-content"),
    ("type", "get-content"),
    ("gc", "get-content"),
    ("gi", "get-item"),
    ("gp", "get-itemproperty"),
    ("ni", "new-item"),
    ("mkdir", "new-item"),
    ("md", "new-item"),
    ("ri", "remove-item"),
    ("del", "remove-item"),
    ("rd", "remove-item"),
    ("rmdir", "remove-item"),
    ("rm", "remove-item"),
    ("erase", "remove-item"),
    ("mi", "move-item"),
    ("mv", "move-item"),
    ("move", "move-item"),
    ("ci", "copy-item"),
    ("cp", "copy-item"),
    ("copy", "copy-item"),
    ("cpi", "copy-item"),
    ("si", "set-item"),
    ("rni", "rename-item"),
    ("ren", "rename-item"),
    // 位置/导航
    ("cd", "set-location"),
    ("sl", "set-location"),
    ("chdir", "set-location"),
    ("pushd", "push-location"),
    ("popd", "pop-location"),
    ("pwd", "get-location"),
    ("gl", "get-location"),
    // 进程
    ("ps", "get-process"),
    ("gps", "get-process"),
    ("kill", "stop-process"),
    ("spps", "stop-process"),
    // 执行/作业/模块/网络
    ("start", "start-process"),
    ("saps", "start-process"),
    ("sajb", "start-job"),
    ("ipmo", "import-module"),
    ("iex", "invoke-expression"),
    ("iwr", "invoke-webrequest"),
    ("irm", "invoke-restmethod"),
    ("icm", "invoke-command"),
    ("ii", "invoke-item"),
    // 远程会话
    ("nsn", "new-pssession"),
    ("etsn", "enter-pssession"),
    ("exsn", "exit-pssession"),
    ("gsn", "get-pssession"),
    ("rsn", "remove-pssession"),
    // 输出/帮助/杂项
    ("echo", "write-output"),
    ("write", "write-output"),
    ("sleep", "start-sleep"),
    ("help", "get-help"),
    ("man", "get-help"),
    ("gcm", "get-command"),
    ("gsv", "get-service"),
    ("gv", "get-variable"),
    ("sv", "set-variable"),
    ("h", "get-history"),
    ("history", "get-history"),
    ("cls", "clear-host"),
    ("clear", "clear-host"),
    // `where`/`?` 在 pwsh 里是 Where-Object 别名（裸 where 不是 where.exe）。
    // Where-Object 的 -FilterScript 可执行脚本块，但脚本块一票否决已兜底：
    // 带 `{` 的命令根本到不了名单匹配。免问名单本就含 where/where-object，无新增风险。
    ("where", "where-object"),
    ("?", "where-object"),
    // foreach 在 pwsh 里既是 ForEach-Object 别名也是语句关键字；语句形式必带 `{ }`，
    // 由脚本块否决兜底。归一目标不在免问名单 → 无脚本块时仍弹卡，仅口径统一。
    ("foreach", "foreach-object"),
    ("%", "foreach-object"),
    ("select", "select-object"),
    ("measure", "measure-object"),
    ("ft", "format-table"),
    ("fl", "format-list"),
    ("fw", "format-wide"),
    ("oh", "out-host"),
];

/// 只读 git 子命令（**正向白名单**，唯一口径）。
///
/// 由 `readonly::is_readonly_git_args` 消费（RunCommand 中 `git ...` 段的只读判定；
/// RunGitCommand 专用工具已退役，git 读写统一走 RunCommand），保证结论不会漂移。
///
/// 为什么是白名单：以前这里用的是「黑名单」（push/commit/rebase/reset/
/// revert/clean/checkout），漏掉了 `git restore .`、`git stash`、`git apply`、`git add`、
/// `git rm` —— 这些在规划模式（以及只读保护下）能直接把未提交的改动丢掉。
/// 黑名单永远补不全，所以翻成正向。
///
/// 为什么没有 `branch` / `tag` / `remote` / `config` / `worktree` / `submodule`：
/// 这几个是**读写混合**命令，带写参数就是改状态（`git branch -D`、`git tag -d`、
/// `git remote add`、`git config k v`）。它们自己就是"能改仓库"的入口，不进白名单。
pub const READONLY_GIT_ARGS: &[&str] = &[
    "status",
    "diff",
    "log",
    "show",
    "describe",
    "rev-parse",
    "rev-list",
    "name-rev",
    "ls-files",
    "ls-tree",
    "ls-remote",
    "cat-file",
    "count-objects",
    "shortlog",
    "blame",
    "annotate",
    "whatchanged",
    "grep",
    "cherry",
    "reflog",
    "show-ref",
    "symbolic-ref",
    "for-each-ref",
    "merge-base",
    "verify-commit",
    "verify-tag",
    "fsck",
    "version",
];

/// git 危险全局标志（**黑名单**，只读判定的第三道关）。
///
/// 由 `readonly::is_readonly_git_args` 消费：这些标志出现在**子命令之前**（全局域）时，
/// 整段 git 命令一律不算只读。理由分三类：
/// 1. **能执行任意代码**：`-c core.fsmonitor=命令` / `core.pager=命令` 借 config 执行；
///    `--exec-path=x` 重定向 git 自身的辅助程序查找路径。
/// 2. **篡改判定基准**：`--git-dir` / `--work-tree` / `--shallow-file` 把对象库/配置
///    换成别处的（那些仓库的 config 同样能配 fsmonitor 之类），路径校验与实际读写位置漂移。
/// 3. **解析差分**：`--attr-source` 后面的 tree-ish 是值、再往后才是子命令——
///    扁平扫描会把值误当子命令匹配白名单（Claude Code 用 GIT_TRACE 实证过
///    `git --attr-source HEAD~10 log status` 实际跑的是 status），出现即拒。
///
/// **值消费型标志必须全量收录**（-c/-C/--exec-path/--config-env/--git-dir/--work-tree/
/// --namespace/--super-prefix/--shallow-file，对照 man git 审计；名单参考 Claude Code
/// `DANGEROUS_GIT_GLOBAL_FLAGS` ∪ `GIT_GLOBAL_FLAGS_WITH_VALUES`）：任何漏网的
/// "标志+值"组合都会让值被误当子命令，制造白名单匹配错乱。
///
/// 注意：名单按**小写**前缀匹配，`-C` 与 `-c` 归一后同为 `-c`（-C 切目录同样危险）；
/// 匹配只作用于全局域（子命令之前的 token），因此不影响子命令自己的选项
/// （如 `git diff -c` 的合并 diff 输出格式）。
pub const DANGEROUS_GIT_GLOBAL_FLAGS: &[&str] = &[
    "-c",
    "--exec-path",
    "--config-env",
    "--git-dir",
    "--work-tree",
    "--namespace",
    "--super-prefix",
    "--shallow-file",
    "--attr-source",
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
/// Windows 只读命令（**整词匹配**，按命令名判定，不看参数）
///
/// 收录标准：不带参数也只会读、带参数也不会改文件系统的命令。
///
/// 刻意**不收录**的：
/// - `set`：`set FOO=bar` 会写用户环境变量（`setx` 同理，靠整词匹配拦住）；
/// - `label`：改卷标；`chkdsk`：带 `/f` 会修盘；`schtasks`：带 `/create` 会注册计划任务
///   （`/create` 另有 `check_scheduled_task` 兜底，但这里不指望它）。
///
/// 历史坑：这份名单曾经用 `name.starts_with(c)` 前缀匹配，于是名单里的 `set`
/// 把 `setx`（写用户环境变量）也判成了只读、免弹窗放行。判定必须整词相等。
pub const READONLY_WIN_COMMANDS: &[&str] = &[
    "ipconfig",
    "netstat",
    "ping",
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
    "dir",
    "cd",
    "vol",
    "driverquery",
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
