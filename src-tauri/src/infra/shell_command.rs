//! # infra/shell_command.rs — Windows shell 命令构造与子进程创建公共层
//!
//! 前台（`core/tools/shell_tools/execution.rs`）与后台（`infra/background.rs`）
//! 在 Windows 上都通过 `powershell -NoProfile -Command` 执行命令，此前各自拼接
//! 命令串，且原样透传 `&&` —— 本机 Windows PowerShell **5.1** 不支持 `&&`
//! （`&&` 是 PowerShell 7+ 语法），链式命令直接 ParserError。
//!
//! 本模块收敛两类 Windows 平台差异：
//! 1. [`split_on_double_ampersand`]：引号感知，只按 `&&` 切段（`;`、`|`、`||`
//!    留在段内——PS 5.1 原生支持它们，语义不改变）；
//! 2. [`build_windows_ps_command`]：把命令展开成"逐段执行 + 前段失败即停"的
//!    PowerShell 脚本，在程序层面实现 `&&` 语义，不依赖 shell 版本；
//! 3. [`NoWindow`]：子进程创建标志——打包后主进程是 GUI 子系统、自身没有控制台，
//!    此时启动 powershell / taskkill 这类控制台程序，系统会新建一个空白终端窗口；
//!    全部进程创建点统一挂该标志，消除这个窗口，也让 dev 与 release 行为一致。
//!
//! ## 语义说明
//! - `&&`（bash / PowerShell 7）= 前一个命令**成功**才执行下一个；
//!   逐段执行时用 `if (-not $?) { exit ... }` 忠实还原（`$?` 对原生命令与
//!   cmdlet 都有效）。
//! - 已知局限：若某段是 **PowerShell cmdlet** 且失败、而更早的原生命令留下过
//!   `$LASTEXITCODE = 0`，该失败可能被吞（罕见：本项目的命令段几乎全是
//!   npm/cargo 等原生命令）。`||`（前段失败才跑下一段）PS 5.1 同样不支持，
//!   暂不处理——模型能当场看到报错并自行调整语法。
//! - 权限一致性：外层验权（`policy_guard`）按全部分隔符切段、要求**全部段**
//!   被允许；本函数只按 `&&` 再分组执行，任何一段命令都已被验权覆盖，
//!   不存在"验权切了段、执行却跑出未验权命令"的缝隙。

/// UTF-8 输出前缀：不设它，npm 等工具的中文输出会以 GBK 字节进入会话变成乱码。
const PS_UTF8_PREFIX: &str = "[Console]::OutputEncoding = [System.Text.Encoding]::UTF8; ";

/// 段间守卫：前一段失败（`$?` 为 false）则终止整个命令。
/// `$LASTEXITCODE` 只在**原生命令**（npm/cargo/git…）执行后才有值；
/// 为 `null`（纯 cmdlet 失败）时兜底 `exit 1`，避免把失败伪装成成功。
const PS_FAIL_GUARD: &str = "if (-not $?) { if ($null -ne $LASTEXITCODE) { exit $LASTEXITCODE }; exit 1 }";

/// Windows 进程创建标志：不为子进程分配控制台窗口。
///
/// 打包后主进程是 GUI 子系统（`main.rs` 的 `windows_subsystem = "windows"`），
/// 自身没有控制台。Windows 的规则是：控制台子系统程序（powershell.exe、
/// taskkill.exe）由没有控制台的父进程启动时，系统必须给它一个控制台，
/// 只能新建一个窗口——这就是那个空白终端。子进程输出已重定向进管道，
/// 所以窗口里没有任何内容。设此标志后系统不分配控制台，输出照旧走管道。
#[cfg(windows)]
const CREATE_NO_WINDOW: u32 = 0x0800_0000;

