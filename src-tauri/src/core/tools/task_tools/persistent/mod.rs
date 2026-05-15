mod batch_create;
mod common;
mod create;
mod delete;
mod get;
mod list;
mod summary;
mod update;

use crate::core::tools::framework::registry::ToolDef;

pub use batch_create::task_batch_create;
pub use create::task_create;
pub use delete::task_delete;
pub use get::task_get;
pub use list::task_list;
pub use summary::task_summary;
pub use update::task_update;

pub(super) fn task_create_tool_def() -> ToolDef {
    create::tool_def()
}

pub(super) fn task_update_tool_def() -> ToolDef {
    update::tool_def()
}

pub(super) fn task_delete_tool_def() -> ToolDef {
    delete::tool_def()
}

pub(super) fn task_list_tool_def() -> ToolDef {
    list::tool_def()
}

pub(super) fn task_summary_tool_def() -> ToolDef {
    summary::tool_def()
}

pub(super) fn task_get_tool_def() -> ToolDef {
    get::tool_def()
}

pub(super) fn task_batch_create_tool_def() -> ToolDef {
    batch_create::tool_def()
}
