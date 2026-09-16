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
//! - 写操作工具（WriteFile, EditFile）设为延迟工具，防止聊天模式下误操作
//! - 只读保护模式（work_mode = chat）下按元数据过滤工具目录：只放行只读工具 + 会话管理工具

use std::collections::HashMap;
use std::sync::OnceLock;

/// 工具定义：包含元数据和完整 JSON Schema
pub struct ToolDef {
    /// 工具唯一名称
    pub name: &'static str,
    /// 简述（用于完整 schema 描述 + DiscoverTools 搜索评分）
    pub description: &'static str,
    /// 搜索提示词（供 DiscoverTools 关键词匹配的补充短语）
    pub search_hint: &'static str,
    /// 完整 JSON Schema（符合 Anthropic tool_use 规范）
    pub schema: serde_json::Value,
    /// 工具分类（用于延迟工具列表分组展示）
    pub category: &'static str,
    /// 是否延迟加载（true = 需通过 DiscoverTools 获取后才能调用）
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

    /// 所有已注册且启用的工具名（保持插入顺序）。
    ///
    /// 用途：权限策略表要做"防漏"检查——新工具没登记分类时让测试直接失败。
    pub fn all_tool_names(&self) -> Vec<&'static str> {
        self.insertion_order
            .iter()
            .filter_map(|name| self.tools.get(name))
            .filter(|t| t.is_enabled)
            .map(|t| t.name)
            .collect()
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

    /// 获取延迟工具列表 (name, description)，按意图 + 工作模式筛选
    pub fn get_deferred_list(
        &self,
        intent: &str,
        work_mode: &str,
    ) -> Vec<(&'static str, &'static str)> {
        self.insertion_order
            .iter()
            .filter_map(|name| self.tools.get(name))
            .filter(|t| t.should_defer && t.is_enabled)
            .filter(|t| Self::is_available(t, intent, work_mode))
            .map(|t| (t.name, t.description))
            .collect()
    }

    /// 获取延迟工具搜索索引 (name, description, search_hint)，按意图 + 工作模式筛选
    pub fn get_deferred_search_entries(
        &self,
        intent: &str,
        work_mode: &str,
    ) -> Vec<(&'static str, &'static str, &'static str)> {
        self.insertion_order
            .iter()
            .filter_map(|name| self.tools.get(name))
            .filter(|t| t.should_defer && t.is_enabled)
            .filter(|t| Self::is_available(t, intent, work_mode))
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
    pub fn get_all_deferred_names(&self, intent: &str, work_mode: &str) -> Vec<&'static str> {
        self.insertion_order
            .iter()
            .filter_map(|name| self.tools.get(name))
            .filter(|t| t.should_defer && t.is_enabled)
            .filter(|t| Self::is_available(t, intent, work_mode))
            .map(|t| t.name)
            .collect()
    }

    /// 获取延迟工具分组（按 category），保持插入顺序，按意图 + 工作模式筛选
    pub fn get_deferred_by_category(
        &self,
        intent: &str,
        work_mode: &str,
    ) -> Vec<(&'static str, Vec<&'static str>)> {
        let mut groups: Vec<(&'static str, Vec<&'static str>)> = Vec::new();
        for name in &self.insertion_order {
            if let Some(t) = self.tools.get(name) {
                if t.should_defer && t.is_enabled && Self::is_available(t, intent, work_mode) {
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
    pub fn is_available_for_intent(tool: &ToolDef, intent: &str) -> bool {
        match intent {
            "CHAT" => false,
            "QUESTION" => {
                // 记忆查询只允许文件读取和会话管理
                matches!(
                    tool.name,
                    "ReadFile" | "CompactConversation" | "ConsolidateMemory"
                )
            }
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
                )
            }
            _ => true, // PROJECT_ACTION
        }
    }

    /// 会改变文件系统/进程状态的工具（唯一一份定义，`tools::is_write_tool` 也复用这里）。
    ///
    /// 这份名单以前只存在于 tools/mod.rs 的运行期兜底里，导致"工具目录"与"运行期拦截"
    /// 是两套判据：规划模式下目录里看得见 WriteFile，调用时才被拦。现在统一到注册表，
    /// 目录过滤 / 搜索 / 执行 / 能力清单都由它推导。
    pub const WRITE_TOOLS: &'static [&'static str] = &[
        "WriteFile",
        "EditFile",
        "DeleteFile",
        "RenameFile",
        "ApplyPatch",
        "RunCommand",
        "StartBackgroundCommand",
        "EditNotebook",
    ];

    /// 是否为写操作工具（按唯一名单判定）
    pub fn is_write_tool_name(name: &str) -> bool {
        Self::WRITE_TOOLS.contains(&name)
    }

    /// 规划模式下**额外**不可用的工具（`WRITE_TOOLS` 之外那部分）。
    ///
    /// 为什么不能只拦 `WRITE_TOOLS`：规划模式的语义是"只探索、提方案"，但下面这两个
    /// 工具都绕得过那份名单，等于给模型留了改文件的暗道——
    /// - `RunSubagent`：子代理内层固定以 `edit` 模式运行，写工具对它全量可见；
    /// - `RunSubagentsSequentially`：调度器派子代理时 `read_only` 参数恒为 `false`。
    ///
    /// 注意 `RunSubagent` 的 `read_only` 是**模型可控入参**：光靠"默认只读"拦不住，
    /// 必须在这一层把工具整个收走。
    /// （原第三项 `SetWorkspace` 已随工具退役移出，见 `system_tools/mod.rs` 模块注释。）
    pub const PLAN_BLOCKED_EXTRA: &'static [&'static str] = &[
        "RunSubagent",
        "RunSubagentsSequentially",
    ];

    /// 规划模式下这个工具名是否不可用（写工具 + [`Self::PLAN_BLOCKED_EXTRA`]）。
    ///
    /// **唯一口径**：工具目录过滤（[`Self::is_available`]）与运行期兜底
    /// （`tools::should_block_write_tool`）都调用这里，避免"目录里看不见、调用时又放行"。
    pub fn is_blocked_in_plan_mode(name: &str) -> bool {
        Self::is_write_tool_name(name) || Self::PLAN_BLOCKED_EXTRA.contains(&name)
    }

    /// 工具可用性判定（意图 + 工作模式）。
    ///
    /// 这是工具目录过滤与运行时校验的**唯一口径**：
    /// 目录里看不见的工具，调用时也一定被拦下。
    pub fn is_available(tool: &ToolDef, intent: &str, work_mode: &str) -> bool {
        if !Self::is_available_for_intent(tool, intent) {
            return false;
        }
        // 规划模式：只探索 + 提交方案，写操作必须切回编辑模式
        if work_mode == "plan" {
            return !Self::is_blocked_in_plan_mode(tool.name);
        }
        true
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
            $( $prop_name:ident : $prop_type:ident $(items $items_tt:tt)? $(enum expr $enum_expr:expr)? => $prop_desc:expr ),* $(,)?
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
                $(
                    prop_schema.as_object_mut().unwrap().insert("items".to_string(), serde_json::json!($items_tt));
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
