//! # 反思审查模块
//!
//! 独立审查 Agent，在主 Agent 工具执行后进行第三方判断。
//! 与 Plan（事前规划）互补，实现事后审查。
//!
//! ## 架构
//! - `prompt`: 审查者 system prompt
//! - `strategy`: 触发策略判断 + 审查执行逻辑
//!
//! ## 设计文档
//! - `doc/reflection-mechanism-design.md`: 原始设计方案
//! - `doc/reflection-architecture-tradeoff.md`: 方案 A vs 方案 B 权衡分析

pub mod prompt;
pub mod strategy;

/// 反思判断结果
#[derive(Debug)]
pub enum ReflectionJudgment {
    /// 工具结果符合预期，继续正常流程
    Ok,
    /// 工具结果不符合预期，需要修正
    NotOk { reason: String, suggestion: String },
}
