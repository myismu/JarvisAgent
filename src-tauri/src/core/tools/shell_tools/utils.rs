//! # utils.rs — Shell 辅助工具
//!
//! 处理工作区目录解析、命令出口代码语义化及终端输出格式化。
//!
//! ## Key Exports
//! - get_workspace(): 获取会话对应的工作区路径
//! - interpret_exit_code(): 转换 exit_code 为具有语义的描述
//! - format_shell_output(): 标准化并截断过长的 Shell 输出
//!
//! ## Dependencies
//! - Internal: crate::core::tools::framework::permission::is_within_workspace
//! - External: tauri::Manager

use super::super::framework::permission::{has_parent_dir_segment, is_within_workspace};
use tauri::Manager;

pub async fn get_workspace(app: &tauri::AppHandle, session_id: &str) -> Option<std::path::PathBuf> {
    if let Some(manager) = app.try_state::<crate::infra::state::state::SessionManager>() {
        let ctx = manager.get_or_create(session_id).await;
        let ws = ctx.workspace.lock().await.clone();
        return ws;
    }
    None
}

/// 提取命令的第一个 token（cmdlet/命令名）
pub fn extract_base_command(cmd: &str) -> &str {
    cmd.split_whitespace().next().unwrap_or("")
}

/// 解读命令 exit code 的语义含义（grep:1=无匹配, diff:1=不同, robocopy:0-7=成功 等）
pub fn interpret_exit_code(cmd: &str, exit_code: i32) -> &'static str {
    let base = extract_base_command(cmd).to_lowercase();

    match base.as_str() {
        // grep/rg（Unix + Windows）: 0=有匹配, 1=无匹配, 2+=错误
        "grep" | "egrep" | "fgrep" | "rg" | "Select-String" => match exit_code {
            0 => "有匹配",
            1 => "无匹配",
            _ => "错误",
        },
        // findstr（Windows）: 0=找到匹配, 1=无匹配, 2+=错误
        "findstr" => match exit_code {
            0 => "找到匹配",
            1 => "无匹配",
            _ => "错误",
        },
        // find（Unix 的 find 命令）: 0=成功, 1=部分失败
        "find" => match exit_code {
            0 => "成功",
            1 => "部分路径不可访问",
            _ => "错误",
        },
        // robocopy（Windows）: 0-7=成功（位域）, 8+=错误
        "robocopy" => match exit_code {
            0..=7 => "成功",
            _ => "错误",
        },
        // diff/fc/comp: 0=相同, 1=不同, 2+=错误
        "diff" | "fc" | "comp" | "cmp" => match exit_code {
            0 => "相同",
            1 => "不同",
            _ => "错误",
        },
        // test/test-path: 0=true, 1=false
        "test" | "Test-Path" | "[" => match exit_code {
            0 => "true",
            1 => "false",
            _ => "错误",
        },
        // make: 0=编译成功
        "make" | "cmake" | "cargo" | "npm" | "pnpm" | "yarn" => match exit_code {
            0 => "成功",
            _ => "构建失败",
        },
        // 默认
        _ => match exit_code {
            0 => "成功",
            _ => "失败",
        },
    }
}