/// 引号感知，只按 `&&` 切段；`;`、`|`、`||` 与单个 `&` 一律保留在段内。
///
/// - 引号（单引号 / 双引号）内的 `&&` 是字符串内容，不切（`echo "a && b"`）；
/// - 单个 `&` 是 PowerShell 的调用操作符（`& "C:\x.ps1"`），不是分隔符；
/// - `||` 不切：PS 5.1 不支持它，原样保留让报错可见（模型可自行调整语法）。
pub fn split_on_double_ampersand(command: &str) -> Vec<String> {
    let mut segments: Vec<String> = Vec::new();
    let mut current = String::new();
    let mut quote: Option<char> = None;
    let mut chars = command.chars().peekable();

    while let Some(c) = chars.next() {
        // 引号内：一律当普通字符原样保留
        if let Some(q) = quote {
            current.push(c);
            if c == q {
                quote = None;
            }
            continue;
        }
        match c {
            '\'' | '"' => {
                quote = Some(c);
                current.push(c);
            }
            '&' => {
                // 只有 `&&` 是分隔符；单个 `&` 保留在段内
                if chars.peek() == Some(&'&') {
                    chars.next();
                    segments.push(current.trim().to_string());
                    current = String::new();
                } else {
                    current.push(c);
                }
            }
            _ => current.push(c),
        }
    }
    segments.push(current.trim().to_string());
    // 空段丢弃（`a && && b` 之类的残缺输入没有语义）
    segments.retain(|s| !s.is_empty());
    segments
}

/// 构造 Windows 下 `powershell -Command` 的完整命令串。
///
/// - 单段（绝大多数）：UTF-8 前缀 + 原命令，行为与改造前完全一致；
/// - 含 `&&` 的多段：展开为 `seg1; <失败守卫>; seg2; <失败守卫>; seg3`，
///   在 PS 5.1 上还原 `&&` 的"前段失败即停"语义，且整体退出码 = 失败段的退出码。
pub fn build_windows_ps_command(command: &str) -> String {
    let segments = split_on_double_ampersand(command);
    if segments.len() <= 1 {
        return format!("{}{}", PS_UTF8_PREFIX, command);
    }
    let mut parts: Vec<String> = Vec::with_capacity(segments.len() * 2);
    for (idx, seg) in segments.iter().enumerate() {
        parts.push(seg.clone());
        // 守卫插在**段与段之间**；最后一段后面不需要
        if idx + 1 < segments.len() {
            parts.push(PS_FAIL_GUARD.to_string());
        }
    }
    format!("{}{}", PS_UTF8_PREFIX, parts.join("; "))
}

/// 子进程创建口径：统一不弹控制台窗口。
///
/// 全项目 4 处进程创建点（前台 `execution.rs`、后台 `background.rs` 的
/// 命令执行 / taskkill / WMI 查询）都挂它，避免"release 下弹黑框、
/// dev 下不弹"的形态差异——dev 之所以不弹，只是因为主进程从终端继承了
/// 控制台、子进程附着了上去，并非行为正确。
///
/// 非 Windows 平台是空实现，调用点无需写 `cfg`，两边代码同形。
pub trait NoWindow {
    /// 标记该子进程不要控制台窗口（Windows）；其他平台无操作。
    fn no_window(&mut self) -> &mut Self;
}

impl NoWindow for tokio::process::Command {
    fn no_window(&mut self) -> &mut Self {
        // tokio 在 Windows 上原生提供 creation_flags，无需引入 std 的 CommandExt
        #[cfg(windows)]
        {
            self.creation_flags(CREATE_NO_WINDOW);
        }
        self
    }
}

