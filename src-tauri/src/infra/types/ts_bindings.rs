//! # ts_bindings.rs — Rust → TypeScript 类型契约守卫（ts-rs 试点）
//!
//! 职责：将核心跨端 Rust 结构体集中导出为前端 `src/types/generated/contracts.ts`，
//! 并在 `cargo test` 中校验磁盘文件与 Rust 定义逐字节一致。
//!
//! ## 工作方式
//! - **平时（CI / 本地 `cargo test`）**：纯只读比对。若 Rust 改了字段却忘同步 TS，测试直接红灯拦截。
//! - **需要刷新时**：在 `src-tauri/` 下执行：
//!   `UPDATE_TS_BINDINGS=1 cargo test ts_contracts_are_up_to_date`
//!   （PowerShell：`$env:UPDATE_TS_BINDINGS="1"; cargo test ts_contracts_are_up_to_date`）
//!
//! ## 为什么集中生成单文件而不是散落多文件
//! 避免多个生成文件之间产生交叉相对导入；前端只需从 `@/types/generated/contracts`
//! 统一引入，所有导出类型在同一文件内自然互相引用。

use std::path::PathBuf;
use ts_rs::TS;

use crate::core::session::{SessionUsageBreakdownView, SessionUsageView, UsageSourceView};
use crate::infra::shell_dialect::{ShellMode, ShellResolution, ShellSettings};

/// 统一渲染所有纳入契约守卫的 TypeScript 类型定义（固定 LF 换行）。
fn render_contracts_ts() -> String {
    let decls = [
        // 会话用量视图契约（core/session/mod.rs）
        UsageSourceView::decl(),
        SessionUsageBreakdownView::decl(),
        SessionUsageView::decl(),
        // Shell 方言设置与探测状态契约（infra/shell_dialect.rs）
        ShellMode::decl(),
        ShellSettings::decl(),
        ShellResolution::decl(),
    ];

    let mut out = String::from(
        "// 本文件由 Rust 端 ts-rs 自动生成，请勿手工修改。\n\
         // 若 Rust 结构体字段有变，请在 src-tauri/ 下运行：\n\
         //   UPDATE_TS_BINDINGS=1 cargo test ts_contracts_are_up_to_date\n\n",
    );

    for decl in decls {
        // ts-rs 的 decl() 返回 `type Foo = ...;`，补上 `export ` 供前端直接导入
        let exported = if decl.starts_with("export ") {
            decl
        } else {
            format!("export {}", decl)
        };
        let cleaned = exported
            .replace("\r\n", "\n")
            .lines()
            .map(str::trim_end)
            .collect::<Vec<_>>()
            .join("\n");
        out.push_str(&cleaned);
        out.push_str("\n\n");
    }

    // 保留单一末尾换行
    while out.ends_with("\n\n") {
        out.pop();
    }
    out
}

fn contracts_file_path() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("..")
        .join("src")
        .join("types")
        .join("generated")
        .join("contracts.ts")
}

#[test]
fn ts_contracts_are_up_to_date() {
    let expected = render_contracts_ts();
    let target = contracts_file_path();

    if std::env::var("UPDATE_TS_BINDINGS").as_deref() == Ok("1") {
        if let Some(parent) = target.parent() {
            std::fs::create_dir_all(parent).expect("创建 src/types/generated 目录失败");
        }
        std::fs::write(&target, expected.as_bytes()).expect("写入 contracts.ts 失败");
        return;
    }

    let actual = std::fs::read_to_string(&target).unwrap_or_else(|err| {
        panic!(
            "读取 {} 失败（{}）。请先运行 UPDATE_TS_BINDINGS=1 cargo test ts_contracts_are_up_to_date 生成契约文件。",
            target.display(),
            err
        )
    });

    assert_eq!(
        actual.replace("\r\n", "\n"),
        expected,
        "前端生成契约 {} 与 Rust 结构体定义已漂移！\n请运行 UPDATE_TS_BINDINGS=1 cargo test ts_contracts_are_up_to_date 同步。",
        target.display()
    );
}
