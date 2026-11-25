//! # shell_dialect.rs — Shell 执行方言
//!
//! 收敛"用哪个 shell 解释命令"这件事。此前这条判断散落在执行层、只读判定层、
//! 提示词层，全部写成 `cfg!(target_os = "windows")`——操作系统和 shell 语义被绑死。
//! 引入本模块后，**实际方言由"自动探测 + 可覆盖设置"决定**：
//!
//! - Windows + `Auto`（默认）：探测到 Git Bash 就用它（`bash.exe -c`），
//!   与 Claude Code 的 Bash 工具同一边界（POSIX 语义、HOME/PATH 由 bash 自行摆正）；
//!   探测不到就用 `powershell -NoProfile -Command`；
//! - Windows + `PowerShell`：不探测，固定使用 PowerShell（逃生阀）；
//! - 非 Windows：恒为系统 `bash`。
//!
//! 默认 `Auto` 是刻意与 Claude Code 对齐的：装没装 Git for Windows 是环境事实，
//! 不该让用户再去设置里声明一次。
//!
//! ## 生效时机
//! 方言在进程启动时解析一次并写入 `OnceLock`。核心工具的 schema 进请求体后必须
//! 整个会话内字节恒定（否则 prompt cache 前缀整体失效），所以**改设置需要重启应用**，
//! 不支持会话内热切换。这与工具开关"只对新会话生效"是两套不同的边界，不要混用。
//!
//! ## Key Exports
//! - `ShellMode` / `ShellSettings`：持久化到 `app-config.json` 的用户设置
//! - `ShellDialect`：解析后的执行方言
//! - `init()` / `current()`：启动期初始化与全局只读访问
//! - `resolve()`：纯函数，供设置命令校验与预览
//!
//! ## Dependencies
//! - External: `serde`, `std::process`

use serde::{Deserialize, Serialize};
use std::path::PathBuf;
use std::sync::OnceLock;

/// 用户可选的 Shell 模式（只有 Windows 有意义；非 Windows 恒为 POSIX bash）
///
/// 只有两档：自动与 PowerShell。
/// 不提供"手动选 Git Bash"——它与"自动 + 指定 bash 路径"行为一致，
/// 多一个选项只会让用户多理解一个概念。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[derive(Default, ts_rs::TS)]
pub enum ShellMode {
    /// 自动（默认）：探测到 Git Bash 就用 Git Bash，否则用 PowerShell。
    ///
    /// 与 Claude Code 的行为一致：用户装了 Git for Windows 就直接享受 POSIX 语义，
    /// 不需要手动切换。
    #[serde(rename = "auto")]
    #[default]
    Auto,
    /// 手动选择 Windows PowerShell：不再探测，固定使用 PowerShell。
    ///
    /// 显式写 `rename`：`rename_all = "snake_case"` 会把 `PowerShell`
    /// 拆成 `power_shell`，与前端约定的 `powershell` 不一致。
    #[serde(rename = "powershell")]
    PowerShell,
}


/// `app-config.json` 中的 `shell` 字段
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, ts_rs::TS)]
#[serde(rename_all = "camelCase")]
#[ts(rename_all = "camelCase")]
pub struct ShellSettings {
    #[serde(default)]
    pub mode: ShellMode,
    /// 用户显式指定的 `bash.exe` 路径。留空表示自动探测。
    ///
    /// 为什么要可配：Git for Windows 允许装到任意目录（本机就在 `E:\StudyTools\Git`），
    /// 硬编码 `C:\Program Files\Git` 在真实环境里必然失效。
    #[serde(default)]
    pub git_bash_path: Option<String>,
}

/// 解析后的执行方言
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ShellDialect {
    /// Windows PowerShell 5.1
    PowerShell,
    /// POSIX bash。Windows 上是 Git Bash，Unix 上是系统 bash。
    Bash {
        /// 可执行文件路径（Windows 为 `bash.exe` 绝对路径，Unix 为 `bash`）
        program: PathBuf,
        /// 是否为 Git for Windows 的 bash（决定提示词与路径换算口径）
        git_bash: bool,
    },
}

