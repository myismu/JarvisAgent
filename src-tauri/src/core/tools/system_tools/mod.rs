//! # system_tools.rs — 系统信息工具模块
//!
//! ## 关键导出
//! - （GetSystemInfo / SetWorkspace 均已移除）
//!
//! ## 约束
//! - 工作区语义：会话挂了项目 = 沙箱会话（`ctx.workspace` 即边界）；不挂项目 = 非沙箱，无边界。
//!   "换工作区"通过 UI 的"打开项目 / 切换会话"表达，不再是模型可调用的工具——
//!   SetWorkspace 曾同时承担"改进程 CWD"与"被误当沙箱边界"两份语义，悬空且误导，已退役。

// --- 工具注册 ---
crate::define_tools! {
    pub fn register_tools(_registry) {
        // 目前系统类没有模型可调用的工具：
        // - GetSystemInfo：OS/CWD/Home 已自动注入提示词，无需手动调用
        // - SetWorkspace：与项目绑定/沙箱边界语义冲突，已退役（见模块注释）
    }
}
