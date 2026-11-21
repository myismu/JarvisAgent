//! # execution.rs — Shell 命令核心执行器
//!
//! 处理本地或沙箱环境中的 Shell 命令执行，包含超时控制、输出截取和安全校验。
//!
//! ## Key Exports
//! -  run_shell(): 工具入口：执行 Shell 命令（支持同步/后台模式）
//!
//! ## Dependencies
//! - Internal: super::security, super::utils, crate::core::tools::framework::permission
//! - External: serde_json, tauri, tokio
//!
//! ## Constraints
//! - 执行时间受限于 DEFAULT_TIMEOUT_SECS 除非转为后台模式

use super::super::framework;
use super::super::framework::permission::{
    is_within_workspace, request_permission_with_origin, PermissionDecision, PermissionKind,
};
use super::background::background_run_internal;
use super::readonly::is_readonly_command;
use super::security::*;
use super::utils::{is_exit_code_error, *};
use crate::core::tools::file_tools::workspace::resolve_exec_path;
use crate::infra::shell_command::NoWindow;
use std::process::Stdio;
use std::time::Duration;
use tokio::io::{AsyncBufReadExt, BufReader};

const MAX_SHELL_OUTPUT_LEN: usize = 50000;
const DEFAULT_TIMEOUT_SECS: u64 = 120;

/// 判断命令里是否存在**真正落盘**的输出重定向。
///
/// `>` / `>>` 后紧跟 `&` 是文件描述符复制（`1>&2` / `2>&1`），只改输出流、不写文件，
/// 因此不算重定向。以前的 `contains('>')` 一刀切把 `cmd /c "echo log 1>&2"` 误判成写盘。
///
/// 只认 ASCII 字节，与 `is_file_mutation_command` 里 `to_lowercase()` 后的口径一致；
/// 两侧（本模块与 readonly.rs）必须共用本函数，避免判定漂移。
pub(super) fn has_file_redirect(cmd: &str) -> bool {
    let b = cmd.as_bytes();
    let mut i = 0;
    while i < b.len() {
        if b[i] == b'>' {
            // `>>` 追加写：跳过第二个 '>' 再看后面是否是 '&'
            let after = if b.get(i + 1) == Some(&b'>') { i + 2 } else { i + 1 };
            if b.get(after) != Some(&b'&') {
                return true;
            }
            // `1>&2` / `2>&1`：复制型重定向，不落盘，跳过
            i = after + 1;
            continue;
        }
        i += 1;
    }
    false
}

/// 取出命令的「段首命令名」，供别名判定使用。
///
/// 按 `;`、`|`、换行、`&&` 切段，每段取第一个 token；遇到 `cmd /c` /
/// `powershell -command` 包装则跳过开关取真正的命令名。
///
/// 为什么不用 `contains("del ")`：那样 `rg "del "`、`git log --grep="rm "`
/// 这类**参数里的字样**也会被当成删除命令拦住。
fn command_names(cmd: &str) -> Vec<String> {
    let normalized = cmd.replace("&&", ";");
    normalized
        .split(|c| c == ';' || c == '|' || c == '\n')
        .filter_map(|seg| {
            let mut tokens = seg.split_whitespace();
            let mut first = tokens
                .next()?
                .trim_matches(|c| c == '"' || c == '\'')
                .to_string();
            let lower = first.to_lowercase();
            if matches!(lower.as_str(), "cmd" | "powershell" | "pwsh") {
                for tok in tokens {
                    let t = tok.trim_matches(|c| c == '"' || c == '\'');
                    if t.starts_with('/') || t.starts_with('-') {
                        continue;
                    }
                    first = t.to_string();
                    break;
                }
            }
            Some(first)
        })
        .collect()
}

/// 裸别名（del / rm / mv / ren / cp）只有作为**段首命令**才算文件操作。
fn has_alias_command(cmd: &str, aliases: &[&str]) -> bool {
    command_names(cmd)
        .iter()
        .any(|name| aliases.iter().any(|a| name.eq_ignore_ascii_case(a)))
}

/// 是否包含目录切换命令。按**段首命令名**判定，不用 contains：
/// 否则 `rg "cd "`、`Select-String "cd "` 这类参数里的字样会被误拦（而且是硬拦）。
pub(super) fn is_dir_change_command(cmd: &str) -> bool {
    has_alias_command(
        cmd,
        &["cd", "sl", "chdir", "set-location", "push-location"],
    )
}

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

    if lower.contains("set-content") || lower.contains("out-file") || lower.contains("add-content")
    {
        return Some("请使用 WriteFile 或 EditFile 工具，不要用 PowerShell cmdlet 写文件");
    }
    // New-Item -ItemType File 创建文件需拦，-ItemType Directory 放行
    // 只看 `-itemtype file`：以前的 `contains("file")` 会被路径/参数里的 file 字样误伤
    if lower.contains("new-item") && lower.contains("-itemtype file") {
        return Some("请使用 WriteFile 工具，不要用 New-Item 创建文件");
    }
    if lower.contains("remove-item") || has_alias_command(&lower, &["del", "rm", "rmdir", "deltree"]) {
        return Some("请使用 DeleteFile 工具，不要用 shell 命令删除文件");
    }
    // 只算真正落盘的重定向：`1>&2` / `2>&1` 这类 fd 复制不算（见 has_file_redirect）
    if has_file_redirect(&lower)
        && !lower.contains("git ")
        && !lower.contains("npm ")
        && !lower.contains("cargo ")
        && !lower.contains("pnpm ")
    {
        return Some("请使用 WriteFile/EditFile 工具，不要用 shell 重定向写文件");
    }
    if lower.contains("move-item")
        || lower.contains("rename-item")
        || has_alias_command(&lower, &["ren", "rename", "mv", "move"])
    {
        return Some("请使用 RenameFile 工具，不要用 shell 命令重命名文件");
    }
    if lower.contains("copy-item") || has_alias_command(&lower, &["cp", "copy"]) {
        return Some("请使用 WriteFile 工具创建文件，不要用 shell 命令复制文件");
    }
    None
}