impl ShellDialect {
    /// 是否为 POSIX/bash 语义
    pub fn is_bash(&self) -> bool {
        matches!(self, Self::Bash { .. })
    }

    /// 是否运行在 Windows 上的 Git Bash
    pub fn is_git_bash(&self) -> bool {
        matches!(self, Self::Bash { git_bash: true, .. })
    }

    /// 展示名（设置页与提示词共用）
    pub fn display_name(&self) -> &'static str {
        match self {
            Self::PowerShell => "PowerShell",
            Self::Bash { .. } => "Git Bash",
        }
    }
}

/// 进程级方言。启动时由 [`init`] 写入；未初始化（单测、早期调用）时按操作系统兜底。
static CURRENT: OnceLock<ShellDialect> = OnceLock::new();

/// 启动时探测到的 Git Bash 路径（仅 Auto 模式且探测成功时有值）。
///
/// 设置页只读这份缓存，不再每次打开页面都跑 `git --exec-path` / `where git`。
/// 用户换电脑或重装 Git 后需要重启应用才能刷新——这与“方言在启动时定死”的
/// 生效边界一致，不是额外的限制。
static DETECTED_BASH: OnceLock<Option<PathBuf>> = OnceLock::new();

/// 启动期初始化。
///
/// 解析永远成功：自动模式探测不到 Git Bash 会退回 PowerShell，这是正常状态，
/// 不是错误。因此这里没有失败分支，也不会因为环境缺东西而影响应用启动。
///
/// **探测只在这里发生一次**。设置页读取的是缓存的 [`cached_detected_bash`]，
/// 不再触发进程调用，避免每次打开设置页卡几秒。
pub fn init(settings: &ShellSettings) {
    let dialect = resolve(settings);
    let detected = match &dialect {
        ShellDialect::Bash {
            program,
            git_bash: true,
        } => Some(program.clone()),
        _ => None,
    };
    let _ = DETECTED_BASH.set(detected);
    let _ = CURRENT.set(dialect);
}

/// 启动时缓存的 Git Bash 探测结果（未初始化或无结果时为 None）。
pub fn cached_detected_bash() -> Option<PathBuf> {
    DETECTED_BASH.get().cloned().flatten()
}

/// 当前方言。未初始化时按操作系统兜底（Windows→PowerShell，其他→bash）。
pub fn current() -> ShellDialect {
    CURRENT.get().cloned().unwrap_or_else(default_dialect)
}

fn default_dialect() -> ShellDialect {
    if cfg!(windows) {
        ShellDialect::PowerShell
    } else {
        ShellDialect::Bash {
            program: PathBuf::from("bash"),
            git_bash: false,
        }
    }
}

/// 纯解析函数：设置 → 方言。启动初始化与设置页状态展示都走这里。
///
/// 永不失败：自动模式下探测不到 Git Bash 就退回 PowerShell。
/// 探测失败是常见状态（用户没装 Git for Windows），不该被当成错误。
pub fn resolve(settings: &ShellSettings) -> ShellDialect {
    if !cfg!(windows) {
        return ShellDialect::Bash {
            program: PathBuf::from("bash"),
            git_bash: false,
        };
    }
    match settings.mode {
        // 自动：探测到就用 Git Bash，探测不到安静地退回 PowerShell。
        ShellMode::Auto => match detect_git_bash(settings.git_bash_path.as_deref()) {
            Some(program) => ShellDialect::Bash {
                program,
                git_bash: true,
            },
            None => ShellDialect::PowerShell,
        },
        ShellMode::PowerShell => ShellDialect::PowerShell,
    }
}

