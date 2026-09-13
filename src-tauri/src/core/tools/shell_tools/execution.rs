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

use super::super::framework;
use super::super::framework::permission::{request_permission, PermissionKind};
use super::background::background_run_internal;
use super::readonly::is_readonly_command;
use super::security::*;
use super::utils::{*, is_exit_code_error};
use std::process::Stdio;
use std::time::Duration;
use tokio::io::{AsyncBufReadExt, BufReader};

const MAX_SHELL_OUTPUT_LEN: usize = 50000;
const DEFAULT_TIMEOUT_SECS: u64 = 120;

/// 检测命令是否为文件修改操作，拦截并引导 Agent 使用专用文件工具
///
/// 对 `pub` 是给权限"只观察"模块复用：观察阶段要按同一套结论记录"本该拒绝"，
/// 不能另写一份判断，否则口径会漂移。
pub fn is_file_mutation_command(cmd: &str) -> Option<&'static str> {
    let lower = cmd.to_lowercase().trim().to_string();

    // ── .NET 直接调用的文件读写（PowerShell 里 `[IO.File]::WriteAllText(...)` 这种写法）──
    //
    // 为什么必须单独认：专用文件工具才有沙箱检查、快照和回滚，而从 PowerShell 调 .NET
    // 能直接落盘，绕过全部三样。之前只认 Set-Content/Out-File 这类 cmdlet，
    // 实测中子代理正是用 `[System.IO.File]::WriteAllText` 把文件写掉的。
    //
    // 认法分两种，避免误伤：
    //   1) 写方法名独一无二：出现即命中（WriteAllText / AppendAllText / OpenWrite …）
    //   2) 通用方法名（Delete/Move/Copy/Create/Replace）必须和 File/Directory 调用上下文同时出现
    const DOTNET_WRITE_METHODS: &[&str] = &[
        "writealltext(",
        "writeallbytes(",
        "writealllines(",
        "appendalltext(",
        "appendalllines(",
        "openwrite(",
        "createdirectory(",
        "deletedirectory(",
        "setlastwritetime(",
        "setcreationtime(",
        "setlastaccesstime(",
    ];
    const DOTNET_MUTATING_METHODS: &[&str] = &[
        "delete(",
        "move(",
        "copy(",
        "replace(",
        "create(",
        "moveto(",
        "copyto(",
        // 写入流：`(New-Object System.IO.StreamWriter(...)).Write("x")`
        "write(",
        "writeline(",
    ];
    // 只要命令里出现这些名字（带不带方括号都算，`[IO.File]::` 和 `New-Object System.IO.File...` 都覆盖），
    // 再叠加"写入类方法名"才判命中——避免误伤 `[System.IO.File]::Exists()` 这种只读用法
    const DOTNET_FILE_CONTEXT: &[&str] = &[
        "io.file",
        "io.directory",
        "io.fileinfo",
        "io.filestream",
        "io.streamwriter",
        "io.binarywriter",
    ];

    let has_dotnet_write = DOTNET_WRITE_METHODS
        .iter()
        .any(|marker| lower.contains(marker))
        || (DOTNET_FILE_CONTEXT.iter().any(|c| lower.contains(c))
            && DOTNET_MUTATING_METHODS
                .iter()
                .any(|method| lower.contains(method)));
    if has_dotnet_write {
        return Some(
            "检测到用 .NET 方法（[IO.File]::WriteAllText 之类）直接改文件。\
             请改用 WriteFile / EditFile / DeleteFile / RenameFile 工具——\
             只有专用工具有沙箱检查、快照与回滚",
        );
    }

    if lower.contains("set-content") || lower.contains("out-file") || lower.contains("add-content") {
        return Some("请使用 WriteFile 或 EditFile 工具，不要用 PowerShell cmdlet 写文件");
    }
    // New-Item -ItemType File 创建文件需拦，-ItemType Directory 放行
    if lower.contains("new-item") && lower.contains("file") {
        return Some("请使用 WriteFile 工具，不要用 New-Item 创建文件");
    }
    if lower.contains("remove-item") || lower.contains("del ") || lower.contains("rm ") || lower.contains("rmdir ") {
        return Some("请使用 DeleteFile 工具，不要用 shell 命令删除文件");
    }
    if (lower.contains(">") || lower.contains(">>"))
        && !lower.contains("git ") && !lower.contains("npm ") && !lower.contains("cargo ") && !lower.contains("pnpm ")
    {
        return Some("请使用 WriteFile/EditFile 工具，不要用 shell 重定向写文件");
    }
    if lower.contains("move-item") || lower.contains("rename-item") || lower.contains("ren ") || lower.contains("mv ") {
        return Some("请使用 RenameFile 工具，不要用 shell 命令重命名文件");
    }
    if lower.contains("copy-item") || lower.contains("cp ") {
        return Some("请使用 WriteFile 工具创建文件，不要用 shell 命令复制文件");
    }
    None
}

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
) -> framework::ToolCallResult {
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
            return framework::ToolCallResult::blocked(format!("安全拦截：{}", msg));
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
            return framework::ToolCallResult::error(e);
        }

        let lower_cmd = cmd.to_lowercase();
        let dir_change_keywords = ["cd ", "sl ", "chdir ", "set-location", "push-location"];
        if dir_change_keywords.iter().any(|k| lower_cmd.contains(k)) {
            return framework::ToolCallResult::blocked("沙箱限制：禁止在沙箱会话中使用目录切换命令（cd/Set-Location）。".to_string());
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

            let decision =
                request_permission(app, session_id, &perm_msg, PermissionKind::Tool).await;
            if !decision.is_allowed() {
                // 明确拒绝 / 未取得结论（取消、通道关闭）都不执行这条命令，
                // 并把用户的原话回灌给模型，避免它换个写法重试同一操作
                let label = if decision.is_rejected() {
                    "权限拒绝"
                } else {
                    "权限确认未完成"
                };
                return framework::ToolCallResult::blocked(format!(
                    "{}：{}",
                    label,
                    decision.model_note()
                ));
            }
        }
    }

    // --- 4. 后台模式 → 委托 BackgroundManager ---
    if run_in_bg {
        let result = background_run_internal(app, cmd, &ws).await;
        let output = if warnings.is_empty() {
            result
        } else {
            format!("{}\n\n[警告]\n{}", result, warnings.join("\n"))
        };
        return framework::ToolCallResult::ok(output);
    }

    // --- 5. 同步模式 → tokio::process::Command + timeout ---
    // 拦截文件修改类命令，引导 Agent 使用专用文件工具
    if let Some(hint) = is_file_mutation_command(cmd) {
        return framework::ToolCallResult::blocked(format!("被拦截：{}\n\nRunCommand 不支持文件写入/删除/重命名操作。请使用以下专用工具：\n- 创建/覆盖文件 → WriteFile\n- 修改文件内容 → EditFile\n- 删除文件 → DeleteFile\n- 重命名/移动文件 → RenameFile", hint));
    }

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
            if is_exit_code_error(cmd, exit_code) {
                framework::ToolCallResult::error(output)
            } else {
                framework::ToolCallResult::ok(output)
            }
        }
        Err(_) => framework::ToolCallResult::error(format!(
            "[exit code: -1]\n命令执行超时（{}秒）。如果是长周期任务，请使用 `run_in_background: true`。",
            timeout_secs
        )),
    }
}