/// 格式化 shell 输出，包含 exit code 语义和截断
pub fn format_shell_output(
    cmd: &str,
    stdout: &str,
    stderr: &str,
    exit_code: i32,
    max_chars: usize,
) -> String {
    let semantics = interpret_exit_code(cmd, exit_code);
    let mut result = if exit_code == 0 {
        format!("[exit code: 0 ({})]\n", semantics)
    } else {
        format!("[exit code: {} ({})]\n", exit_code, semantics)
    };

    let stdout_trimmed = stdout.trim_end();
    let stderr_trimmed = stderr.trim_end();

    if !stdout_trimmed.is_empty() {
        result.push_str(&format!("STDOUT:\n{}", stdout_trimmed));
    }
    if !stderr_trimmed.is_empty() {
        if !stdout_trimmed.is_empty() {
            result.push('\n');
        }
        result.push_str(&format!("STDERR:\n{}", stderr_trimmed));
    }

    // 截断：头尾各留一段，只留头部会把尾部（错误摘要/失败原因）丢掉。
    // 计数与切分都按「字符」而非字节，切点必须落在 UTF-8 字符边界上。
    let total_chars = result.chars().count();
    if total_chars > max_chars {
        let keep = max_chars.saturating_sub(120); // 留出截断提示空间
        let head = keep * 2 / 3;
        let tail = keep - head;

        // 头部：第 head 个字符的起始字节位（回溯到 <= head 的最近字符边界）
        let head_end = result
            .char_indices()
            .nth(head)
            .map(|(i, _)| i)
            .unwrap_or(result.len());
        // 尾部：从末尾回溯 tail 个字符，取 >= len-tail 的最近字符边界
        let tail_start = if tail == 0 {
            result.len()
        } else {
            result
                .char_indices()
                .rev()
                .nth(tail - 1)
                .map(|(i, _)| i)
                .unwrap_or(0)
                .max(head_end)
        };

        let omitted = total_chars.saturating_sub(head + tail);
        let mut truncated = String::with_capacity(head_end + (result.len() - tail_start) + 64);
        truncated.push_str(&result[..head_end]);
        truncated.push_str(&format!("\n\n[输出已截断，中间省略 {} 字符]\n\n", omitted));
        truncated.push_str(&result[tail_start..]);
        return truncated;
    }

    result
}

/// 将命令字符串按空格分割为 token，保留引号包裹的完整路径
fn split_command_tokens(cmd: &str) -> Vec<String> {
    let mut tokens = Vec::new();
    let mut current = String::new();
    let mut in_quote = false;
    let mut quote_char = '"';

    for ch in cmd.chars() {
        match ch {
            '"' | '\'' => {
                if in_quote && ch == quote_char {
                    // 结束引号：闭合当前 token
                    current.push(ch);
                    tokens.push(current.clone());
                    current.clear();
                    in_quote = false;
                } else if !in_quote {
                    // 开始引号
                    if !current.trim().is_empty() {
                        tokens.push(current.clone());
                    }
                    current.clear();
                    current.push(ch);
                    in_quote = true;
                    quote_char = ch;
                } else {
                    // 引号内遇到另一种引号，当作普通字符
                    current.push(ch);
                }
            }
            ' ' | '\t' => {
                if in_quote {
                    current.push(ch);
                } else if !current.trim().is_empty() {
                    tokens.push(current.clone());
                    current.clear();
                } else {
                    current.clear();
                }
            }
            _ => {
                current.push(ch);
            }
        }
    }
    if !current.trim().is_empty() {
        tokens.push(current);
    }
    tokens
}

/// 检查命令中的路径引用是否在沙箱内（检查绝对路径和 `..` 相对路径）
/// 剥掉重定向前缀: >  >>  2>  1>  *>  n>  n>> 等
fn strip_redirection_prefix(token: &str) -> &str {
    let bytes = token.as_bytes();
    let len = bytes.len();
    if len == 0 {
        return token;
    }
    // >>  2>>  1>>
    if len >= 3 && (bytes[0] == b'2' || bytes[0] == b'1') && bytes[1] == b'>' && bytes[2] == b'>' {
        return token[3..].trim_start();
    }
    // n> n>> (stdout/stderr numbered redirect)
    if len >= 2 && bytes[0].is_ascii_digit() && bytes[1] == b'>' {
        let start = if len > 2 && bytes[2] == b'>' { 3 } else { 2 };
        return token[start..].trim_start();
    }
    // >>  >  *>  *>>
    if len >= 2 && bytes[0] == b'>' && bytes[1] == b'>' {
        return token[2..].trim_start();
    }
    if len >= 2 && bytes[0] == b'*' && bytes[1] == b'>' {
        let start = if len > 2 && bytes[2] == b'>' { 3 } else { 2 };
        return token[start..].trim_start();
    }
    if bytes[0] == b'>' {
        return token[1..].trim_start();
    }
    token
}

/// `..` 只有作为独立路径段才算遍历，`30..42` / `a..b` 不算。
/// 反斜杠统一按分隔符处理，避免 `..\secret` 这类 Windows 写法漏判。
fn has_parent_segment(token: &str) -> bool {
    // 与文件工具的路径闸门共用同一实现（framework::permission），
    // 避免「命令里拦、文件工具里不拦」这类口径漂移
    has_parent_dir_segment(token)
}