/// 探测 Git Bash 的 `bash.exe`。
///
/// 顺序（先准后全）：
/// 1. 用户显式配置的路径；
/// 2. `git --exec-path`：拿到的就是**当前会被调用的那个 git** 的 exec 目录，
///    从中向上找 `bin\bash.exe`，能覆盖自定义安装目录与便携版；
/// 3. `where.exe git` 的每一条结果，同样向上找；
/// 4. 常见安装位置（Program Files / 用户目录）。
///
/// 只认 `bin\bash.exe`：`usr\bin\bash.exe` 依赖 msys 运行时初始化，
/// 从非登录环境直接拉起时 `HOME` 可能不正确，正是本次要规避的问题。
pub fn detect_git_bash(explicit: Option<&str>) -> Option<PathBuf> {
    if let Some(path) = explicit.map(str::trim).filter(|p| !p.is_empty()) {
        let path = PathBuf::from(path);
        if path.is_file() {
            return Some(path);
        }
        return None;
    }
    if !cfg!(windows) {
        return None;
    }

    // 2. 从当前 git 的 exec-path 反推安装根目录
    if let Ok(out) = std::process::Command::new("git")
        .args(["--exec-path"])
        .output()
    {
        if out.status.success() {
            let exec_path = String::from_utf8_lossy(&out.stdout).trim().to_string();
            if let Some(bash) = find_bash_near(&PathBuf::from(exec_path)) {
                return Some(bash);
            }
        }
    }

    // 3. PATH 里的所有 git.exe
    if let Ok(out) = std::process::Command::new("where.exe").arg("git").output() {
        if out.status.success() {
            for line in String::from_utf8_lossy(&out.stdout).lines() {
                let line = line.trim();
                if line.is_empty() {
                    continue;
                }
                if let Some(bash) = find_bash_near(&PathBuf::from(line)) {
                    return Some(bash);
                }
            }
        }
    }

    // 4. 常见安装位置
    let mut candidates: Vec<PathBuf> = Vec::new();
    if let Some(program_files) = std::env::var_os("ProgramFiles") {
        candidates.push(
            PathBuf::from(program_files)
                .join("Git")
                .join("bin")
                .join("bash.exe"),
        );
    }
    if let Some(program_files_x86) = std::env::var_os("ProgramFiles(x86)") {
        candidates.push(
            PathBuf::from(program_files_x86)
                .join("Git")
                .join("bin")
                .join("bash.exe"),
        );
    }
    if let Some(local_app_data) = std::env::var_os("LOCALAPPDATA") {
        candidates.push(
            PathBuf::from(local_app_data)
                .join("Programs")
                .join("Git")
                .join("bin")
                .join("bash.exe"),
        );
    }
    candidates.into_iter().find(|path| path.is_file())
}

/// 从给定路径（`git.exe` 或 `git-core` 目录）向上最多 4 层找安装根下的 `bin\bash.exe`。
fn find_bash_near(start: &std::path::Path) -> Option<PathBuf> {
    let mut cursor = if start.is_dir() {
        start.to_path_buf()
    } else {
        start.parent()?.to_path_buf()
    };
    for _ in 0..5 {
        let candidate = cursor.join("bin").join("bash.exe");
        if candidate.is_file() {
            return Some(candidate);
        }
        cursor = cursor.parent()?.to_path_buf();
    }
    None
}

/// 设置页展示用的解析结果（序列化给前端）
#[derive(Debug, Clone, Serialize, ts_rs::TS)]
#[serde(rename_all = "camelCase")]
#[ts(rename_all = "camelCase")]
pub struct ShellResolution {
    /// 当前平台是否支持切换（只有 Windows 有自动 / PowerShell 两种选择）
    pub platform_supported: bool,
    /// 用户选择的模式：`auto` / `powershell`
    pub mode: ShellMode,
    /// 用户显式配置的 bash 路径（可能为空）
    pub git_bash_path: Option<String>,
    /// 实际探测到的 bash.exe 路径（探测失败为 None）
    pub detected_bash_path: Option<String>,
    /// 需要提示给用户的问题（例如手填路径不存在）。正常状态为 None。
    pub error: Option<String>,
}

