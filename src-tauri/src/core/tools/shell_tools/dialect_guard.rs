//! # dialect_guard.rs — 方言语法预检
//!
//! 防止模型在 Git Bash 环境下写出 PowerShell 命令。
//!
//! 为什么必须在执行前拦：这类错误交给 shell 不只是"报错重试"。
//! 例如 `Get-Content a.txt > b.txt`，bash 会报 `Get-Content: command not found`，
//! 但重定向仍会创建/清空 `b.txt`——错误命令产生了真实副作用。
//! 预检直接拒绝执行，并给出改法，模型下一轮就能改对。
//!
//! 只做**单向**检测：bash 环境下出现 PowerShell cmdlet。
//! 反向（PowerShell 环境写 bash 命令）不检测——PowerShell 自带
//! `ls`/`cat`/`rm`/`mv` 别名，按命令名判断误伤率过高。
//!
//! ## Key Exports
//! - `detect_foreign_shell_syntax()`: 命中返回可直接回给模型的提示文案
//!
//! ## Dependencies
//! - Internal: `crate::infra::shell_dialect`

use crate::infra::shell_dialect::ShellDialect;

/// PowerShell cmdlet 的动词前缀（`动词-名词` 形态）。
///
/// 只认常见的、在 bash 里绝不可能作为段首出现的动词；
/// `sort`、`split`、`copy` 这类与 POSIX 命令同名的词**不列入**——
/// 它们必须带连字符（`Sort-Object`）才算 cmdlet，而 `sort` 本身不在匹配范围内。
const PS_VERBS: &[&str] = &[
    "Get",
    "Set",
    "New",
    "Remove",
    "Add",
    "Clear",
    "Copy",
    "Move",
    "Rename",
    "Select",
    "Where",
    "ForEach",
    "Invoke",
    "Start",
    "Stop",
    "Test",
    "Write",
    "Read",
    "Out",
    "Import",
    "Export",
    "Enable",
    "Disable",
    "Update",
    "Register",
    "Unregister",
    "Measure",
    "Group",
    "Compare",
    "ConvertTo",
    "ConvertFrom",
    "Format",
    "Resolve",
    "Restart",
    "Wait",
    "Join",
];

/// 会包装真实命令的前置命令：跳过它们继续找段首
/// （`env FOO=1 Get-ChildItem` 的真正命令是 Get-ChildItem）
const WRAPPERS: &[&str] = &["env", "command", "exec", "time", "nohup"];

/// 检测命令里是否混入了当前方言不支持的语法。
///
/// 返回 `Some(提示文案)` 表示应当拒绝执行；`None` 表示通过。
pub fn detect_foreign_shell_syntax(dialect: &ShellDialect, cmd: &str) -> Option<String> {
    // 只有 bash 方言需要这道检查：PowerShell 下不存在"cmdlet 认不出来"的问题
    if !dialect.is_bash() {
        return None;
    }
    for segment in cmd.split(['|', ';', '&', '\n', '\r']) {
        let Some(head) = segment_head(segment) else {
            continue;
        };
        if is_powershell_cmdlet(&head) {
            return Some(build_message(&head));
        }
    }
    None
}

/// 取一个命令段的段首，跳过前导环境变量赋值与包装命令。
fn segment_head(segment: &str) -> Option<String> {
    let mut tokens = segment.split_whitespace();
    loop {
        let token = tokens.next()?;
        let token = token.trim_matches(|c| {
            c == '"' || c == '\'' || c == '(' || c == ')' || c == '{' || c == '}'
        });
        if token.is_empty() {
            continue;
        }
        // `NAME=value` 形式的环境变量前缀，不是命令本身
        if !token.starts_with('-') {
            if let Some((name, _)) = token.split_once('=') {
                let is_env_name =
                    !name.is_empty() && name.chars().all(|c| c.is_ascii_alphanumeric() || c == '_');
                if is_env_name {
                    continue;
                }
            }
        }
        let lower = token.to_lowercase();
        if WRAPPERS.iter().any(|wrapper| *wrapper == lower) {
            continue;
        }
        return Some(token.to_string());
    }
}