/// POSIX 参数 token 归一后的形态
enum NormalizedToken {
    /// 无害设备路径（/dev/null 等），不参与越界判定
    Skip,
    /// 换算成了 Windows 口径的绝对路径
    Path(String),
    /// 保持原样（相对路径、Windows 路径、未识别的 POSIX 绝对路径）
    Unchanged,
}

/// 把 Git Bash 的 MSYS 路径换算成 Windows 路径：`/e/demo` → `E:\demo`。
///
/// 只在 Git Bash 方言下调用——Linux 主机上的 `/c/Users` 是真实目录，
/// 不能按盘符误解。
fn msys_path_to_windows(token: &str) -> Option<String> {
    let rest = if let Some(rest) = token.strip_prefix("/cygdrive/") {
        rest
    } else {
        let bytes = token.as_bytes();
        if bytes.len() >= 3 && bytes[0] == b'/' && bytes[1].is_ascii_alphabetic() && bytes[2] == b'/' {
            // 跳过开头的 '/'，得到 `e/foo`
            &token[1..]
        } else {
            return None;
        }
    };
    let bytes = rest.as_bytes();
    if bytes.is_empty() || !bytes[0].is_ascii_alphabetic() {
        return None;
    }
    let drive = (bytes[0] as char).to_ascii_uppercase();
    let tail = &rest[1..];
    if tail.is_empty() {
        return Some(format!("{}:\\", drive));
    }
    // 只认 `/e/...` 形态；`/etc` 这类不是盘符路径
    let tail = tail.strip_prefix('/')?;
    Some(format!("{}:\\{}", drive, tail.replace('/', "\\")))
}

/// 展开 `~/...` 为用户主目录（Windows 用 USERPROFILE，Unix 用 HOME）。
fn expand_home(token: &str) -> Option<String> {
    if token != "~" && !token.starts_with("~/") && !token.starts_with("~\\") {
        return None;
    }
    let home = if cfg!(windows) {
        std::env::var("USERPROFILE").ok()?
    } else {
        std::env::var("HOME").ok()?
    };
    let rest = token.trim_start_matches('~').trim_start_matches(['/', '\\']);
    if rest.is_empty() {
        Some(home)
    } else {
        Some(format!("{}\\{}", home.trim_end_matches(['/', '\\']), rest.replace('/', "\\")))
    }
}

/// Windows 程序的 `/F`、`/PID` 形式开关。
///
/// 判定刻意收窄：只有"单段、纯字母、要么单字母要么不超过 5 字母且全大写"才算开关。
/// `/e/demo`、`/usr/bin`、`/tmp/x` 都会落到路径分支，不会被误当成开关放过。
/// 已知代价：单字母小写开关与盘符根目录（`/c`）无法区分，这里按开关处理——
/// 裸盘符根作为命令参数没有实际用途，而 `cmd /c` 是高频写法。
fn is_windows_style_switch(token: &str) -> bool {
    let Some(rest) = token.strip_prefix('/') else {
        return false;
    };
    if rest.is_empty() || rest.contains('/') {
        return false;
    }
    let all_alpha = rest.chars().all(|c| c.is_ascii_alphabetic());
    all_alpha && (rest.len() == 1 || (rest.len() <= 5 && rest.chars().all(|c| c.is_ascii_uppercase())))
}

/// POSIX token 归一：设备路径跳过、Git Bash 路径换算、`~` 展开。
fn normalize_posix_token(token: &str, git_bash: bool) -> NormalizedToken {
    if matches!(
        token,
        "/dev/null" | "/dev/stdout" | "/dev/stderr" | "/dev/tty" | "/dev/zero"
    ) {
        return NormalizedToken::Skip;
    }
    if git_bash {
        if let Some(path) = msys_path_to_windows(token) {
            return NormalizedToken::Path(path);
        }
    }
    if let Some(path) = expand_home(token) {
        return NormalizedToken::Path(path);
    }
    NormalizedToken::Unchanged
}

/// 是否为 Windows 盘符绝对路径（`X:\` 或 `X:/`）
fn is_windows_absolute(token: &str) -> bool {
    token.len() >= 3
        && token.as_bytes()[1] == b':'
        && (token.as_bytes()[2] == b'\\' || token.as_bytes()[2] == b'/')
}

