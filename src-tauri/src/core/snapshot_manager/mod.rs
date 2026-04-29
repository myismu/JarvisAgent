//! 快照管理器
//!
//! 提供会话级的快照生命周期管理：
//! - `SessionManager`: 单会话的快照树、日志、沙箱、分支合并
//! - `SessionManagerRegistry`: 多会话管理器注册表
//! - `SnapshotStore`: 文件系统持久化存储

pub mod session_manager;
pub mod store;

pub use session_manager::{SessionManager, SessionManagerRegistry};
pub use store::SnapshotStore;
