//! 快照引擎模块
//!
//! 提供文件级别的版本控制能力，核心组件包括：
//! - `patch`: 补丁系统（文件增删改查的差异表示）
//! - `snapshot`: 快照树（支持分支、检查点、版本链）
//! - `replay`: 重放引擎（从快照重建工作区状态）
//! - `journal`: 操作日志（持久化记录所有快照操作）
//! - `gc`: 垃圾回收（清理过期快照和孤立分支）
//! - `multi_agent`: 多代理协作（沙箱隔离与分支合并）

pub mod patch;
pub mod snapshot;
pub mod replay;
pub mod journal;
pub mod gc;
pub mod multi_agent;

pub use patch::{Patch, PatchError, PatchSummary, TextDiff, DiffHunk, DiffLine};
pub use snapshot::{Snapshot, SnapshotTree, SnapshotNode, SnapshotTreeView, SnapshotSummary, Workspace, WorkspaceState, FileInfo, Branch};
pub use replay::{ReplayEngine, AtomicFileRollback, UndoEntry, UndoAction};
pub use journal::{Journal, JournalEntry};
pub use gc::{GarbageCollector, GcConfig, GcResult};
pub use multi_agent::{AgentSandbox, SandboxManager, SandboxStatus, SandboxComparison, MergeEngine, MergeResult, Conflict, ConflictResolution, ConflictType};