/// `--opt=value` 的值如果是路径，收集落在沙箱外的路径（如 `npm --prefix /e/out`）。
fn collect_option_value_outside_path(
    value: &str,
    git_bash: bool,
    workspace: &std::path::Path,
    outside: &mut Vec<String>,
) {
    let normalized = match normalize_posix_token(value, git_bash) {
        NormalizedToken::Skip => return,
        NormalizedToken::Path(path) => path,
        NormalizedToken::Unchanged => value.to_string(),
    };
    if (is_windows_absolute(&normalized) || normalized.starts_with('/'))
        && !is_within_workspace(&normalized, Some(workspace))
    {
        if !outside.contains(&normalized) {
            outside.push(normalized);
        }
        return;
    }
    if has_parent_segment(&normalized) {
        let combined = workspace.join(&normalized);
        let combined_str = combined.to_string_lossy().into_owned();
        if !is_within_workspace(&combined_str, Some(workspace)) && !outside.contains(&combined_str) {
            outside.push(combined_str);
        }
    }
}

/// 收集命令中所有落在沙箱外的路径；遇到无法静态解析的环境变量绕过写法直接返回 `Err`。
///
/// 唯一口径：外层策略判定（`policy_guard`）与执行层（`execution`）共用，
/// 避免“只读命令放行项目外、非只读命令拦截项目外”两侧口径漂移。
pub fn collect_outside_command_paths(
    cmd: &str,
    workspace: &std::path::Path,
) -> Result<Vec<String>, String> {
    collect_outside_command_paths_for(&crate::infra::shell_dialect::current(), cmd, workspace)
}

