use crate::core::tools::registry::ToolRegistry;

pub fn register_tools(registry: &mut ToolRegistry) {
    registry.register(super::todo_write::tool_def());
    registry.register(super::persistent::task_create_tool_def());
    registry.register(super::persistent::task_update_tool_def());
    registry.register(super::persistent::task_delete_tool_def());
    registry.register(super::persistent::task_list_tool_def());
    registry.register(super::persistent::task_summary_tool_def());
    registry.register(super::persistent::task_get_tool_def());
}
