//! # shell.rs — Shell 执行方言设置命令
//!
//! 设置页读写"用哪个 shell 执行命令"。两条命令都只做配置读写与校验，
//! 真正的生效点在进程启动（见 `infra::shell_dialect`）——保存后需要重启应用。
//!
//! ## Key Exports
//! - `get_shell_settings()`: 读取当前设置并附带探测结果
//! - `save_shell_settings()`: 保存并返回当前解析状态
//!
//! ## Dependencies
//! - Internal: `command::app_config`（持久化）、`infra::shell_dialect`（解析与探测）

use crate::infra::shell_dialect::{ShellResolution, ShellSettings};

/// 读取设置。`detectedBashPath` 用于设置页展示"自动探测到了哪个 bash"，
/// `error` 表示当前配置无法解析（例如 bash.exe 已被卸载）。
#[tauri::command]
pub fn get_shell_settings() -> ShellResolution {
    crate::infra::shell_dialect::describe(&crate::command::app_config::get_shell_settings())
}

/// 保存设置。自动模式永远可解析（探测不到 Git Bash 会回退 PowerShell），
/// 因此这里只需要落盘并返回展示状态；手填路径无效会在 `error` 里提示。
#[tauri::command]
pub fn save_shell_settings(settings: ShellSettings) -> Result<ShellResolution, String> {
    crate::command::app_config::save_shell_settings(settings.clone())?;
    Ok(crate::infra::shell_dialect::describe(&settings))
}