/// 按指定方言收集命令中落在沙箱外的路径；遇到环境变量构造路径等绕过手法直接返回 `Err`。
pub fn collect_outside_command_paths_for(
    dialect: &crate::infra::shell_dialect::ShellDialect,
    cmd: &str,
    workspace: &std::path::Path,
) -> Result<Vec<String>, String> {
    let mut outside: Vec<String> = Vec::new();
    let git_bash = dialect.is_git_bash();
    for (i, part) in split_command_tokens(cmd).into_iter().enumerate() {
        let mut trimmed = part.trim_matches('"').trim_matches('\'').to_string();
        // 剥掉重定向前缀 (>  >>  2>  1>  *>  2>> 等)，避免路径被 ">C:\..." 格式隐藏
        trimmed = strip_redirection_prefix(&trimmed).to_string();

        // 跳过第一个 token（命令可执行文件本身，如 "C:\Program Files\nodejs\npm.cmd"）
        if i == 0 {
            continue;
        }

        // 选项与开关：两种方言的形态不同
        if dialect.is_bash() {
            // `--opt=value` 的值可能是路径（如 npm 的 --prefix），不能整条跳过
            if trimmed.starts_with('-') {
                if let Some((_, value)) = trimmed.split_once('=') {
                    collect_option_value_outside_path(value, git_bash, workspace, &mut outside);
                }
                continue;
            }
            // Windows 程序的 `/F`、`/PID` 形式开关
            if is_windows_style_switch(&trimmed) {
                continue;
            }
        } else if trimmed.starts_with('-') || trimmed.starts_with('/') {
            // PowerShell 方言维持原口径：`/` 开头一律视为开关
            continue;
        }

        // 跳过子命令关键字 (npm install 中的 install, git clone 中的 clone 等)
        if i == 1
            && matches!(
                trimmed.to_lowercase().as_str(),
                "install"
                    | "uninstall"
                    | "run"
                    | "start"
                    | "test"
                    | "build"
                    | "dev"
                    | "serve"
                    | "lint"
                    | "format"
                    | "clean"
                    | "init"
                    | "add"
                    | "remove"
                    | "update"
                    | "upgrade"
                    | "publish"
                    | "login"
                    | "logout"
                    | "whoami"
                    | "config"
                    | "link"
                    | "unlink"
                    | "audit"
                    | "outdated"
                    | "dedupe"
                    | "prune"
                    | "shrinkwrap"
                    | "bundle"
                    | "help"
                    | "version"
                    | "search"
                    | "docs"
                    | "clone"
                    | "fetch"
                    | "pull"
                    | "push"
                    | "merge"
                    | "rebase"
                    | "branch"
                    | "checkout"
                    | "commit"
                    | "diff"
                    | "log"
                    | "status"
                    | "stash"
                    | "tag"
                    | "remote"
                    | "reset"
                    | "restore"
                    | "revert"
                    | "switch"
                    | "cherry-pick"
                    | "bisect"
                    | "blame"
                    | "grep"
            ) {
                continue;
            }

        // 跳过常见的命令关键字和 shell 内建命令
        if matches!(
            trimmed.to_lowercase().as_str(),
            "npm"
                | "npx"
                | "node"
                | "git"
                | "cargo"
                | "python"
                | "python3"
                | "mkdir"
                | "dir"
                | "ls"
                | "echo"
                | "type"
                | "copy"
                | "move"
                | "del"
                | "ren"
                | "set"
                | "cd"
                | "pwd"
                | "cat"
                | "rm"
                | "cp"
                | "mv"
                | "find"
                | "grep"
                | "sed"
                | "awk"
                | "chmod"
                | "chown"
                | "netstat"
                | "tasklist"
                | "Get-Process"
                | "Get-Service"
                | "Get-ChildItem"
                | "Set-Location"
                | "Select-Object"
                | "Select-String"
                | "Where-Object"
                | "Start-Sleep"
                | "Stop-Process"
                | "Start-Service"
                | "Write-Output"
                | "Write-Host"
                | "New-Item"
                | "Remove-Item"
                | "Copy-Item"
                | "Move-Item"
                | "Rename-Item"
                | "Test-Path"
                | "Invoke-WebRequest"
                | "Invoke-RestMethod"
                | "ConvertFrom-Json"
                | "ConvertTo-Json"
                | "ForEach-Object"
                | "Sort-Object"
                | "Group-Object"
                | "Format-Table"
                | "Format-List"
                | "Out-File"
                | "Out-String"
                | "Set-Content"
                | "Get-Content"
                | "Add-Content"
                | "Clear-Content"
                | "Measure-Object"
                | "Export-Csv"
                | "Import-Csv"
        ) {
            continue;
        }

        // POSIX 方言：先换算 MSYS 路径（/e/... → E:\...）与 ~ 展开，
        // 否则所有 Git Bash 绝对路径都会被当成"非绝对路径"直接放行。
        let normalized = if dialect.is_bash() {
            match normalize_posix_token(&trimmed, git_bash) {
                NormalizedToken::Skip => continue,
                NormalizedToken::Path(path) => path,
                NormalizedToken::Unchanged => trimmed.clone(),
            }
        } else {
            trimmed.clone()
        };

        // POSIX 方言禁止用环境变量构造路径（$HOME/x 这类写法绕过静态校验）
        if dialect.is_bash() && normalized.starts_with('$') && normalized.contains('/') {
            return Err(format!(
                "沙箱限制：命令包含环境变量引用 '{}'，沙箱内禁止使用环境变量构造路径。\
                 请使用沙箱内的相对路径或绝对路径。",
                normalized
            ));
        }

        // 检查绝对路径：Windows 的 X:\ / X:/，以及 POSIX 的 / 开头路径
        if is_windows_absolute(&normalized) || normalized.starts_with('/') {
            if !is_within_workspace(&normalized, Some(workspace)) && !outside.contains(&normalized) {
                outside.push(normalized);
            }
        }
        // 检查相对路径遍历：`..` 必须是独立路径段（30..42 / a..b 不算遍历）
        else if has_parent_segment(&normalized) {
            let combined = workspace.join(&normalized);
            let combined_str = combined.to_string_lossy().into_owned();
            if !is_within_workspace(&combined_str, Some(workspace))
                && !outside.contains(&combined_str)
            {
                outside.push(combined_str);
            }
        }
        // 沙箱内禁止使用环境变量构造路径（$env:VAR / %VAR% / PowerShell 未展开的 ~）
        // 修复：原先误嵌套在 `has_parent_segment` 内部，导致不含 `..` 的 `$env:USERPROFILE\...` 漏检。
        else if is_env_var_reference(&trimmed) {
            return Err(format!(
                "沙箱限制：命令包含环境变量引用 '{}'，沙箱内禁止使用环境变量构造路径。请使用明确的绝对路径或沙箱内相对路径。",
                trimmed
            ));
        }
    }
    Ok(outside)
}

