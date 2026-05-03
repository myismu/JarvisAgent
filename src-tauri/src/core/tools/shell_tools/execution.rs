//! # execution.rs — Shell 命令核心执行器
//!
//! 处理本地或沙箱环境中的 Shell 命令执行，包含超时控制、输出截取和安全校验。
//!
//! ## Key Exports
//! -  run_shell(): 工具入口：执行 Shell 命令（支持同步/后台模式）
//!
//! ## Dependencies
//! - Internal: super::security, super::utils, crate::core::tools::framework::permission
//! - External: serde_json, 	auri, 	okio
//!
//! ## Constraints
//! - 执行时间受限于 DEFAULT_TIMEOUT_SECS 除非转为后台模式

use super::super::framework::permission::request_permission;
use super::background::background_run_internal;
use super::readonly::is_readonly_command;
use super::security::*;
use super::utils::*;
use std::process::Stdio;
use std::time::Duration;
use tokio::io::{AsyncBufReadExt, BufReader};

const MAX_SHELL_OUTPUT_LEN: usize = 50000;
const DEFAULT_TIMEOUT_SECS: u64 = 120;

/// 异步执行 shell 命令（按平台选择 PowerShell 或 bash），返回 (stdout, stderr, exit_code)
async fn run_shell_async(cmd: &str, exec_dir: &std::path::Path) -> (String, String, i32) {
    let (shell, args) = if cfg!(target_os = "windows") {
        let ps_cmd = format!(
            "[Console]::OutputEncoding = [System.Text.Encoding]::UTF8; {}",
            cmd
        );
        (
            "powershell".to_string(),
            vec![
                "-NoProfile".to_string(),
                "-NonInteractive".to_string(),
                "-Command".to_string(),
                ps_cmd,
            ],
        )
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

            let exit_code = child
                .wait()
                .await
                .map(|s| s.code().unwrap_or(-1))
                .unwrap_or(-1);

            (stdout_out, stderr_out, exit_code)
        }
        Err(e) => (String::new(), format!("执行失败: {}", e), -1),
    }
}

/// 执行 shell 命令（统一入口，支持同步/后台模式，按平台自动选择 PowerShell 或 bash）
pub async fn run_shell(
    app: &tauri::AppHandle,
    input: &serde_json::Value,
    session_id: &str,
) -> String {
    let cmd = input["command"].as_str().unwrap_or("");
    let description = input["description"].as_str().unwrap_or("");
    let timeout_secs = input["timeout"]
        .as_u64()
        .unwrap_or(DEFAULT_TIMEOUT_SECS)
        .clamp(5, 600);
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
            "del ",
            "rm ",
            "format ",
            "rd ",
            "rmdir ",
            "remove-item",
            "clear-content",
            "stop-process",
            "kill ",
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
    )
    .await;

    match result {
        Ok((stdout, stderr, exit_code)) => {
            let mut output =
                format_shell_output(cmd, &stdout, &stderr, exit_code, MAX_SHELL_OUTPUT_LEN);
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
