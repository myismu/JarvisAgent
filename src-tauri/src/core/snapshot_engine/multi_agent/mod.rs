//! 多代理协作模块
//!
//! 支持多个代理并行工作的隔离与合并机制：
//! - `sandbox`: 沙箱管理（为每个代理创建独立的工作区）
//! - `merge`: 分支合并（冲突检测与解决）

pub mod merge;
pub mod sandbox;

pub use merge::{Conflict, ConflictResolution, ConflictType, MergeEngine, MergeResult};
pub use sandbox::{AgentSandbox, SandboxComparison, SandboxManager, SandboxStatus};