pub fn check_command_paths(cmd: &str, workspace: &std::path::Path) -> Result<(), String> {
    check_command_paths_for(&crate::infra::shell_dialect::current(), cmd, workspace)
}

/// 按指定方言校验命令里的路径是否都在沙箱内（供非只读命令硬拦截与既有单测调用）。
pub fn check_command_paths_for(
    dialect: &crate::infra::shell_dialect::ShellDialect,
    cmd: &str,
    workspace: &std::path::Path,
) -> Result<(), String> {
    let outside = collect_outside_command_paths_for(dialect, cmd, workspace)?;
    if let Some(first) = outside.first() {
        return Err(format!(
            "沙箱限制：命令包含沙箱外路径 '{}'（沙箱目录为 '{}'）",
            first,
            workspace.display()
        ));
    }
    Ok(())
}

/// 检测 token 是否包含环境变量引用（$env:VAR / %VAR% / ~ home）
fn is_env_var_reference(token: &str) -> bool {
    if token.contains("$env:") || token.contains("${env:") {
        return true;
    }
    if token.starts_with('%') && token.contains('%') && token.len() > 2 {
        return true;
    }
    // Unix/PowerShell home 目录展开: ~/Desktop, ~\Documents
    if token.starts_with('~') {
        return true;
    }
    false
}

/// 判断退出码是否代表真正的错误（基于命令语义）
///
/// `interpret_exit_code` 已知的特殊语义不算错误：
/// - grep:1=无匹配, diff:1=不同, robocopy:1-7=成功, Test-Path:1=false 等
/// - 只有语义为 "错误"/"失败"/"构建失败" 的才算真正的 error
pub fn is_exit_code_error(cmd: &str, exit_code: i32) -> bool {
    if exit_code == 0 {
        return false;
    }
    let semantics = interpret_exit_code(cmd, exit_code);
    // 只有语义明确是"错误"/"失败"/"构建失败"的才算 error
    // "无匹配"/"不同"/"成功"/"true"/"false"/"有匹配"/"找到匹配" 等都不算
    semantics == "错误" || semantics == "失败" || semantics == "构建失败"
}

/// `dir` 参数的描述（RunCommand 前后台模式共用）。
///
/// 为什么共用而不是各写一份：两个工具的 `dir` 是同一套语义与取值规则
/// （尤其"要指向 package.json 所在子目录"那条），各写一份必然漂移 ——
/// 而漂移之后模型会拿到两套说法，正是这次下沉重构要消掉的东西。
pub fn dir_param_description() -> &'static str {
    "命令执行的工作目录（绝对路径，或相对工作区根目录的路径）。沙箱会话下必须落在工作区内；省略则在（沙箱）根目录执行。npm install / npm run 类命令，dir 必须指向 package.json 所在的子目录，不要用沙箱根目录。"
}

