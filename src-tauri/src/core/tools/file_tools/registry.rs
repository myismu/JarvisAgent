//! # registry.rs — 注册 file_tools 提供给 Agent 的工具定义
//!
//! 通过 `define_tools!` 宏声明文件读取、写入、编辑、搜索和目录列表工具的 schema、搜索提示、只读属性与并发安全属性。
//!
//! ## Key Exports
//! - `register_tools()`: 向工具注册表加入 file_tools 的可调用工具定义
//!
//! ## Dependencies
//! - Internal: `crate::core::tools::framework::registry::ToolDef`, `crate::define_tools!`
//! - External: `serde_json`

use serde_json::json;

use crate::core::tools::framework::registry::ToolDef;

// --- 工具注册 ---
crate::define_tools! {
    pub fn register_tools(registry) {
        ToolDef {
            name: "ReadFile",
            description: "读取文件内容，支持按行号精确读取",
            search_hint: "read file content view",
            schema: json!({
                "name": "ReadFile",
                "description": "读取文件内容。支持语义化点读技术，可通过 start_line 和 end_line 获取特定代码块，避免 Context 过长。",
                "input_schema": {
                    "type": "object",
                    "properties": {
                        "path": {"type": "string"},
                        "start_line": {"type": "integer", "description": "可选。起始行号（从 1 开始）"},
                        "end_line": {"type": "integer", "description": "可选。结束行号（包含）"}
                    },
                    "required": ["path"]
                }
            }),
            should_defer: true,
            is_read_only: true,
            is_concurrency_safe: true,
            is_enabled: true,
        },
        ToolDef {
            name: "ReadFileSkeleton",
            description: "提取文件结构骨架（类、函数签名及行号）",
            search_hint: "skeleton structure class function signature",
            schema: json!({
                "name": "ReadFileSkeleton",
                "description": "提取文件结构骨架（Skeleton）。快速扫描并返回文件的类、函数签名及其行号，结合 read_file 进行精确片段阅读。",
                "input_schema": {
                    "type": "object",
                    "properties": { "path": {"type": "string"} },
                    "required": ["path"]
                }
            }),
            should_defer: true,
            is_read_only: true,
            is_concurrency_safe: true,
            is_enabled: true,
        },
        ToolDef {
            name: "WriteFile",
            description: "写入普通文本文件内容；不得用于 .ipynb/Jupyter Notebook JSON",
            search_hint: "write file create new",
            schema: json!({
                "name": "WriteFile",
                "description": "写入普通文本文件内容。不要用于 .ipynb/Jupyter Notebook 或 notebook-shaped JSON；Notebook 必须使用 notebook_edit 进行 cell 级 replace/insert/delete。",
                "input_schema": {
                    "type": "object",
                    "properties": {
                        "path": {"type": "string"},
                        "content": {"type": "string"}
                    },
                    "required": ["path", "content"]
                }
            }),
            should_defer: true,
            is_read_only: false,
            is_concurrency_safe: false,
            is_enabled: true,
        },
        ToolDef {
            name: "EditFile",
            description: "基于搜索与替换修改普通文本；不得用于 .ipynb/Jupyter Notebook JSON",
            search_hint: "edit file replace search modify",
            schema: json!({
                "name": "EditFile",
                "description": "基于搜索与替换修改普通文本文件中的特定文本片段。不要用于 .ipynb/Jupyter Notebook 或 notebook-shaped JSON；Notebook 必须使用 notebook_edit 按 cell_id 修改，避免破坏 cells、metadata、outputs。",
                "input_schema": {
                    "type": "object",
                    "properties": {
                        "path": {"type": "string"},
                        "old_text": {"type": "string", "description": "要替换的确切旧文本内容"},
                        "new_text": {"type": "string", "description": "替换后的新文本内容"}
                    },
                    "required": ["path", "old_text", "new_text"]
                }
            }),
            should_defer: true,
            is_read_only: false,
            is_concurrency_safe: false,
            is_enabled: true,
        },
        ToolDef {
            name: "SearchRepo",
            description: "在指定目录下全局搜索包含关键词的文本",
            search_hint: "search find grep text pattern",
            schema: json!({
                "name": "SearchRepo",
                "description": "在指定目录下全局搜索包含特定关键词或正则表达式的文本内容。自动忽略编译产物和静态资源。",
                "input_schema": {
                    "type": "object",
                    "properties": {
                        "pattern": {"type": "string", "description": "要搜索的关键词或正则表达式"},
                        "dir": {"type": "string", "description": "要搜索的目录路径，默认搜索整个项目根目录"},
                        "regex": {"type": "boolean", "description": "是否将 pattern 作为正则表达式处理，默认 false"},
                        "case_insensitive": {"type": "boolean", "description": "是否忽略大小写，默认 false"}
                    },
                    "required": ["pattern"]
                }
            }),
            should_defer: true,
            is_read_only: true,
            is_concurrency_safe: true,
            is_enabled: true,
        },
        ToolDef {
            name: "ListDirectory",
            description: "列出指定目录下的所有文件和文件夹",
            search_hint: "list directory folder files ls",
            schema: json!({
                "name": "ListDirectory",
                "description": "列出指定目录下的所有文件和文件夹。",
                "input_schema": {
                    "type": "object",
                    "properties": {
                        "path": {"type": "string", "description": "目录路径"}
                    },
                    "required": ["path"]
                }
            }),
            should_defer: true,
            is_read_only: true,
            is_concurrency_safe: true,
            is_enabled: true,
        }
    }
}