/// 把设置解析成前端可展示的状态，**不触发任何进程探测**。
///
/// 设置页每次打开都调用它，因此必须廉价：
/// - 显式路径只做 `is_file` 检查（本地文件系统调用，微秒级）；
/// - 自动模式且未填路径时，读取启动时缓存的探测结果；
/// - PowerShell 模式不展示检测路径。
pub fn resolution_without_detection(settings: &ShellSettings) -> ShellResolution {
    let platform_supported = cfg!(windows);
    let explicit = settings
        .git_bash_path
        .as_deref()
        .map(str::trim)
        .filter(|path| !path.is_empty());

    let (detected_bash_path, error) =
        if !platform_supported || settings.mode == ShellMode::PowerShell {
            (None, None)
        } else if let Some(path) = explicit {
        let path = PathBuf::from(path);
        if path.is_file() {
            (Some(path.display().to_string()), None)
        } else {
            // 显式路径写错不该静默忽略：用户以为指定了 bash，实际却退回了 PowerShell。
            (
                None,
                Some("指定的 bash.exe 路径不存在，自动模式将使用 PowerShell".to_string()),
            )
        }
    } else {
        (
            cached_detected_bash().map(|path| path.display().to_string()),
            None,
        )
    };

    ShellResolution {
        platform_supported,
        mode: settings.mode,
        git_bash_path: settings.git_bash_path.clone(),
        detected_bash_path,
        error,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 前后端约定的字符串必须与序列化结果一致。
    /// 这条守的是 `rename_all` 的坑：`PowerShell` 会被拆成 `power_shell`。
    #[test]
    fn mode_serializes_to_frontend_tokens() {
        assert_eq!(
            serde_json::to_string(&ShellMode::Auto).unwrap(),
            "\"auto\""
        );
        assert_eq!(
            serde_json::to_string(&ShellMode::PowerShell).unwrap(),
            "\"powershell\""
        );
        assert_eq!(ShellMode::default(), ShellMode::Auto);
    }

    /// 自动模式：探测到就用 Git Bash，探测不到退回 PowerShell，两种都合法。
    #[test]
    fn auto_mode_always_resolves() {
        let dialect = resolve(&ShellSettings {
            mode: ShellMode::Auto,
            git_bash_path: None,
        });
        // 环境相关：装了 Git for Windows 是 Bash，否则 PowerShell
        assert!(dialect.is_bash() || dialect == ShellDialect::PowerShell);
    }

    #[test]
    fn powershell_mode_resolves_on_windows() {
        if !cfg!(windows) {
            return;
        }
        let dialect = resolve(&ShellSettings {
            mode: ShellMode::PowerShell,
            git_bash_path: None,
        });
        assert_eq!(dialect, ShellDialect::PowerShell);
    }

    /// 手填路径不存在时不报错、回退 PowerShell，但设置页必须能看到提示。
    #[test]
    fn invalid_explicit_path_is_visible_but_not_fatal() {
        if !cfg!(windows) {
            return;
        }
        let settings = ShellSettings {
            mode: ShellMode::Auto,
            git_bash_path: Some("C:\\definitely\\missing\\bash.exe".to_string()),
        };
        assert_eq!(resolve(&settings), ShellDialect::PowerShell);
        let described = resolution_without_detection(&settings);
        let error = described.error.expect("应给出可见提示");
        assert!(error.contains("bash.exe"), "提示要点明缺的是什么: {}", error);
    }

    #[test]
    fn non_windows_always_bash() {
        if cfg!(windows) {
            return;
        }
        let dialect = resolve(&ShellSettings {
            mode: ShellMode::PowerShell,
            git_bash_path: None,
        });
        assert!(dialect.is_bash());
        assert!(!dialect.is_git_bash());
    }
}
