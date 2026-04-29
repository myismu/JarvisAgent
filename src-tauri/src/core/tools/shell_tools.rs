//! # shell_tools.rs — Shell 命令执行工具模块
//!
//! 提供 run_shell（统一入口，支持同步/后台模式）、git_command（只读 git）、
//! background_run（后台执行）、check_background（状态查询）等工具。
//!
//! ## 关键导出
//! - `run_shell()`: 统一 Shell 执行入口，按平台自动选择 PowerShell/bash
//! - `git_command()`: 只读 git 操作（status/diff/log 等）
//! - `background_run()`: 后台执行长时间命令
//! - `check_background()`: 检查后台任务状态
//!
//! ## 依赖
//! - Internal: `permission::request_permission`, `shell_security::{check_command_safety, is_readonly_command}`
//! - External: `serde_json`, `tauri`, `tokio`
//!
//! ## 约束
//! - 只读命令自动放行无需确认
//! - 高危命令（del/rm/format 等）需用户确认
//! - 输出超过 50000 字符自动截断
//! - exit code 含义自动解读（grep:1=无匹配, diff:1=不同 等）

use serde_json::json;
use std::process::Stdio;
use std::time::Duration;
use tauri::Manager;
use tokio::io::{AsyncBufReadExt, BufReader};
use super::permission::{request_permission, is_within_workspace};
use super::shell_security::{check_command_safety, is_readonly_command, get_destructive_warning, SafetyResult};
use crate::core::tools::registry::ToolDef;

/// Shell 输出最大字符数（截断阈值）
const MAX_SHELL_OUTPUT_LEN: usize = 50000;

/// 默认超时秒数
const DEFAULT_TIMEOUT_SECS: u64 = 120;

async fn get_workspace(app: &tauri::AppHandle, session_id: &str) -> Option<std::path::PathBuf> {
    if let Some(manager) = app.try_state::<crate::core::state::SessionManager>() {
        let ctx = manager.get_or_create(session_id).await;
        let ws = ctx.workspace.lock().await.clone();
        return ws;
    }
    None
}

/// 提取命令的第一个 token（cmdlet/命令名）
fn extract_base_command(cmd: &str) -> &str {
    cmd.trim().split_whitespace().next().unwrap_or("")
}

