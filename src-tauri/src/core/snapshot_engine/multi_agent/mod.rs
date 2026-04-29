//! 多代理协作模块
//!
//! 支持多个代理并行工作的隔离与合并机制：
//! - `sandbox`: 沙箱管理（为每个代理创建独立的工作区）
//! - `merge`: 分支合并（冲突检测与解决）

pub mod sandbox;
pub mod merge;

pub use sandbox::{AgentSandbox, SandboxManager, SandboxStatus, SandboxComparison};
pub use merge::{MergeEngine, MergeResult, Conflict, ConflictResolution, ConflictType};
