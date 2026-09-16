//! # git.rs — 只读 Git 操作封装
//!
//! 执行受限的 git 命令，拦截任何可能修改代码库状态的操作。
//!
//! ## Key Exports
//! - git_command(): 工具入口：执行 git (如 status, diff, log)
//!
//! ## Dependencies
//! - Internal: super::utils, super::readonly, super::regexes, crate::core::tools::framework::permission
//! - External: serde_json, tauri, tokio
//!
//! ## Constraints
//! - 仅允许部分安全的 read-only git 子命令（正向白名单，见 `regexes::READONLY_GIT_ARGS`）

use super::super::framework;
use super::super::framework::permission::is_within_workspace;
use super::utils::*;
use std::process::Stdio;
use tokio::io::{AsyncBufReadExt, BufReader};

const MAX_SHELL_OUTPUT_LEN: usize = 50000;

/// 执行只读 git 命令
pub async fn git_command(
    app: &tauri::AppHandle,
    input: &serde_json::Value,
    session_id: &str,
) -> framework::ToolCallResult {
    let args_value = input["args"].as_array().unwrap();
    let args: Vec<&str> = args_value.iter().filter_map(|v| v.as_str()).collect();

    // 只读判定走**正向白名单**（实现见 `readonly::is_readonly_git_args`）。
    //
    // 这里以前是一份黑名单（push/commit/rebase/reset/revert/clean/checkout），
    // 漏掉了 `git restore .`、`git stash`、`git apply`、`git add`、`git rm` ——
    // 也就是说规划模式（和只读保护）下能把未提交的改动直接丢掉。
    // 黑名单永远补不全，所以翻成正向：名单之外的子命令一律算"非只读"。
    if !super::readonly::is_readonly_git_args(&args) {
        return framework::ToolCallResult::error(format!(
            "安全拦截：RunGitCommand 工具仅用于只读操作，禁止执行 'git {}'。\n\
             允许的只读子命令：{}\n\
             需要改动仓库的操作（add/commit/restore/stash/apply/push…）请改用 RunCommand 并说明用途。",
            args.join(" "),
            super::regexes::READONLY_GIT_ARGS.join(" / ")
        ));
    }

    let ws = get_workspace(app, session_id).await;

    // 如果是沙箱会话，检查路径参数
    if let Some(ref workspace) = ws {
        for arg in &args {
            if arg.contains(":") || arg.contains("..") {
                if !is_within_workspace(arg, Some(workspace)) {
                    return framework::ToolCallResult::error(format!("沙箱限制：git 参数包含沙箱外路径 '{}'", arg));
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

            let exit_code = child
                .wait()
                .await
                .map(|s| s.code().unwrap_or(-1))
                .unwrap_or(-1);

            let output = format_shell_output(
                &format!("git {}", args.join(" ")),
                &stdout_out,
                &stderr_out,
                exit_code,
                MAX_SHELL_OUTPUT_LEN,
            );
            // git 的只读命令（diff/grep/log 等）exit code 1 通常是信息性的
            // 使用 is_exit_code_error 基于命令语义判断，而非简单的 exit_code == 0
            if is_exit_code_error(&format!("git {}", args.join(" ")), exit_code) {
                framework::ToolCallResult::error(output)
            } else {
                framework::ToolCallResult::ok(output)
            }
        }
        Err(e) => framework::ToolCallResult::error(format!("[exit code: -1]\nGit 命令执行失败: {}", e)),
    }
}
