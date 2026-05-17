//! # registry.rs — 工具注册表
//!
//! 全局工具注册表，为每个工具提供统一的元数据 + JSON Schema 定义。
//! 各工具模块通过 `define_tools!` 宏注册自己的工具，tool_search 和路由层从 registry 查询。
//!
//! ## 关键导出
//! - `ToolDef`: 工具定义结构体（名称、描述、搜索提示、Schema、是否延迟/只读/并发安全等）
//! - `ToolRegistry`: 全局注册表，支持按名称查找、核心/延迟工具过滤、意图筛选
//! - `define_tools!`: 注册宏，自动将 ToolDef 列表注册到 registry
//!
//! ## 约束
//! - 注册表通过 `OnceLock` 懒初始化，全局唯一
//! - 保持插入顺序用于稳定输出

use std::collections::HashMap;
use std::sync::OnceLock;

/// 工具定义：包含元数据和完整 JSON Schema
pub struct ToolDef {
    /// 工具唯一名称
    pub name: &'static str,
    /// 简述（用于完整 schema 描述 + SearchTools 搜索评分）
    pub description: &'static str,
    /// 搜索提示词（供 SearchTools 关键词匹配的补充短语）
    pub search_hint: &'static str,
    /// 完整 JSON Schema（符合 Anthropic tool_use 规范）
    pub schema: serde_json::Value,
    /// 工具分类（用于延迟工具列表分组展示）
    pub category: &'static str,
    /// 是否延迟加载（true = 需通过 SearchTools 获取后才能调用）
    pub should_defer: bool,
    /// 是否只读（read_only 子代理会过滤掉非只读工具）
    pub is_read_only: bool,
    /// 是否支持并发执行
    pub is_concurrency_safe: bool,
    /// 运行时是否启用
    pub is_enabled: bool,
}

/// 全局工具注册表（懒初始化）
static TOOL_REGISTRY: OnceLock<ToolRegistry> = OnceLock::new();

pub struct ToolRegistry {
    tools: HashMap<&'static str, ToolDef>,
    /// 保持插入顺序用于稳定输出
    insertion_order: Vec<&'static str>,
}

impl ToolRegistry {
    /// 获取全局注册表
    pub fn global() -> &'static ToolRegistry {
        TOOL_REGISTRY.get_or_init(|| {
            let mut registry = ToolRegistry {
                tools: HashMap::new(),
                insertion_order: Vec::new(),
            };
            // 各模块注册自己的工具（渐进迁移，已迁移的模块在此注册）
            crate::core::tools::task_tools::register_tools(&mut registry);
            crate::core::tools::file_tools::register_tools(&mut registry);
            crate::core::tools::notebook_tools::register_tools(&mut registry);
            crate::core::tools::search_tools::register_tools(&mut registry);
            crate::core::tools::shell_tools::register_tools(&mut registry);
            crate::core::tools::system_tools::register_tools(&mut registry);
            crate::core::tools::agent_tools::register_tools(&mut registry);
            super::tool_search::register_tools(&mut registry);
            registry
        })
    }

    /// 注册一个工具
    pub fn register(&mut self, tool: ToolDef) {
        if !self.tools.contains_key(tool.name) {
            self.insertion_order.push(tool.name);
        }
        self.tools.insert(tool.name, tool);
    }

    /// 按名称查找工具
    pub fn get(&self, name: &str) -> Option<&ToolDef> {
        self.tools.get(name)
    }

    /// 获取核心工具定义（should_defer == false && is_enabled）
    pub fn get_core_definitions(&self) -> Vec<serde_json::Value> {
        self.insertion_order
            .iter()
            .filter_map(|name| self.tools.get(name))
            .filter(|t| !t.should_defer && t.is_enabled)
            .map(|t| t.schema.clone())
            .collect()
    }

    /// 获取延迟工具列表 (name, description)，按意图筛选
    pub fn get_deferred_list(&self, intent: &str) -> Vec<(&'static str, &'static str)> {
        self.insertion_order
            .iter()
            .filter_map(|name| self.tools.get(name))
            .filter(|t| t.should_defer && t.is_enabled)
            .filter(|t| Self::is_available_for_intent(t, intent))
            .map(|t| (t.name, t.description))
            .collect()
    }

    /// 获取延迟工具搜索索引 (name, description, search_hint)，按意图筛选
    pub fn get_deferred_search_entries(
        &self,
        intent: &str,
    ) -> Vec<(&'static str, &'static str, &'static str)> {
        self.insertion_order
            .iter()
            .filter_map(|name| self.tools.get(name))
            .filter(|t| t.should_defer && t.is_enabled)
            .filter(|t| Self::is_available_for_intent(t, intent))
            .map(|t| (t.name, t.description, t.search_hint))
            .collect()
    }

    /// 获取延迟工具的完整 Schema
    pub fn get_deferred_full_schema(&self, name: &str) -> Option<serde_json::Value> {
        self.tools
            .get(name)
            .filter(|t| t.should_defer && t.is_enabled)
            .map(|t| t.schema.clone())
    }

    /// 获取所有延迟工具的名称列表（用于 search 时的全量展示）
    pub fn get_all_deferred_names(&self, intent: &str) -> Vec<&'static str> {
        self.insertion_order
            .iter()
            .filter_map(|name| self.tools.get(name))
            .filter(|t| t.should_defer && t.is_enabled)
            .filter(|t| Self::is_available_for_intent(t, intent))
            .map(|t| t.name)
            .collect()
    }

    /// 获取延迟工具分组（按 category），保持插入顺序
    pub fn get_deferred_by_category(
        &self,
        intent: &str,
    ) -> Vec<(&'static str, Vec<&'static str>)> {
        let mut groups: Vec<(&'static str, Vec<&'static str>)> = Vec::new();
        for name in &self.insertion_order {
            if let Some(t) = self.tools.get(name) {
                if t.should_defer && t.is_enabled && Self::is_available_for_intent(t, intent) {
                    let cat = if t.category.is_empty() { "其他" } else { t.category };
                    if let Some((_, names)) = groups.iter_mut().find(|(c, _)| *c == cat) {
                        names.push(t.name);
                    } else {
                        groups.push((cat, vec![t.name]));
                    }
                }
            }
        }
        groups
    }

    /// 获取可写工具名列表（供子代理 read_only 模式过滤）
    pub fn get_writable_tools(&self) -> Vec<&'static str> {
        self.insertion_order
            .iter()
            .filter_map(|name| self.tools.get(name))
            .filter(|t| !t.is_read_only && t.is_enabled)
            .map(|t| t.name)
            .collect()
    }

    /// 按意图过滤工具可用性
    fn is_available_for_intent(tool: &ToolDef, intent: &str) -> bool {
        match intent {
            "CHAT" | "QUESTION" => false,
            "SUBAGENT" => {
                // 子代理只能执行具体操作，不能调用主控/调度/会话管理工具
                !matches!(
                    tool.name,
                    // 子代理控制
                    "RunSubagent"
                        | "RunSubagentsSequentially"
                        // 任务编排（主Agent 管理）
                        | "CreateTask"
                        | "UpdateTask"
                        | "DeleteTask"
                        | "ListTasks"
                        | "GetTask"
                        | "SummarizeTasks"
                        // 会话管理
                        | "SwitchWorkMode"
                        | "UpdateTodos"
                        | "CompactConversation"
                        | "ConsolidateMemory"
                        // 后台服务（主Agent 统一管理）
                        | "StartBackgroundCommand"
                        | "CheckBackgroundCommand"
                        // 工作区设置
                        | "SetWorkspace"
                )
            }
            _ => true, // PROJECT_ACTION
        }
    }
}