/// 是否为 `动词-名词` 形态的 PowerShell cmdlet。
///
/// 排除三类不可能成立的写法：
/// - 带路径前缀（`./Get-Foo.exe`）——真的可执行文件，不是 cmdlet；
/// - 带 `.exe` 后缀——同上；
/// - 连字符后面为空（`Get-`）——不构成 cmdlet。
fn is_powershell_cmdlet(head: &str) -> bool {
    if head.contains('/') || head.contains('\\') || head.to_lowercase().ends_with(".exe") {
        return false;
    }
    let Some((verb, noun)) = head.split_once('-') else {
        return false;
    };
    !noun.is_empty() && PS_VERBS.iter().any(|v| v.eq_ignore_ascii_case(verb))
}

/// 常见 cmdlet 的 POSIX 改法（没有对应项时返回 None）
fn posix_hint(head: &str) -> Option<&'static str> {
    match head.to_lowercase().as_str() {
        "get-childitem" => Some("Get-ChildItem → ls"),
        "get-content" => Some("Get-Content → cat"),
        "set-content" => Some("Set-Content → 使用 WriteFile / EditFile 工具"),
        "add-content" => Some("Add-Content → 使用 EditFile 工具"),
        "remove-item" => Some("Remove-Item → 使用 DeleteFile 工具"),
        "copy-item" => Some("Copy-Item → 使用 WriteFile 工具"),
        "move-item" => Some("Move-Item → 使用 RenameFile 工具"),
        "new-item" => Some("New-Item → 创建文件用 WriteFile，创建目录用 mkdir -p"),
        "select-string" => Some("Select-String → grep"),
        "where-object" => Some("Where-Object → grep"),
        "foreach-object" => Some("ForEach-Object → 管道配合 xargs 或 shell 循环"),
        "invoke-webrequest" | "invoke-restmethod" => Some("Invoke-WebRequest → curl"),
        _ => None,
    }
}

fn build_message(head: &str) -> String {
    let hint = posix_hint(head)
        .map(|line| format!("\n改法：{}", line))
        .unwrap_or_else(|| "\n改法：请改用对应的 POSIX 命令（ls/cat/grep/rm 等）。".to_string());
    format!(
        "未执行：检测到 PowerShell 语法 `{}`，但当前执行环境是 Git Bash（POSIX）。{hint}\n\
         确需执行 PowerShell 命令时，请显式写成 powershell -Command \"...\"。",
        head
    )
}

#[cfg(test)]
mod dialect_guard_tests {
    use super::detect_foreign_shell_syntax;
    use crate::infra::shell_dialect::ShellDialect;
    use std::path::PathBuf;

    fn bash() -> ShellDialect {
        ShellDialect::Bash {
            program: PathBuf::from("bash"),
            git_bash: true,
        }
    }

    fn powershell() -> ShellDialect {
        ShellDialect::PowerShell
    }

    #[test]
    fn blocks_powershell_cmdlets_in_bash() {
        for cmd in [
            "Get-ChildItem src",
            "Remove-Item -Recurse node_modules",
            "Get-Content a.txt > b.txt",
            "echo hi | Select-String foo",
            "FOO=1 Get-ChildItem",
            "env FOO=1 Remove-Item x",
        ] {
            assert!(
                detect_foreign_shell_syntax(&bash(), cmd).is_some(),
                "应拦下 PowerShell 语法：{}",
                cmd
            );
        }
    }

    #[test]
    fn allows_real_posix_commands_and_quoted_text() {
        for cmd in [
            "ls -la",
            "git commit -m \"Get-ChildItem\"",
            "npm run build",
            "./Get-Foo.exe --help",
            "powershell -Command \"Get-ChildItem\"",
            "grep -r TODO src",
            "sort file.txt",
        ] {
            assert!(
                detect_foreign_shell_syntax(&bash(), cmd).is_none(),
                "不该误伤：{}",
                cmd
            );
        }
    }

    /// PowerShell 方言下不做这项检查：`Get-ChildItem` 本来就是合法命令。
    #[test]
    fn powershell_dialect_is_not_checked() {
        assert!(detect_foreign_shell_syntax(&powershell(), "Get-ChildItem src").is_none());
    }

    /// 提示里必须给出可执行的改法，而不是只说"语法错误"。
    #[test]
    fn message_contains_actionable_hint() {
        let msg = detect_foreign_shell_syntax(&bash(), "Get-ChildItem src").unwrap();
        assert!(msg.contains("ls"), "提示应给出 ls 改法: {}", msg);
        assert!(
            msg.contains("powershell -Command"),
            "提示应给出显式调用 PowerShell 的退路: {}",
            msg
        );
    }
}
