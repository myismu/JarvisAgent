//! # types.rs — Shell 工具基础类型
//!
//! 定义安全检查和执行等过程中的共享基础枚举类型。
//!
//! ## Key Exports
//! - SafetyResult: 安全检查结果枚举（Safe, Warn, Block）

#[derive(Debug, PartialEq)]
pub enum SafetyResult {
    Safe,
    Warn(String),
    Block(String),
}