/// 获取 run_shell 工具的平台适配描述
pub fn shell_tool_description() -> &'static str {
    // 末尾那句"不支持文件写入"从提示词搬来（2026-09-21）：执行层本来就会拦
    // （见 execution.rs 的 is_file_mutation_command），但事前告知能省一次失败调用。
    // 搬来后提示词里 base_p0_write.md 与 subagent.md 的两处同款说明即删除。
    // 文案必须与**实际方言**一致：Windows 上开了 Git Bash 还告诉模型"PowerShell"，
    // 模型会写 $env:VAR / Get-ChildItem，整轮命令全部语法报错。
    match crate::infra::shell_dialect::current() {
        crate::infra::shell_dialect::ShellDialect::PowerShell => {
            "执行 Windows PowerShell 命令。支持管道、脚本。可通过 run_in_background 启动长周期任务（如 npm run dev）。只读命令（dir/type/git status 等）自动放行无需确认。输出超过 50000 字符自动截断。exit code 含义自动解读（findstr:1=无匹配, robocopy:0-7=成功）。不支持文件写入/删除/重命名：用 [System.IO.File] 方法、Set-Content/Out-File/Add-Content/New-Item 或 shell 重定向 > 落盘都会被拦下，请改用 WriteFile/EditFile/DeleteFile/RenameFile 等专用工具（只有它们有沙箱边界检查、改动快照与回滚）。"
        }
        crate::infra::shell_dialect::ShellDialect::Bash { .. } if cfg!(windows) => {
            "执行 Git Bash（POSIX/bash 语义，运行在 Windows 上）命令。支持管道、&&、||、脚本。可通过 run_in_background 启动长周期任务（如 npm run dev）。只读命令（ls/cat/grep/git status 等）自动放行无需确认。输出超过 50000 字符自动截断。exit code 含义自动解读（grep:1=无匹配, diff:1=不同）。路径用 bash 口径：E:\\xxx 写作 /e/xxx；沙箱内优先用相对路径。不要写 PowerShell 语法（$env:VAR / Get-ChildItem / Remove-Item 等）。不支持文件写入/删除/重命名（echo >、tee、sed -i 等都会被拦下），请改用 WriteFile/EditFile/DeleteFile/RenameFile 等专用工具（只有它们有沙箱边界检查、改动快照与回滚）。"
        }
        crate::infra::shell_dialect::ShellDialect::Bash { .. } => {
            "执行 Unix bash 命令。支持管道、脚本。可通过 run_in_background 启动长周期任务（如 npm run dev）。只读命令（ls/cat/git status 等）自动放行无需确认。输出超过 50000 字符自动截断。exit code 含义自动解读（grep:1=无匹配, diff:1=不同）。不支持文件写入/删除/重命名（echo >、tee、sed -i 等都会被拦下），请改用 WriteFile/EditFile/DeleteFile/RenameFile 等专用工具（只有它们有沙箱边界检查、改动快照与回滚）。"
        }
    }
}

/// RunCommand 的 `command` 参数说明（须与方言一致）
pub fn shell_command_param_description() -> &'static str {
    match crate::infra::shell_dialect::current() {
        crate::infra::shell_dialect::ShellDialect::PowerShell => "要执行的 PowerShell 命令",
        crate::infra::shell_dialect::ShellDialect::Bash { .. } if cfg!(windows) => {
            "要执行的 Git Bash 命令（POSIX 语法）"
        }
        crate::infra::shell_dialect::ShellDialect::Bash { .. } => "要执行的 bash 命令",
    }
}

/// RunCommand 的工具简述（工具列表里那行）
pub fn shell_tool_short_description() -> &'static str {
    match crate::infra::shell_dialect::current() {
        crate::infra::shell_dialect::ShellDialect::PowerShell => {
            "执行 Windows PowerShell 命令（统一入口）"
        }
        crate::infra::shell_dialect::ShellDialect::Bash { .. } if cfg!(windows) => {
            "执行 Git Bash 命令（统一入口）"
        }
        crate::infra::shell_dialect::ShellDialect::Bash { .. } => "执行 Unix bash 命令（统一入口）",
    }
}

#[cfg(test)]
mod path_and_truncation_tests {
    use super::{check_command_paths, check_command_paths_for, format_shell_output, msys_path_to_windows};
    use crate::infra::shell_dialect::ShellDialect;
    use std::path::{Path, PathBuf};

