//! # infra/shell_command.rs — Windows shell 命令构造公共层
//!
//! 前台（`core/tools/shell_tools/execution.rs`）与后台（`infra/background.rs`）
//! 在 Windows 上都通过 `powershell -NoProfile -Command` 执行命令，此前各自拼接
//! 命令串，且原样透传 `&&` —— 本机 Windows PowerShell **5.1** 不支持 `&&`
//! （`&&` 是 PowerShell 7+ 语法），链式命令直接 ParserError。
//!
//! 本模块提供两件事：
//! 1. [`split_on_double_ampersand`]：引号感知，只按 `&&` 切段（`;`、`|`、`||`
//!    留在段内——PS 5.1 原生支持它们，语义不改变）；
//! 2. [`build_windows_ps_command`]：把命令展开成"逐段执行 + 前段失败即停"的
//!    PowerShell 脚本，在程序层面实现 `&&` 语义，不依赖 shell 版本。
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
}