#[cfg(test)]
mod file_mutation_tests {
    use super::is_file_mutation_command;

    #[test]
    fn blocks_dotnet_file_writes() {
        // 实测中子代理就是用这两种写法把文件写掉的（绕过快照与回滚）
        for cmd in [
            r#"[System.IO.File]::WriteAllText("C:\p\a.ts", $content)"#,
            r#"$b = [System.IO.File]::WriteAllBytes("C:\p\a.ts", $bytes)"#,
            r#"[IO.File]::AppendAllText("a.txt", "x")"#,
            r#"[System.IO.File]::Delete("a.txt")"#,
            r#"[IO.Directory]::CreateDirectory("new")"#,
            r#"(New-Object System.IO.StreamWriter("a.txt")).Write("x")"#,
        ] {
            assert!(
                is_file_mutation_command(cmd).is_some(),
                "应拦下 .NET 写文件命令：{}",
                cmd
            );
        }
    }

    #[test]
    fn still_blocks_traditional_shell_writes() {
        for cmd in [
            "Set-Content a.txt 'x'",
            "Out-File a.txt",
            "Remove-Item a.txt -Force",
            "echo hi > a.txt",
            "Move-Item a.txt b.txt",
        ] {
            assert!(
                is_file_mutation_command(cmd).is_some(),
                "应拦下传统写文件命令：{}",
                cmd
            );
        }
    }

    #[test]
    fn does_not_block_dotnet_reads_or_plain_reads() {
        // 只读的 .NET 调用不属于"改文件"，不该被这条规则拦（是否询问由权限策略决定）
        for cmd in [
            r#"$b = [System.IO.File]::ReadAllBytes("a.ts")"#,
            r#"[System.IO.File]::ReadAllText("a.ts")"#,
            r#"[System.IO.Path]::GetFullPath("a.ts")"#,
            "Get-ChildItem",
            "git status",
        ] {
            assert!(
                is_file_mutation_command(cmd).is_none(),
                "不该拦下只读命令：{}",
                cmd
            );
        }
    }

    /// 用实测日志里的**原样命令**做回归：这条是子代理当时真的执行成功、把文件写掉的命令
    #[test]
    fn blocks_real_dotnet_write_from_actual_log() {
        let real = r#"$content = "a`r`n// test`r`n// test`r`n// sub-1`r`n"; [System.IO.File]::WriteAllText("C:\Users\沐\Desktop\permission-test\a.ts", $content)"#;
        assert!(
            is_file_mutation_command(real).is_some(),
            "实测里绕过快照的这条 .NET 写命令必须被拦"
        );
    }

    /// 同一批实测命令里的只读那条：不拦（是否询问由权限策略决定）
    #[test]
    fn allows_real_dotnet_read_from_actual_log() {
        let real = r#"$bytes = [System.IO.File]::ReadAllBytes("C:\Users\沐\Desktop\permission-test\c.ts"); ($bytes | ForEach-Object { $_.ToString("X2") })"#;
        assert!(
            is_file_mutation_command(real).is_none(),
            "只读的 .NET 调用不属于改文件，不该被这条规则拦"
        );
    }
}