    fn ws() -> &'static Path {
        Path::new("C:\\workspace")
    }

    fn git_bash() -> ShellDialect {
        ShellDialect::Bash {
            program: PathBuf::from("bash"),
            git_bash: true,
        }
    }

    #[test]
    fn msys_paths_map_to_drive_letters() {
        assert_eq!(msys_path_to_windows("/e/demo/x"), Some("E:\\demo\\x".to_string()));
        assert_eq!(
            msys_path_to_windows("/c/Users/沐/.ssh"),
            Some("C:\\Users\\沐\\.ssh".to_string())
        );
        assert_eq!(
            msys_path_to_windows("/cygdrive/d/work"),
            Some("D:\\work".to_string())
        );
        // 非盘符形态不换算，避免把 /etc、/usr 误认成 E:\tc、U:\sr
        assert_eq!(msys_path_to_windows("/etc/passwd"), None);
        assert_eq!(msys_path_to_windows("./relative"), None);
    }

    /// Git Bash 方言下，MSYS 绝对路径必须参与沙箱校验；
    /// 以前"以 / 开头一律当开关"会让 `/e/other` 直接穿过边界。
    #[test]
    fn git_bash_paths_are_checked_against_workspace() {
        let dialect = git_bash();
        let ws = Path::new("E:\\demo\\JarvisAgent");
        assert!(check_command_paths_for(&dialect, "cat /e/demo/JarvisAgent/src/main.rs", ws).is_ok());
        assert!(check_command_paths_for(&dialect, "cat /e/other/secret.txt", ws).is_err());
        // 未换算的 POSIX 绝对路径同样越界
        assert!(check_command_paths_for(&dialect, "cat /tmp/secret", ws).is_err());
        // `--opt=value` 里的路径也要过边界
        assert!(check_command_paths_for(&dialect, "npm --prefix=/e/other install", ws).is_err());
    }

    /// Windows 程序的 `/F`、`/PID` 开关不能被当成路径；设备重定向无害。
    #[test]
    fn windows_switches_and_dev_null_are_not_paths() {
        let dialect = git_bash();
        let ws = Path::new("E:\\demo\\JarvisAgent");
        assert!(check_command_paths_for(&dialect, "taskkill /F /IM node.exe", ws).is_ok());
        assert!(check_command_paths_for(&dialect, "npm run dev 2>/dev/null", ws).is_ok());
    }

    /// `30..42` 这类数字区间/普通单词里的 `..` 不是路径遍历，不该被判越权
    #[test]
    fn dot_dot_inside_numbers_or_words_is_not_traversal() {
        for cmd in ["echo 30..42", "echo 1..100", "echo a..b"] {
            assert!(
                check_command_paths(cmd, ws()).is_ok(),
                "不该被判越权路径：{}",
                cmd
            );
        }
    }

    /// 真正的父目录段仍必须拦下（反斜杠/正斜杠/单独 `..` 三种写法）
    #[test]
    fn real_parent_segments_are_still_blocked() {
        for cmd in [r"type ..\secret", "type a/../../b", "type .."] {
            assert!(
                check_command_paths(cmd, ws()).is_err(),
                "应拦下越权路径：{}",
                cmd
            );
        }
    }

    /// 回归测试：PowerShell 下不含 `..` 的环境变量/家目录引用必须被拦截
    /// （修复原先 `else if is_env_var_reference` 误嵌套在 `has_parent_segment` 内部的 BUG）
    #[test]
    fn powershell_env_var_paths_without_parent_segment_are_blocked() {
        for cmd in [
            r"Get-Content $env:USERPROFILE\Desktop\secret.txt",
            r"type %USERPROFILE%\Desktop\secret.txt",
            r"Get-Content ~\Desktop\secret.txt",
        ] {
            assert!(
                check_command_paths_for(&ShellDialect::PowerShell, cmd, ws()).is_err(),
                "应拦下环境变量构造的路径：{}",
                cmd
            );
        }
    }

    /// 截断后头部与尾部都要保留（尾部常是错误摘要），中间给省略提示
    #[test]
    fn truncation_keeps_head_and_tail() {
        let stdout = format!("HEAD_MARK{}{}TAIL_MARK", "a".repeat(900), "b".repeat(900));
        assert!(stdout.len() > 200);

        let out = format_shell_output("echo", &stdout, "", 0, 200);
        assert!(out.starts_with("[exit code: 0 (成功)]"), "exit code 语义行应在最前");
        assert!(out.contains("HEAD_MARK"), "头部应保留");
        assert!(out.contains("TAIL_MARK"), "尾部应保留：{}", out);
        assert!(out.contains("中间省略"), "应有中间省略提示：{}", out);
    }

    /// 多字节字符（中文/emoji）截断不得落在字符中间导致 panic
    #[test]
    fn truncation_is_utf8_safe() {
        let stdout = format!("开始{}结尾🚀", "中文测试😀".repeat(400));
        let out = format_shell_output("echo", &stdout, "错误😀失败", 1, 200);

        assert!(out.starts_with("[exit code: 1"));
        assert!(out.contains("开始"));
        assert!(out.contains("🚀"));
        assert!(out.contains("中间省略"));
    }

    /// 短输出（<= max_chars）保持原有格式，不加省略提示
    #[test]
    fn short_output_is_untouched() {
        let out = format_shell_output("echo", "hi", "", 0, 200);
        assert_eq!(out, "[exit code: 0 (成功)]\nSTDOUT:\nhi");
    }
}
