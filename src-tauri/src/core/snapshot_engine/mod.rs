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
