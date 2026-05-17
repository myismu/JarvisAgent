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
            category: "文件操作",
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
            category: "文件操作",
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
            category: "文件操作",
            schema: json!({
                "name": "WriteFile",
                "description": "写入普通文本文件内容。优先用于创建新文件；修改已有文件时应优先使用 EditFile，除非用户明确要求重写整个文件，或文件很小且已经完整读取过。大量修改应使用 ApplyPatch。不要用于 .ipynb/Jupyter Notebook 或 notebook-shaped JSON；Notebook 必须使用 notebook_edit 进行 cell 级 replace/insert/delete。",
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
            category: "文件操作",
            schema: json!({
                "name": "EditFile",
                "description": "基于搜索与替换修改普通文本文件中的特定文本片段。old_text 必须在文件中唯一匹配（包含足够多的上下文行，通常 3~5 行），否则会返回所有匹配位置让你修正。小范围单点修改优先使用此工具；多处修改或跨文件修改应使用 ApplyPatch。",
                "input_schema": {
                    "type": "object",
                    "properties": {
                        "path": {"type": "string"},
                        "old_text": {"type": "string", "description": "要替换的旧文本，必须包含足够上下文（3~5 行）确保在文件中唯一匹配"},
                        "new_text": {"type": "string", "description": "替换后的新文本"}
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
            name: "ApplyPatch",
            description: "事务式应用 unified diff 多文件补丁",
            search_hint: "apply patch diff multi file hunk dry run preview edit",
            category: "文件操作",
            schema: json!({
                "name": "ApplyPatch",
                "description": "事务式应用 unified diff / *** Begin Patch 格式补丁。适合多 hunk、多文件复杂修改；小范围单点替换优先使用 EditFile。支持 dry_run 预览；仍会执行 workspace 权限校验、Notebook 拦截、编码保留、TOCTOU 检查和快照记录。",
                "input_schema": {
                    "type": "object",
                    "properties": {
                        "patch": {"type": "string", "description": "unified diff 或 *** Begin Patch 格式补丁文本"},
                        "dry_run": {"type": "boolean", "description": "仅预检和预览，不写入文件。默认 false"}
                    },
                    "required": ["patch"]
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
            category: "搜索检索",
            schema: json!({
                "name": "SearchRepo",
                "description": "在指定目录下全局搜索包含特定关键词或正则表达式的文本内容。自动忽略编译产物和静态资源。",
                "input_schema": {
                    "type": "object",
                    "properties": {
                        "pattern": {"type": "string", "description": "要搜索的关键词或正则表达式"},
                        "dir": {"type": "string", "description": "要搜索的目录路径，默认搜索整个项目根目录"},
                        "regex": {"type": "boolean", "description": "是否将 pattern 作为正则表达式处理，默认 false"},
                        "case_insensitive": {"type": "boolean", "description": "是否忽略大小写，默认 false"},
                        "limit": {"type": "integer", "description": "最大返回匹配数，默认 50，最大 500"},
                        "context_lines": {"type": "integer", "description": "每个匹配前后返回的上下文行数，默认 0，最大 10"},
                        "include": {"type": "string", "description": "可选。include glob 过滤，例如 src/**/*.rs 或 *.{ts,tsx}"},
                        "exclude": {"type": "string", "description": "可选。exclude glob 过滤，例如 **/*.test.ts 或 dist/**"},
                        "type": {"type": "string", "description": "可选。文件类型过滤，例如 rust、typescript、vue、json"},
                        "ignore_dirs": {"type": ["string", "array"], "description": "可选。额外忽略目录，支持逗号/空格分隔字符串或字符串数组"}
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
            name: "FindSymbol",
            description: "查找符号定义候选位置",
            search_hint: "find symbol definition function class type component variable",
            category: "搜索检索",
            schema: json!({
                "name": "FindSymbol",
                "description": "查找符号定义候选位置。初版基于扩展名和轻量规则识别 function/class/type/component/variable，并返回文件、行号、签名和置信度。",
                "input_schema": {
                    "type": "object",
                    "properties": {
                        "symbol": {"type": "string", "description": "要查找的符号名"},
                        "dir": {"type": "string", "description": "可选。搜索目录，默认项目根目录"},
                        "kind": {"type": "string", "enum": ["function", "class", "type", "component", "variable", "any"], "description": "可选。符号类型过滤，默认 any"},
                        "limit": {"type": "integer", "description": "最大返回候选数，默认 50，最大 200"},
                        "include": {"type": "string", "description": "可选。include glob 过滤，例如 src/**/*.rs"},
                        "exclude": {"type": "string", "description": "可选。exclude glob 过滤"},
                        "type": {"type": "string", "description": "可选。文件类型过滤，例如 rust、typescript、vue"},
                        "ignore_dirs": {"type": ["string", "array"], "description": "可选。额外忽略目录"}
                    },
                    "required": ["symbol"]
                }
            }),
            should_defer: true,
            is_read_only: true,
            is_concurrency_safe: true,
            is_enabled: true,
        },
        ToolDef {
            name: "ReadSymbol",
            description: "读取指定符号所在代码块",
            search_hint: "read symbol definition block function class type",
            category: "搜索检索",
            schema: json!({
                "name": "ReadSymbol",
                "description": "读取指定文件中符号所在的完整代码块。初版基于括号或缩进推断范围，适合函数、类、类型、组件等常见代码块。",
                "input_schema": {
                    "type": "object",
                    "properties": {
                        "path": {"type": "string", "description": "符号所在文件路径"},
                        "symbol": {"type": "string", "description": "要读取的符号名"}
                    },
                    "required": ["path", "symbol"]
                }
            }),
            should_defer: true,
            is_read_only: true,
            is_concurrency_safe: true,
            is_enabled: true,
        },
        ToolDef {
            name: "FindReferences",
            description: "查找符号引用关系",
            search_hint: "find references usages import export reference symbol",
            category: "搜索检索",
            schema: json!({
                "name": "FindReferences",
                "description": "查找符号的引用关系。初版基于文本搜索，将结果区分为可能定义、可能引用和 import/export。",
                "input_schema": {
                    "type": "object",
                    "properties": {
                        "symbol": {"type": "string", "description": "要查找的符号名"},
                        "dir": {"type": "string", "description": "可选。搜索目录，默认项目根目录"},
                        "limit": {"type": "integer", "description": "最大返回结果数，默认 100，最大 500"},
                        "include": {"type": "string", "description": "可选。include glob 过滤"},
                        "exclude": {"type": "string", "description": "可选。exclude glob 过滤"},
                        "type": {"type": "string", "description": "可选。文件类型过滤"},
                        "ignore_dirs": {"type": ["string", "array"], "description": "可选。额外忽略目录"}
                    },
                    "required": ["symbol"]
                }
            }),
            should_defer: true,
            is_read_only: true,
            is_concurrency_safe: true,
            is_enabled: true,
        },
        ToolDef {
            name: "CodeSearch",
            description: "组合代码搜索并给出下一步读取建议",
            search_hint: "code search combined find files symbol text read next",
            category: "搜索检索",
            schema: json!({
                "name": "CodeSearch",
                "description": "组合代码搜索：先按 include/exclude/type/ignore_dirs 过滤文件，再同时查找符号定义和文本匹配，输出可直接用于 ReadFile/ReadSymbol 的下一步建议。",
                "input_schema": {
                    "type": "object",
                    "properties": {
                        "query": {"type": "string", "description": "要搜索的符号名或文本关键词"},
                        "dir": {"type": "string", "description": "可选。搜索目录，默认项目根目录"},
                        "include": {"type": "string", "description": "可选。include glob 过滤，例如 src/**/*"},
                        "exclude": {"type": "string", "description": "可选。exclude glob 过滤"},
                        "type": {"type": "string", "description": "可选。文件类型过滤，例如 rust、typescript、vue"},
                        "ignore_dirs": {"type": ["string", "array"], "description": "可选。额外忽略目录"},
                        "limit": {"type": "integer", "description": "最大返回结果数，默认 30，最大 100"}
                    },
                    "required": ["query"]
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
            category: "文件操作",
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
        },
        ToolDef {
            name: "DeleteFile",
            description: "删除指定文件",
            search_hint: "delete file remove rm",
            category: "文件操作",
            schema: json!({
                "name": "DeleteFile",
                "description": "删除指定文件。删除前会自动备份文件内容用于快照回滚。",
                "input_schema": {
                    "type": "object",
                    "properties": {
                        "path": {"type": "string", "description": "要删除的文件路径"}
                    },
                    "required": ["path"]
                }
            }),
            should_defer: true,
            is_read_only: false,
            is_concurrency_safe: false,
            is_enabled: true,
        },
        ToolDef {
            name: "RenameFile",
            description: "重命名或移动指定文件",
            search_hint: "rename move file mv ren",
            category: "文件操作",
            schema: json!({
                "name": "RenameFile",
                "description": "重命名或移动指定文件到新路径。",
                "input_schema": {
                    "type": "object",
                    "properties": {
                        "path": {"type": "string", "description": "要重命名的文件路径"},
                        "new_path": {"type": "string", "description": "新的文件路径"}
                    },
                    "required": ["path", "new_path"]
                }
            }),
            should_defer: true,
            is_read_only: false,
            is_concurrency_safe: false,
            is_enabled: true,
        }
    }
}
