//! # task_tools
//!
//! Tool-first task modules with shared persistent-task helpers.
//!
//! ## Exports
//! - `todo_write()`: update the current session's lightweight checklist
//! - `task_create()`: create a persistent task
//! - `task_update()`: update task status or dependency metadata
//! - `task_delete()`: permanently delete a persistent task
//! - `task_list()`: list persistent tasks
//! - `task_get()`: fetch one persistent task
//! - `task_summary()`: generate a persistent task overview

mod persistent;
mod registry;
mod todo_write;

pub use persistent::{task_create, task_delete, task_get, task_list, task_summary, task_update};
pub use registry::register_tools;
pub use todo_write::todo_write;
