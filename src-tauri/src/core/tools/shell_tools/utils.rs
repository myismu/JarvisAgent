//! # utils.rs — Shell 辅助工具
//!
//! 处理工作区目录解析、命令出口代码语义化及终端输出格式化。
//!
//! ## Key Exports
//! - get_workspace(): 获取会话对应的工作区路径
//! - interpret_exit_code(): 转换 exit_code 为具有语义的描述
//! - ormat_shell_output(): 标准化并截断过长的 Shell 输出
//!
//! ## Dependencies
//! - Internal: crate::core::tools::framework::permission::is_within_workspace
//! - External: 	auri::Manager

use super::super::framework::permission::is_within_workspace;
use tauri::Manager;

pub async fn get_workspace(app: &tauri::AppHandle, session_id: &str) -> Option<std::path::PathBuf> {
    if let Some(manager) = app.try_state::<crate::core::state::SessionManager>() {
        let ctx = manager.get_or_create(session_id).await;
        let ws = ctx.workspace.lock().await.clone();
        return ws;
    }
    None
}

/// 提取命令的第一个 token（cmdlet/命令名）
pub fn extract_base_command(cmd: &str) -> &str {
    cmd.trim().split_whitespace().next().unwrap_or("")
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

    // 截断：保留头部，尾部截断
    if result.len() > max_chars {
        let keep = max_chars.saturating_sub(100); // 留出截断提示空间
        let truncated_chars = result.len() - keep;
        result.truncate(keep);
        result.push_str(&format!("\n\n[输出已截断，省略 {} 字符]", truncated_chars));
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
pub fn check_command_paths(cmd: &str, workspace: &std::path::Path) -> Result<(), String> {
    for (i, part) in split_command_tokens(cmd).into_iter().enumerate() {
        let trimmed = part.trim_matches('"').trim_matches('\'');

        // 跳过第一个 token（命令可执行文件本身，如 "C:\Program Files\nodejs\npm.cmd"）
        if i == 0 {
            continue;
        }

        // 跳过命令行开关和选项
        if trimmed.starts_with('-') || trimmed.starts_with('/') {
            continue;
        }

        // 跳过子命令关键字 (npm install 中的 install, git clone 中的 clone 等)
        if i == 1 {
            if matches!(
                trimmed.to_lowercase().as_str(),
                "install" | "uninstall" | "run" | "start" | "test" | "build"
                    | "dev" | "serve" | "lint" | "format" | "clean" | "init"
                    | "add" | "remove" | "update" | "upgrade" | "publish"
                    | "login" | "logout" | "whoami" | "config" | "link" | "unlink"
                    | "audit" | "outdated" | "dedupe" | "prune" | "shrinkwrap"
                    | "bundle" | "help" | "version" | "search" | "docs"
                    | "clone" | "fetch" | "pull" | "push" | "merge" | "rebase"
                    | "branch" | "checkout" | "commit" | "diff" | "log" | "status"
                    | "stash" | "tag" | "remote" | "reset" | "restore" | "revert"
                    | "switch" | "cherry-pick" | "bisect" | "blame" | "grep"
            ) {
                continue;
            }
        }

        // 跳过常见的命令关键字和 shell 内建命令
        if matches!(
            trimmed.to_lowercase().as_str(),
            "npm" | "npx" | "node" | "git" | "cargo" | "python" | "python3"
                | "mkdir" | "dir" | "ls" | "echo" | "type" | "copy" | "move"
                | "del" | "ren" | "set" | "cd" | "pwd" | "cat" | "rm"
                | "cp" | "mv" | "find" | "grep" | "sed" | "awk" | "chmod"
                | "chown" | "netstat" | "tasklist" | "Get-Process" | "Get-Service"
                | "Get-ChildItem" | "Set-Location" | "Select-Object" | "Select-String"
                | "Where-Object" | "Start-Sleep" | "Stop-Process" | "Start-Service"
                | "Write-Output" | "Write-Host" | "New-Item" | "Remove-Item"
                | "Copy-Item" | "Move-Item" | "Rename-Item" | "Test-Path"
                | "Invoke-WebRequest" | "Invoke-RestMethod" | "ConvertFrom-Json"
                | "ConvertTo-Json" | "ForEach-Object" | "Sort-Object" | "Group-Object"
                | "Format-Table" | "Format-List" | "Out-File" | "Out-String"
                | "Set-Content" | "Get-Content" | "Add-Content" | "Clear-Content"
                | "Measure-Object" | "Export-Csv" | "Import-Csv"
        ) {
            continue;
        }

        // 检查 Windows 绝对路径：X:\ 或 X:/
        if trimmed.len() >= 3
            && trimmed.as_bytes()[1] == b':'
            && (trimmed.as_bytes()[2] == b'\\' || trimmed.as_bytes()[2] == b'/')
        {
            if !is_within_workspace(trimmed, Some(workspace)) {
                return Err(format!(
                    "沙箱限制：命令包含沙箱外路径 '{}'（沙箱目录为 '{}'）",
                    trimmed,
                    workspace.display()
                ));
            }
        }
        // 检查相对路径遍历：包含 ".."
        else if trimmed.contains("..") {
            let combined = workspace.join(trimmed);
            if !is_within_workspace(&combined.to_string_lossy(), Some(workspace)) {
                return Err(format!(
                    "沙箱限制：命令包含越权相对路径 '{}'（解析后不在沙箱内）",
                    trimmed
                ));
            }
        }
    }
    Ok(())
}

/// 获取 run_shell 工具的平台适配描述
pub fn shell_tool_description() -> &'static str {
    if cfg!(target_os = "windows") {
        "执行 Windows PowerShell 命令。支持管道、脚本。可通过 run_in_background 启动长周期任务（如 npm run dev）。只读命令（dir/type/git status 等）自动放行无需确认。输出超过 50000 字符自动截断。exit code 含义自动解读（findstr:1=无匹配, robocopy:0-7=成功）。"
    } else {
        "执行 Unix bash 命令。支持管道、脚本。可通过 run_in_background 启动长周期任务（如 npm run dev）。只读命令（ls/cat/git status 等）自动放行无需确认。输出超过 50000 字符自动截断。exit code 含义自动解读（grep:1=无匹配, diff:1=不同）。"
    }
}