/// 解读命令 exit code 的语义含义（grep:1=无匹配, diff:1=不同, robocopy:0-7=成功 等）
fn interpret_exit_code(cmd: &str, exit_code: i32) -> &'static str {
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
fn format_shell_output(cmd: &str, stdout: &str, stderr: &str, exit_code: i32, max_chars: usize) -> String {
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

/// 检查命令中的路径引用是否在沙箱内（检查绝对路径和 `..` 相对路径）
fn check_command_paths(cmd: &str, workspace: &std::path::Path) -> Result<(), String> {
    for part in cmd.split_whitespace() {
        let trimmed = part.trim_matches('"').trim_matches('\'');

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

/// 异步执行 shell 命令（按平台选择 PowerShell 或 bash），返回 (stdout, stderr, exit_code)
async fn run_shell_async(cmd: &str, exec_dir: &std::path::Path) -> (String, String, i32) {
    let (shell, args) = if cfg!(target_os = "windows") {
        let ps_cmd = format!(
            "[Console]::OutputEncoding = [System.Text.Encoding]::UTF8; {}",
            cmd
        );
        ("powershell".to_string(), vec!["-NoProfile".to_string(), "-NonInteractive".to_string(), "-Command".to_string(), ps_cmd])
    } else {
        ("bash".to_string(), vec!["-c".to_string(), cmd.to_string()])
    };

    match tokio::process::Command::new(&shell)
        .current_dir(exec_dir)
        .args(&args)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
    {
        Ok(mut child) => {
            let stdout = child.stdout.take();
            let stderr = child.stderr.take();

            let stdout_handle = tokio::spawn(async move {
                if let Some(stdout) = stdout {
                    let reader = BufReader::new(stdout);
                    let mut lines = reader.lines();
                    let mut output = String::new();
                    while let Ok(Some(line)) = lines.next_line().await {
                        output.push_str(&line);
                        output.push('\n');
                    }
                    output
                } else {
                    String::new()
                }
            });

            let stderr_handle = tokio::spawn(async move {
                if let Some(stderr) = stderr {
                    let reader = BufReader::new(stderr);
                    let mut lines = reader.lines();
                    let mut output = String::new();
                    while let Ok(Some(line)) = lines.next_line().await {
                        output.push_str(&line);
                        output.push('\n');
                    }
                    output
                } else {
                    String::new()
                }
            });

            let stdout_out = stdout_handle.await.unwrap_or_default();
            let stderr_out = stderr_handle.await.unwrap_or_default();

            let exit_code = child.wait().await
                .map(|s| s.code().unwrap_or(-1))
                .unwrap_or(-1);

            (stdout_out, stderr_out, exit_code)
        }
        Err(e) => (String::new(), format!("执行失败: {}", e), -1),
    }
}

/// 内部函数：后台执行命令（委托 BackgroundManager）
async fn background_run_internal(
    app: &tauri::AppHandle,
    cmd: &str,
    workspace: &Option<std::path::PathBuf>,
) -> String {
    let exec_dir = workspace.as_ref().map(|p| p.to_string_lossy().into_owned());
    crate::core::infra::background::BackgroundManager::run(app.clone(), cmd.to_string(), exec_dir).await
}

/// 执行 shell 命令（统一入口，支持同步/后台模式，按平台自动选择 PowerShell 或 bash）
pub async fn run_shell(
    app: &tauri::AppHandle,
    input: &serde_json::Value,
    session_id: &str,
) -> String {
    let cmd = input["command"].as_str().unwrap_or("");
    let description = input["description"].as_str().unwrap_or("");
    let timeout_secs = input["timeout"].as_u64().unwrap_or(DEFAULT_TIMEOUT_SECS).clamp(5, 600);
    let run_in_bg = input["run_in_background"].as_bool().unwrap_or(false);

    // --- 1. 安全检查 ---
    let mut warnings: Vec<String> = Vec::new();
    match check_command_safety(cmd) {
        SafetyResult::Block(msg) => {
            return format!("安全拦截：{}", msg);
        }
        SafetyResult::Warn(msg) => {
            warnings.push(msg);
        }
        SafetyResult::Safe => {}
    }

    let ws = get_workspace(app, session_id).await;

    // --- 2. 沙箱路径检查 ---
    if let Some(ref workspace) = ws {
        if let Err(e) = check_command_paths(cmd, workspace) {
            return e;
        }

        let lower_cmd = cmd.to_lowercase();
        let dir_change_keywords = ["cd ", "sl ", "chdir ", "set-location", "push-location"];
        if dir_change_keywords.iter().any(|k| lower_cmd.contains(k)) {
            return "沙箱限制：禁止在沙箱会话中使用目录切换命令（cd/Set-Location）。".to_string();
        }
    }

    // --- 3. 权限检查（只读自动放行 + 危险命令确认 + 破坏性警告） ---
    if !is_readonly_command(cmd) {
        let lower_cmd = cmd.to_lowercase();
        let dangerous_keywords = [
            "del ", "rm ", "format ", "rd ", "rmdir ",
            "remove-item", "clear-content", "stop-process", "kill ",
        ];
        let needs_permission = dangerous_keywords.iter().any(|k| lower_cmd.contains(k));

        if needs_permission {
            let mut perm_msg = format!("高风险命令：{}", cmd);

            // 附加用途说明
            if !description.is_empty() {
                perm_msg.push_str(&format!("\n用途说明：{}", description));
            }

            // 附加破坏性命令警告
            if let Some(warning) = get_destructive_warning(cmd) {
                perm_msg.push_str(&format!("\n\n{}", warning));
            }

            let decision = request_permission(app, session_id, &perm_msg).await;
            if decision == "reject" {
                return "权限拒绝".to_string();
            }
        }
    }

    // --- 4. 后台模式 → 委托 BackgroundManager ---
    if run_in_bg {
        let result = background_run_internal(app, cmd, &ws).await;
        if warnings.is_empty() {
            return result;
        } else {
            return format!("{}\n\n[警告]\n{}", result, warnings.join("\n"));
        }
    }

    // --- 5. 同步模式 → tokio::process::Command + timeout ---
    let exec_dir = ws.unwrap_or_else(|| std::env::current_dir().unwrap_or_default());

    let result = tokio::time::timeout(
        Duration::from_secs(timeout_secs),
        run_shell_async(cmd, &exec_dir),
    ).await;

    match result {
        Ok((stdout, stderr, exit_code)) => {
            let mut output = format_shell_output(cmd, &stdout, &stderr, exit_code, MAX_SHELL_OUTPUT_LEN);
            if !warnings.is_empty() {
                output.push_str(&format!("\n\n[警告]\n{}", warnings.join("\n")));
            }
            output
        }
        Err(_) => {
            format!(
                "[exit code: -1]\n命令执行超时（{}秒）。如果是长周期任务，请使用 `run_in_background: true`。",
                timeout_secs
            )
        }
    }
}

/// 执行只读 git 命令
pub async fn git_command(
    app: &tauri::AppHandle,
    input: &serde_json::Value,
    session_id: &str,
) -> String {
    let args_value = input["args"].as_array().unwrap();
    let args: Vec<&str> = args_value.iter().filter_map(|v| v.as_str()).collect();

    let dangerous_git_args = [
        "push", "commit", "rebase", "reset", "revert", "clean", "checkout",
    ];
    if args
        .iter()
        .any(|arg| dangerous_git_args.contains(&arg.to_lowercase().as_str()))
    {
        return format!(
            "安全拦截：git_command 工具仅用于只读操作，禁止执行 '{}'。",
            args.join(" ")
        );
    }

    let ws = get_workspace(app, session_id).await;

    // 如果是沙箱会话，检查路径参数
    if let Some(ref workspace) = ws {
        for arg in &args {
            if arg.contains(":") || arg.contains("..") {
                 if !is_within_workspace(arg, Some(workspace)) {
                     return format!("沙箱限制：git 参数包含沙箱外路径 '{}'", arg);
                 }
            }
        }
    }

    let exec_dir = ws.unwrap_or_else(|| std::env::current_dir().unwrap_or_default());

    // 使用 tokio 异步执行 git
    match tokio::process::Command::new("git")
        .current_dir(&exec_dir)
        .args(&args)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
    {
        Ok(mut child) => {
            let stdout = child.stdout.take();
            let stderr = child.stderr.take();

            let stdout_handle = tokio::spawn(async move {
                if let Some(stdout) = stdout {
                    let reader = BufReader::new(stdout);
                    let mut lines = reader.lines();
                    let mut output = String::new();
                    while let Ok(Some(line)) = lines.next_line().await {
                        output.push_str(&line);
                        output.push('\n');
                    }
                    output
                } else {
                    String::new()
                }
            });

            let stderr_handle = tokio::spawn(async move {
                if let Some(stderr) = stderr {
                    let reader = BufReader::new(stderr);
                    let mut lines = reader.lines();
                    let mut output = String::new();
                    while let Ok(Some(line)) = lines.next_line().await {
                        output.push_str(&line);
                        output.push('\n');
                    }
                    output
                } else {
                    String::new()
                }
            });

            let stdout_out = stdout_handle.await.unwrap_or_default();
            let stderr_out = stderr_handle.await.unwrap_or_default();

            let exit_code = child.wait().await
                .map(|s| s.code().unwrap_or(-1))
                .unwrap_or(-1);

            format_shell_output(&format!("git {}", args.join(" ")), &stdout_out, &stderr_out, exit_code, MAX_SHELL_OUTPUT_LEN)
        }
        Err(e) => format!("[exit code: -1]\nGit 命令执行失败: {}", e),
    }
}

/// 后台执行长时间运行的命令（独立工具，保留供 UI 直接触发）
pub async fn background_run(
    app: &tauri::AppHandle,
    input: &serde_json::Value,
    session_id: &str,
) -> String {
    let cmd = input["command"].as_str().unwrap_or("");
    let dir = input["dir"].as_str().map(|s| s.to_string());

    let ws = get_workspace(app, session_id).await;

    // 如果是沙箱会话，验证 dir
    if let Some(ref workspace) = ws {
        if let Some(ref d) = dir {
            if !is_within_workspace(d, Some(workspace)) {
                return format!("沙箱限制：指定的目录 '{}' 不在沙箱内。", d);
            }
        }
    }

    // 如果没有提供路径，则用工作目录
    let exec_dir = if let Some(d) = dir {
        Some(d)
    } else {
        ws.map(|p| p.to_string_lossy().into_owned())
    };

    crate::core::infra::background::BackgroundManager::run(app.clone(), cmd.to_string(), exec_dir).await
}

/// 检查后台任务状态
pub async fn check_background(
    app: &tauri::AppHandle,
    input: &serde_json::Value,
    _session_id: &str,
) -> String {
    let task_id = input["task_id"].as_str().map(|s| s.to_string());
    crate::core::infra::background::BackgroundManager::check(app, task_id).await
}

// --- 工具注册 ---
crate::define_tools! {
    pub fn register_tools(registry) {
        ToolDef {
            name: "run_shell",
            description: if cfg!(target_os = "windows") { "执行 Windows PowerShell 命令（统一入口）" } else { "执行 Unix bash 命令（统一入口）" },
            search_hint: "shell powershell bash command execute run background",
            schema: json!({
                "name": "run_shell",
                "description": shell_tool_description(),
                "input_schema": {
                    "type": "object",
                    "properties": {
                        "command": {"type": "string", "description": if cfg!(target_os = "windows") { "要执行的 PowerShell 命令" } else { "要执行的 bash 命令" }},
                        "description": {"type": "string", "description": "一句话说明命令用途（显示在权限确认中）"},
                        "timeout": {"type": "integer", "description": "超时秒数，默认 120，范围 5-600"},
                        "run_in_background": {"type": "boolean", "description": "是否后台执行。长周期任务（如开发服务器）必须设为 true。"}
                    },
                    "required": ["command", "description"]
                }
            }),
            should_defer: true,
            is_read_only: false,
            is_concurrency_safe: false,
            is_enabled: true,
        },
        ToolDef {
            name: "git_command",
            description: "执行低风险的 git 操作（status/diff/log 等）",
            search_hint: "git status diff log version control",
            schema: json!({
                "name": "git_command",
                "description": "执行低风险的 git 操作（如 status, diff, log）。禁止执行修改历史或推送的操作。",
                "input_schema": {
                    "type": "object",
                    "properties": {
                        "args": {
                            "type": "array",
                            "items": {"type": "string"},
                            "description": "git 命令的参数列表，例如 [\"status\"] 或 [\"log\", \"-n\", \"5\"]"
                        }
                    },
                    "required": ["args"]
                }
            }),
            should_defer: true,
            is_read_only: true,
            is_concurrency_safe: true,
            is_enabled: true,
        },
        ToolDef {
            name: "background_run",
            description: "在后台执行长时间运行的命令（独立入口）",
            search_hint: "background long running server dev",
            schema: json!({
                "name": "background_run",
                "description": "在后台执行长时间运行的命令（如启动前端 npm run dev、后端服务器等）。执行后立刻返回任务ID，不阻塞对话。推荐优先使用 run_shell 的 run_in_background 参数。",
                "input_schema": {
                    "type": "object",
                    "properties": {
                        "command": {"type": "string", "description": "要执行的具体命令（如 npm run dev）"},
                        "dir": {"type": "string", "description": "命令执行的工作目录的绝对路径"}
                    },
                    "required": ["command", "dir"]
                }
            }),
            should_defer: true,
            is_read_only: false,
            is_concurrency_safe: true,
            is_enabled: true,
        },
        ToolDef {
            name: "check_background",
            description: "检查后台任务的执行状态和输出",
            search_hint: "check background task status output",
            schema: json!({
                "name": "check_background",
                "description": "检查后台任务的执行状态和输出。仅当用户主动询问后台任务状态时才使用，严禁在自己的思考循环中连续轮询此工具！",
                "input_schema": {
                    "type": "object",
                    "properties": {
                        "task_id": {"type": "string", "description": "后台任务 ID。如果留空则返回所有任务状态。"}
                    }
                }
            }),
            should_defer: true,
            is_read_only: true,
            is_concurrency_safe: true,
            is_enabled: true,
        }
    }
}
