//! # mod.rs — 聚合并导出 Agent 文件工具
//!
//! 组织文件读取、写入、编辑、搜索、目录列表和工具注册子模块，统一暴露给 `core::tools` 的上层路由。
//!
//! ## Key Exports
//! - `read_file()`: 读取文件内容（支持行号范围，超大文件自动截断）
//! - `read_file_skeleton()`: 提取文件结构骨架（函数/类/import 签名）
//! - `write_file()`: 写入文件（自动备份 + 快照 + TOCTOU 防护）
//! - `edit_file()`: 基于搜索替换编辑文件（唯一性检查 + 引号归一化）
//! - `search_repo()`: 在目录下搜索关键词（支持正则）
//! - `search_in_dir()`: 递归搜索关键词
//! - `list_directory()`: 列出目录内容
//! - `generate_repo_map()`: 生成仓库目录树
//! - `register_tools()`: 注册文件工具 schema 与路由元数据

mod common;
mod diff;
mod directory;
mod edit;
mod notebook_guard;
mod read;
mod registry;
mod search;
pub mod workspace;
mod write;

pub use directory::{generate_repo_map, list_directory};
pub use edit::edit_file;
pub use read::{read_file, read_file_skeleton};
pub use registry::register_tools;
pub use search::{search_in_dir, search_repo};
pub use workspace::{commit_pending_snapshot, has_pending_patches};
pub use write::write_file;
