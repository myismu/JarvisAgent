//! # main.rs — Tauri 应用入口点
//!
//! 这是 JarvisAgent 桌面应用的主入口文件。负责启动 Tauri 运行时，
//! 在 Windows release 模式下隐藏控制台窗口。
//!
//! ## 关键导出
//! - `main()`: 应用启动函数，调用 `jarvisagent_lib::run()` 初始化应用
//!
//! ## 依赖
//! - Internal: `jarvisagent_lib::run`
//! - External: `tauri` (隐式通过 lib.rs)
//!
//! ## 约束
//! - Windows release 模式下必须隐藏控制台窗口（`windows_subsystem = "windows"`）
//! - 不要删除 `cfg_attr` 属性，否则 Windows 用户会看到额外的控制台窗口

// Prevents additional console window on Windows in release, DO NOT REMOVE!!
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

fn main() {
    jarvisagent_lib::run()
}