impl NoWindow for std::process::Command {
    fn no_window(&mut self) -> &mut Self {
        #[cfg(windows)]
        {
            // std 侧经 CommandExt trait 暴露同一个 API
            use std::os::windows::process::CommandExt;
            self.creation_flags(CREATE_NO_WINDOW);
        }
        self
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn single_segment_stays_intact() {
        assert_eq!(split_on_double_ampersand("npm install"), ["npm install"]);
        // `;` 与 `|` 留在段内：PS 5.1 原生支持，语义不变
        assert_eq!(
            split_on_double_ampersand("npm install; npm start"),
            ["npm install; npm start"]
        );
        assert_eq!(
            split_on_double_ampersand("cat file | grep foo"),
            ["cat file | grep foo"]
        );
    }

    #[test]
    fn double_ampersand_splits() {
        assert_eq!(
            split_on_double_ampersand("npm install && npm start"),
            ["npm install", "npm start"]
        );
        assert_eq!(
            split_on_double_ampersand("a && b && c"),
            ["a", "b", "c"]
        );
    }

    #[test]
    fn quoted_ampersand_not_split() {
        assert_eq!(
            split_on_double_ampersand(r#"echo "a && b" && echo done"#),
            [r#"echo "a && b""#, "echo done"]
        );
        assert_eq!(
            split_on_double_ampersand("echo 'x && y'"),
            ["echo 'x && y'"]
        );
    }

    #[test]
    fn single_ampersand_is_call_operator() {
        // PowerShell 调用操作符 `& "path"` 不是分隔符
        assert_eq!(
            split_on_double_ampersand(r#"& "C:\x.ps1""#),
            [r#"& "C:\x.ps1""#]
        );
    }

    #[test]
    fn mixed_separators_keep_semantics() {
        // `a && b; c`：`b; c` 作为一段（`;` 由 PS 原生处理），b 失败不影响 c，
        // 段退出码取 c —— 与 bash 对该表达式的分组语义一致
        assert_eq!(split_on_double_ampersand("a && b; c"), ["a", "b; c"]);
    }

    #[test]
    fn ps_command_single_segment() {
        let out = build_windows_ps_command("npm install");
        assert!(out.starts_with(PS_UTF8_PREFIX));
        assert!(out.ends_with("npm install"));
        assert!(!out.contains("exit $LASTEXITCODE"));
    }

    #[test]
    fn ps_command_expands_ampersand_chain() {
        let out = build_windows_ps_command("npm install && npm start");
        assert!(out.starts_with(PS_UTF8_PREFIX));
        // 顺序：seg1 → 守卫 → seg2，守卫只出现在段间
        let tail = out.trim_start_matches(PS_UTF8_PREFIX).to_string();
        assert_eq!(
            tail,
            format!(
                "npm install; {}; npm start",
                PS_FAIL_GUARD
            )
        );
    }

    #[test]
    fn ps_command_three_segments_two_guards() {
        let out = build_windows_ps_command("a && b && c");
        let tail = out.trim_start_matches(PS_UTF8_PREFIX).to_string();
        assert_eq!(
            tail,
            format!("a; {}; b; {}; c", PS_FAIL_GUARD, PS_FAIL_GUARD)
        );
    }

    #[test]
    fn ps_command_quoted_chain_untouched() {
        // 引号内的 && 不展开：整条仍是单段
        let out = build_windows_ps_command(r#"echo "a && b""#);
        assert!(out.ends_with(r#"echo "a && b""#));
        assert!(!out.contains(PS_FAIL_GUARD));
    }

    /// 测试用：构造一条输出 `ok` 的跨平台命令
    fn echo_ok_program() -> (&'static str, &'static [&'static str]) {
        if cfg!(windows) {
            ("cmd", &["/C", "echo ok"])
        } else {
            ("echo", &["ok"])
        }
    }

    #[tokio::test]
    async fn no_window_keeps_command_runnable() {
        // 挂了 no_window() 后，两个 impl（std / tokio）都必须仍能正常启动并取到输出。
        // 本用例守的是"执行链路没被破坏"；"窗口是否真的不再弹出"只能在 release
        // 安装包上人工验证——cargo test 时父进程自带控制台，子进程本来就附着
        // 父控制台，这个场景下看不到弹窗。
        let (program, args) = echo_ok_program();

        // std 侧：background.rs 的 taskkill 与 WMI 查询走这条
        let out = std::process::Command::new(program)
            .no_window()
            .args(args)
            .output()
            .expect("std 侧启动失败");
        assert!(String::from_utf8_lossy(&out.stdout).contains("ok"));

        // tokio 侧：前台命令与后台长任务走这条
        let out = tokio::process::Command::new(program)
            .no_window()
            .args(args)
            .output()
            .await
            .expect("tokio 侧启动失败");
        assert!(String::from_utf8_lossy(&out.stdout).contains("ok"));
    }
}