/// 异步执行 shell 命令（按平台选择 PowerShell 或 bash），返回 (stdout, stderr, exit_code)
async fn run_shell_async(cmd: &str, exec_dir: &std::path::Path) -> (String, String, i32) {
    let (shell, args) = if cfg!(target_os = "windows") {
        // Windows 命令构造统一走 infra 公共层：本机 PowerShell 5.1 不支持 `&&`
        // （PS 7+ 语法），公共层会把它展开成"逐段执行 + 前段失败即停"，
        // 语义与 bash 的 `&&` 一致，不依赖 shell 版本（详见 infra/shell_command.rs）。
        let ps_cmd = crate::infra::shell_command::build_windows_ps_command(cmd);
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
        .no_window()
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
    outer_approved: bool,
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
        if is_dir_change_command(&lower_cmd) {
            return framework::ToolCallResult::blocked(
                "沙箱限制：禁止在沙箱会话中使用目录切换命令（cd/Set-Location）。".to_string(),
            );
        }
    }

    // --- 2b. 解析 dir 参数（命令的工作目录；2026-09-21 新增）---
    //
    // 背景：RunCommand 原先恒在工作区根目录执行（dir 是悬空引用），而沙箱禁止 cd，
    // "在子目录跑 npm install" 根本无法做到。2026-11-20 起前后台模式统一按 dir 执行，
    // 独立的后台工具 StartBackgroundCommand 已并入本工具（run_in_background=true）。
    //
    // 校验口径：沙箱会话下 dir 必须落在工作区内；相对路径按工作区目录解析
    // （复用 resolve_exec_path，避免"校验按 ws join、执行按进程 CWD"的沙箱逃逸）。
    let exec_dir = match input["dir"]
        .as_str()
        .map(str::trim)
        .filter(|s| !s.is_empty())
    {
        Some(dir) => {
            if let Some(ref workspace) = ws {
                if !is_within_workspace(dir, Some(workspace)) {
                    return framework::ToolCallResult::blocked(format!(
                        "沙箱限制：指定的 dir '{}' 不在沙箱内。",
                        dir
                    ));
                }
            }
            std::path::PathBuf::from(resolve_exec_path(dir, ws.as_deref()))
        }
        // 未指定 → 沿用原行为：沙箱会话用工作区根目录，非沙箱会话用进程 CWD
        None => ws.unwrap_or_else(|| std::env::current_dir().unwrap_or_default()),
    };

    // --- 3. 二道门兜底确认（仅直调路径；正常 dispatch 路径外层已批，不再重复弹卡） ---
    // 正常路径：dispatch_tool_call → enforce（外层弹卡人审）→ 批准后才到这里，
    // outer_approved=true 直接跳过本段——否则同一次执行会连弹两张卡。
    // 二道门本身保留：若未来出现绕过 enforce 的直调路径（outer_approved=false），
    // 危险命令在这里仍会被拦下问一次（含「本次会话都允许」登记，口径与外层一致）。
    if !outer_approved && !is_readonly_command(cmd) {
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

            // 附加破坏性命令警告（同时透传给前端做弱警示样式，格式与外层弹卡一致）
            let destructive = get_destructive_warning(cmd);
            if let Some(warning) = &destructive {
                perm_msg.push_str(&format!("\n风险提示：{}", warning));
            }

            // 第二道门：外层判定（dispatch_tool_call → policy_guard::enforce）已经问过一次，
            // 所以先查"本会话都允许"是否已覆盖这条命令——否则用户点过"本次会话都允许"之后，
            // 同一条命令每跑一次还会再弹一次卡，等于那个按钮对命令类不起作用。
            // 键口径与 `policy_guard::allowance_key_for` 共用同一实现，两道门不会打架。
            // 与外层判定同口径：链式命令一段一把键（command_prefix_scopes）
            let keys = framework::policy_guard::command_prefix_scopes(cmd).unwrap_or_default();
            // 危险命令与外层 enforce 同口径（policy::ask_always_repeated）：每次亲手批——
            // 不查会话允许（防历史登记残留绕过），不发范围键（防「本次会话都允许」
            // 从这张卡绕回来：下面 AllowSession 分支会真的 grant_session_allowance）
            let already_allowed = destructive.is_none()
                && framework::policy_guard::command_allowed(app, session_id, cmd).await;

            if !already_allowed {
                // 把放开范围写进卡片，跟外层弹卡的文案保持一致（危险卡没有范围键，不写）
                if destructive.is_none() {
                    if keys.len() == 1 {
                        perm_msg.push_str(&format!("\n允许范围：{}", keys[0].1));
                    } else if keys.len() > 1 {
                        perm_msg.push_str("\n允许范围（一条链式命令，点一次会同时登记以下范围）：");
                        for (_, label) in &keys {
                            perm_msg.push_str(&format!("\n· {}", label));
                        }
                    }
                }
                let pending_keys: Vec<(String, String)> = if destructive.is_some() {
                    // 危险命令：没有「本次会话都允许」，与外层弹卡同口径
                    Vec::new()
                } else {
                    keys.iter()
                        .map(|(scope, _)| {
                            (
                                framework::policy_guard::ALLOWANCE_KIND_COMMAND.to_string(),
                                scope.clone(),
                            )
                        })
                        .collect()
                };
                let decision = request_permission_with_origin(
                    app,
                    session_id,
                    &perm_msg,
                    PermissionKind::Tool,
                    pending_keys,
                    None,
                    destructive.as_deref(),
                )
                .await;
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
                // 用户授权本会话 → 登记键（顺带放行其它已挂起的同范围请求）
                if decision == PermissionDecision::AllowSession {
                    // 一次点击把各段的键都登记（用户已在卡片上看到"同时登记以下范围"）
                    for (scope, label) in keys {
                        framework::policy_guard::grant_session_allowance(
                            app,
                            session_id,
                            framework::policy_guard::ALLOWANCE_KIND_COMMAND,
                            &scope,
                            label,
                        )
                        .await;
                    }
                }
            }
        }
    }

    // --- 4. 后台模式 → 委托 BackgroundManager ---
    if run_in_bg {
        let result = background_run_internal(app, cmd, &exec_dir, session_id).await;
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
    fn dir_change_detection_ignores_arguments() {
        use super::is_dir_change_command;
        // 真命令：段首是目录切换命令
        assert!(is_dir_change_command("cd sub"));
        assert!(is_dir_change_command("npm install && cd sub"));
        assert!(is_dir_change_command("set-location sub"));
        // 参数里的字样：不该被当成切目录（以前 contains("cd ") 会硬拦）
        assert!(!is_dir_change_command(r#"rg "cd " src/"#));
        assert!(!is_dir_change_command(r#"Select-String "cd " a.txt"#));
        assert!(!is_dir_change_command("Get-ChildItem a.txt"));
    }

    #[test]
    fn allows_aliases_that_only_appear_inside_arguments() {
        // 参数里的 `del ` / `rm ` 字样不是删除命令（以前 contains("del ") 会误拦）
        for cmd in [
            r#"rg "del " src/"#,
            r#"git log --grep="rm ""#,
            r#"Get-Content a.txt | Select-String "cp ""#,
        ] {
            assert!(
                is_file_mutation_command(cmd).is_none(),
                "参数里的字样不该被当成文件操作：{}",
                cmd
            );
        }
    }

    #[test]
    fn still_blocks_bare_alias_commands() {
        for cmd in [
            "del a.txt",
            "rm -rf build",
            "rmdir empty",
            "mv a.txt b.txt",
            "ren a.txt b.txt",
            "cp a.txt b.txt",
            r#"cmd /c del a.txt"#,
        ] {
            assert!(
                is_file_mutation_command(cmd).is_some(),
                "裸别名作为段首命令时仍应拦住：{}",
                cmd
            );
        }
    }

    #[test]
    fn new_item_judges_by_itemtype() {
        // 建目录放行，建文件拦下；不再用 contains("file") 这种全串模糊判定
        assert!(is_file_mutation_command("New-Item -ItemType Directory -Path build").is_none());
        assert!(is_file_mutation_command("New-Item -ItemType File -Path a.txt").is_some());
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

    /// fd 复制型重定向（`1>&2` / `2>&1`）只改输出流、不落盘，不算写文件
    #[test]
    fn allows_fd_redirection() {
        for cmd in [
            r#"cmd /c "echo log 1>&2""#,
            "echo hi 2>&1",
            r#"cmd /c "exit 3" 2>&1"#,
        ] {
            assert!(
                is_file_mutation_command(cmd).is_none(),
                "fd 复制重定向不算写文件，不该被拦：{}",
                cmd
            );
        }
    }

    /// 收紧粒度后真正的重定向写盘仍必须拦下
    #[test]
    fn still_blocks_real_file_redirect() {
        for cmd in [
            "echo hi > a.txt",
            "echo hi >> a.txt",
            "Get-Content a.txt > b.txt",
        ] {
            assert!(
                is_file_mutation_command(cmd).is_some(),
                "真正的重定向写文件必须被拦：{}",
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