/// 注册宏：自动将 ToolDef 列表注册到 registry
///
/// 用法：
/// ```rust,ignore
/// crate::define_tools! {
///     pub fn register_tools(registry) {
///         ToolDef { name: "CreateTask", ... },
///         ToolDef { name: "UpdateTask", ... },
///     }
/// }
/// ```
#[macro_export]
macro_rules! tool_def {
    (
        $name:expr,
        desc: $desc:expr,
        hint: $hint:expr,
        schema_desc: $schema_desc:expr,
        $( props: {
            $( $prop_name:ident : $prop_type:ident $(enum expr $enum_expr:expr)? => $prop_desc:expr ),* $(,)?
        }, )?
        $( required: $required:expr, )?
        $( category: $category:expr, )?
        $( defer: $defer:expr, )?
        $( read_only: $read_only:expr, )?
        $( concurrency_safe: $concurrency_safe:expr $(,)? )?
    ) => {{
        #![allow(unused_mut, unused_assignments)]
        let mut props_map = serde_json::Map::new();
        $(
            $(
                let mut prop_schema = serde_json::json!({
                    "type": stringify!($prop_type),
                    "description": $prop_desc,
                });
                $(
                    prop_schema.as_object_mut().unwrap().insert("enum".to_string(), serde_json::json!($enum_expr));
                )?
                props_map.insert(stringify!($prop_name).to_string(), prop_schema);
            )*
        )?
        let mut required_fields = serde_json::json!([]);
        $( required_fields = serde_json::json!($required); )?
        let schema = serde_json::json!({
            "name": $name,
            "description": $schema_desc,
            "input_schema": {
                "type": "object",
                "properties": props_map,
                "required": required_fields
            }
        });

        let mut should_defer = false;
        $( should_defer = $defer; )?
        let mut is_read_only = false;
        $( is_read_only = $read_only; )?
        let mut is_concurrency_safe = false;
        $( is_concurrency_safe = $concurrency_safe; )?
        let mut category = "";
        $( category = $category; )?

        $crate::core::tools::framework::registry::ToolDef {
            name: $name,
            description: $desc,
            search_hint: $hint,
            category,
            schema,
            should_defer,
            is_read_only,
            is_concurrency_safe,
            is_enabled: true,
        }
    }};
}

// 定义工具注册函数
#[macro_export]
macro_rules! define_tools {
    (pub fn register_tools($registry:ident) { $($tool:expr),* $(,)? }) => {
        pub fn register_tools($registry: &mut $crate::core::tools::framework::registry::ToolRegistry) {
            $($registry.register($tool);)*
        }
    };
}
