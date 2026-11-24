//! # shell.rs — Shell 执行方言设置命令
//!
//! 设置页读写"用哪个 shell 执行命令"。两条命令都只做配置读写与校验，
//! 真正的生效点在进程启动（见 `infra::shell_dialect`）——保存后需要重启应用。
//!
//! **探测只在应用启动时发生一次**：这里读取的是启动期缓存的结果，
//! 打开设置页不会再跑 `git --exec-path` / `where git`。
//!
//! ## Key Exports
//! - `get_shell_settings()`: 读取当前设置与启动期探测缓存
//! - `save_shell_settings()`: 保存并返回展示状态
//!
//! ## Dependencies
//! - Internal: `command::app_config`（持久化）、`infra::shell_dialect`（解析与缓存）

use crate::infra::shell_dialect::{ShellResolution, ShellSettings};

/// 读取设置。`detectedBashPath` 来自启动期缓存，本命令不触发进程探测。
#[tauri::command]
pub fn get_shell_settings() -> ShellResolution {
    crate::infra::shell_dialect::resolution_without_detection(
        &crate::command::app_config::get_shell_settings(),
    )
}

/// 保存设置。自动模式永远可解析（探测不到 Git Bash 会回退 PowerShell），
/// 因此这里只需要落盘并返回展示状态；手填路径无效会在 `error` 里提示。
#[tauri::command]
pub fn save_shell_settings(settings: ShellSettings) -> Result<ShellResolution, String> {
    crate::command::app_config::save_shell_settings(settings.clone())?;
    Ok(crate::infra::shell_dialect::resolution_without_detection(
        &settings,
    ))
}
