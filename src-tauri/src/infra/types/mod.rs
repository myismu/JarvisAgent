//! # infra/types/mod.rs — 领域原语模块
//!
//! 数据模型、错误类型、trait 定义和常量的集合。
//! 这些是整个系统的"语言"，被所有上层模块依赖。

pub mod constants;
pub mod error;
pub mod models;
pub mod traits;
