fn main() {
    tauri_build::build();

    // tauri-build 仅通过 cargo:rustc-link-arg-bins 为主程序链接 resource.lib（含 comctl32 v6 清单）；
    // lib 单元测试可执行文件未链接该清单，在 Windows 上引用 comctl32 v6 符号时会触发
    // 0xc0000139 (STATUS_ENTRYPOINT_NOT_FOUND)。
    // 若对全目标重复传 resource.lib 会导致 bin 目标出现 CVT1100 (duplicate resource)，
    // 改为仅在 MSVC 下注入 Common-Controls v6 清单依赖即可同时兼容 bin 与 lib test。
    if std::env::var("CARGO_CFG_TARGET_ENV").as_deref() == Ok("msvc") {
        println!(
            "cargo:rustc-link-arg=/MANIFESTDEPENDENCY:type='win32' name='Microsoft.Windows.Common-Controls' version='6.0.0.0' processorArchitecture='*' publicKeyToken='6595b64144ccf1df' language='*'"
        );
    }
}
